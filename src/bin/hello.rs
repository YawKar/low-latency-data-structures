use low_latency_data_structures::spsc;

fn main() {
    let (producer, consumer) = spsc::new::<i32, 128>();
    assert!(producer.push(123).is_none());
    assert!(matches!(consumer.pop(), Some(123)));
    println!("Hello");
}
