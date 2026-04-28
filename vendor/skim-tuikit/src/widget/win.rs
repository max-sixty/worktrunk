use super::Size;
use super::split::Split;
use super::util::adjust_event;
use super::{Rectangle, Widget};
use crate::attr::Attr;
use crate::canvas::{BoundedCanvas, Canvas};
use crate::cell::Cell;
use crate::draw::{Draw, DrawResult};
use crate::event::Event;
use crate::widget::align::{AlignSelf, HorizontalAlign};
use crate::{ok_or_return, some_or_return};
use std::cmp::max;
use unicode_width::UnicodeWidthStr;

type FnDrawHeader = dyn Fn(&mut dyn Canvas) -> DrawResult<()>;

/// A Win is like a div in HTML, it has its margin/padding, and border
pub struct Win<'a, Message = ()> {
    margin_top: Size,
    margin_right: Size,
    margin_bottom: Size,
    margin_left: Size,

    padding_top: Size,
    padding_right: Size,
    padding_bottom: Size,
    padding_left: Size,

    border_top: bool,
    border_right: bool,
    border_bottom: bool,
    border_left: bool,

    border_top_attr: Attr,
    border_right_attr: Attr,
    border_bottom_attr: Attr,
    border_left_attr: Attr,

    fn_draw_header: Option<Box<FnDrawHeader>>,
    title: Option<String>,
    title_attr: Attr,
    right_prompt: Option<String>,
    right_prompt_attr: Attr,
    title_align: HorizontalAlign,
    title_on_top: bool,

    basis: Size,
    grow: usize,
    shrink: usize,

    inner: Box<dyn Widget<Message> + 'a>,
}

// Builder
impl<'a, Message> Win<'a, Message> {
    pub fn new(widget: impl Widget<Message> + 'a) -> Self {
        Self {
            margin_top: Default::default(),
            margin_right: Default::default(),
            margin_bottom: Default::default(),
            margin_left: Default::default(),
            padding_top: Default::default(),
            padding_right: Default::default(),
            padding_bottom: Default::default(),
            padding_left: Default::default(),
            border_top: false,
            border_right: false,
            border_bottom: false,
            border_left: false,
            border_top_attr: Default::default(),
            border_right_attr: Default::default(),
            border_bottom_attr: Default::default(),
            border_left_attr: Default::default(),
            fn_draw_header: None,
            title: None,
            title_attr: Default::default(),
            right_prompt: None,
            right_prompt_attr: Default::default(),
            title_align: HorizontalAlign::Left,
            title_on_top: true,
            basis: Size::Default,
            grow: 1,
            shrink: 1,
            inner: Box::new(widget),
        }
    }

    pub fn margin_top(mut self, margin_top: impl Into<Size>) -> Self {
        self.margin_top = margin_top.into();
        self
    }

    pub fn margin_right(mut self, margin_right: impl Into<Size>) -> Self {
        self.margin_right = margin_right.into();
        self
    }

    pub fn margin_bottom(mut self, margin_bottom: impl Into<Size>) -> Self {
        self.margin_bottom = margin_bottom.into();
        self
    }

    pub fn margin_left(mut self, margin_left: impl Into<Size>) -> Self {
        self.margin_left = margin_left.into();
        self
    }

    pub fn margin(mut self, margin: impl Into<Size>) -> Self {
        let margin = margin.into();
        self.margin_top = margin;
        self.margin_right = margin;
        self.margin_bottom = margin;
        self.margin_left = margin;
        self
    }

    pub fn padding_top(mut self, padding_top: impl Into<Size>) -> Self {
        self.padding_top = padding_top.into();
        self
    }

    pub fn padding_right(mut self, padding_right: impl Into<Size>) -> Self {
        self.padding_right = padding_right.into();
        self
    }

    pub fn padding_bottom(mut self, padding_bottom: impl Into<Size>) -> Self {
        self.padding_bottom = padding_bottom.into();
        self
    }

    pub fn padding_left(mut self, padding_left: impl Into<Size>) -> Self {
        self.padding_left = padding_left.into();
        self
    }

    pub fn padding(mut self, padding: impl Into<Size>) -> Self {
        let padding = padding.into();
        self.padding_top = padding;
        self.padding_right = padding;
        self.padding_bottom = padding;
        self.padding_left = padding;
        self
    }

    pub fn border_top(mut self, border_top: bool) -> Self {
        self.border_top = border_top;
        self
    }

    pub fn border_right(mut self, border_right: bool) -> Self {
        self.border_right = border_right;
        self
    }

    pub fn border_bottom(mut self, border_bottom: bool) -> Self {
        self.border_bottom = border_bottom;
        self
    }

    pub fn border_left(mut self, border_left: bool) -> Self {
        self.border_left = border_left;
        self
    }

    pub fn border(mut self, border: bool) -> Self {
        self.border_top = border;
        self.border_right = border;
        self.border_bottom = border;
        self.border_left = border;
        self
    }

    pub fn border_top_attr(mut self, border_top_attr: impl Into<Attr>) -> Self {
        self.border_top_attr = border_top_attr.into();
        self
    }

    pub fn border_right_attr(mut self, border_right_attr: impl Into<Attr>) -> Self {
        self.border_right_attr = border_right_attr.into();
        self
    }

    pub fn border_bottom_attr(mut self, border_bottom_attr: impl Into<Attr>) -> Self {
        self.border_bottom_attr = border_bottom_attr.into();
        self
    }

    pub fn border_left_attr(mut self, border_left_attr: impl Into<Attr>) -> Self {
        self.border_left_attr = border_left_attr.into();
        self
    }

    pub fn border_attr(mut self, attr: impl Into<Attr>) -> Self {
        let attr = attr.into();
        self.border_top_attr = attr;
        self.border_right_attr = attr;
        self.border_bottom_attr = attr;
        self.border_left_attr = attr;
        self
    }

    pub fn fn_draw_header(mut self, fn_draw_header: Box<FnDrawHeader>) -> Self {
        self.fn_draw_header = Some(fn_draw_header);
        self
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn title_attr(mut self, title_attr: impl Into<Attr>) -> Self {
        self.title_attr = title_attr.into();
        self
    }

    pub fn right_prompt(mut self, right_prompt: impl Into<String>) -> Self {
        self.right_prompt = Some(right_prompt.into());
        self
    }

    pub fn right_prompt_attr(mut self, right_prompt_attr: impl Into<Attr>) -> Self {
        self.right_prompt_attr = right_prompt_attr.into();
        self
    }

    pub fn title_align(mut self, align: HorizontalAlign) -> Self {
        self.title_align = align;
        self
    }

    pub fn title_on_top(mut self, title_on_top: bool) -> Self {
        self.title_on_top = title_on_top;
        self
    }

    pub fn basis(mut self, basis: impl Into<Size>) -> Self {
        self.basis = basis.into();
        self
    }

    pub fn grow(mut self, grow: usize) -> Self {
        self.grow = grow;
        self
    }

    pub fn shrink(mut self, shrink: usize) -> Self {
        self.shrink = shrink;
        self
    }
}

impl<'a, Message> Win<'a, Message> {
    fn rect_reserve_margin(&self, rect: Rectangle) -> DrawResult<Rectangle> {
        let Rectangle { width, height, .. } = rect;

        let margin_top = self.margin_top.calc_fixed_size(height, 0);
        let margin_right = self.margin_right.calc_fixed_size(width, 0);
        let margin_bottom = self.margin_bottom.calc_fixed_size(height, 0);
        let margin_left = self.margin_left.calc_fixed_size(width, 0);

        if margin_top + margin_bottom >= height || margin_left + margin_right >= width {
            return Err("margin takes too much screen".into());
        }

        let top = margin_top;
        let left = margin_left;
        let width = width - (margin_left + margin_right);
        let height = height - (margin_top + margin_bottom);
        Ok(Rectangle {
            top,
            left,
            width,
            height,
        })
    }

    fn rect_header(&self, rect_reserve_margin: Rectangle) -> Rectangle {
        let Rectangle {
            top,
            mut left,
            width,
            height,
        } = rect_reserve_margin;

        let new_top = if self.title_on_top {
            top
        } else {
            max(top + height, 1) - 1
        };

        let height_needed = if self.title_on_top && self.border_bottom { 2 } else { 1 };
        if height_needed > height {
            // not enough space, don't draw at all
            return Rectangle {
                top: new_top,
                left,
                width,
                height: 0,
            };
        }

        let mut width_needed = 0;
        if self.border_left {
            width_needed += 1;
            left += 1;
        }
        if self.border_right {
            width_needed += 1;
        }
        if width_needed > width {
            return Rectangle {
                top: new_top,
                left,
                width: 0,
                height,
            };
        }

        Rectangle {
            top: new_top,
            left,
            width: width - width_needed,
            height: 1,
        }
    }

    fn rect_reserve_border(&self, rect: Rectangle) -> DrawResult<Rectangle> {
        let Rectangle {
            top,
            left,
            width,
            height,
        } = rect;

        // title and right prompt will be displayed on top
        let border_top =
            self.border_top || (self.title_on_top && (self.title.is_some() || self.right_prompt.is_some()));
        let border_bottom =
            self.border_bottom || (!self.title_on_top && (self.title.is_some() || self.right_prompt.is_some()));

        if (border_top || border_bottom) && ((height < 1) || (border_top && border_bottom && height < 2)) {
            return Err("not enough height for border".into());
        }

        if (self.border_left || self.border_right)
            && ((width < 1) || (self.border_left && self.border_right && width < 2))
        {
            return Err("not enough width for border".into());
        }

        let top = if border_top { top + 1 } else { top };
        let left = if self.border_left { left + 1 } else { left };
        let width = if self.border_left { width - 1 } else { width };
        let width = if self.border_right { width - 1 } else { width };
        let height = if border_top { height - 1 } else { height };
        let height = if border_bottom { height - 1 } else { height };

        Ok(Rectangle {
            top,
            left,
            width,
            height,
        })
    }

    fn rect_reserve_padding(&self, rect: Rectangle) -> DrawResult<Rectangle> {
        let Rectangle {
            top,
            left,
            width,
            height,
        } = rect;

        let padding_top = self.padding_top.calc_fixed_size(height, 0);
        let padding_right = self.padding_right.calc_fixed_size(width, 0);
        let padding_bottom = self.padding_bottom.calc_fixed_size(height, 0);
        let padding_left = self.padding_left.calc_fixed_size(width, 0);

        if padding_top + padding_bottom >= height || padding_left + padding_right >= width {
            return Err("padding takes too much screen, won't draw".into());
        }

        let top = top + padding_top;
        let left = left + padding_left;
        let width = width - (padding_left + padding_right);
        let height = height - (padding_top + padding_bottom);
        Ok(Rectangle {
            top,
            left,
            width,
            height,
        })
    }

    /// Calculate the inner rectangle(inside margin, border, padding)
    fn calc_inner_rect(&self, rect: Rectangle) -> DrawResult<Rectangle> {
        self.rect_reserve_padding(self.rect_reserve_border(self.rect_reserve_margin(rect)?)?)
    }

    /// draw border and return the position & size of the inner canvas
    /// (top, left, width, height)
    fn draw_border(&self, rect: Rectangle, canvas: &mut dyn Canvas) -> DrawResult<()> {
        let Rectangle {
            top,
            left,
            width,
            height,
        } = rect;

        if (self.border_top || self.border_bottom)
            && ((height < 1) || (self.border_top && self.border_bottom && height < 2))
        {
            return Err("not enough height for border".into());
        }

        if (self.border_left || self.border_right)
            && ((width < 1) || (self.border_left && self.border_right && width < 2))
        {
            return Err("not enough width for border".into());
        }

        let bottom = max(top + height, 1) - 1;
        let right = max(left + width, 1) - 1;

        if self.border_top {
            let _ = canvas.print_with_attr(top, left, &"─".repeat(width), self.border_top_attr);
        }

        if self.border_bottom {
            let _ = canvas.print_with_attr(bottom, left, &"─".repeat(width), self.border_bottom_attr);
        }

        if self.border_left {
            for i in top..(top + height) {
                let _ = canvas.print_with_attr(i, left, "│", self.border_left_attr);
            }
        }

        if self.border_right {
            for i in top..(top + height) {
                let _ = canvas.print_with_attr(i, right, "│", self.border_right_attr);
            }
        }

        // draw 4 corners if necessary

        if self.border_top && self.border_left {
            let _ = canvas.put_cell(top, left, Cell::default().ch('┌').attribute(self.border_top_attr));
        }

        if self.border_top && self.border_right {
            let _ = canvas.put_cell(top, right, Cell::default().ch('┐').attribute(self.border_top_attr));
        }

        if self.border_bottom && self.border_left {
            let _ = canvas.put_cell(bottom, left, Cell::default().ch('└').attribute(self.border_bottom_attr));
        }

        if self.border_bottom && self.border_right {
            let _ = canvas.put_cell(
                bottom,
                right,
                Cell::default().ch('┘').attribute(self.border_bottom_attr),
            );
        }

        Ok(())
    }

    fn draw_title_and_prompt(&self, canvas: &mut dyn Canvas) -> DrawResult<()> {
        let (width, height) = canvas.size()?;
        let row = if self.title_on_top { 0 } else { max(height, 1) - 1 };

        if self.right_prompt.is_some() {
            let prompt = self.right_prompt.as_ref().unwrap();
            let text_width = prompt.width_cjk();
            let left = HorizontalAlign::Right.adjust(0, width, text_width);
            canvas.print_with_attr(row, left, prompt, self.right_prompt_attr)?;
        }

        if self.title.is_some() {
            let title = self.title.as_ref().unwrap();
            let text_width = title.width_cjk();
            let left = self.title_align.adjust(0, width, text_width);
            canvas.print_with_attr(row, left, title, self.right_prompt_attr)?;
        }

        Ok(())
    }

    fn draw_header(&self, canvas: &mut dyn Canvas) -> DrawResult<()> {
        let (width, height) = canvas.size()?;
        if width == 0 || height == 0 {
            return Ok(());
        }

        if self.fn_draw_header.is_some() {
            self.fn_draw_header.as_ref().unwrap()(canvas)?;
        } else {
            self.draw_title_and_prompt(canvas)?;
        }

        Ok(())
    }

    fn draw_context(&self, canvas: &'a mut dyn Canvas) -> DrawResult<BoundedCanvas<'a>> {
        let (width, height) = canvas.size()?;
        let outer_rect = Rectangle {
            top: 0,
            left: 0,
            width,
            height,
        };

        let rect_in_margin = self.rect_reserve_margin(outer_rect)?;
        self.draw_border(rect_in_margin, canvas)?;

        let Rectangle {
            top,
            left,
            width,
            height,
        } = self.rect_header(rect_in_margin);
        let mut header_canvas = BoundedCanvas::new(top, left, width, height, canvas);
        self.draw_header(&mut header_canvas)?;

        let Rectangle {
            top,
            left,
            width,
            height,
        } = self.calc_inner_rect(outer_rect)?;

        Ok(BoundedCanvas::new(top, left, width, height, canvas))
    }
}

impl<Message> Draw for Win<'_, Message> {
    /// Reserve margin & padding, draw border.
    fn draw(&self, canvas: &mut dyn Canvas) -> DrawResult<()> {
        let mut new_canvas = self.draw_context(canvas)?;
        self.inner.draw(&mut new_canvas)
    }

    fn draw_mut(&mut self, canvas: &mut dyn Canvas) -> DrawResult<()> {
        let mut new_canvas = self.draw_context(canvas)?;
        self.inner.draw_mut(&mut new_canvas)
    }
}

impl<Message> Widget<Message> for Win<'_, Message> {
    fn size_hint(&self) -> (Option<usize>, Option<usize>) {
        // plus border size
        let (width, height) = self.inner.size_hint();
        let width = width.map(|mut w| {
            w += if self.border_left { 1 } else { 0 };
            w += if self.border_right { 1 } else { 0 };
            w
        });

        let height = height.map(|mut h| {
            h += if self.border_top { 1 } else { 0 };
            h += if self.border_bottom { 1 } else { 0 };
            h
        });

        (width, height)
    }

    fn on_event(&self, event: Event, rect: Rectangle) -> Vec<Message> {
        let empty = vec![];
        let inner_rect = ok_or_return!(self.calc_inner_rect(rect), empty);
        let adjusted_event = some_or_return!(adjust_event(event, inner_rect), empty);
        self.inner.on_event(adjusted_event, inner_rect)
    }

    fn on_event_mut(&mut self, event: Event, rect: Rectangle) -> Vec<Message> {
        let empty = vec![];
        let inner_rect = ok_or_return!(self.calc_inner_rect(rect), empty);
        let adjusted_event = some_or_return!(adjust_event(event, inner_rect), empty);
        self.inner.on_event(adjusted_event, inner_rect)
    }
}

impl<Message> Split<Message> for Win<'_, Message> {
    fn get_basis(&self) -> Size {
        self.basis
    }

    fn get_grow(&self) -> usize {
        self.grow
    }

    fn get_shrink(&self) -> usize {
        self.shrink
    }
}

#[cfg(test)]
#[allow(dead_code)]
mod test {
    use super::*;
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
    fn size_hint_for_window_should_include_border() {
        let inner = WinHint {
            width_hint: None,
            height_hint: None,
        };
        let win_border_top = Win::new(&inner).border_top(true);
        assert_eq!((None, None), win_border_top.size_hint());
        let win_border_right = Win::new(&inner).border_right(true);
        assert_eq!((None, None), win_border_right.size_hint());
        let win_border_bottom = Win::new(&inner).border_bottom(true);
        assert_eq!((None, None), win_border_bottom.size_hint());
        let win_border_left = Win::new(&inner).border_left(true);
        assert_eq!((None, None), win_border_left.size_hint());

        let inner = WinHint {
            width_hint: Some(1),
            height_hint: None,
        };
        let win_border_top = Win::new(&inner).border_top(true);
        assert_eq!((Some(1), None), win_border_top.size_hint());
        let win_border_right = Win::new(&inner).border_right(true);
        assert_eq!((Some(2), None), win_border_right.size_hint());
        let win_border_bottom = Win::new(&inner).border_bottom(true);
        assert_eq!((Some(1), None), win_border_bottom.size_hint());
        let win_border_left = Win::new(&inner).border_left(true);
        assert_eq!((Some(2), None), win_border_left.size_hint());

        let inner = WinHint {
            width_hint: None,
            height_hint: Some(1),
        };
        let win_border_top = Win::new(&inner).border_top(true);
        assert_eq!((None, Some(2)), win_border_top.size_hint());
        let win_border_right = Win::new(&inner).border_right(true);
        assert_eq!((None, Some(1)), win_border_right.size_hint());
        let win_border_bottom = Win::new(&inner).border_bottom(true);
        assert_eq!((None, Some(2)), win_border_bottom.size_hint());
        let win_border_left = Win::new(&inner).border_left(true);
        assert_eq!((None, Some(1)), win_border_left.size_hint());
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
            let mut win = Win::new(&mut mutable);
            win.draw_mut(&mut canvas).unwrap();
        }
        assert_eq!(Called::Mut, *mutable.called.lock().unwrap());

        let immutable = Drawn {
            called: Mutex::new(Called::No),
        };
        let win = Win::new(&immutable);
        win.draw(&mut canvas).unwrap();
        assert_eq!(Called::Immut, *immutable.called.lock().unwrap());
    }
}
