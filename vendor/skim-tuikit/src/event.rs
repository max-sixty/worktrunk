//! events a `Term` could return

pub use crate::key::Key;

#[derive(Eq, PartialEq, Hash, Debug, Copy, Clone)]
pub enum Event<UserEvent: Send + 'static = ()> {
    Key(Key),
    Resize {
        width: usize,
        height: usize,
    },
    Restarted,
    /// user defined signal 1
    User(UserEvent),

    #[doc(hidden)]
    __Nonexhaustive,
}
