pub use self::align::*;
// Various pre-defined widget that implements Draw
pub use self::split::*;
pub use self::stack::*;
pub use self::win::*;
use crate::draw::Draw;
use crate::event::Event;
use std::cmp::min;
mod align;
mod split;
mod stack;
mod util;
mod win;

/// Whether fixed size or percentage
#[derive(Debug, Copy, Clone, Default)]
pub enum Size {
    Fixed(usize),
    Percent(usize),
    #[default]
    Default,
}

impl Size {
    pub fn calc_fixed_size(&self, total_size: usize, default_size: usize) -> usize {
        match *self {
            Size::Fixed(fixed) => min(total_size, fixed),
            Size::Percent(percent) => min(total_size, total_size * percent / 100),
            Size::Default => default_size,
        }
    }
}

impl From<usize> for Size {
    fn from(size: usize) -> Self {
        Size::Fixed(size)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Rectangle {
    pub top: usize,
    pub left: usize,
    pub width: usize,
    pub height: usize,
}

impl Rectangle {
    /// check if the given point(row, col) lies in the rectangle
    pub fn contains(&self, row: usize, col: usize) -> bool {
        if row < self.top || row >= self.top + self.height {
            false
        } else {
            !(col < self.left || col >= self.left + self.width)
        }
    }

    /// assume the point (row, col) lies in the rectangle, adjust the origin to the rectangle's
    /// origin (top, left)
    pub fn relative_to_origin(&self, row: usize, col: usize) -> (usize, usize) {
        (row - self.top, col - self.left)
    }

    pub fn adjust_origin(&self) -> Rectangle {
        Self {
            top: 0,
            left: 0,
            width: self.width,
            height: self.height,
        }
    }
}

/// A widget could be recursive nested
pub trait Widget<Message = ()>: Draw {
    /// the (width, height) of the content
    /// it will be the hint for layouts to calculate the final size
    fn size_hint(&self) -> (Option<usize>, Option<usize>) {
        (None, None)
    }

    /// given a key event, emit zero or more messages
    /// typical usage is the mouse click event where containers would pass the event down
    /// to their children.
    fn on_event(&self, event: Event, rect: Rectangle) -> Vec<Message> {
        let _ = (event, rect); // avoid warning
        Vec::new()
    }

    /// same as `on_event` except that the self reference is mutable
    fn on_event_mut(&mut self, event: Event, rect: Rectangle) -> Vec<Message> {
        let _ = (event, rect); // avoid warning
        Vec::new()
    }
}

impl<Message, T: Widget<Message>> Widget<Message> for &T {
    fn size_hint(&self) -> (Option<usize>, Option<usize>) {
        (*self).size_hint()
    }

    fn on_event(&self, event: Event, rect: Rectangle) -> Vec<Message> {
        (*self).on_event(event, rect)
    }

    fn on_event_mut(&mut self, event: Event, rect: Rectangle) -> Vec<Message> {
        (**self).on_event(event, rect)
    }
}

impl<Message, T: Widget<Message>> Widget<Message> for &mut T {
    fn size_hint(&self) -> (Option<usize>, Option<usize>) {
        (**self).size_hint()
    }

    fn on_event(&self, event: Event, rect: Rectangle) -> Vec<Message> {
        (**self).on_event(event, rect)
    }

    fn on_event_mut(&mut self, event: Event, rect: Rectangle) -> Vec<Message> {
        (**self).on_event_mut(event, rect)
    }
}

impl<Message, T: Widget<Message> + ?Sized> Widget<Message> for Box<T> {
    fn size_hint(&self) -> (Option<usize>, Option<usize>) {
        self.as_ref().size_hint()
    }

    fn on_event(&self, event: Event, rect: Rectangle) -> Vec<Message> {
        self.as_ref().on_event(event, rect)
    }

    fn on_event_mut(&mut self, event: Event, rect: Rectangle) -> Vec<Message> {
        self.as_mut().on_event_mut(event, rect)
    }
}
