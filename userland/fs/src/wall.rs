//! Wall-clock lazy per userfs (Fase 50, P1 orologio).
//!
//! I provider (`ramfs`, `Fat32`) producono `Meta.mtime` da qui: UNA query al
//! servizio `Time` al primo bisogno, poi solo aritmetica sul monotono PIT
//! (`get_ticks`, 100 Hz) — niente IPC nel per-op. Alla morte del server
//! (`note_peer_death` dal ramo `EXIT_NOTIFY`) il baseline cade e la prossima
//! chiamata lo rilegge (mai wall stale oltre un restart).

use super::*;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

static HAVE: AtomicBool = AtomicBool::new(false);
static EPOCH: AtomicU64 = AtomicU64::new(0);
static TICK: AtomicI64 = AtomicI64::new(0);
static CHAN: AtomicU64 = AtomicU64::new(u64::MAX);

fn refresh() {
    let Ok(chan) = libr::spawn::service_lookup(libr::Service::Time) else {
        return;
    };
    let Ok(r) = libr::ipc::send(chan as u64, libr::TIME_NOW, 0, 0) else {
        return;
    };
    // `tick_base` campionato subito dopo la reply: lo skew (frazione di un
    // round-trip IPC) e' trascurabile contro la granularita' di 10 ms.
    EPOCH.store(r.w0, Ordering::Relaxed);
    TICK.store(libr::get_ticks(), Ordering::Relaxed);
    CHAN.store(chan as u64, Ordering::Relaxed);
    HAVE.store(true, Ordering::Relaxed);
}

/// Secondi epoch correnti (UTC) per `Meta.mtime`. 0 a servizio assente
/// (stesso valore di prima della Fase 50: mtime sconosciuto, mai inventato).
pub fn wall_secs() -> u64 {
    if !HAVE.load(Ordering::Relaxed) {
        refresh();
    }
    if !HAVE.load(Ordering::Relaxed) {
        return 0;
    }
    let dt = libr::get_ticks().wrapping_sub(TICK.load(Ordering::Relaxed)).max(0) as u64;
    EPOCH.load(Ordering::Relaxed).wrapping_add(dt / 100)
}

/// Invalida il baseline alla morte del peer servito sul canale `chan`
/// (ramo `EXIT_NOTIFY` del loop): al prossimo bisogno si rilegge (epoch
/// riletta dal server riavviato, mai wall di un'epoca morta).
pub fn note_peer_death(chan: u64) {
    if CHAN.load(Ordering::Relaxed) == chan {
        HAVE.store(false, Ordering::Relaxed);
        CHAN.store(u64::MAX, Ordering::Relaxed);
    }
}
