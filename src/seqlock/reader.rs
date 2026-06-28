use std::sync::Arc;

use crate::seqlock::lock::SeqLock;

pub struct Reader<T: Copy> {
    inner: Arc<SeqLock<T>>,
}

impl<T: Copy> Reader<T> {
    pub(super) fn new(seqlock: Arc<SeqLock<T>>) -> Self {
        Self { inner: seqlock }
    }

    #[inline]
    pub fn read(&self) -> T {
        self.inner.read()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // There may be multiple readers as they only read and don't mutate anything.
    // In case someone remove unsafe impl on SeqLock
    static_assertions::assert_impl_all!(Reader<u32>: Send, Sync);
    static_assertions::assert_impl_all!(Arc<Reader<u32>>: Send, Clone);
}
