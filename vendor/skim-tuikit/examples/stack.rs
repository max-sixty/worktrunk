use skim_tuikit::prelude::*;

struct Model {
    win: String,
}

impl Draw for Model {
    fn draw(&self, canvas: &mut dyn Canvas) -> DrawResult<()> {
        let (width, height) = canvas.size()?;
        let _ = canvas.clear();
        let message_width = self.win.len();
        let left = (width - message_width) / 2;
        let top = height / 2;
        let _ = canvas.print(top, left, &self.win);
        Ok(())
    }
}

impl Widget<String> for Model {
    fn on_event(&self, event: Event, _rect: Rectangle) -> Vec<String> {
        if let Event::Key(Key::SingleClick(_, _, _)) = event {
            vec![format!("{} clicked", self.win)]
        } else {
            vec![]
        }
    }
}

fn main() {
    let term = Term::with_options(TermOptions::default().mouse_enabled(true)).unwrap();
    let (mut width, mut height) = term.term_size().unwrap();

    while let Ok(ev) = term.poll_event() {
        match ev {
            Event::Key(Key::Char('q')) | Event::Key(Key::Ctrl('c')) => break,
            Event::Key(Key::MouseRelease(_, _)) => continue,
            Event::Resize { width: w, height: h } => {
                width = w;
                height = h;
            }
            _ => (),
        }
        let stack = Stack::<String>::new()
            .top(
                Win::new(Model {
                    win: "win floating on top".to_string(),
                })
                .border(true)
                .margin(Size::Percent(30)),
            )
            .bottom(
                HSplit::default()
                    .split(
                        Win::new(Model {
                            win: String::from("left"),
                        })
                        .border(true),
                    )
                    .split(
                        Win::new(Model {
                            win: String::from("right"),
                        })
                        .border(true),
                    ),
            );

        let message = stack.on_event(
            ev,
            Rectangle {
                width,
                height,
                top: 0,
                left: 0,
            },
        );
        let click_message = if message.is_empty() { "" } else { &message[0] };
        let _ = term.draw(&stack);
        let _ = term.print(1, 1, "press 'q' to exit, try clicking on windows");
        let _ = term.print(2, 1, &(String::from(click_message) + "                       "));
        let _ = term.present();
    }
    let _ = term.show_cursor(false);
}
