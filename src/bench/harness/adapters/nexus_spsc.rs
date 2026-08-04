//! Adapter for `nexus_queue::spsc`. Gated behind `_bench_nexus`.

use std::marker::PhantomData;

use nexus_queue::Full;
use nexus_queue::spsc::{Consumer, Producer, ring_buffer};

use crate::bench::harness::spsc::{SpscBench, SpscCons, SpscProd};

/// Marker type that constructs a `nexus_queue::spsc` pair with capacity `C`.
///
/// `nexus_queue::spsc::ring_buffer` rounds capacity up to the next power of
/// two; for pow2 values of `C` (including 1) the effective capacity matches.
pub struct NexusSpsc<T, const C: usize>(PhantomData<fn() -> T>);

impl<T: Send + 'static, const C: usize> SpscBench<T, C> for NexusSpsc<T, C> {
    type Prod = Producer<T>;
    type Cons = Consumer<T>;
    const NAME: &'static str = "nexus";
    const WARNINGS: &'static [&'static str] =
        &["nexus-queue rounds capacity up to next power of two"];
    fn new() -> (Self::Prod, Self::Cons) {
        ring_buffer(C)
    }
}

impl<T: Send> SpscProd<T> for Producer<T> {
    #[inline]
    fn try_push(&mut self, v: T) -> Result<(), T> {
        Producer::push(self, v).map_err(|Full(v)| v)
    }
}

impl<T: Send> SpscCons<T> for Consumer<T> {
    #[inline]
    fn try_pop(&mut self) -> Option<T> {
        Consumer::pop(self)
    }
}
