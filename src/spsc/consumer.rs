use std::marker::PhantomData;

use crate::mem::Allocation;
use crate::shim::cell::Cell;
use crate::shim::sync::Arc;
use crate::shim::sync::atomic::AtomicUsize;
use crate::spsc::queue::Queue;

#[repr(C, align(128))]
pub(super) struct ConsumerState {
    pub head: AtomicUsize,
    pub cached_tail: Cell<usize>,
}

pub struct Consumer<T, AllocT: Allocation<T>> {
    inner: Arc<Queue<T, AllocT>>,
    _not_sync: PhantomData<*const ()>,
}

unsafe impl<T: Send, AllocT: Allocation<T> + Send> Send for Consumer<T, AllocT> {}

impl<T, AllocT: Allocation<T>> Consumer<T, AllocT> {
    pub(super) fn new(queue: Arc<Queue<T, AllocT>>) -> Self {
        Self {
            inner: queue,
            _not_sync: PhantomData,
        }
    }

    #[inline]
    pub fn pop(&self) -> Option<T> {
        self.inner.pop()
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::shim::cell::UnsafeCell;

    struct NeverAlloc;
    impl<T> Allocation<T> for NeverAlloc {
        fn ptr(&self) -> *mut UnsafeCell<std::mem::MaybeUninit<T>> {
            unreachable!("it's just a stub")
        }
    }

    // Shouldn't be possible to construct Arc<Consumer<T>> and then use it from different threads as it
    // will break the requirement of *Single* producer *Single* consumer queue.
    static_assertions::assert_not_impl_any!(Consumer<u32, NeverAlloc>: Sync);
}
