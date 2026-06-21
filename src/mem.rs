//! Module for all things memory

use std::mem::MaybeUninit;

use crate::shim::cell::UnsafeCell;

/// Allocate a typed buffer with `capacity` uninitialized items each with memory layout of `T`.
/// Loom tests version.
#[cfg(feature = "test_loom")]
pub fn allocate_buffer<T>(
    capacity: usize,
    _use_hugepages: bool,
) -> *mut UnsafeCell<MaybeUninit<T>> {
    let mut buffer = Vec::with_capacity(capacity);
    for _ in 0..capacity {
        buffer.push(UnsafeCell::new(MaybeUninit::uninit()));
    }
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    ptr
}

/// Allocate a typed buffer with `capacity` uninitialized items each with memory layout of `T`.
#[cfg(not(feature = "test_loom"))]
pub fn allocate_buffer<T>(capacity: usize, use_hugepages: bool) -> *mut UnsafeCell<MaybeUninit<T>> {
    use crate::shim::alloc;
    let layout_size = alloc::Layout::array::<UnsafeCell<MaybeUninit<T>>>(capacity)
        .unwrap()
        .size();
    let ptr = if use_hugepages {
        let ptr = unsafe {
            // Using 2 MiB page size
            let page_size = 2 * 1024 * 1024;
            // Round up to page size
            let alloc_size = (layout_size + page_size - 1) & !(page_size - 1);
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
        ptr as *mut UnsafeCell<MaybeUninit<T>>
    } else {
        let layout = alloc::Layout::from_size_align(
            layout_size,
            std::mem::align_of::<UnsafeCell<MaybeUninit<T>>>(),
        )
        .unwrap();
        unsafe { alloc::alloc(layout) as *mut UnsafeCell<MaybeUninit<T>> }
    };
    if ptr.is_null() {
        panic!("failed to allocate buffer");
    }
    ptr
}
