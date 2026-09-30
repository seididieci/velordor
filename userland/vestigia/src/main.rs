//! vestigia — Gateway centrale di logging L1 (Fase 57, ADR-0039).
//!
//! Registra `Service::Vestigia` e serve APPEND nel bucket `log` nativo (+ coda
//! RAM quando il volume manca), READ per `(sorgente, giorno, seq)`, SEAL via
//! snapshot e STATS. Il kernel non e' nel percorso; lo storage-TCB non lo
//! chiama mai (anti-ciclo). Early-boot resta su seriale (qui specchiato).

#![no_std]
#![no_main]

extern crate alloc;

mod server;

use civis::println;

civis::entry!(real_main);
fn real_main(_sp: u64) -> ! {
    println!("[vestigia] starting, pid={}", civis::getpid());
    server::run()
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    println!("[vestigia] panic");
    civis::exit(1)
}
