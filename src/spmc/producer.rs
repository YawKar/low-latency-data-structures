use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crate::spmc::queue::Queue;

#[repr(C, align(128))]
pub(super) struct ProducerState {
    pub(super) write_cursor: AtomicUsize,
}

pub struct Producer<T: bytemuck::AnyBitPattern, const CAPACITY: usize, const NCONSUMERS: usize> {
    inner: Arc<Queue<T, CAPACITY, NCONSUMERS>>,
    _not_sync: PhantomData<*const ()>,
}

// SAFETY: it is Send on its own but we need to restrict only Sync.
unsafe impl<T: bytemuck::AnyBitPattern, const CAPACITY: usize, const NCONSUMERS: usize> Send
    for Producer<T, CAPACITY, NCONSUMERS>
{
}

static_assertions::assert_impl_all!(Producer<u32, 1, 1>: Send);
static_assertions::assert_not_impl_any!(Producer<u32, 1, 1>: Sync, Clone, Copy);

impl<T, const CAPACITY: usize, const NCONSUMERS: usize> Producer<T, CAPACITY, NCONSUMERS>
where
    T: bytemuck::AnyBitPattern,
{
    pub(super) fn new(queue: Arc<Queue<T, CAPACITY, NCONSUMERS>>) -> Self {
        Self {
            inner: queue,
            _not_sync: PhantomData,
        }
    }

    #[inline]
    pub fn publish(&self, value: T) {
        self.inner.publish(value)
    }
}
