//! Direct one-way handoff latency under coherency contention:
//! writer writes rdtscp timestamp
//! reader reads and records `now - ts`.
//! Relies on invariant_tsc + nonstop_tsc being
//! synchronized across cores on the same socket (preflight checks this).
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use hdrhistogram::Histogram;
use low_latency_data_structures::bench::tsc::rdtscp;
use low_latency_data_structures::bench::{loc, preflight};
use low_latency_data_structures::seqlock::new;

fn preflight(used_cores: &[usize]) {
    let mut r = preflight::PreflightReport::default();
    preflight::release_build(&mut r);
    preflight::cores_online(&mut r, used_cores);
    preflight::cores_isolated(&mut r, used_cores);
    preflight::cores_nohz_full(&mut r, used_cores);
    preflight::cores_performance_governor(&mut r, used_cores);
    preflight::turbo_disabled(&mut r);
    preflight::cores_distinct_physical(&mut r, used_cores);
    preflight::cores_smt_siblings_quiet(&mut r, used_cores);
    preflight::cores_share_l3(&mut r, used_cores);
    preflight::tsc_invariant_and_nonstop(&mut r);
    r.finish();
}

fn main() {
    let cores = core_affinity::get_core_ids().expect("expected to get list of available cores");
    assert!(
        cores.len() >= 2,
        "need at least 2 separate cores for this benchmark"
    );
    let writer_core = cores[0];
    let reader_core = cores[1];
    let used_cpu_ids = [writer_core.id, reader_core.id];
    preflight(&used_cpu_ids);

    unsafe {
        let rc = libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE);
        assert_eq!(rc, 0, "mlockall failed (need CAP_IPC_LOCK or sudo)");
    }

    const N: u64 = 2_000_000;
    const WARMUP: u64 = 1_000_000;
    const INIT_VALUE: u64 = 0;

    let (writer, reader) = new(INIT_VALUE);
    let barrier = Arc::new(Barrier::new(2));
    let done = Arc::new(AtomicBool::new(false));
    let clock = quanta::Clock::new();

    let loc_before = loc::read(&used_cpu_ids);

    let r_thread = {
        let barrier = barrier.clone();
        let done = done.clone();
        thread::spawn(move || -> Histogram<u64> {
            assert!(
                core_affinity::set_for_current(reader_core),
                "failed to set core affinity for reader: desired core: {reader_core:?}"
            );
            let actual = unsafe { libc::sched_getcpu() };
            assert_eq!(
                actual, reader_core.id as i32,
                "reader not pinned where requested"
            );

            let mut hist = Histogram::<u64>::new(3).unwrap();
            let mut seen: u64 = 0;
            barrier.wait();
            loop {
                barrier.wait();
                let ts = reader.read();
                let now = rdtscp();
                if done.load(Ordering::Acquire) {
                    break;
                }
                if seen >= WARMUP {
                    // wrapping_sub guards against rare cross-core TSC skew;
                    // record() will reject zero/wraparound silently via ok().
                    let _ = hist.record(now.wrapping_sub(ts));
                }
                seen += 1;
                barrier.wait();
            }
            hist
        })
    };

    let w_thread = {
        let barrier = barrier.clone();
        let done = done.clone();
        thread::spawn(move || {
            assert!(
                core_affinity::set_for_current(writer_core),
                "failed to set core affinity for writer: desired core: {writer_core:?}"
            );
            let actual = unsafe { libc::sched_getcpu() };
            assert_eq!(
                actual, writer_core.id as i32,
                "writer not pinned where requested"
            );

            barrier.wait();
            for _ in 0..N {
                let ts = rdtscp();
                writer.write(ts);
                barrier.wait();
                barrier.wait();
            }
            done.store(true, Ordering::Release);
            barrier.wait();
        })
    };

    w_thread.join().unwrap();
    let hist = r_thread.join().unwrap();
    let loc_after = loc::read(&used_cpu_ids);

    let report = |label: &str, raw: u64| {
        let ns = clock.delta_as_nanos(0, raw);
        println!("  {label:<6} {raw:>7} cycles ({ns:>5} ns)");
    };
    report("p50", hist.value_at_quantile(0.50));
    report("p90", hist.value_at_quantile(0.90));
    report("p99", hist.value_at_quantile(0.99));
    report("p99.9", hist.value_at_quantile(0.999));
    report("max", hist.max());

    println!();
    println!("  Local timer interrupts during run (per cpu, nohz_full should keep these near 0):");
    for (i, &cpu) in used_cpu_ids.iter().enumerate() {
        match (loc_before[i], loc_after[i]) {
            (Some(b), Some(a)) => println!("    cpu{cpu:>2}: +{}", a.saturating_sub(b)),
            _ => println!("    cpu{cpu:>2}: unreadable"),
        }
    }
}
