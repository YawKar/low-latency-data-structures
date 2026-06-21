#[cfg(not(feature = "tests_loom"))]
pub use std::{alloc, sync};

#[cfg(feature = "tests_loom")]
pub use loom::*;

#[cfg(not(feature = "tests_loom"))]
pub mod cell {
    pub use std::cell::*;

    #[derive(Debug)]
    pub(crate) struct UnsafeCell<T>(std::cell::UnsafeCell<T>);

    impl<T> UnsafeCell<T> {
        pub(crate) fn with_mut<R>(&self, f: impl FnOnce(*mut T) -> R) -> R {
            f(self.0.get())
        }
    }
}
