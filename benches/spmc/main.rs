use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use duplicate::duplicate;
use low_latency_data_structures::spmc::new;

criterion_main!(benches);

criterion_group!(benches, single_thread_ping_pong,);

/// A tiny deterministic smoking regression test. Catches inline regressions.
/// Measures cost of push/pop round-trip in 1 thread. No cross-core coherency.
/// No actual queuing.
fn single_thread_ping_pong(c: &mut Criterion) {
    let mut g = c.benchmark_group("spmc/single_thread_ping_pong");
    duplicate! {
        [
            CAPACITY;
            [64];
            [1024];
            [65536];
        ]
        {
            let capacity_label = CAPACITY;
            g.bench_function(format!("capacity={capacity_label}"), |b| {
                let (p, c) = new::<_, CAPACITY, 1>();
                let [mut c] = c;
                b.iter(|| {
                    black_box(p.publish(black_box(42)));
                    black_box(c.try_read());
                });
            });
        }
    }
}
