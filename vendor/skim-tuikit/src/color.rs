/// Color of a character, could be 8 bit(256 color) or RGB color
///
/// ```
/// use skim_tuikit::attr::Color;
/// Color::RED; // predefined values
/// Color::Rgb(255, 0, 0); // RED
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub enum Color {
    #[default]
    Default,
    AnsiValue(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    pub const BLACK: Color = Color::AnsiValue(0);
    pub const RED: Color = Color::AnsiValue(1);
    pub const GREEN: Color = Color::AnsiValue(2);
    pub const YELLOW: Color = Color::AnsiValue(3);
    pub const BLUE: Color = Color::AnsiValue(4);
    pub const MAGENTA: Color = Color::AnsiValue(5);
    pub const CYAN: Color = Color::AnsiValue(6);
    pub const WHITE: Color = Color::AnsiValue(7);
    pub const LIGHT_BLACK: Color = Color::AnsiValue(8);
    pub const LIGHT_RED: Color = Color::AnsiValue(9);
    pub const LIGHT_GREEN: Color = Color::AnsiValue(10);
    pub const LIGHT_YELLOW: Color = Color::AnsiValue(11);
    pub const LIGHT_BLUE: Color = Color::AnsiValue(12);
    pub const LIGHT_MAGENTA: Color = Color::AnsiValue(13);
    pub const LIGHT_CYAN: Color = Color::AnsiValue(14);
    pub const LIGHT_WHITE: Color = Color::AnsiValue(15);
}
