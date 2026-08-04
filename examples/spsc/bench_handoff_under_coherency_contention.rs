//! Direct one-way handoff latency under coherency contention: producer pushes rdtscp timestamp, consumer
//! pops and records `now - ts`. Relies on invariant_tsc + nonstop_tsc being
//! synchronized across cores on the same socket (preflight checks this).
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
//! Required environment:
//! - Kernel cmdline:
//!     isolcpus=<P>,<C> nohz_full=<P>,<C> rcu_nocbs=<P>,<C>
//!     intel_idle.max_cstate=0 processor.max_cstate=0
//! - `echo performance > /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor`
//! - `echo 1 > /sys/devices/system/cpu/intel_pstate/no_turbo`
//! - Offline SMT siblings of the two bench cores (or isolate them too).
//! - Pick two cores that share L3 but are different physical cores (lscpu -e).
//! - Run with `ulimit -l unlimited` (or sudo) so mlockall succeeds.
//! - Run on AC power if on a laptop (SMI rate is higher on battery).
//!
//! NixOS specific:
//!   boot.kernelParams = [
//!     "isolcpus=7,8"
//!     "nohz_full=7,8"
//!     "rcu_nocbs=7,8"
//!     "intel_idle.max_cstate=0"
//!     "processor.max_cstate=0"
//!   ];
//!
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
use low_latency_data_structures::bench::harness::spsc::{SpscHandoffCfg, run_spsc_handoff};

fn main() {
    let ctx = TwoCoreCtx::discover_and_preflight();
    let cfg = SpscHandoffCfg::default();

    // Capacity 1 keeps the queue at depth 0 or 1, so each measurement
    // reflects pure handoff (push -> pop), not in-queue residence time.
    const CAPACITY: usize = 1;

    let impl_name = std::env::var("BENCH_IMPL").unwrap_or_else(|_| "ours".to_string());
    let report = match impl_name.as_str() {
        "ours" => run_spsc_handoff::<OursSpsc<u64, CAPACITY>, CAPACITY>(&ctx, cfg),
        #[cfg(feature = "_bench_rtrb")]
        "rtrb" => run_spsc_handoff::<RtrbSpsc<u64, CAPACITY>, CAPACITY>(&ctx, cfg),
        #[cfg(feature = "_bench_ringbuf")]
        "ringbuf" => run_spsc_handoff::<RingbufSpsc<u64, CAPACITY>, CAPACITY>(&ctx, cfg),
        #[cfg(feature = "_bench_heapless")]
        "heapless" => {
            run_spsc_handoff::<HeaplessSpsc<u64, CAPACITY, { CAPACITY + 1 }>, CAPACITY>(&ctx, cfg)
        }
        #[cfg(feature = "_bench_nexus")]
        "nexus" => run_spsc_handoff::<NexusSpsc<u64, CAPACITY>, CAPACITY>(&ctx, cfg),
        #[cfg(feature = "_bench_cbchan")]
        "crossbeam-channel" => run_spsc_handoff::<CbChanSpsc<u64, CAPACITY>, CAPACITY>(&ctx, cfg),
        #[cfg(feature = "_bench_flume")]
        "flume" => run_spsc_handoff::<FlumeSpsc<u64, CAPACITY>, CAPACITY>(&ctx, cfg),
        #[cfg(feature = "_bench_stdmpsc")]
        "std-mpsc" => run_spsc_handoff::<StdMpscSpsc<u64, CAPACITY>, CAPACITY>(&ctx, cfg),
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
    report.print(&ctx.clock);
}
