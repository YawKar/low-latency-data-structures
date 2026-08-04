//! SPSC bench traits and bench cores.
//!
//! The trait is const-generic over `CAPACITY` so it matches how our own
//! SPSC exposes capacity (compile-time). Adapters over crates whose
//! capacity is runtime forward `CAPACITY` to their runtime constructor;
//! adapters over crates whose capacity is a type parameter forward it to
//! that parameter.

mod handoff;
mod throttled;

pub use handoff::{SpscHandoffCfg, run_spsc_handoff};
pub use throttled::{
    SpscThrottledCfg, Stamped, ThrottledRateResult, ThrottledReport, run_spsc_throttled,
};

/// Producer half of an SPSC pair, as seen by the bench harness.
///
/// `try_push` is the only method the harness needs. `Ok(())` on success,
/// `Err(v)` if the queue is full (adapter must not block or spin
/// internally; the bench loops the spin externally so the wait cost is
/// visible in the recorded percentile).
pub trait SpscProd<T>: Send {
    /// Attempt to publish `v`. Returns `Err(v)` if the queue is full.
    fn try_push(&mut self, v: T) -> Result<(), T>;
}

/// Consumer half of an SPSC pair, as seen by the bench harness.
///
/// `try_pop` returns `Some(v)` on success, `None` if empty. Same
/// non-blocking contract as [`SpscProd::try_push`].
pub trait SpscCons<T>: Send {
    /// Attempt to consume the next value. Returns `None` if the queue is
    /// empty.
    fn try_pop(&mut self) -> Option<T>;
}

/// Tag type that constructs an SPSC pair with capacity `CAPACITY`.
///
/// Implementors are typically zero-sized markers (e.g. `OursSpsc<T, C>`,
/// `RtrbSpsc<T, C>`) that only exist to route generic instantiations
/// through [`new`](Self::new).
pub trait SpscBench<T, const CAPACITY: usize>: Sized {
    /// Concrete producer handle. Owned by the producer thread.
    type Prod: SpscProd<T> + 'static;
    /// Concrete consumer handle. Owned by the consumer thread.
    type Cons: SpscCons<T> + 'static;
    /// Short label printed in the report header (e.g. `"ours"`, `"rtrb"`).
    const NAME: &'static str;
    /// Static caveats to surface at the top of the report (e.g. "MPMC
    /// under the hood"). Default: no warnings.
    const WARNINGS: &'static [&'static str] = &[];
    /// Construct the SPSC pair.
    fn new() -> (Self::Prod, Self::Cons);
}
