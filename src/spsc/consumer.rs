use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::sync::atomic;

use crate::mem::{Allocation, Allocator};
use crate::shim::cell::UnsafeCell;
use crate::shim::sync::Arc;
use crate::spsc::queue::Queue;

/// The popping handle of an SPSC FIFO queue.
///
/// Created together with its paired [`Producer`](crate::spsc::Producer) by
/// [`new`](crate::spsc::new).
/// `Consumer` is [`Send`] but not [`Sync`]: at most one thread may pop at a
/// time.
pub struct Consumer<T, const CAPACITY: usize, A>
where
    A: Allocator,
{
    inner: Arc<Queue<T, CAPACITY, A>>,
    slots_ptr: *mut UnsafeCell<MaybeUninit<T>>,
    cached_head: usize,
    cached_tail: usize,
    _not_sync: PhantomData<*const ()>,
}

impl<T, const CAPACITY: usize, A> std::fmt::Debug for Consumer<T, CAPACITY, A>
where
    A: Allocator,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Consumer")
            .field("capacity", &CAPACITY)
            .finish_non_exhaustive()
    }
}

// SAFETY: Consumer is Send because the underlying Queue is Send when both T
// and the allocation are Send; PhantomData<*const ()> blocks Sync.
unsafe impl<T, const CAPACITY: usize, A> Send for Consumer<T, CAPACITY, A>
where
    T: Send,
    A: Allocator,
    A::Allocation<T>: Send,
{
}

impl<T, const CAPACITY: usize, A> Consumer<T, CAPACITY, A>
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
    /// let (mut producer, mut consumer) = new::<u64, 4, GlobalAllocator>(
    ///     spsc::Options::global_mlocked(),
    /// );
    /// assert_eq!(consumer.pop(), None);
    /// let _ = producer.push(42);
    /// assert_eq!(consumer.pop(), Some(42));
    /// ```
    #[inline]
    #[must_use = "ignoring the popped item silently drops it"]
    pub fn pop(&mut self) -> Option<T> {
        if self.cached_head == self.cached_tail {
            if self.still_empty() {
                return None;
            }
        }
        let slot_ptr = self
            .slots_ptr
            .wrapping_add(self.cached_head & (CAPACITY - 1));
        // SAFETY: we read the cached_tail value that was released some time ago, it means we are
        // guaranteed to see written value here. And it's not copied more than once because we
        // increment head on the next line.
        let item = unsafe {
            slot_ptr
                .as_ref_unchecked()
                .with_mut(|ptr| ptr.cast::<T>().read())
        };
        self.cached_head += 1;
        self.inner
            .head
            .store(self.cached_head, atomic::Ordering::Release);
        Some(item)
    }

    #[cold]
    fn still_empty(&mut self) -> bool {
        // producer may have written something
        self.cached_tail = self.inner.tail.load(atomic::Ordering::Acquire);
        self.cached_head == self.cached_tail
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::test_util::NeverAllocator;

    // Shouldn't be possible to construct Arc<Consumer<T>> and then use it from different threads as it
    // will break the requirement of *Single* producer *Single* consumer queue.
    static_assertions::assert_not_impl_any!(Consumer<u32, 0, NeverAllocator>: Sync);
}
