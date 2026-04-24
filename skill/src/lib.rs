//! mission-skill — a pre-compiled WASM skill that analyses space mission logs.
//!
//! ABI (C-style, all integers are u32):
//!   alloc(size) -> ptr                   // align = 8
//!   dealloc(ptr, size)
//!   analyze(log_ptr, log_len, filter_ptr, filter_len) -> packed u64
//!
//! `analyze` returns (ptr << 32) | len of a UTF-8 result string. The caller
//! is responsible for freeing the returned buffer via `dealloc`.
//!
//! Return conventions:
//!   packed == 0                    → error (skill panicked or invalid UTF-8)
//!   len == 0 && ptr != 0 → no match (ptr is still valid, len is 0)
//!   otherwise              → "code|id|date|duration|crew"

use std::alloc::{alloc as raw_alloc, dealloc as raw_dealloc, Layout};

const ALIGN: usize = 8;
const ERROR_SENTINEL: u64 = 0;

fn layout(size: u32) -> Option<Layout> {
    if size == 0 {
        return None;
    }
    Layout::from_size_align(size as usize, ALIGN).ok()
}

#[no_mangle]
pub extern "C" fn alloc(size: u32) -> u32 {
    match layout(size) {
        Some(l) => unsafe { raw_alloc(l) as u32 },
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn dealloc(ptr: u32, size: u32) {
    if ptr == 0 { return; }
    if let Some(l) = layout(size) {
        unsafe { raw_dealloc(ptr as *mut u8, l) }
    }
}

#[no_mangle]
pub extern "C" fn analyze(log_ptr: u32, log_len: u32, filter_ptr: u32, filter_len: u32) -> u64 {
    // Safety: caller guarantees these point to valid readable memory of the given length.
    let log_bytes = unsafe { std::slice::from_raw_parts(log_ptr as *const u8, log_len as usize) };
    let filter_bytes = unsafe { std::slice::from_raw_parts(filter_ptr as *const u8, filter_len as usize) };

    let Ok(log) = std::str::from_utf8(log_bytes) else { return ERROR_SENTINEL; };
    let Ok(filter) = std::str::from_utf8(filter_bytes) else { return ERROR_SENTINEL; };

    let mut parts = filter.split('|');
    let Some(destination) = parts.next() else { return ERROR_SENTINEL; };
    let Some(status) = parts.next() else { return ERROR_SENTINEL; };

    let result = mission_core::find_longest(log, destination, status);

    let encoded = match result {
        Some(best) => best.encode(),
        None => String::new(),
    };

    let bytes = encoded.as_bytes();
    let len = bytes.len() as u32;

    if len == 0 {
        // Allocate a 1-byte placeholder so the host always has a valid ptr to dealloc.
        let Some(l) = layout(1) else { return ERROR_SENTINEL };
        let ptr = unsafe { raw_alloc(l) } as u32;
        if ptr == 0 { return ERROR_SENTINEL; }
        return (ptr as u64) << 32; // len = 0, ptr valid
    }

    let Some(l) = layout(len) else { return ERROR_SENTINEL };
    let ptr = unsafe { raw_alloc(l) };
    if ptr.is_null() {
        return ERROR_SENTINEL;
    }
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, len as usize) };

    ((ptr as u64) << 32) | (len as u64)
}
