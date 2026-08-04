//! SPMC bench traits and bench cores.
//!
//! The trait is const-generic over `CAPACITY` so it matches how our own SPMC
//! exposes capacity (compile-time). Adapters over crates whose capacity is
//! runtime forward `CAPACITY` to their runtime constructor.
//!
//! Two semantic families exist behind this trait:
//!
//! - Overwriting broadcast (ours, nexus-queue): `try_publish` always returns
//!   `Ok(())`; slow consumers observe `SpmcReadResult::Lapped { skipped }` on
//!   their next `try_read`.
//! - Backpressure broadcast (bus): `try_publish` returns `Err(v)` when the
//!   slowest consumer is behind; no data loss, but the publisher can be
//!   throttled by the slowest reader.
//!
//! The two families use the same trait signatures. Bench cores that only
//! make sense against one family gate on [`SpmcSemantics`] via
//! [`SpmcBench::SEMANTICS`] and skip the others with a diagnostic.

use std::fmt;

mod capacity_sweep;
mod handoff;
mod lapped;

pub use capacity_sweep::{
    CapacitySweepResult, SpmcCapacitySweepCfg, print_capacity_row, run_spmc_capacity_sweep_one,
};
pub use handoff::{SpmcHandoffCfg, run_spmc_handoff};
pub use lapped::{LappedRateResult, LappedReport, SpmcLappedCfg, run_spmc_lapped};

/// Outcome of a single `try_read` call on an SPMC consumer.
///
/// Backpressure impls never return `Lapped` (no data loss by construction);
/// overwriting impls return it when the producer has lapped this consumer's
/// read cursor since the last read.
#[must_use = "the returned SpmcReadResult indicates whether a value was read, the queue was empty, or the consumer was lapped"]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpmcReadResult<T> {
    /// A value was successfully read.
    Value(T),
    /// The queue is currently empty.
    Empty,
    /// The producer lapped this consumer since the last read. `skipped` is
    /// the number of values that were never observed. Overwriting impls only.
    Lapped {
        /// Number of values skipped over by the lap-recovery jump.
        skipped: usize,
    },
}

/// Producer half of an SPMC broadcast, as seen by the bench harness.
///
/// `try_publish` returns `Ok(())` on success. For overwriting impls the
/// method always succeeds (the queue overwrites its oldest slot). For
/// backpressure impls the method returns `Err(v)` when the slowest consumer
/// is behind; the bench loops the spin externally so the wait cost is
/// visible in the recorded percentile. The unified return type keeps bench
/// cores impl-agnostic.
pub trait SpmcProd<T>: Send {
    /// Attempt to publish `v`. Always `Ok(())` for overwriting impls; may
    /// return `Err(v)` for backpressure impls when the slowest consumer is
    /// behind.
    fn try_publish(&mut self, v: T) -> Result<(), T>;
}

/// Consumer half of an SPMC broadcast, as seen by the bench harness.
pub trait SpmcCons<T>: Send {
    /// Attempt to read the next value.
    fn try_read(&mut self) -> SpmcReadResult<T>;
}

/// Which of the two SPMC semantic families a given impl belongs to.
///
/// Bench cores that only make sense against one family gate on this const
/// and skip incompatible impls with a diagnostic. See the module docs for
/// the family definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpmcSemantics {
    /// Publisher never blocks; slow consumers observe `Lapped { skipped }`
    /// on their next read.
    OverwritingLapNotified,
    /// Publisher's `try_publish` returns `Err(v)` when the slowest consumer
    /// is behind; no data loss.
    Backpressure,
}

impl fmt::Display for SpmcSemantics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OverwritingLapNotified => write!(f, "overwriting (lap-notified)"),
            Self::Backpressure => write!(f, "backpressure"),
        }
    }
}

/// Tag type that constructs an SPMC producer plus `n_consumers` consumer
/// handles, all with the same underlying queue of capacity `CAPACITY`.
///
/// Implementors are zero-sized markers that route generic instantiations
/// through [`new`](Self::new). The constructor takes `n_consumers` because
/// impls differ in how consumers are minted (clone vs `add_rx` on the
/// publisher); hiding that behind the trait keeps bench cores uniform.
pub trait SpmcBench<T, const CAPACITY: usize>: Sized {
    /// Concrete producer handle. Owned by the producer thread.
    type Prod: SpmcProd<T> + 'static;
    /// Concrete consumer handle. Owned by a consumer thread.
    type Cons: SpmcCons<T> + 'static;
    /// Short label printed in the report header (e.g. `"ours"`, `"nexus"`,
    /// `"bus"`).
    const NAME: &'static str;
    /// Static caveats to surface at the top of the report (e.g. capacity
    /// rounding, MPMC-under-the-hood).
    const WARNINGS: &'static [&'static str] = &[];
    /// Which semantic family this impl belongs to.
    const SEMANTICS: SpmcSemantics;
    /// Construct a producer and `n_consumers` consumer handles.
    fn new(n_consumers: usize) -> (Self::Prod, Vec<Self::Cons>);
}
