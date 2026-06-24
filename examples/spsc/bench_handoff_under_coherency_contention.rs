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
use low_latency_data_structures::spsc::new;

/// Inline rdtscp. Returns the TSC value with partial serialization (prevents
/// the CPU from speculatively moving the read across surrounding ops).
#[inline(always)]
fn tsc() -> u64 {
    let mut aux = 0u32;
    unsafe { core::arch::x86_64::__rdtscp(&mut aux) }
}

/// Read LOC (local timer interrupt) counter from /proc/interrupts for each
/// requested cpu. Returns `None` for any cpu not found (e.g. offlined).
///
/// /proc/interrupts columns are positional and only include online CPUs, so we
/// must parse the header to map CPU ids to column indices.
fn read_loc_counters(cpus: &[usize]) -> Vec<Option<u64>> {
    let contents = match std::fs::read_to_string("/proc/interrupts") {
        Ok(s) => s,
        Err(_) => return vec![None; cpus.len()],
    };
    let mut lines = contents.lines();
    let header = match lines.next() {
        Some(h) => h,
        None => return vec![None; cpus.len()],
    };
    let columns: Vec<usize> = header
        .split_whitespace()
        .filter_map(|tok| tok.strip_prefix("CPU").and_then(|n| n.parse().ok()))
        .collect();

    let loc_line = lines.find(|l| l.trim_start().starts_with("LOC:"));
    let counts: Vec<u64> = match loc_line {
        Some(l) => l
            .trim_start()
            .trim_start_matches("LOC:")
            .split_whitespace()
            .filter_map(|t| t.parse().ok())
            .collect(),
        None => return vec![None; cpus.len()],
    };

    cpus.iter()
        .map(|&cpu| {
            let col = columns.iter().position(|&c| c == cpu)?;
            counts.get(col).copied()
        })
        .collect()
}

fn preflight(used_cores: &[usize]) {
    let mut failures = vec![];
    let mut warnings = vec![];

    fn read(path: &str) -> Option<String> {
        std::fs::read_to_string(path)
            .ok()
            .map(|s| s.trim().to_string())
    }
    /// "2-3,5,7-9" -> [2,3,5,7,8,9]
    fn parse_cpu_list(s: &str) -> Vec<usize> {
        let mut out = vec![];
        for part in s.split(',').filter(|p| !p.is_empty()) {
            if let Some((a, b)) = part.split_once('-') {
                let (a, b): (usize, usize) = (a.parse().unwrap(), b.parse().unwrap());
                out.extend(a..=b);
            } else if let Ok(n) = part.parse::<usize>() {
                out.push(n);
            }
        }
        out
    }
    if cfg!(debug_assertions) {
        failures.push("debug build, build with `--release` instead".to_string());
    }

    let online = read("/sys/devices/system/cpu/online")
        .map(|s| parse_cpu_list(&s))
        .unwrap_or_default();
    for &c in used_cores {
        if !online.contains(&c) {
            failures.push(format!("core {c} is offline"));
        }
    }

    let isolated = read("/sys/devices/system/cpu/isolated")
        .map(|s| parse_cpu_list(&s))
        .unwrap_or_default();
    for &c in used_cores {
        if !isolated.contains(&c) {
            warnings.push(format!("core {c} not in isolcpus (got {isolated:?})"));
        }
    }

    let nohz = read("/sys/devices/system/cpu/nohz_full")
        .map(|s| parse_cpu_list(&s))
        .unwrap_or_default();
    for &c in used_cores {
        if !nohz.contains(&c) {
            warnings.push(format!("core {c} not in nohz_full"));
        }
    }

    for &c in used_cores {
        let g = read(&format!(
            "/sys/devices/system/cpu/cpu{c}/cpufreq/scaling_governor"
        ));
        match g.as_deref() {
            Some("performance") => {}
            Some(other) => warnings.push(format!("core {c} governor = {other}, want performance")),
            None => warnings.push(format!("core {c} governor unreadable")),
        }
    }

    match read("/sys/devices/system/cpu/intel_pstate/no_turbo") {
        Some(s) if s == "1" => {}
        Some(other) => warnings.push(format!("intel_pstate/no_turbo = {other}, want 1")),
        None => match read("/sys/devices/system/cpu/cpufreq/boost") {
            Some(s) if s == "0" => {}
            Some(other) => warnings.push(format!("cpufreq/boost = {other}, want 0")),
            None => {
                warnings.push("turbo state unreadable (neither Intel pstate nor AMD boost)".into())
            }
        },
    }

    if used_cores.len() >= 2 {
        let core_id = |c: usize| read(&format!("/sys/devices/system/cpu/cpu{c}/topology/core_id"));
        let a = core_id(used_cores[0]);
        let b = core_id(used_cores[1]);
        if a.is_some() && a == b {
            failures.push(format!(
                "used_cores {} and {} are SMT siblings of physical core {}",
                used_cores[0],
                used_cores[1],
                a.unwrap()
            ));
        }
    }

    fn siblings_of(c: usize) -> Vec<usize> {
        read(&format!(
            "/sys/devices/system/cpu/cpu{c}/topology/thread_siblings_list"
        ))
        .map(|s| parse_cpu_list(&s))
        .unwrap_or_default()
        .into_iter()
        .filter(|&s| s != c)
        .collect()
    }
    for &c in used_cores {
        for sib in siblings_of(c) {
            let sib_online =
                read(&format!("/sys/devices/system/cpu/cpu{sib}/online")).as_deref() != Some("0");
            let sib_isolated = isolated.contains(&sib);
            if sib_online && !sib_isolated {
                warnings.push(format!(
                  "SMT sibling of {c} is cpu {sib}, online and not isolated will steal execution units"
              ));
            }
        }
    }

    if used_cores.len() >= 2 {
        let l3 = |c: usize| {
            read(&format!(
                "/sys/devices/system/cpu/cpu{c}/cache/index3/shared_cpu_list"
            ))
        };
        if let (Some(a), Some(b)) = (l3(used_cores[0]), l3(used_cores[1])) {
            if a != b {
                warnings.push(format!(
                    "used_cores {} and {} don't share L3 ({a} vs {b})",
                    used_cores[0], used_cores[1]
                ));
            }
        }
    }

    // TSC quality
    if let Some(cpuinfo) = read("/proc/cpuinfo") {
        for flag in ["constant_tsc", "nonstop_tsc"] {
            if !cpuinfo.contains(flag) {
                failures.push(format!("CPU missing `{flag}`; TSC unsuitable"));
            }
        }
    }

    for warn in warnings {
        eprintln!("WARNING: {warn}");
    }
    if !failures.is_empty() {
        for fail in failures {
            eprintln!("FAILURE: {fail}");
        }
        panic!("preflight failed: results would be meaningless");
    }
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

    let (producer, consumer) = new::<u64, CAPACITY>();
    let barrier = Arc::new(Barrier::new(3));
    let done = Arc::new(AtomicBool::new(false));
    let clock = quanta::Clock::new();

    let loc_before = read_loc_counters(&used_cpu_ids);

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
                    let now = tsc();
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
                        let now = tsc();
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
                    let ts = tsc();
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
    let loc_after = read_loc_counters(&used_cpu_ids);

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
