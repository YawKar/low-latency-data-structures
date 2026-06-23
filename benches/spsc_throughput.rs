use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use duplicate::duplicate;
use low_latency_data_structures::spsc;

fn bench_cross_thread_throughput(c: &mut Criterion) {
    let core_ids = core_affinity::get_core_ids().unwrap();
    // They should share L3 cache
    let producer_core = core_ids[0];
    let consumer_core = core_ids[1];

    let mut group = c.benchmark_group("spsc_cross_thread_throughput");

    duplicate! {
        [
            CAPACITY;
            [64];
            [1024];
            [65536];
        ]
        {
            group.throughput(criterion::Throughput::Elements(CAPACITY as u64));
            let capacity_label = CAPACITY;
            group.bench_function(format!("capacity_{capacity_label}"), |b| {
                b.iter_custom(|iters| {
                    // Each criterion "iteration" = one full fill+drain of the queue.
                    // Total items = iters * capacity.
                    let total_items = iters as usize * CAPACITY;
                    let (producer, consumer) = spsc::new::<u64, CAPACITY>();

                    let pc = producer_core;
                    let cc = consumer_core;

                    let elapsed = std::time::Instant::now();

                    let t1 = std::thread::spawn(move || {
                        core_affinity::set_for_current(pc);
                        for i in 0..total_items as u64 {
                            while producer.push(i).is_some() {
                                std::hint::spin_loop();
                            }
                        }
                    });

                    let t2 = std::thread::spawn(move || {
                        core_affinity::set_for_current(cc);
                        for _ in 0..total_items {
                            while consumer.pop().is_none() {
                                std::hint::spin_loop();
                            }
                        }
                    });

                    t1.join().unwrap();
                    t2.join().unwrap();

                    elapsed.elapsed()
                });
            });
        }
    };
    group.finish();
}

fn bench_ping_pong_single_thread(c: &mut Criterion) {
    let mut group = c.benchmark_group("spsc_ping_pong_single_thread");
    duplicate! {
        [
            CAPACITY;
            [64];
            [1024];
            [65536];
        ]
        {
            let capacity_label = CAPACITY;
            group.bench_function(format!("capacity_{capacity_label}"), |b| {
                let (producer, consumer) = spsc::new::<u64, CAPACITY>();
                b.iter(|| {
                    black_box(producer.push(black_box(42u64)));
                    black_box(consumer.pop());
                });
            });
        }
    }
}

criterion_group!(
    benches,
    bench_ping_pong_single_thread,
    bench_cross_thread_throughput
);
criterion_main!(benches);
