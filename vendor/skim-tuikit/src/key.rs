//! Defines all the keys `tuikit` recognizes.

// http://ascii-table.com/ansi-escape-sequences.php
/// Single key
#[non_exhaustive]
#[rustfmt::skip]
#[derive(Eq, PartialEq, Hash, Debug, Copy, Clone)]
pub enum Key {
    Null,
    ESC,

    Ctrl(char),
    Tab, // Ctrl-I
    Enter, // Ctrl-M

    BackTab, Backspace, AltBackTab,

    Up, Down, Left, Right, Home, End, Insert, Delete, PageUp, PageDown,
    CtrlUp, CtrlDown, CtrlLeft, CtrlRight,
    ShiftUp, ShiftDown, ShiftLeft, ShiftRight,
    AltUp, AltDown, AltLeft, AltRight, AltHome, AltEnd, AltPageUp, AltPageDown,
    AltShiftUp, AltShiftDown, AltShiftLeft, AltShiftRight,

    F(u8),

    CtrlAlt(char), // chars are lower case
    AltEnter,
    AltBackspace,
    AltTab,
    Alt(char),  // chars could be lower or upper case
    Char(char), // chars could be lower or upper case
    CursorPos(u16, u16), // row, col

    // raw mouse events, will only generated if raw mouse mode is enabled
    MousePress(MouseButton, u16, u16), // row, col
    MouseRelease(u16, u16), // row, col
    MouseHold(u16, u16), // row, col

    // parsed mouse events, will be generated if raw mouse mode is disabled
    SingleClick(MouseButton, u16, u16), // row, col
    DoubleClick(MouseButton, u16, u16), // row, col, will only record left button double click
    WheelUp(u16, u16, u16), // row, col, number of scroll
    WheelDown(u16, u16, u16), // row, col, number of scroll

    BracketedPasteStart,
    BracketedPasteEnd,
}

/// A mouse button.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum MouseButton {
    /// The left mouse button.
    Left,
    /// The right mouse button.
    Right,
    /// The middle mouse button.
    Middle,
    /// Mouse wheel is going up.
    ///
    /// This event is typically only used with MousePress.
    WheelUp,
    /// Mouse wheel is going down.
    ///
    /// This event is typically only used with MousePress.
    WheelDown,
}

#[rustfmt::skip]
pub fn from_keyname(keyname: &str) -> Option<Key> {
    use self::Key::*;
    match keyname.to_lowercase().as_ref() {
        "ctrl-space" | "ctrl-`" | "ctrl-@" => Some(Ctrl(' ')),
        "ctrl-a" => Some(Ctrl('a')),
        "ctrl-b" => Some(Ctrl('b')),
        "ctrl-c" => Some(Ctrl('c')),
        "ctrl-d" => Some(Ctrl('d')),
        "ctrl-e" => Some(Ctrl('e')),
        "ctrl-f" => Some(Ctrl('f')),
        "ctrl-g" => Some(Ctrl('g')),
        "ctrl-h" => Some(Ctrl('h')),
        "tab" | "ctrl-i" => Some(Tab),
        "ctrl-j" => Some(Ctrl('j')),
        "ctrl-k" => Some(Ctrl('k')),
        "ctrl-l" => Some(Ctrl('l')),
        "enter" | "return" | "ctrl-m" => Some(Enter),
        "ctrl-n" => Some(Ctrl('n')),
        "ctrl-o" => Some(Ctrl('o')),
        "ctrl-p" => Some(Ctrl('p')),
        "ctrl-q" => Some(Ctrl('q')),
        "ctrl-r" => Some(Ctrl('r')),
        "ctrl-s" => Some(Ctrl('s')),
        "ctrl-t" => Some(Ctrl('t')),
        "ctrl-u" => Some(Ctrl('u')),
        "ctrl-v" => Some(Ctrl('v')),
        "ctrl-w" => Some(Ctrl('w')),
        "ctrl-x" => Some(Ctrl('x')),
        "ctrl-y" => Some(Ctrl('y')),
        "ctrl-z" => Some(Ctrl('z')),
        "ctrl-up"    => Some(CtrlUp),
        "ctrl-down"  => Some(CtrlDown),
        "ctrl-left"  => Some(CtrlLeft),
        "ctrl-right" => Some(CtrlRight),

        "ctrl-alt-space" => Some(Ctrl(' ')),
        "ctrl-alt-a" => Some(CtrlAlt('a')),
        "ctrl-alt-b" => Some(CtrlAlt('b')),
        "ctrl-alt-c" => Some(CtrlAlt('c')),
        "ctrl-alt-d" => Some(CtrlAlt('d')),
        "ctrl-alt-e" => Some(CtrlAlt('e')),
        "ctrl-alt-f" => Some(CtrlAlt('f')),
        "ctrl-alt-g" => Some(CtrlAlt('g')),
        "ctrl-alt-h" => Some(CtrlAlt('h')),
        "ctrl-alt-j" => Some(CtrlAlt('j')),
        "ctrl-alt-k" => Some(CtrlAlt('k')),
        "ctrl-alt-l" => Some(CtrlAlt('l')),
        "ctrl-alt-n" => Some(CtrlAlt('n')),
        "ctrl-alt-o" => Some(CtrlAlt('o')),
        "ctrl-alt-p" => Some(CtrlAlt('p')),
        "ctrl-alt-q" => Some(CtrlAlt('q')),
        "ctrl-alt-r" => Some(CtrlAlt('r')),
        "ctrl-alt-s" => Some(CtrlAlt('s')),
        "ctrl-alt-t" => Some(CtrlAlt('t')),
        "ctrl-alt-u" => Some(CtrlAlt('u')),
        "ctrl-alt-v" => Some(CtrlAlt('v')),
        "ctrl-alt-w" => Some(CtrlAlt('w')),
        "ctrl-alt-x" => Some(CtrlAlt('x')),
        "ctrl-alt-y" => Some(CtrlAlt('y')),
        "ctrl-alt-z" => Some(CtrlAlt('z')),

        "esc"                => Some(ESC),
        "btab" | "shift-tab" => Some(BackTab),
        "bspace" | "bs"      => Some(Backspace),
        "ins" | "insert"     => Some(Insert),
        "del"                => Some(Delete),
        "pgup" | "page-up"   => Some(PageUp),
        "pgdn" | "page-down" => Some(PageDown),
        "up"                 => Some(Up),
        "down"               => Some(Down),
        "left"               => Some(Left),
        "right"              => Some(Right),
        "home"               => Some(Home),
        "end"                => Some(End),
        "shift-up"           => Some(ShiftUp),
        "shift-down"         => Some(ShiftDown),
        "shift-left"         => Some(ShiftLeft),
        "shift-right"        => Some(ShiftRight),

        "f1"  => Some(F(1)),
        "f2"  => Some(F(2)),
        "f3"  => Some(F(3)),
        "f4"  => Some(F(4)),
        "f5"  => Some(F(5)),
        "f6"  => Some(F(6)),
        "f7"  => Some(F(7)),
        "f8"  => Some(F(8)),
        "f9"  => Some(F(9)),
        "f10" => Some(F(10)),
        "f11" => Some(F(11)),
        "f12" => Some(F(12)),

        "alt-a" => Some(Alt('a')),
        "alt-b" => Some(Alt('b')),
        "alt-c" => Some(Alt('c')),
        "alt-d" => Some(Alt('d')),
        "alt-e" => Some(Alt('e')),
        "alt-f" => Some(Alt('f')),
        "alt-g" => Some(Alt('g')),
        "alt-h" => Some(Alt('h')),
        "alt-i" => Some(Alt('i')),
        "alt-j" => Some(Alt('j')),
        "alt-k" => Some(Alt('k')),
        "alt-l" => Some(Alt('l')),
        "alt-m" => Some(Alt('m')),
        "alt-n" => Some(Alt('n')),
        "alt-o" => Some(Alt('o')),
        "alt-p" => Some(Alt('p')),
        "alt-q" => Some(Alt('q')),
        "alt-r" => Some(Alt('r')),
        "alt-s" => Some(Alt('s')),
        "alt-t" => Some(Alt('t')),
        "alt-u" => Some(Alt('u')),
        "alt-v" => Some(Alt('v')),
        "alt-w" => Some(Alt('w')),
        "alt-x" => Some(Alt('x')),
        "alt-y" => Some(Alt('y')),
        "alt-z" => Some(Alt('z')),
        "alt-/" => Some(Alt('/')),

        "shift-a" => Some(Char('A')),
        "shift-b" => Some(Char('B')),
        "shift-c" => Some(Char('C')),
        "shift-d" => Some(Char('D')),
        "shift-e" => Some(Char('E')),
        "shift-f" => Some(Char('F')),
        "shift-g" => Some(Char('G')),
        "shift-h" => Some(Char('H')),
        "shift-i" => Some(Char('I')),
        "shift-j" => Some(Char('J')),
        "shift-k" => Some(Char('K')),
        "shift-l" => Some(Char('L')),
        "shift-m" => Some(Char('M')),
        "shift-n" => Some(Char('N')),
        "shift-o" => Some(Char('O')),
        "shift-p" => Some(Char('P')),
        "shift-q" => Some(Char('Q')),
        "shift-r" => Some(Char('R')),
        "shift-s" => Some(Char('S')),
        "shift-t" => Some(Char('T')),
        "shift-u" => Some(Char('U')),
        "shift-v" => Some(Char('V')),
        "shift-w" => Some(Char('W')),
        "shift-x" => Some(Char('X')),
        "shift-y" => Some(Char('Y')),
        "shift-z" => Some(Char('Z')),

        "alt-shift-a" => Some(Alt('A')),
        "alt-shift-b" => Some(Alt('B')),
        "alt-shift-c" => Some(Alt('C')),
        "alt-shift-d" => Some(Alt('D')),
        "alt-shift-e" => Some(Alt('E')),
        "alt-shift-f" => Some(Alt('F')),
        "alt-shift-g" => Some(Alt('G')),
        "alt-shift-h" => Some(Alt('H')),
        "alt-shift-i" => Some(Alt('I')),
        "alt-shift-j" => Some(Alt('J')),
        "alt-shift-k" => Some(Alt('K')),
        "alt-shift-l" => Some(Alt('L')),
        "alt-shift-m" => Some(Alt('M')),
        "alt-shift-n" => Some(Alt('N')),
        "alt-shift-o" => Some(Alt('O')),
        "alt-shift-p" => Some(Alt('P')),
        "alt-shift-q" => Some(Alt('Q')),
        "alt-shift-r" => Some(Alt('R')),
        "alt-shift-s" => Some(Alt('S')),
        "alt-shift-t" => Some(Alt('T')),
        "alt-shift-u" => Some(Alt('U')),
        "alt-shift-v" => Some(Alt('V')),
        "alt-shift-w" => Some(Alt('W')),
        "alt-shift-x" => Some(Alt('X')),
        "alt-shift-y" => Some(Alt('Y')),
        "alt-shift-z" => Some(Alt('Z')),

        "alt-btab" | "alt-shift-tab" => Some(AltBackTab),
        "alt-bspace" | "alt-bs"      => Some(AltBackspace),
        "alt-pgup" | "alt-page-up"   => Some(AltPageUp),
        "alt-pgdn" | "alt-page-down" => Some(AltPageDown),
        "alt-up"                     => Some(AltUp),
        "alt-down"                   => Some(AltDown),
        "alt-left"                   => Some(AltLeft),
        "alt-right"                  => Some(AltRight),
        "alt-home"                   => Some(AltHome),
        "alt-end"                    => Some(AltEnd),
        "alt-shift-up"               => Some(AltShiftUp),
        "alt-shift-down"             => Some(AltShiftDown),
        "alt-shift-left"             => Some(AltShiftLeft),
        "alt-shift-right"            => Some(AltShiftRight),
        "alt-enter" | "alt-ctrl-m"   => Some(AltEnter),
        "alt-tab" | "alt-ctrl-i"     => Some(AltTab),

        "space" => Some(Char(' ')),
        "alt-space" => Some(Alt(' ')),

        ch if ch.chars().count() == 1 => {
            Some(Char(ch.chars().next().expect("input:parse_key: no key is specified")))
        },
        _ => None,
    }
}

#[cfg(test)]
mod test {
    use super::Key::*;
    use super::*;

    #[test]
    fn bind_shift_key() {
        // Without the "shift-" prefix, "from_keyname" ignores the case.
        assert_eq!(from_keyname("A").unwrap(), Char('a'));

        // A correct way to refer to an uppercase char.
        assert_eq!(from_keyname("shift-a").unwrap(), Char('A'));
    }
}
