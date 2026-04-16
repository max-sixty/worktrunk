use std::{io, mem};

use super::cvt;
use nix::libc::{TIOCGWINSZ, c_int, c_ushort, ioctl};

#[repr(C)]
struct TermSize {
    row: c_ushort,
    col: c_ushort,
    _x: c_ushort,
    _y: c_ushort,
}

/// Get the size of the terminal.
pub fn terminal_size(fd: c_int) -> io::Result<(usize, usize)> {
    unsafe {
        let mut size: TermSize = mem::zeroed();
        cvt(ioctl(fd, TIOCGWINSZ, &mut size as *mut _))?;
        Ok((size.col as usize, size.row as usize))
    }
}
