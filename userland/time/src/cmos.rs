//! Lettura dell'orologio CMOS/RTC (Fase 50, P1 orologio).
//!
//! Porte `0x70` (indice) / `0x71` (dati), concesse da init via `io_ranges`
//! (TSS per-processo, ADR-0006): qualunque altro accesso porta e' #GP.
//! La lettura avviene UNA volta all'avvio: ne esce `(epoch_base, tick_base)`
//! e il tempo poi avanza sul monotono PIT (`get_ticks`, 100 Hz), mai piu'
//! toccando il CMOS.

/// Porte CMOS/RTC (MC146818).
const CMOS_ADDR: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;

/// Registri RTC letti.
const REG_SEC: u8 = 0x00;
const REG_MIN: u8 = 0x02;
const REG_HOUR: u8 = 0x04;
const REG_DAY: u8 = 0x07;
const REG_MONTH: u8 = 0x08;
const REG_YEAR: u8 = 0x09;
/// Registro secolo (non standard IBM, ma presente su QEMU/Bochs e quasi
/// tutto l'hardware reale): best-effort, con validazione + fallback.
const REG_CENTURY: u8 = 0x32;
/// Registri di stato.
const REG_STATUS_A: u8 = 0x0A;
const REG_STATUS_B: u8 = 0x0B;

/// Bit 7 di `0x70`: a 1 disabilita l'NMI durante l'accesso. Si preserva il
/// valore corrente (mai abilitare/disabilitare l'NMI di nascosto).
fn cmos_read(reg: u8) -> u8 {
    unsafe {
        let addr = libr::pio::inb(CMOS_ADDR);
        libr::pio::outb(CMOS_ADDR, (addr & 0x80) | (reg & 0x7F));
        libr::pio::inb(CMOS_DATA)
    }
}

fn bcd_to_bin(v: u8) -> u8 {
    (v & 0x0F) + ((v >> 4) * 10)
}

/// Istantanea dei registri data/ora (grezzi, come letti dal chip).
#[derive(Clone, Copy, PartialEq, Eq)]
struct RtcRaw {
    sec: u8,
    min: u8,
    hour: u8,
    day: u8,
    month: u8,
    year: u8,
    century: u8,
}

fn read_raw() -> RtcRaw {
    RtcRaw {
        sec: cmos_read(REG_SEC),
        min: cmos_read(REG_MIN),
        hour: cmos_read(REG_HOUR),
        day: cmos_read(REG_DAY),
        month: cmos_read(REG_MONTH),
        year: cmos_read(REG_YEAR),
        century: cmos_read(REG_CENTURY),
    }
}

/// Attende che il flag UIP (update-in-progress, bit 7 di status A) si abbassi:
/// durante l'update i registri sono incoerenti. Bound anti-hang (mai wedge il
/// boot su hardware ostile): a timeout si procede comunque (la coerenza e'
/// poi verificata dalla doppia lettura).
fn wait_no_uip() {
    for _ in 0..100_000 {
        if cmos_read(REG_STATUS_A) & 0x80 == 0 {
            return;
        }
        core::hint::spin_loop();
    }
}

/// Legge un'istantanea coerente: doppia lettura finche' due campioni consecutivi
/// coincidono (chiude la race del rollover del secondo senza interrupt RTC).
/// Bound: a campioni mai stabili si usa l'ultimo (fail-loud a valle se assurdo).
fn read_stable() -> RtcRaw {
    let mut prev = read_raw();
    for _ in 0..8 {
        wait_no_uip();
        let cur = read_raw();
        if cur == prev {
            return cur;
        }
        prev = cur;
    }
    prev
}

/// Legge il CMOS e ritorna `(epoch_base_sec_utc, tick_base)`.
///
/// `tick_base` e' il `get_ticks()` letto subito dopo l'istantanea coerente:
/// il server poi serve `epoch_base + (now - tick_base)/100` senza piu'
/// toccare l'hardware. `None` se i registri sono assurdi (fuori range
/// dopo la conversione): il chiamante degrada a epoch 0 con log loud
/// (monotono salvo, wall-clock da verificare nei test).
pub fn read_epoch() -> Option<(u64, i64)> {
    let raw = read_stable();
    let tick_base = libr::get_ticks();
    let status_b = cmos_read(REG_STATUS_B);
    // Status B: bit 2 = 24h (senza: 12h + bit 7 PM su hour), bit 1 = binario
    // (senza: BCD). Campionati dopo i dati: se un update li ha cambiati nel
    // mentre, la doppia lettura sopra ha gia' garantito coerenza dei dati ma
    // non del modo — margine accettato (QEMU: modo fisso 24h/BCD o binario).
    let binary = status_b & 0x04 != 0;
    let h24 = status_b & 0x02 != 0;
    let cvt = |v: u8| -> u8 { if binary { v } else { bcd_to_bin(v) } };

    let sec = cvt(raw.sec) as i64;
    let min = cvt(raw.min) as i64;
    let mut hour = cvt(raw.hour) as i64;
    let day = cvt(raw.day) as i64;
    let month = cvt(raw.month) as i64;
    let year2 = cvt(raw.year) as i64;
    let cent = cvt(raw.century) as i64;
    if !h24 {
        let pm = hour & 0x80 != 0;
        hour &= 0x7F;
        if pm {
            hour = (hour % 12) + 12;
        } else if hour == 12 {
            hour = 0;
        }
    }
    // Secolo: valido 19..=21, altrimenti fallback dichiarato 20 (QEMU senza
    // century = 0x00 → fallback, mai anno 0).
    let century = if (19..=21).contains(&cent) { cent } else { 20 };
    let year = century * 100 + year2;

    // Validazione di plausibilita' (mai epoch assurdi in silenzio).
    if !(1970..=2100).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || min > 59
        || sec > 60
    {
        return None;
    }
    let days = libr::time::days_from_civil(year, month, day);
    let epoch = days * 86_400 + hour * 3600 + min * 60 + sec;
    if epoch < 0 {
        return None;
    }
    Some((epoch as u64, tick_base))
}
