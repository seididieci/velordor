//! Thread Velordo v1 (S1.3): solo sleep su ticks (spawn OS in S2 sopra
//! SYS_THREAD_CREATE; il resto da `unsupported`: spawn=Err, yield=no-op).
//! Forma dell'arm uefi in `sys/thread/mod.rs`.

use crate::sys::pal::ticks as pal_ticks;
use crate::time::Duration;

/// Dorme almeno `dur` (spin su ticks con hint: niente syscall sleep nel
/// kernel — IF=1 quasi tutto il tempo, il timer previene la fame).
pub fn sleep(dur: Duration) {
    let ms = dur.as_millis();
    if ms == 0 {
        return;
    }
    // 10 ms per tick, arrotonda in eccesso (+1 tick di margine).
    let wait = ms.div_ceil(10) as u64 + 1;
    let t0 = pal_ticks();
    while pal_ticks().wrapping_sub(t0) < wait {
        core::hint::spin_loop();
    }
}
