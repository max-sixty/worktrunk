/// `Cell` is a cell of the terminal.
/// It has a display character and an attribute (fg and bg color, effects).
use crate::attr::{Attr, Color, Effect};

const EMPTY_CHAR: char = '\0';

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    pub ch: char,
    pub attr: Attr,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            attr: Attr::default(),
        }
    }
}

impl Cell {
    pub fn empty() -> Self {
        Self::default().ch(EMPTY_CHAR)
    }

    pub fn ch(mut self, ch: char) -> Self {
        self.ch = ch;
        self
    }

    pub fn fg(mut self, fg: Color) -> Self {
        self.attr.fg = fg;
        self
    }

    pub fn bg(mut self, bg: Color) -> Self {
        self.attr.bg = bg;
        self
    }

    pub fn effect(mut self, effect: Effect) -> Self {
        self.attr.effect = effect;
        self
    }

    pub fn attribute(mut self, attr: Attr) -> Self {
        self.attr = attr;
        self
    }

    /// check if a cell is empty
    pub fn is_empty(self) -> bool {
        self.ch == EMPTY_CHAR && self.attr == Attr::default()
    }
}

impl From<char> for Cell {
    fn from(ch: char) -> Self {
        Cell {
            ch,
            attr: Attr::default(),
        }
    }
}
