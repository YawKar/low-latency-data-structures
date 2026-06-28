use std::marker::PhantomData;
use std::sync::Arc;

use crate::seqlock::lock::SeqLock;

pub struct Writer<T: bytemuck::AnyBitPattern> {
    inner: Arc<SeqLock<T>>,
    // Remove possibility to share ownership
    _not_sync: PhantomData<*const ()>,
}

// SAFETY: SeqLock<T> is Send, we just need to forbid Sync.
unsafe impl<T: bytemuck::AnyBitPattern> Send for Writer<T> {}

impl<T: bytemuck::AnyBitPattern> Writer<T> {
    pub(super) fn new(seqlock: Arc<SeqLock<T>>) -> Self {
        Self {
            inner: seqlock,
            _not_sync: PhantomData,
        }
    }

    #[inline]
    pub fn write(&self, value: T) {
        self.inner.write(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Writer can be sent to another thread.
    static_assertions::assert_impl_all!(Writer<u32>: Send);
    // Though, there may only be single writer at most.
    static_assertions::assert_not_impl_any!(Writer<u32>: Sync, Clone, Copy);
    static_assertions::assert_not_impl_any!(Arc<Writer<u32>>: Send, Sync, Copy);
}
