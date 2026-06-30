use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering, fence};

use crate::seqlock::reader::Reader;
use crate::seqlock::writer::Writer;

/// Creates a new seqlock initialised with `initial_value`.
///
/// Returns a paired `(Writer, Reader)`. Additional readers can be obtained
/// by cloning the [`Reader`].
///
/// # Examples
///
/// ```
/// use low_latency_data_structures::seqlock::new;
///
/// let (writer, reader) = new(0u64);
/// assert_eq!(reader.read(), 0);
/// writer.write(42);
/// assert_eq!(reader.read(), 42);
/// ```
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

// SAFETY: UnsafeCell is not Sync but access is synchronised through the
// write()/read() methods using the seq-number protocol. Torn reads of `data`
// would be undefined under the abstract C11/C++11 model, but `T:
// AnyBitPattern` guarantees every bit pattern materialises a valid T, so the
// worst-case observed value is stale, never UB. The seq check around the
// read rejects stale or torn values before they leave `Reader::read`.
unsafe impl<T: bytemuck::AnyBitPattern> Sync for SeqLock<T> {}

impl<T: bytemuck::AnyBitPattern> SeqLock<T> {
    #[inline]
    pub(super) fn write(&self, value: T) {
        let s = self.seq.load(Ordering::Relaxed);
        self.seq.store(s.wrapping_add(1), Ordering::Relaxed);
        // ARM: prevents the write from reordering above the s load
        fence(Ordering::Release);
        // SAFETY: see the unsafe impl Sync for SeqLock above. write_volatile
        // also pins the store: it cannot be reordered with the surrounding
        // seq stores by the compiler.
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
            // SAFETY: see the unsafe impl Sync for SeqLock above. The torn
            // read is filtered by the s1==s2 check below.
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
        assert!(
            readers_results
                .iter()
                .all(|result| result.len() == VALUES.len() + 1 && &result[1..] == &VALUES[..])
        );
    }
}

#[cfg(test)]
#[cfg(feature = "tests_dhat")]
mod tests_dhat {
    use std::hint::black_box;

    use super::*;

    #[test]
    fn hot_path_without_allocations() {
        let _profiler = dhat::Profiler::builder().testing().build();
        let (writer, reader) = new(0u64);
        const ITERS: u64 = 10_000_000;

        // Warm up to absorb any one-time platform allocations: lazy symbol
        // resolution in the dynamic linker, libstd TLS init, debug-build
        // slow-path stubs that LLVM emits for overflow/panic messages. None
        // of these scale with iteration count; the per-iteration regression
        // we actually care about would dwarf the slack budget below.
        for i in 0..10_000u64 {
            writer.write(black_box(i));
            black_box(reader.read());
        }

        let before = dhat::HeapStats::get();
        for i in 0..ITERS {
            writer.write(black_box(i));
            black_box(reader.read());
        }
        let after = dhat::HeapStats::get();

        let allocs = after.total_blocks - before.total_blocks;
        assert!(
            allocs < 64,
            "hot path allocated {allocs} blocks over {ITERS} iterations; \
             a real regression would be O(ITERS), this slack absorbs O(1) \
             platform noise (lazy linker, debug stubs)"
        );
    }
}
