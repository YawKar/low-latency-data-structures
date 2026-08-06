use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::sync::atomic;

use crate::mem::{Allocation, Allocator};
use crate::shim::cell::UnsafeCell;
use crate::shim::sync::Arc;
use crate::spsc::queue::Queue;

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
    slots_ptr: *mut UnsafeCell<MaybeUninit<T>>,
    cached_head: usize,
    cached_tail: usize,
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
        let slots_ptr = queue.slots_allocation.ptr();
        Self {
            inner: queue,
            slots_ptr,
            cached_head: 0,
            cached_tail: 0,
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
    /// let (mut producer, mut consumer) = new::<u64, 2, GlobalAllocator>(
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
    pub fn push(&mut self, item: T) -> Option<T> {
        if self.cached_tail.wrapping_sub(self.cached_head) == CAPACITY {
            if self.still_full() {
                return Some(item);
            }
        }
        let slot_ptr = self
            .slots_ptr
            .wrapping_add(self.cached_tail & (CAPACITY - 1));
        // SAFETY: slot_ptr can't point to something after the slots buffer because of `% capacity`
        // above. And it can be converted to a reference to T because T is self-contained bitwise
        // (&T is 'static during the with_mut closure).
        unsafe {
            slot_ptr
                .as_ref_unchecked()
                .with_mut(|ptr| ptr.cast::<T>().write(item))
        };
        self.cached_tail += 1;
        self.inner
            .tail
            .store(self.cached_tail, atomic::Ordering::Release);
        None
    }

    #[cold]
    fn still_full(&mut self) -> bool {
        // consumer may have moved the head
        self.cached_head = self.inner.head.load(atomic::Ordering::Acquire);
        if self.cached_tail.wrapping_sub(self.cached_head) == CAPACITY {
            return true;
        }
        false
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
