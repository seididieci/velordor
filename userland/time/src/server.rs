//! Server `Time` (Fase 50, P1 orologio).
//!
//! Baseline letto UNA volta all'avvio (`cmos::read_epoch`); poi solo
//! aritmetica sul monotono PIT: `sec = epoch_base + dt/100`,
//! `csec = dt % 100` con `dt = get_ticks() - tick_base` (saturato a 0).
//! Server sincrono veloce: niente stato per-client, niente async.

use super::*;
use civis::{EXIT_NOTIFY, TIME_NOW};

/// Baseline del servizio (epoch CMOS + tick PIT del campionamento).
#[derive(Clone, Copy)]
pub struct Baseline {
    epoch_base: u64,
    tick_base: i64,
}

impl Baseline {
    /// Data/ora correnti dal monotono: `(sec_epoch_utc, centesimi 0..99)`.
    /// Monotona per costruzione (PIT monotono, epoch fissato al boot).
    pub fn now(&self) -> (u64, u64) {
        let dt = civis::get_ticks().wrapping_sub(self.tick_base).max(0) as u64;
        (self.epoch_base.wrapping_add(dt / 100), dt % 100)
    }
}

pub fn run() -> ! {
    let baseline = match cmos::read_epoch() {
        Some((epoch, tick)) => {
            println!("[usertime] CMOS epoch={} tick_base={}", epoch, tick);
            Baseline { epoch_base: epoch, tick_base: tick }
        }
        None => {
            // Degrado loud (mai wedge il boot): monotono salvo, wall-clock
            // da verificare — i test su mtime falliscono e lo segnalano.
            println!("[usertime] CMOS illeggibile, degrado a epoch=0");
            Baseline { epoch_base: 0, tick_base: civis::get_ticks() }
        }
    };

    if civis::service_register(civis::Service::Time).is_err() {
        println!("[usertime] FAILED to register service Time");
        civis::exit(1);
    }
    println!("[usertime] registered as service Time");
    civis::signal_ready(1);

    loop {
        match civis::recv() {
            Ok(m) if m.tag == TIME_NOW => {
                let (sec, csec) = baseline.now();
                let _ = civis::reply(TIME_NOW, sec, csec);
            }
            Ok(m) if m.tag == EXIT_NOTIFY => {
                // Morte di un peer (parent/test): nessun stato per-client
                // da pulire, il baseline sopravvive. Niente reply.
            }
            Ok(_) => {
                // Tag ignoto su `send` sincrona: rispondere errore invece di
                // appendere il mittente (mai hang silenziosi).
                let _ = civis::reply(TIME_NOW, u64::MAX, u64::MAX);
            }
            Err(_) => {}
        }
    }
}
