use crate::canvas::Canvas;
use crate::draw::{Draw, DrawResult};
use crate::event::Event;
use crate::widget::{Rectangle, Widget};

/// A stack of widgets, will draw the including widgets back to front
pub struct Stack<'a, Message = ()> {
    inner: Vec<Box<dyn Widget<Message> + 'a>>,
}

impl<Message> Default for Stack<'_, Message> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a, Message> Stack<'a, Message> {
    pub fn new() -> Self {
        Self { inner: vec![] }
    }

    pub fn top(mut self, widget: impl Widget<Message> + 'a) -> Self {
        self.inner.push(Box::new(widget));
        self
    }

    pub fn bottom(mut self, widget: impl Widget<Message> + 'a) -> Self {
        self.inner.insert(0, Box::new(widget));
        self
    }
}

impl<Message> Draw for Stack<'_, Message> {
    fn draw(&self, canvas: &mut dyn Canvas) -> DrawResult<()> {
        for widget in self.inner.iter() {
            widget.draw(canvas)?
        }

        Ok(())
    }
    fn draw_mut(&mut self, canvas: &mut dyn Canvas) -> DrawResult<()> {
        for widget in self.inner.iter_mut() {
            widget.draw_mut(canvas)?
        }

        Ok(())
    }
}

impl<Message> Widget<Message> for Stack<'_, Message> {
    fn size_hint(&self) -> (Option<usize>, Option<usize>) {
        // max of the inner widgets
        let width = self
            .inner
            .iter()
            .map(|widget| widget.size_hint().0)
            .max()
            .unwrap_or(None);
        let height = self
            .inner
            .iter()
            .map(|widget| widget.size_hint().1)
            .max()
            .unwrap_or(None);
        (width, height)
    }

    fn on_event(&self, event: Event, rect: Rectangle) -> Vec<Message> {
        // like javascript's capture, from top to bottom
        for widget in self.inner.iter().rev() {
            let message = widget.on_event(event, rect);
            if !message.is_empty() {
                return message;
            }
        }
        vec![]
    }

    fn on_event_mut(&mut self, event: Event, rect: Rectangle) -> Vec<Message> {
        // like javascript's capture, from top to bottom
        for widget in self.inner.iter_mut().rev() {
            let message = widget.on_event_mut(event, rect);
            if !message.is_empty() {
                return message;
            }
        }
        vec![]
    }
}

#[cfg(test)]
#[allow(dead_code)]
mod test {
    use super::*;
    use crate::cell::Cell;
    use std::sync::Mutex;

    struct WinHint {
        pub width_hint: Option<usize>,
        pub height_hint: Option<usize>,
    }

    impl Draw for WinHint {
        fn draw(&self, _canvas: &mut dyn Canvas) -> DrawResult<()> {
            unimplemented!()
        }
    }

    impl Widget for WinHint {
        fn size_hint(&self) -> (Option<usize>, Option<usize>) {
            (self.width_hint, self.height_hint)
        }
    }

    #[test]
    fn size_hint() {
        let stack = Stack::new().top(WinHint {
            width_hint: None,
            height_hint: None,
        });
        assert_eq!((None, None), stack.size_hint());

        let stack = Stack::new().top(WinHint {
            width_hint: Some(1),
            height_hint: Some(1),
        });
        assert_eq!((Some(1), Some(1)), stack.size_hint());

        let stack = Stack::new()
            .top(WinHint {
                width_hint: Some(1),
                height_hint: Some(2),
            })
            .top(WinHint {
                width_hint: Some(2),
                height_hint: Some(1),
            });
        assert_eq!((Some(2), Some(2)), stack.size_hint());

        let stack = Stack::new()
            .top(WinHint {
                width_hint: None,
                height_hint: None,
            })
            .top(WinHint {
                width_hint: Some(2),
                height_hint: Some(1),
            });
        assert_eq!((Some(2), Some(1)), stack.size_hint());
    }

    #[derive(PartialEq, Debug)]
    enum Called {
        No,
        Mut,
        Immut,
    }

    struct Drawn {
        called: Mutex<Called>,
    }

    impl Draw for Drawn {
        fn draw(&self, _canvas: &mut dyn Canvas) -> DrawResult<()> {
            *self.called.lock().unwrap() = Called::Immut;
            Ok(())
        }
        fn draw_mut(&mut self, _canvas: &mut dyn Canvas) -> DrawResult<()> {
            *self.called.lock().unwrap() = Called::Mut;
            Ok(())
        }
    }

    impl Widget for Drawn {}

    #[derive(Default)]
    struct TestCanvas {}

    #[allow(unused_variables)]
    impl Canvas for TestCanvas {
        fn size(&self) -> crate::Result<(usize, usize)> {
            Ok((100, 100))
        }

        fn clear(&mut self) -> crate::Result<()> {
            unimplemented!()
        }

        fn put_cell(&mut self, row: usize, col: usize, cell: Cell) -> crate::Result<usize> {
            Ok(1)
        }

        fn set_cursor(&mut self, row: usize, col: usize) -> crate::Result<()> {
            unimplemented!()
        }

        fn show_cursor(&mut self, show: bool) -> crate::Result<()> {
            unimplemented!()
        }
    }

    #[test]
    fn mutable_widget() {
        let mut canvas = TestCanvas::default();

        let mut mutable = Drawn {
            called: Mutex::new(Called::No),
        };
        {
            let mut stack = Stack::new().top(&mut mutable);
            stack.draw_mut(&mut canvas).unwrap();
        }
        assert_eq!(Called::Mut, *mutable.called.lock().unwrap());

        let immutable = Drawn {
            called: Mutex::new(Called::No),
        };
        let stack = Stack::new().top(&immutable);
        stack.draw(&mut canvas).unwrap();
        assert_eq!(Called::Immut, *immutable.called.lock().unwrap());
    }
}
