//! Server `Log` (Fase 57, L1 nativo, ADR-0039).
//!
//! Gateway centrale WRITE+READ, primo servizio dopo init (prima di disk/fs):
//! all'avvio ZERO contatti FS/`Time` (solo `get_ticks`, syscall diretta).
//! Ogni APPEND e' sync-su-RAM (µs, mai disco nel percorso del chiamante);
//! alla `LOG_FLUSH` (solo parent/init, dopo fs+time) il server fa handshake
//! FS, retro-data e ri-chiavia i record pre-Time in RAM (il giorno-0 non
//! raggiunge mai il disco) e riversa in incrementale (4/giro) + dual-write.
//!
//! Bucket per IDENTITA' del chiamante (`peer_info` al REG, mai dichiarato):
//! chiavi `<hash>/<giorno>/<seq>`; indice persistente `<hash>/<giorno>/!idx`
//! per la latest dopo un restart. Seal = snapshot esplicito (commit per-op:
//! niente R_SYNC). Drop ammesso e contato (mai wedge).
//!
//! NON posix (verifica: `rg "open\(|read_fs|write_fs|::mount" src` = zero):
//! solo `R_OBJ_*`/`R_SNAP_*` nativi + `TIME_NOW`. Lo storage-TCB non chiama
//! mai qui (anti-ciclo); il kernel resta su seriale per disegno.

use super::*;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;
use libr::{
    EXIT_NOTIFY, LOG_APPEND, LOG_FLUSH, LOG_MSG_MAX, LOG_RAM_TAIL, LOG_READ, LOG_REG,
    LOG_SEAL, LOG_SRC_MAX, LOG_STATS,
};

/// Registrazione di un client: canale → phys dei suoi ring LOG + attribuzione
/// (pid + hash immagine al REG; anti-spoof pieno rimandato ad A4/ABAC).
struct Reg {
    chan: u64,
    req_phys: u64,
    resp_phys: u64,
    pid: i64,
    hash: u64,
}

/// Stato del servizio (RAM; il durevole sta nel bucket `log`).
struct State {
    regs: Vec<Reg>,
    /// Indice `(hash, giorno) → max seq` (ricostruito dal volume via `!idx`
    /// quando la RAM e' vuota: restart-safe).
    index: BTreeMap<(u64, u64), u64>,
    /// Coda degli ultimi record `(chiave, record)`: leggibile anche senza
    /// volume; oltre il cap si butta il piu' vecchio (`evicted`).
    tail: VecDeque<(Vec<u8>, Vec<u8>)>,
    /// Cursore del riversamento (indice in `tail` del prossimo da persistere).
    flush_cursor: usize,
    want_volume: bool,
    appended: u64,
    evicted: u64,
    durable: bool,
    last_seal: u64,
}

impl State {
    fn new() -> State {
        State {
            regs: Vec::new(),
            index: BTreeMap::new(),
            tail: VecDeque::new(),
            flush_cursor: 0,
            want_volume: false,
            appended: 0,
            evicted: 0,
            durable: false,
            last_seal: 0,
        }
    }
}

/// Legge l'header del frame in testa al ring a `va` senza avanzare:
/// `(tag, w0, w1, payload_disponibile)`. None a ring corto (< 20 B).
unsafe fn peek_header(va: u64) -> Option<(u32, u64, u64, usize)> {
    unsafe {
        let (head, tail) = libr::ring_positions(va);
        let avail = libr::ring_available(head, tail);
        if avail < 20 {
            return None;
        }
        let mut hdr = [0u8; 20];
        for (i, b) in hdr.iter_mut().enumerate() {
            let p = ((tail as usize) + i) % libr::RING_DATA_CAP;
            *b = core::ptr::read_volatile((va as *const u8).add(p));
        }
        let tag = u32::from_le_bytes(hdr[0..4].try_into().ok()?);
        let w0 = u64::from_le_bytes(hdr[4..12].try_into().ok()?);
        let w1 = u64::from_le_bytes(hdr[12..20].try_into().ok()?);
        Some((tag, w0, w1, avail - 20))
    }
}

/// Resync fail-closed del ring client (tail=head, stampo cardo): il frame in
/// testa e' impossibile e qualunque consumo parziale disallineerebbe per
/// sempre. Il mittente vede ERR e ritenta/riporta.
unsafe fn resync(va: u64) {
    unsafe {
        let (head, _) = libr::ring_positions(va);
        core::ptr::write_volatile((va + libr::RING_HEAD as u64) as *mut u32, head);
        core::ptr::write_volatile((va + libr::RING_TAIL as u64) as *mut u32, head);
    }
}

/// Registra/aggiorna il client (LOG_REG register-only: w0=req_phys,
/// w1=resp_phys). Cap 64: oltre si rifiuta fail-closed (mai tabella infinita).
/// Ritorna l'hash-bucket del chiamante (il client conosce il proprio bucket
/// senza poterlo scegliere).
#[inline(never)]
fn handle_reg(st: &mut State, chan: u64, req_phys: u64, resp_phys: u64) -> u64 {
    if req_phys == 0 || resp_phys == 0 {
        return libr::ERR;
    }
    let pid = libr::peer_pid(chan).unwrap_or(-1);
    let hash = libr::peer_info(chan).unwrap_or(0);
    if let Some(r) = st.regs.iter_mut().find(|r| r.chan == chan) {
        r.req_phys = req_phys;
        r.resp_phys = resp_phys;
        r.pid = pid;
        r.hash = hash;
        return hash;
    }
    if st.regs.len() >= 64 {
        return libr::ERR;
    }
    st.regs.push(Reg { chan, req_phys, resp_phys, pid, hash });
    hash
}

/// Persistenza opportunistica di UN record (chiave+bytes): PUT dato + PUT
/// `!idx` (max seq del giorno). True se entrambi Ok interi (durevole).
fn persist_one(hash: u64, day: u64, seq: u64, key: &[u8], rec: &[u8]) -> bool {
    let idx_key = libr::log::log_idx_key(hash, day);
    let idx_val = seq.to_le_bytes();
    match libr::obj_put(libr::log::LOG_BUCKET, key, rec) {
        Ok(n) if n as usize == rec.len() => {}
        _ => return false,
    }
    match libr::obj_put(libr::log::LOG_BUCKET, &idx_key, &idx_val) {
        Ok(8) => true,
        _ => false,
    }
}

/// APPEND: legge il frame dal ring LOG del client, timbra (tick sempre,
/// epoch se `Time` c'e'), accoda in RAM; persiste subito solo se il volume e'
/// gia' legato (post-FLUSH). Ritorna `(seq|ERR, durable)`.
/// Payload client `[level:1][taglen:1][tag][msg]`; `expect` = sua lunghezza.
/// `#[inline(never)]`: firewall di frame (payload in heap, mai stack KiB).
#[inline(never)]
fn handle_append(st: &mut State, chan: u64, expect: u64) -> (u64, u64) {
    let hash = match st.regs.iter().find(|r| r.chan == chan) {
        Some(r) => r.hash,
        None => return (libr::ERR_NOHANDSHAKE, 0),
    };
    let req_phys = match st.regs.iter().find(|r| r.chan == chan) {
        Some(r) => r.req_phys,
        None => return (libr::ERR_NOHANDSHAKE, 0),
    };
    if libr::map_physical(req_phys, libr::CLI_REQ_VA, 1).is_err() {
        return (libr::ERR, 0);
    }
    let (tag, w0, _w1, avail) = unsafe {
        match peek_header(libr::CLI_REQ_VA) {
            Some(h) => h,
            None => return (libr::ERR, 0),
        }
    };
    if tag != LOG_APPEND as u32 || w0 != expect || expect == 0 || (avail as u64) < expect {
        unsafe { resync(libr::CLI_REQ_VA) };
        return (libr::ERR, 0);
    }
    let n = expect as usize;
    let mut frame = alloc::vec![0u8; n];
    unsafe { libr::req_frame_read(libr::CLI_REQ_VA, &mut frame, n) };
    if frame.len() < 2 {
        return (libr::ERR, 0);
    }
    let level = frame[0];
    let taglen = frame[1] as usize;
    if level > libr::log::LOG_ERR
        || taglen == 0
        || taglen > LOG_SRC_MAX
        || frame.len() < 2 + taglen
    {
        return (libr::ERR, 0);
    }
    let ctag = &frame[2..2 + taglen];
    let msg = &frame[2 + taglen..];
    if !libr::log::tag_valid(ctag) || !libr::log::msg_valid(msg) {
        return (libr::ERR, 0);
    }

    let tick = libr::get_ticks() as u64;
    let epoch = libr::time::wall_secs().unwrap_or(0);
    let day = if epoch == 0 { 0 } else { epoch / 86_400 };
    let rec = match libr::log::record_encode(tick, epoch, level, ctag, msg) {
        Some(r) => r,
        None => return (libr::ERR, 0),
    };
    let seq = st.index.get(&(hash, day)).copied().unwrap_or(0) + 1;
    let key = libr::log::log_key(hash, day, seq);

    // Durevole subito solo a volume legato (post-FLUSH o re-bind): prima il
    // disco non esiste ancora per noi (mai hang a boot). A fallimento si
    // resta RAM-only (il cursore di flush ci ripassera').
    let mut durable = false;
    if st.want_volume {
        durable = persist_one(hash, day, seq, &key, &rec);
        st.durable = durable;
    }
    st.index.insert((hash, day), seq);
    if st.tail.len() >= LOG_RAM_TAIL {
        st.tail.pop_front();
        st.evicted += 1;
        if st.flush_cursor > 0 {
            st.flush_cursor -= 1;
        }
    }
    st.tail.push_back((key, rec));
    st.appended += 1;
    (seq, durable as u64)
}

/// READ own-bucket: payload `[giorno:8][seq:8]` (`seq=0` → latest).
/// Cerca in coda RAM, poi nell'indice persistente `!idx` + volume; scrive il
/// record nel response ring del client e ritorna `(len|ERR, seq)`. Il frame si
/// scrive PRIMA della reply (il client legge dopo il risveglio).
#[inline(never)]
fn handle_read(st: &mut State, chan: u64, expect: u64) -> (u64, u64) {
    let (hash, req_phys, resp_phys) = match st.regs.iter().find(|r| r.chan == chan) {
        Some(r) => (r.hash, r.req_phys, r.resp_phys),
        None => return (libr::ERR_NOHANDSHAKE, 0),
    };
    if libr::map_physical(req_phys, libr::CLI_REQ_VA, 1).is_err() {
        return (libr::ERR, 0);
    }
    let (tag, w0, _w1, avail) = unsafe {
        match peek_header(libr::CLI_REQ_VA) {
            Some(h) => h,
            None => return (libr::ERR, 0),
        }
    };
    if tag != LOG_READ as u32 || w0 != expect || expect != 16 || avail < 16 {
        unsafe { resync(libr::CLI_REQ_VA) };
        return (libr::ERR, 0);
    }
    let mut frame = [0u8; 16];
    unsafe { libr::req_frame_read(libr::CLI_REQ_VA, &mut frame, 16) };
    let day = u64::from_le_bytes(frame[0..8].try_into().unwrap_or([0; 8]));
    let mut seq = u64::from_le_bytes(frame[8..16].try_into().unwrap_or([0; 8]));
    if seq == 0 {
        // Latest: indice RAM, senno' `!idx` sul volume (restart-safe).
        seq = match st.index.get(&(hash, day)) {
            Some(&s) if s > 0 => s,
            _ => match st.want_volume {
                true => match libr::obj_get(libr::log::LOG_BUCKET, &libr::log::log_idx_key(hash, day)) {
                    Ok(v) if v.len() == 8 => {
                        let s = u64::from_le_bytes(v[..8].try_into().unwrap_or([0; 8]));
                        if s == 0 {
                            return (libr::ERR_NOTFOUND, 0);
                        }
                        st.index.insert((hash, day), s);
                        s
                    }
                    _ => return (libr::ERR_NOTFOUND, 0),
                },
                false => return (libr::ERR_NOTFOUND, 0),
            },
        };
    }
    if seq == 0 {
        return (libr::ERR_NOTFOUND, 0);
    }
    let key = libr::log::log_key(hash, day, seq);
    // Coda RAM prima (dagli ultimi: le versioni post-restart stanno in coda),
    // poi il volume.
    let mut rec: Option<Vec<u8>> = None;
    for (k, r) in st.tail.iter().rev() {
        if *k == key {
            rec = Some(r.clone());
            break;
        }
    }
    if rec.is_none() {
        if !st.want_volume {
            return (libr::ERR_NOTFOUND, 0);
        }
        match libr::obj_get(libr::log::LOG_BUCKET, &key) {
            Ok(v) if !v.is_empty() => rec = Some(v),
            _ => return (libr::ERR_NOTFOUND, 0),
        }
    }
    let rec = rec.unwrap_or_default();
    if rec.is_empty() || rec.len() > libr::RING_MAX_PAYLOAD {
        return (libr::ERR, 0);
    }
    if libr::map_physical(resp_phys, libr::CLI_RESP_VA, 1).is_err() {
        return (libr::ERR, 0);
    }
    unsafe { libr::resp_frame_write(libr::CLI_RESP_VA, &rec) };
    (rec.len() as u64, seq)
}

/// FLUSH (solo parent/init): lega il volume (primo contatto FS di sempre —
/// cardo e' su per costruzione dell'ordine di boot), retro-data e ri-chiavia
/// in RAM i record pre-Time (il giorno-0 non raggiunge mai il disco) e avvia
/// il riversamento incrementale (il loop drena 4/giro: mai bloccare gli
/// APPEND). Idempotente.
#[inline(never)]
fn handle_flush(st: &mut State, chan: u64) -> u64 {
    let is_parent = libr::peer_pid(chan).unwrap_or(-1) == 1;
    if !is_parent {
        return libr::ERR;
    }
    if !st.want_volume {
        st.want_volume = true;
        backdate_rekey(st);
    }
    0
}

/// Retro-data + re-key in RAM (solo pre-FLUSH e' gratis: niente e' su disco).
/// Per ogni record con epoch==0 e `Time` raggiungibile: epoch esatta
/// `now - (tick_now - tick_rec)/100`, giorno vero, nuova chiave con seq
/// ridato sul giorno; poi indice ricostruito da zero sulla coda finale.
/// Senza `Time`: niente (degrado dichiarato, chiavi giorno-0 persistono).
#[inline(never)]
fn backdate_rekey(st: &mut State) {
    let now_tick = libr::get_ticks() as u64;
    let now_epoch = match libr::time::wall_secs() {
        Ok(e) if e > 0 => e,
        _ => return,
    };
    // Tollera orologi assurdi (mai epoch passate dal record piu' avanti di ore).
    let mut moved = false;
    // Ricostruzione: si svuota la coda in un buffer e la si reinserisce con
    // chiavi finali (bound 128: heap, mai stack).
    let mut tmp: Vec<(u64, u64, Vec<u8>, Vec<u8>)> = Vec::new();
    while let Some((old_key, rec)) = st.tail.pop_front() {
        let (tick, epoch, level, tag, msg) = match libr::log::record_decode(&rec) {
            Some(v) => v,
            None => continue,
        };
        // Hash-bucket dalla chiave vecchia (primi 16 hex): la si riusa senza
        // fidarsi del contenuto (il tag e' cosmetico).
        let hash = match key_hash(&old_key) {
            Some(h) => h,
            None => continue,
        };
        if epoch != 0 {
            tmp.push((hash, old_day(&old_key), old_key, rec));
            continue;
        }
        let dt = now_tick.saturating_sub(tick) / 100;
        let back = now_epoch.saturating_sub(dt);
        if back == 0 {
            tmp.push((hash, 0, old_key, rec));
            continue;
        }
        let day = back / 86_400;
        let fixed = match libr::log::record_encode(tick, back, level, tag, msg) {
            Some(r) => r,
            None => continue,
        };
        tmp.push((hash, day, Vec::new(), fixed));
        moved = true;
    }
    if !moved {
        // Niente da rilocare: rimette tutto com'era (ordine stabile).
        for (h, d, k, r) in tmp {
            let _ = (h, d);
            st.tail.push_back((k, r));
        }
        return;
    }
    // Riassegna chiavi+seq per (hash, giorno) in ordine di coda (tick
    // crescente: l'ordine di arrivo e' l'ordine di coda per costruzione).
    // I record intatti tengono chiave E giorno vecchi (la tupla porta solo
    // il giorno nuovo per i rilocati).
    let mut counters: BTreeMap<(u64, u64), u64> = BTreeMap::new();
    st.index.clear();
    for (h, d, k, r) in tmp {
        let (day, key) = if k.is_empty() {
            // Rilocato: seq ridato sul giorno vero.
            let s = counters.get(&(h, d)).copied().unwrap_or(0) + 1;
            counters.insert((h, d), s);
            (d, libr::log::log_key(h, d, s))
        } else {
            // Intatto: giorno+seq dalla chiave vecchia.
            let od = old_day(&k);
            let s = key_seq(&k);
            let e = counters.get(&(h, od)).copied().unwrap_or(0);
            if s > e {
                counters.insert((h, od), s);
            }
            (od, k)
        };
        let e = st.index.get(&(h, day)).copied().unwrap_or(0);
        let s = key_seq(&key);
        if s > e {
            st.index.insert((h, day), s);
        }
        let _ = r.len();
        st.tail.push_back((key, r));
    }
    // Il cursore di flush riparte da zero: tutto va persistito con chiavi finali.
    st.flush_cursor = 0;
    println!("[userlog] backdate+rekey fatto");
}

/// Hash-bucket (u64) dai primi 16 hex della chiave. None se malformata.
fn key_hash(key: &[u8]) -> Option<u64> {
    if key.len() < 17 || key[16] != b'/' {
        return None;
    }
    let mut h: u64 = 0;
    for &c in &key[..16] {
        h = (h << 4) | hexval(c)? as u64;
    }
    Some(h)
}

/// Giorno (u64) dagli 8 hex dopo il primo `/`. 0 se malformata.
fn old_day(key: &[u8]) -> u64 {
    if key.len() < 25 || key[16] != b'/' {
        return 0;
    }
    let mut d: u64 = 0;
    for &c in &key[17..25] {
        match hexval(c) {
            Some(v) => d = (d << 4) | v as u64,
            None => return 0,
        }
    }
    d
}

/// Seq (u64) dagli ultimi 16 hex della chiave. 0 se malformata.
fn key_seq(key: &[u8]) -> u64 {
    if key.len() < 16 || key[key.len() - 17] != b'/' {
        return 0;
    }
    let mut s: u64 = 0;
    for &c in &key[key.len() - 16..] {
        match hexval(c) {
            Some(v) => s = (s << 4) | v as u64,
            None => return 0,
        }
    }
    s
}

fn hexval(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

/// Drena fino a 4 record pendenti verso il volume (chiamato a ogni giro di
/// loop post-FLUSH: riversamento incrementale, mai bloccare gli APPEND).
/// A volume assente/fallito si resta RAM-only (retry al prossimo giro).
fn drain_flush(st: &mut State) {
    if !st.want_volume {
        return;
    }
    let mut n = 0;
    while st.flush_cursor < st.tail.len() && n < 4 {
        let (key, rec) = &st.tail[st.flush_cursor];
        let hash = match key_hash(key) {
            Some(h) => h,
            None => {
                st.flush_cursor += 1;
                continue;
            }
        };
        let day = old_day(key);
        let seq = key_seq(key);
        if seq == 0 {
            st.flush_cursor += 1;
            continue;
        }
        if persist_one(hash, day, seq, key, rec) {
            st.durable = true;
            st.flush_cursor += 1;
            n += 1;
        } else {
            st.durable = false;
            return;
        }
    }
    // APPEND post-FLUSH persistono subito in `handle_append`: qui solo il
    // pregresso (il cursore resta in coda per i nuovi arrivi? no: i nuovi
    // vanno diretti; il cursore avanza solo sul pregresso).
}

pub fn run() -> ! {
    // Avvio a ZERO dipendenze (Fase 57 rivista): niente FS, niente Time —
    // solo registrazione + READY. Il primo contatto FS avviene alla FLUSH.
    if libr::service_register(libr::Service::Log).is_err() {
        println!("[userlog] FAILED to register service Log");
        libr::exit(1);
    }
    println!("[userlog] registered as service Log");
    libr::signal_ready(1);

    // Re-bind opportunistico dopo un restart (init-restart): se Fs e' gia'
    // registrato (lookup singolo, mai attesa) si rilega il volume, senno'
    // si resta RAM-only (primo boot: Fs non esiste ancora, niente hang).
    let mut st = State::new();
    if libr::service_lookup(libr::Service::Cardo).is_ok() {
        st.want_volume = true;
    }

    loop {
        match libr::recv() {
            Ok(m) if m.tag == LOG_REG => {
                let h = handle_reg(&mut st, m.channel, m.w0, m.w1);
                let _ = libr::reply(LOG_REG, h, 0);
            }
            Ok(m) if m.tag == LOG_APPEND => {
                let (w0, w1) = handle_append(&mut st, m.channel, m.w0);
                let _ = libr::reply(LOG_APPEND, w0, w1);
            }
            Ok(m) if m.tag == LOG_READ => {
                let (w0, w1) = handle_read(&mut st, m.channel, m.w0);
                let _ = libr::reply(LOG_READ, w0, w1);
            }
            Ok(m) if m.tag == LOG_FLUSH => {
                let r = handle_flush(&mut st, m.channel);
                let _ = libr::reply(LOG_FLUSH, r, 0);
            }
            Ok(m) if m.tag == LOG_SEAL => {
                match libr::snap_create(libr::log::LOG_BUCKET) {
                    Ok(id) => {
                        st.last_seal = id;
                        let _ = libr::reply(LOG_SEAL, id, 0);
                    }
                    Err(_) => {
                        let _ = libr::reply(LOG_SEAL, libr::ERR, 0);
                    }
                }
            }
            Ok(m) if m.tag == LOG_STATS => {
                let mut frame = Vec::with_capacity(16);
                frame.extend_from_slice(&(st.durable as u64).to_le_bytes());
                frame.extend_from_slice(&st.last_seal.to_le_bytes());
                let mut ok = false;
                if let Some(r) = st.regs.iter().find(|r| r.chan == m.channel) {
                    if libr::map_physical(r.resp_phys, libr::CLI_RESP_VA, 1).is_ok() {
                        unsafe { libr::resp_frame_write(libr::CLI_RESP_VA, &frame) };
                        ok = true;
                    }
                }
                if ok {
                    let _ = libr::reply(LOG_STATS, st.appended, st.evicted);
                } else {
                    let _ = libr::reply(LOG_STATS, libr::ERR_NOHANDSHAKE, 0);
                }
            }
            Ok(m) if m.tag == EXIT_NOTIFY => {
                // Morte di un client: purge della sua registrazione (il client
                // rifara' REG al prossimo uso). Niente reply (notify kernel).
                let before = st.regs.len();
                st.regs.retain(|r| r.chan != m.channel);
                if st.regs.len() != before {
                    println!("[userlog] purge reg chan={}", m.channel);
                }
            }
            Ok(_) => {
                // Tag ignoto su `send` sincrona: errore invece di appendere il
                // mittente (mai hang silenziosi, stampo Time).
                let _ = libr::reply(LOG_APPEND, libr::ERR, 0);
            }
            Err(_) => {}
        }
        // Riversamento incrementale post-FLUSH (4/giro): il disco non e' mai
        // nel percorso degli APPEND, ma il pregresso si persiste da solo.
        drain_flush(&mut st);
    }
}
