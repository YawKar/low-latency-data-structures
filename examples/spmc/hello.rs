//! Minimal SPMC broadcast demo: one producer publishes a few values, two
//! consumer threads each receive every value independently.

use std::thread;

use low_latency_data_structures::mem::Allocation;
use low_latency_data_structures::mem::global::GlobalAllocator;
use low_latency_data_structures::spmc::{self, ReadResult, Slot};

fn main() {
    let (producer, consumers) =
        spmc::new::<u64, 16, 2, GlobalAllocator>(spmc::Options::global_mlocked());
    let [mut c1, mut c2] = consumers;

    let producer_h = thread::spawn(move || {
        for i in 0..8 {
            producer.publish(i);
        }
    });

    let consumer_a = thread::spawn(move || collect(&mut c1, 8));
    let consumer_b = thread::spawn(move || collect(&mut c2, 8));

    producer_h.join().unwrap();
    let a = consumer_a.join().unwrap();
    let b = consumer_b.join().unwrap();
    println!("consumer A observed: {a:?}");
    println!("consumer B observed: {b:?}");
}

fn collect<AllocT: Allocation<Slot<u64>>>(
    c: &mut spmc::Consumer<u64, 16, AllocT>,
    n: usize,
) -> Vec<u64> {
    let mut seen = Vec::with_capacity(n);
    while seen.len() < n {
        match c.try_read() {
            ReadResult::Value(v) => seen.push(v),
            ReadResult::Lapped { skipped } => {
                eprintln!("consumer lapped, skipped {skipped} values");
            }
            ReadResult::Empty => std::hint::spin_loop(),
        }
    }
    seen
}
