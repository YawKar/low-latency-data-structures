use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{Ordering, fence};

use crate::spmc::queue::Queue;

#[repr(C, align(128))]
pub(super) struct ConsumerState {
    read_cursor: usize,
    cached_write_cursor: usize,
}

#[derive(Debug, PartialEq)]
pub enum ReadResult<T> {
    Value(T),
    Empty,
    Lapped { skipped: usize },
}

#[repr(C)]
pub struct Consumer<T: bytemuck::AnyBitPattern, const CAPACITY: usize, const NCONSUMERS: usize> {
    state: ConsumerState,
    inner: Arc<Queue<T, CAPACITY, NCONSUMERS>>,
    _not_sync: PhantomData<*const ()>,
}

// SAFETY: It is Send on its own but we need to forbid the Sync.
unsafe impl<T: bytemuck::AnyBitPattern, const CAPACITY: usize, const NCONSUMERS: usize> Send
    for Consumer<T, CAPACITY, NCONSUMERS>
{
}

static_assertions::assert_impl_all!(Consumer<u32, 2, 1>: Send);
static_assertions::assert_not_impl_any!(Consumer<u32, 2, 1>: Sync, Clone, Copy);

impl<T, const CAPACITY: usize, const NCONSUMERS: usize> Consumer<T, CAPACITY, NCONSUMERS>
where
    T: bytemuck::AnyBitPattern,
{
    pub(super) fn new(queue: Arc<Queue<T, CAPACITY, NCONSUMERS>>) -> Self {
        Self {
            state: ConsumerState {
                read_cursor: 0,
                cached_write_cursor: 0,
            },
            inner: queue,
            _not_sync: PhantomData,
        }
    }

    #[inline]
    pub fn try_read(&mut self) -> ReadResult<T> {
        if self.state.read_cursor == self.state.cached_write_cursor {
            // It still may not be empty
            if self.is_still_empty() {
                return ReadResult::Empty;
            }
        }
        // SAFETY: it is guaranteed at compile-time that slots has exactly CAPACITY items
        let slot = unsafe {
            self.inner
                .slots
                .get_unchecked(self.state.read_cursor & (CAPACITY - 1))
        };
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
                // We were overlapped => skip everything and go up to the `write_cursor - 1`
                let new_r_cursor = self.state.cached_write_cursor.wrapping_sub(1);
                let skipped = new_r_cursor.wrapping_sub(self.state.read_cursor);
                self.state.read_cursor = new_r_cursor;
                return ReadResult::Lapped { skipped };
            }
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
}
