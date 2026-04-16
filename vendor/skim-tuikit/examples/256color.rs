use skim_tuikit::attr::Color;
use skim_tuikit::output::Output;
use std::io;

fn main() {
    let mut output = Output::new(Box::new(io::stdout())).unwrap();

    for fg in 0..=255 {
        output.set_fg(Color::AnsiValue(fg));
        output.write(format!("{:5}", fg).as_str());
        if fg % 16 == 15 {
            output.reset_attributes();
            output.write("\n");
            output.flush()
        }
    }

    output.reset_attributes();

    for bg in 0..=255 {
        output.set_bg(Color::AnsiValue(bg));
        output.write(format!("{:5}", bg).as_str());
        if bg % 16 == 15 {
            output.reset_attributes();
            output.write("\n");
            output.flush()
        }
    }

    output.flush()
}
