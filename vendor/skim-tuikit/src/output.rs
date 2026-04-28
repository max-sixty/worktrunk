//! `Output` is the output stream that deals with ANSI Escape codes.
//! normally you should not use it directly.
//!
//! ```
//! use std::io;
//! use skim_tuikit::attr::Color;
//! use skim_tuikit::output::Output;
//!
//! let Ok(mut output) = Output::new(Box::new(io::stdout())) else { return; };
//! output.set_fg(Color::YELLOW);
//! output.write("YELLOW\n");
//! output.flush();
//!
//! ```

use std::io;
use std::io::Write;
use std::os::unix::io::AsRawFd;

use crate::attr::{Attr, Color, Effect};
use crate::sys::size::terminal_size;

use term::terminfo::TermInfo;
use term::terminfo::parm::{Param, Variables, expand};

// modeled after python-prompt-toolkit
// term info: https://ftp.netbsd.org/pub/NetBSD/NetBSD-release-7/src/share/terminfo/terminfo

const DEFAULT_BUFFER_SIZE: usize = 1024;

/// Output is an abstraction over the ANSI codes.
pub struct Output {
    /// A callable which returns the `Size` of the output terminal.
    buffer: Vec<u8>,
    stdout: Box<dyn WriteAndAsRawFdAndSend>,
    /// The terminal environment variable. (xterm, xterm-256color, linux, ...)
    terminfo: TermInfo,
}

pub trait WriteAndAsRawFdAndSend: Write + AsRawFd + Send {}

impl<T> WriteAndAsRawFdAndSend for T where T: Write + AsRawFd + Send {}

impl Output {
    pub fn new(stdout: Box<dyn WriteAndAsRawFdAndSend>) -> io::Result<Self> {
        Result::Ok(Self {
            buffer: Vec::with_capacity(DEFAULT_BUFFER_SIZE),
            stdout,
            terminfo: TermInfo::from_env()?,
        })
    }

    fn write_cap(&mut self, cmd: &str) {
        self.write_cap_with_params(cmd, &[])
    }

    fn write_cap_with_params(&mut self, cap: &str, params: &[Param]) {
        if let Some(cmd) = self.terminfo.strings.get(cap)
            && let Ok(s) = expand(cmd, params, &mut Variables::new())
        {
            self.buffer.extend(&s);
        }
    }

    /// Write text (Terminal escape sequences will be removed/escaped.)
    pub fn write(&mut self, data: &str) {
        self.buffer.extend(data.replace("\x1b", "?").as_bytes());
    }

    /// Write raw texts to the terminal.
    pub fn write_raw(&mut self, data: &[u8]) {
        self.buffer.extend_from_slice(data);
    }

    /// Return the encoding for this output, e.g. 'utf-8'.
    /// (This is used mainly to know which characters are supported by the
    /// output the data, so that the UI can provide alternatives, when
    /// required.)
    pub fn encoding(&self) -> &str {
        unimplemented!()
    }

    /// Set terminal title.
    pub fn set_title(&mut self, title: &str) {
        if self.terminfo.names.contains(&"linux".to_string())
            || self.terminfo.names.contains(&"eterm-color".to_string())
        {
            return;
        }

        let title = title.replace("\x1b", "").replace("\x07", "");
        self.write_raw(format!("\x1b]2;{title}\x07").as_bytes());
    }

    /// Clear title again. (or restore previous title.)
    pub fn clear_title(&mut self) {
        self.set_title("");
    }

    /// Write to output stream and flush.
    pub fn flush(&mut self) {
        let _ = self.stdout.write_all(&self.buffer);
        self.buffer.clear();
        let _ = self.stdout.flush();
    }

    /// Erases the screen with the background colour and moves the cursor to home.
    pub fn erase_screen(&mut self) {
        self.write_cap("clear");
    }

    /// Go to the alternate screen buffer. (For full screen applications).
    pub fn enter_alternate_screen(&mut self) {
        self.write_cap("smcup");
    }

    /// Leave the alternate screen buffer.
    pub fn quit_alternate_screen(&mut self) {
        self.write_cap("rmcup");
    }

    /// Enable mouse.
    pub fn enable_mouse_support(&mut self) {
        self.write_raw("\x1b[?1000h".as_bytes());

        // Enable urxvt Mouse mode. (For terminals that understand this.)
        self.write_raw("\x1b[?1015h".as_bytes());

        // Also enable Xterm SGR mouse mode. (For terminals that understand this.)
        self.write_raw("\x1b[?1006h".as_bytes());

        // Note: E.g. lxterminal understands 1000h, but not the urxvt or sgr extensions.
    }

    /// Disable mouse.
    pub fn disable_mouse_support(&mut self) {
        self.write_raw("\x1b[?1000l".as_bytes());
        self.write_raw("\x1b[?1015l".as_bytes());
        self.write_raw("\x1b[?1006l".as_bytes());
    }

    /// Erases from the current cursor position to the end of the current line.
    pub fn erase_end_of_line(&mut self) {
        self.write_cap("el");
    }

    /// Erases the screen from the current line down to the bottom of the screen.
    pub fn erase_down(&mut self) {
        self.write_cap("ed");
    }

    /// Reset color and styling attributes.
    pub fn reset_attributes(&mut self) {
        self.write_cap("sgr0");
    }

    /// Set current foreground color
    pub fn set_fg(&mut self, color: Color) {
        match color {
            Color::Default => {
                self.write_raw("\x1b[39m".as_bytes());
            }
            Color::AnsiValue(x) => {
                self.write_cap_with_params("setaf", &[Param::Number(x as i32)]);
            }
            Color::Rgb(r, g, b) => {
                self.write_raw(format!("\x1b[38;2;{r};{g};{b}m").as_bytes());
            }
        }
    }

    /// Set current background color
    pub fn set_bg(&mut self, color: Color) {
        match color {
            Color::Default => {
                self.write_raw("\x1b[49m".as_bytes());
            }
            Color::AnsiValue(x) => {
                self.write_cap_with_params("setab", &[Param::Number(x as i32)]);
            }
            Color::Rgb(r, g, b) => {
                self.write_raw(format!("\x1b[48;2;{r};{g};{b}m").as_bytes());
            }
        }
    }

    /// Set current effect (underline, bold, etc)
    pub fn set_effect(&mut self, effect: Effect) {
        if effect.contains(Effect::BOLD) {
            self.write_cap("bold");
        }
        if effect.contains(Effect::DIM) {
            self.write_cap("dim");
        }
        if effect.contains(Effect::UNDERLINE) {
            self.write_cap("smul");
        }
        if effect.contains(Effect::BLINK) {
            self.write_cap("blink");
        }
        if effect.contains(Effect::REVERSE) {
            self.write_cap("rev");
        }
    }

    /// Set new color and styling attributes.
    pub fn set_attribute(&mut self, attr: Attr) {
        self.set_fg(attr.fg);
        self.set_bg(attr.bg);
        self.set_effect(attr.effect);
    }

    /// Disable auto line wrapping.
    pub fn disable_autowrap(&mut self) {
        self.write_cap("rmam");
    }

    /// Enable auto line wrapping.
    pub fn enable_autowrap(&mut self) {
        self.write_cap("smam");
    }

    /// Move cursor position.
    pub fn cursor_goto(&mut self, row: usize, column: usize) {
        self.write_cap_with_params("cup", &[Param::Number(row as i32), Param::Number(column as i32)]);
    }

    /// Move cursor `amount` place up.
    pub fn cursor_up(&mut self, amount: usize) {
        match amount {
            0 => {}
            1 => self.write_cap("cuu1"),
            _ => self.write_cap_with_params("cuu", &[Param::Number(amount as i32)]),
        }
    }

    /// Move cursor `amount` place down.
    pub fn cursor_down(&mut self, amount: usize) {
        match amount {
            0 => {}
            1 => self.write_cap("cud1"),
            _ => self.write_cap_with_params("cud", &[Param::Number(amount as i32)]),
        }
    }

    /// Move cursor `amount` place forward.
    pub fn cursor_forward(&mut self, amount: usize) {
        match amount {
            0 => {}
            1 => self.write_cap("cuf1"),
            _ => self.write_cap_with_params("cuf", &[Param::Number(amount as i32)]),
        }
    }

    /// Move cursor `amount` place backward.
    pub fn cursor_backward(&mut self, amount: usize) {
        match amount {
            0 => {}
            1 => self.write_cap("cub1"),
            _ => self.write_cap_with_params("cub", &[Param::Number(amount as i32)]),
        }
    }

    /// Hide cursor.
    pub fn hide_cursor(&mut self) {
        self.write_cap("civis");
    }

    /// Show cursor.
    pub fn show_cursor(&mut self) {
        self.write_cap("cnorm");
    }

    /// Asks for a cursor position report (CPR). (VT100 only.)
    pub fn ask_for_cpr(&mut self) {
        self.write_raw("\x1b[6n".as_bytes());
        self.flush()
    }

    /// Sound bell.
    pub fn bell(&mut self) {
        self.write_cap("bel");
        self.flush()
    }

    /// get terminal size (width, height)
    pub fn terminal_size(&self) -> io::Result<(usize, usize)> {
        terminal_size(self.stdout.as_raw_fd())
    }

    /// For vt100/xterm etc.
    pub fn enable_bracketed_paste(&mut self) {
        self.write_raw("\x1b[?2004h".as_bytes());
    }

    /// For vt100/xterm etc.
    pub fn disable_bracketed_paste(&mut self) {
        self.write_raw("\x1b[?2004l".as_bytes());
    }

    ///  Execute the command
    pub fn execute(&mut self, cmd: Command) {
        match cmd {
            Command::PutChar(c) => self.write(c.to_string().as_str()),
            Command::Write(content) => self.write(&content),
            Command::SetTitle(title) => self.set_title(&title),
            Command::ClearTitle => self.clear_title(),
            Command::Flush => self.flush(),
            Command::EraseScreen => self.erase_screen(),
            Command::AlternateScreen(enable) => {
                if enable {
                    self.enter_alternate_screen()
                } else {
                    self.quit_alternate_screen()
                }
            }
            Command::MouseSupport(enable) => {
                if enable {
                    self.enable_mouse_support();
                } else {
                    self.disable_mouse_support();
                }
            }
            Command::EraseEndOfLine => self.erase_end_of_line(),
            Command::EraseDown => self.erase_down(),
            Command::ResetAttributes => self.reset_attributes(),
            Command::Fg(fg) => self.set_fg(fg),
            Command::Bg(bg) => self.set_bg(bg),
            Command::Effect(effect) => self.set_effect(effect),
            Command::SetAttribute(attr) => self.set_attribute(attr),
            Command::AutoWrap(enable) => {
                if enable {
                    self.enable_autowrap();
                } else {
                    self.disable_autowrap();
                }
            }
            Command::CursorGoto { row, col } => self.cursor_goto(row, col),
            Command::CursorUp(amount) => self.cursor_up(amount),
            Command::CursorDown(amount) => self.cursor_down(amount),
            Command::CursorLeft(amount) => self.cursor_backward(amount),
            Command::CursorRight(amount) => self.cursor_forward(amount),
            Command::CursorShow(show) => {
                if show {
                    self.show_cursor()
                } else {
                    self.hide_cursor()
                }
            }
            Command::BracketedPaste(enable) => {
                if enable {
                    self.enable_bracketed_paste()
                } else {
                    self.disable_bracketed_paste()
                }
            }
        }
    }
}

/// Instead of calling functions of `Output`, we could send commands.
#[derive(Debug, Clone)]
pub enum Command {
    /// Put a char to screen
    PutChar(char),
    /// Write content to screen (escape codes will be escaped)
    Write(String),
    /// Set the title of the terminal
    SetTitle(String),
    /// Clear the title of the terminal
    ClearTitle,
    /// Flush all the buffered contents
    Flush,
    /// Erase the entire screen
    EraseScreen,
    /// Enter(true)/Quit(false) the alternate screen mode
    AlternateScreen(bool),
    /// Enable(true)/Disable(false) mouse support
    MouseSupport(bool),
    /// Erase contents to the end of current line
    EraseEndOfLine,
    /// Erase contents till the bottom of the screen
    EraseDown,
    /// Reset attributes
    ResetAttributes,
    /// Set the foreground color
    Fg(Color),
    /// Set the background color
    Bg(Color),
    /// Set the effect(e.g. underline, dim, bold, ...)
    Effect(Effect),
    /// Set the fg, bg & effect.
    SetAttribute(Attr),
    /// Enable(true)/Disable(false) autowrap
    AutoWrap(bool),
    /// move the cursor to `(row, col)`
    CursorGoto { row: usize, col: usize },
    /// move cursor up `x` lines
    CursorUp(usize),
    /// move cursor down `x` lines
    CursorDown(usize),
    /// move cursor left `x` characters
    CursorLeft(usize),
    /// move cursor right `x` characters
    CursorRight(usize),
    /// Show(true)/Hide(false) cursor
    CursorShow(bool),
    /// Enable(true)/Disable(false) the bracketed paste mode
    BracketedPaste(bool),
}
