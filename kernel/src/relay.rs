//! Relay — ciò che permette ai componenti di comunicare (Fase R1, ADR-0040).
//!
//! IPC per nome + canali (Fase 12, ADR-0008): `channels` e' il registry dei
//! servizi e le coppie bidirezionali tra processi. L'entry syscall resta in
//! `syscall/` (entry point, non meccanismo).

pub mod channels;
