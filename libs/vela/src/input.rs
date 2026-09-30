//! Input (server `kbd`): scancode PS/2 e notify verso la terminale.
//!
//! Primo accumulo da R3 (mossi da `kbd` tali e quali, zero comportamento):
//! porte PS/2 e coda scancode condivisibili coi futuri driver input (usb).

/// Porta dati PS/2.
pub const PS2_DATA: u16 = 0x60;
/// Porta stato/comandi PS/2.
pub const PS2_STATUS: u16 = 0x64;
/// Bit 0 di 0x64: byte in attesa (OBF).
pub const PS2_OBF: u8 = 0x01;
/// Bit 1 di 0x64: controller occupato (IBF).
pub const PS2_IBF: u8 = 0x02;
/// Bit 5 di 0x64: il byte in attesa viene dal mouse (AUX), non dalla tastiera.
pub const PS2_AUX: u8 = 0x20;
/// Bit di errore di 0x64: parita' (7) e timeout (6) — il byte e' spazzatura.
pub const PS2_ERR: u8 = 0xC0;

/// Coda scancode interna (cap 256, oltre si scarta: stesso contratto della
/// vecchia coda kernel). Spostata dal driver senza modifiche.
pub struct ScanQueue {
    buf: [u8; 256],
    head: usize,
    tail: usize,
    len: usize,
}

impl ScanQueue {
    /// Coda vuota (costruibile in `static`/constesto).
    pub const fn new() -> Self {
        Self { buf: [0; 256], head: 0, tail: 0, len: 0 }
    }

    /// Accoda uno scancode (no-op a coda piena).
    pub fn push(&mut self, sc: u8) {
        if self.len == 256 {
            return;
        }
        self.buf[self.tail] = sc;
        self.tail = (self.tail + 1) % 256;
        self.len += 1;
    }

    /// Byte in attesa.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Drena fino a `out.len()` byte, ritorna quanti.
    pub fn drain_into(&mut self, out: &mut [u8]) -> usize {
        let mut n = 0;
        while self.len > 0 && n < out.len() {
            out[n] = self.buf[self.head];
            self.head = (self.head + 1) % 256;
            self.len -= 1;
            n += 1;
        }
        n
    }
}

