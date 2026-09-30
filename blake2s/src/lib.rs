//! BLAKE2s-256 (Fase 54, P5 integrita').
//!
//! Reimplementazione propria, auditabile (~250 righe, solo `u32`, niente
//! `alloc`, niente heap: tutto lo stato vive nel chiamante). Usata da
//! cardo (`R_GET_HASH`), guest `arca` e `testsarca`. RFC 7693, non-keyed
//! (key_length = 0: la chiave e' un attributo futuro, mai silenzioso).
//!
//! Cancelli in-place rispettati (vedi P5 in ROADMAP): build freestanding
//! `no_std` senza alloc; il binario che la linka resta entro
//! `SPAWN_IMAGE_MAX` (cardo ~181 KiB + ~3 KiB qui); vettori di riferimento
//! generati da due implementazioni indipendenti (Python `hashlib` +
//! OpenSSL, coincidenti) nei `#[cfg(test)]`; niente heap nel per-op
//! (`Hasher` sta in stack/caller, `update`/`finalize` non allocano mai).

// `std` solo per l'harness `#[cfg(test)]` su host (il target bare-metal del
// repo non ha la crate `test`: i test host girano fuori dal config repo).
#![cfg_attr(not(test), no_std)]

/// Lunghezza digest in byte (BLAKE2s-256).
pub const OUT_LEN: usize = 32;
/// Blocco di compressione in byte.
pub const BLOCK_LEN: usize = 64;

/// Vettori messaggio di SIGMA (RFC 7693 §2.1, 10 round).
const SIGMA: [[u8; 16]; 10] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
    [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
    [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
    [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
    [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
    [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
    [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
    [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
];

/// Vettore di inizializzazione (frazionaria di pi greco, RFC 7693 §2.2).
const IV: [u32; 8] = [
    0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A, 0x510E527F, 0x9B05688C,
    0x1F83D9AB, 0x5BE0CD19,
];

/// Funzione di mixing G (RFC 7693 §2.1): opera su `v` con le word `x`, `y`.
#[inline(always)]
fn g(v: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize, x: u32, y: u32) {
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(12);
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
    v[d] = (v[d] ^ v[a]).rotate_right(8);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(7);
}

/// Compressione di un blocco (RFC 7693 §2.2): `h` mutato sul posto.
/// `t0`/`t1` = byte compressi finora (contatore a 128 bit come coppia u64),
/// `final_block` = ultimo blocco del messaggio.
fn compress(h: &mut [u32; 8], block: &[u32; 16], t0: u64, t1: u64, final_block: bool) {
    let mut v = [0u32; 16];
    v[..8].copy_from_slice(h);
    v[8..].copy_from_slice(&IV);
    v[12] ^= t0 as u32;
    v[13] ^= (t0 >> 32) as u32;
    v[14] ^= t1 as u32;
    v[15] ^= (t1 >> 32) as u32;
    if final_block {
        v[14] = !v[14];
    }
    for r in 0..10 {
        let s = &SIGMA[r];
        g(&mut v, 0, 4, 8, 12, block[s[0] as usize], block[s[1] as usize]);
        g(&mut v, 1, 5, 9, 13, block[s[2] as usize], block[s[3] as usize]);
        g(&mut v, 2, 6, 10, 14, block[s[4] as usize], block[s[5] as usize]);
        g(&mut v, 3, 7, 11, 15, block[s[6] as usize], block[s[7] as usize]);
        g(&mut v, 0, 5, 10, 15, block[s[8] as usize], block[s[9] as usize]);
        g(&mut v, 1, 6, 11, 12, block[s[10] as usize], block[s[11] as usize]);
        g(&mut v, 2, 7, 8, 13, block[s[12] as usize], block[s[13] as usize]);
        g(&mut v, 3, 4, 9, 14, block[s[14] as usize], block[s[15] as usize]);
    }
    for i in 0..8 {
        h[i] ^= v[i] ^ v[i + 8];
    }
}

/// Legge 16 word LE da 64 byte (il chiamante garantisce la lunghezza).
fn block_words(block: &[u8]) -> [u32; 16] {
    let mut m = [0u32; 16];
    for (i, w) in m.iter_mut().enumerate() {
        let o = i * 4;
        *w = u32::from_le_bytes([block[o], block[o + 1], block[o + 2], block[o + 3]]);
    }
    m
}

/// Stato streaming (tutto caller-owned: niente alloc, niente heap).
/// `buflen` < 64 sempre dopo `update` (l'ultimo blocco si comprime solo a
/// `finalize`: cosi' un input multiplo esatto di 64 NON produce un blocco
/// finale vuoto spurio).
pub struct Hasher {
    h: [u32; 8],
    buf: [u8; BLOCK_LEN],
    buflen: usize,
    t0: u64,
    t1: u64,
}

impl Hasher {
    /// Nuovo hasher non-keyed (param block: digest 32, key 0, fanout 1,
    /// depth 1, resto 0 — RFC 7693 §2.3).
    pub fn new() -> Self {
        let mut h = IV;
        h[0] ^= 0x01010000 ^ (OUT_LEN as u32);
        Self { h, buf: [0u8; BLOCK_LEN], buflen: 0, t0: 0, t1: 0 }
    }

    /// Somma `n` al contatore (t0, t1) con riporto (i test non lo esercitano
    /// mai: servirebbero 2^64 byte; la correttezza non dipende dal volume).
    fn add_len(&mut self, n: u64) {
        let (t0, carry) = self.t0.overflowing_add(n);
        self.t0 = t0;
        if carry {
            self.t1 = self.t1.wrapping_add(1);
        }
    }

    /// Assorbe byte (chiamabile piu' volte, qualunque spezzatura).
    pub fn update(&mut self, mut input: &[u8]) {
        while !input.is_empty() {
            let take = (BLOCK_LEN - self.buflen).min(input.len());
            self.buf[self.buflen..self.buflen + take].copy_from_slice(&input[..take]);
            self.buflen += take;
            input = &input[take..];
            // Buffer pieno MA resta input: il blocco NON e' l'ultimo (se
            // fosse l'ultimo, finalize lo marcherebbe final — mai comprimere
            // qui un blocco che potrebbe essere finale).
            if self.buflen == BLOCK_LEN && !input.is_empty() {
                self.add_len(BLOCK_LEN as u64);
                let m = block_words(&self.buf);
                compress(&mut self.h, &m, self.t0, self.t1, false);
                self.buflen = 0;
            }
        }
    }

    /// Chiude e ritorna il digest (padding zero, ultimo blocco final).
    pub fn finalize(mut self) -> [u8; OUT_LEN] {
        for b in self.buf[self.buflen..].iter_mut() {
            *b = 0;
        }
        self.add_len(self.buflen as u64);
        let m = block_words(&self.buf);
        compress(&mut self.h, &m, self.t0, self.t1, true);
        let mut out = [0u8; OUT_LEN];
        for (i, w) in self.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        out
    }
}

impl Default for Hasher {
    fn default() -> Self {
        Self::new()
    }
}

/// One-shot: BLAKE2s-256 di `data` (non-keyed).
pub fn blake2s(data: &[u8]) -> [u8; OUT_LEN] {
    let mut h = Hasher::new();
    h.update(data);
    h.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(digest: &[u8; OUT_LEN]) -> String {
        // Solo-test su host (std disponibile nei `#[cfg(test)]`).
        let mut s = String::new();
        for b in digest {
            s.push_str(&format!("{:02x}", b));
        }
        s
    }

    #[test]
    fn rfc_vectors() {
        // Vettori generati da due implementazioni indipendenti (Python
        // `hashlib` + OpenSSL, coincidenti): empty, "abc", 56 B (un blocco),
        // 64 B (confine esatto: niente blocco finale spurio), 65 B
        // (due blocchi), pattern 1024 B (streaming multi-blocco).
        let cases: &[(&[u8], &str)] = &[
            (b"", "69217a3079908094e11121d042354a7c1f55b6482ca1a51e1b250dfd1ed0eef9"),
            (b"abc", "508c5e8c327c14e2e1a72ba34eeb452f37458b209ed63a294d999b4c86675982"),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "6f4df5116a6f332edab1d9e10ee87df6557beab6259d7663f3bcd5722c13f189",
            ),
        ];
        for (input, expect) in cases {
            assert_eq!(hex(&blake2s(input)), *expect, "one-shot len={}", input.len());
            // Stesso digest via streaming spezzato (1 byte alla volta).
            let mut h = Hasher::new();
            for chunk in input.chunks(1) {
                h.update(chunk);
            }
            assert_eq!(hex(&h.finalize()), *expect, "streaming len={}", input.len());
        }
    }

    #[test]
    fn block_boundaries() {
        // 64 B esatti (un blocco, final su di esso) e 65 B (due blocchi).
        let b64: Vec<u8> = (0..64u8).collect();
        let b65: Vec<u8> = (0..65u8).collect();
        assert_eq!(
            hex(&blake2s(&b64)),
            "56f34e8b96557e90c1f24b52d0c89d51086acf1b00f634cf1dde9233b8eaaa3e"
        );
        // Spezzatura 64+1 deve coincidere con one-shot (l'ultimo blocco e'
        // final in entrambi i casi, mai extra).
        let mut h = Hasher::new();
        h.update(&b65[..64]);
        h.update(&b65[64..]);
        assert_eq!(hex(&h.finalize()), hex(&blake2s(&b65)));
        assert_eq!(
            hex(&blake2s(&b65)),
            "1b53ee94aaf34e4b159d48de352c7f0661d0a40edff95a0b1639b4090e974472"
        );
    }

    #[test]
    fn long_streaming() {
        // 1024 B a chunk irregolari (7, 64, 300, resto): lo streaming deve
        // coincidere col one-shot qualunque sia la spezzatura.
        let data: Vec<u8> = (0..1024usize).map(|i| ((i * 7) % 251) as u8).collect();
        let expect = "691037ff5619f6d4de186823379396efe99a6e242a8a1ba93f29296e5ea8827f";
        assert_eq!(hex(&blake2s(&data)), expect);
        let mut h = Hasher::new();
        h.update(&data[..7]);
        h.update(&data[7..71]);
        h.update(&data[71..371]);
        h.update(&data[371..]);
        assert_eq!(hex(&h.finalize()), expect);
    }
}
