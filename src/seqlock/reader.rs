use std::sync::Arc;

use crate::seqlock::lock::SeqLock;

/// A reading handle of a [`SeqLock`](crate::seqlock).
///
/// Cheap to clone: multiple readers may share the same lock. [`Reader`] is
/// both [`Send`] and [`Sync`]; concurrent reads do not interfere because the
/// underlying validation protocol is wait-free for readers in the absence of
/// a concurrent writer and lock-free in its presence.
pub struct Reader<T: bytemuck::AnyBitPattern> {
    inner: Arc<SeqLock<T>>,
}

impl<T: bytemuck::AnyBitPattern> std::fmt::Debug for Reader<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reader").finish_non_exhaustive()
    }
}

impl<T: bytemuck::AnyBitPattern> Clone for Reader<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T: bytemuck::AnyBitPattern> Reader<T> {
    pub(super) fn new(seqlock: Arc<SeqLock<T>>) -> Self {
        Self { inner: seqlock }
    }

    /// Reads the latest stable value.
    ///
    /// Spin-loops while the writer is mid-write or while a torn read is
    /// observed; returns once a consistent value has been observed.
    ///
    /// # Examples
    ///
    /// ```
    /// use low_latency_data_structures::seqlock::new;
    ///
    /// let (writer, reader) = new(0u64);
    /// writer.write(42);
    /// assert_eq!(reader.read(), 42);
    /// ```
    #[inline]
    #[must_use]
    pub fn read(&self) -> T {
        self.inner.read()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // There may be multiple readers as they only read and don't mutate anything.
    // In case someone removes unsafe impl on SeqLock
    static_assertions::assert_impl_all!(Reader<u32>: Send, Sync);
    static_assertions::assert_impl_all!(Arc<Reader<u32>>: Send, Clone);
}
