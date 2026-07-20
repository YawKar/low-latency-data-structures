use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{Ordering, fence};

use crate::mem::Allocation;
use crate::spmc::queue::{Queue, Slot};

#[repr(C, align(128))]
pub(super) struct ConsumerState {
    read_cursor: usize,
    cached_write_cursor: usize,
}

/// Outcome of a [`Consumer::try_read`] call.
///
/// `try_read` is non-blocking and never allocates, so the return value
/// distinguishes three cases the caller must handle: a successful read, an
/// empty queue, or a detected lap. The enum is `#[must_use]` because
/// silently dropping `Value` would lose the only copy of the data, and
/// silently dropping `Lapped` would hide message loss.
#[must_use = "the returned ReadResult indicates whether a value was read, the queue was empty, or the consumer was lapped"]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadResult<T> {
    /// A value was successfully read from the next slot.
    Value(T),
    /// The queue is currently empty.
    Empty,
    /// The producer lapped this consumer at least once. The read cursor has
    /// been advanced to the most recently published slot; `skipped` is the
    /// number of values that were never observed.
    Lapped {
        /// Number of values skipped over by the lap-recovery jump.
        skipped: usize,
    },
}

/// A reading handle of an SPMC broadcast queue.
///
/// Each consumer has its own private read cursor: every consumer observes
/// every value the producer publishes, independently, unless it falls behind
/// by more than `CAPACITY` slots.
///
/// `Consumer` is [`Send`] but not [`Sync`]: at most one thread may call
/// [`try_read`](Self::try_read) on a given consumer at a time. To get more
/// consumers, request more at construction via [`new`](crate::spmc::new).
pub struct Consumer<T: bytemuck::AnyBitPattern, const CAPACITY: usize, AllocT: Allocation<Slot<T>>>
{
    state: ConsumerState,
    inner: Arc<Queue<T, CAPACITY, AllocT>>,
    _not_sync: PhantomData<*const ()>,
}

impl<T: bytemuck::AnyBitPattern, const CAPACITY: usize, AllocT: Allocation<Slot<T>>> std::fmt::Debug
    for Consumer<T, CAPACITY, AllocT>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Consumer")
            .field("read_cursor", &self.state.read_cursor)
            .field("cached_write_cursor", &self.state.cached_write_cursor)
            .finish_non_exhaustive()
    }
}

// SAFETY: It is Send on its own but we need to forbid the Sync.
unsafe impl<T: bytemuck::AnyBitPattern, const CAPACITY: usize, AllocT: Allocation<Slot<T>>> Send
    for Consumer<T, CAPACITY, AllocT>
{
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::test_util::NeverAlloc;

    static_assertions::assert_impl_all!(Consumer<u32, 2, NeverAlloc>: Send);
    static_assertions::assert_not_impl_any!(Consumer<u32, 2, NeverAlloc>: Sync, Clone, Copy);
}

impl<T, const CAPACITY: usize, AllocT> Consumer<T, CAPACITY, AllocT>
where
    T: bytemuck::AnyBitPattern,
    AllocT: Allocation<Slot<T>>,
{
    pub(super) fn new(queue: Arc<Queue<T, CAPACITY, AllocT>>) -> Self {
        Self {
            state: ConsumerState {
                read_cursor: 0,
                cached_write_cursor: 0,
            },
            inner: queue,
            _not_sync: PhantomData,
        }
    }

    /// Attempts to read the next value.
    ///
    /// Wait-free in the common case (value available, or queue empty);
    /// lock-free when the producer is actively writing the target slot (the
    /// consumer spin-loops until the producer publishes).
    ///
    /// # Protocol
    ///
    /// Each slot carries an even/odd seq number: even = stable, odd = mid
    /// write. The consumer:
    ///
    /// 1. Loads the slot's seq. If odd, spin until it goes even.
    /// 2. If the even seq is not the one expected for the current read
    ///    cursor, the producer has lapped us; reload the write cursor, jump
    ///    to the most recent slot, and return [`ReadResult::Lapped`].
    /// 3. Otherwise read the slot's data, then re-load the seq. If the seq
    ///    is unchanged the read is committed and a [`ReadResult::Value`] is
    ///    returned; if the seq has moved, the read was torn and the loop
    ///    retries.
    ///
    /// # Examples
    ///
    /// ```
    /// use low_latency_data_structures::spmc::{self, ReadResult, new};
    /// use low_latency_data_structures::mem::global::GlobalAllocator;
    ///
    /// let (producer, [mut consumer]) = new::<u64, 16, 1, GlobalAllocator>(
    ///     spmc::Options::global_mlocked(),
    /// );
    /// assert_eq!(consumer.try_read(), ReadResult::Empty);
    /// producer.publish(7);
    /// assert_eq!(consumer.try_read(), ReadResult::Value(7));
    /// ```
    #[inline]
    pub fn try_read(&mut self) -> ReadResult<T> {
        if self.state.read_cursor == self.state.cached_write_cursor {
            // It still may not be empty
            if self.is_still_empty() {
                return ReadResult::Empty;
            }
        }
        let slot = self.slot(self.state.read_cursor);
        // That's the expected seq_no after producer has written the item
        let expected_seq = self.state.read_cursor.wrapping_mul(2).wrapping_add(2);
        loop {
            let seq1 = slot.seq.load(Ordering::Acquire);
            // If producer currently writes it, we wait (by the way, it means that we were overlapped)
            if seq1 & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            if seq1 != expected_seq {
                // We were overlapped. Reload cached_write_cursor before jumping:
                // under sustained producer overflow the cached value is itself
                // stale (cached_write_cursor - 1 may have been lapped past
                // again), and without the reload the consumer gets pinned in
                // a Lapped -> Lapped cycle with skipped=0 forever.
                self.state.cached_write_cursor = self
                    .inner
                    .producer_state
                    .write_cursor
                    .load(Ordering::Acquire);
                let new_r_cursor = self.state.cached_write_cursor.wrapping_sub(1);
                let skipped = new_r_cursor.wrapping_sub(self.state.read_cursor);
                self.state.read_cursor = new_r_cursor;
                return ReadResult::Lapped { skipped };
            }
            // SAFETY: T: AnyBitPattern, so even a torn read materialises a valid T;
            // the seq2 check below rejects the value if the read raced a producer write.
            let attempted_read = unsafe { slot.data.get().read_volatile() };
            // ASM: prevent the seq1 load from moving below the read
            fence(Ordering::Acquire);
            let seq2 = slot.seq.load(Ordering::Relaxed);
            if seq1 == seq2 {
                self.state.read_cursor = self.state.read_cursor.wrapping_add(1);
                return ReadResult::Value(attempted_read);
            }
            // Torn-read, retry
        }
    }

    #[cold]
    fn is_still_empty(&mut self) -> bool {
        self.state.cached_write_cursor = self
            .inner
            .producer_state
            .write_cursor
            .load(Ordering::Acquire);
        self.state.read_cursor == self.state.cached_write_cursor
    }

    /// Wraps the given `i` around `CAPACITY - 1`.
    ///
    /// # Safety
    ///
    /// - The masked index is always in `[0, CAPACITY)`, so the derived
    ///   pointer stays inside the allocation.
    /// - The allocation is aligned and dereferenceable per the `Allocator`
    ///   contract.
    /// - Every slot is initialized in [`new`](super::new) before the
    ///   `Queue` is wrapped in `Arc`, so `assume_init_ref` is sound.
    /// - Returning `&Slot<T>` while another thread mutates through
    ///   `slot.data` / `slot.seq` is legal because the mutable state sits
    ///   behind `UnsafeCell` and atomics.
    #[inline(always)]
    fn slot(&self, i: usize) -> &Slot<T> {
        unsafe {
            self.inner
                .slots
                .ptr()
                .wrapping_add(i & (CAPACITY - 1))
                .as_ref_unchecked()
                .get()
                .as_ref_unchecked()
                .assume_init_ref()
        }
    }
}
