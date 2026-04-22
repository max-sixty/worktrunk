use bitflags::_core::result::Result::Ok;

use skim_tuikit::prelude::*;

fn main() {
    let term: Term<String> = Term::with_height(TermHeight::Percent(30)).expect("term creation error");
    let _ = term.print(0, 0, "Press 'q' or 'Ctrl-c' to quit!");
    while let Ok(ev) = term.poll_event() {
        match ev {
            Event::Key(Key::Char('q')) | Event::Key(Key::Ctrl('c')) => break,
            Event::Key(key) => {
                let _ = term.print(1, 0, format!("get key: {:?}", key).as_str());
                let _ = term.send_event(Event::User(format!("key: {:?}", key)));
            }
            Event::User(ev_str) => {
                let _ = term.print(2, 0, format!("user event: {}", &ev_str).as_str());
            }
            _ => {
                let _ = term.print(3, 0, format!("event: {:?}", ev).as_str());
            }
        }
        let _ = term.present();
    }
}
