use std::marker::PhantomData;

use crate::shim::cell::Cell;
use crate::shim::sync::Arc;
use crate::shim::sync::atomic::AtomicUsize;
use crate::spsc::queue::Queue;

#[repr(C, align(128))]
pub(super) struct ProducerState {
    pub tail: AtomicUsize,
    pub cached_head: Cell<usize>,
}

pub struct Producer<T> {
    inner: Arc<Queue<T>>,
    _not_sync: PhantomData<*const ()>,
}

unsafe impl<T: Send> Send for Producer<T> {}

impl<T> Producer<T> {
    pub(super) fn new(queue: Arc<Queue<T>>) -> Self {
        Self {
            inner: queue,
            _not_sync: PhantomData,
        }
    }

    #[inline]
    pub fn push(&self, item: T) -> Option<T> {
        self.inner.push(item)
    }
}
