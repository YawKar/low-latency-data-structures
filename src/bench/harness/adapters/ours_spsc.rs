//! Adapter for our own `spsc` primitive. Always available under
//! `_bench_utils` (no extra dep).

use std::marker::PhantomData;

use crate::bench::harness::spsc::{SpscBench, SpscCons, SpscProd};
use crate::mem::global::GlobalAllocator;
use crate::spsc::{self, Consumer, Options, Producer};

/// Marker type that constructs an SPSC pair backed by this crate's
/// `spsc::new` with a mlocked global allocator.
pub struct OursSpsc<T, const C: usize>(PhantomData<fn() -> T>);

impl<T, const C: usize> SpscBench<T, C> for OursSpsc<T, C>
where
    T: bytemuck::AnyBitPattern + Send + 'static,
{
    type Prod = Producer<T, C, GlobalAllocator>;
    type Cons = Consumer<T, C, GlobalAllocator>;
    const NAME: &'static str = "ours";
    fn new() -> (Self::Prod, Self::Cons) {
        spsc::new::<T, C, GlobalAllocator>(Options::global_mlocked())
    }
}

impl<T, const C: usize> SpscProd<T> for Producer<T, C, GlobalAllocator>
where
    T: bytemuck::AnyBitPattern + Send,
{
    #[inline]
    fn try_push(&mut self, v: T) -> Result<(), T> {
        // Our push returns Some(v) when full, None on success.
        match Producer::push(self, v) {
            None => Ok(()),
            Some(v) => Err(v),
        }
    }
}

impl<T, const C: usize> SpscCons<T> for Consumer<T, C, GlobalAllocator>
where
    T: bytemuck::AnyBitPattern + Send,
{
    #[inline]
    fn try_pop(&mut self) -> Option<T> {
        Consumer::pop(self)
    }
}
