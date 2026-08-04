//! Adapter for `crossbeam_channel::bounded` used as an SPSC. Gated behind
//! `_bench_cbchan`.
//!
//! Ships with a warning: crossbeam-channel is MPMC internally. It carries
//! extra bookkeeping (per-send/recv ticket ordering, general MPMC waker
//! machinery) that a purpose-built SPSC ring does not. The comparison is
//! interesting as a "community default" baseline, not as a fair peer.

use std::marker::PhantomData;

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};

use crate::bench::harness::spsc::{SpscBench, SpscCons, SpscProd};

/// Marker type that constructs a `crossbeam_channel::bounded` pair used
/// as SPSC, with capacity `C`.
pub struct CbChanSpsc<T, const C: usize>(PhantomData<fn() -> T>);

impl<T: Send + 'static, const C: usize> SpscBench<T, C> for CbChanSpsc<T, C> {
    type Prod = Sender<T>;
    type Cons = Receiver<T>;
    const NAME: &'static str = "crossbeam-channel";
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
        self.try_recv().ok()
    }
}
