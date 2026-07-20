//! Direct one-way handoff latency under coherency contention: producer pushes rdtscp timestamp, consumer
//! pops and records `now - ts`. Relies on invariant_tsc + nonstop_tsc being
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
use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use hdrhistogram::Histogram;
use low_latency_data_structures::bench::tsc::rdtscp;
use low_latency_data_structures::bench::{loc, preflight};
use low_latency_data_structures::mem::global::GlobalAllocator;
use low_latency_data_structures::spsc::{Options, new};

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
    let producer_core = cores[0];
    let consumer_core = cores[1];
    let used_cpu_ids = [producer_core.id, consumer_core.id];
    preflight(&used_cpu_ids);

    unsafe {
        let rc = libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE);
        assert_eq!(rc, 0, "mlockall failed (need CAP_IPC_LOCK or sudo)");
    }

    // Capacity 1 keeps the queue at depth 0 or 1, so each measurement reflects
    // pure handoff (push -> pop), not in-queue residence time.
    const CAPACITY: usize = 1;
    const N: u64 = 10_000_000;
    const WARMUP: u64 = 1_000_000;

    let (producer, consumer) = new::<u64, CAPACITY, GlobalAllocator>(Options::global_mlocked());
    let barrier = Arc::new(Barrier::new(3));
    let done = Arc::new(AtomicBool::new(false));
    let clock = quanta::Clock::new();

    let loc_before = loc::read(&used_cpu_ids);

    let cthread = {
        let barrier = barrier.clone();
        let done = done.clone();
        thread::spawn(move || -> Histogram<u64> {
            assert!(
                core_affinity::set_for_current(consumer_core),
                "failed to set core affinity for consumer: desired core: {consumer_core:?}"
            );
            let actual = unsafe { libc::sched_getcpu() };
            assert_eq!(
                actual, consumer_core.id as i32,
                "consumer not pinned where requested"
            );

            let mut hist = Histogram::<u64>::new(3).unwrap();
            let mut seen: u64 = 0;
            barrier.wait();
            loop {
                while let Some(ts) = consumer.pop() {
                    let now = rdtscp();
                    if seen >= WARMUP {
                        // wrapping_sub guards against rare cross-core TSC skew;
                        // record() will reject zero/wraparound silently via ok().
                        let _ = hist.record(now.wrapping_sub(ts));
                    }
                    seen += 1;
                }
                if done.load(Ordering::Acquire) {
                    // Producer's `done` Release happens-after its last push, so
                    // any items still in the queue are visible now. Drain.
                    while let Some(ts) = consumer.pop() {
                        let now = rdtscp();
                        if seen >= WARMUP {
                            let _ = hist.record(now.wrapping_sub(ts));
                        }
                        seen += 1;
                    }
                    break;
                }
                spin_loop();
            }
            hist
        })
    };

    let pthread = {
        let barrier = barrier.clone();
        let done = done.clone();
        thread::spawn(move || {
            assert!(
                core_affinity::set_for_current(producer_core),
                "failed to set core affinity for producer: desired core: {producer_core:?}"
            );
            let actual = unsafe { libc::sched_getcpu() };
            assert_eq!(
                actual, producer_core.id as i32,
                "producer not pinned where requested"
            );

            barrier.wait();
            for _ in 0..N {
                // Refresh `ts` on every push attempt so the recorded value
                // reflects the moment the slot was actually published, not the
                // moment we first noticed the queue was full.
                loop {
                    let ts = rdtscp();
                    if producer.push(ts).is_none() {
                        break;
                    }
                    spin_loop();
                }
            }
            done.store(true, Ordering::Release);
        })
    };

    barrier.wait();
    pthread.join().unwrap();
    let hist = cthread.join().unwrap();
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
