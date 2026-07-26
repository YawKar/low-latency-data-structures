use std::marker::PhantomData;

use crossbeam_utils::CachePadded;

use crate::mem::Allocator;
use crate::shim::cell::Cell;
use crate::shim::sync::Arc;
use crate::shim::sync::atomic::AtomicUsize;
use crate::spsc::queue::Queue;

pub(super) type ProducerState = CachePadded<ProducerStateInner>;

#[derive(Default)]
pub(super) struct ProducerStateInner {
    pub tail: AtomicUsize,
    pub cached_head: Cell<usize>,
}

/// The pushing handle of an SPSC FIFO queue.
///
/// Created together with its paired [`Consumer`](crate::spsc::Consumer) by
/// [`new`](crate::spsc::new).
/// `Producer` is [`Send`] but not [`Sync`]: at most one thread may push at a
/// time.
pub struct Producer<T, const CAPACITY: usize, A>
where
    A: Allocator,
{
    inner: Arc<Queue<T, CAPACITY, A>>,
    _not_sync: PhantomData<*const ()>,
}

impl<T, const CAPACITY: usize, A> std::fmt::Debug for Producer<T, CAPACITY, A>
where
    A: Allocator,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Producer")
            .field("capacity", &CAPACITY)
            .finish_non_exhaustive()
    }
}

// SAFETY: Producer is Send because the underlying Queue is Send when both T
// and the allocation are Send; PhantomData<*const ()> blocks Sync.
unsafe impl<T, const CAPACITY: usize, A> Send for Producer<T, CAPACITY, A>
where
    T: Send,
    A: Allocator,
    A::Allocation<T>: Send,
{
}

impl<T, const CAPACITY: usize, A> Producer<T, CAPACITY, A>
where
    A: Allocator,
{
    pub(super) fn new(queue: Arc<Queue<T, CAPACITY, A>>) -> Self {
        Self {
            inner: queue,
            _not_sync: PhantomData,
        }
    }

    /// Pushes `item` onto the queue.
    ///
    /// Wait-free. Never blocks, never allocates. Returns `None` on success.
    /// If the queue is full the item is returned unchanged as `Some(item)`,
    /// so the caller can retry or back off without losing data.
    ///
    /// # Examples
    ///
    /// ```
    /// use low_latency_data_structures::spsc::{self, new};
    /// use low_latency_data_structures::mem::global::GlobalAllocator;
    ///
    /// let (producer, consumer) = new::<u64, 2, GlobalAllocator>(
    ///     spsc::Options::global_mlocked(),
    /// );
    /// assert_eq!(producer.push(1), None);
    /// assert_eq!(producer.push(2), None);
    /// // Queue is full; item is handed back so we can retry later.
    /// assert_eq!(producer.push(3), Some(3));
    /// # let _ = consumer;
    /// ```
    #[inline]
    #[must_use = "if the queue is full, the returned item must be handled (e.g. retried) or it is silently dropped"]
    pub fn push(&self, item: T) -> Option<T> {
        self.inner.push(item)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::test_util::NeverAllocator;

    // Shouldn't be possible to construct Arc<Producer<T>> and then use it from different threads as it
    // will break the requirement of *Single* producer *Single* consumer queue.
    static_assertions::assert_not_impl_any!(Producer<u32, 0, NeverAllocator>: Sync);
}
