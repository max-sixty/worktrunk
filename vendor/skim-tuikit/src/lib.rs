//!
//! ## Tuikit
//! Tuikit is a TUI library for writing terminal UI applications. Highlights:
//!
//! - Thread safe.
//! - Support non-fullscreen mode as well as fullscreen mode.
//! - Support `Alt` keys, mouse events, etc.
//! - Buffering for efficient rendering.
//!
//! Tuikit is modeld after [termbox](https://github.com/nsf/termbox) which views the
//! terminal as a table of fixed-size cells and input being a stream of structured
//! messages.
//!
//! ## Usage
//!
//! In your `Cargo.toml` add the following:
//!
//! ```toml
//! [dependencies]
//! tuikit = "*"
//! ```
//!
//! Here is an example:
//!
//! ```no_run
//! use skim_tuikit::attr::*;
//! use skim_tuikit::term::{Term, TermHeight};
//! use skim_tuikit::event::{Event, Key};
//! use std::cmp::{min, max};
//!
//! let term: Term<()> = Term::with_height(TermHeight::Percent(30)).unwrap();
//! let mut row = 1;
//! let mut col = 0;
//!
//! let _ = term.print(0, 0, "press arrow key to move the text, (q) to quit");
//! let _ = term.present();
//!
//! while let Ok(ev) = term.poll_event() {
//!     let _ = term.clear();
//!     let _ = term.print(0, 0, "press arrow key to move the text, (q) to quit");
//!
//!     let (width, height) = term.term_size().unwrap();
//!     match ev {
//!         Event::Key(Key::ESC) | Event::Key(Key::Char('q')) => break,
//!         Event::Key(Key::Up) => row = max(row-1, 1),
//!         Event::Key(Key::Down) => row = min(row+1, height-1),
//!         Event::Key(Key::Left) => col = max(col, 1)-1,
//!         Event::Key(Key::Right) => col = min(col+1, width-1),
//!         _ => {}
//!     }
//!
//!     let attr = Attr{ fg: Color::RED, ..Attr::default() };
//!     let _ = term.print_with_attr(row, col, "Hello World! 你好！今日は。", attr);
//!     let _ = term.set_cursor(row, col);
//!     let _ = term.present();
//! }
//! ```
pub mod attr;
pub mod canvas;
pub mod cell;
mod color;
pub mod draw;
pub mod error;
pub mod event;
pub mod input;
pub mod key;
mod macros;
pub mod output;
pub mod prelude;
pub mod raw;
pub mod screen;
use skim_common::spinlock;
mod sys;
pub mod term;
pub mod widget;

#[macro_use]
extern crate log;

use crate::error::TuikitError;

pub type Result<T> = std::result::Result<T, TuikitError>;
