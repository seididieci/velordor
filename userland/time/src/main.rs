//! usertime — Fornitore di data/ora in userspace (Fase 50, P1 orologio).
//!
//! Legge il CMOS/RTC (`0x70/0x71`, porte concesse da init) una volta
//! all'avvio e serve `TIME_NOW` (secondi epoch UTC + centesimi) sul servizio
//! `Time`. Il tempo poi avanza sul monotono PIT, mai piu' sull'hardware.
//! Clienti: cardo (mtime), log futuri, qualunque servizio (via `civis::time`).

#![no_std]
#![no_main]

mod cmos;
mod server;

use civis::println;

civis::entry!(real_main);
fn real_main(_sp: u64) -> ! {
    println!("[usertime] starting, pid={}", civis::getpid());
    server::run()
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    println!("[usertime] panic");
    civis::exit(1)
}
