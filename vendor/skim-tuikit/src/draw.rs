/// A trait defines something that could be drawn
use crate::canvas::Canvas;

pub type DrawResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Something that knows how to draw itself onto the canvas
#[allow(unused_variables)]
pub trait Draw {
    fn draw(&self, canvas: &mut dyn Canvas) -> DrawResult<()> {
        Ok(())
    }
    fn draw_mut(&mut self, canvas: &mut dyn Canvas) -> DrawResult<()> {
        self.draw(canvas)
    }
}

impl<T: Draw> Draw for &T {
    fn draw(&self, canvas: &mut dyn Canvas) -> DrawResult<()> {
        (*self).draw(canvas)
    }
    fn draw_mut(&mut self, canvas: &mut dyn Canvas) -> DrawResult<()> {
        (*self).draw(canvas)
    }
}

impl<T: Draw> Draw for &mut T {
    fn draw(&self, canvas: &mut dyn Canvas) -> DrawResult<()> {
        (**self).draw(canvas)
    }
    fn draw_mut(&mut self, canvas: &mut dyn Canvas) -> DrawResult<()> {
        (**self).draw_mut(canvas)
    }
}

impl<T: Draw + ?Sized> Draw for Box<T> {
    fn draw(&self, canvas: &mut dyn Canvas) -> DrawResult<()> {
        self.as_ref().draw(canvas)
    }

    fn draw_mut(&mut self, canvas: &mut dyn Canvas) -> DrawResult<()> {
        self.as_mut().draw_mut(canvas)
    }
}
