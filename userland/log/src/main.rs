//! userlog — Gateway centrale di logging L1 (Fase 57, ADR-0039).
//!
//! Registra `Service::Log` e serve APPEND nel bucket `log` nativo (+ coda
//! RAM quando il volume manca), READ per `(sorgente, giorno, seq)`, SEAL via
//! snapshot e STATS. Il kernel non e' nel percorso; lo storage-TCB non lo
//! chiama mai (anti-ciclo). Early-boot resta su seriale (qui specchiato).

#![no_std]
#![no_main]

extern crate alloc;

mod server;

use libr::println;

libr::entry!(real_main);
fn real_main(_sp: u64) -> ! {
    println!("[userlog] starting, pid={}", libr::getpid());
    server::run()
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    println!("[userlog] panic");
    libr::exit(1)
}
