use skim_tuikit::output::Output;
use std::io;

fn main() {
    let output = Output::new(Box::new(io::stdout())).unwrap();
    let (width, height) = output.terminal_size().unwrap();
    println!("width: {}, height: {}", width, height);
}
