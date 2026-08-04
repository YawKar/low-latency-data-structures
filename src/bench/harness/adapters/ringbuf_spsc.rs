//! Adapter for the `ringbuf` crate's `HeapRb`. Gated behind `_bench_ringbuf`.

use std::marker::PhantomData;
use std::sync::Arc;

use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::wrap::caching::{CachingCons, CachingProd};

use crate::bench::harness::spsc::{SpscBench, SpscCons, SpscProd};

/// Marker type that constructs a `ringbuf::HeapRb` SPSC pair with capacity `C`.
///
/// `ringbuf` takes capacity at runtime; the const generic just gets forwarded
/// to `HeapRb::new` so the same bench-core call site works uniformly across
/// compile-time-capacity (ours, heapless) and runtime-capacity (rtrb, ringbuf,
/// ...) adapters.
pub struct RingbufSpsc<T, const C: usize>(PhantomData<fn() -> T>);

impl<T: Send + 'static, const C: usize> SpscBench<T, C> for RingbufSpsc<T, C> {
    type Prod = CachingProd<Arc<HeapRb<T>>>;
    type Cons = CachingCons<Arc<HeapRb<T>>>;
    const NAME: &'static str = "ringbuf";
    fn new() -> (Self::Prod, Self::Cons) {
        HeapRb::<T>::new(C).split()
    }
}

impl<T: Send> SpscProd<T> for CachingProd<Arc<HeapRb<T>>> {
    #[inline]
    fn try_push(&mut self, v: T) -> Result<(), T> {
        Producer::try_push(self, v)
    }
}

impl<T: Send> SpscCons<T> for CachingCons<Arc<HeapRb<T>>> {
    #[inline]
    fn try_pop(&mut self) -> Option<T> {
        Consumer::try_pop(self)
    }
}
