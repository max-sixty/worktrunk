// copy from https://docs.rs/crate/termion/1.5.1/source/src/sys/unix/mod.rs
use std::io;
pub mod file;
pub mod signal;
pub mod size;

trait IsMinusOne {
    fn is_minus_one(&self) -> bool;
}

macro_rules! impl_is_minus_one {
        ($($t:ident)*) => ($(impl IsMinusOne for $t {
            fn is_minus_one(&self) -> bool {
                *self == -1
            }
        })*)
    }

impl_is_minus_one! { i8 i16 i32 i64 isize }

fn cvt<T: IsMinusOne>(t: T) -> io::Result<T> {
    if t.is_minus_one() {
        Err(io::Error::last_os_error())
    } else {
        Ok(t)
    }
}
