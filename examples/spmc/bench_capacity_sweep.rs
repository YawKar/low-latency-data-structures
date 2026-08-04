//! Capacity sweep with a sustained producer. The producer publishes flat
//! out for a fixed wall-clock window. A single consumer reads as fast as
//! it can. We sweep CAPACITY from very small (producer and consumer touch
//! the same cache lines, lots of coherency traffic) to very large (the
//! producer is far ahead, the consumer reads slot lines that the producer
//! wrote long ago and that have likely settled into shared L2 or L3 state
//! already).
//!
//! What this is meant to answer: how does the read-latency distribution
//! and the lap rate change when the producer and consumer working sets
//! overlap versus separate. With packed slots (multiple slots per cache
//! line, the current layout for `ours`) we expect small CAPACITY to look
//! worst, because every producer write invalidates the cache line that the
//! consumer is about to touch. Large CAPACITY should look better because
//! the producer write and the consumer read land on different cache lines.
//!
//! Impl selection: `BENCH_IMPL=<name>` picks which SPMC broadcast to bench.
//! Available names depend on cargo features:
//!   - `ours`  (default; always available under `_bench_utils`)
//!   - `bus`   (requires `--features _bench_bus`)  WARN: backpressure, mutex+condvar
//!
//! For overwriting impls (ours) the lapped column is meaningful. For
//! backpressure impls (bus) the lapped column is always 0; that row shows
//! the joint publisher/consumer roundtrip cost under backpressure instead.
//!
//! `BENCH_RUN_SECS=N` overrides the per-capacity run length (default 2s).
//!
//! Required environment: see other spmc bench binaries.
//!
use std::env;

use duplicate::duplicate;
use low_latency_data_structures::bench::harness::TwoCoreCtx;
#[cfg(feature = "_bench_bus")]
use low_latency_data_structures::bench::harness::adapters::bus_spmc::BusSpmc;
use low_latency_data_structures::bench::harness::adapters::ours_spmc::OursSpmc;
use low_latency_data_structures::bench::harness::spmc::{
    SpmcCapacitySweepCfg, print_capacity_row, run_spmc_capacity_sweep_one,
};

fn main() {
    let ctx = TwoCoreCtx::discover_and_preflight();
    let mut cfg = SpmcCapacitySweepCfg::default();
    if let Ok(s) = env::var("BENCH_RUN_SECS") {
        cfg.run_secs = s.parse().unwrap_or(cfg.run_secs);
    }
    println!(
        "TSC freq: {} Hz ({:.3} GHz). run={}s per capacity",
        ctx.tsc_hz,
        ctx.tsc_hz as f64 / 1e9,
        cfg.run_secs,
    );

    let impl_name = env::var("BENCH_IMPL").unwrap_or_else(|_| "ours".to_string());
    println!("== {impl_name} ==");
    println!(
        "{:>10} {:>12} {:>11} {:>11} {:>9} {:>10} {:>10} {:>10}",
        "capacity", "published", "values", "lapped", "p50", "p99", "p99.9", "max"
    );

    match impl_name.as_str() {
        "ours" => {
            duplicate! {
                [ CAP; [16]; [256]; [4096]; [65536]; [1048576]; ]
                {
                    let r = run_spmc_capacity_sweep_one::<OursSpmc<u64, CAP>, CAP>(&ctx, cfg);
                    print_capacity_row(&r, &ctx);
                }
            }
        }
        #[cfg(feature = "_bench_bus")]
        "bus" => {
            duplicate! {
                [ CAP; [16]; [256]; [4096]; [65536]; [1048576]; ]
                {
                    let r = run_spmc_capacity_sweep_one::<BusSpmc<u64, CAP>, CAP>(&ctx, cfg);
                    print_capacity_row(&r, &ctx);
                }
            }
        }
        other => panic!(
            "unknown BENCH_IMPL={other:?}. Available: 'ours' (always), \
             'bus' (requires --features _bench_bus)."
        ),
    }
}
