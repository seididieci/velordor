//! `arcafs` — casa del sottosistema ArcaFS (Fase 56.2a).
//!
//! Contiene tutto cio' che guest (userfs), client (via `libr`) e tool host
//! condividono: tag di protocollo (`proto`), builder/parser dei frame
//! (`wire`, puri, niente I/O) e formato blocchi on-disk (`format`).
//! I wrapper IPC con retry vivono in `libr` (usano i suoi ring/sessione:
//! spostarli qui creerebbe un ciclo `libr`↔`arcafs`); `libr` li riesporta,
//! quindi i client esistenti non cambiano import.
//!
//! `no_std` + `alloc` (come `blake2s`): niente syscall, niente I/O qui.

// `std` solo per l'harness `#[cfg(test)]` su host.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod format;
pub mod proto;
pub mod wire;

pub use proto::*;
// Comodita' per il tool host (e chi verifica checksum): la FNV-1a resta
// definita in `syscall-numbers` (usata anche dal manifest/kernel).
pub use syscall_numbers::image_hash;
