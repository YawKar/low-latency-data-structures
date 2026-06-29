use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use low_latency_data_structures::seqlock::new;

criterion_main!(benches);
criterion_group!(benches, single_thread_write_read);

/// A tiny deterministic smoking regression test. Catches inline regressions.
/// Measures cost of write/read round-trip in 1 thread. No cross-core coherency.
/// No actual queuing.
fn single_thread_write_read(c: &mut Criterion) {
    let mut g = c.benchmark_group("seqlock/single_thread_write_read");
    g.bench_function("ping_pong_rtt", |b| {
        let (writer, reader) = new(0);
        b.iter(|| {
            black_box(writer.write(black_box(42)));
            black_box(reader.read());
        });
    });
}
