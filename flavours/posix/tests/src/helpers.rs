use super::*;

// ── Helper della suite POSIX (Fase 58.5, ADR-0041) ───────────────────
// Sottoinsieme di `testland/usertests/src/helpers.rs` (self-contained: le due
// suite sono separate, non condividono un crate). Tag/modi speculari a
// usertest-client / usertest-spin (stessa convenzione dei test storici).

pub const T_CFG: u64 = 100;
pub const T_ACK: u64 = 101;
pub const T_REQ: u64 = 102;
pub const T_DONE: u64 = 103;
pub const T_STOP: u64 = 104;

// Modi client usati dai test POSIX.
pub const M_SRV: u64 = 3;
pub const M_KILLME: u64 = 5;
pub const M_HARDEN: u64 = 23;
pub const M_DUPCLAIM: u64 = 26;
pub const M_DUPGRANT: u64 = 27;
pub const M_DUPSIBCLAIM: u64 = 28;
pub const M_SEEKDENY: u64 = 29;
pub const M_SUSPENDENY: u64 = 30;
pub const M_SIGCATCH: u64 = 31;

/// Mini-harness: registra il risultato di un test con prefisso `[posixtests]`.
pub fn report(total: &mut u32, ok: &mut u32, name: &str, pass: bool) {
    *total += 1;
    if pass {
        *ok += 1;
        println!("[posixtests] {}: PASS", name);
    } else {
        println!("[posixtests] {}: FAIL", name);
    }
}

/// Spawna un helper da disco (`/test/*.bin`) e gli invia la CFG (modo=w0,
/// param=w1) sul canale di nascita. Ritorna (canale verso il figlio, ack.w0).
pub fn spawn_cfg(path: &str, name: &str, prio: u8, mode: u64, param: u64) -> Option<(u64, u64)> {
    let img = civis::load_file(path)?;
    let meta = civis::SpawnMeta::new(name, prio, &[])?;
    let chan = civis::spawn_image(&img, &meta).ok()? as u64;
    let ack = civis::send(chan, T_CFG, mode, param).ok()?;
    Some((chan, ack.w0))
}

/// Attende un T_DONE da uno dei canali in `chans` (risponde col request-id;
/// scarta gli estranei con reply, le notifiche di morte senza). Ritorna
/// (ok, canale del mittente).
pub fn recv_done(chans: &[u64]) -> (bool, u64) {
    loop {
        match civis::recv() {
            Ok(m) => {
                let _ = civis::reply(T_ACK, 0, 0);
                if m.tag == T_DONE && chans.contains(&m.channel) {
                    return (m.w0 == 1, m.channel);
                }
            }
            Err(_) => return (false, 0),
        }
    }
}

/// Svuota i messaggi residui in coda (EXIT_NOTIFY dei helper precedenti
/// scartate senza reply). Da chiamare a inizio dei test che usano
/// `recv`/`wait_reply` "stretti".
pub fn drain_stray() {
    while let Some(m) = civis::recv_poll() {
        if !civis::is_exit_notify(&m) {
            let _ = civis::reply(T_ACK, 0, 0);
        }
    }
}

/// Attende sul canale `chan` la notifica `EXIT_NOTIFY` del kernel (figlio
/// morto: w0 = exit code, w1 = pid). Gli estranei vengono ignorati.
pub fn wait_exit(chan: u64) -> Option<(i64, i64)> {
    loop {
        match civis::recv() {
            Ok(m) if m.channel == chan && civis::is_exit_notify(&m) => {
                return Some((m.w0 as i64, m.w1 as i64));
            }
            Ok(_) => {}
            Err(_) => return None,
        }
    }
}
