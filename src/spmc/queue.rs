use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering, fence};

use crate::mem::{Allocation, Allocator};
use crate::shim::cell::UnsafeCell;
use crate::spmc::builder::Options;
use crate::spmc::consumer::Consumer;
use crate::spmc::producer::{Producer, ProducerState};

/// Creates a new SPMC broadcast queue with `CAPACITY` slots and `NCONSUMERS`
/// pre-built [`Consumer`]s.
///
/// `CAPACITY` must be a power of two (compile-time enforced). The queue is
/// allocated up front and never grows. `T: AnyBitPattern` is required so
/// that torn reads materialise a valid `T` rather than triggering undefined
/// behaviour; the surrounding seq protocol then rejects the torn value
/// before it leaves [`Consumer::try_read`].
///
/// # Examples
///
/// ```
/// use low_latency_data_structures::spmc::{self, ReadResult, new};
/// use low_latency_data_structures::mem::global::GlobalAllocator;
///
/// let (producer, [mut a, mut b]) = new::<u64, 4, 2, GlobalAllocator>(
///     spmc::Options::global_mlocked(),
/// );
/// producer.publish(1);
/// assert_eq!(a.try_read(), ReadResult::Value(1));
/// assert_eq!(b.try_read(), ReadResult::Value(1));
/// ```
///
/// Capacities that are not powers of two fail to compile:
///
/// ```compile_fail
/// # use low_latency_data_structures::spmc::{self, new};
/// # use low_latency_data_structures::mem::global::GlobalAllocator;
/// # use seq_macro::seq;
/// seq!(N in 2..20 {
///     {
///         const CAP: usize = 2usize.wrapping_pow(N);
///         let _fail = new::<u64, { CAP - 1 }, 3, GlobalAllocator>(
///             spmc::Options::global_mlocked(),
///         );
///         let _fail = new::<u64, { CAP + 1 }, 4, GlobalAllocator>(
///             spmc::Options::global_mlocked(),
///         );
///     }
/// });
/// ```
pub fn new<T, const CAPACITY: usize, const NCONSUMERS: usize, Alloc: Allocator>(
    options: Options<Alloc>,
) -> (
    Producer<T, CAPACITY, impl Allocation<Slot<T>>>,
    [Consumer<T, CAPACITY, impl Allocation<Slot<T>>>; NCONSUMERS],
)
where
    T: bytemuck::AnyBitPattern,
{
    const {
        assert!(
            CAPACITY.is_power_of_two(),
            "Given capacity is not a power of two",
        );
    }
    let slots = Alloc::allocate::<Slot<T>>(CAPACITY, options.alloc);
    // Initialize every slot before any producer/consumer can touch it. The
    // seqlock protocol assumes `seq` starts at an even value (0); without
    // this loop, an allocator that returns garbage (e.g. GlobalAllocator on
    // arenas with reused memory) can cause the first read to see a bogus
    // matching `seq` and hand back uninitialized `T`.
    for ix in 0..CAPACITY {
        // SAFETY: `ix < CAPACITY`, so the derived pointer is inside the
        // allocation returned by `Alloc::allocate` and is aligned and
        // dereferenceable per the `Allocator` contract.
        let slot = unsafe { slots.ptr().wrapping_add(ix).as_mut_unchecked() };
        // SAFETY: `slot.get()` returns a `*mut MaybeUninit<Slot<T>>` into
        // uninitialized memory that no one else observes yet, so the write
        // does not overlap any live reference.
        unsafe {
            slot.get().write(MaybeUninit::new(Slot {
                seq: AtomicUsize::new(0),
                data: UnsafeCell::new(T::zeroed()),
            }));
        }
    }
    let q = Arc::new(Queue {
        producer_state: ProducerState {
            write_cursor: AtomicUsize::new(0),
        },
        slots,
        _t: PhantomData,
    });
    let producer = Producer::new(q.clone());
    let consumers = std::array::from_fn(|_| Consumer::new(q.clone()));
    (producer, consumers)
}

/// One cell in the SPMC ring. Carries the payload plus a sequence number the
/// seqlock protocol uses to detect torn or lapped reads.
///
/// Exposed only so callers can name the [`Allocation<Slot<T>>`](Allocation)
/// bound when writing generic helpers over the queue; there are no methods
/// intended for direct use.
pub struct Slot<T: bytemuck::AnyBitPattern> {
    pub(super) seq: AtomicUsize,
    pub(super) data: UnsafeCell<T>,
}

pub(super) struct Queue<T, const CAPACITY: usize, AllocT: Allocation<Slot<T>>>
where
    T: bytemuck::AnyBitPattern,
{
    pub(super) producer_state: ProducerState,
    pub(super) slots: AllocT,
    pub(super) _t: PhantomData<T>,
}

// SAFETY: Queue uses Slot<T> which is !Sync because of UnsafeCell, but the queue itself can only be
// used through publish/Consumer APIs both of which synchronize themselves using seqlock seq.
unsafe impl<T: bytemuck::AnyBitPattern, const CAPACITY: usize, AllocT: Allocation<Slot<T>>> Sync
    for Queue<T, CAPACITY, AllocT>
{
}

impl<T, const CAPACITY: usize, AllocT> Queue<T, CAPACITY, AllocT>
where
    T: bytemuck::AnyBitPattern,
    AllocT: Allocation<Slot<T>>,
{
    #[inline]
    pub(super) fn publish(&self, value: T) {
        let w_pos = self.producer_state.write_cursor.load(Ordering::Relaxed);
        // Used as both generation control and even-odd guarantee
        let seq_no = w_pos.wrapping_mul(2);
        let slot = self.slot(w_pos);
        slot.seq.store(seq_no.wrapping_add(1), Ordering::Relaxed);
        // ARM: prevent the subsequent write from moving above the odd seq store
        fence(Ordering::Release);
        unsafe { slot.data.get().write_volatile(value) };
        slot.seq.store(seq_no.wrapping_add(2), Ordering::Release);
        self.producer_state
            .write_cursor
            .store(w_pos.wrapping_add(1), Ordering::Release);
    }

    /// Wraps the given `i` around `CAPACITY - 1`.
    ///
    /// # Safety
    ///
    /// - The masked index is always in `[0, CAPACITY)`, so the derived
    ///   pointer stays inside the allocation.
    /// - The allocation is aligned and dereferenceable per the `Allocator`
    ///   contract.
    /// - Every slot is initialized in [`new`] before the `Queue` is wrapped
    ///   in `Arc`, so `assume_init_ref` is sound.
    /// - Returning `&Slot<T>` while another thread mutates through
    ///   `slot.data` / `slot.seq` is legal because the mutable state sits
    ///   behind `UnsafeCell` and atomics.
    #[inline(always)]
    fn slot(&self, i: usize) -> &Slot<T> {
        unsafe {
            self.slots
                .ptr()
                .wrapping_add(i & (CAPACITY - 1))
                .as_ref_unchecked()
                .get()
                .as_ref_unchecked()
                .assume_init_ref()
        }
    }
}

#[cfg(test)]
#[cfg(feature = "tests_basic")]
mod tests {
    use super::*;
    #[cfg(not(feature = "tests_hugepage"))]
    use crate::mem::global::GlobalAllocator;
    #[cfg(feature = "tests_hugepage")]
    use crate::mem::hugepages::{HugepageAllocator, HugepageAllocatorOptions, HugepageSize};
    use crate::mem::test_util::NeverAlloc;
    use crate::spmc::consumer::ReadResult;

    static_assertions::assert_impl_all!(Queue<u32, 1, NeverAlloc>: Sync, Send);

    #[cfg(not(feature = "tests_hugepage"))]
    fn spmc_options() -> Options<GlobalAllocator> {
        Options::global_mlocked()
    }
    #[cfg(feature = "tests_hugepage")]
    fn spmc_options() -> Options<HugepageAllocator> {
        Options::builder()
            .alloc(
                HugepageAllocatorOptions::builder()
                    .mlock(true)
                    .hugepage_size(HugepageSize::H2MB)
                    .build(),
            )
            .build()
    }

    #[test]
    fn single_thread_multiple_consumers_read_messages() {
        const CAPACITY: usize = 128;
        const NCONSUMERS: usize = 3;
        let (producer, consumers) = new::<u64, CAPACITY, NCONSUMERS, _>(spmc_options());
        let [mut c1, mut c2, mut c3] = consumers;
        assert_eq!(c1.try_read(), ReadResult::Empty);
        assert_eq!(c2.try_read(), ReadResult::Empty);
        assert_eq!(c3.try_read(), ReadResult::Empty);

        producer.publish(1);
        assert_eq!(c1.try_read(), ReadResult::Value(1));
        assert_eq!(c2.try_read(), ReadResult::Value(1));
        assert_eq!(c3.try_read(), ReadResult::Value(1));
        assert_eq!(c1.try_read(), ReadResult::Empty);
        assert_eq!(c2.try_read(), ReadResult::Empty);
        assert_eq!(c3.try_read(), ReadResult::Empty);

        producer.publish(2);
        assert_eq!(c1.try_read(), ReadResult::Value(2));
        assert_eq!(c2.try_read(), ReadResult::Value(2));
        assert_eq!(c1.try_read(), ReadResult::Empty);
        assert_eq!(c2.try_read(), ReadResult::Empty);

        producer.publish(3);
        assert_eq!(c1.try_read(), ReadResult::Value(3));
        assert_eq!(c1.try_read(), ReadResult::Empty);

        assert_eq!(c2.try_read(), ReadResult::Value(3));
        assert_eq!(c2.try_read(), ReadResult::Empty);

        assert_eq!(c3.try_read(), ReadResult::Value(2));
        assert_eq!(c3.try_read(), ReadResult::Value(3));
        assert_eq!(c3.try_read(), ReadResult::Empty);
    }

    #[test]
    fn concurrent_single_producer_multiple_consumers() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::thread;

        const CAPACITY: usize = 1024;
        const NCONSUMERS: usize = 4;
        const N: u64 = 200_000;

        let (producer, consumers) = new::<u64, CAPACITY, NCONSUMERS, _>(spmc_options());
        let done = Arc::new(AtomicBool::new(false));

        let handles: Vec<_> = consumers
            .into_iter()
            .map(|mut c| {
                let done = done.clone();
                thread::spawn(move || -> u64 {
                    let mut last: Option<u64> = None;
                    let mut got = 0u64;
                    loop {
                        match c.try_read() {
                            ReadResult::Value(v) => {
                                if let Some(prev) = last {
                                    assert!(v > prev);
                                }
                                last = Some(v);
                                got += 1;
                            }
                            ReadResult::Lapped { .. } => {}
                            ReadResult::Empty => {
                                if done.load(Ordering::Acquire) {
                                    while let ReadResult::Value(v) = c.try_read() {
                                        if let Some(prev) = last {
                                            assert!(v > prev);
                                        }
                                        last = Some(v);
                                        got += 1;
                                    }
                                    return got;
                                }
                            }
                        }
                    }
                })
            })
            .collect();

        for i in 0..N {
            producer.publish(i);
        }
        done.store(true, Ordering::Release);

        for h in handles {
            let got = h.join().unwrap();
            assert!(got > 0);
        }
    }

    #[test]
    fn consumer_can_be_overlapped_by_writer() {
        const CAPACITY: usize = 8;
        const NCONSUMERS: usize = 1;
        let (producer, consumers) = new::<usize, CAPACITY, NCONSUMERS, _>(spmc_options());
        let [mut c1] = consumers;
        assert_eq!(c1.try_read(), ReadResult::Empty);

        // After this loop write_cursor = 8. First try_read below loads it into
        // cached_write_cursor and returns the first item.
        for i in 0..CAPACITY {
            producer.publish(i);
        }
        // read_cursor becomes 1.
        assert_eq!(c1.try_read(), ReadResult::Value(0));
        // Two more publishes lap the consumer. write_cursor = 10 after these.
        producer.publish(CAPACITY + 1);
        producer.publish(CAPACITY + 2);
        // The Lapped branch reloads cached_write_cursor (= 10) and jumps to
        // cached - 1 = 9. skipped = 9 - read_cursor = 9 - 1 = 8.
        assert_eq!(c1.try_read(), ReadResult::Lapped { skipped: 8 });
        // Slot 9 & 7 = slot 1, last written at w_pos = 9 with data = CAPACITY + 2.
        assert_eq!(c1.try_read(), ReadResult::Value(CAPACITY + 2));
    }
}
