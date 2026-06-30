use std::marker::PhantomData;

use crate::mem::Allocation;
use crate::shim::cell::Cell;
use crate::shim::sync::Arc;
use crate::shim::sync::atomic::AtomicUsize;
use crate::spsc::queue::Queue;

#[repr(C, align(128))]
pub(super) struct ProducerState {
    pub tail: AtomicUsize,
    pub cached_head: Cell<usize>,
}

/// The pushing handle of an SPSC FIFO queue.
///
/// Created together with its paired [`Consumer`](crate::spsc::Consumer) by
/// [`new`](crate::spsc::new) (or [`new_hugepage_backed`](crate::spsc::new_hugepage_backed)).
/// `Producer` is [`Send`] but not [`Sync`]: at most one thread may push at a
/// time.
pub struct Producer<T, const CAPACITY: usize, AllocT: Allocation<T>> {
    inner: Arc<Queue<T, CAPACITY, AllocT>>,
    _not_sync: PhantomData<*const ()>,
}

impl<T, const CAPACITY: usize, AllocT: Allocation<T>> std::fmt::Debug
    for Producer<T, CAPACITY, AllocT>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Producer")
            .field("capacity", &CAPACITY)
            .finish_non_exhaustive()
    }
}

// SAFETY: Producer is Send because the underlying Queue is Send when both T
// and the allocation are Send; PhantomData<*const ()> blocks Sync.
unsafe impl<T: Send, const CAPACITY: usize, AllocT: Allocation<T> + Send> Send
    for Producer<T, CAPACITY, AllocT>
{
}

impl<T, const CAPACITY: usize, AllocT: Allocation<T>> Producer<T, CAPACITY, AllocT> {
    pub(super) fn new(queue: Arc<Queue<T, CAPACITY, AllocT>>) -> Self {
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
    /// use low_latency_data_structures::spsc::new;
    ///
    /// let (producer, consumer) = new::<u64, 2>();
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
    use crate::shim::cell::UnsafeCell;

    struct NeverAlloc;
    impl<T> Allocation<T> for NeverAlloc {
        fn ptr(&self) -> *mut UnsafeCell<std::mem::MaybeUninit<T>> {
            unreachable!("it's just a stub")
        }
    }

    // Shouldn't be possible to construct Arc<Producer<T>> and then use it from different threads as it
    // will break the requirement of *Single* producer *Single* consumer queue.
    static_assertions::assert_not_impl_any!(Producer<u32, 0, NeverAlloc>: Sync);
}
