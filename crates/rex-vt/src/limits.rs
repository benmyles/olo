//! Resource limits for a single Rex pane. Ghostty maps grid pages separately
//! from its allocator, so both the grid and its ordinary heap have budgets.

use std::{
    alloc::{Layout, alloc, dealloc},
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::{VtError, ffi};

pub const MAX_CELLS: usize = 256 * 1024;
pub const MAX_AXIS: u16 = 4096;
pub const IMAGE_BYTES: usize = 64 << 20;
pub const SNAPSHOT_BYTES: usize = 128 << 20;
pub const HISTORY_BYTES: usize = 128 << 20;
pub const HEAP_BYTES: usize = 256 << 20;

pub fn grid(cols: u16, rows: u16) -> Result<(), VtError> {
    if cols == 0 || rows == 0 {
        Err(VtError::InvalidValue)
    } else if cols > MAX_AXIS || rows > MAX_AXIS || usize::from(cols) * usize::from(rows) > MAX_CELLS {
        Err(VtError::LimitExceeded)
    } else {
        Ok(())
    }
}

pub fn rgba_len(w: u32, h: u32) -> Option<usize> {
    if w == 0 || h == 0 {
        return None;
    }
    (w as usize).checked_mul(h as usize)?.checked_mul(4).filter(|&len| len <= IMAGE_BYTES)
}

// The C allocator ABI passes the original size and alignment back on free.
// Allocations outside the budget fail in Ghostty's usual fallible paths.
unsafe extern "C" fn allocate(ctx: *mut c_void, len: usize, shift: u8, _: usize) -> *mut c_void {
    let Some(align) = 1usize.checked_shl(shift.into()) else { return ptr::null_mut() };
    let Ok(layout) = Layout::from_size_align(len, align) else { return ptr::null_mut() };
    if len == 0 || len > HEAP_BYTES {
        return ptr::null_mut();
    }
    // SAFETY: Budget supplies its boxed counter as ctx until all native owners are freed.
    let used = unsafe { &*ctx.cast::<AtomicUsize>() };
    if used.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(len).filter(|n| *n <= HEAP_BYTES)).is_err() {
        return ptr::null_mut();
    }
    // SAFETY: layout has a nonzero size, valid alignment and fits isize.
    let block = unsafe { alloc(layout) };
    if block.is_null() {
        used.fetch_sub(len, Ordering::Relaxed);
    }
    block.cast()
}

unsafe extern "C" fn release(ctx: *mut c_void, block: *mut c_void, len: usize, shift: u8, _: usize) {
    // SAFETY: allocator.h guarantees precisely the block, size and alignment from allocate.
    unsafe {
        dealloc(block.cast(), Layout::from_size_align_unchecked(len, 1usize << shift));
        (*ctx.cast::<AtomicUsize>()).fetch_sub(len, Ordering::Relaxed);
    }
}

struct Vtable(ffi::GhosttyAllocatorVtable);
// SAFETY: static function pointers and permanently null reserved slots, never mutated.
unsafe impl Sync for Vtable {}
static VTABLE: Vtable = Vtable(ffi::GhosttyAllocatorVtable { alloc: Some(allocate), free: Some(release), resize: None, remap: None, reserved: [ptr::null_mut(); 4] });

pub(crate) struct Budget {
    raw: ffi::GhosttyAllocator,
    used: Box<AtomicUsize>,
}

// SAFETY: ctx points into the owned Box, the vtable is static, and accounting is atomic.
unsafe impl Send for Budget {}
// SAFETY: the native interface is immutable. Only the atomic byte count is shared.
unsafe impl Sync for Budget {}

impl Budget {
    pub fn new() -> Self {
        let used = Box::new(AtomicUsize::new(0));
        let raw = ffi::GhosttyAllocator { ctx: ptr::from_ref(&*used).cast_mut().cast(), vtable: &VTABLE.0 };
        Self { raw, used }
    }

    pub fn raw(&self) -> &ffi::GhosttyAllocator {
        &self.raw
    }
    pub fn used(&self) -> usize {
        self.used.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_allocations_share_a_ceiling_and_release_the_charge() {
        let budget = Budget::new();
        // SAFETY: the context belongs to this live budget. The one successful
        // allocation is freed once with the identical length and alignment.
        unsafe {
            assert!(allocate(budget.raw.ctx, HEAP_BYTES + 1, 0, 0).is_null());
            assert!(allocate(budget.raw.ctx, 12, u8::MAX, 0).is_null());
            let p = allocate(budget.raw.ctx, 32, 6, 0);
            assert!(!p.is_null() && p.addr().is_multiple_of(64));
            assert_eq!(budget.used(), 32);
            assert!(allocate(budget.raw.ctx, HEAP_BYTES, 0, 0).is_null());
            release(budget.raw.ctx, p, 32, 6, 0);
            assert_eq!(budget.used(), 0);
        }
        assert_eq!(rgba_len(u32::MAX, u32::MAX), None);
    }
}
