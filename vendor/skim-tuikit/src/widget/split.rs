use super::Size;
use super::util::adjust_event;
use super::{Rectangle, Widget};
use crate::canvas::{BoundedCanvas, Canvas};
use crate::draw::Draw;
use crate::draw::DrawResult;
use crate::event::Event;
use std::cmp::{Ordering, min};

/// A Split item would contain 3 things
/// 0. inner_size, will be used if `basis` is `Size::Default`.
/// 1. basis, the original size
/// 2. grow, the factor to grow if there is still enough room
/// 3. shrink, the factor to shrink if there is not enough room
pub trait Split<Message = ()>: Widget<Message> {
    fn get_basis(&self) -> Size;

    fn get_grow(&self) -> usize;

    fn get_shrink(&self) -> usize;

    /// get the default size of inner content, will be used if `basis` is Default
    fn inner_size(&self) -> (Size, Size) {
        let (width, height) = self.size_hint();
        let width = width.map(Size::Fixed).unwrap_or(Size::Default);
        let height = height.map(Size::Fixed).unwrap_or(Size::Default);
        (width, height)
    }
}

impl<Message, T: Split<Message> + Widget<Message>> Split<Message> for &T {
    fn get_basis(&self) -> Size {
        (*self).get_basis()
    }

    fn get_grow(&self) -> usize {
        (*self).get_grow()
    }

    fn get_shrink(&self) -> usize {
        (*self).get_shrink()
    }

    fn inner_size(&self) -> (Size, Size) {
        (*self).inner_size()
    }
}

impl<Message, T: Split<Message> + Widget<Message>> Split<Message> for &mut T {
    fn get_basis(&self) -> Size {
        (**self).get_basis()
    }

    fn get_grow(&self) -> usize {
        (**self).get_grow()
    }

    fn get_shrink(&self) -> usize {
        (**self).get_shrink()
    }

    fn inner_size(&self) -> (Size, Size) {
        (**self).inner_size()
    }
}

enum Op {
    Noop,
    Grow,
    Shrink,
}

enum SplitType {
    Horizontal,
    Vertical,
}

trait SplitContainer<'a, Message = ()> {
    fn get_splits(&self) -> &[Box<dyn Split<Message> + 'a>];

    fn get_split_type(&self) -> SplitType;

    /// return the target sizes of the splits
    fn retrieve_split_info(&self, actual_size: usize) -> Vec<usize> {
        let split_type = self.get_split_type();

        let split_sizes: Vec<usize> = self
            .get_splits()
            .iter()
            .map(|split| {
                let (width, height) = split.inner_size();
                let default = match &split_type {
                    SplitType::Horizontal => width,
                    SplitType::Vertical => height,
                };

                match split.get_basis() {
                    Size::Default => default,
                    basis => basis,
                }
            })
            .map(|size| size.calc_fixed_size(actual_size, actual_size))
            .collect();

        let target_total_size: usize = split_sizes.iter().sum();

        let op = match target_total_size.cmp(&actual_size) {
            Ordering::Equal => Op::Noop,
            Ordering::Less => Op::Grow,
            Ordering::Greater => Op::Shrink,
        };

        let size_diff = match op {
            Op::Noop => 0,
            Op::Grow => actual_size - target_total_size,
            Op::Shrink => target_total_size - actual_size,
        };

        let split_factors: Vec<usize> = self
            .get_splits()
            .iter()
            .map(|split| match op {
                Op::Noop => 0,
                Op::Shrink => split.get_shrink(),
                Op::Grow => split.get_grow(),
            })
            .collect();

        let total_factors: usize = split_factors.iter().sum();

        let unit = if total_factors == 0 {
            0
        } else {
            size_diff / total_factors
        };

        (0..split_sizes.len())
            .map(|idx| {
                let diff = split_factors[idx] * unit;
                match op {
                    Op::Noop => split_sizes[idx],
                    Op::Grow => split_sizes[idx] + diff,
                    Op::Shrink => split_sizes[idx] - min(split_sizes[idx], diff),
                }
            })
            .collect()
    }
}

/// HSplit will split the area horizontally. It will
/// 1. Count the total width(basis) of the split items it contains
/// 2. Judge if the current width is enough or not for the split items
/// 3. shrink/grow the split items according to their factors / (total factors)
/// 4. If still not enough room, the last one(s) would be set width 0
pub struct HSplit<'a, Message = ()> {
    basis: Size,
    grow: usize,
    shrink: usize,
    splits: Vec<Box<dyn Split<Message> + 'a>>,
}

impl<Message> Default for HSplit<'_, Message> {
    fn default() -> Self {
        Self {
            basis: Size::Default,
            grow: 1,
            shrink: 1,
            splits: Vec::new(),
        }
    }
}

impl<'a, Message> HSplit<'a, Message> {
    pub fn split(mut self, split: impl Split<Message> + 'a) -> Self {
        self.splits.push(Box::new(split));
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

impl<'a, Message> SplitContainer<'a, Message> for HSplit<'a, Message> {
    fn get_splits(&self) -> &[Box<dyn Split<Message> + 'a>] {
        &self.splits
    }

    fn get_split_type(&self) -> SplitType {
        SplitType::Horizontal
    }
}

impl<Message> Draw for HSplit<'_, Message> {
    fn draw(&self, canvas: &mut dyn Canvas) -> DrawResult<()> {
        let (width, height) = canvas.size()?;
        let target_widths = self.retrieve_split_info(width);

        // iterate over the splits
        let mut left = 0;
        for (idx, split) in self.splits.iter().enumerate() {
            let target_width = target_widths[idx];
            let right = min(left + target_width, width);
            let mut new_canvas = BoundedCanvas::new(0, left, right - left, height, canvas);
            let _ = split.draw(&mut new_canvas);
            left = right;
        }

        Ok(())
    }

    fn draw_mut(&mut self, canvas: &mut dyn Canvas) -> DrawResult<()> {
        let (width, height) = canvas.size()?;
        let target_widths = self.retrieve_split_info(width);

        // iterate over the splits
        let mut left = 0;
        for (idx, split) in self.splits.iter_mut().enumerate() {
            let target_width = target_widths[idx];
            let right = min(left + target_width, width);
            let mut new_canvas = BoundedCanvas::new(0, left, right - left, height, canvas);
            let _ = split.draw_mut(&mut new_canvas);
            left = right;
        }

        Ok(())
    }
}

impl<Message> Widget<Message> for HSplit<'_, Message> {
    fn size_hint(&self) -> (Option<usize>, Option<usize>) {
        let has_width_hint = self.splits.iter().any(|split| split.size_hint().0.is_some());
        let has_height_hint = self.splits.iter().any(|split| split.size_hint().1.is_some());

        let width = if has_width_hint {
            Some(self.splits.iter().map(|split| split.size_hint().0.unwrap_or(0)).sum())
        } else {
            None
        };

        let height = if has_height_hint {
            Some(
                self.splits
                    .iter()
                    .map(|split| split.size_hint().1.unwrap_or(0))
                    .max()
                    .unwrap_or(0),
            )
        } else {
            None
        };

        (width, height)
    }

    fn on_event(&self, event: Event, rect: Rectangle) -> Vec<Message> {
        // should collect events from every children
        let target_widths = self.retrieve_split_info(rect.width);
        let Rectangle { top, width, height, .. } = rect;
        let mut messages = vec![];

        // iterate over the splits
        let mut left = 0;
        for (idx, split) in self.splits.iter().enumerate() {
            let target_width = target_widths[idx];
            let right = min(left + target_width, width);
            let sub_rect = Rectangle {
                top,
                left,
                width: target_width,
                height,
            };

            let mut sub_message = adjust_event(event, sub_rect)
                .map(|ev| split.as_ref().on_event(ev, sub_rect.adjust_origin()))
                .unwrap_or_default();
            messages.append(&mut sub_message);
            left = right;
        }

        messages
    }

    fn on_event_mut(&mut self, event: Event, rect: Rectangle) -> Vec<Message> {
        // should collect events from every children
        let target_widths = self.retrieve_split_info(rect.width);
        let Rectangle { top, width, height, .. } = rect;
        let mut messages = vec![];

        // iterate over the splits
        let mut left = 0;
        for (idx, split) in self.splits.iter_mut().enumerate() {
            let target_width = target_widths[idx];
            let right = min(left + target_width, width);
            let sub_rect = Rectangle {
                top,
                left,
                width: target_width,
                height,
            };

            let mut sub_message = adjust_event(event, sub_rect)
                .map(|ev| split.as_mut().on_event_mut(ev, sub_rect.adjust_origin()))
                .unwrap_or_default();
            messages.append(&mut sub_message);
            left = right;
        }

        messages
    }
}

impl<Message> Split<Message> for HSplit<'_, Message> {
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

/// VSplit will split the area vertically. It will
/// 1. Count the total height(basis) of the split items it contains
/// 2. Judge if the current height is enough or not for the split items
/// 3. shrink/grow the split items according to their factors / (total factors)
/// 4. If still not enough room, the last one(s) would be set height 0
pub struct VSplit<'a, Message = ()> {
    basis: Size,
    grow: usize,
    shrink: usize,
    splits: Vec<Box<dyn Split<Message> + 'a>>,
}

impl<Message> Default for VSplit<'_, Message> {
    fn default() -> Self {
        Self {
            basis: Size::Default,
            grow: 1,
            shrink: 1,
            splits: Vec::new(),
        }
    }
}

impl<'a, Message> VSplit<'a, Message> {
    pub fn split(mut self, split: impl Split<Message> + 'a) -> Self {
        self.splits.push(Box::new(split));
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

impl<'a, Message> SplitContainer<'a, Message> for VSplit<'a, Message> {
    fn get_splits(&self) -> &[Box<dyn Split<Message> + 'a>] {
        &self.splits
    }

    fn get_split_type(&self) -> SplitType {
        SplitType::Vertical
    }
}

impl<Message> Draw for VSplit<'_, Message> {
    fn draw(&self, canvas: &mut dyn Canvas) -> DrawResult<()> {
        let (width, height) = canvas.size()?;
        let target_heights = self.retrieve_split_info(height);

        // iterate over the splits
        let mut top = 0;
        for (idx, split) in self.splits.iter().enumerate() {
            let target_height = target_heights[idx];
            let bottom = min(top + target_height, height);
            let mut new_canvas = BoundedCanvas::new(top, 0, width, bottom - top, canvas);
            let _ = split.draw(&mut new_canvas);
            top = bottom;
        }

        Ok(())
    }
    fn draw_mut(&mut self, canvas: &mut dyn Canvas) -> DrawResult<()> {
        let (width, height) = canvas.size()?;
        let target_heights = self.retrieve_split_info(height);

        // iterate over the splits
        let mut top = 0;
        for (idx, split) in self.splits.iter_mut().enumerate() {
            let target_height = target_heights[idx];
            let bottom = min(top + target_height, height);
            let mut new_canvas = BoundedCanvas::new(top, 0, width, bottom - top, canvas);
            let _ = split.draw_mut(&mut new_canvas);
            top = bottom;
        }

        Ok(())
    }
}

impl<Message> Widget<Message> for VSplit<'_, Message> {
    fn size_hint(&self) -> (Option<usize>, Option<usize>) {
        let has_width_hint = self.splits.iter().any(|split| split.size_hint().0.is_some());
        let has_height_hint = self.splits.iter().any(|split| split.size_hint().1.is_some());

        let width = if has_width_hint {
            Some(
                self.splits
                    .iter()
                    .map(|split| split.size_hint().0.unwrap_or(0))
                    .max()
                    .unwrap_or(0),
            )
        } else {
            None
        };

        let height = if has_height_hint {
            Some(self.splits.iter().map(|split| split.size_hint().1.unwrap_or(0)).sum())
        } else {
            None
        };

        (width, height)
    }

    fn on_event(&self, event: Event, rect: Rectangle) -> Vec<Message> {
        // should collect events from every children
        let target_heights = self.retrieve_split_info(rect.height);
        let Rectangle {
            left, width, height, ..
        } = rect;
        let mut messages = vec![];

        // iterate over the splits
        let mut top = 0;
        for (idx, split) in self.splits.iter().enumerate() {
            let target_height = target_heights[idx];
            let bottom = min(top + target_height, height);
            let sub_rect = Rectangle {
                top,
                left,
                width,
                height: target_height,
            };
            let mut sub_message = adjust_event(event, sub_rect)
                .map(|ev| split.as_ref().on_event(ev, sub_rect.adjust_origin()))
                .unwrap_or_default();
            messages.append(&mut sub_message);
            top = bottom;
        }

        messages
    }

    fn on_event_mut(&mut self, event: Event, rect: Rectangle) -> Vec<Message> {
        // should collect events from every children
        let target_heights = self.retrieve_split_info(rect.height);
        let Rectangle {
            left, width, height, ..
        } = rect;
        let mut messages = vec![];

        // iterate over the splits
        let mut top = 0;
        for (idx, split) in self.splits.iter_mut().enumerate() {
            let target_height = target_heights[idx];
            let bottom = min(top + target_height, height);
            let sub_rect = Rectangle {
                top,
                left,
                width,
                height: target_height,
            };
            let mut sub_message = adjust_event(event, sub_rect)
                .map(|ev| split.as_mut().on_event_mut(ev, sub_rect.adjust_origin()))
                .unwrap_or_default();
            messages.append(&mut sub_message);
            top = bottom;
        }

        messages
    }
}

impl<Message> Split<Message> for VSplit<'_, Message> {
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
    use crate::Result;
    use crate::cell::Cell;
    use crate::key::Key;
    use crate::key::Key::*;
    use crate::key::MouseButton;
    use std::sync::Mutex;

    struct TestCanvas {
        pub width: usize,
        pub height: usize,
    }

    impl Canvas for TestCanvas {
        fn size(&self) -> Result<(usize, usize)> {
            Ok((self.width, self.height))
        }

        fn clear(&mut self) -> Result<()> {
            unimplemented!()
        }

        fn put_cell(&mut self, _row: usize, _col: usize, _cell: Cell) -> Result<usize> {
            unimplemented!()
        }

        fn set_cursor(&mut self, _row: usize, _col: usize) -> Result<()> {
            unimplemented!()
        }

        fn show_cursor(&mut self, _show: bool) -> Result<()> {
            unimplemented!()
        }
    }

    struct WSplit<'a> {
        pub basis: Size,
        pub grow: usize,
        pub shrink: usize,
        pub draw: &'a dyn Draw,
    }

    impl<'a> WSplit<'a> {
        pub fn new(draw: &'a dyn Draw) -> Self {
            Self {
                basis: Size::Default,
                grow: 1,
                shrink: 1,
                draw,
            }
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

    impl Split for WSplit<'_> {
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

    impl Draw for WSplit<'_> {
        fn draw(&self, canvas: &mut dyn Canvas) -> DrawResult<()> {
            self.draw.draw(canvas)
        }
    }

    impl Widget for WSplit<'_> {}

    #[derive(Default)]
    struct SingleWindow {
        pub width: usize,
        pub height: usize,
    }

    impl Draw for SingleWindow {
        fn draw(&self, canvas: &mut dyn Canvas) -> DrawResult<()> {
            let (width, height) = canvas.size().unwrap();
            assert_eq!(self.width, width);
            assert_eq!(self.height, height);
            Ok(())
        }
    }

    #[test]
    fn splits_should_create_on_empty_items() {
        let mut canvas = TestCanvas { width: 80, height: 60 };
        let hsplit = HSplit::<()>::default();
        let vsplit = VSplit::<()>::default();
        let _ = hsplit.draw(&mut canvas);
        let _ = vsplit.draw(&mut canvas);
    }

    #[test]
    fn single_splits_should_take_over_all_spaces() {
        let width = 80;
        let height = 60;
        let mut canvas = TestCanvas { width, height };
        let window = SingleWindow { width, height };
        let hsplit = HSplit::default().split(WSplit::new(&window));
        let vsplit = VSplit::default().split(WSplit::new(&window));
        let _ = hsplit.draw(&mut canvas);
        let _ = vsplit.draw(&mut canvas);
    }

    #[test]
    fn two_splits_should_take_50_percent() {
        let width = 80;
        let height = 60;
        let mut canvas = TestCanvas { width, height };

        let h_window = SingleWindow {
            width: width / 2,
            height,
        };
        let v_window = SingleWindow {
            width,
            height: height / 2,
        };

        let hsplit = HSplit::default()
            .split(WSplit::new(&h_window))
            .split(WSplit::new(&h_window));
        let vsplit = VSplit::default()
            .split(WSplit::new(&v_window))
            .split(WSplit::new(&v_window));

        let _ = hsplit.draw(&mut canvas);
        let _ = vsplit.draw(&mut canvas);
    }

    #[test]
    fn exceeded_should_be_ignored() {
        // |<--     screen width: 80   -->|
        // |<--     60        -->|<--     60        -->|
        // |<--     60        -->|<--     | (will be cut)

        let width = 80;
        let height = 80;
        let mut canvas = TestCanvas { width, height };

        let h_first = SingleWindow { width: 60, height };
        let h_second = SingleWindow { width: 20, height };
        let h_third = SingleWindow { width: 0, height };

        let hsplit = HSplit::default()
            .split(WSplit::new(&h_first).basis(60).shrink(0))
            .split(WSplit::new(&h_second).basis(60).shrink(0))
            .split(WSplit::new(&h_third).basis(60).shrink(0));

        let _ = hsplit.draw(&mut canvas);

        let v_first = SingleWindow { width, height: 60 };
        let v_second = SingleWindow { width, height: 20 };
        let v_third = SingleWindow { width, height: 0 };

        let vsplit = VSplit::default()
            .split(WSplit::new(&v_first).basis(60).shrink(0))
            .split(WSplit::new(&v_second).basis(60).shrink(0))
            .split(WSplit::new(&v_third).basis(60).shrink(0));

        let _ = vsplit.draw(&mut canvas);
    }

    #[test]
    fn grow() {
        // |<--     screen width: 80   -->|
        // 1. 10 (with grow: 1) => 30
        // 2. 10 (with grow: 2) => 50

        let width = 80;
        let height = 80;
        let mut canvas = TestCanvas { width, height };

        let h_first = SingleWindow { width: 30, height };
        let h_second = SingleWindow { width: 50, height };

        let hsplit = HSplit::default()
            .split(WSplit::new(&h_first).basis(10).grow(1))
            .split(WSplit::new(&h_second).basis(10).grow(2));

        let _ = hsplit.draw(&mut canvas);

        let v_first = SingleWindow { width, height: 30 };
        let v_second = SingleWindow { width, height: 50 };

        let vsplit = VSplit::default()
            .split(WSplit::new(&v_first).basis(10).grow(1))
            .split(WSplit::new(&v_second).basis(10).grow(2));

        let _ = vsplit.draw(&mut canvas);
    }

    #[test]
    fn shrink() {
        // |<--     screen width: 80   -->|
        // 1. 70 (with shrink: 1) => 30
        // 2. 70 (with shrink: 2) => 50

        let width = 80;
        let height = 80;
        let mut canvas = TestCanvas { width, height };

        let h_first = SingleWindow { width: 50, height };
        let h_second = SingleWindow { width: 30, height };

        let hsplit = HSplit::default()
            .split(WSplit::new(&h_first).basis(70).shrink(1))
            .split(WSplit::new(&h_second).basis(70).shrink(2));

        let _ = hsplit.draw(&mut canvas);

        let v_first = SingleWindow { width, height: 50 };
        let v_second = SingleWindow { width, height: 30 };

        let vsplit = VSplit::default()
            .split(WSplit::new(&v_first).basis(70).shrink(1))
            .split(WSplit::new(&v_second).basis(70).shrink(2));

        let _ = vsplit.draw(&mut canvas);
    }

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

    impl Split for WinHint {
        fn get_basis(&self) -> Size {
            Size::Default
        }
        fn get_grow(&self) -> usize {
            0
        }
        fn get_shrink(&self) -> usize {
            0
        }
    }

    #[test]
    fn size_hint_of_hsplit() {
        let hint_none = WinHint {
            width_hint: None,
            height_hint: None,
        };
        let hint_width_1 = WinHint {
            width_hint: Some(1),
            height_hint: None,
        };
        let hint_width_2 = WinHint {
            width_hint: Some(2),
            height_hint: None,
        };
        let hint_height_1 = WinHint {
            width_hint: None,
            height_hint: Some(1),
        };
        let hint_height_2 = WinHint {
            width_hint: None,
            height_hint: Some(2),
        };

        // sum(width), max(height)
        let split = HSplit::default()
            .split(&hint_none)
            .split(&hint_width_1)
            .split(&hint_width_2)
            .split(&hint_height_1)
            .split(&hint_height_2);

        assert_eq!((Some(3), Some(2)), split.size_hint());

        // None, max(height)
        let split = HSplit::default()
            .split(&hint_none)
            .split(&hint_height_1)
            .split(&hint_height_2);

        assert_eq!((None, Some(2)), split.size_hint());

        // sum(width), None
        let split = HSplit::default()
            .split(&hint_none)
            .split(&hint_width_1)
            .split(&hint_width_2);
        assert_eq!((Some(3), None), split.size_hint());

        // None
        let split = HSplit::default().split(&hint_none).split(&hint_none);
        assert_eq!((None, None), split.size_hint());
    }

    #[test]
    fn size_hint_of_vsplit() {
        let hint_none = WinHint {
            width_hint: None,
            height_hint: None,
        };
        let hint_width_1 = WinHint {
            width_hint: Some(1),
            height_hint: None,
        };
        let hint_width_2 = WinHint {
            width_hint: Some(2),
            height_hint: None,
        };
        let hint_height_1 = WinHint {
            width_hint: None,
            height_hint: Some(1),
        };
        let hint_height_2 = WinHint {
            width_hint: None,
            height_hint: Some(2),
        };

        // max(width), sum(height)
        let split = VSplit::default()
            .split(&hint_none)
            .split(&hint_width_1)
            .split(&hint_width_2)
            .split(&hint_height_1)
            .split(&hint_height_2);

        assert_eq!((Some(2), Some(3)), split.size_hint());

        // None, sum(height)
        let split = VSplit::default()
            .split(&hint_none)
            .split(&hint_height_1)
            .split(&hint_height_2);

        assert_eq!((None, Some(3)), split.size_hint());

        // max(width), None
        let split = VSplit::default()
            .split(&hint_none)
            .split(&hint_width_1)
            .split(&hint_width_2);
        assert_eq!((Some(2), None), split.size_hint());

        // None
        let split = VSplit::default().split(&hint_none).split(&hint_none);
        assert_eq!((None, None), split.size_hint());
    }

    #[derive(Copy, Clone, PartialOrd, PartialEq, Debug)]
    enum Message {
        Window(i32),
    }

    struct WindowWithId {
        id: i32,
    }

    impl WindowWithId {
        pub fn new(id: i32) -> Self {
            Self { id }
        }
    }

    impl Draw for WindowWithId {
        fn draw(&self, _canvas: &mut dyn Canvas) -> DrawResult<()> {
            unimplemented!()
        }
    }

    impl Widget<Message> for WindowWithId {
        fn on_event(&self, _event: Event, _rect: Rectangle) -> Vec<Message> {
            vec![Message::Window(self.id)]
        }
        fn on_event_mut(&mut self, _event: Event, _rect: Rectangle) -> Vec<Message> {
            vec![Message::Window(self.id)]
        }
    }

    impl Split<Message> for WindowWithId {
        fn get_basis(&self) -> Size {
            Size::Default
        }
        fn get_grow(&self) -> usize {
            1
        }
        fn get_shrink(&self) -> usize {
            1
        }
    }

    #[test]
    fn message_should_be_dispatched_correctly() {
        let width = 80;
        let height = 60;
        let rect = Rectangle {
            top: 0,
            left: 0,
            width,
            height,
        };

        let win1 = WindowWithId::new(1);
        let win2 = WindowWithId::new(2);
        let win3 = WindowWithId::new(3);
        let win4 = WindowWithId::new(4);

        let ev_left_1 = Event::Key(Key::MouseHold(0, 0));
        let ev_left_2 = Event::Key(Key::MouseHold(0, 39));
        let ev_right_1 = Event::Key(Key::MouseHold(20, 40));
        let ev_right_2 = Event::Key(Key::MouseHold(20, 41));
        let ev_right_3 = Event::Key(Key::MouseHold(59, 79));
        let ev_out_of_bound = Event::Key(Key::MouseHold(60, 80));

        let hsplit = HSplit::default().split(&win1).split(&win2);
        let msg = hsplit.on_event(ev_left_1, rect);
        assert!(!msg.is_empty());
        assert_eq!(Message::Window(1), msg[0]);
        let msg = hsplit.on_event(ev_left_2, rect);
        assert!(!msg.is_empty());
        assert_eq!(Message::Window(1), msg[0]);
        let msg = hsplit.on_event(ev_right_1, rect);
        assert!(!msg.is_empty());
        assert_eq!(Message::Window(2), msg[0]);
        let msg = hsplit.on_event(ev_right_2, rect);
        assert!(!msg.is_empty());
        assert_eq!(Message::Window(2), msg[0]);
        let msg = hsplit.on_event(ev_right_3, rect);
        assert!(!msg.is_empty());
        assert_eq!(Message::Window(2), msg[0]);
        let msg = hsplit.on_event(ev_out_of_bound, rect);
        assert!(msg.is_empty());

        let ev_top_1 = Event::Key(Key::MouseHold(0, 0));
        let ev_top_2 = Event::Key(Key::MouseHold(29, 39));
        let ev_bottom_1 = Event::Key(Key::MouseHold(30, 40));
        let ev_bottom_2 = Event::Key(Key::MouseHold(31, 41));
        let ev_bottom_3 = Event::Key(Key::MouseHold(59, 79));
        let ev_out_of_bound = Event::Key(Key::MouseHold(60, 80));

        let vsplit = VSplit::default().split(&win1).split(&win2);

        let msg = vsplit.on_event(ev_top_1, rect);
        assert!(!msg.is_empty());
        assert_eq!(Message::Window(1), msg[0]);
        let msg = vsplit.on_event(ev_top_2, rect);
        assert!(!msg.is_empty());
        assert_eq!(Message::Window(1), msg[0]);
        let msg = vsplit.on_event(ev_bottom_1, rect);
        assert!(!msg.is_empty());
        assert_eq!(Message::Window(2), msg[0]);
        let msg = vsplit.on_event(ev_bottom_2, rect);
        assert!(!msg.is_empty());
        assert_eq!(Message::Window(2), msg[0]);
        let msg = vsplit.on_event(ev_bottom_3, rect);
        assert!(!msg.is_empty());
        assert_eq!(Message::Window(2), msg[0]);
        let msg = vsplit.on_event(ev_out_of_bound, rect);
        assert!(msg.is_empty());

        // 1 | 2
        // --|--
        // 3 | 4
        let nested = HSplit::default()
            .split(VSplit::default().split(&win1).split(&win3))
            .split(VSplit::default().split(&win2).split(&win4));
        let row_col_event = [
            ((0, 0), Message::Window(1)),
            ((0, 39), Message::Window(1)),
            ((29, 0), Message::Window(1)),
            ((29, 39), Message::Window(1)),
            ((0, 40), Message::Window(2)),
            ((0, 79), Message::Window(2)),
            ((29, 40), Message::Window(2)),
            ((29, 79), Message::Window(2)),
            ((30, 0), Message::Window(3)),
            ((30, 39), Message::Window(3)),
            ((59, 0), Message::Window(3)),
            ((59, 39), Message::Window(3)),
            ((30, 40), Message::Window(4)),
            ((30, 79), Message::Window(4)),
            ((59, 40), Message::Window(4)),
            ((59, 79), Message::Window(4)),
        ];

        for &((row, col), event) in row_col_event.iter() {
            let ev = Event::Key(MousePress(MouseButton::Left, row, col));
            let msg = nested.on_event(ev, rect);
            assert_eq!(msg[0], event);
            let ev = Event::Key(MouseRelease(row, col));
            let msg = nested.on_event(ev, rect);
            assert_eq!(msg[0], event);
            let ev = Event::Key(MouseHold(row, col));
            let msg = nested.on_event(ev, rect);
            assert_eq!(msg[0], event);
            let ev = Event::Key(SingleClick(MouseButton::Left, row, col));
            let msg = nested.on_event(ev, rect);
            assert_eq!(msg[0], event);
            let ev = Event::Key(DoubleClick(MouseButton::Left, row, col));
            let msg = nested.on_event(ev, rect);
            assert_eq!(msg[0], event);
            let ev = Event::Key(Key::WheelUp(row, col, 1));
            let msg = nested.on_event(ev, rect);
            assert_eq!(msg[0], event);
            let ev = Event::Key(Key::WheelDown(row, col, 1));
            let msg = nested.on_event(ev, rect);
            assert_eq!(msg[0], event);
        }
    }

    #[test]
    fn message_should_be_dispatched_correctly_mut() {
        let width = 80;
        let height = 60;
        let rect = Rectangle {
            top: 0,
            left: 0,
            width,
            height,
        };

        let mut win1 = WindowWithId::new(1);
        let mut win2 = WindowWithId::new(2);
        let mut win3 = WindowWithId::new(3);
        let mut win4 = WindowWithId::new(4);

        let ev_left_1 = Event::Key(Key::MouseHold(0, 0));
        let ev_left_2 = Event::Key(Key::MouseHold(0, 39));
        let ev_right_1 = Event::Key(Key::MouseHold(20, 40));
        let ev_right_2 = Event::Key(Key::MouseHold(20, 41));
        let ev_right_3 = Event::Key(Key::MouseHold(59, 79));
        let ev_out_of_bound = Event::Key(Key::MouseHold(60, 80));

        {
            let mut hsplit = HSplit::default().split(&mut win1).split(&mut win2);
            let msg = hsplit.on_event_mut(ev_left_1, rect);
            assert!(!msg.is_empty());
            assert_eq!(Message::Window(1), msg[0]);
            let msg = hsplit.on_event_mut(ev_left_2, rect);
            assert!(!msg.is_empty());
            assert_eq!(Message::Window(1), msg[0]);
            let msg = hsplit.on_event_mut(ev_right_1, rect);
            assert!(!msg.is_empty());
            assert_eq!(Message::Window(2), msg[0]);
            let msg = hsplit.on_event_mut(ev_right_2, rect);
            assert!(!msg.is_empty());
            assert_eq!(Message::Window(2), msg[0]);
            let msg = hsplit.on_event_mut(ev_right_3, rect);
            assert!(!msg.is_empty());
            assert_eq!(Message::Window(2), msg[0]);
            let msg = hsplit.on_event_mut(ev_out_of_bound, rect);
            assert!(msg.is_empty());
        }

        let ev_top_1 = Event::Key(Key::MouseHold(0, 0));
        let ev_top_2 = Event::Key(Key::MouseHold(29, 39));
        let ev_bottom_1 = Event::Key(Key::MouseHold(30, 40));
        let ev_bottom_2 = Event::Key(Key::MouseHold(31, 41));
        let ev_bottom_3 = Event::Key(Key::MouseHold(59, 79));
        let ev_out_of_bound = Event::Key(Key::MouseHold(60, 80));

        {
            let mut vsplit = VSplit::default().split(&mut win1).split(&mut win2);

            let msg = vsplit.on_event_mut(ev_top_1, rect);
            assert!(!msg.is_empty());
            assert_eq!(Message::Window(1), msg[0]);
            let msg = vsplit.on_event_mut(ev_top_2, rect);
            assert!(!msg.is_empty());
            assert_eq!(Message::Window(1), msg[0]);
            let msg = vsplit.on_event_mut(ev_bottom_1, rect);
            assert!(!msg.is_empty());
            assert_eq!(Message::Window(2), msg[0]);
            let msg = vsplit.on_event_mut(ev_bottom_2, rect);
            assert!(!msg.is_empty());
            assert_eq!(Message::Window(2), msg[0]);
            let msg = vsplit.on_event_mut(ev_bottom_3, rect);
            assert!(!msg.is_empty());
            assert_eq!(Message::Window(2), msg[0]);
            let msg = vsplit.on_event_mut(ev_out_of_bound, rect);
            assert!(msg.is_empty());
        }

        // 1 | 2
        // --|--
        // 3 | 4
        {
            let mut nested = HSplit::default()
                .split(VSplit::default().split(&mut win1).split(&mut win3))
                .split(VSplit::default().split(&mut win2).split(&mut win4));
            let row_col_event = [
                ((0, 0), Message::Window(1)),
                ((0, 39), Message::Window(1)),
                ((29, 0), Message::Window(1)),
                ((29, 39), Message::Window(1)),
                ((0, 40), Message::Window(2)),
                ((0, 79), Message::Window(2)),
                ((29, 40), Message::Window(2)),
                ((29, 79), Message::Window(2)),
                ((30, 0), Message::Window(3)),
                ((30, 39), Message::Window(3)),
                ((59, 0), Message::Window(3)),
                ((59, 39), Message::Window(3)),
                ((30, 40), Message::Window(4)),
                ((30, 79), Message::Window(4)),
                ((59, 40), Message::Window(4)),
                ((59, 79), Message::Window(4)),
            ];

            for &((row, col), event) in row_col_event.iter() {
                let ev = Event::Key(MousePress(MouseButton::Left, row, col));
                let msg = nested.on_event_mut(ev, rect);
                assert_eq!(msg[0], event);
                let ev = Event::Key(MouseRelease(row, col));
                let msg = nested.on_event_mut(ev, rect);
                assert_eq!(msg[0], event);
                let ev = Event::Key(MouseHold(row, col));
                let msg = nested.on_event_mut(ev, rect);
                assert_eq!(msg[0], event);
                let ev = Event::Key(SingleClick(MouseButton::Left, row, col));
                let msg = nested.on_event_mut(ev, rect);
                assert_eq!(msg[0], event);
                let ev = Event::Key(DoubleClick(MouseButton::Left, row, col));
                let msg = nested.on_event_mut(ev, rect);
                assert_eq!(msg[0], event);
                let ev = Event::Key(Key::WheelUp(row, col, 1));
                let msg = nested.on_event_mut(ev, rect);
                assert_eq!(msg[0], event);
                let ev = Event::Key(Key::WheelDown(row, col, 1));
                let msg = nested.on_event_mut(ev, rect);
                assert_eq!(msg[0], event);
            }
        }
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

    impl Split for Drawn {
        fn get_basis(&self) -> Size {
            Size::Default
        }

        fn get_grow(&self) -> usize {
            1
        }

        fn get_shrink(&self) -> usize {
            1
        }
    }

    #[test]
    fn mutable_widget() {
        let mut canvas = TestCanvas { width: 80, height: 80 };

        let mut mutable = Drawn {
            called: Mutex::new(Called::No),
        };
        {
            let mut hsplit = HSplit::default().split(&mut mutable);
            hsplit.draw_mut(&mut canvas).unwrap();
        }
        assert_eq!(Called::Mut, *mutable.called.lock().unwrap());

        let mut mutable = Drawn {
            called: Mutex::new(Called::No),
        };
        {
            let mut vsplit = VSplit::default().split(&mut mutable);
            vsplit.draw_mut(&mut canvas).unwrap();
        }
        assert_eq!(Called::Mut, *mutable.called.lock().unwrap());

        let immutable = Drawn {
            called: Mutex::new(Called::No),
        };
        let hsplit = HSplit::default().split(&immutable);
        hsplit.draw(&mut canvas).unwrap();
        assert_eq!(Called::Immut, *immutable.called.lock().unwrap());
        let immutable = Drawn {
            called: Mutex::new(Called::No),
        };
        let vsplit = VSplit::default().split(&immutable);
        vsplit.draw(&mut canvas).unwrap();
        assert_eq!(Called::Immut, *immutable.called.lock().unwrap());
    }
}
