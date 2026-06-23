/// - Kernel cmdline: isolcpus=2,3 nohz_full=2,3 rcu_nocbs=2,3
/// - echo performance > /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor
/// - echo 1 > /sys/devices/system/cpu/intel_pstate/no_turbo
/// - Disable SMT on test cores, or pin to physical cores with sibling thread parked
/// - Pick two cores that share L3 but are different physical cores (lscpu -e)
use std::{
    hint::{black_box, spin_loop},
    sync::{Arc, Barrier},
    thread,
};

use hdrhistogram::Histogram;
use low_latency_data_structures::spsc::new;

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
    preflight(&[producer_core.id, consumer_core.id]);

    const CAPACITY: usize = 2;
    const N: u64 = 10_000_000;

    let (ptoc_producer, ptoc_consumer) = new::<u64, CAPACITY>();
    let (ctop_producer, ctop_consumer) = new::<u64, CAPACITY>();
    let barrier = Arc::new(Barrier::new(3));
    let clock = Arc::new(quanta::Clock::new());

    let pthread = {
        let ptoc_producer = ptoc_producer;
        let barrier = barrier.clone();
        let mut hist = Histogram::<u64>::new(3).unwrap();
        let clock = clock.clone();
        thread::spawn(move || {
            if !core_affinity::set_for_current(producer_core) {
                panic!("failed to set core affinity for producer: desired core: {producer_core:?}");
            }
            let actual = unsafe { libc::sched_getcpu() };
            assert_eq!(
                actual, producer_core.id as i32,
                "thread not pinned where requested"
            );

            barrier.wait();
            for i in 0..N {
                let t0 = clock.raw();
                black_box(ptoc_producer.push(black_box(i)));
                while ctop_consumer.pop().is_none() {
                    spin_loop();
                }
                let t1 = clock.raw();
                hist.record(t1 - t0).unwrap();
            }
            hist
        })
    };
    {
        let barrier = barrier.clone();
        thread::spawn(move || {
            if !core_affinity::set_for_current(consumer_core) {
                panic!("failed to set core affinity for consumer: desired core: {consumer_core:?}");
            }
            let actual = unsafe { libc::sched_getcpu() };
            assert_eq!(
                actual, consumer_core.id as i32,
                "thread not pinned where requested"
            );

            barrier.wait();
            loop {
                if let Some(value) = ptoc_consumer.pop() {
                    ctop_producer.push(value);
                }
            }
        })
    };

    barrier.wait();
    let hist = pthread.join().unwrap();
    let report = |label: &str, raw: u64| {
        let ns = clock.delta_as_nanos(0, raw);
        println!("  {label:<6} {raw:>7} cycles ({ns:>5} ns)");
    };

    report("p50", hist.value_at_quantile(0.50));
    report("p90", hist.value_at_quantile(0.90));
    report("p99", hist.value_at_quantile(0.99));
    report("p99.9", hist.value_at_quantile(0.999));
    report("max", hist.max());
}
