use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::seqlock::reader::Reader;
use crate::seqlock::writer::Writer;

pub fn new<T: Copy>(initial_value: T) -> (Writer<T>, Reader<T>) {
    let sl = Arc::new(SeqLock {
        seq: AtomicU64::new(0),
        data: Cell::new(initial_value),
    });
    let writer = Writer::new(sl.clone());
    let reader = Reader::new(sl);
    (writer, reader)
}

#[repr(C, align(128))]
pub(super) struct SeqLock<T: Copy> {
    seq: AtomicU64,
    data: Cell<T>,
}

// SAFETY: there will only be 1 writer at any time, thus it's safe to call write() and utilize
// access through Arc<SeqLock<T>> in Writer.
unsafe impl<T: Copy> Sync for SeqLock<T> {}

impl<T: Copy> SeqLock<T> {
    #[inline]
    pub(super) fn write(&self, value: T) {
        self.seq.fetch_add(1, Ordering::Release);
        // SAFETY: SeqLock utilizes UB and volatile here adds guarantees that neither compiler, nor
        // processor will try to reorder things
        unsafe { self.data.as_ptr().write_volatile(value) };
        self.seq.fetch_add(1, Ordering::Release);
    }

    #[inline]
    pub(super) fn read(&self) -> T {
        loop {
            let s1 = self.seq.load(Ordering::Acquire);
            if s1 & 1 == 1 {
                continue;
            }
            // SAFETY: look at SAFETY commentary in write method
            let read_attempt = unsafe { self.data.as_ptr().read_volatile() };
            let s2 = self.seq.load(Ordering::Acquire);
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
            .into_iter()
            .map(|_| {
                let barrier = barrier.clone();
                let reader = reader.clone();
                thread::spawn(move || {
                    let mut collected: Vec<u64> = vec![];
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
