use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crate::spmc::queue::Queue;

#[repr(C, align(128))]
pub(super) struct ProducerState {
    pub(super) write_cursor: AtomicUsize,
}

/// The publishing handle of an SPMC broadcast queue.
///
/// Created together with its [`Consumer`](crate::spmc::Consumer)s by
/// [`new`](crate::spmc::new). A `Producer` is [`Send`] but not [`Sync`]:
/// at most one thread may publish at a time. To enforce that, the type is
/// neither [`Clone`] nor [`Copy`].
///
/// Publishing never blocks and never allocates: when the queue is full the
/// oldest slot is overwritten, and any consumer that hasn't read past that
/// slot will observe a [`ReadResult::Lapped`](crate::spmc::ReadResult::Lapped)
/// on its next read.
pub struct Producer<T: bytemuck::AnyBitPattern, const CAPACITY: usize> {
    inner: Arc<Queue<T, CAPACITY>>,
    _not_sync: PhantomData<*const ()>,
}

impl<T: bytemuck::AnyBitPattern, const CAPACITY: usize> std::fmt::Debug for Producer<T, CAPACITY> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Producer")
            .field("capacity", &CAPACITY)
            .finish_non_exhaustive()
    }
}

// SAFETY: it is Send on its own but we need to restrict only Sync.
unsafe impl<T: bytemuck::AnyBitPattern, const CAPACITY: usize> Send for Producer<T, CAPACITY> {}

static_assertions::assert_impl_all!(Producer<u32, 1>: Send);
static_assertions::assert_not_impl_any!(Producer<u32, 1>: Sync, Clone, Copy);

impl<T, const CAPACITY: usize> Producer<T, CAPACITY>
where
    T: bytemuck::AnyBitPattern,
{
    pub(super) fn new(queue: Arc<Queue<T, CAPACITY>>) -> Self {
        Self {
            inner: queue,
            _not_sync: PhantomData,
        }
    }

    /// Publishes `value` into the next slot.
    ///
    /// Wait-free. Never blocks, never allocates. If the queue is full, the
    /// oldest slot is silently overwritten; consumers that haven't read past
    /// that slot will observe a [`ReadResult::Lapped`](crate::spmc::ReadResult::Lapped)
    /// on their next [`try_read`](crate::spmc::Consumer::try_read).
    ///
    /// # Examples
    ///
    /// ```
    /// use low_latency_data_structures::spmc::{ReadResult, new};
    ///
    /// let (producer, [mut consumer]) = new::<u64, 16, 1>();
    /// producer.publish(42);
    /// assert_eq!(consumer.try_read(), ReadResult::Value(42));
    /// ```
    #[inline]
    pub fn publish(&self, value: T) {
        self.inner.publish(value)
    }
}
