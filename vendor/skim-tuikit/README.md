[![Crates.io](https://img.shields.io/crates/v/skim-tuikit.svg)](https://crates.io/crates/skim-tuikit)

## Tuikit

Tuikit is a TUI library for writing terminal UI applications. Highlights:

- Thread safe.
- Support non-fullscreen mode as well as fullscreen mode.
- Support `Alt` keys, mouse events, etc.
- Buffering for efficient rendering.

Tuikit is modeld after [termbox](https://github.com/nsf/termbox) which views the
terminal as a table of fixed-size cells and input being a stream of structured
messages.

**WARNING**: The library is not stable yet, the API might change.

## Usage

In your `Cargo.toml` add the following:

```toml
[dependencies]
skim-tuikit = "*"
```


Here is an example (could also be run by `cargo run --example hello-world`):

```rust
use skim_tuikit::prelude::*;
use std::cmp::{min, max};

fn main() {
    let term: Term<()> = Term::with_height(TermHeight::Percent(30)).unwrap();
    let mut row = 1;
    let mut col = 0;

    let _ = term.print(0, 0, "press arrow key to move the text, (q) to quit");
    let _ = term.present();

    while let Ok(ev) = term.poll_event() {
        let _ = term.clear();
        let _ = term.print(0, 0, "press arrow key to move the text, (q) to quit");

        let (width, height) = term.term_size().unwrap();
        match ev {
            Event::Key(Key::ESC) | Event::Key(Key::Char('q')) => break,
            Event::Key(Key::Up) => row = max(row-1, 1),
            Event::Key(Key::Down) => row = min(row+1, height-1),
            Event::Key(Key::Left) => col = max(col, 1)-1,
            Event::Key(Key::Right) => col = min(col+1, width-1),
            _ => {}
        }

        let attr = Attr{ fg: Color::RED, ..Attr::default() };
        let _ = term.print_with_attr(row, col, "Hello World! 你好！今日は。", attr);
        let _ = term.set_cursor(row, col);
        let _ = term.present();
    }
}
```

## Layout

`tuikit` provides `HSplit`, `VSplit` and `Win` for managing layouts:

1. `HSplit` allow you to split area horizontally into pieces.
2. `VSplit` works just like `HSplit` but splits vertically.
3. `Win` do not split, it could have margin, padding and border.

For example:

```rust
use skim_tuikit::prelude::*;

struct Model(String);

impl Draw for Model {
    fn draw(&self, canvas: &mut dyn Canvas) -> DrawResult<()> {
        let (width, height) = canvas.size()?;
        let message_width = self.0.len();
        let left = (width - message_width) / 2;
        let top = height / 2;
        let _ = canvas.print(top, left, &self.0);
        Ok(())
    }
}

impl Widget for Model{}

fn main() {
    let term: Term<()> = Term::with_height(TermHeight::Percent(50)).unwrap();
    let model = Model("middle!".to_string());

    while let Ok(ev) = term.poll_event() {
        if let Event::Key(Key::Char('q')) = ev {
            break;
        }
        let _ = term.print(0, 0, "press 'q' to exit");

        let hsplit = HSplit::default()
            .split(
                VSplit::default()
                    .basis(Size::Percent(30))
                    .split(Win::new(&model).border(true).basis(Size::Percent(30)))
                    .split(Win::new(&model).border(true).basis(Size::Percent(30)))
            )
            .split(Win::new(&model).border(true));

        let _ = term.draw(&hsplit);
        let _ = term.present();
    }
}
```

The split algorithm is simple:

1. Both `HSplit` and `VSplit` will take several `Split` where a `Split` would
   contains:
    1. basis, the original size
    2. grow, the factor to grow if there is still enough room
    3. shrink, the factor to shrink if there is not enough room
2. `HSplit/VSplit` will count the total width/height(basis) of the split items
3. Judge if the current width/height is enough or not for the split items
4. shrink/grow the split items according to their grow/shrink: `factor / sum(factors)`
5. If still not enough room, the last one(s) would be set width/height 0

## References

`Tuikit` borrows ideas from lots of other projects:

- [rustyline](https://github.com/kkawakam/rustyline) Readline Implementation in Rust.
    - How to enter the raw mode.
    - Part of the keycode parsing logic.
- [termion](https://gitlab.redox-os.org/redox-os/termion) A bindless library for controlling terminals/TTY.
    - How to parse mouse events.
    - How to enter raw mode.
- [rustbox](https://github.com/gchp/rustbox) and [termbox](https://github.com/nsf/termbox)
    - The idea of viewing terminal as table of fixed cells.
- [termfest](https://github.com/agatan/termfest) Easy TUI library written in Rust
    - The buffering idea.
