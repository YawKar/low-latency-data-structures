use std::marker::PhantomData;
use std::sync::Arc;

use crate::seqlock::lock::SeqLock;

/// The writing handle of a [`SeqLock`](crate::seqlock).
///
/// `Writer` is [`Send`] but not [`Sync`]: at most one thread may write at a
/// time. To enforce that, the type is neither [`Clone`] nor [`Copy`].
pub struct Writer<T: bytemuck::AnyBitPattern> {
    inner: Arc<SeqLock<T>>,
    // Remove possibility to share ownership
    _not_sync: PhantomData<*const ()>,
}

impl<T: bytemuck::AnyBitPattern> std::fmt::Debug for Writer<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Writer").finish_non_exhaustive()
    }
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

    /// Writes a new value, replacing the current one.
    ///
    /// Wait-free. Concurrent readers may observe the in-progress write but
    /// will retry until they see a consistent value.
    ///
    /// # Examples
    ///
    /// ```
    /// use low_latency_data_structures::seqlock::new;
    ///
    /// let (writer, reader) = new(0u64);
    /// writer.write(7);
    /// assert_eq!(reader.read(), 7);
    /// ```
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
