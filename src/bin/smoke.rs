//! End-to-end smoke binary. Exercises one trivial path through each
//! primitive so a release build catches obvious regressions before the
//! benchmark suite runs.

#![cfg(not(feature = "tests_loom"))]

use low_latency_data_structures::mem::global::GlobalAllocator;
use low_latency_data_structures::spmc::{self, ReadResult};
use low_latency_data_structures::{seqlock, spsc};

fn main() {
    println!("hello from smoke tests");
    smoke_spsc();
    smoke_seqlock();
    smoke_spmc();
    println!("smoke tests seem ok");
}

fn smoke_spsc() {
    println!("smoke_spsc...");
    let (producer, consumer) =
        spsc::new::<i32, 128, GlobalAllocator>(spsc::Options::global_mlocked());
    assert!(producer.push(123).is_none());
    assert!(matches!(consumer.pop(), Some(123)));
}

fn smoke_seqlock() {
    println!("smoke_seqlock...");
    let (writer, reader) = seqlock::new(0);
    writer.write(123);
    assert_eq!(reader.read(), 123);
}

fn smoke_spmc() {
    println!("smoke_spmc...");
    let (producer, consumers) =
        spmc::new::<i32, 128, 2, GlobalAllocator>(spmc::Options::global_mlocked());
    let [mut c1, mut c2] = consumers;
    assert_eq!(c1.try_read(), ReadResult::Empty);
    assert_eq!(c2.try_read(), ReadResult::Empty);
    producer.publish(123);
    assert_eq!(c1.try_read(), ReadResult::Value(123));
    assert_eq!(c2.try_read(), ReadResult::Value(123));
    assert_eq!(c1.try_read(), ReadResult::Empty);
    assert_eq!(c2.try_read(), ReadResult::Empty);
}
