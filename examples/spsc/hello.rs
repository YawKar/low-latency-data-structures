//! Minimal SPSC demo: a producer thread pushes a few integers, a consumer
//! thread pops them and prints what it saw.

use std::thread;

use low_latency_data_structures::mem::global::GlobalAllocator;
use low_latency_data_structures::spsc;

fn main() {
    let (producer, consumer) =
        spsc::new::<u64, 16, GlobalAllocator>(spsc::Options::global_mlocked());

    let producer_h = thread::spawn(move || {
        for i in 0..8 {
            while producer.push(i).is_some() {
                std::hint::spin_loop();
            }
        }
    });

    let consumer_h = thread::spawn(move || {
        let mut seen = Vec::with_capacity(8);
        while seen.len() < 8 {
            if let Some(v) = consumer.pop() {
                seen.push(v);
            } else {
                std::hint::spin_loop();
            }
        }
        seen
    });

    producer_h.join().unwrap();
    let seen = consumer_h.join().unwrap();
    println!("consumer observed: {seen:?}");
}
