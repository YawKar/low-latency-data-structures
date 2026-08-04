//! Adapter for `heapless::spsc::Queue`. Gated behind `_bench_heapless`.
//!
//! Two const generics: `C` is the logical capacity the bench harness cares
//! about (usable items in flight), `N` is the raw slot count that heapless
//! requires as a type parameter. heapless reserves one slot to distinguish
//! full from empty, so the caller must pass `N = C + 1`. An inline const
//! assert enforces the relationship at monomorphization time.
//!
//! The queue is heap-allocated once and leaked so the returned
//! `Producer<'static, T>` / `Consumer<'static, T>` are 'static. That leak is
//! per bench run (one queue for the whole process lifetime) which is fine
//! for a benchmark harness.

use std::marker::PhantomData;

use heapless::spsc::{Consumer, Producer, Queue};

use crate::bench::harness::spsc::{SpscBench, SpscCons, SpscProd};

/// Marker type for the heapless SPSC adapter. `C` is the harness's logical
/// capacity, `N` is the raw heapless slot count. Enforced: `N == C + 1`.
pub struct HeaplessSpsc<T, const C: usize, const N: usize>(PhantomData<fn() -> T>);

impl<T: Send + 'static, const C: usize, const N: usize> SpscBench<T, C> for HeaplessSpsc<T, C, N> {
    type Prod = Producer<'static, T>;
    type Cons = Consumer<'static, T>;
    const NAME: &'static str = "heapless";
    const WARNINGS: &'static [&'static str] =
        &["heapless::Queue<T, N> holds N-1 items; adapter's raw N must equal logical C+1"];
    fn new() -> (Self::Prod, Self::Cons) {
        // Enforce N == C + 1 at monomorphization. Cheap to add here since
        // the mismatch would otherwise silently give a queue with the wrong
        // usable capacity, which would corrupt latency numbers under load.
        const {
            assert!(
                N == C + 1,
                "HeaplessSpsc: raw slot count N must equal logical capacity C + 1"
            );
        }
        let q: &'static mut Queue<T, N> = Box::leak(Box::new(Queue::new()));
        q.split()
    }
}

impl<T: Send> SpscProd<T> for Producer<'static, T> {
    #[inline]
    fn try_push(&mut self, v: T) -> Result<(), T> {
        self.enqueue(v)
    }
}

impl<T: Send> SpscCons<T> for Consumer<'static, T> {
    #[inline]
    fn try_pop(&mut self) -> Option<T> {
        self.dequeue()
    }
}
