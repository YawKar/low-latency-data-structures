use std::mem::MaybeUninit;

use anyhow::bail;

use crate::mem::allocate_buffer;
use crate::shim::cell::UnsafeCell;
use crate::shim::sync::{Arc, atomic};
use crate::spsc::consumer::{Consumer, ConsumerState};
use crate::spsc::producer::{Producer, ProducerState};

pub fn new<T>(capacity: usize) -> anyhow::Result<(Producer<T>, Consumer<T>)> {
    if !capacity.is_power_of_two() {
        bail!("the given capacity is not power of two: {capacity}");
    }
    let slots_buffer = allocate_buffer(capacity, false);
    let q = Arc::new(Queue {
        producer_state: ProducerState {
            tail: Default::default(),
            cached_head: Default::default(),
        },
        consumer_state: ConsumerState {
            head: Default::default(),
            cached_tail: Default::default(),
        },
        slots: slots_buffer,
        capacity,
    });
    let producer = Producer::new(q.clone());
    let consumer = Consumer::new(q);
    Ok((producer, consumer))
}

#[repr(C)]
pub(super) struct Queue<T> {
    producer_state: ProducerState,
    consumer_state: ConsumerState,
    slots: *mut UnsafeCell<MaybeUninit<T>>,
    capacity: usize,
}

impl<T> Queue<T> {
    #[inline]
    pub fn pop(&self) -> Option<T> {
        let head = self.consumer_state.head.load(atomic::Ordering::Relaxed);
        if head == self.consumer_state.cached_tail.get() {
            // it's still may not be empty
            if self.pop_slow_check(head) {
                return None;
            }
        }
        let slot_ptr = self.slots.wrapping_add(head & (self.capacity - 1));
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
    fn pop_slow_check(&self, head: usize) -> bool {
        self.consumer_state
            .cached_tail
            .set(self.producer_state.tail.load(atomic::Ordering::Acquire));
        head == self.consumer_state.cached_tail.get()
    }

    #[inline]
    pub fn push(&self, item: T) -> Option<T> {
        let tail = self.producer_state.tail.load(atomic::Ordering::Relaxed);
        debug_assert!(tail.wrapping_sub(self.producer_state.cached_head.get()) <= self.capacity);
        if tail.wrapping_sub(self.producer_state.cached_head.get()) >= self.capacity {
            // it's still may not be full
            if self.push_slow_check(tail) {
                return Some(item);
            }
        }
        let slot_ptr = self.slots.wrapping_add(tail & (self.capacity - 1));
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
    fn push_slow_check(&self, tail: usize) -> bool {
        self.producer_state
            .cached_head
            .set(self.consumer_state.head.load(atomic::Ordering::Acquire));
        debug_assert!(tail.wrapping_sub(self.producer_state.cached_head.get()) <= self.capacity);
        tail.wrapping_sub(self.producer_state.cached_head.get()) >= self.capacity
    }
}

impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        let head = self.consumer_state.head.load(atomic::Ordering::Relaxed);
        let tail = self.producer_state.tail.load(atomic::Ordering::Relaxed);
        for i in head..tail {
            let slot_ptr = self.slots.wrapping_add(i & (self.capacity - 1));
            // SAFETY: it's not null because `i & (sef.capacity - 1)` limits it to [0;
            // allocated_cap). And it can be safely converted to a reference because T is self
            // contained bitwise.
            unsafe {
                slot_ptr.as_ref_unchecked().with_mut(|ptr| {
                    ptr.as_mut_unchecked().assume_init_drop();
                })
            }
        }
        #[cfg(feature = "tests_loom")]
        unsafe {
            drop(Vec::from_raw_parts(
                self.slots,
                self.capacity,
                self.capacity,
            ))
        }
        #[cfg(not(feature = "tests_loom"))]
        {
            use crate::shim::alloc;
            let layout = alloc::Layout::array::<UnsafeCell<MaybeUninit<T>>>(self.capacity).unwrap();
            // TODO: when hugepages are used, should use libc::munmap instead
            // SAFETY: Queue is share-owned by 2 Arc objects (Producer/Consumer), so double-free is
            // not possible unless Producer/Consumer are Sync
            unsafe {
                static_assertions::assert_impl_all!(Producer<u32>: Send);
                static_assertions::assert_not_impl_any!(Producer<u32>: Sync);
                static_assertions::assert_impl_all!(Consumer<u32>: Send);
                static_assertions::assert_not_impl_any!(Consumer<u32>: Sync);
                alloc::dealloc(self.slots.cast::<u8>(), layout)
            };
        }
    }
}

#[cfg(test)]
#[cfg(feature = "tests_basic")]
mod tests_basic {
    use std::rc::Rc;
    use std::thread;

    use super::*;
    use crate::shim::cell::Cell;

    #[test]
    fn move_producer_consumer_to_threads() -> anyhow::Result<()> {
        let (producer, consumer) = new(2)?;
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
        Ok(())
    }

    #[test]
    fn handoff_one_value() -> anyhow::Result<()> {
        let (producer, consumer) = new(2)?;
        assert_eq!(producer.push(123), None);
        assert_eq!(consumer.pop(), Some(123));
        Ok(())
    }

    #[test]
    fn allows_queues_with_powers_of_two_capacity() -> anyhow::Result<()> {
        for power in 0..20 {
            new::<()>(2usize.pow(power))?;
        }
        Ok(())
    }

    #[test]
    fn prohibits_queues_with_not_powers_of_two_capacity() -> anyhow::Result<()> {
        for power in 2..20 {
            assert!(matches!(new::<()>(2usize.pow(power) - 1), Err(_)));
            assert!(matches!(new::<()>(2usize.pow(power) + 1), Err(_)));
        }
        Ok(())
    }

    #[test]
    fn drops_unread_items() -> anyhow::Result<()> {
        let counter = Rc::new(Cell::new(0));
        #[derive(Debug, PartialEq)]
        struct Droppable {
            counter: Rc<Cell<i32>>,
        }
        impl Drop for Droppable {
            fn drop(&mut self) {
                let cnt = self.counter.get();
                self.counter.set(cnt + 1);
            }
        }
        let capacity = 64i32;
        let (producer, consumer) = new(capacity as usize)?;
        for _ in 0..capacity {
            let counter = counter.clone();
            assert_eq!(producer.push(Droppable { counter }), None);
        }
        let read = capacity / 2;
        for _ in 0..read {
            assert!(matches!(consumer.pop(), Some(_)));
        }
        assert_eq!(read, counter.get());
        drop(producer);
        drop(consumer);
        assert_eq!(capacity, counter.get());
        Ok(())
    }

    #[test]
    fn empty_returns_none() -> anyhow::Result<()> {
        let (_, consumer) = new::<i32>(4)?;
        assert_eq!(consumer.pop(), None);
        assert_eq!(consumer.pop(), None);
        Ok(())
    }

    #[test]
    fn full_returns_item_back() -> anyhow::Result<()> {
        let (producer, _) = new::<i32>(2)?;
        assert_eq!(producer.push(1), None);
        assert_eq!(producer.push(2), None);
        // Full — item comes back untouched
        assert_eq!(producer.push(3), Some(3));
        assert_eq!(producer.push(4), Some(4));
        Ok(())
    }

    #[test]
    fn fifo_ordering() -> anyhow::Result<()> {
        let (producer, consumer) = new(8)?;
        for i in 0..8 {
            assert_eq!(producer.push(i), None);
        }
        for i in 0..8 {
            assert_eq!(consumer.pop(), Some(i));
        }
        Ok(())
    }

    #[test]
    fn wraparound_n_laps() -> anyhow::Result<()> {
        let capacity = 4;
        let laps = 100;
        let (producer, consumer) = new(capacity)?;
        for lap in 0..laps {
            for i in 0..capacity {
                let val = lap * capacity + i;
                assert_eq!(producer.push(val), None, "push failed at lap {lap}, i {i}");
            }
            // Queue is full
            assert_eq!(producer.push(9999), Some(9999));
            for i in 0..capacity {
                let val = lap * capacity + i;
                assert_eq!(consumer.pop(), Some(val), "wrong value at lap {lap}, i {i}");
            }
            // Queue is empty
            assert_eq!(consumer.pop(), None);
        }
        Ok(())
    }

    #[test]
    fn interleaved_push_pop() -> anyhow::Result<()> {
        let (producer, consumer) = new(2)?;
        // Push 1, pop 1, repeat — tests wraparound with tiny queue
        for i in 0..1000 {
            assert_eq!(producer.push(i), None);
            assert_eq!(consumer.pop(), Some(i));
        }
        Ok(())
    }

    #[test]
    fn capacity_one() -> anyhow::Result<()> {
        let (producer, consumer) = new(1)?;
        assert_eq!(consumer.pop(), None);
        assert_eq!(producer.push(42), None);
        assert_eq!(producer.push(43), Some(43)); // full
        assert_eq!(consumer.pop(), Some(42));
        assert_eq!(consumer.pop(), None); // empty again
        Ok(())
    }

    #[test]
    fn move_only_type() -> anyhow::Result<()> {
        // Verify non-Copy, non-Clone types work
        let (producer, consumer) = new(4)?;
        let s = String::from("hello");
        assert_eq!(producer.push(s), None);
        let got = consumer.pop().unwrap();
        assert_eq!(got, "hello");
        Ok(())
    }
}

#[cfg(test)]
#[cfg(feature = "tests_loom")]
mod tests_loom {
    use super::*;

    #[test]
    fn concurrent_push_pop() {
        loom::model(|| {
            let (producer, consumer) = new::<i32>(4).unwrap();

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
            let (producer, consumer) = new::<i32>(1).unwrap();

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
            let (producer, consumer) = new::<i32>(2).unwrap();

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
            let (producer, _) = new::<i32>(16).unwrap();
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
            let (_, consumer) = new::<i32>(16).unwrap();
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

    #[global_allocator]
    static ALLOC: dhat::Alloc = dhat::Alloc;

    #[test]
    fn hot_path_zero_allocations() {
        let _profiler = dhat::Profiler::builder().testing().build();
        let (producer, consumer) = new::<u64>(1024).unwrap();

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
