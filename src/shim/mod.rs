#[cfg(not(feature = "test_loom"))]
pub use std::{alloc, sync};

#[cfg(feature = "test_loom")]
pub use loom::*;

#[cfg(not(feature = "test_loom"))]
pub mod cell {
    pub use std::cell::*;

    #[derive(Debug)]
    pub(crate) struct UnsafeCell<T>(std::cell::UnsafeCell<T>);

    impl<T> UnsafeCell<T> {
        pub(crate) fn new(data: T) -> UnsafeCell<T> {
            UnsafeCell(std::cell::UnsafeCell::new(data))
        }

        pub(crate) fn with<R>(&self, f: impl FnOnce(*const T) -> R) -> R {
            f(self.0.get())
        }

        pub(crate) fn with_mut<R>(&self, f: impl FnOnce(*mut T) -> R) -> R {
            f(self.0.get())
        }

        pub(crate) fn get_mut(&mut self) -> &mut T {
            self.0.get_mut()
        }

        pub(crate) fn get(&self) -> *mut T {
            self.0.get()
        }
    }
}
