//! Cold-cache drain throughput: pre-fill the queue, evict it from every cache
//! level, then time a back-to-back drain on a single thread. Compares regular
//! vs hugepage-backed allocators across capacities spanning L1 through past
//! LLC and dTLB reach.
//!
//! Empirical finding (Intel Core i7-class, ~2.6 GHz): drain runs at ~6 ns/item
//! across the entire capacity sweep (32 KiB to 64 MiB working set), with no
//! measurable benefit from hugepages even when the slot buffer is many times
//! larger than dTLB reach. That is the result this bench exists to prove --
//! and to let re-prove on other hardware where it may not hold.
//!
//! Why memory doesn't show up:
//! - SPSC pop walks the slot buffer in a perfectly linear pattern. This is
//!   the canonical workload for the L2 stream prefetcher and the speculative
//!   page-table walker; both refill ahead of the consumer faster than it can
//!   pop, so the "cold" prep is undone before the first measurement tick.
//! - With 64-byte (cache-line-sized) items, every pop touches a fresh line --
//!   the prefetcher's absolute minimum head start. It still wins.
//! - 6 ns/item at ~2.6 GHz works out to ~15 cycles per pop, which is what the
//!   pop body costs as straight-line code (load + store + release + branch).
//!   The inner loop, not memory, sets drain throughput.
//!
//! Why single-threaded:
//! - Drain rate is bounded by the consumer's per-item work plus memory-system
//!   stalls on the slot it reads. Adding a producer would just remove the
//!   "cold" property as the producer would already have warmed the lines.
//! - The producer/consumer split is `Send`, so we run both on this thread
//!   sequentially: push CAP items, evict caches, then pop CAP items.
//!
//! Why compare regular vs hugepage anyway:
//! - The comparison is the falsifiable claim. On a CPU with a weaker
//!   prefetcher, or a workload that breaks the linear pattern, the ratio
//!   column would diverge from 1.00x and the allocator choice would start to
//!   matter. Keeping the bench means future-you can re-check that quickly
//!   instead of re-deriving the question from scratch.
//!
//! Median of N trials per cell: each individual trial includes one allocation
//! and one cache flush, both of which can occasionally fault into kernel work.
//! The median absorbs those outliers without smearing the signal.
//!
//! Required environment:
//! - Kernel cmdline: isolcpus=<C> nohz_full=<C> rcu_nocbs=<C>
//!   intel_idle.max_cstate=0 processor.max_cstate=0
//! - `echo performance > /sys/devices/system/cpu/cpu<C>/cpufreq/scaling_governor`
//! - `echo 1 > /sys/devices/system/cpu/intel_pstate/no_turbo`
//! - `just enable-hugepages` (or `sudo sysctl -w vm.nr_hugepages=16`).
//! - Run with `sudo` so mlockall and large `ulimit -l` work.
//!
use std::time::Duration;

use duplicate::duplicate;
use low_latency_data_structures::bench::tsc::{calibrate_hz, rdtscp};
use low_latency_data_structures::bench::{fmt, loc, preflight};
use low_latency_data_structures::spsc::{new, new_hugepage_backed};

/// One cache line per item. See module docs for why this matters.
type Item = [u64; 8];

/// Largest capacity we sweep, in items. 1 MiB items * 64 B == 64 MiB, which
/// requires 32 free 2 MiB hugepages. Preflight enforces this.
const MAX_CAP_ITEMS: usize = 1 << 20;
const HUGEPAGES_NEEDED: u64 =
    ((MAX_CAP_ITEMS * size_of::<Item>()) as u64).div_ceil(2 * 1024 * 1024);

/// Flush buffer sized to comfortably exceed any laptop/desktop L3 (typical
/// 4-32 MiB). 64 MiB walked at 64 B stride evicts the working set without
/// dominating the run time of the trial itself.
const FLUSH_BYTES: usize = 64 * 1024 * 1024;
const CACHE_LINE: usize = 64;

/// Odd so the median is a single observed value rather than an average of
/// two. 7 trials is enough that one bad trial doesn't move the median while
/// keeping wall-time reasonable for the largest capacity.
const TRIALS: usize = 7;

fn preflight(used_cores: &[usize]) {
    let mut r = preflight::PreflightReport::default();
    preflight::release_build(&mut r);
    preflight::cores_online(&mut r, used_cores);
    preflight::cores_isolated(&mut r, used_cores);
    preflight::cores_nohz_full(&mut r, used_cores);
    preflight::cores_performance_governor(&mut r, used_cores);
    preflight::turbo_disabled(&mut r);
    preflight::cores_smt_siblings_quiet(&mut r, used_cores);
    preflight::tsc_invariant_and_nonstop(&mut r);
    preflight::hugepages_at_least(&mut r, HUGEPAGES_NEEDED);
    r.finish();
}

/// Walk a buffer at cache-line stride and write to every line. Writing (not
/// just reading) is required to actually own the lines in M state and evict
/// whatever was there before. `volatile` defeats the optimizer's right to
/// elide the loop as dead-store-only.
#[inline(never)]
fn flush_caches(buf: &mut [u8]) {
    let n = buf.len();
    let mut i = 0;
    while i < n {
        unsafe { std::ptr::write_volatile(buf.as_mut_ptr().add(i), i as u8) };
        i += CACHE_LINE;
    }
}

fn median(mut xs: Vec<u64>) -> u64 {
    xs.sort_unstable();
    xs[xs.len() / 2]
}

/// Body of one drain trial. A macro (rather than a generic fn) because the
/// `Producer`/`Consumer` types returned by `new` / `new_hugepage_backed` are
/// opaque (`impl Allocation<T>`) and can't be named in a signature, so we'd
/// need to either expose the trait publicly or duplicate the body. The body
/// is short and identical across allocators -- macro is the lighter touch.
macro_rules! drain_trial {
    ($cap:expr, $flush:expr, $tsc_hz:expr, $make:expr) => {{
        let cap: usize = $cap;
        let (producer, consumer) = $make;
        for i in 0..cap as u64 {
            // Single-threaded, no concurrent consumer: push only fails if we
            // overfill, which we don't.
            assert!(producer.push([i; 8]).is_none(), "unexpected full at i={i}");
        }
        flush_caches($flush);
        let t0 = rdtscp();
        for _ in 0..cap {
            let v = consumer.pop().expect("queue should be non-empty mid-drain");
            std::hint::black_box(v);
        }
        let t1 = rdtscp();
        let elapsed_ticks = t1 - t0;
        let elapsed_ns = (elapsed_ticks as u128 * 1_000_000_000) / $tsc_hz as u128;
        (elapsed_ns / cap as u128) as u64
    }};
}

fn main() {
    let cores = core_affinity::get_core_ids().expect("expected to get list of available cores");
    assert!(!cores.is_empty(), "need at least 1 core for this benchmark");
    let consumer_core = cores[0];
    let used_cpu_ids = [consumer_core.id];
    preflight(&used_cpu_ids);

    assert!(
        core_affinity::set_for_current(consumer_core),
        "failed to set core affinity: desired core: {consumer_core:?}"
    );
    let actual = unsafe { libc::sched_getcpu() };
    assert_eq!(
        actual, consumer_core.id as i32,
        "not pinned where requested"
    );

    unsafe {
        let rc = libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE);
        assert_eq!(rc, 0, "mlockall failed (need CAP_IPC_LOCK or sudo)");
    }

    let tsc_hz = calibrate_hz(Duration::from_millis(200));
    println!("TSC freq: {} Hz ({:.3} GHz)", tsc_hz, tsc_hz as f64 / 1e9);
    println!(
        "Trials per cell: {TRIALS}, flush buffer: {} MiB",
        FLUSH_BYTES / 1024 / 1024
    );
    println!();
    println!(
        "{:>10} {:>10} {:>14} {:>14} {:>8}",
        "cap", "bytes", "regular", "hugepage", "ratio"
    );

    // The flush buffer must outlive every trial and be writable. Box<[u8]> so
    // it lives on the heap (mlockall pins it), and reuse it across trials so
    // we're not measuring allocation in the trial itself.
    let mut flush: Box<[u8]> = vec![0u8; FLUSH_BYTES].into_boxed_slice();

    let loc_before = loc::read(&used_cpu_ids);

    duplicate! {
        [
            CAP;
            [512];
            [8192];
            [131072];
            [1048576];
        ]
        {
            const CAPACITY: usize = CAP;
            let bytes = CAPACITY * size_of::<Item>();

            let reg_ns: Vec<u64> = (0..TRIALS)
                .map(|_| drain_trial!(CAPACITY, &mut flush, tsc_hz, new::<Item, CAPACITY>()))
                .collect();
            let huge_ns: Vec<u64> = (0..TRIALS)
                .map(|_| drain_trial!(CAPACITY, &mut flush, tsc_hz, new_hugepage_backed::<Item, CAPACITY>()))
                .collect();

            let reg = median(reg_ns);
            let huge = median(huge_ns);
            let ratio = reg as f64 / huge.max(1) as f64;
            let bytes_str = if bytes >= 1024 * 1024 {
                format!("{}MiB", bytes / 1024 / 1024)
            } else if bytes >= 1024 {
                format!("{}KiB", bytes / 1024)
            } else {
                format!("{bytes}B")
            };
            println!(
                "{:>10} {:>10} {:>14} {:>14} {:>7.2}x",
                CAPACITY,
                bytes_str,
                format!("{}/it", fmt::ns(reg)),
                format!("{}/it", fmt::ns(huge)),
                ratio,
            );
        }
    }

    let loc_after = loc::read(&used_cpu_ids);
    println!();
    println!("LOC delta (nohz_full should keep these near 0):");
    for (i, &cpu) in used_cpu_ids.iter().enumerate() {
        match (loc_before[i], loc_after[i]) {
            (Some(b), Some(a)) => println!("  cpu{cpu:>2}: +{}", a.saturating_sub(b)),
            _ => println!("  cpu{cpu:>2}: unreadable"),
        }
    }
}
