//! Minimal SeqLock demo: a writer thread updates a u64 in a loop, a reader
//! thread observes consistent (non-torn) snapshots and prints a few of them.

use std::thread;

use low_latency_data_structures::seqlock;

fn main() {
    let (writer, reader) = seqlock::new(0u64);

    let writer_h = thread::spawn(move || {
        for i in 1..=1000 {
            writer.write(i);
        }
    });

    let reader_h = thread::spawn(move || {
        let mut samples = Vec::new();
        for _ in 0..5 {
            samples.push(reader.read());
            std::thread::sleep(std::time::Duration::from_micros(100));
        }
        samples
    });

    writer_h.join().unwrap();
    let samples = reader_h.join().unwrap();
    println!("reader sampled: {samples:?}");
}
