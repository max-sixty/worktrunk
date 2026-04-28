//! module to handle keystrokes
//!
//! ```no_run
//! use skim_tuikit::input::KeyBoard;
//! use skim_tuikit::key::Key;
//! use std::time::Duration;
//! let mut keyboard = KeyBoard::new_with_tty();
//! let key = keyboard.next_key();
//! ```

use std::fs::File;
use std::io::prelude::*;
use std::os::fd::AsFd as _;
use std::os::unix::io::AsRawFd;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nix::fcntl::{FcntlArg, OFlag, fcntl};

use crate::Result;
use crate::error::TuikitError;
use crate::key::Key::*;
use crate::key::{Key, MouseButton};
use crate::raw::get_tty;
use crate::spinlock::SpinLock;
use crate::sys::file::wait_until_ready;

pub trait ReadAndAsRawFd: Read + AsRawFd + Send {}

const KEY_WAIT: Duration = Duration::from_millis(10);
const DOUBLE_CLICK_DURATION: u128 = 300;

impl<T> ReadAndAsRawFd for T where T: Read + AsRawFd + Send {}

pub struct KeyBoard {
    file: File,
    sig_tx: Arc<SpinLock<File>>,
    sig_rx: File,
    // bytes will be poped from front, normally the buffer size will be small(< 10 bytes)
    byte_buf: Vec<u8>,

    raw_mouse: bool,
    next_key: Option<Result<Key>>,
    last_click: Key,
    last_click_time: SpinLock<Instant>,
}

// https://www.xfree86.org/4.8.0/ctlseqs.html
// http://man7.org/linux/man-pages/man4/console_codes.4.html
impl KeyBoard {
    pub fn new(file: File) -> Self {
        // the self-pipe trick for interrupt `select`
        let (rx, tx) = nix::unistd::pipe().expect("failed to set pipe");

        // set the signal pipe to non-blocking mode
        let flag = fcntl(rx.as_raw_fd(), FcntlArg::F_GETFL).expect("Get fcntl failed");
        let mut flag = OFlag::from_bits_truncate(flag);
        flag.insert(OFlag::O_NONBLOCK);
        let _ = fcntl(rx.as_raw_fd(), FcntlArg::F_SETFL(flag));

        // set file to non-blocking mode
        let flag = fcntl(file.as_raw_fd(), FcntlArg::F_GETFL).expect("Get fcntl failed");
        let mut flag = OFlag::from_bits_truncate(flag);
        flag.insert(OFlag::O_NONBLOCK);
        let _ = fcntl(file.as_raw_fd(), FcntlArg::F_SETFL(flag));

        KeyBoard {
            file,
            sig_tx: Arc::new(SpinLock::new(File::from(tx))),
            sig_rx: File::from(rx),
            byte_buf: Vec::new(),
            raw_mouse: false,
            next_key: None,
            last_click: Key::Null,
            last_click_time: SpinLock::new(Instant::now()),
        }
    }

    pub fn new_with_tty() -> Self {
        Self::new(get_tty().expect("KeyBoard::new_with_tty: failed to get tty"))
    }

    pub fn raw_mouse(mut self, raw_mouse: bool) -> Self {
        self.raw_mouse = raw_mouse;
        self
    }

    pub fn get_interrupt_handler(&self) -> KeyboardHandler {
        KeyboardHandler {
            handler: self.sig_tx.clone(),
        }
    }

    fn fetch_bytes(&mut self, timeout: Duration) -> Result<()> {
        let mut reader_buf = [0; 1];

        // clear interrupt signal
        while self.sig_rx.read(&mut reader_buf).is_ok() {}

        wait_until_ready(self.file.as_fd(), Some(self.sig_rx.as_fd()), timeout)?; // wait timeout

        self.read_unread_bytes();
        Ok(())
    }

    fn read_unread_bytes(&mut self) {
        let mut reader_buf = [0; 1];
        while self.file.read(&mut reader_buf).is_ok() {
            self.byte_buf.push(reader_buf[0]);
        }
    }

    #[allow(dead_code)]
    fn next_byte(&mut self) -> Result<u8> {
        self.next_byte_timeout(Duration::new(0, 0))
    }

    fn next_byte_timeout(&mut self, timeout: Duration) -> Result<u8> {
        trace!("next_byte_timeout: timeout: {timeout:?}");
        if self.byte_buf.is_empty() {
            self.fetch_bytes(timeout)?;
        }

        trace!("next_byte_timeout: after fetch, buf = {:?}", self.byte_buf);
        Ok(self.byte_buf.remove(0))
    }

    #[allow(dead_code)]
    fn next_char(&mut self) -> Result<char> {
        self.next_char_timeout(Duration::new(0, 0))
    }

    fn next_char_timeout(&mut self, timeout: Duration) -> Result<char> {
        trace!("next_char_timeout: timeout: {timeout:?}");
        if self.byte_buf.is_empty() {
            self.fetch_bytes(timeout)?;
        }

        trace!("get_chars: buf: {:?}", self.byte_buf);
        let bytes = std::mem::take(&mut self.byte_buf);
        match String::from_utf8(bytes) {
            Ok(string) => {
                let ret = string.chars().next().expect("failed to get next char from input");
                self.byte_buf.extend_from_slice(&string.as_bytes()[ret.len_utf8()..]);
                Ok(ret)
            }
            Err(error) => {
                let valid_up_to = error.utf8_error().valid_up_to();
                let bytes = error.into_bytes();
                let string = String::from_utf8_lossy(&bytes[..valid_up_to]);
                let ret = string.chars().next().expect("failed to get next char from input");
                self.byte_buf.extend_from_slice(&bytes[ret.len_utf8()..]);
                Ok(ret)
            }
        }
    }

    fn merge_wheel(&mut self, current_key: Result<Key>) -> (Result<Key>, Option<Result<Key>>) {
        match current_key {
            Ok(Key::MousePress(key @ MouseButton::WheelUp, row, col))
            | Ok(Key::MousePress(key @ MouseButton::WheelDown, row, col)) => {
                let mut count = 1;
                let mut o_next_key;
                loop {
                    o_next_key = self.try_next_raw_key();
                    match o_next_key {
                        Some(Ok(Key::MousePress(k, r, c))) if key == k && row == r && col == c => count += 1,
                        _ => break,
                    }
                }

                match key {
                    MouseButton::WheelUp => (Ok(Key::WheelUp(row, col, count)), o_next_key),
                    MouseButton::WheelDown => (Ok(Key::WheelDown(row, col, count)), o_next_key),
                    _ => unreachable!(),
                }
            }
            _ => (current_key, None),
        }
    }

    pub fn next_key(&mut self) -> Result<Key> {
        self.next_key_timeout(Duration::new(0, 0))
    }

    pub fn next_key_timeout(&mut self, timeout: Duration) -> Result<Key> {
        if self.raw_mouse {
            return self.next_raw_key_timeout(timeout);
        }

        let next_key = if self.next_key.is_some() {
            self.next_key.take().unwrap()
        } else {
            // fetch next key
            let next_key = self.next_raw_key_timeout(timeout);
            let (next_key, next_next_key) = self.merge_wheel(next_key);
            self.next_key = next_next_key;
            next_key
        };

        // parse double click
        match next_key {
            Ok(key @ MousePress(..)) => {
                if let MousePress(button, row, col) = key {
                    let ret = if key == self.last_click
                        && self.last_click_time.lock().elapsed().as_millis() < DOUBLE_CLICK_DURATION
                    {
                        DoubleClick(button, row, col)
                    } else {
                        self.last_click = key;
                        SingleClick(button, row, col)
                    };

                    *self.last_click_time.lock() = Instant::now();
                    Ok(ret)
                } else {
                    unreachable!();
                }
            }
            _ => next_key,
        }
    }

    #[allow(dead_code)]
    fn next_raw_key(&mut self) -> Result<Key> {
        self.next_raw_key_timeout(Duration::new(0, 0))
    }

    fn try_next_raw_key(&mut self) -> Option<Result<Key>> {
        match self.next_raw_key_timeout(KEY_WAIT) {
            Ok(key) => Some(Ok(key)),
            Err(TuikitError::Timeout(_)) => None,
            Err(error) => Some(Err(error)),
        }
    }

    /// Wait `timeout` until next key stroke
    fn next_raw_key_timeout(&mut self, timeout: Duration) -> Result<Key> {
        trace!("next_raw_key_timeout: {timeout:?}");
        let ch = self.next_char_timeout(timeout)?;
        match ch {
            '\u{00}' => Ok(Ctrl(' ')),
            '\u{01}' => Ok(Ctrl('a')),
            '\u{02}' => Ok(Ctrl('b')),
            '\u{03}' => Ok(Ctrl('c')),
            '\u{04}' => Ok(Ctrl('d')),
            '\u{05}' => Ok(Ctrl('e')),
            '\u{06}' => Ok(Ctrl('f')),
            '\u{07}' => Ok(Ctrl('g')),
            '\u{08}' => Ok(Ctrl('h')),
            '\u{09}' => Ok(Tab),
            '\u{0A}' => Ok(Ctrl('j')),
            '\u{0B}' => Ok(Ctrl('k')),
            '\u{0C}' => Ok(Ctrl('l')),
            '\u{0D}' => Ok(Enter),
            '\u{0E}' => Ok(Ctrl('n')),
            '\u{0F}' => Ok(Ctrl('o')),
            '\u{10}' => Ok(Ctrl('p')),
            '\u{11}' => Ok(Ctrl('q')),
            '\u{12}' => Ok(Ctrl('r')),
            '\u{13}' => Ok(Ctrl('s')),
            '\u{14}' => Ok(Ctrl('t')),
            '\u{15}' => Ok(Ctrl('u')),
            '\u{16}' => Ok(Ctrl('v')),
            '\u{17}' => Ok(Ctrl('w')),
            '\u{18}' => Ok(Ctrl('x')),
            '\u{19}' => Ok(Ctrl('y')),
            '\u{1A}' => Ok(Ctrl('z')),
            '\u{1B}' => self.escape_sequence(),
            '\u{7F}' => Ok(Backspace),
            ch => Ok(Char(ch)),
        }
    }

    fn escape_sequence(&mut self) -> Result<Key> {
        let seq1 = self.next_char_timeout(KEY_WAIT).unwrap_or('\u{1B}');
        match seq1 {
            '[' => self.escape_csi(),
            'O' => self.escape_o(),
            _ => self.parse_alt(seq1),
        }
    }

    fn parse_alt(&mut self, ch: char) -> Result<Key> {
        match ch {
            '\u{1B}' => {
                match self.next_byte_timeout(KEY_WAIT) {
                    Ok(b'[') => {}
                    Ok(c) => {
                        return Err(TuikitError::UnknownSequence(format!("ESC ESC {c}")));
                    }
                    Err(_) => return Ok(ESC),
                }

                match self.escape_csi() {
                    Ok(Up) => Ok(AltUp),
                    Ok(Down) => Ok(AltDown),
                    Ok(Left) => Ok(AltLeft),
                    Ok(Right) => Ok(AltRight),
                    Ok(PageUp) => Ok(AltPageUp),
                    Ok(PageDown) => Ok(AltPageDown),
                    _ => Err(TuikitError::UnknownSequence("ESC ESC [ ...".to_string())),
                }
            }
            '\u{00}' => Ok(CtrlAlt(' ')),
            '\u{01}' => Ok(CtrlAlt('a')),
            '\u{02}' => Ok(CtrlAlt('b')),
            '\u{03}' => Ok(CtrlAlt('c')),
            '\u{04}' => Ok(CtrlAlt('d')),
            '\u{05}' => Ok(CtrlAlt('e')),
            '\u{06}' => Ok(CtrlAlt('f')),
            '\u{07}' => Ok(CtrlAlt('g')),
            '\u{08}' => Ok(CtrlAlt('h')),
            '\u{09}' => Ok(AltTab),
            '\u{0A}' => Ok(CtrlAlt('j')),
            '\u{0B}' => Ok(CtrlAlt('k')),
            '\u{0C}' => Ok(CtrlAlt('l')),
            '\u{0D}' => Ok(AltEnter),
            '\u{0E}' => Ok(CtrlAlt('n')),
            '\u{0F}' => Ok(CtrlAlt('o')),
            '\u{10}' => Ok(CtrlAlt('p')),
            '\u{11}' => Ok(CtrlAlt('q')),
            '\u{12}' => Ok(CtrlAlt('r')),
            '\u{13}' => Ok(CtrlAlt('s')),
            '\u{14}' => Ok(CtrlAlt('t')),
            '\u{15}' => Ok(CtrlAlt('u')),
            '\u{16}' => Ok(CtrlAlt('v')),
            '\u{17}' => Ok(CtrlAlt('w')),
            '\u{18}' => Ok(CtrlAlt('x')),
            '\u{19}' => Ok(AltBackTab),
            '\u{1A}' => Ok(CtrlAlt('z')),
            '\u{7F}' => Ok(AltBackspace),
            ch => Ok(Alt(ch)),
        }
    }

    fn escape_csi(&mut self) -> Result<Key> {
        let cursor_pos = self.parse_cursor_report();
        if cursor_pos.is_ok() {
            return cursor_pos;
        }

        let seq2 = self.next_byte_timeout(KEY_WAIT)?;
        match seq2 {
            b'0' | b'9' => Err(TuikitError::UnknownSequence(format!("ESC [ {seq2:x?}"))),
            b'1'..=b'8' => self.extended_escape(seq2),
            b'[' => {
                // Linux Console ESC [ [ _
                let seq3 = self.next_byte_timeout(KEY_WAIT)?;
                match seq3 {
                    b'A' => Ok(F(1)),
                    b'B' => Ok(F(2)),
                    b'C' => Ok(F(3)),
                    b'D' => Ok(F(4)),
                    b'E' => Ok(F(5)),
                    _ => Err(TuikitError::UnknownSequence(format!("ESC [ [ {seq3:x?}"))),
                }
            }
            b'A' => Ok(Up),    // kcuu1
            b'B' => Ok(Down),  // kcud1
            b'C' => Ok(Right), // kcuf1
            b'D' => Ok(Left),  // kcub1
            b'H' => Ok(Home),  // khome
            b'F' => Ok(End),
            b'Z' => Ok(BackTab),
            b'M' => {
                // X10 emulation mouse encoding: ESC [ M Bxy (6 characters only)
                let cb = self.next_byte_timeout(KEY_WAIT)?;
                // (1, 1) are the coords for upper left.
                let cx = self.next_byte_timeout(KEY_WAIT)?.saturating_sub(32) as u16 - 1; // 0 based
                let cy = self.next_byte_timeout(KEY_WAIT)?.saturating_sub(32) as u16 - 1; // 0 based
                match cb & 0b11 {
                    0 => {
                        if cb & 0x40 != 0 {
                            Ok(MousePress(MouseButton::WheelUp, cy, cx))
                        } else {
                            Ok(MousePress(MouseButton::Left, cy, cx))
                        }
                    }
                    1 => {
                        if cb & 0x40 != 0 {
                            Ok(MousePress(MouseButton::WheelDown, cy, cx))
                        } else {
                            Ok(MousePress(MouseButton::Middle, cy, cx))
                        }
                    }
                    2 => Ok(MousePress(MouseButton::Right, cy, cx)),
                    3 => Ok(MouseRelease(cy, cx)),
                    _ => Err(TuikitError::UnknownSequence(format!("ESC M {cb:?}{cx:?}{cy:?}"))),
                }
            }
            b'<' => {
                // xterm mouse encoding:
                // ESC [ < Cb ; Cx ; Cy ; (M or m)
                self.read_unread_bytes();
                if !self.byte_buf.contains(&b'm') && !self.byte_buf.contains(&b'M') {
                    return Err(TuikitError::UnknownSequence(
                        "ESC [ < (not ending with m/M)".to_string(),
                    ));
                }

                let mut str_buf = String::new();
                let mut c = self.next_char_timeout(KEY_WAIT)?;
                while c != 'm' && c != 'M' {
                    str_buf.push(c);
                    c = self.next_char_timeout(KEY_WAIT)?;
                }
                let nums = &mut str_buf.split(';');

                let cb = nums.next().unwrap().parse::<u16>().unwrap();
                let cx = nums.next().unwrap().parse::<u16>().unwrap() - 1; // 0 based
                let cy = nums.next().unwrap().parse::<u16>().unwrap() - 1; // 0 based

                match cb {
                    0..=2 | 64..=65 => {
                        let button = match cb {
                            0 => MouseButton::Left,
                            1 => MouseButton::Middle,
                            2 => MouseButton::Right,
                            64 => MouseButton::WheelUp,
                            65 => MouseButton::WheelDown,
                            _ => {
                                return Err(TuikitError::UnknownSequence(format!("ESC [ < {str_buf} {c}")));
                            }
                        };

                        match c {
                            'M' => Ok(MousePress(button, cy, cx)),
                            'm' => Ok(MouseRelease(cy, cx)),
                            _ => Err(TuikitError::UnknownSequence(format!("ESC [ < {str_buf} {c}"))),
                        }
                    }
                    32 => Ok(MouseHold(cy, cx)),
                    _ => Err(TuikitError::UnknownSequence(format!("ESC [ < {str_buf} {c}"))),
                }
            }
            _ => Err(TuikitError::UnknownSequence(format!("ESC [ {seq2:?}"))),
        }
    }

    // Cursor position report (CPR): Answer is ESC [ y ; x R, where x,y is the cursor location.
    fn parse_cursor_report(&mut self) -> Result<Key> {
        self.read_unread_bytes();
        let pos_semi = self.byte_buf.iter().position(|&b| b == b';');
        let pos_r = self.byte_buf.iter().position(|&b| b == b'R');

        if let Some(pos_semi) = pos_semi
            && let Some(pos_r) = pos_r
        {
            if pos_r > pos_semi {
                let remain = self.byte_buf.split_off(pos_r + 1);
                let mut col_str = self.byte_buf.split_off(pos_semi + 1);
                let mut row_str = std::mem::replace(&mut self.byte_buf, remain);

                row_str.pop(); // remove the ';' character
                col_str.pop(); // remove the 'R' character
                let row = String::from_utf8(row_str)?;
                let col = String::from_utf8(col_str)?;

                let row_num = row.parse::<u16>()?;
                let col_num = col.parse::<u16>()?;

                return Ok(CursorPos(row_num - 1, col_num - 1));
            }
        }

        Err(TuikitError::NoCursorReportResponse)
    }

    fn extended_escape(&mut self, seq2: u8) -> Result<Key> {
        let seq3 = self.next_byte_timeout(KEY_WAIT)?;
        if seq3 == b'~' {
            match seq2 {
                b'1' | b'7' => Ok(Home), // tmux, xrvt
                b'2' => Ok(Insert),
                b'3' => Ok(Delete),     // kdch1
                b'4' | b'8' => Ok(End), // tmux, xrvt
                b'5' => Ok(PageUp),     // kpp
                b'6' => Ok(PageDown),   // knp
                _ => Err(TuikitError::UnknownSequence(format!("ESC [ {seq2} ~"))),
            }
        } else if seq3.is_ascii_digit() {
            let mut str_buf = String::new();
            str_buf.push(seq2 as char);
            str_buf.push(seq3 as char);

            let mut seq_last = self.next_byte_timeout(KEY_WAIT)?;
            while seq_last != b'M' && seq_last != b'~' {
                str_buf.push(seq_last as char);
                seq_last = self.next_byte_timeout(KEY_WAIT)?;
            }

            match seq_last {
                b'M' => {
                    // rxvt mouse encoding:
                    // ESC [ Cb ; Cx ; Cy ; M
                    let mut nums = str_buf.split(';');

                    let cb = nums.next().unwrap().parse::<u16>().unwrap();
                    let cx = nums.next().unwrap().parse::<u16>().unwrap() - 1; // 0 based
                    let cy = nums.next().unwrap().parse::<u16>().unwrap() - 1; // 0 based

                    match cb {
                        32 => Ok(MousePress(MouseButton::Left, cy, cx)),
                        33 => Ok(MousePress(MouseButton::Middle, cy, cx)),
                        34 => Ok(MousePress(MouseButton::Right, cy, cx)),
                        35 => Ok(MouseRelease(cy, cx)),
                        64 => Ok(MouseHold(cy, cx)),
                        96 | 97 => Ok(MousePress(MouseButton::WheelUp, cy, cx)),
                        _ => Err(TuikitError::UnknownSequence(format!("ESC [ {str_buf} M"))),
                    }
                }
                b'~' => {
                    let num: u8 = str_buf.parse().unwrap();
                    match num {
                        v @ 11..=15 => Ok(F(v - 10)),
                        v @ 17..=21 => Ok(F(v - 11)),
                        v @ 23..=24 => Ok(F(v - 12)),
                        200 => Ok(BracketedPasteStart),
                        201 => Ok(BracketedPasteEnd),
                        _ => Err(TuikitError::UnknownSequence(format!("ESC [ {str_buf} ~"))),
                    }
                }
                _ => unreachable!(),
            }
        } else if seq3 == b';' {
            let seq4 = self.next_byte_timeout(KEY_WAIT)?;
            if seq4.is_ascii_digit() {
                let seq5 = self.next_byte_timeout(KEY_WAIT)?;
                if seq2 == b'1' {
                    match (seq4, seq5) {
                        (b'5', b'A') => Ok(CtrlUp),
                        (b'5', b'B') => Ok(CtrlDown),
                        (b'5', b'C') => Ok(CtrlRight),
                        (b'5', b'D') => Ok(CtrlLeft),
                        (b'4', b'A') => Ok(AltShiftUp),
                        (b'4', b'B') => Ok(AltShiftDown),
                        (b'4', b'C') => Ok(AltShiftRight),
                        (b'4', b'D') => Ok(AltShiftLeft),
                        (b'3', b'H') => Ok(AltHome),
                        (b'3', b'F') => Ok(AltEnd),
                        (b'2', b'A') => Ok(ShiftUp),
                        (b'2', b'B') => Ok(ShiftDown),
                        (b'2', b'C') => Ok(ShiftRight),
                        (b'2', b'D') => Ok(ShiftLeft),
                        _ => Err(TuikitError::UnknownSequence(format!("ESC [ 1 ; {seq4:x?} {seq5:x?}"))),
                    }
                } else {
                    Err(TuikitError::UnknownSequence(format!(
                        "ESC [ {seq2:x?} ; {seq4:x?} {seq5:x?}"
                    )))
                }
            } else {
                Err(TuikitError::UnknownSequence(format!("ESC [ {seq2:x?} ; {seq4:x?}")))
            }
        } else {
            match (seq2, seq3) {
                (b'5', b'A') => Ok(CtrlUp),
                (b'5', b'B') => Ok(CtrlDown),
                (b'5', b'C') => Ok(CtrlRight),
                (b'5', b'D') => Ok(CtrlLeft),
                _ => Err(TuikitError::UnknownSequence(format!("ESC [ {seq2:x?} {seq3:x?}"))),
            }
        }
    }

    // SSS3
    fn escape_o(&mut self) -> Result<Key> {
        let seq2 = self.next_byte_timeout(KEY_WAIT)?;
        match seq2 {
            b'A' => Ok(Up),    // kcuu1
            b'B' => Ok(Down),  // kcud1
            b'C' => Ok(Right), // kcuf1
            b'D' => Ok(Left),  // kcub1
            b'F' => Ok(End),   // kend
            b'H' => Ok(Home),  // khome
            b'P' => Ok(F(1)),  // kf1
            b'Q' => Ok(F(2)),  // kf2
            b'R' => Ok(F(3)),  // kf3
            b'S' => Ok(F(4)),  // kf4
            b'a' => Ok(CtrlUp),
            b'b' => Ok(CtrlDown),
            b'c' => Ok(CtrlRight), // rxvt
            b'd' => Ok(CtrlLeft),  // rxvt
            _ => Err(TuikitError::UnknownSequence(format!("ESC O {seq2:x?}"))),
        }
    }
}

pub struct KeyboardHandler {
    handler: Arc<SpinLock<File>>,
}

impl KeyboardHandler {
    pub fn interrupt(&self) {
        let mut handler = self.handler.lock();
        let _ = handler.write_all(b"x");
        let _ = handler.flush();
    }
}
