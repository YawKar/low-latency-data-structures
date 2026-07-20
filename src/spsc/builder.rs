use bon::Builder;

use crate::mem::Allocator;
use crate::mem::global::{GlobalAllocator, GlobalAllocatorOptions};

/// Construction-time options for [`new`](super::new).
///
/// Wraps the allocator-specific options so the same `new` signature works for
/// every [`Allocator`] impl. Use [`Options::builder`] to build one explicitly,
/// or [`Options::global_mlocked`] for the common global + `mlock` case.
#[derive(Builder)]
pub struct Options<Alloc: Allocator> {
    pub(crate) alloc: Alloc::Options,
}

impl Options<GlobalAllocator> {
    /// Convenience shorthand for the common case: the global allocator with
    /// `mlock` enabled.
    pub fn global_mlocked() -> Self {
        Self {
            alloc: GlobalAllocatorOptions::builder().mlock(true).build(),
        }
    }
}
