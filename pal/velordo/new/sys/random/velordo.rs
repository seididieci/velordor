//! casualita' Velordo (S1.3): RDRAND hardware (presente su qemu64/KVM,
//! verificato via cpuid a boot del progetto).
//!
//! `fill_bytes` non mente mai: o RDRAND riesce, o abort (mai zeri
//! spacciati per casuali — gli hash DoS-sensitive dipendono da questo).

use core::arch::x86_64::_rdrand64_step;

pub fn fill_bytes(buf: &mut [u8]) {
    let mut chunks = buf.chunks_exact_mut(8);
    for c in &mut chunks {
        let mut v: u64 = 0;
        let mut ok = false;
        // Intel guidance: retry finche' riesce (max 10 tentativi).
        for _ in 0..10 {
            unsafe {
                let mut tmp = 0u64;
                if _rdrand64_step(&mut tmp) == 1 {
                    v = tmp;
                    ok = true;
                    break;
                }
            }
        }
        if !ok {
            crate::sys::pal::abort_internal();
        }
        c.copy_from_slice(&v.to_ne_bytes());
    }
    let rem = chunks.into_remainder();
    if !rem.is_empty() {
        let mut v: u64 = 0;
        let mut ok = false;
        for _ in 0..10 {
            unsafe {
                let mut tmp = 0u64;
                if _rdrand64_step(&mut tmp) == 1 {
                    v = tmp;
                    ok = true;
                    break;
                }
            }
        }
        if !ok {
            crate::sys::pal::abort_internal();
        }
        rem.copy_from_slice(&v.to_ne_bytes()[..rem.len()]);
    }
}
