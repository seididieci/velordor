//! Client del servizio `Time` (Fase 50, P1 orologio).
//!
//! `usertime` legge il CMOS all'avvio (epoch) e serve `TIME_NOW` (epoch +
//! monotono PIT a 100 Hz). Questo modulo e' il client sincrono: una `send`
//! per chiamata, niente cache locale (la politica di cache — es. baseline
//! + `get_ticks` in userfs — sta nel chiamante, non qui).

use super::*;

/// Legge data/ora dal servizio `Time`: `(secondi_epoch_utc, centesimi)`.
///
/// I centesimi sono `0..99` nel secondo corrente (PIT a 100 Hz). Fallisce
/// solo a servizio assente/morto (`ServerDied`).
#[inline]
pub fn time_now() -> Result<(u64, u64), Error> {
    let chan = spawn::service_lookup(Service::Time)? as u64;
    let r = ipc::send(chan, TIME_NOW, 0, 0)?;
    Ok((r.w0, r.w1))
}

/// Scorciatoia per chi serve solo il secondo (mtime del FS, log): secondi
/// epoch (UTC) o `Err` a servizio assente/morto.
#[inline]
pub fn wall_secs() -> Result<u64, Error> {
    time_now().map(|(sec, _)| sec)
}

// ── Aritmetica civile pura (Fase 50): condivisa tra `usertime` (CMOS →
// epoch) e userfs (timestamp DOS FAT ↔ epoch). Nessun I/O, nessun alloc,
// solo interi (algoritmi days-from-civil / civil-from-days, Hinnant). ──

/// Giorni civili da epoch (1970-01-01) per data Gregoriana. Dominio valido
/// anni >= 1970 con aritmetica i64 (mai overflow qui).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Inversa di `days_from_civil`: `(anno, mese, giorno)` dai giorni da epoch.
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Timestamp DOS FAT (WrtDate/WrtTime on-disk, LE16) → secondi epoch (UTC).
/// Layout: data = `((anno-1980)<<9)|(mese<<5)|giorno`, ora =
/// `(ora<<11)|(min<<5)|(sec/2)`. Fuori range (campi assurdi) → 0
/// (sconosciuto, mai epoch inventata).
pub fn dos_to_epoch(wdate: u16, wtime: u16) -> u64 {
    let year = ((wdate >> 9) & 0x7F) as i64 + 1980;
    let month = ((wdate >> 5) & 0x0F) as i64;
    let day = (wdate & 0x1F) as i64;
    let hour = ((wtime >> 11) & 0x1F) as i64;
    let min = ((wtime >> 5) & 0x3F) as i64;
    let sec = ((wtime & 0x1F) as i64) * 2;
    if !(1980..=2107).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || min > 59
        || sec > 59
    {
        return 0;
    }
    let days = days_from_civil(year, month, day);
    (days * 86_400 + hour * 3600 + min * 60 + sec).max(0) as u64
}

/// Secondi epoch (UTC) → timestamp DOS FAT `(WrtDate, WrtTime)`.
/// Clamp al dominio DOS (1980-01-01 .. 2107-12-31 23:59:58, risoluzione 2 s):
/// prima del 1980 → 1980-01-01 00:00:00, oltre → fondo scala.
pub fn epoch_to_dos(epoch: u64) -> (u16, u16) {
    const DOS_MIN: u64 = 315_532_800; // 1980-01-01 00:00:00 UTC
    const DOS_MAX: u64 = 4_354_838_398; // 2107-12-31 23:59:58 UTC
    let clamped = epoch.clamp(DOS_MIN, DOS_MAX);
    let days = (clamped / 86_400) as i64;
    let tod = (clamped % 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    let wdate = (((y - 1980) << 9) | (m << 5) | d) as u16;
    let wtime = ((tod / 3600) << 11 | ((tod % 3600) / 60) << 5 | ((tod % 60) / 2)) as u16;
    (wdate, wtime)
}
