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

impl Widget for Model {}

fn main() {
    let term: Term<()> = Term::with_options(
        TermOptions::default()
            .height(TermHeight::Percent(50))
            .disable_alternate_screen(true)
            .clear_on_start(false),
    )
    .unwrap();
    let model = Model("Hey, I'm in middle!".to_string());

    while let Ok(ev) = term.poll_event() {
        match ev {
            Event::Key(Key::Char('q')) | Event::Key(Key::Ctrl('c')) => break,
            _ => (),
        }
        let _ = term.print(0, 0, "press 'q' to exit");

        let inner_win = Win::new(&model)
            .fn_draw_header(Box::new(|canvas| {
                let _ = canvas.print(0, 0, "header printed with function");
                Ok(())
            }))
            .border(true);

        let win_bottom_title = Win::new(&inner_win)
            .title_align(HorizontalAlign::Center)
            .title("Title (at bottom) center aligned")
            .right_prompt("Right Prompt stays")
            .title_on_top(false)
            .border_bottom(true);

        let win = Win::new(&win_bottom_title)
            .margin(Size::Percent(10))
            .padding(1)
            .title("Window Title")
            .right_prompt("Right Prompt")
            .border(true)
            .border_top_attr(Color::BLUE)
            .border_right_attr(Color::YELLOW)
            .border_bottom_attr(Color::RED)
            .border_left_attr(Color::GREEN);

        let _ = term.draw(&win);
        let _ = term.present();
    }
}
