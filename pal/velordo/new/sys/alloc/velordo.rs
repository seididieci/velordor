//! Allocatore globale Velordo (S1.3): bump-pointer sopra `sbrk`.
//!
//! Mai free (leak intenzionale v1: il bump avanza solo; `dealloc` e' no-op).
//! Allineamento onorato (il break e' page-aligned dal kernel: si allinea in
//! avanti). Thread-safety: bump atomico + sbrk seriale — con thread multipli
//! due alloc concorrenti non si sovrappongono MAI (ognuno riserva via sbrk
//! prima di scrivere), ma lo spazio tra due bump puo' restare inutilizzato
//! (buco benigno, mai unsound). Single-thread e' il caso testato in S1.3.

use crate::sys::pal::sbrk as pal_sbrk;
use crate::alloc::Layout;
use crate::sync::atomic::{AtomicUsize, Ordering};

/// Vecchi break noti (debug/misura, mai usati per decisioni).
static BUMP_HIGH: AtomicUsize = AtomicUsize::new(0);

pub unsafe fn alloc(layout: Layout) -> *mut u8 {
    let size = layout.size();
    if size == 0 {
        return crate::ptr::without_provenance_mut(layout.align());
    }
    let align = layout.align().max(1);
    // Riserva: sbrk(size + align) copre il worst-case di allineamento.
    let need = size.checked_add(align).unwrap_or(usize::MAX);
    if need == usize::MAX {
        return crate::ptr::null_mut();
    }
    let old = pal_sbrk(need);
    if old < 0 {
        return crate::ptr::null_mut();
    }
    let base = old as usize;
    let aligned = base.next_multiple_of(align);
    BUMP_HIGH.store(aligned + size, Ordering::Relaxed);
    crate::ptr::without_provenance_mut(aligned)
}

pub unsafe fn dealloc(_ptr: *mut u8, _layout: Layout) {
    // Bump-only v1: no-op intenzionale (mai riuso, mai double-free).
}

pub unsafe fn realloc(ptr: *mut u8, old_layout: Layout, new_size: usize) -> *mut u8 {
    unsafe { super::realloc_fallback(ptr, old_layout, new_size) }
}

pub unsafe fn alloc_zeroed(layout: Layout) -> *mut u8 {
    let ptr = unsafe { alloc(layout) };
    if !ptr.is_null() {
        unsafe { ptr.write_bytes(0, layout.size()) };
    }
    ptr
}
