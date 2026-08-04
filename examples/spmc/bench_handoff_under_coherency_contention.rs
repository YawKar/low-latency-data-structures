//! Direct one-way handoff latency under coherency contention: producer
//! publishes an rdtscp timestamp, the sole consumer reads and records
//! `now - ts`. Relies on invariant_tsc + nonstop_tsc being synchronized
//! across cores on the same socket (preflight checks this).
//!
//! Impl selection: `BENCH_IMPL=<name>` picks which SPMC broadcast to bench.
//! Available names depend on cargo features:
//!   - `ours`  (default; always available under `_bench_utils`)
//!   - `bus`   (requires `--features _bench_bus`)  WARN: backpressure, mutex+condvar
//!
//! Required environment:
//! - Kernel cmdline:
//!     isolcpus=<P>,<C> nohz_full=<P>,<C> rcu_nocbs=<P>,<C>
//!     intel_idle.max_cstate=0 processor.max_cstate=0
//! - `echo performance > /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor`
//! - `echo 1 > /sys/devices/system/cpu/intel_pstate/no_turbo`
//! - Offline SMT siblings of the two bench cores (or isolate them too).
//! - Pick two cores that share L3 but are different physical cores (lscpu -e).
//! - Run with `ulimit -l unlimited` (or sudo) so mlockall succeeds.
//!
use low_latency_data_structures::bench::harness::TwoCoreCtx;
#[cfg(feature = "_bench_bus")]
use low_latency_data_structures::bench::harness::adapters::bus_spmc::BusSpmc;
use low_latency_data_structures::bench::harness::adapters::ours_spmc::OursSpmc;
use low_latency_data_structures::bench::harness::spmc::{SpmcHandoffCfg, run_spmc_handoff};

fn main() {
    let ctx = TwoCoreCtx::discover_and_preflight();
    let cfg = SpmcHandoffCfg::default();

    // Capacity 1 keeps the queue at depth 0 or 1, so each measurement
    // reflects pure handoff (publish -> read), not in-queue residence time.
    const CAPACITY: usize = 1;

    let impl_name = std::env::var("BENCH_IMPL").unwrap_or_else(|_| "ours".to_string());
    let report = match impl_name.as_str() {
        "ours" => run_spmc_handoff::<OursSpmc<u64, CAPACITY>, CAPACITY>(&ctx, cfg),
        #[cfg(feature = "_bench_bus")]
        "bus" => run_spmc_handoff::<BusSpmc<u64, CAPACITY>, CAPACITY>(&ctx, cfg),
        other => panic!(
            "unknown BENCH_IMPL={other:?}. Available: 'ours' (always), \
             'bus' (requires --features _bench_bus)."
        ),
    };
    report.print(&ctx.clock);
}
