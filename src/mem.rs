//! Module for all things memory

use std::mem::MaybeUninit;

use crate::shim::alloc;
use crate::shim::cell::UnsafeCell;

#[allow(dead_code)]
pub enum Allocation<T> {
    HugePages {
        ptr: *mut UnsafeCell<MaybeUninit<T>>,
        length: usize,
    },
    Global {
        ptr: *mut UnsafeCell<MaybeUninit<T>>,
        layout: alloc::Layout,
    },
    #[cfg(feature = "tests_loom")]
    LoomVecBacked {
        ptr: *mut UnsafeCell<MaybeUninit<T>>,
        capacity: usize,
    },
}

impl<T> Allocation<T> {
    #[inline(always)]
    pub fn ptr(&self) -> *mut UnsafeCell<MaybeUninit<T>> {
        match self {
            Self::HugePages { ptr, .. } => *ptr,
            Self::Global { ptr, .. } => *ptr,
            #[cfg(feature = "tests_loom")]
            Self::LoomVecBacked { ptr, .. } => *ptr,
        }
    }
}

impl<T> Drop for Allocation<T> {
    fn drop(&mut self) {
        match self {
            Allocation::Global { ptr, layout } => {
                use crate::shim::alloc;
                unsafe { alloc::dealloc(ptr.cast::<u8>(), *layout) };
            }
            Allocation::HugePages { ptr, length } => {
                unsafe {
                    if libc::munmap(*ptr.cast(), *length) == -1 {
                        // TODO: should we panic here or emit an event?
                    }
                };
            }
            #[cfg(feature = "tests_loom")]
            Allocation::LoomVecBacked { ptr, capacity } => unsafe {
                drop(Vec::from_raw_parts(*ptr, *capacity, *capacity))
            },
        }
    }
}

/// Allocate a typed buffer with `capacity` uninitialized items each with memory layout of `T`.
/// Loom tests version.
#[cfg(feature = "tests_loom")]
pub fn allocate_buffer<T>(capacity: usize, _use_hugepages: bool) -> Allocation<T> {
    type RealT<T> = UnsafeCell<MaybeUninit<T>>;
    let mut buffer: Vec<RealT<T>> = Vec::with_capacity(capacity);
    for _ in 0..capacity {
        buffer.push(UnsafeCell::new(MaybeUninit::uninit()));
    }
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    Allocation::LoomVecBacked { ptr, capacity }
}

/// Allocate a typed buffer with `capacity` uninitialized items each with memory layout of `T`.
#[cfg(not(feature = "tests_loom"))]
pub fn allocate_buffer<T>(capacity: usize, use_hugepages: bool) -> Allocation<T> {
    use crate::shim::alloc;
    let layout_size = alloc::Layout::array::<UnsafeCell<MaybeUninit<T>>>(capacity)
        .unwrap()
        .size();

    if use_hugepages {
        // Using 2 MiB page size
        let page_size = 2 * 1024 * 1024;
        // Round up to page size
        let alloc_size = (layout_size + page_size - 1) & !(page_size - 1);
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                alloc_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_HUGETLB | libc::MAP_HUGE_2MB,
                -1,
                0,
            )
        };
        assert_ne!(ptr, libc::MAP_FAILED, "hugepage mmap failed");
        Allocation::HugePages {
            ptr: ptr as *mut UnsafeCell<MaybeUninit<T>>,
            length: alloc_size,
        }
    } else {
        let layout = alloc::Layout::from_size_align(
            layout_size,
            std::mem::align_of::<UnsafeCell<MaybeUninit<T>>>(),
        )
        .unwrap();
        Allocation::Global {
            ptr: unsafe { alloc::alloc(layout) } as *mut UnsafeCell<MaybeUninit<T>>,
            layout,
        }
    }
}
