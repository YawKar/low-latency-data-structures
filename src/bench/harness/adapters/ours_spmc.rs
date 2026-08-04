//! Adapter for our own `spmc` primitive. Always available under
//! `_bench_utils` (no extra dep).

use std::marker::PhantomData;

use crate::bench::harness::spmc::{SpmcBench, SpmcCons, SpmcProd, SpmcReadResult, SpmcSemantics};
use crate::mem::global::GlobalAllocator;
use crate::spmc::{self, Consumer, Options, Producer, ReadResult};

/// Marker type that constructs an SPMC pair backed by this crate's
/// `spmc::new` with a mlocked global allocator.
pub struct OursSpmc<T, const C: usize>(PhantomData<fn() -> T>);

impl<T, const C: usize> SpmcBench<T, C> for OursSpmc<T, C>
where
    T: bytemuck::AnyBitPattern + Send + 'static,
{
    type Prod = Producer<T, C, GlobalAllocator>;
    type Cons = Consumer<T, C, GlobalAllocator>;
    const NAME: &'static str = "ours";
    const SEMANTICS: SpmcSemantics = SpmcSemantics::OverwritingLapNotified;
    fn new(n_consumers: usize) -> (Self::Prod, Vec<Self::Cons>) {
        let (producer, c0) = spmc::new::<T, C, GlobalAllocator>(Options::global_mlocked());
        // Clone c0 n-1 more times; each clone inherits the current cursor
        // (empty here since nothing has been published yet).
        let mut consumers = Vec::with_capacity(n_consumers);
        for _ in 0..n_consumers.saturating_sub(1) {
            consumers.push(c0.clone());
        }
        if n_consumers > 0 {
            consumers.push(c0);
        }
        (producer, consumers)
    }
}

impl<T, const C: usize> SpmcProd<T> for Producer<T, C, GlobalAllocator>
where
    T: bytemuck::AnyBitPattern + Send,
{
    #[inline]
    fn try_publish(&mut self, v: T) -> Result<(), T> {
        Producer::publish(self, v);
        Ok(())
    }
}

impl<T, const C: usize> SpmcCons<T> for Consumer<T, C, GlobalAllocator>
where
    T: bytemuck::AnyBitPattern + Send,
{
    #[inline]
    fn try_read(&mut self) -> SpmcReadResult<T> {
        match Consumer::try_read(self) {
            ReadResult::Value(v) => SpmcReadResult::Value(v),
            ReadResult::Empty => SpmcReadResult::Empty,
            ReadResult::Lapped { skipped } => SpmcReadResult::Lapped { skipped },
        }
    }
}
