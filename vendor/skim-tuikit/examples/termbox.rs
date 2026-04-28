use skim_tuikit::prelude::*;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

extern crate env_logger;

/// This example is testing tuikit with multi-threads.

const COL: usize = 4;

fn main() {
    env_logger::init();
    let term = Arc::new(Term::with_height(TermHeight::Fixed(10)).unwrap());
    let _ = term.enable_mouse_support();
    let now = Instant::now();

    print_banner(&term);

    let th = thread::spawn(move || {
        while let Ok(ev) = term.poll_event() {
            match ev {
                Event::Key(Key::Char('q')) | Event::Key(Key::Ctrl('c')) => break,
                Event::Key(Key::Char('r')) => {
                    let term = term.clone();
                    thread::spawn(move || {
                        let _ = term.pause();
                        println!("restart in 2 seconds");
                        thread::sleep(Duration::from_secs(2));
                        let _ = term.restart();
                        let _ = term.clear();
                    });
                }
                _ => (),
            }

            print_banner(&term);
            print_event(&term, ev, &now);
        }
    });
    let _ = th.join();
}

fn print_banner(term: &Term) {
    let (_, height) = term.term_size().unwrap_or((5, 5));
    for row in 0..height {
        let _ = term.print(row, 0, format!("{} ", row).as_str());
    }
    let attr = Attr {
        fg: Color::GREEN,
        effect: Effect::UNDERLINE,
        ..Attr::default()
    };
    let _ = term.print_with_attr(0, COL, "How to use: (q)uit, (r)estart", attr);
    let _ = term.present();
}

fn print_event(term: &Term, ev: Event, now: &Instant) {
    let elapsed = now.elapsed();
    let (_, height) = term.term_size().unwrap_or((5, 5));
    let _ = term.print(1, COL, format!("{:?}", ev).as_str());
    let _ = term.print(
        height - 1,
        COL,
        format!(
            "time elapsed since program start: {}s + {}ms",
            elapsed.as_secs(),
            elapsed.subsec_millis()
        )
        .as_str(),
    );
    let _ = term.present();
}
