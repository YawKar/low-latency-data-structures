//! Harness for cross-crate benchmarks.
//!
//! Every benchmark family gets a trait describing the minimal API each
//! candidate crate must provide, plus a generic bench core parameterized by
//! that trait. Adapters wire third-party crates into those traits. The
//! example binary picks an adapter at runtime via `BENCH_IMPL=<name>` and
//! monomorphizes the generic bench core for the chosen adapter.
//!
//! Adapters for external crates are gated behind per-crate features
//! (`_bench_rtrb`, etc.) so `_bench_utils` alone stays dep-light.

use core_affinity::CoreId;

use crate::bench::preflight;

pub mod adapters;
pub mod report;
pub mod spsc;

pub use report::LatencyReport;

/// Shared cross-core bench context.
///
/// Picks two distinct cores from `core_affinity::get_core_ids()`, runs the
/// standard two-core preflight, and holds a calibrated `quanta::Clock` used
/// to translate TSC deltas into nanoseconds when the report is printed.
pub struct TwoCoreCtx {
    /// Core that runs the producer / writer thread.
    pub producer_core: CoreId,
    /// Core that runs the consumer / reader thread.
    pub consumer_core: CoreId,
    /// TSC-backed clock used by `LatencyReport::print` to render nanoseconds.
    pub clock: quanta::Clock,
}

impl TwoCoreCtx {
    /// Discovers the first two available cores, runs preflight, and calls
    /// `mlockall`. Panics on any failure.
    pub fn discover_and_preflight() -> Self {
        let cores = core_affinity::get_core_ids().expect("core_affinity::get_core_ids failed");
        assert!(
            cores.len() >= 2,
            "need at least 2 separate cores for cross-core benches"
        );
        let producer_core = cores[0];
        let consumer_core = cores[1];
        let used = [producer_core.id, consumer_core.id];
        Self::preflight(&used);
        Self::mlockall();
        Self {
            producer_core,
            consumer_core,
            clock: quanta::Clock::new(),
        }
    }

    /// Two CPU ids used by the bench, in `[producer, consumer]` order. Handy
    /// for LOC snapshots and for the report header.
    pub fn used_cpu_ids(&self) -> [usize; 2] {
        [self.producer_core.id, self.consumer_core.id]
    }

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

    fn mlockall() {
        // SAFETY: FFI call, no aliasing invariants at stake. Failure is fatal
        // for the bench (bench pages could fault mid-run and destroy the
        // percentiles), so we assert.
        unsafe {
            let rc = libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE);
            assert_eq!(rc, 0, "mlockall failed (need CAP_IPC_LOCK or sudo)");
        }
    }
}
