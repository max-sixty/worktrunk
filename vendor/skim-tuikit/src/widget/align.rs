pub trait AlignSelf {
    /// say horizontal align, given container's (start, end) and self's size
    /// Adjust the actual start position of self.
    ///
    /// Note that if the container's size < self_size, will return `start`
    fn adjust(&self, start: usize, end_exclusive: usize, self_size: usize) -> usize;
}

pub enum HorizontalAlign {
    Left,
    Center,
    Right,
}

pub enum VerticalAlign {
    Top,
    Middle,
    Bottom,
}

impl AlignSelf for HorizontalAlign {
    fn adjust(&self, start: usize, end: usize, self_size: usize) -> usize {
        if start >= end {
            // wrong input
            return start;
        }
        let container_size = end - start;
        if container_size <= self_size {
            return start;
        }

        match self {
            HorizontalAlign::Left => start,
            HorizontalAlign::Center => start + (container_size - self_size) / 2,
            HorizontalAlign::Right => end - self_size,
        }
    }
}

impl AlignSelf for VerticalAlign {
    fn adjust(&self, start: usize, end: usize, self_size: usize) -> usize {
        if start >= end {
            // wrong input
            return start;
        }
        let container_size = end - start;
        if container_size <= self_size {
            return start;
        }

        match self {
            VerticalAlign::Top => start,
            VerticalAlign::Middle => start + (container_size - self_size) / 2,
            VerticalAlign::Bottom => end - self_size,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::widget::align::{AlignSelf, HorizontalAlign, VerticalAlign};

    #[test]
    fn size_lt0_return_start() {
        assert_eq!(0, HorizontalAlign::Left.adjust(0, 0, 2));
        assert_eq!(0, HorizontalAlign::Center.adjust(0, 0, 2));
        assert_eq!(0, HorizontalAlign::Right.adjust(0, 0, 2));
        assert_eq!(0, VerticalAlign::Top.adjust(0, 0, 2));
        assert_eq!(0, VerticalAlign::Middle.adjust(0, 0, 2));
        assert_eq!(0, VerticalAlign::Bottom.adjust(0, 0, 2));

        assert_eq!(2, HorizontalAlign::Left.adjust(2, 0, 2));
        assert_eq!(2, HorizontalAlign::Center.adjust(2, 0, 2));
        assert_eq!(2, HorizontalAlign::Right.adjust(2, 0, 2));
        assert_eq!(2, VerticalAlign::Top.adjust(2, 0, 2));
        assert_eq!(2, VerticalAlign::Middle.adjust(2, 0, 2));
        assert_eq!(2, VerticalAlign::Bottom.adjust(2, 0, 2));
    }

    #[test]
    fn container_size_too_small_return_start() {
        assert_eq!(2, HorizontalAlign::Left.adjust(2, 3, 2));
        assert_eq!(2, HorizontalAlign::Center.adjust(2, 3, 2));
        assert_eq!(2, HorizontalAlign::Right.adjust(2, 3, 2));
        assert_eq!(2, VerticalAlign::Top.adjust(2, 3, 2));
        assert_eq!(2, VerticalAlign::Middle.adjust(2, 3, 2));
        assert_eq!(2, VerticalAlign::Bottom.adjust(2, 3, 2));
    }

    #[test]
    fn align_start() {
        assert_eq!(2, HorizontalAlign::Left.adjust(2, 8, 2));
        assert_eq!(2, VerticalAlign::Top.adjust(2, 8, 2));
        assert_eq!(2, HorizontalAlign::Left.adjust(2, 7, 2));
        assert_eq!(2, VerticalAlign::Top.adjust(2, 7, 2));
        assert_eq!(2, HorizontalAlign::Left.adjust(2, 8, 3));
        assert_eq!(2, VerticalAlign::Top.adjust(2, 8, 3));
    }

    #[test]
    fn align_end() {
        assert_eq!(6, HorizontalAlign::Right.adjust(2, 8, 2));
        assert_eq!(6, VerticalAlign::Bottom.adjust(2, 8, 2));
        assert_eq!(5, HorizontalAlign::Right.adjust(2, 7, 2));
        assert_eq!(5, VerticalAlign::Bottom.adjust(2, 7, 2));
        assert_eq!(5, HorizontalAlign::Right.adjust(2, 8, 3));
        assert_eq!(5, VerticalAlign::Bottom.adjust(2, 8, 3));
    }

    #[test]
    fn align_center() {
        assert_eq!(4, HorizontalAlign::Center.adjust(2, 8, 2));
        assert_eq!(4, VerticalAlign::Middle.adjust(2, 8, 2));
        assert_eq!(3, HorizontalAlign::Center.adjust(2, 7, 2));
        assert_eq!(3, VerticalAlign::Middle.adjust(2, 7, 2));
        assert_eq!(3, HorizontalAlign::Center.adjust(2, 8, 3));
        assert_eq!(3, VerticalAlign::Middle.adjust(2, 8, 3));
    }
}
