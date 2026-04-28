//! attr modules defines the attributes(colors, effects) of a terminal cell

use bitflags::bitflags;

pub use crate::color::Color;

/// `Attr` is a rendering attribute that contains fg color, bg color and text effect.
///
/// ```
/// use skim_tuikit::attr::{Attr, Effect, Color};
///
/// let attr = Attr { fg: Color::RED, effect: Effect::BOLD, ..Attr::default() };
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Attr {
    pub fg: Color,
    pub bg: Color,
    pub effect: Effect,
}

impl Default for Attr {
    fn default() -> Self {
        Attr {
            fg: Color::default(),
            bg: Color::default(),
            effect: Effect::empty(),
        }
    }
}

impl Attr {
    /// extend the properties with the new attr's if the properties in new attr is not default.
    /// ```
    /// use skim_tuikit::attr::{Attr, Color, Effect};
    ///
    /// let default = Attr{fg: Color::BLUE, bg: Color::YELLOW, effect: Effect::BOLD};
    /// let new = Attr{fg: Color::Default, bg: Color::WHITE, effect: Effect::REVERSE};
    /// let extended = default.extend(new);
    ///
    /// assert_eq!(Color::BLUE, extended.fg);
    /// assert_eq!(Color::WHITE, extended.bg);
    /// assert_eq!(Effect::BOLD | Effect::REVERSE, extended.effect);
    /// ```
    pub fn extend(&self, new_attr: Self) -> Attr {
        Attr {
            fg: if new_attr.fg != Color::default() {
                new_attr.fg
            } else {
                self.fg
            },
            bg: if new_attr.bg != Color::default() {
                new_attr.bg
            } else {
                self.bg
            },
            effect: self.effect | new_attr.effect,
        }
    }

    pub fn fg(mut self, fg: Color) -> Self {
        self.fg = fg;
        self
    }

    pub fn bg(mut self, bg: Color) -> Self {
        self.bg = bg;
        self
    }

    pub fn effect(mut self, effect: Effect) -> Self {
        self.effect = effect;
        self
    }
}

bitflags! {
    /// `Effect` is the effect of a text
    pub struct Effect: u8 {
        const BOLD = 0b00000001;
        const DIM = 0b00000010;
        const UNDERLINE = 0b00000100;
        const BLINK = 0b00001000;
        const REVERSE = 0b00010000;
    }
}

impl From<Color> for Attr {
    fn from(fg: Color) -> Self {
        Attr {
            fg,
            ..Default::default()
        }
    }
}

impl From<Effect> for Attr {
    fn from(effect: Effect) -> Self {
        Attr {
            effect,
            ..Default::default()
        }
    }
}
