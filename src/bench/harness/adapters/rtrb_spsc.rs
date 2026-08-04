//! Adapter for the `rtrb` SPSC ring. Gated behind `_bench_rtrb`.

use std::marker::PhantomData;

use rtrb::{Consumer, Producer, PushError, RingBuffer};

use crate::bench::harness::spsc::{SpscBench, SpscCons, SpscProd};

/// Marker type that constructs an rtrb SPSC pair with capacity `C`.
///
/// rtrb takes capacity at runtime; the const generic just gets forwarded
/// to `RingBuffer::new` so the same bench-core call site works uniformly
/// across compile-time-capacity (ours, heapless) and runtime-capacity
/// (rtrb, ringbuf, crossbeam, ...) adapters.
pub struct RtrbSpsc<T, const C: usize>(PhantomData<fn() -> T>);

impl<T: Send + 'static, const C: usize> SpscBench<T, C> for RtrbSpsc<T, C> {
    type Prod = Producer<T>;
    type Cons = Consumer<T>;
    const NAME: &'static str = "rtrb";
    fn new() -> (Self::Prod, Self::Cons) {
        RingBuffer::new(C)
    }
}

impl<T: Send> SpscProd<T> for Producer<T> {
    #[inline]
    fn try_push(&mut self, v: T) -> Result<(), T> {
        self.push(v).map_err(|PushError::Full(v)| v)
    }
}

impl<T: Send> SpscCons<T> for Consumer<T> {
    #[inline]
    fn try_pop(&mut self) -> Option<T> {
        self.pop().ok()
    }
}
