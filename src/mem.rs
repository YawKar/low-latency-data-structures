//! Module for all things memory

use std::mem::MaybeUninit;

use crate::shim::alloc;
use crate::shim::cell::UnsafeCell;

pub trait Allocation<T> {
    fn ptr(&self) -> *mut UnsafeCell<MaybeUninit<T>>;
}

struct HugePageAllocation<T> {
    ptr: *mut UnsafeCell<MaybeUninit<T>>,
    length: usize,
}

unsafe impl<T: Send> Send for HugePageAllocation<T> {}

impl<T> Drop for HugePageAllocation<T> {
    fn drop(&mut self) {
        unsafe {
            assert_ne!(
                libc::munmap(self.ptr.cast(), self.length),
                -1,
                "failed to munmap hugepage allocation"
            );
        };
    }
}

impl<T> Allocation<T> for HugePageAllocation<T> {
    #[inline]
    fn ptr(&self) -> *mut UnsafeCell<MaybeUninit<T>> {
        self.ptr
    }
}

#[cfg(not(feature = "tests_loom"))]
struct GlobalAllocation<T> {
    ptr: *mut UnsafeCell<MaybeUninit<T>>,
    layout: alloc::Layout,
}

#[cfg(not(feature = "tests_loom"))]
unsafe impl<T: Send> Send for GlobalAllocation<T> {}

#[cfg(not(feature = "tests_loom"))]
impl<T> Drop for GlobalAllocation<T> {
    fn drop(&mut self) {
        unsafe { alloc::dealloc(self.ptr.cast(), self.layout) };
    }
}

#[cfg(not(feature = "tests_loom"))]
impl<T> Allocation<T> for GlobalAllocation<T> {
    #[inline]
    fn ptr(&self) -> *mut UnsafeCell<MaybeUninit<T>> {
        self.ptr
    }
}

#[cfg(feature = "tests_loom")]
struct LoomVecAllocation<T> {
    ptr: *mut UnsafeCell<MaybeUninit<T>>,
    capacity: usize,
}

#[cfg(feature = "tests_loom")]
unsafe impl<T: Send> Send for LoomVecAllocation<T> {}

#[cfg(feature = "tests_loom")]
impl<T> Drop for LoomVecAllocation<T> {
    fn drop(&mut self) {
        unsafe { drop(Vec::from_raw_parts(self.ptr, self.capacity, self.capacity)) };
    }
}

#[cfg(feature = "tests_loom")]
impl<T> Allocation<T> for LoomVecAllocation<T> {
    #[inline]
    fn ptr(&self) -> *mut UnsafeCell<MaybeUninit<T>> {
        self.ptr
    }
}

/// Allocate a typed buffer with `capacity` uninitialized items each with memory layout of `T`.
/// Loom tests version.
#[cfg(feature = "tests_loom")]
pub fn allocate_buffer<T>(capacity: usize) -> impl Allocation<T> {
    type RealT<T> = UnsafeCell<MaybeUninit<T>>;
    let mut buffer: Vec<RealT<T>> = Vec::with_capacity(capacity);
    for _ in 0..capacity {
        buffer.push(UnsafeCell::new(MaybeUninit::uninit()));
    }
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    LoomVecAllocation { ptr, capacity }
}

/// Allocate a typed buffer with `capacity` uninitialized items each with memory layout of `T`.
#[cfg(not(feature = "tests_loom"))]
pub fn allocate_buffer<T>(capacity: usize) -> impl Allocation<T> {
    let layout = alloc::Layout::array::<UnsafeCell<MaybeUninit<T>>>(capacity).unwrap();
    let ptr = unsafe { alloc::alloc(layout) };
    if ptr.is_null() {
        alloc::handle_alloc_error(layout);
    }
    unsafe {
        assert_ne!(
            libc::mlock(ptr.cast(), layout.size()),
            -1,
            "mlock global allocation failed"
        )
    };
    GlobalAllocation {
        ptr: ptr.cast(),
        layout,
    }
}

/// Allocate a typed buffer with `capacity` uninitialized items each with memory layout of `T`
/// backed by hugepages.
pub fn allocate_hugepage_buffer<T>(capacity: usize) -> impl Allocation<T> {
    let layout_size = alloc::Layout::array::<UnsafeCell<MaybeUninit<T>>>(capacity)
        .unwrap()
        .size();
    // Using 2 MiB page size
    // TODO: what if host system has a different size?
    let page_size = 2 * 1024 * 1024;
    // Round up to page size
    let alloc_size = (layout_size + page_size - 1) & !(page_size - 1);
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            alloc_size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_POPULATE
                | libc::MAP_PRIVATE
                | libc::MAP_ANONYMOUS
                | libc::MAP_HUGETLB
                | libc::MAP_HUGE_2MB,
            -1,
            0,
        )
    };
    assert_ne!(ptr, libc::MAP_FAILED, "hugepage mmap failed");
    unsafe {
        assert_ne!(
            libc::mlock(ptr.cast(), alloc_size),
            -1,
            "mlock hugepage allocation failed"
        )
    };
    HugePageAllocation {
        ptr: ptr as *mut UnsafeCell<MaybeUninit<T>>,
        length: alloc_size,
    }
}
