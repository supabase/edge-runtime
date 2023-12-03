use deno_core::v8;
use deno_core::v8::UniqueRef;
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub struct CustomAllocator {
    max: usize,
    count: AtomicUsize,
}

#[allow(clippy::unnecessary_cast)]
unsafe extern "C" fn allocate(allocator: &CustomAllocator, n: usize) -> *mut c_void {
    allocator.count.fetch_add(n, Ordering::SeqCst);
    let count_loaded = allocator.count.load(Ordering::SeqCst);
    if count_loaded > allocator.max {
        return std::ptr::null::<*mut [u8]>() as *mut c_void;
    }

    Box::into_raw(vec![0u8; n].into_boxed_slice()) as *mut [u8] as *mut c_void
}

#[allow(clippy::unnecessary_cast)]
#[allow(clippy::uninit_vec)]
unsafe extern "C" fn allocate_uninitialized(allocator: &CustomAllocator, n: usize) -> *mut c_void {
    allocator.count.fetch_add(n, Ordering::SeqCst);
    let count_loaded = allocator.count.load(Ordering::SeqCst);
    if count_loaded > allocator.max {
        return std::ptr::null::<*mut [u8]>() as *mut c_void;
    }

    let mut store = Vec::with_capacity(n);
    store.set_len(n);
    Box::into_raw(store.into_boxed_slice()) as *mut [u8] as *mut c_void
}

unsafe extern "C" fn free(allocator: &CustomAllocator, data: *mut c_void, n: usize) {
    allocator.count.fetch_sub(n, Ordering::SeqCst);
    let _ = Box::from_raw(std::slice::from_raw_parts_mut(data as *mut u8, n));
}

#[allow(clippy::unnecessary_cast)]
unsafe extern "C" fn reallocate(
    allocator: &CustomAllocator,
    prev: *mut c_void,
    oldlen: usize,
    newlen: usize,
) -> *mut c_void {
    allocator
        .count
        .fetch_add(newlen.wrapping_sub(oldlen), Ordering::SeqCst);
    let count_loaded = allocator.count.load(Ordering::SeqCst);
    if count_loaded > allocator.max {
        return std::ptr::null::<*mut [u8]>() as *mut c_void;
    }

    let old_store = Box::from_raw(std::slice::from_raw_parts_mut(prev as *mut u8, oldlen));
    let mut new_store = Vec::with_capacity(newlen);
    let copy_len = oldlen.min(newlen);
    new_store.extend_from_slice(&old_store[..copy_len]);
    new_store.resize(newlen, 0u8);
    Box::into_raw(new_store.into_boxed_slice()) as *mut [u8] as *mut c_void
}

unsafe extern "C" fn drop(allocator: *const CustomAllocator) {
    Arc::from_raw(allocator);
}

pub fn custom_allocator(max: usize) -> UniqueRef<deno_core::v8::Allocator> {
    let vtable: &'static v8::RustAllocatorVtable<CustomAllocator> = &v8::RustAllocatorVtable {
        allocate,
        allocate_uninitialized,
        free,
        reallocate,
        drop,
    };
    let allocator = Arc::new(CustomAllocator {
        count: AtomicUsize::new(0),
        max,
    });
    unsafe { v8::new_rust_allocator(Arc::into_raw(allocator), vtable) }
}
