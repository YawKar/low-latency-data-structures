//! Adapter for `std::sync::mpsc::sync_channel` used as an SPSC. Gated behind
//! `_bench_stdmpsc`. No third-party dep required.
//!
//! Ships with a warning: `sync_channel` in std is an MPSC blocking channel
//! backed by a mutex + condvar (Rust std docs: "mpsc: multiple producer,
//! single consumer FIFO queue communication primitives"). Using it as SPSC
//! is a "stdlib baseline" comparison, not a fair peer.

use std::marker::PhantomData;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};

use crate::bench::harness::spsc::{SpscBench, SpscCons, SpscProd};

/// Marker type that constructs a `std::sync::mpsc::sync_channel` pair used
/// as SPSC, with capacity `C`.
pub struct StdMpscSpsc<T, const C: usize>(PhantomData<fn() -> T>);

impl<T: Send + 'static, const C: usize> SpscBench<T, C> for StdMpscSpsc<T, C> {
    type Prod = SyncSender<T>;
    type Cons = Receiver<T>;
    const NAME: &'static str = "std-mpsc";
    const WARNINGS: &'static [&'static str] =
        &["std::sync::mpsc::sync_channel is mutex + condvar based, not lock-free"];
    fn new() -> (Self::Prod, Self::Cons) {
        sync_channel(C)
    }
}

impl<T: Send> SpscProd<T> for SyncSender<T> {
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
