//! userposixtests — suite della **personalita' POSIX** (Fase 58.5, ADR-0041).
//!
//! Casa `flavours/posix/tests`, separata da `testland/usertests` (meccanismo).
//! Copre i test che fissano la traduzione POSIX: fondamenta/errno + gate di
//! registro (t53), fd virtuali + redirect (t54), job control suspend/resume +
//! cancel cooperativo (t55/t56). Spawnata da init in sequenza, PRIMA di
//! `usertests` (t54 deve precedere i drop di diritti di t34).
//!
//! Reporting: riga `[posixtests] PASS N/N` (o FAIL) + righe per singolo test.

#![no_std]
#![no_main]

extern crate alloc;
use alloc::vec::Vec;

use civis::println;

mod helpers;
mod t_fdredir;
mod t_jobctl;
mod t_posix;

libr::entry!(real_main);
fn real_main(_sp: u64) -> ! {
    let my_pid = civis::getpid();
    println!("[posixtests] suite up, pid={}", my_pid);

    let mut total = 0u32;
    let mut ok = 0u32;

    helpers::report(
        &mut total,
        &mut ok,
        "t53 fondamenta posix (lookup/errno/gate)",
        t_posix::t_posix_foundation(),
    );
    helpers::report(
        &mut total,
        &mut ok,
        "t54 fd virtuali + redirect (trunc/append/lseek/dup/stdio)",
        t_fdredir::t_fd_virtual_redirect(),
    );
    helpers::report(
        &mut total,
        &mut ok,
        "t55 suspend/resume + TIME congelato + gate",
        t_jobctl::t_suspend_resume(),
    );
    helpers::report(
        &mut total,
        &mut ok,
        "t56 cancel cooperativo + escalation 130",
        t_jobctl::t_sigcatch_cancel(),
    );

    println!("[posixtests] SUMMARY {}/{} PASS", ok, total);
    let _ = civis::send(civis::CHANNEL_PARENT, civis::TEST_DONE, ok as u64, 0);
    if ok == total {
        println!("[posixtests] PASS {}/{}", ok, total);
        civis::exit(0);
    } else {
        println!("[posixtests] FAIL {}/{}", total - ok, total);
        civis::exit(1);
    }
}

#[panic_handler]
fn panic_handler(_info: &core::panic::PanicInfo) -> ! {
    println!("[posixtests] panic");
    civis::exit(1)
}
