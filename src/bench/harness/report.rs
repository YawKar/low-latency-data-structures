//! Standard latency report shared across all cross-crate benchmarks.

/// Latency percentiles from one bench run, plus the LOC delta per pinned
/// CPU and any per-impl warnings that should surface in the printed table.
///
/// Percentile fields are raw TSC cycles; `print` converts them to
/// nanoseconds through the caller-supplied `quanta::Clock`.
pub struct LatencyReport {
    /// Name of the impl (e.g. `"ours"`, `"rtrb"`). Printed as a section
    /// header so multiple impls in one run stay visually separated.
    pub impl_name: &'static str,
    /// Static caveats the adapter attached (e.g. `"MPMC under the hood"`).
    /// Printed under the header so unfair comparisons are loud, not
    /// buried in a README footnote.
    pub warnings: &'static [&'static str],
    /// TSC cycles at the given quantile.
    pub p50: u64,
    /// TSC cycles at the given quantile.
    pub p90: u64,
    /// TSC cycles at the given quantile.
    pub p99: u64,
    /// TSC cycles at the given quantile.
    pub p999: u64,
    /// Max observed TSC cycles for a single sample.
    pub max: u64,
    /// Per-CPU `(cpu_id, delta or None)` of `/proc/interrupts` LOC counters
    /// across the run. `None` means the counter was unreadable.
    pub loc_delta: Vec<(usize, Option<u64>)>,
}

impl LatencyReport {
    /// Pretty-print the report to stdout in the same layout the pre-harness
    /// benches used, extended with a section header and warnings.
    pub fn print(&self, clock: &quanta::Clock) {
        println!("== {} ==", self.impl_name);
        for w in self.warnings {
            println!("  WARN: {w}");
        }
        let row = |label: &str, raw: u64| {
            let ns = clock.delta_as_nanos(0, raw);
            println!("  {label:<6} {raw:>7} cycles ({ns:>5} ns)");
        };
        row("p50", self.p50);
        row("p90", self.p90);
        row("p99", self.p99);
        row("p99.9", self.p999);
        row("max", self.max);
        println!();
        println!(
            "  Local timer interrupts during run (per cpu, nohz_full should keep these near 0):"
        );
        for (cpu, delta) in &self.loc_delta {
            match delta {
                Some(d) => println!("    cpu{cpu:>2}: +{d}"),
                None => println!("    cpu{cpu:>2}: unreadable"),
            }
        }
    }
}
