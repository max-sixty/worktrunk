use skim_tuikit::prelude::*;
use std::cmp::{max, min};

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
            Event::Key(Key::ESC) | Event::Key(Key::Char('q')) | Event::Key(Key::Ctrl('c')) => break,
            Event::Key(Key::Up) => row = max(row - 1, 1),
            Event::Key(Key::Down) => row = min(row + 1, height - 1),
            Event::Key(Key::Left) => col = max(col, 1) - 1,
            Event::Key(Key::Right) => col = min(col + 1, width - 1),
            _ => {}
        }

        let _ = term.print_with_attr(row, col, "Hello World! 你好！今日は。", Color::RED);
        let _ = term.set_cursor(row, col);
        let _ = term.present();
    }
}
