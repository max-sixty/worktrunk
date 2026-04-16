//! Buffering screen cells and try to optimize rendering contents
use crate::Result;
use crate::attr::Attr;
use crate::canvas::Canvas;
use crate::cell::Cell;
use crate::error::TuikitError;
use crate::output::Command;
use std::cmp::{max, min};
use unicode_width::UnicodeWidthChar;

// much of the code comes from https://github.com/agatan/termfest/blob/master/src/screen.rs

/// A Screen is a table of cells to draw on.
/// It's a buffer holding the contents
#[derive(Debug)]
pub struct Screen {
    width: usize,
    height: usize,
    cursor: Cursor,
    cells: Vec<Cell>,
    painted_cells: Vec<Cell>,
    painted_cursor: Cursor,
    clear_on_start: bool,
}

impl Screen {
    /// create an empty screen with size: (width, height)
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![Cell::default(); width * height],
            cursor: Cursor::default(),
            painted_cells: vec![Cell::default(); width * height],
            painted_cursor: Cursor::default(),
            clear_on_start: false,
        }
    }

    pub fn clear_on_start(&mut self, clear_on_start: bool) {
        self.clear_on_start = clear_on_start;
    }

    /// get the width of the screen
    #[inline]
    pub fn width(&self) -> usize {
        self.width
    }

    /// get the height of the screen
    #[inline]
    pub fn height(&self) -> usize {
        self.height
    }

    #[inline]
    fn index(&self, row: usize, col: usize) -> Result<usize> {
        if row >= self.height || col >= self.width {
            Err(TuikitError::IndexOutOfBound(row, col))
        } else {
            Ok(row * self.width + col)
        }
    }

    fn empty_canvas(&self, width: usize, height: usize) -> Vec<Cell> {
        vec![Cell::empty(); width * height]
    }

    fn copy_cells(&self, original: &[Cell], width: usize, height: usize) -> Vec<Cell> {
        let mut new_cells = self.empty_canvas(width, height);
        use std::cmp;
        let min_height = cmp::min(height, self.height);
        let min_width = cmp::min(width, self.width);
        for row in 0..min_height {
            let orig_start = row * self.width;
            let orig_end = min_width + orig_start;
            let start = row * width;
            let end = min_width + start;
            new_cells[start..end].copy_from_slice(&original[orig_start..orig_end]);
        }
        new_cells
    }

    /// to resize the screen to `(width, height)`
    pub fn resize(&mut self, width: usize, height: usize) {
        self.cells = self.copy_cells(&self.cells, width, height);
        self.painted_cells = self.empty_canvas(width, height);
        self.width = width;
        self.height = height;

        self.cursor.row = min(self.cursor.row, height);
        self.cursor.col = min(self.cursor.col, width);
    }

    /// sync internal buffer with the terminal
    pub fn present(&mut self) -> Vec<Command> {
        let mut commands = Vec::with_capacity(2048);
        let default_attr = Attr::default();
        let mut last_attr = default_attr;

        // hide cursor && reset Attributes
        commands.push(Command::CursorShow(false));
        commands.push(Command::CursorGoto { row: 0, col: 0 });
        commands.push(Command::ResetAttributes);

        let mut last_cursor = Cursor::default();

        for row in 0..self.height {
            // calculate the last col that has contents
            let mut empty_col_index = 0;
            for col in (0..self.width).rev() {
                let index = self.index(row, col).unwrap();
                let cell = &self.cells[index];
                if cell.is_empty() {
                    self.painted_cells[index] = *cell;
                } else {
                    empty_col_index = col + 1;
                    break;
                }
            }

            // compare cells and print necessary escape codes
            let mut last_ch_is_wide = false;
            for col in 0..empty_col_index {
                let index = self.index(row, col).unwrap();

                // advance if the last character is wide
                if last_ch_is_wide {
                    last_ch_is_wide = false;
                    self.painted_cells[index] = self.cells[index];
                    continue;
                }

                let cell_to_paint = self.cells[index];
                let cell_painted = self.painted_cells[index];

                // no need to paint if the content did not change
                if cell_to_paint == cell_painted {
                    continue;
                }

                // move cursor if necessary
                if last_cursor.row != row || last_cursor.col != col {
                    commands.push(Command::CursorGoto { row, col });
                }

                if cell_to_paint.attr != last_attr {
                    commands.push(Command::ResetAttributes);
                    commands.push(Command::SetAttribute(cell_to_paint.attr));
                    last_attr = cell_to_paint.attr;
                }

                // correctly draw the characters
                match cell_to_paint.ch {
                    '\n' | '\r' | '\t' | '\0' => {
                        commands.push(Command::PutChar(' '));
                    }
                    _ => {
                        commands.push(Command::PutChar(cell_to_paint.ch));
                    }
                }

                let display_width = cell_to_paint.ch.width().unwrap_or(2);

                // wide character
                if display_width == 2 {
                    last_ch_is_wide = true;
                }

                last_cursor.row = row;
                last_cursor.col = col + display_width;
                self.painted_cells[index] = cell_to_paint;
            }

            if empty_col_index != self.width {
                commands.push(Command::CursorGoto {
                    row,
                    col: empty_col_index,
                });
                commands.push(Command::ResetAttributes);
                if self.clear_on_start {
                    commands.push(Command::EraseEndOfLine);
                }
                last_attr = Attr::default();
            }
        }

        // restore cursor
        commands.push(Command::CursorGoto {
            row: self.cursor.row,
            col: self.cursor.col,
        });
        if self.cursor.visible {
            commands.push(Command::CursorShow(true));
        }

        self.painted_cursor = self.cursor;

        commands
    }

    /// ```
    /// use skim_tuikit::cell::Cell;
    /// use skim_tuikit::canvas::Canvas;
    /// use skim_tuikit::screen::Screen;
    ///
    ///
    /// let mut screen = Screen::new(1, 1);
    /// screen.put_cell(0, 0, Cell{ ch: 'a', ..Cell::default()});
    /// let mut iter = screen.iter_cell();
    /// assert_eq!(Some((0, 0, &Cell{ ch: 'a', ..Cell::default()})), iter.next());
    /// assert_eq!(None, iter.next());
    /// ```
    pub fn iter_cell(&self) -> CellIterator<'_> {
        CellIterator {
            width: self.width,
            index: 0,
            vec: &self.cells,
        }
    }
}

impl Canvas for Screen {
    /// Get the canvas size (width, height)
    fn size(&self) -> Result<(usize, usize)> {
        Ok((self.width(), self.height()))
    }

    /// clear the screen buffer
    fn clear(&mut self) -> Result<()> {
        for cell in self.cells.iter_mut() {
            *cell = Cell::empty();
        }
        Ok(())
    }

    /// change a cell of position `(row, col)` to `cell`
    fn put_cell(&mut self, row: usize, col: usize, cell: Cell) -> Result<usize> {
        let ch_width = cell.ch.width().unwrap_or(2);
        if ch_width > 1 {
            let _ = self.index(row, col + 1).map(|index| {
                self.cells[index - 1] = cell;
                self.cells[index].ch = ' ';
            });
        } else {
            let _ = self.index(row, col).map(|index| {
                self.cells[index] = cell;
            });
        }
        Ok(ch_width)
    }

    /// move cursor position (row, col) and show cursor
    fn set_cursor(&mut self, row: usize, col: usize) -> Result<()> {
        self.cursor.row = min(row, max(self.height, 1) - 1);
        self.cursor.col = min(col, max(self.width, 1) - 1);
        self.cursor.visible = true;
        Ok(())
    }

    /// show/hide cursor, set `show` to `false` to hide the cursor
    fn show_cursor(&mut self, show: bool) -> Result<()> {
        self.cursor.visible = show;
        Ok(())
    }
}

pub struct CellIterator<'a> {
    width: usize,
    index: usize,
    vec: &'a Vec<Cell>,
}

impl<'a> Iterator for CellIterator<'a> {
    type Item = (usize, usize, &'a Cell);

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.vec.len() {
            return None;
        }

        let (row, col) = (self.index / self.width, self.index % self.width);
        let ret = self.vec.get(self.index).map(|cell| (row, col, cell));
        self.index += 1;
        ret
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Cursor {
    pub row: usize,
    pub col: usize,
    visible: bool,
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_cell_iterator() {
        let mut screen = Screen::new(2, 2);
        let _ = screen.put_cell(
            0,
            0,
            Cell {
                ch: 'a',
                attr: Attr::default(),
            },
        );
        let _ = screen.put_cell(
            0,
            1,
            Cell {
                ch: 'b',
                attr: Attr::default(),
            },
        );
        let _ = screen.put_cell(
            1,
            0,
            Cell {
                ch: 'c',
                attr: Attr::default(),
            },
        );
        let _ = screen.put_cell(
            1,
            1,
            Cell {
                ch: 'd',
                attr: Attr::default(),
            },
        );

        let mut iter = screen.iter_cell();
        assert_eq!(
            Some((
                0,
                0,
                &Cell {
                    ch: 'a',
                    attr: Attr::default()
                }
            )),
            iter.next()
        );
        assert_eq!(
            Some((
                0,
                1,
                &Cell {
                    ch: 'b',
                    attr: Attr::default()
                }
            )),
            iter.next()
        );
        assert_eq!(
            Some((
                1,
                0,
                &Cell {
                    ch: 'c',
                    attr: Attr::default()
                }
            )),
            iter.next()
        );
        assert_eq!(
            Some((
                1,
                1,
                &Cell {
                    ch: 'd',
                    attr: Attr::default()
                }
            )),
            iter.next()
        );
        assert_eq!(None, iter.next());

        let empty_screen = Screen::new(0, 0);
        let mut empty_iter = empty_screen.iter_cell();
        assert_eq!(None, empty_iter.next());
    }
}
