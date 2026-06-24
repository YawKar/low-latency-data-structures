//! Composable preflight checks for low-latency benchmarks.
//!
//! Each check is a free function `fn(&mut PreflightReport, ...)` that pushes
//! warnings or failures into the report. Callers build their own preflight
//! routine by calling the subset they need, in any order, then `finish()` to
//! print warnings and panic on any failure.
//!
//! Distinction:
//! - **warning**: result may still be meaningful but is degraded or non-ideal.
//! - **failure**: result would be misleading; the run is aborted.
//!
//! Example:
//! ```ignore
//! use low_latency_data_structures::bench::preflight::*;
//!
//! let mut r = PreflightReport::default();
//! release_build(&mut r);
//! cores_online(&mut r, &[7, 8]);
//! cores_isolated(&mut r, &[7, 8]);
//! tsc_invariant_and_nonstop(&mut r);
//! r.finish();
//! ```

#[derive(Default)]
pub struct PreflightReport {
    pub warnings: Vec<String>,
    pub failures: Vec<String>,
}

impl PreflightReport {
    pub fn warn(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }
    pub fn fail(&mut self, msg: impl Into<String>) {
        self.failures.push(msg.into());
    }
    /// Print all warnings to stderr, panic if any failures were recorded.
    pub fn finish(self) {
        for w in &self.warnings {
            eprintln!("WARNING: {w}");
        }
        if !self.failures.is_empty() {
            for f in &self.failures {
                eprintln!("FAILURE: {f}");
            }
            panic!("preflight failed: results would be meaningless");
        }
    }
}

fn read_trim(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

/// "2-3,5,7-9" -> [2,3,5,7,8,9]
fn parse_cpu_list(s: &str) -> Vec<usize> {
    let mut out = vec![];
    for part in s.split(',').filter(|p| !p.is_empty()) {
        if let Some((a, b)) = part.split_once('-') {
            if let (Ok(a), Ok(b)) = (a.parse::<usize>(), b.parse::<usize>()) {
                out.extend(a..=b);
            }
        } else if let Ok(n) = part.parse::<usize>() {
            out.push(n);
        }
    }
    out
}

fn read_cpu_list(path: &str) -> Vec<usize> {
    read_trim(path)
        .map(|s| parse_cpu_list(&s))
        .unwrap_or_default()
}

fn siblings_of(c: usize) -> Vec<usize> {
    read_cpu_list(&format!(
        "/sys/devices/system/cpu/cpu{c}/topology/thread_siblings_list"
    ))
    .into_iter()
    .filter(|&s| s != c)
    .collect()
}

/// Fail if compiled without optimizations. Debug builds give meaningless
/// numbers for anything tighter than disk I/O.
pub fn release_build(r: &mut PreflightReport) {
    if cfg!(debug_assertions) {
        r.fail("debug build, build with `--release` instead");
    }
}

/// Fail if any of the requested cores is currently offline.
pub fn cores_online(r: &mut PreflightReport, cores: &[usize]) {
    let online = read_cpu_list("/sys/devices/system/cpu/online");
    for &c in cores {
        if !online.contains(&c) {
            r.fail(format!("core {c} is offline"));
        }
    }
}

/// Warn if any of the requested cores is not in the kernel `isolcpus` set.
/// Non-isolated cores will receive scheduler load from other tasks.
pub fn cores_isolated(r: &mut PreflightReport, cores: &[usize]) {
    let isolated = read_cpu_list("/sys/devices/system/cpu/isolated");
    for &c in cores {
        if !isolated.contains(&c) {
            r.warn(format!("core {c} not in isolcpus (got {isolated:?})"));
        }
    }
}

/// Warn if any of the requested cores is not in the kernel `nohz_full` set.
/// Without nohz_full each core takes a periodic local timer interrupt that
/// shows up as p99-class jitter.
pub fn cores_nohz_full(r: &mut PreflightReport, cores: &[usize]) {
    let nohz = read_cpu_list("/sys/devices/system/cpu/nohz_full");
    for &c in cores {
        if !nohz.contains(&c) {
            r.warn(format!("core {c} not in nohz_full"));
        }
    }
}

/// Warn if any of the requested cores is not using the `performance` cpufreq
/// governor. Other governors will dynamically rescale frequency and corrupt
/// TSC-derived latencies.
pub fn cores_performance_governor(r: &mut PreflightReport, cores: &[usize]) {
    for &c in cores {
        let g = read_trim(&format!(
            "/sys/devices/system/cpu/cpu{c}/cpufreq/scaling_governor"
        ));
        match g.as_deref() {
            Some("performance") => {}
            Some(other) => r.warn(format!("core {c} governor = {other}, want performance")),
            None => r.warn(format!("core {c} governor unreadable")),
        }
    }
}

/// Warn if Intel turbo boost / AMD core boost is enabled. Turbo introduces
/// per-core frequency variation that breaks any cross-core latency claim.
pub fn turbo_disabled(r: &mut PreflightReport) {
    match read_trim("/sys/devices/system/cpu/intel_pstate/no_turbo") {
        Some(s) if s == "1" => return,
        Some(other) => {
            r.warn(format!("intel_pstate/no_turbo = {other}, want 1"));
            return;
        }
        None => {}
    }
    match read_trim("/sys/devices/system/cpu/cpufreq/boost") {
        Some(s) if s == "0" => {}
        Some(other) => r.warn(format!("cpufreq/boost = {other}, want 0")),
        None => r.warn("turbo state unreadable (neither Intel pstate nor AMD boost)"),
    }
}

/// Fail if two of the requested cores share the same physical core (i.e. are
/// SMT siblings). They would compete for the same execution units and the
/// measurement would no longer reflect cross-core behavior.
pub fn cores_distinct_physical(r: &mut PreflightReport, cores: &[usize]) {
    for (i, &a) in cores.iter().enumerate() {
        for &b in &cores[i + 1..] {
            let id_a = read_trim(&format!("/sys/devices/system/cpu/cpu{a}/topology/core_id"));
            let id_b = read_trim(&format!("/sys/devices/system/cpu/cpu{b}/topology/core_id"));
            if id_a.is_some() && id_a == id_b {
                r.fail(format!(
                    "cores {a} and {b} are SMT siblings of physical core {}",
                    id_a.unwrap_or_default()
                ));
            }
        }
    }
}

/// Warn if any SMT sibling of a requested core is online but not isolated.
/// An active sibling steals shared execution units and front-end bandwidth
/// even if it never directly contends with this thread.
pub fn cores_smt_siblings_quiet(r: &mut PreflightReport, cores: &[usize]) {
    let isolated = read_cpu_list("/sys/devices/system/cpu/isolated");
    for &c in cores {
        for sib in siblings_of(c) {
            let sib_online = read_trim(&format!("/sys/devices/system/cpu/cpu{sib}/online"))
                .as_deref()
                != Some("0");
            let sib_isolated = isolated.contains(&sib);
            if sib_online && !sib_isolated {
                r.warn(format!(
                    "SMT sibling of {c} is cpu {sib}, online and not isolated will steal execution units"
                ));
            }
        }
    }
}

/// Warn if pairs of requested cores don't share L3. Cross-socket benchmarks
/// have entirely different latency characteristics and usually aren't what
/// the author intended on a single-socket bench machine.
pub fn cores_share_l3(r: &mut PreflightReport, cores: &[usize]) {
    let l3 = |c: usize| {
        read_trim(&format!(
            "/sys/devices/system/cpu/cpu{c}/cache/index3/shared_cpu_list"
        ))
    };
    for (i, &a) in cores.iter().enumerate() {
        for &b in &cores[i + 1..] {
            if let (Some(la), Some(lb)) = (l3(a), l3(b))
                && la != lb
            {
                r.warn(format!("cores {a} and {b} don't share L3 ({la} vs {lb})"));
            }
        }
    }
}

/// Fail if fewer than `min` 2 MiB hugepages are free. Drain-style benches
/// allocate up-front and need the kernel pool already populated; falling
/// back to base pages silently would defeat the comparison point.
pub fn hugepages_at_least(r: &mut PreflightReport, min: u64) {
    let Some(meminfo) = std::fs::read_to_string("/proc/meminfo").ok() else {
        r.warn("/proc/meminfo unreadable, cannot verify hugepage availability");
        return;
    };
    let free = meminfo
        .lines()
        .find_map(|l| l.strip_prefix("HugePages_Free:"))
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.parse::<u64>().ok());
    match free {
        Some(n) if n >= min => {}
        Some(n) => r.fail(format!(
            "need >= {min} free 2MiB hugepages, have {n} (try `just enable-hugepages`)"
        )),
        None => r.warn("could not parse HugePages_Free from /proc/meminfo"),
    }
}

/// Fail if the CPU lacks `constant_tsc` and `nonstop_tsc`. Without both, the
/// TSC ticks at variable rate (P-state dependent) or pauses during deep
/// C-states, making any TSC-based latency measurement garbage.
pub fn tsc_invariant_and_nonstop(r: &mut PreflightReport) {
    let Some(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo").ok() else {
        r.warn("/proc/cpuinfo unreadable, cannot verify TSC quality");
        return;
    };
    for flag in ["constant_tsc", "nonstop_tsc"] {
        if !cpuinfo.contains(flag) {
            r.fail(format!("CPU missing `{flag}`; TSC unsuitable"));
        }
    }
}
