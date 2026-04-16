use skim_tuikit::attr::Color;
use skim_tuikit::output::Output;
use std::io;

// ported from: https://github.com/gnachman/iTerm2/blob/master/tests/24-bit-color.sh
// should be run in terminals that supports true color

// given a color idx/22 along HSV, return (r, g, b)
fn rainbow_color(idx: u8) -> (u8, u8, u8) {
    let h = idx / 43;
    let f = idx - 43 * h;
    let t = ((f as i32 * 255) / 43) as u8;
    let q = 255 - t;

    match h {
        0 => (255, t, 0),
        1 => (q, 255, 0),
        2 => (0, 255, t),
        3 => (0, q, 255),
        4 => (t, 0, 255),
        5 => (255, 0, q),
        _ => unreachable!(),
    }
}

fn try_background(output: &mut Output, r: u8, g: u8, b: u8) {
    output.set_bg(Color::Rgb(r, g, b));
    output.write(" ")
}

fn reset_output(output: &mut Output) {
    output.reset_attributes();
    output.write("\n");
    output.flush();
}

fn main() {
    let mut output = Output::new(Box::new(io::stdout())).unwrap();
    for i in 0..=127 {
        try_background(&mut output, i, 0, 0);
    }
    reset_output(&mut output);

    for i in (128..=255).rev() {
        try_background(&mut output, i, 0, 0);
    }
    reset_output(&mut output);

    for i in 0..=127 {
        try_background(&mut output, 0, i, 0);
    }
    reset_output(&mut output);

    for i in (128..=255).rev() {
        try_background(&mut output, 0, i, 0);
    }
    reset_output(&mut output);

    for i in 0..=127 {
        try_background(&mut output, 0, 0, i);
    }
    reset_output(&mut output);

    for i in (128..=255).rev() {
        try_background(&mut output, 0, 0, i);
    }
    reset_output(&mut output);

    for i in 0..=127 {
        let (r, g, b) = rainbow_color(i);
        try_background(&mut output, r, g, b);
    }
    reset_output(&mut output);

    for i in (128..=255).rev() {
        let (r, g, b) = rainbow_color(i);
        try_background(&mut output, r, g, b);
    }
    reset_output(&mut output);
}
