use low_latency_data_structures::{seqlock, spsc};

fn main() {
    println!("hello from smoke tests");
    smoke_spsc();
    smoke_seqlock();
    println!("smoke tests seem ok");
}

fn smoke_spsc() {
    println!("smoke_spsc...");
    let (producer, consumer) = spsc::new::<i32, 128>();
    assert!(producer.push(123).is_none());
    assert!(matches!(consumer.pop(), Some(123)));
}

fn smoke_seqlock() {
    println!("smoke_seqlock...");
    let (writer, reader) = seqlock::new(0);
    writer.write(123);
    assert_eq!(reader.read(), 123);
}
