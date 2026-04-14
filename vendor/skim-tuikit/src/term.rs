//! Term is a thread-safe "terminal".
//!
//! It allows you to:
//! - Listen to key stroke events
//! - Output contents to the terminal
//!
//! ```no_run
//! use skim_tuikit::prelude::*;
//!
//! let term = Term::<()>::new().unwrap();
//!
//! while let Ok(ev) = term.poll_event() {
//!     if let Event::Key(Key::Char('q')) = ev {
//!         break;
//!     }
//!
//!     term.print(0, 0, format!("got event: {:?}", ev).as_str());
//!     term.present();
//! }
//! ```
//!
//! Term is modeled after [termbox](https://github.com/nsf/termbox). The main idea is viewing
//! terminals as a table of fixed-size cells and input being a stream of structured messages

use std::cmp::{max, min};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::Duration;

use crate::Result;
use crate::attr::Attr;
use crate::canvas::Canvas;
use crate::cell::Cell;
use crate::draw::Draw;
use crate::error::TuikitError;
use crate::event::Event;
use crate::input::{KeyBoard, KeyboardHandler};
use crate::key::Key;
use crate::output::Command;
use crate::output::Output;
use crate::raw::{IntoRawMode, get_tty};
use crate::screen::Screen;
use crate::spinlock::SpinLock;
use crate::sys::signal::{initialize_signals, notify_on_sigwinch, unregister_sigwinch};

const MIN_HEIGHT: usize = 1;
const WAIT_TIMEOUT: Duration = Duration::from_millis(300);
const POLLING_TIMEOUT: Duration = Duration::from_millis(10);

#[derive(Debug, Copy, Clone)]
pub enum TermHeight {
    Fixed(usize),
    Percent(usize),
}

pub struct Term<UserEvent: Send + 'static = ()> {
    components_to_stop: Arc<AtomicUsize>,
    keyboard_handler: SpinLock<Option<KeyboardHandler>>,
    resize_signal_id: Arc<AtomicUsize>,
    term_lock: SpinLock<TermLock>,
    event_rx: SpinLock<Receiver<Event<UserEvent>>>,
    event_tx: Arc<SpinLock<Sender<Event<UserEvent>>>>,
    raw_mouse: bool, // to produce raw mouse event or the parsed event(e.g. DoubleClick)
}

pub struct TermOptions {
    max_height: TermHeight,
    min_height: TermHeight,
    height: TermHeight,
    clear_on_exit: bool,
    clear_on_start: bool,
    mouse_enabled: bool,
    raw_mouse: bool,
    hold: bool, // to start term or not on creation
    disable_alternate_screen: bool,
}

impl Default for TermOptions {
    fn default() -> Self {
        Self {
            max_height: TermHeight::Percent(100),
            min_height: TermHeight::Fixed(3),
            height: TermHeight::Percent(100),
            clear_on_exit: true,
            clear_on_start: true,
            mouse_enabled: false,
            raw_mouse: false,
            hold: false,
            disable_alternate_screen: false,
        }
    }
}

// Builder
impl TermOptions {
    pub fn max_height(mut self, max_height: TermHeight) -> Self {
        self.max_height = max_height;
        self
    }

    pub fn min_height(mut self, min_height: TermHeight) -> Self {
        self.min_height = min_height;
        self
    }
    pub fn height(mut self, height: TermHeight) -> Self {
        self.height = height;
        self
    }
    pub fn clear_on_exit(mut self, clear: bool) -> Self {
        self.clear_on_exit = clear;
        self
    }
    pub fn clear_on_start(mut self, clear: bool) -> Self {
        self.clear_on_start = clear;
        self
    }
    pub fn mouse_enabled(mut self, enabled: bool) -> Self {
        self.mouse_enabled = enabled;
        self
    }
    pub fn raw_mouse(mut self, enabled: bool) -> Self {
        self.raw_mouse = enabled;
        self
    }
    pub fn hold(mut self, hold: bool) -> Self {
        self.hold = hold;
        self
    }
    pub fn disable_alternate_screen(mut self, disable_alternate_screen: bool) -> Self {
        self.disable_alternate_screen = disable_alternate_screen;
        self
    }
}

impl<UserEvent: Send + 'static> Term<UserEvent> {
    /// Create a Term with height specified.
    ///
    /// Internally if the calculated height would fill the whole screen, `Alternate Screen` will
    /// be enabled, otherwise only part of the screen will be used.
    ///
    /// If the preferred height is larger than the current screen, whole screen is used.
    ///
    /// ```no_run
    /// use skim_tuikit::term::{Term, TermHeight};
    ///
    /// let term: Term<()> = Term::with_height(TermHeight::Percent(30)).unwrap(); // 30% of the terminal height
    /// let term: Term<()> = Term::with_height(TermHeight::Fixed(20)).unwrap(); // fixed 20 lines
    /// ```
    pub fn with_height(height: TermHeight) -> Result<Term<UserEvent>> {
        Term::with_options(TermOptions::default().height(height))
    }

    /// Create a Term (with 100% height)
    ///
    /// ```no_run
    /// use skim_tuikit::term::{Term, TermHeight};
    ///
    /// let term: Term<()> = Term::new().unwrap();
    /// let term: Term<()> = Term::with_height(TermHeight::Percent(100)).unwrap();
    /// ```
    pub fn new() -> Result<Term<UserEvent>> {
        Term::with_options(TermOptions::default())
    }

    /// Create a Term with custom options
    ///
    /// ```no_run
    /// use skim_tuikit::term::{Term, TermHeight, TermOptions};
    ///
    /// let term: Term<()> = Term::with_options(TermOptions::default().height(TermHeight::Percent(100))).unwrap();
    /// ```
    pub fn with_options(options: TermOptions) -> Result<Term<UserEvent>> {
        initialize_signals();

        let (event_tx, event_rx) = channel();
        let raw_mouse = options.raw_mouse;
        let ret = Term {
            components_to_stop: Arc::new(AtomicUsize::new(0)),
            keyboard_handler: SpinLock::new(None),
            resize_signal_id: Arc::new(AtomicUsize::new(0)),
            term_lock: SpinLock::new(TermLock::with_options(&options)),
            event_tx: Arc::new(SpinLock::new(event_tx)),
            event_rx: SpinLock::new(event_rx),
            raw_mouse,
        };
        if options.hold {
            Ok(ret)
        } else {
            ret.restart().map(|_| ret)
        }
    }

    fn ensure_not_stopped(&self) -> Result<()> {
        if self.components_to_stop.load(Ordering::SeqCst) == 2 {
            Ok(())
        } else {
            Err(TuikitError::TerminalNotStarted)
        }
    }

    fn get_cursor_pos(&self, keyboard: &mut KeyBoard, output: &mut Output) -> Result<(usize, usize)> {
        output.ask_for_cpr();

        if let Ok(Key::CursorPos(row, col)) = keyboard.next_key_timeout(WAIT_TIMEOUT) {
            return Ok((row as usize, col as usize));
        }

        Ok((0, 0))
    }

    /// restart the terminal if it had been stopped
    pub fn restart(&self) -> Result<()> {
        let mut termlock = self.term_lock.lock();
        if self.components_to_stop.load(Ordering::SeqCst) == 2 {
            return Ok(());
        }

        let ttyout = get_tty()?.into_raw_mode()?;
        let mut output = Output::new(Box::new(ttyout))?;
        let mut keyboard = KeyBoard::new_with_tty().raw_mouse(self.raw_mouse);
        self.keyboard_handler.lock().replace(keyboard.get_interrupt_handler());
        let cursor_pos = self.get_cursor_pos(&mut keyboard, &mut output)?;
        termlock.restart(output, cursor_pos)?;

        // start two listener
        self.start_key_listener(keyboard);
        self.start_size_change_listener();

        // wait for components to start
        while self.components_to_stop.load(Ordering::SeqCst) < 2 {
            debug!(
                "restart: components: {}",
                self.components_to_stop.load(Ordering::SeqCst)
            );
            thread::sleep(POLLING_TIMEOUT);
        }

        let event_tx = self.event_tx.lock();
        let _ = event_tx.send(Event::Restarted);

        Ok(())
    }

    /// Pause the Term
    ///
    /// This function will cause the Term to give away the control to the terminal(such as listening
    /// to the key strokes). After the Term was "paused", `poll_event` will block indefinitely and
    /// recover after the Term was `restart`ed.
    pub fn pause(&self) -> Result<()> {
        self.pause_internal(false)
    }

    fn pause_internal(&self, exiting: bool) -> Result<()> {
        debug!("pause");
        let mut termlock = self.term_lock.lock();

        if self.components_to_stop.load(Ordering::SeqCst) == 0 {
            return Ok(());
        }

        // wait for the components to stop
        // i.e. key_listener & size_change_listener
        if let Some(h) = self.keyboard_handler.lock().take() {
            h.interrupt()
        }
        unregister_sigwinch(self.resize_signal_id.load(Ordering::Relaxed)).map(|tx| tx.send(()));

        termlock.pause(exiting)?;

        // wait for the components to stop
        while self.components_to_stop.load(Ordering::SeqCst) > 0 {
            debug!("pause: components: {}", self.components_to_stop.load(Ordering::SeqCst));
            thread::sleep(POLLING_TIMEOUT);
        }

        Ok(())
    }

    fn start_key_listener(&self, mut keyboard: KeyBoard) {
        let event_tx_clone = self.event_tx.clone();
        let components_to_stop = self.components_to_stop.clone();
        thread::spawn(move || {
            components_to_stop.fetch_add(1, Ordering::SeqCst);
            debug!("key listener start");
            loop {
                let next_key = keyboard.next_key();
                trace!("next key: {next_key:?}");
                match next_key {
                    Ok(key) => {
                        let event_tx = event_tx_clone.lock();
                        let _ = event_tx.send(Event::Key(key));
                    }
                    Err(TuikitError::Interrupted) => break,
                    _ => {} // ignored
                }
            }
            components_to_stop.fetch_sub(1, Ordering::SeqCst);
            debug!("key listener stop");
        });
    }

    fn start_size_change_listener(&self) {
        let event_tx_clone = self.event_tx.clone();
        let resize_signal_id = self.resize_signal_id.clone();
        let components_to_stop = self.components_to_stop.clone();

        thread::spawn(move || {
            let (id, sigwinch_rx) = notify_on_sigwinch();
            resize_signal_id.store(id, Ordering::Relaxed);

            components_to_stop.fetch_add(1, Ordering::SeqCst);
            debug!("size change listener started");
            loop {
                if sigwinch_rx.recv().is_ok() {
                    let event_tx = event_tx_clone.lock();
                    let _ = event_tx.send(Event::Resize { width: 0, height: 0 });
                } else {
                    break;
                }
            }
            components_to_stop.fetch_sub(1, Ordering::SeqCst);
            debug!("size change listener stop");
        });
    }

    fn filter_event(&self, event: Event<UserEvent>) -> Event<UserEvent> {
        match event {
            Event::Resize { .. } => {
                {
                    let mut termlock = self.term_lock.lock();
                    let _ = termlock.on_resize();
                }
                let (width, height) = self.term_size().unwrap_or((0, 0));
                Event::Resize { width, height }
            }
            Event::Key(Key::MousePress(button, row, col)) => {
                // adjust mouse event position
                let cursor_row = self.term_lock.lock().get_term_start_row() as u16;
                if row < cursor_row {
                    Event::__Nonexhaustive
                } else {
                    Event::Key(Key::MousePress(button, row - cursor_row, col))
                }
            }
            Event::Key(Key::MouseRelease(row, col)) => {
                // adjust mouse event position
                let cursor_row = self.term_lock.lock().get_term_start_row() as u16;
                if row < cursor_row {
                    Event::__Nonexhaustive
                } else {
                    Event::Key(Key::MouseRelease(row - cursor_row, col))
                }
            }
            Event::Key(Key::MouseHold(row, col)) => {
                // adjust mouse event position
                let cursor_row = self.term_lock.lock().get_term_start_row() as u16;
                if row < cursor_row {
                    Event::__Nonexhaustive
                } else {
                    Event::Key(Key::MouseHold(row - cursor_row, col))
                }
            }
            Event::Key(Key::SingleClick(button, row, col)) => {
                let cursor_row = self.term_lock.lock().get_term_start_row() as u16;
                if row < cursor_row {
                    Event::__Nonexhaustive
                } else {
                    Event::Key(Key::SingleClick(button, row - cursor_row, col))
                }
            }
            Event::Key(Key::DoubleClick(button, row, col)) => {
                let cursor_row = self.term_lock.lock().get_term_start_row() as u16;
                if row < cursor_row {
                    Event::__Nonexhaustive
                } else {
                    Event::Key(Key::DoubleClick(button, row - cursor_row, col))
                }
            }
            Event::Key(Key::WheelUp(row, col, num)) => {
                let cursor_row = self.term_lock.lock().get_term_start_row() as u16;
                if row < cursor_row {
                    Event::__Nonexhaustive
                } else {
                    Event::Key(Key::WheelUp(row - cursor_row, col, num))
                }
            }
            Event::Key(Key::WheelDown(row, col, num)) => {
                let cursor_row = self.term_lock.lock().get_term_start_row() as u16;
                if row < cursor_row {
                    Event::__Nonexhaustive
                } else {
                    Event::Key(Key::WheelDown(row - cursor_row, col, num))
                }
            }
            ev => ev,
        }
    }

    /// Wait an event up to `timeout` and return it
    pub fn peek_event(&self, timeout: Duration) -> Result<Event<UserEvent>> {
        let event_rx = self.event_rx.lock();
        event_rx
            .recv_timeout(timeout)
            .map(|ev| self.filter_event(ev))
            .map_err(|_| TuikitError::Timeout(timeout))
    }

    /// Wait for an event indefinitely and return it
    pub fn poll_event(&self) -> Result<Event<UserEvent>> {
        let event_rx = self.event_rx.lock();
        event_rx
            .recv()
            .map(|ev| self.filter_event(ev))
            .map_err(TuikitError::ChannelReceiveError)
    }

    /// An interface to inject event to the terminal's event queue
    pub fn send_event(&self, event: Event<UserEvent>) -> Result<()> {
        let event_tx = self.event_tx.lock();
        event_tx
            .send(event)
            .map_err(|err| TuikitError::SendEventError(err.to_string()))
    }

    /// Sync internal buffer with terminal
    pub fn present(&self) -> Result<()> {
        self.ensure_not_stopped()?;
        let mut termlock = self.term_lock.lock();
        termlock.present()
    }

    /// Return the printable size(width, height) of the term
    pub fn term_size(&self) -> Result<(usize, usize)> {
        self.ensure_not_stopped()?;
        let termlock = self.term_lock.lock();
        termlock.term_size()
    }

    /// Clear internal buffer
    pub fn clear(&self) -> Result<()> {
        self.ensure_not_stopped()?;
        let mut termlock = self.term_lock.lock();
        termlock.clear()
    }

    /// Change a cell of position `(row, col)` to `cell`
    pub fn put_cell(&self, row: usize, col: usize, cell: Cell) -> Result<usize> {
        self.ensure_not_stopped()?;
        let mut termlock = self.term_lock.lock();
        termlock.put_cell(row, col, cell)
    }

    /// Print `content` starting with position `(row, col)`
    pub fn print(&self, row: usize, col: usize, content: &str) -> Result<usize> {
        self.print_with_attr(row, col, content, Attr::default())
    }

    /// print `content` starting with position `(row, col)` with `attr`
    pub fn print_with_attr(&self, row: usize, col: usize, content: &str, attr: impl Into<Attr>) -> Result<usize> {
        self.ensure_not_stopped()?;
        let mut termlock = self.term_lock.lock();
        termlock.print_with_attr(row, col, content, attr)
    }

    /// Set cursor position to (row, col), and show the cursor
    pub fn set_cursor(&self, row: usize, col: usize) -> Result<()> {
        self.ensure_not_stopped()?;
        let mut termlock = self.term_lock.lock();
        termlock.set_cursor(row, col)
    }

    /// show/hide cursor, set `show` to `false` to hide the cursor
    pub fn show_cursor(&self, show: bool) -> Result<()> {
        self.ensure_not_stopped()?;
        let mut termlock = self.term_lock.lock();
        termlock.show_cursor(show)
    }

    /// Enable mouse support
    pub fn enable_mouse_support(&self) -> Result<()> {
        self.ensure_not_stopped()?;
        let mut termlock = self.term_lock.lock();
        termlock.enable_mouse_support()
    }

    /// Disable mouse support
    pub fn disable_mouse_support(&self) -> Result<()> {
        self.ensure_not_stopped()?;
        let mut termlock = self.term_lock.lock();
        termlock.disable_mouse_support()
    }

    /// Whether to clear the terminal upon exiting. Defaults to true.
    pub fn clear_on_exit(&self, clear: bool) -> Result<()> {
        self.ensure_not_stopped()?;
        let mut termlock = self.term_lock.lock();
        termlock.clear_on_exit(clear);
        Ok(())
    }

    pub fn draw(&self, draw: &dyn Draw) -> Result<()> {
        let mut canvas = TermCanvas { term: self };
        draw.draw(&mut canvas).map_err(TuikitError::DrawError)
    }

    pub fn draw_mut(&self, draw: &mut dyn Draw) -> Result<()> {
        let mut canvas = TermCanvas { term: self };
        draw.draw_mut(&mut canvas).map_err(TuikitError::DrawError)
    }
}

impl<UserEvent: Send + 'static> Drop for Term<UserEvent> {
    fn drop(&mut self) {
        let _ = self.pause_internal(true);
    }
}

pub struct TermCanvas<'a, UserEvent: Send + 'static> {
    term: &'a Term<UserEvent>,
}

impl<UserEvent: Send + 'static> Canvas for TermCanvas<'_, UserEvent> {
    fn size(&self) -> Result<(usize, usize)> {
        self.term.term_size()
    }

    fn clear(&mut self) -> Result<()> {
        self.term.clear()
    }

    fn put_cell(&mut self, row: usize, col: usize, cell: Cell) -> Result<usize> {
        self.term.put_cell(row, col, cell)
    }

    fn print_with_attr(&mut self, row: usize, col: usize, content: &str, attr: Attr) -> Result<usize> {
        self.term.print_with_attr(row, col, content, attr)
    }

    fn set_cursor(&mut self, row: usize, col: usize) -> Result<()> {
        self.term.set_cursor(row, col)
    }

    fn show_cursor(&mut self, show: bool) -> Result<()> {
        self.term.show_cursor(show)
    }
}

struct TermLock {
    prefer_height: TermHeight,
    max_height: TermHeight,
    min_height: TermHeight,
    // keep bottom intact when resize?
    bottom_intact: bool,
    clear_on_exit: bool,
    clear_on_start: bool,
    mouse_enabled: bool,
    alternate_screen: bool,
    disable_alternate_screen: bool,
    cursor_row: usize,
    screen_height: usize,
    screen_width: usize,
    screen: Screen,
    output: Option<Output>,
}

impl Default for TermLock {
    fn default() -> Self {
        Self {
            prefer_height: TermHeight::Percent(100),
            max_height: TermHeight::Percent(100),
            min_height: TermHeight::Fixed(3),
            bottom_intact: false,
            alternate_screen: false,
            disable_alternate_screen: false,
            cursor_row: 0,
            screen_height: 0,
            screen_width: 0,
            screen: Screen::new(0, 0),
            output: None,
            clear_on_exit: true,
            clear_on_start: true,
            mouse_enabled: false,
        }
    }
}

impl TermLock {
    pub fn with_options(options: &TermOptions) -> Self {
        let mut term = TermLock::default();
        term.prefer_height = options.height;
        term.max_height = options.max_height;
        term.min_height = options.min_height;
        term.clear_on_exit = options.clear_on_exit;
        term.clear_on_start = options.clear_on_start;
        term.screen.clear_on_start(options.clear_on_start);
        term.disable_alternate_screen = options.disable_alternate_screen;
        term.mouse_enabled = options.mouse_enabled;
        term
    }

    /// Present the content to the terminal
    pub fn present(&mut self) -> Result<()> {
        let output = self.output.as_mut().ok_or(TuikitError::TerminalNotStarted)?;
        let mut commands = self.screen.present();

        let cursor_row = self.cursor_row;
        // add cursor_row to all CursorGoto commands
        for cmd in commands.iter_mut() {
            if let Command::CursorGoto { row, col } = *cmd {
                *cmd = Command::CursorGoto {
                    row: row + cursor_row,
                    col,
                }
            }
        }

        for cmd in commands.into_iter() {
            output.execute(cmd);
        }
        output.flush();
        Ok(())
    }

    /// Resize the internal buffer to according to new terminal size
    pub fn on_resize(&mut self) -> Result<()> {
        let output = self.output.as_mut().ok_or(TuikitError::TerminalNotStarted)?;
        let (screen_width, screen_height) = output.terminal_size().expect("term:restart get terminal size failed");
        self.screen_height = screen_height;
        self.screen_width = screen_width;

        let width = screen_width;
        let height =
            Self::calc_preferred_height(&self.min_height, &self.max_height, &self.prefer_height, screen_height);

        // update the cursor position
        if self.cursor_row + height >= screen_height {
            self.bottom_intact = true;
        }

        if self.bottom_intact {
            self.cursor_row = screen_height - height;
        }

        // clear the screen
        output.cursor_goto(self.cursor_row, 0);
        if self.clear_on_start {
            output.erase_down();
        }

        // clear the screen buffer
        self.screen.resize(width, height);
        Ok(())
    }

    fn calc_height(height_spec: &TermHeight, actual_height: usize) -> usize {
        match *height_spec {
            TermHeight::Fixed(h) => h,
            TermHeight::Percent(p) => actual_height * min(p, 100) / 100,
        }
    }

    fn calc_preferred_height(
        min_height: &TermHeight,
        max_height: &TermHeight,
        prefer_height: &TermHeight,
        height: usize,
    ) -> usize {
        let max_height = Self::calc_height(max_height, height);
        let min_height = Self::calc_height(min_height, height);
        let prefer_height = Self::calc_height(prefer_height, height);

        // ensure the calculated height is in range (MIN_HEIGHT, height)
        let max_height = max(min(max_height, height), MIN_HEIGHT);
        let min_height = max(min(min_height, height), MIN_HEIGHT);
        max(min(prefer_height, max_height), min_height)
    }

    /// Pause the terminal
    fn pause(&mut self, exiting: bool) -> Result<()> {
        self.disable_mouse()?;
        if let Some(mut output) = self.output.take() {
            output.show_cursor();
            if self.clear_on_exit || !exiting {
                // clear drawn contents
                if !self.disable_alternate_screen {
                    output.quit_alternate_screen();
                } else {
                    output.cursor_goto(self.cursor_row, 0);
                    output.erase_down();
                }
            } else {
                output.cursor_goto(self.cursor_row + self.screen.height(), 0);
                if self.bottom_intact {
                    output.write("\n");
                }
            }
            output.flush();
        }
        Ok(())
    }

    /// ensure the screen had enough height
    /// If the prefer height is full screen, it will enter alternate screen
    /// otherwise it will ensure there are enough lines at the bottom
    fn ensure_height(&mut self, cursor_pos: (usize, usize)) -> Result<()> {
        let output = self.output.as_mut().ok_or(TuikitError::TerminalNotStarted)?;

        // initialize

        let (screen_width, screen_height) = output
            .terminal_size()
            .expect("termlock:ensure_height get terminal size failed");
        let height_to_be =
            Self::calc_preferred_height(&self.min_height, &self.max_height, &self.prefer_height, screen_height);

        self.alternate_screen = false;
        let (mut cursor_row, cursor_col) = cursor_pos;
        if height_to_be >= screen_height {
            // whole screen
            self.alternate_screen = true;
            self.bottom_intact = false;
            self.cursor_row = 0;
            if !self.disable_alternate_screen {
                output.enter_alternate_screen();
            }
        } else {
            // only use part of the screen

            // go to a new line so that existing line won't be messed up
            if cursor_col > 0 {
                output.write("\n");
                cursor_row += 1;
            }

            if (cursor_row + height_to_be) <= screen_height {
                self.bottom_intact = false;
                self.cursor_row = cursor_row;
            } else {
                for _ in 0..(height_to_be - 1) {
                    output.write("\n");
                }
                self.bottom_intact = true;
                self.cursor_row = min(cursor_row, screen_height - height_to_be);
            }
        }

        output.cursor_goto(self.cursor_row, 0);
        output.flush();
        self.screen_height = screen_height;
        self.screen_width = screen_width;
        Ok(())
    }

    /// get the start row of the terminal
    pub fn get_term_start_row(&self) -> usize {
        self.cursor_row
    }

    /// restart the terminal
    pub fn restart(&mut self, output: Output, cursor_pos: (usize, usize)) -> Result<()> {
        // ensure the output area had enough height
        self.output.replace(output);
        self.ensure_height(cursor_pos)?;
        self.on_resize()?;
        if self.mouse_enabled {
            self.enable_mouse()?;
        }
        Ok(())
    }

    /// return the printable size(width, height) of the term
    pub fn term_size(&self) -> Result<(usize, usize)> {
        self.screen.size()
    }

    /// clear internal buffer
    pub fn clear(&mut self) -> Result<()> {
        self.screen.clear()
    }

    /// change a cell of position `(row, col)` to `cell`
    pub fn put_cell(&mut self, row: usize, col: usize, cell: Cell) -> Result<usize> {
        self.screen.put_cell(row, col, cell)
    }

    /// print `content` starting with position `(row, col)`
    pub fn print_with_attr(&mut self, row: usize, col: usize, content: &str, attr: impl Into<Attr>) -> Result<usize> {
        self.screen.print_with_attr(row, col, content, attr.into())
    }

    /// set cursor position to (row, col)
    pub fn set_cursor(&mut self, row: usize, col: usize) -> Result<()> {
        self.screen.set_cursor(row, col)
    }

    /// show/hide cursor, set `show` to `false` to hide the cursor
    pub fn show_cursor(&mut self, show: bool) -> Result<()> {
        self.screen.show_cursor(show)
    }

    /// Enable mouse support
    pub fn enable_mouse_support(&mut self) -> Result<()> {
        self.mouse_enabled = true;
        self.enable_mouse()
    }

    /// Disable mouse support
    pub fn disable_mouse_support(&mut self) -> Result<()> {
        self.mouse_enabled = false;
        self.disable_mouse()
    }

    pub fn clear_on_exit(&mut self, clear: bool) {
        self.clear_on_exit = clear;
    }

    /// Enable mouse (send ANSI codes to enable mouse)
    fn enable_mouse(&mut self) -> Result<()> {
        let output = self.output.as_mut().ok_or(TuikitError::TerminalNotStarted)?;
        output.enable_mouse_support();
        Ok(())
    }

    /// Disable mouse (send ANSI codes to disable mouse)
    fn disable_mouse(&mut self) -> Result<()> {
        let output = self.output.as_mut().ok_or(TuikitError::TerminalNotStarted)?;
        output.disable_mouse_support();
        Ok(())
    }
}

impl Drop for TermLock {
    fn drop(&mut self) {
        let _ = self.pause(true);
    }
}
