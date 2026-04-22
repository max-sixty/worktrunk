use crate::event::Event;
use crate::key::Key;
use crate::widget::Rectangle;

pub fn adjust_event(event: Event, inner_rect: Rectangle) -> Option<Event> {
    match event {
        Event::Key(Key::MousePress(button, row, col)) => {
            if inner_rect.contains(row as usize, col as usize) {
                let (row, col) = inner_rect.relative_to_origin(row as usize, col as usize);
                Some(Event::Key(Key::MousePress(button, row as u16, col as u16)))
            } else {
                None
            }
        }
        Event::Key(Key::MouseRelease(row, col)) => {
            if inner_rect.contains(row as usize, col as usize) {
                let (row, col) = inner_rect.relative_to_origin(row as usize, col as usize);
                Some(Event::Key(Key::MouseRelease(row as u16, col as u16)))
            } else {
                None
            }
        }
        Event::Key(Key::MouseHold(row, col)) => {
            if inner_rect.contains(row as usize, col as usize) {
                let (row, col) = inner_rect.relative_to_origin(row as usize, col as usize);
                Some(Event::Key(Key::MouseHold(row as u16, col as u16)))
            } else {
                None
            }
        }
        Event::Key(Key::SingleClick(button, row, col)) => {
            if inner_rect.contains(row as usize, col as usize) {
                let (row, col) = inner_rect.relative_to_origin(row as usize, col as usize);
                Some(Event::Key(Key::SingleClick(button, row as u16, col as u16)))
            } else {
                None
            }
        }
        Event::Key(Key::DoubleClick(button, row, col)) => {
            if inner_rect.contains(row as usize, col as usize) {
                let (row, col) = inner_rect.relative_to_origin(row as usize, col as usize);
                Some(Event::Key(Key::DoubleClick(button, row as u16, col as u16)))
            } else {
                None
            }
        }
        Event::Key(Key::WheelDown(row, col, count)) => {
            if inner_rect.contains(row as usize, col as usize) {
                let (row, col) = inner_rect.relative_to_origin(row as usize, col as usize);
                Some(Event::Key(Key::WheelDown(row as u16, col as u16, count)))
            } else {
                None
            }
        }
        Event::Key(Key::WheelUp(row, col, count)) => {
            if inner_rect.contains(row as usize, col as usize) {
                let (row, col) = inner_rect.relative_to_origin(row as usize, col as usize);
                Some(Event::Key(Key::WheelUp(row as u16, col as u16, count)))
            } else {
                None
            }
        }
        ev => Some(ev),
    }
}
