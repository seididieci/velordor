//! Ordo — ciò che ordina l'esecuzione (Fase R1, ADR-0040).
//!
//! Scheduler RT a 32 priorita' + CBS (Fase 11, ADR-0007): `sched` e' lo
//! scheduler preemptive, `aegis` e' l'isolamento temporale / bandwidth
//! reservation (Constant Bandwidth Server) che vive dentro lo scheduler.
//! `process`/`context` sono PCB e context switch. I chiamanti usano i path
//! `crate::ordo::*` (mai re-export piatti: la struttura resta vera).

pub mod aegis;
pub mod context;
pub mod process;
pub mod sched;
