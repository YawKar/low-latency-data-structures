//! Throttled-producer offered-load sweep with coordinated-omission
//! correction. See `bench::harness::spsc::throttled` for the method.
//!
//! Impl selection: `BENCH_IMPL=<name>` picks which SPSC to bench.
//! Available names depend on cargo features:
//!   - `ours`               (default; always available under `_bench_utils`)
//!   - `rtrb`               (requires `--features _bench_rtrb`)
//!   - `ringbuf`            (requires `--features _bench_ringbuf`)
//!   - `heapless`           (requires `--features _bench_heapless`)
//!   - `nexus`              (requires `--features _bench_nexus`)
//!   - `crossbeam-channel`  (requires `--features _bench_cbchan`)  WARN: MPMC-as-SPSC
//!   - `flume`              (requires `--features _bench_flume`)   WARN: MPMC-as-SPSC
//!   - `std-mpsc`           (requires `--features _bench_stdmpsc`) WARN: mutex+condvar
//!
//! `BENCH_DEBUG=1` additionally captures SYSTEM latency (`now - push_tsc`).
//! Meaningful only when the run is NOT saturated; suppressed otherwise.
//!
//! `BENCH_RATES=1000000,10000000,...` overrides the default sweep.
//!
//! Required environment (identical to bench_handoff_under_coherency_contention.rs):
//!   isolcpus=<P>,<C> nohz_full=<P>,<C> rcu_nocbs=<P>,<C>
//!   intel_idle.max_cstate=0 processor.max_cstate=0
//!   performance governor, no_turbo=1, SMT siblings offline, ulimit -l.
//!   Pass env vars through sudo with `sudo -E env BENCH_DEBUG=1 ...`.

use low_latency_data_structures::bench::harness::TwoCoreCtx;
#[cfg(feature = "_bench_cbchan")]
use low_latency_data_structures::bench::harness::adapters::cbchan_spsc::CbChanSpsc;
#[cfg(feature = "_bench_flume")]
use low_latency_data_structures::bench::harness::adapters::flume_spsc::FlumeSpsc;
#[cfg(feature = "_bench_heapless")]
use low_latency_data_structures::bench::harness::adapters::heapless_spsc::HeaplessSpsc;
#[cfg(feature = "_bench_nexus")]
use low_latency_data_structures::bench::harness::adapters::nexus_spsc::NexusSpsc;
use low_latency_data_structures::bench::harness::adapters::ours_spsc::OursSpsc;
#[cfg(feature = "_bench_ringbuf")]
use low_latency_data_structures::bench::harness::adapters::ringbuf_spsc::RingbufSpsc;
#[cfg(feature = "_bench_rtrb")]
use low_latency_data_structures::bench::harness::adapters::rtrb_spsc::RtrbSpsc;
#[cfg(feature = "_bench_stdmpsc")]
use low_latency_data_structures::bench::harness::adapters::stdmpsc_spsc::StdMpscSpsc;
use low_latency_data_structures::bench::harness::spsc::{
    SpscThrottledCfg, Stamped, run_spsc_throttled,
};

// Capacity sized to absorb the worst micro-burst from a single LOC tick on
// nohz_full without triggering false saturation, while still being small
// enough that real saturation backs up promptly.
const CAPACITY: usize = 4096;

fn parse_rates(s: &str) -> Vec<u64> {
    s.split(',')
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(|t| {
            t.parse::<u64>()
                .unwrap_or_else(|_| panic!("invalid rate in BENCH_RATES: {t:?}"))
        })
        .collect()
}

fn main() {
    let ctx = TwoCoreCtx::discover_and_preflight();
    println!(
        "TSC freq: {} Hz ({:.3} GHz)",
        ctx.tsc_hz,
        ctx.tsc_hz as f64 / 1e9
    );

    let mut cfg = SpscThrottledCfg::default();
    if let Ok(s) = std::env::var("BENCH_RATES") {
        cfg.rates_hz = parse_rates(&s);
    }
    cfg.capture_sys_latency = std::env::var("BENCH_DEBUG")
        .map(|v| v == "1")
        .unwrap_or(false);

    let impl_name = std::env::var("BENCH_IMPL").unwrap_or_else(|_| "ours".to_string());
    let _report = match impl_name.as_str() {
        "ours" => run_spsc_throttled::<OursSpsc<Stamped, CAPACITY>, CAPACITY>(&ctx, cfg),
        #[cfg(feature = "_bench_rtrb")]
        "rtrb" => run_spsc_throttled::<RtrbSpsc<Stamped, CAPACITY>, CAPACITY>(&ctx, cfg),
        #[cfg(feature = "_bench_ringbuf")]
        "ringbuf" => run_spsc_throttled::<RingbufSpsc<Stamped, CAPACITY>, CAPACITY>(&ctx, cfg),
        #[cfg(feature = "_bench_heapless")]
        "heapless" => run_spsc_throttled::<
            HeaplessSpsc<Stamped, CAPACITY, { CAPACITY + 1 }>,
            CAPACITY,
        >(&ctx, cfg),
        #[cfg(feature = "_bench_nexus")]
        "nexus" => run_spsc_throttled::<NexusSpsc<Stamped, CAPACITY>, CAPACITY>(&ctx, cfg),
        #[cfg(feature = "_bench_cbchan")]
        "crossbeam-channel" => {
            run_spsc_throttled::<CbChanSpsc<Stamped, CAPACITY>, CAPACITY>(&ctx, cfg)
        }
        #[cfg(feature = "_bench_flume")]
        "flume" => run_spsc_throttled::<FlumeSpsc<Stamped, CAPACITY>, CAPACITY>(&ctx, cfg),
        #[cfg(feature = "_bench_stdmpsc")]
        "std-mpsc" => run_spsc_throttled::<StdMpscSpsc<Stamped, CAPACITY>, CAPACITY>(&ctx, cfg),
        other => panic!(
            "unknown BENCH_IMPL={other:?}. Available: 'ours' (always), \
             'rtrb' (requires --features _bench_rtrb), \
             'ringbuf' (requires --features _bench_ringbuf), \
             'heapless' (requires --features _bench_heapless), \
             'nexus' (requires --features _bench_nexus), \
             'crossbeam-channel' (requires --features _bench_cbchan), \
             'flume' (requires --features _bench_flume), \
             'std-mpsc' (requires --features _bench_stdmpsc)."
        ),
    };
}
