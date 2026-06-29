use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crate::spmc::queue::Queue;

#[repr(C, align(128))]
pub(super) struct ProducerState {
    pub(super) write_cursor: AtomicUsize,
}

pub struct Producer<T: bytemuck::AnyBitPattern, const CAPACITY: usize, const NCONSUMERS: usize> {
    inner: Arc<Queue<T, CAPACITY, NCONSUMERS>>,
}

impl<T, const CAPACITY: usize, const NCONSUMERS: usize> Producer<T, CAPACITY, NCONSUMERS>
where
    T: bytemuck::AnyBitPattern,
{
    pub(super) fn new(queue: Arc<Queue<T, CAPACITY, NCONSUMERS>>) -> Self {
        Self { inner: queue }
    }

    #[inline]
    pub fn publish(&self, value: T) {
        self.inner.publish(value)
    }
}
