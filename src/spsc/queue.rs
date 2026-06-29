use std::marker::PhantomData;

use crate::mem::{Allocation, allocate_buffer, allocate_hugepage_buffer};
use crate::shim::sync::{Arc, atomic};
use crate::spsc::consumer::{Consumer, ConsumerState};
use crate::spsc::producer::{Producer, ProducerState};

/// Should fail at compile time if used with CAPACITY not a power of two:
/// ```compile_fail
/// # use low_latency_data_structures::spsc::new;
/// # use seq_macro::seq;
/// seq!(N in 2..20 {
///     {
///         const CAP: usize = 2usize.wrapping_pow(N);
///         let _fail = new::<u64, { CAP - 1 }>();
///         let _fail = new::<u64, { CAP + 1 }>();
///     }
/// });
/// ```
pub fn new<T, const CAPACITY: usize>() -> (
    Producer<T, CAPACITY, impl Allocation<T>>,
    Consumer<T, CAPACITY, impl Allocation<T>>,
) {
    const {
        assert!(
            CAPACITY.is_power_of_two(),
            "Given capacity is not a power of two!"
        );
    };
    let slots_allocation = allocate_buffer::<T>(CAPACITY);
    let q = Arc::new(Queue {
        producer_state: ProducerState {
            tail: Default::default(),
            cached_head: Default::default(),
        },
        consumer_state: ConsumerState {
            head: Default::default(),
            cached_tail: Default::default(),
        },
        slots_allocation,
        _t: PhantomData,
    });
    let producer = Producer::new(q.clone());
    let consumer = Consumer::new(q);
    (producer, consumer)
}

pub fn new_hugepage_backed<T, const CAPACITY: usize>() -> (
    Producer<T, CAPACITY, impl Allocation<T>>,
    Consumer<T, CAPACITY, impl Allocation<T>>,
) {
    const {
        assert!(
            CAPACITY.is_power_of_two(),
            "Given capacity is not a power of two!"
        );
    };
    let slots_allocation = allocate_hugepage_buffer(CAPACITY);
    let q = Arc::new(Queue {
        producer_state: ProducerState {
            tail: Default::default(),
            cached_head: Default::default(),
        },
        consumer_state: ConsumerState {
            head: Default::default(),
            cached_tail: Default::default(),
        },
        slots_allocation,
        _t: PhantomData,
    });
    let producer = Producer::new(q.clone());
    let consumer = Consumer::new(q);
    (producer, consumer)
}

#[repr(C)]
pub(super) struct Queue<T, const CAPACITY: usize, AllocT: Allocation<T>> {
    producer_state: ProducerState,
    consumer_state: ConsumerState,
    slots_allocation: AllocT,
    _t: PhantomData<T>,
}

impl<T, const CAPACITY: usize, AllocT: Allocation<T>> Queue<T, CAPACITY, AllocT> {
    #[inline]
    pub fn pop(&self) -> Option<T> {
        let head = self.consumer_state.head.load(atomic::Ordering::Relaxed);
        if head == self.consumer_state.cached_tail.get() {
            // it's still may not be empty
            if self.pop_still_empty(head) {
                return None;
            }
        }
        let slot_ptr = self
            .slots_allocation
            .ptr()
            .wrapping_add(head & (CAPACITY - 1));
        // SAFETY: we read the cached_tail value that was released some time ago, it means we are
        // guaranteed to see written value here. And it's not copied more than once because we
        // increment head on the next line.
        let item = unsafe {
            slot_ptr
                .as_ref_unchecked()
                .with_mut(|ptr| ptr.cast::<T>().read())
        };
        self.consumer_state
            .head
            .store(head.wrapping_add(1), atomic::Ordering::Release);
        Some(item)
    }

    #[cold]
    fn pop_still_empty(&self, head: usize) -> bool {
        self.consumer_state
            .cached_tail
            .set(self.producer_state.tail.load(atomic::Ordering::Acquire));
        head == self.consumer_state.cached_tail.get()
    }

    #[inline]
    pub fn push(&self, item: T) -> Option<T> {
        let tail = self.producer_state.tail.load(atomic::Ordering::Relaxed);
        debug_assert!(tail.wrapping_sub(self.producer_state.cached_head.get()) <= CAPACITY);
        if tail.wrapping_sub(self.producer_state.cached_head.get()) >= CAPACITY {
            // it's still may not be full
            if self.push_still_full(tail) {
                return Some(item);
            }
        }
        let slot_ptr = self
            .slots_allocation
            .ptr()
            .wrapping_add(tail & (CAPACITY - 1));
        // SAFETY: slot_ptr can't point to something after the slots buffer because of `% capacity`
        // above. And it can be converted to a reference to T because T is self-contained bitwise
        // (&T is 'static during the with_mut closure).
        unsafe {
            slot_ptr
                .as_ref_unchecked()
                .with_mut(|ptr| ptr.cast::<T>().write(item))
        };
        self.producer_state
            .tail
            .store(tail.wrapping_add(1), atomic::Ordering::Release);
        None
    }

    #[cold]
    fn push_still_full(&self, tail: usize) -> bool {
        self.producer_state
            .cached_head
            .set(self.consumer_state.head.load(atomic::Ordering::Acquire));
        debug_assert!(tail.wrapping_sub(self.producer_state.cached_head.get()) <= CAPACITY);
        tail.wrapping_sub(self.producer_state.cached_head.get()) >= CAPACITY
    }
}

impl<T, const CAPACITY: usize, AllocT: Allocation<T>> Drop for Queue<T, CAPACITY, AllocT> {
    fn drop(&mut self) {
        let head = self.consumer_state.head.load(atomic::Ordering::Relaxed);
        let tail = self.producer_state.tail.load(atomic::Ordering::Relaxed);
        let count = tail.wrapping_sub(head);
        for k in 0..count {
            let i = head.wrapping_add(k);
            let slot_ptr = self.slots_allocation.ptr().wrapping_add(i & (CAPACITY - 1));
            // SAFETY: it's not null because `i & (sef.capacity - 1)` limits it to [0;
            // allocated_cap). And it can be safely converted to a reference because T is self
            // contained bitwise.
            unsafe {
                slot_ptr.as_ref_unchecked().with_mut(|ptr| {
                    ptr.as_mut_unchecked().assume_init_drop();
                })
            }
        }
    }
}

#[cfg(test)]
#[cfg(feature = "tests_basic")]
mod tests_basic {
    use std::rc::Rc;
    use std::thread;

    use seq_macro::seq;

    #[cfg(not(feature = "tests_hugepage"))]
    use super::new;
    #[cfg(feature = "tests_hugepage")]
    use super::new_hugepage_backed as new;
    use crate::shim::cell::Cell;

    #[test]
    fn move_producer_consumer_to_threads() {
        let (producer, consumer) = new::<_, 2>();
        thread::spawn(move || {
            producer.push(123);
        })
        .join()
        .unwrap();
        thread::spawn(move || {
            assert_eq!(consumer.pop(), Some(123));
        })
        .join()
        .unwrap();
    }

    #[test]
    fn handoff_one_value() {
        let (producer, consumer) = new::<_, 2>();
        assert_eq!(producer.push(123), None);
        assert_eq!(consumer.pop(), Some(123));
    }

    #[test]
    fn allows_queues_with_powers_of_two_capacity() {
        #[cfg(feature = "tests_hugepage")]
        seq!(N in 0..6 {
            {
                const POWER: usize = 2usize.pow(N);
                new::<u32, POWER>();
            }
        });
        #[cfg(not(feature = "tests_hugepage"))]
        seq!(N in 0..20 {
            {
                const POWER: usize = 2usize.pow(N);
                new::<u32, POWER>();
            }
        });
    }

    #[test]
    fn drops_unread_items() {
        let counter = Rc::new(Cell::new(0));
        #[derive(Debug, PartialEq)]
        struct Droppable {
            counter: Rc<Cell<usize>>,
        }
        impl Drop for Droppable {
            fn drop(&mut self) {
                let cnt = self.counter.get();
                self.counter.set(cnt + 1);
            }
        }
        const CAPACITY: usize = 64;
        let (producer, consumer) = new::<_, CAPACITY>();
        for _ in 0..CAPACITY {
            let counter = counter.clone();
            assert_eq!(producer.push(Droppable { counter }), None);
        }
        let read = CAPACITY / 2;
        for _ in 0..read {
            assert!(matches!(consumer.pop(), Some(_)));
        }
        assert_eq!(read, counter.get());
        drop(producer);
        drop(consumer);
        assert_eq!(CAPACITY, counter.get());
    }

    #[test]
    fn empty_returns_none() {
        let (_, consumer) = new::<i32, 4>();
        assert_eq!(consumer.pop(), None);
        assert_eq!(consumer.pop(), None);
    }

    #[test]
    fn full_returns_item_back() {
        let (producer, _) = new::<i32, 2>();
        assert_eq!(producer.push(1), None);
        assert_eq!(producer.push(2), None);
        // Full: item comes back untouched
        assert_eq!(producer.push(3), Some(3));
        assert_eq!(producer.push(4), Some(4));
    }

    #[test]
    fn fifo_ordering() -> anyhow::Result<()> {
        let (producer, consumer) = new::<_, 8>();
        for i in 0..8 {
            assert_eq!(producer.push(i), None);
        }
        for i in 0..8 {
            assert_eq!(consumer.pop(), Some(i));
        }
        Ok(())
    }

    #[test]
    fn wraparound_n_laps() {
        const CAPACITY: usize = 4;
        let laps = 100;
        let (producer, consumer) = new::<_, CAPACITY>();
        for lap in 0..laps {
            for i in 0..CAPACITY {
                let val = lap * CAPACITY + i;
                assert_eq!(producer.push(val), None, "push failed at lap {lap}, i {i}");
            }
            // Queue is full
            assert_eq!(producer.push(9999), Some(9999));
            for i in 0..CAPACITY {
                let val = lap * CAPACITY + i;
                assert_eq!(consumer.pop(), Some(val), "wrong value at lap {lap}, i {i}");
            }
            // Queue is empty
            assert_eq!(consumer.pop(), None);
        }
    }

    #[test]
    fn interleaved_push_pop() {
        let (producer, consumer) = new::<_, 2>();
        // Push 1, pop 1, repeat: tests wraparound with tiny queue
        for i in 0..1000 {
            assert_eq!(producer.push(i), None);
            assert_eq!(consumer.pop(), Some(i));
        }
    }

    #[test]
    fn capacity_one() {
        let (producer, consumer) = new::<_, 1>();
        assert_eq!(consumer.pop(), None);
        assert_eq!(producer.push(42), None);
        assert_eq!(producer.push(43), Some(43)); // full
        assert_eq!(consumer.pop(), Some(42));
        assert_eq!(consumer.pop(), None); // empty again
    }

    #[test]
    fn move_only_type() {
        // Verify non-Copy, non-Clone types work
        let (producer, consumer) = new::<_, 4>();
        let s = String::from("hello");
        assert_eq!(producer.push(s), None);
        let got = consumer.pop().unwrap();
        assert_eq!(got, "hello");
    }
}

#[cfg(test)]
#[cfg(feature = "tests_loom")]
mod tests_loom {
    use super::*;

    static_assertions::assert_cfg!(
        not(feature = "tests_hugepage"),
        "tests_loom incompatible with tests_hugepage as loom uses custom buffer allocator",
    );

    #[test]
    fn concurrent_push_pop() {
        loom::model(|| {
            let (producer, consumer) = new::<i32, 4>();

            let t1 = loom::thread::spawn(move || {
                producer.push(1);
                producer.push(2);
                producer.push(3);
            });

            let t2 = loom::thread::spawn(move || {
                let mut collected = Vec::new();
                while collected.len() < 3 {
                    if let Some(v) = consumer.pop() {
                        collected.push(v);
                    } else {
                        loom::thread::yield_now();
                    }
                }
                collected
            });

            t1.join().unwrap();
            let values = t2.join().unwrap();
            // FIFO: must be in order
            assert_eq!(values, vec![1, 2, 3]);
        });
    }

    /// Producer must retry if the queue is full.
    #[test]
    fn concurrent_with_full_queue() {
        loom::model(|| {
            let (producer, consumer) = new::<i32, 1>();

            let t1 = loom::thread::spawn(move || {
                for i in 0..3 {
                    while producer.push(i).is_some() {
                        loom::thread::yield_now();
                    }
                }
            });

            let t2 = loom::thread::spawn(move || {
                let mut collected = Vec::new();
                while collected.len() < 3 {
                    if let Some(v) = consumer.pop() {
                        collected.push(v);
                    } else {
                        loom::thread::yield_now();
                    }
                }
                collected
            });

            t1.join().unwrap();
            let values = t2.join().unwrap();
            assert_eq!(values, vec![0, 1, 2]);
        });
    }

    #[test]
    fn concurrent_no_values_lost() {
        loom::model(|| {
            let (producer, consumer) = new::<i32, 2>();

            let t1 = loom::thread::spawn(move || {
                for i in 0..3 {
                    while producer.push(i).is_some() {
                        loom::thread::yield_now();
                    }
                }
            });

            let t2 = loom::thread::spawn(move || {
                let mut sum = 0;
                let mut count = 0;
                while count < 3 {
                    if let Some(v) = consumer.pop() {
                        sum += v;
                        count += 1;
                    } else {
                        loom::thread::yield_now();
                    }
                }
                (count, sum)
            });

            t1.join().unwrap();
            let (count, sum) = t2.join().unwrap();
            assert_eq!(count, 3);
            assert_eq!(sum, 0 + 1 + 2);
        });
    }

    /// Because there should always be only 1 producer thread.
    #[test]
    #[should_panic = "Causality violation: Concurrent write accesses to `UnsafeCell`.\n"]
    fn loom_detects_concurrent_producers() {
        loom::model(|| {
            let (producer, _) = new::<i32, 16>();
            let producer = Arc::new(producer);
            let p1 = producer.clone();
            let t1 = loom::thread::spawn(move || {
                p1.push(1);
            });
            let p2 = producer.clone();
            let t2 = loom::thread::spawn(move || {
                p2.push(2);
            });
            t1.join().unwrap();
            t2.join().unwrap();
        });
    }

    /// Because there should always be only 1 consumer thread.
    #[test]
    #[should_panic = "Causality violation: Concurrent read and write accesses.\n"]
    fn loom_detects_concurrent_consumers() {
        loom::model(|| {
            let (_, consumer) = new::<i32, 16>();
            let consumer = Arc::new(consumer);
            let c1 = consumer.clone();
            let t1 = loom::thread::spawn(move || {
                c1.pop();
            });
            let c2 = consumer.clone();
            let t2 = loom::thread::spawn(move || {
                c2.pop();
            });
            t1.join().unwrap();
            t2.join().unwrap();
        });
    }
}

#[cfg(test)]
#[cfg(feature = "tests_dhat")]
mod tests_dhat {
    use super::*;

    #[test]
    fn hot_path_zero_allocations() {
        let _profiler = dhat::Profiler::builder().testing().build();
        let (producer, consumer) = new::<u64, 1024>();

        let before = dhat::HeapStats::get();

        for i in 0..1024u64 {
            producer.push(i);
        }

        for _ in 0..1024 {
            consumer.pop();
        }

        let after = dhat::HeapStats::get();
        dhat::assert!(
            after.total_blocks - before.total_blocks == 0,
            "hot path allocated!"
        );
    }
}
