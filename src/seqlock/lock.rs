use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering, fence};

use crate::seqlock::reader::Reader;
use crate::seqlock::writer::Writer;

pub fn new<T: bytemuck::AnyBitPattern>(initial_value: T) -> (Writer<T>, Reader<T>) {
    let sl = Arc::new(SeqLock {
        seq: AtomicU64::new(0),
        data: UnsafeCell::new(initial_value),
    });
    let writer = Writer::new(sl.clone());
    let reader = Reader::new(sl);
    (writer, reader)
}

/// 128 = adjacent-line prefetcher granularity, not cache line size
#[repr(C, align(128))]
pub(super) struct SeqLock<T: bytemuck::AnyBitPattern> {
    seq: AtomicU64,
    data: UnsafeCell<T>,
}

// SAFETY: there will only be 1 writer at any time, thus it's safe to call write() and utilize
// access through Arc<SeqLock<T>> in Writer.
unsafe impl<T: bytemuck::AnyBitPattern> Sync for SeqLock<T> {}

impl<T: bytemuck::AnyBitPattern> SeqLock<T> {
    #[inline]
    pub(super) fn write(&self, value: T) {
        let s = self.seq.load(Ordering::Relaxed);
        self.seq.store(s.wrapping_add(1), Ordering::Relaxed);
        // ARM: prevents the write from reordering above the s load
        fence(Ordering::Release);
        // SAFETY: SeqLock utilizes UB and volatile here adds guarantees that neither compiler, nor
        // processor will try to reorder things
        unsafe { self.data.get().write_volatile(value) };
        self.seq.store(s.wrapping_add(2), Ordering::Release);
    }

    #[inline]
    pub(super) fn read(&self) -> T {
        loop {
            let s1 = self.seq.load(Ordering::Acquire);
            if s1 & 1 == 1 {
                continue;
            }
            // SAFETY: look at SAFETY commentary in write method
            let read_attempt = unsafe { self.data.get().read_volatile() };
            // ARM: prevents the read from reordering below the s2 load
            fence(Ordering::Acquire);
            let s2 = self.seq.load(Ordering::Relaxed);
            if s1 == s2 {
                return read_attempt;
            }
        }
    }
}

#[cfg(feature = "tests_basic")]
#[cfg(test)]
mod tests {
    use std::sync::Barrier;
    use std::thread;

    use super::*;

    #[test]
    fn first_read_returns_initial_value() {
        for init_value in [42, 1, 23] {
            let (_, reader) = new(init_value);
            assert_eq!(reader.read(), init_value);
        }
    }

    #[test]
    fn single_thread_reads_what_it_has_written() {
        let (writer, reader) = new(0);
        for value in [42, 1, 23] {
            writer.write(value);
            assert_eq!(reader.read(), value);
        }
    }

    #[test]
    fn single_writer_multi_reader() {
        const READERS: usize = 3;
        const VALUES: [u64; 4] = [1, 4, 7, 9];
        let (writer, reader) = new(0);
        let reader = Arc::new(reader);
        let barrier = Arc::new(Barrier::new(READERS + 1));
        let writer_h = {
            let barrier = barrier.clone();
            thread::spawn(move || {
                // wait for readers to read the initial value
                barrier.wait();
                for value in VALUES {
                    writer.write(value);
                    // waits for all readers to get the value
                    barrier.wait();
                }
            })
        };
        let readers_hs: Vec<_> = (0..READERS)
            .map(|_| {
                let barrier = barrier.clone();
                let reader = reader.clone();
                thread::spawn(move || {
                    let mut collected: Vec<u64> = Vec::with_capacity(VALUES.len() + 1);
                    loop {
                        if collected.len() == VALUES.len() + 1 {
                            break;
                        }
                        let item = reader.read();
                        if let Some(&last) = collected.last()
                            && last == item
                        {
                            continue;
                        } else {
                            collected.push(item);
                            barrier.wait();
                        }
                    }
                    collected
                })
            })
            .collect();
        writer_h.join().unwrap();
        let readers_results: Vec<Vec<u64>> =
            readers_hs.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(readers_results.iter().all(
            |result| result[1..VALUES.len() + 1] == VALUES && result.len() - 1 == VALUES.len()
        ));
    }
}
