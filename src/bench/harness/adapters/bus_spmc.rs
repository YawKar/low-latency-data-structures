//! Adapter for `bus::Bus` used as an SPMC broadcast. Gated behind
//! `_bench_bus`.
//!
//! Ships with a warning: bus is a backpressure broadcast (parking_lot mutex
//! + condvar under the hood). `try_broadcast` returns `Err(v)` when the
//! slowest reader is behind, so it belongs to the [`SpmcSemantics::Backpressure`]
//! family rather than the overwriting family that ours uses.

use std::marker::PhantomData;
use std::sync::mpsc::TryRecvError;

use bus::{Bus, BusReader};

use crate::bench::harness::spmc::{SpmcBench, SpmcCons, SpmcProd, SpmcReadResult, SpmcSemantics};

/// Marker type that constructs a `bus::Bus` publisher plus N readers, with
/// buffer length `C`.
pub struct BusSpmc<T, const C: usize>(PhantomData<fn() -> T>);

impl<T, const C: usize> SpmcBench<T, C> for BusSpmc<T, C>
where
    T: Clone + Sync + Send + 'static,
{
    type Prod = Bus<T>;
    type Cons = BusReader<T>;
    const NAME: &'static str = "bus";
    const WARNINGS: &'static [&'static str] = &[
        "backpressure broadcast: try_publish returns Err when slowest reader lags",
        "parking_lot mutex + condvar under the hood, not lock-free",
    ];
    const SEMANTICS: SpmcSemantics = SpmcSemantics::Backpressure;
    fn new(n_consumers: usize) -> (Self::Prod, Vec<Self::Cons>) {
        let mut bus = Bus::new(C);
        // Readers must be added on the publisher side before consumer
        // threads start pulling; bus has no consumer-side clone.
        let consumers = (0..n_consumers).map(|_| bus.add_rx()).collect();
        (bus, consumers)
    }
}

impl<T: Sync + Send> SpmcProd<T> for Bus<T> {
    #[inline]
    fn try_publish(&mut self, v: T) -> Result<(), T> {
        Bus::try_broadcast(self, v)
    }
}

impl<T: Clone + Sync + Send> SpmcCons<T> for BusReader<T> {
    #[inline]
    fn try_read(&mut self) -> SpmcReadResult<T> {
        match self.try_recv() {
            Ok(v) => SpmcReadResult::Value(v),
            Err(TryRecvError::Empty) => SpmcReadResult::Empty,
            // Producer dropped. Bench threads treat this as end-of-stream by
            // observing repeated Empty and their own done flag, so surface
            // it as Empty here.
            Err(TryRecvError::Disconnected) => SpmcReadResult::Empty,
        }
    }
}
