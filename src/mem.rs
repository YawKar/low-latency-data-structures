//! Module for all things memory

use std::mem::MaybeUninit;

use crate::shim::alloc;
use crate::shim::cell::UnsafeCell;

/// Backing allocator for the queue slot buffer.
///
/// Implementors are called once at construction, before any producer or
/// consumer touches the returned memory, and are expected to hand back a
/// contiguous, aligned region big enough to hold `items` cells.
pub trait Allocator {
    /// Configuration passed to [`allocate`](Self::allocate). Each allocator
    /// defines its own options type (e.g. mlock toggle, hugepage size).
    type Options;

    /// Allocates space for `items` cells of `T`.
    ///
    /// The returned [`Allocation`] owns the memory and releases it on drop.
    fn allocate<T>(items: usize, options: Self::Options) -> impl Allocation<T>;
}

/// Handle for a slot buffer produced by an [`Allocator`].
///
/// The pointer returned by [`ptr`](Self::ptr) stays valid for the lifetime of
/// the `Allocation`; dropping it releases the underlying memory.
pub trait Allocation<T> {
    /// Base pointer of the allocated region. Points to `items` contiguous,
    /// aligned cells (see [`Allocator::allocate`]).
    fn ptr(&self) -> *mut UnsafeCell<MaybeUninit<T>>;
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;

    /// A stub `Allocation` used only in `static_assertions` trait-bound
    /// checks. Its `ptr()` must never be called.
    pub(crate) struct NeverAlloc;

    impl<T> Allocation<T> for NeverAlloc {
        fn ptr(&self) -> *mut UnsafeCell<MaybeUninit<T>> {
            unreachable!("NeverAlloc is a stub for trait checks only")
        }
    }
}

/// [`Allocator`] backed by Linux hugepages via `mmap(MAP_HUGETLB)`.
pub mod hugepages {
    use super::*;

    /// Linux hugepage size, selected via `MAP_HUGE_*` mmap flags.
    #[derive(Clone, Copy)]
    pub enum HugepageSize {
        /// Allocates using libc mmap with hugepage size of 64KB
        H64KB,
        /// Allocates using libc mmap with hugepage size of 512KB
        H512KB,
        /// Allocates using libc mmap with hugepage size of 1MB
        H1MB,
        /// Allocates using libc mmap with hugepage size of 2MB
        H2MB,
        /// Allocates using libc mmap with hugepage size of 8MB
        H8MB,
        /// Allocates using libc mmap with hugepage size of 16MB
        H16MB,
        /// Allocates using libc mmap with hugepage size of 32MB
        H32MB,
        /// Allocates using libc mmap with hugepage size of 256MB
        H256MB,
        /// Allocates using libc mmap with hugepage size of 512MB
        H512MB,
    }

    impl HugepageSize {
        fn as_usize_bytes(&self) -> usize {
            match self {
                HugepageSize::H64KB => 64 * 1024,
                HugepageSize::H512KB => 512 * 1024,
                HugepageSize::H1MB => 1024 * 1024,
                HugepageSize::H2MB => 2 * 1024 * 1024,
                HugepageSize::H8MB => 8 * 1024 * 1024,
                HugepageSize::H16MB => 16 * 1024 * 1024,
                HugepageSize::H32MB => 32 * 1024 * 1024,
                HugepageSize::H256MB => 256 * 1024 * 1024,
                HugepageSize::H512MB => 512 * 1024 * 1024,
            }
        }

        fn as_mmap_size_flag(&self) -> libc::c_int {
            match self {
                HugepageSize::H64KB => libc::MAP_HUGE_64KB,
                HugepageSize::H512KB => libc::MAP_HUGE_512KB,
                HugepageSize::H1MB => libc::MAP_HUGE_1MB,
                HugepageSize::H2MB => libc::MAP_HUGE_2MB,
                HugepageSize::H8MB => libc::MAP_HUGE_8MB,
                HugepageSize::H16MB => libc::MAP_HUGE_16MB,
                HugepageSize::H32MB => libc::MAP_HUGE_32MB,
                HugepageSize::H256MB => libc::MAP_HUGE_256MB,
                HugepageSize::H512MB => libc::MAP_HUGE_512MB,
            }
        }
    }

    /// Allocates via `mmap(MAP_HUGETLB | MAP_HUGE_<size>)`. Requires the
    /// kernel to have enough free hugepages of the requested size.
    pub struct HugepageAllocator;

    /// Options for [`HugepageAllocator`].
    #[derive(bon::Builder)]
    pub struct HugepageAllocatorOptions {
        /// If true, `mlock` the allocated region so it stays resident.
        mlock: bool,
        /// Hugepage size passed to `mmap` via `MAP_HUGE_*` flags.
        hugepage_size: HugepageSize,
    }

    impl Allocator for HugepageAllocator {
        type Options = HugepageAllocatorOptions;

        #[inline]
        fn allocate<T>(items: usize, options: Self::Options) -> impl Allocation<T> {
            let layout =
                alloc::Layout::array::<UnsafeCell<MaybeUninit<T>>>(items).unwrap_or_else(|_| {
                    panic!(
                        "Layout::array overflow: {items} x {} bytes exceeds isize::MAX",
                        std::mem::size_of::<UnsafeCell<MaybeUninit<T>>>()
                    )
                });
            let page_size = options.hugepage_size.as_usize_bytes();
            // Round up to page size
            let alloc_size = (layout.size() + page_size - 1) & !(page_size - 1);
            let ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    alloc_size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_POPULATE
                        | libc::MAP_PRIVATE
                        | libc::MAP_ANONYMOUS
                        | libc::MAP_HUGETLB
                        | options.hugepage_size.as_mmap_size_flag(),
                    -1,
                    0,
                )
            };
            assert_ne!(
                ptr,
                libc::MAP_FAILED,
                "hugepage mmap failed: {}",
                std::io::Error::last_os_error()
            );
            if options.mlock {
                let rc = unsafe { libc::mlock(ptr.cast(), alloc_size) };
                assert_ne!(
                    rc,
                    -1,
                    "mlock hugepage allocation failed: {}",
                    std::io::Error::last_os_error()
                );
            }
            HugePageAllocation {
                ptr: ptr as *mut UnsafeCell<MaybeUninit<T>>,
                length: alloc_size,
            }
        }
    }

    pub(crate) struct HugePageAllocation<T> {
        ptr: *mut UnsafeCell<MaybeUninit<T>>,
        length: usize,
    }

    unsafe impl<T: Send> Send for HugePageAllocation<T> {}

    impl<T> Drop for HugePageAllocation<T> {
        fn drop(&mut self) {
            let rc = unsafe { libc::munmap(self.ptr.cast(), self.length) };
            assert_ne!(
                rc,
                -1,
                "failed to munmap hugepage allocation: {}",
                std::io::Error::last_os_error()
            );
        }
    }

    impl<T> Allocation<T> for HugePageAllocation<T> {
        #[inline]
        fn ptr(&self) -> *mut UnsafeCell<MaybeUninit<T>> {
            self.ptr
        }
    }
}

/// [`Allocator`] backed by the process's `#[global_allocator]`.
pub mod global {
    use super::*;

    /// Allocates via the process's `#[global_allocator]`. This is the
    /// default choice when hugepages aren't available or wanted.
    pub struct GlobalAllocator;

    /// Options for [`GlobalAllocator`].
    #[derive(bon::Builder)]
    pub struct GlobalAllocatorOptions {
        /// If true, `mlock` the allocated region so it stays resident.
        mlock: bool,
    }

    impl Allocator for GlobalAllocator {
        type Options = GlobalAllocatorOptions;

        #[inline]
        fn allocate<T>(items: usize, options: Self::Options) -> impl Allocation<T> {
            let layout =
                alloc::Layout::array::<UnsafeCell<MaybeUninit<T>>>(items).unwrap_or_else(|_| {
                    panic!(
                        "Layout::array overflow: {items} x {} bytes exceeds isize::MAX",
                        std::mem::size_of::<UnsafeCell<MaybeUninit<T>>>()
                    )
                });
            let ptr = unsafe { alloc::alloc(layout) };
            if ptr.is_null() {
                alloc::handle_alloc_error(layout);
            }
            if options.mlock {
                let rc = unsafe { libc::mlock(ptr.cast(), layout.size()) };
                assert_ne!(
                    rc,
                    -1,
                    "mlock global allocation failed: {}",
                    std::io::Error::last_os_error()
                );
            }
            GlobalAllocation {
                ptr: ptr.cast(),
                layout,
            }
        }
    }

    pub(crate) struct GlobalAllocation<T> {
        ptr: *mut UnsafeCell<MaybeUninit<T>>,
        layout: alloc::Layout,
    }

    unsafe impl<T: Send> Send for GlobalAllocation<T> {}

    impl<T> Drop for GlobalAllocation<T> {
        fn drop(&mut self) {
            unsafe { alloc::dealloc(self.ptr.cast(), self.layout) };
        }
    }

    impl<T> Allocation<T> for GlobalAllocation<T> {
        #[inline]
        fn ptr(&self) -> *mut UnsafeCell<MaybeUninit<T>> {
            self.ptr
        }
    }
}

#[cfg(test)]
#[cfg(feature = "tests_loom")]
pub(crate) mod loom {
    use super::*;

    pub(crate) struct LoomVecAllocator;

    pub(crate) struct LoomVecAllocatorOptions;

    impl Allocator for LoomVecAllocator {
        type Options = LoomVecAllocatorOptions;

        #[inline]
        fn allocate<T>(items: usize, _options: Self::Options) -> impl Allocation<T> {
            type RealT<T> = UnsafeCell<MaybeUninit<T>>;
            let mut buffer: Vec<RealT<T>> = Vec::with_capacity(items);
            for _ in 0..items {
                buffer.push(UnsafeCell::new(MaybeUninit::uninit()));
            }
            let ptr = buffer.as_mut_ptr();
            std::mem::forget(buffer);
            LoomVecAllocation {
                ptr,
                capacity: items,
            }
        }
    }

    pub(crate) struct LoomVecAllocation<T> {
        ptr: *mut UnsafeCell<MaybeUninit<T>>,
        capacity: usize,
    }

    unsafe impl<T: Send> Send for LoomVecAllocation<T> {}

    impl<T> Drop for LoomVecAllocation<T> {
        fn drop(&mut self) {
            unsafe { drop(Vec::from_raw_parts(self.ptr, self.capacity, self.capacity)) };
        }
    }

    impl<T> Allocation<T> for LoomVecAllocation<T> {
        #[inline]
        fn ptr(&self) -> *mut UnsafeCell<MaybeUninit<T>> {
            self.ptr
        }
    }
}

/// Type of backing allocation
pub enum BackingAllocation {
    /// Allocates using whatever `#[global_allocator]` is set to.
    Global,
    /// Allocates via `mmap(MAP_HUGETLB)` at the given hugepage size.
    Hugepages(hugepages::HugepageSize),
}
