use skim_tuikit::attr::{Attr, Color};
use skim_tuikit::canvas::Canvas;
use skim_tuikit::output::Output;
use skim_tuikit::screen::Screen;
use std::io;

fn main() {
    let mut output = Output::new(Box::new(io::stdout())).unwrap();
    let (width, height) = output.terminal_size().unwrap();
    let mut screen = Screen::new(width, height);

    for fg in 0..=255 {
        let _ = screen.print_with_attr(
            fg / 16,
            (fg % 16) * 5,
            format!("{:5}", fg).as_str(),
            Color::AnsiValue(fg as u8).into(),
        );
    }

    let _ = screen.set_cursor(15, 80);
    let commands = screen.present();

    commands.into_iter().for_each(|cmd| output.execute(cmd));
    output.flush();

    let _ = screen.print_with_attr(0, 78, "HELLO WORLD", Attr::default());
    let commands = screen.present();

    commands.into_iter().for_each(|cmd| output.execute(cmd));
    output.flush();

    for bg in 0..=255 {
        let _ = screen.print_with_attr(
            bg / 16,
            (bg % 16) * 5,
            format!("{:5}", bg).as_str(),
            Attr {
                bg: Color::AnsiValue(bg as u8),
                ..Attr::default()
            },
        );
    }
    let commands = screen.present();
    commands.into_iter().for_each(|cmd| output.execute(cmd));
    output.reset_attributes();
    output.flush()
}
