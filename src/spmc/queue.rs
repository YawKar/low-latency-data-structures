use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering, fence};

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
/// use low_latency_data_structures::spmc::{ReadResult, new};
///
/// let (producer, [mut a, mut b]) = new::<u64, 4, 2>();
/// producer.publish(1);
/// assert_eq!(a.try_read(), ReadResult::Value(1));
/// assert_eq!(b.try_read(), ReadResult::Value(1));
/// ```
///
/// Capacities that are not powers of two fail to compile:
///
/// ```compile_fail
/// # use low_latency_data_structures::spmc::new;
/// # use seq_macro::seq;
/// seq!(N in 2..20 {
///     {
///         const CAP: usize = 2usize.wrapping_pow(N);
///         let _fail = new::<u64, { CAP - 1 }, 3>();
///         let _fail = new::<u64, { CAP + 1 }, 4>();
///     }
/// });
/// ```
pub fn new<T, const CAPACITY: usize, const NCONSUMERS: usize>()
-> (Producer<T, CAPACITY>, [Consumer<T, CAPACITY>; NCONSUMERS])
where
    T: bytemuck::AnyBitPattern,
{
    const {
        assert!(
            CAPACITY.is_power_of_two(),
            "Given capacity is not a power of two",
        );
    }
    let q = Arc::new(Queue {
        producer_state: ProducerState {
            write_cursor: AtomicUsize::new(0),
        },
        slots: std::array::from_fn(|_| Slot {
            seq: AtomicUsize::new(0),
            data: UnsafeCell::new(T::zeroed()),
        }),
    });
    let producer = Producer::new(q.clone());
    let consumers = std::array::from_fn(|_| Consumer::new(q.clone()));
    (producer, consumers)
}

pub(super) struct Slot<T: bytemuck::AnyBitPattern> {
    pub(super) seq: AtomicUsize,
    pub(super) data: UnsafeCell<T>,
}

pub(super) struct Queue<T, const CAPACITY: usize>
where
    T: bytemuck::AnyBitPattern,
{
    pub(super) producer_state: ProducerState,
    pub(super) slots: [Slot<T>; CAPACITY],
}

// SAFETY: Queue uses Slot<T> which is !Sync because of UnsafeCell, but the queue itself can only be
// used through publish/Consumer APIs both of which synchronize themselves using seqlock seq.
unsafe impl<T: bytemuck::AnyBitPattern, const CAPACITY: usize> Sync for Queue<T, CAPACITY> {}

static_assertions::assert_impl_all!(Queue<u32, 1>: Sync, Send);

impl<T, const CAPACITY: usize> Queue<T, CAPACITY>
where
    T: bytemuck::AnyBitPattern,
{
    #[inline]
    pub(super) fn publish(&self, value: T) {
        let w_pos = self.producer_state.write_cursor.load(Ordering::Relaxed);
        // Used as both generation control and even-odd guarantee
        let seq_no = w_pos.wrapping_mul(2);
        // SAFETY: slots buffer is guaranteed to be the length of CAPACITY items
        let slot = unsafe { self.slots.get_unchecked(w_pos & (CAPACITY - 1)) };
        slot.seq.store(seq_no.wrapping_add(1), Ordering::Relaxed);
        // ARM: prevent the subsequent write from moving above the odd seq store
        fence(Ordering::Release);
        unsafe { slot.data.get().write_volatile(value) };
        slot.seq.store(seq_no.wrapping_add(2), Ordering::Release);
        self.producer_state
            .write_cursor
            .store(w_pos.wrapping_add(1), Ordering::Release);
    }
}

#[cfg(test)]
#[cfg(feature = "tests_basic")]
mod tests {
    use super::*;
    use crate::spmc::consumer::ReadResult;

    #[test]
    fn single_thread_multiple_consumers_read_messages() {
        const CAPACITY: usize = 128;
        const NCONSUMERS: usize = 3;
        let (producer, consumers) = new::<u64, CAPACITY, NCONSUMERS>();
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

        let (producer, consumers) = new::<u64, CAPACITY, NCONSUMERS>();
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
        let (producer, consumers) = new::<usize, CAPACITY, NCONSUMERS>();
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
