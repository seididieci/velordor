//! PAL Velordo (S1.3): entry/exit + unsupported.
//!
//! Modello xous/uefi: init no-op (argc/argv gestiti da sys::args, il kernel
//! passa argc=0), abort/exit via syscall raw, `_start` che chiama il wrapper
//! `main` generato dal compilatore (== lang_start -> rt::init -> user main).

#![forbid(unsafe_op_in_unsafe_fn)]

use core::arch::asm;

// Numeri syscall (single source: syscall-numbers del repo; duplicati qui
// perche' la PAL non puo' dipendere dai crate Velordo in questa fase).
// `con` e' una maniglia console, non un fd POSIX (1/2 = coincidenza storica).
const SYS_WRITE: u64 = 2;
const CONSOLE_OUT: u64 = 1;
const SYS_EXIT: u64 = 0;
const SYS_GET_TICKS: u64 = 22;
const SYS_SBRK: u64 = 25;

/// Syscall raw a 4 argomenti (ABI Velordo: rax=num, rdi/rsi/rdx/r10=args).
#[inline(always)]
pub(crate) unsafe fn syscall4(n: u64, a: u64, b: u64, c: u64, d: u64) -> i64 {
    let r: i64;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") n => r,
            in("rdi") a,
            in("rsi") b,
            in("rdx") c,
            in("r10") d,
            out("rcx") _,
            out("r11") _,
            options(nostack, preserves_flags),
        );
    }
    r
}

pub(crate) fn write_console(buf: &[u8]) -> usize {
    if buf.is_empty() {
        return 0;
    }
    let r = unsafe {
        syscall4(SYS_WRITE, CONSOLE_OUT, buf.as_ptr().addr() as u64, buf.len() as u64, 0)
    };
    if r < 0 { 0 } else { r as usize }
}

/// Ticks PIT (100 Hz) dal boot.
pub(crate) fn ticks() -> u64 {
    let r = unsafe { syscall4(SYS_GET_TICKS, 0, 0, 0, 0) };
    if r < 0 { 0 } else { r as u64 }
}

/// Estende l'heap di `inc` byte (solo VA, pagine lazy). Ritorna il vecchio
/// break o valore negativo.
pub(crate) fn sbrk(inc: usize) -> isize {
    unsafe { syscall4(SYS_SBRK, inc as u64, 0, 0, 0) as isize }
}

pub(crate) fn exit_process(code: i32) -> ! {
    unsafe {
        syscall4(SYS_EXIT, code as u64, 0, 0, 0);
    }
    // SYS_EXIT non ritorna mai; se lo facesse, spin (maihalt in userspace).
    loop {
        core::hint::spin_loop()
    }
}

/// Uscita per `sys::exit`.
pub fn exit(code: i32) -> ! {
    exit_process(code)
}

pub unsafe fn init(_argc: isize, _argv: *const *const u8, _sigpipe: u8) {
    // argv vuoto (argc=0 dal kernel): sys::args non ha stato da init.
    // Niente SIGPIPE (niente segnali), niente sanitize fd (niente fd).
}

pub unsafe fn cleanup() {}

pub fn abort_internal() -> ! {
    exit_process(134);
}

pub fn unsupported<T>() -> crate::io::Result<T> {
    Err(unsupported_err())
}

pub fn unsupported_err() -> crate::io::Error {
    crate::io::Error::UNSUPPORTED_PLATFORM
}

#[cfg(not(test))]
mod c_compat {
    /// Entry del programma (S1.3): il kernel salta a USER_CODE (= `_start`,
    /// KEEP nel linker script) con RSP allo stack user. Legge argc da [rsp]
    /// e chiama il wrapper `main` generato dal compilatore (== lang_start:
    /// rt::init -> user main -> cleanup -> exit). argc del kernel e' 0
    /// (argv ignorato: sys::args e' stub vuoto in v1).
    #[unsafe(no_mangle)]
    pub extern "C" fn _start() -> ! {
        let (argc, argv): (isize, *const *const u8);
        unsafe {
            let rsp: *const u64;
            core::arch::asm!("mov {}, rsp", out(reg) rsp, options(nostack, preserves_flags));
            argc = (rsp as *const isize).read() as isize;
            argv = (rsp as *const *const u8).add(1);
        }
        unsafe extern "C" {
            fn main(argc: isize, argv: *const *const u8) -> isize;
        }
        let code = unsafe { main(argc, argv) };
        super::exit_process(code as i32)
    }
}
