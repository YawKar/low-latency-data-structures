use std::marker::PhantomData;

use crate::mem::Allocation;
use crate::shim::cell::Cell;
use crate::shim::sync::Arc;
use crate::shim::sync::atomic::AtomicUsize;
use crate::spsc::queue::Queue;

#[repr(C, align(128))]
#[derive(Default)]
pub(super) struct ConsumerState {
    pub head: AtomicUsize,
    pub cached_tail: Cell<usize>,
}

/// The popping handle of an SPSC FIFO queue.
///
/// Created together with its paired [`Producer`](crate::spsc::Producer) by
/// [`new`](crate::spsc::new).
/// `Consumer` is [`Send`] but not [`Sync`]: at most one thread may pop at a
/// time.
pub struct Consumer<T, const CAPACITY: usize, AllocT: Allocation<T>> {
    inner: Arc<Queue<T, CAPACITY, AllocT>>,
    _not_sync: PhantomData<*const ()>,
}

impl<T, const CAPACITY: usize, AllocT: Allocation<T>> std::fmt::Debug
    for Consumer<T, CAPACITY, AllocT>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Consumer")
            .field("capacity", &CAPACITY)
            .finish_non_exhaustive()
    }
}

// SAFETY: Consumer is Send because the underlying Queue is Send when both T
// and the allocation are Send; PhantomData<*const ()> blocks Sync.
unsafe impl<T: Send, const CAPACITY: usize, AllocT: Allocation<T> + Send> Send
    for Consumer<T, CAPACITY, AllocT>
{
}

impl<T, const CAPACITY: usize, AllocT: Allocation<T>> Consumer<T, CAPACITY, AllocT> {
    pub(super) fn new(queue: Arc<Queue<T, CAPACITY, AllocT>>) -> Self {
        Self {
            inner: queue,
            _not_sync: PhantomData,
        }
    }

    /// Pops the next item from the queue.
    ///
    /// Wait-free. Returns `Some(item)` on success, `None` if the queue is
    /// currently empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use low_latency_data_structures::spsc::{self, new};
    /// use low_latency_data_structures::mem::global::GlobalAllocator;
    ///
    /// let (producer, consumer) = new::<u64, 4, GlobalAllocator>(
    ///     spsc::Options::global_mlocked(),
    /// );
    /// assert_eq!(consumer.pop(), None);
    /// let _ = producer.push(42);
    /// assert_eq!(consumer.pop(), Some(42));
    /// ```
    #[inline]
    #[must_use = "ignoring the popped item silently drops it"]
    pub fn pop(&self) -> Option<T> {
        self.inner.pop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::test_util::NeverAlloc;

    // Shouldn't be possible to construct Arc<Consumer<T>> and then use it from different threads as it
    // will break the requirement of *Single* producer *Single* consumer queue.
    static_assertions::assert_not_impl_any!(Consumer<u32, 0, NeverAlloc>: Sync);
}
