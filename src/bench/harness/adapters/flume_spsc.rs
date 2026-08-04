//! Adapter for `flume::bounded` used as an SPSC. Gated behind `_bench_flume`.
//!
//! Ships with a warning: flume is MPMC internally, so this is an
//! MPMC-as-SPSC baseline, not a fair peer to a purpose-built SPSC ring.

use std::marker::PhantomData;

use flume::{Receiver, Sender, TryRecvError, TrySendError, bounded};

use crate::bench::harness::spsc::{SpscBench, SpscCons, SpscProd};

/// Marker type that constructs a `flume::bounded` pair used as SPSC.
pub struct FlumeSpsc<T, const C: usize>(PhantomData<fn() -> T>);

impl<T: Send + 'static, const C: usize> SpscBench<T, C> for FlumeSpsc<T, C> {
    type Prod = Sender<T>;
    type Cons = Receiver<T>;
    const NAME: &'static str = "flume";
    const WARNINGS: &'static [&'static str] =
        &["MPMC channel used as SPSC: extra bookkeeping vs a purpose-built SPSC ring"];
    fn new() -> (Self::Prod, Self::Cons) {
        bounded(C)
    }
}

impl<T: Send> SpscProd<T> for Sender<T> {
    #[inline]
    fn try_push(&mut self, v: T) -> Result<(), T> {
        match self.try_send(v) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(v)) => Err(v),
            Err(TrySendError::Disconnected(v)) => Err(v),
        }
    }
}

impl<T: Send> SpscCons<T> for Receiver<T> {
    #[inline]
    fn try_pop(&mut self) -> Option<T> {
        match self.try_recv() {
            Ok(v) => Some(v),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }
}
