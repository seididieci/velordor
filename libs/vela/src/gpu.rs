//! Terminale video (server `gpu`): VGA + echo + cursore hardware.
//!
//! Primo accumulo da R4 (mosse da `gpu` tali e quali, zero comportamento):
//! geometria testo e porte CRTC condivisibili coi futuri driver video.
//! La line discipline (`porta`) resta fuori.

/// Indirizzo fisico del frame buffer VGA testo.
pub const VGA_PHYS: u64 = 0xB8000;
/// Righe del buffer testo VGA.
pub const VGA_ROWS: usize = 25;
/// Colonne del buffer testo VGA.
pub const VGA_COLS: usize = 80;
/// Porta indice CRTC (cursore hardware).
pub const CRTC_INDEX: u16 = 0x3D4;
/// Porta dati CRTC (cursore hardware).
pub const CRTC_DATA: u16 = 0x3D5;
