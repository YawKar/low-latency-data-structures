use std::marker::PhantomData;

use crate::shim::cell::Cell;
use crate::shim::sync::Arc;
use crate::shim::sync::atomic::AtomicUsize;
use crate::spsc::queue::Queue;

#[repr(C, align(128))]
pub(super) struct ConsumerState {
    pub head: AtomicUsize,
    pub cached_tail: Cell<usize>,
}

pub struct Consumer<T> {
    inner: Arc<Queue<T>>,
    _not_sync: PhantomData<*const ()>,
}

unsafe impl<T: Send> Send for Consumer<T> {}

impl<T> Consumer<T> {
    pub(super) fn new(queue: Arc<Queue<T>>) -> Self {
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
