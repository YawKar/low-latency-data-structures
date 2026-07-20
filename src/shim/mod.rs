#[cfg(not(feature = "tests_loom"))]
pub use std::{alloc, sync};

#[cfg(feature = "tests_loom")]
pub use loom::*;
#[cfg(feature = "tests_loom")]
pub mod alloc {
    pub use std::alloc::handle_alloc_error;

    pub use loom::alloc::*;
}

#[cfg(not(feature = "tests_loom"))]
pub mod cell {
    pub use std::cell::*;

    #[derive(Debug)]
    #[repr(transparent)]
    pub struct UnsafeCell<T>(std::cell::UnsafeCell<T>);

    impl<T> UnsafeCell<T> {
        pub(crate) fn new(value: T) -> Self {
            Self(std::cell::UnsafeCell::new(value))
        }

        pub(crate) fn with_mut<R>(&self, f: impl FnOnce(*mut T) -> R) -> R {
            f(self.0.get())
        }

        /// Escape hatch for algorithms that don't use loom
        pub(crate) fn get(&self) -> *mut T {
            self.0.get()
        }
    }
}
