//! Argomenti riga di comando su Velordo (S2.0).
//!
//! Il kernel stende argv stile Linux (`layout_argv`: [argc][argv..][NULL]
//! [envp..][NULL]); `_start` legge argc/argv da [rsp] e `pal::init` li
//! registra qui. Ramo semplice con statiche (niente .init_array: binario
//! statico, `_start` gira sempre).

#![allow(dead_code)] // init non usata nei test host

pub use super::common::Args;
use crate::ffi::{CStr, c_char};
use crate::os::velordo::ffi::OsStringExt;
use crate::ptr;
use crate::sync::atomic::{AtomicIsize, AtomicPtr, Ordering};
use crate::vec::Vec;

static ARGC: AtomicIsize = AtomicIsize::new(0);
static ARGV: AtomicPtr<*const u8> = AtomicPtr::new(ptr::null_mut());

/// One-time global initialization (da `pal::init`, una sola volta).
pub unsafe fn init(argc: isize, argv: *const *const u8) {
    // Solo i puntatori forniti dal sistema, mai modificati dopo.
    ARGC.store(argc, Ordering::Relaxed);
    ARGV.store(argv as *mut _, Ordering::Relaxed);
}

/// Returns the command line arguments.
pub fn args() -> Args {
    let argc = ARGC.load(Ordering::Relaxed);
    let argv = ARGV.load(Ordering::Relaxed) as *const *const u8;

    let mut vec = Vec::with_capacity(argc.max(0) as usize);
    for i in 0..argc.max(0) {
        // SAFETY: argv non-NULL se argc > 0 e lungo almeno argc (garantito
        // da `layout_argv`); stop al primo NULL come unix.
        let ptr = unsafe { argv.add(i as usize).read() };
        if ptr.is_null() {
            break;
        }
        let cstr = unsafe { CStr::from_ptr(ptr as *const c_char) };
        vec.push(OsStringExt::from_vec(cstr.to_bytes().to_vec()));
    }
    Args::new(vec)
}
