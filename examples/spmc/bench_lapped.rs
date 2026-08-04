//! Lapped behaviour under a sustained producer. Overwriting SPMC impls only:
//! backpressure impls (bus) never lap by construction, so `run_spmc_lapped`
//! refuses them at runtime.
//!
//! The producer publishes flat out for a fixed wall-clock window. The
//! consumer adds a configurable per-read delay to provoke lapping. For each
//! delay we report the cost of a `try_read` that returned `Value`, the cost
//! of a `try_read` that returned `Lapped`, the fraction of reads that
//! lapped, and the distribution of skipped counts.
//!
//! We do not report a "recovery latency" (Lapped to next Value time): under
//! sustained producer overflow the consumer is permanently behind, so there
//! is no stable recovery time to report. Per-call Value and Lapped costs
//! plus the lap rate carry the same information without the trap.
//!
//! Why time-bounded and not item-bounded: an item-bounded producer can
//! finish all N publishes inside a few tens of ms (it does not wait for the
//! consumer). A heavily delayed consumer then never accumulates enough
//! samples before the producer signals done. Running the producer for a
//! fixed wall window decouples sample count from producer speed.
//!
//! Impl selection: `BENCH_IMPL=<name>`. Only `ours` is currently supported;
//! `bus` is a backpressure impl and would panic.
//!
//! `BENCH_DELAYS=0,500,5000,...` overrides the default sweep (TSC cycles
//! per consumer iteration). `BENCH_RUN_SECS=N` overrides the per-delay run
//! length (default 2 seconds).
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
//! - Pass env vars through sudo with `sudo -E env BENCH_DELAYS=... ...`.
//!
use std::env;

use low_latency_data_structures::bench::harness::TwoCoreCtx;
use low_latency_data_structures::bench::harness::adapters::ours_spmc::OursSpmc;
use low_latency_data_structures::bench::harness::spmc::{SpmcLappedCfg, run_spmc_lapped};

/// Small enough that a sluggish consumer laps within a handful of producer
/// publishes, large enough to keep the seq protocol exercised across genuine
/// laps rather than back-to-back same-slot rewrites.
const CAPACITY: usize = 128;

fn parse_delays(s: &str) -> Vec<u64> {
    s.split(',')
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(|t| {
            t.parse::<u64>()
                .unwrap_or_else(|_| panic!("invalid delay in BENCH_DELAYS: {t:?}"))
        })
        .collect()
}

fn main() {
    let ctx = TwoCoreCtx::discover_and_preflight();
    println!(
        "TSC freq: {} Hz ({:.3} GHz). CAPACITY={CAPACITY}",
        ctx.tsc_hz,
        ctx.tsc_hz as f64 / 1e9,
    );

    let mut cfg = SpmcLappedCfg::default();
    if let Ok(s) = env::var("BENCH_DELAYS") {
        cfg.delays_cycles = parse_delays(&s);
    }
    if let Ok(s) = env::var("BENCH_RUN_SECS") {
        cfg.run_secs = s.parse().unwrap_or(cfg.run_secs);
    }

    let impl_name = env::var("BENCH_IMPL").unwrap_or_else(|_| "ours".to_string());
    let report = match impl_name.as_str() {
        "ours" => run_spmc_lapped::<OursSpmc<u64, CAPACITY>, CAPACITY>(&ctx, cfg),
        other => panic!(
            "unknown or unsupported BENCH_IMPL={other:?} for lapped bench. \
             Only overwriting impls apply here; currently: 'ours'."
        ),
    };
    println!(
        "\n== {} lapped sweep complete ({} rates) ==",
        report.impl_name,
        report.rates.len()
    );
    for w in report.warnings {
        println!("  WARN: {w}");
    }
}
