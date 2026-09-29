//! usertestsarca — test ArcaFS P5+A1+56.1+56.2a (Fase 54/55/56).
//!
//! Assert (con i drive ArcaFS presenti; il core senza, run manuale):
//!   1. vettori BLAKE2s (vuoto/abc/lungo, valori noti)
//!   2. ramfs: hash BLAKE2s via `R_GET_HASH` == ricalcolo indipendente
//!   3. tamper: contenuto diverso -> hash diverso
//!   4. object store nativo R_OBJ_PUT/GET: round-trip piccolo (Fase 55, A1)
//!   5. chunking: blob > RING_MAX_PAYLOAD -> GET multi-round-trip
//!   6. chiave assente -> errore (mai dati inventati)
//!   7. un disco o partizione espone il superblock ACFS (scan per magic, mai
//!      per lettera — Fase 55, Parte 4: esteso a sda1..sda4)
//!   8. mount `/arca` del volume ArcaFS riesce
//!   9. `open` sul mount rifiutato (stub: mai dati inventati)
//!  10. `readdir` sul mount rifiutato
//!  11. umount `/arca` riesce (cleanup)
//!  12. un disco GPT (protective-MBR 0xEE a byte 450) espone ACFS in
//!      partizione (parse GPT guest: header+entry UEFI reali)
//!  13. mount/umount del volume GPT con `open` rifiutato
//!  14. versioni: PUT ripetute = catena, GET = latest (56.1)
//!  15. snapshot del bucket (56.1)
//!  16. rollback ripristina la versione pinnata come nuova head (56.1)
//!  17. snapshot delete non tocca il live (56.1)
//!  18. retention: oltre 8 versioni si trimma la coda (56.1)
//!  19. delete oggetto: GET/STAT rifiutati (56.1)
//!  20. clone di bucket: count + dati (56.1)
//!  21. STAT ritorna id + GET_ID/STAT_ID round-trip (56.1)
//!  22. volume on-disk: open + open assente rifiutata (56.2a)
//!  23. alloc due blocchi distinti mai-zero (56.2a)
//!  24. write/read round-trip 3560 B con checksum (56.2a)
//!  25. stat volume: high_water/live coerenti (56.2a)
//!  26. free + realloc LIFO dallo stesso blocco (56.2a)
//!  27. double-free, free(0), free ignoto, read(0) rifiutati (56.2a)
//! Con `ARCA_IMG=1` (gate) i drive ci sono sempre; senza, il core (1-6)
//! resta PASS — n/n adattivo, mai FAIL per drive assente.

#![no_std]
#![no_main]

extern crate alloc;

use libr;
use libr::println;

/// Conta pass/total e stampa la riga di gate `[testsarca] PASS n/n`.
struct Checks {
    pass: u32,
    total: u32,
}

impl Checks {
    fn ok(&mut self, name: &str, cond: bool) {
        self.total += 1;
        if cond {
            self.pass += 1;
            println!("[testsarca] {}: PASS", name);
        } else {
            println!("[testsarca] {}: FAIL", name);
        }
    }
}

/// Verifica completa del superblock ArcaFS (magic + versione + block-size +
/// checksum FNV-1a self-verifying): un BPB FAT non puo' collidere.
fn is_arca_super(sec: &[u8; 512]) -> bool {
    let sb = &sec[..libr::ARCA_SUPER_LEN];
    if sb[libr::ARCA_OFF_MAGIC..libr::ARCA_OFF_MAGIC + 4] != *libr::ARCA_MAGIC {
        return false;
    }
    let u32le = |o: usize| u32::from_le_bytes([sb[o], sb[o + 1], sb[o + 2], sb[o + 3]]);
    let u64le = |o: usize| {
        u64::from_le_bytes([
            sb[o], sb[o + 1], sb[o + 2], sb[o + 3], sb[o + 4], sb[o + 5], sb[o + 6], sb[o + 7],
        ])
    };
    u32le(libr::ARCA_OFF_VERSION) == libr::ARCA_VERSION
        && u32le(libr::ARCA_OFF_BLOCK_SIZE) == libr::ARCA_BLOCK_SIZE
        && libr::image_hash(&sb[..libr::ARCA_OFF_CHECK]) == u64le(libr::ARCA_OFF_CHECK)
}

/// Legge 512 B da un path /dev (open/read/close) o None.
fn read_sector(path: &str) -> Option<[u8; 512]> {
    let Ok(fd) = libr::open(path, 0) else { return None };
    let mut sec = [0u8; 512];
    let n = libr::read_fs(fd, &mut sec, 512);
    let _ = libr::close(fd);
    if n != Ok(512) {
        return None;
    }
    Some(sec)
}

/// Cerca un disco o partizione il cui LBA0 e' un superblock ArcaFS valido.
/// Ritorna il nome breve (es. "sdc" o "sda1") in un buffer, o None.
/// Scan per magic (lettera-agnostico).
fn find_arca() -> Option<[u8; 4]> {
    // Fino a 8 dischi + 4 partizioni per disco (bound difensivo).
    for i in 0..8u64 {
        let letter = b'a' + i as u8;
        if letter > b'z' { break; }

        // Whole-disk: sda, sdb, ...
        let mut name = [b's', b'd', letter, 0];
        let mut pbuf = [0u8; 16];
        pbuf[..5].copy_from_slice(b"/dev/");
        pbuf[5..8].copy_from_slice(&name[..3]);
        if let Some(sec) = read_sector(core::str::from_utf8(&pbuf[..8]).unwrap_or("")) {
            if is_arca_super(&sec) {
                return Some([b's', b'd', letter, 0]);
            }
        }

        // Partizioni: sda1..sda4, sdb1..sdb4, ...
        for p in 1..=4u8 {
            name = [b's', b'd', letter, b'0' + p];
            pbuf[..5].copy_from_slice(b"/dev/");
            pbuf[5..9].copy_from_slice(&name);
            if let Some(sec) = read_sector(core::str::from_utf8(&pbuf[..9]).unwrap_or("")) {
                if is_arca_super(&sec) {
                    return Some(name);
                }
            }
        }
    }
    None
}

/// Cerca un disco GPT (protective-MBR: tipo prima voce 0xEE a byte 450, NON
/// 446 che e' il boot flag) la cui partizione espone ACFS. Ritorna
/// (disco, partizione) o None. Prova il parse GPT guest end-to-end:
/// protective → header UEFI → entry → superblock partition-relative.
fn find_gpt_arca() -> Option<([u8; 4], [u8; 4])> {
    for i in 0..8u64 {
        let letter = b'a' + i as u8;
        if letter > b'z' { break; }
        let disk = [b's', b'd', letter, 0];
        let mut pbuf = [0u8; 16];
        pbuf[..5].copy_from_slice(b"/dev/");
        pbuf[5..8].copy_from_slice(&disk[..3]);
        let sec0 = match read_sector(core::str::from_utf8(&pbuf[..8]).unwrap_or("")) {
            Some(s) => s,
            None => continue,
        };
        if sec0[450] != 0xEE {
            continue;
        }
        for p in 1..=4u8 {
            let part = [b's', b'd', letter, b'0' + p];
            pbuf[..5].copy_from_slice(b"/dev/");
            pbuf[5..9].copy_from_slice(&part);
            if let Some(sec) = read_sector(core::str::from_utf8(&pbuf[..9]).unwrap_or("")) {
                if is_arca_super(&sec) {
                    return Some((disk, part));
                }
            }
        }
    }
    None
}

/// `R_GET_HASH` di un path (32 B) o None.
fn get_hash(path: &str) -> Option<[u8; 32]> {
    let mut h = [0u8; 32];
    match libr::get_hash(path, &mut h) {
        Ok(()) => Some(h),
        Err(_) => None,
    }
}

libr::entry!(real_main);
fn real_main(_sp: u64) -> ! {
    println!("[testsarca] starting, pid={}", libr::getpid());
    let mut c = Checks { pass: 0, total: 0 };

    // 1. Vettori BLAKE2s (valori generati da due implementazioni indipendenti
    // — Python hashlib + OpenSSL, coincidenti: vedi `blake2s` crate).
    let v_empty = blake2s::blake2s(b"");
    let v_abc = blake2s::blake2s(b"abc");
    let long = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
    let v_long = blake2s::blake2s(long);
    let want_empty = hex32("69217a3079908094e11121d042354a7c1f55b6482ca1a51e1b250dfd1ed0eef9");
    let want_abc = hex32("508c5e8c327c14e2e1a72ba34eeb452f37458b209ed63a294d999b4c86675982");
    let want_long = hex32("6f4df5116a6f332edab1d9e10ee87df6557beab6259d7663f3bcd5722c13f189");
    c.ok(
        "blake2s vectors",
        v_empty == want_empty && v_abc == want_abc && v_long == want_long,
    );

    // 2. ramfs: content_hash via R_GET_HASH == ricalcolo indipendente.
    let payload = b"velordor-arcafs-p5-content";
    let mut rt_ok = false;
    if let Ok(fd) = libr::open("/sarca.txt", libr::O_CREAT) {
        let w = libr::write_fs(fd, payload, payload.len());
        let _ = libr::close(fd);
        if w == Ok(payload.len()) {
            let expect = blake2s::blake2s(payload);
            rt_ok = get_hash("/sarca.txt") == Some(expect);
        }
    }
    c.ok("ramfs content_hash", rt_ok);

    // 3. tamper: contenuto diverso -> hash diverso (e non quello vecchio).
    let mut tamper_ok = false;
    if let Ok(fd) = libr::open("/sarca.txt", libr::O_TRUNC) {
        let other = b"velordor-arcafs-p5-TAMPERED";
        let w = libr::write_fs(fd, other, other.len());
        let _ = libr::close(fd);
        if w == Ok(other.len()) {
            let h = get_hash("/sarca.txt");
            tamper_ok = h == Some(blake2s::blake2s(other))
                && h != Some(blake2s::blake2s(payload));
        }
    }
    c.ok("tamper hash cambia", tamper_ok);
    let _ = libr::remove("/sarca.txt");

    // 4. Object store nativo R_OBJ_PUT/GET (Fase 55, A1): round-trip piccolo.
    let small = b"nativo-arcafs-obj";
    let put_ok = libr::obj_put(b"test", b"k1", small) == Ok(small.len() as u64);
    c.ok(
        "obj round-trip piccolo",
        put_ok && matches!(libr::obj_get(b"test", b"k1"), Ok(v) if v == small),
    );

    // 5. Chunking: blob > RING_MAX_PAYLOAD (4000B) → GET multi-round-trip.
    let big: alloc::vec::Vec<u8> = (0..10000u32).map(|i| (i % 251) as u8).collect();
    let put_big = libr::obj_put(b"test", b"big", &big) == Ok(big.len() as u64);
    c.ok(
        "obj chunking 10000B",
        put_big && matches!(libr::obj_get(b"test", b"big"), Ok(v) if v == big),
    );

    // 6. Chiave assente → errore (mai dati inventati).
    c.ok("obj assente -> errore", libr::obj_get(b"test", b"nope").is_err());

    // 4-8. Volume ArcaFS (solo se un disco/partizione espone superblock ACFS).
    match find_arca() {
        Some(name) => {
            c.ok("volume ACFS trovato", true);
            let len = if name[3] == 0 { 3 } else { 4 };
            let dev = core::str::from_utf8(&name[..len]).unwrap_or("sdc");
            let mut src = [0u8; 16];
            src[..5].copy_from_slice(b"/dev/");
            src[5..5 + len].copy_from_slice(&name[..len]);
            let src = core::str::from_utf8(&src[..5 + len]).unwrap_or("/dev/sdc");

            let mounted = libr::mount(src, "/arca").is_ok();
            c.ok("mount /arca", mounted);
            if mounted {
                let opened = libr::open("/arca/anything", 0);
                c.ok("open stub rifiutato", opened.is_err());
                let mut buf = [0u8; 64];
                let rd = libr::readdir("/arca", &mut buf, 64);
                c.ok("readdir stub rifiutato", rd.is_err());
                c.ok("umount /arca", libr::umount("/arca").is_ok());
            } else {
                println!("[testsarca] mount {} FAILED (stub?)", dev);
                c.ok("open stub rifiutato", false);
                c.ok("readdir stub rifiutato", false);
                c.ok("umount /arca", false);
            }
        }
        None => {
            // Run manuale senza ARCA_IMG: il core passa comunque (mai FAIL
            // per drive assente); la scansione e' la riga 4 del gate.
            println!("[testsarca] nessun volume ACFS (ARCA_IMG=0?): salto 4-8");
        }
    }

    // 12-13. Volume GPT in partizione (solo se presente: con ARCA_IMG=1 c'e'
    // sempre; senza, salto senza FAIL come sopra).
    match find_gpt_arca() {
        Some((disk, part)) => {
            let dlen = if disk[3] == 0 { 3 } else { 4 };
            let plen = if part[3] == 0 { 3 } else { 4 };
            println!(
                "[testsarca] GPT: {} protective + {} con ACFS",
                core::str::from_utf8(&disk[..dlen]).unwrap_or("?"),
                core::str::from_utf8(&part[..plen]).unwrap_or("?"),
            );
            c.ok("volume ACFS in partizione GPT", true);
            let mut src = [0u8; 16];
            src[..5].copy_from_slice(b"/dev/");
            src[5..5 + plen].copy_from_slice(&part[..plen]);
            let src = core::str::from_utf8(&src[..5 + plen]).unwrap_or("/dev/sdd1");
            // Round-trip mount in un solo assert: mount ok + open rifiutato
            // (stub) + umount ok. Dettaglio nel log a fallimento parziale.
            let gpt_ok = match libr::mount(src, "/arca") {
                Ok(()) => {
                    let open_ok = libr::open("/arca/anything", 0).is_err();
                    let umount_ok = libr::umount("/arca").is_ok();
                    if !open_ok {
                        println!("[testsarca] GPT: open su stub riuscita?!");
                    }
                    if !umount_ok {
                        println!("[testsarca] GPT: umount fallito");
                    }
                    open_ok && umount_ok
                }
                Err(_) => {
                    println!("[testsarca] GPT: mount {} fallito", src);
                    false
                }
            };
            c.ok("mount/umount GPT + open rifiutato", gpt_ok);
        }
        None => {
            println!("[testsarca] nessun volume GPT (ARCA_IMG=0?): salto 12-13");
        }
    }

    // 14-21. Versioni + snapshot (56.1, bucket dedicato: niente interferenze
    // con sys/test usati altrove).
    {
        let (b, k1) = (&b"v56"[..], &b"k1"[..]);
        let a = b"versione-A";
        let bb = b"versione-B";
        // 14. PUT ripetute = catena, GET = latest.
        let v14 = libr::obj_put(b, k1, a) == Ok(a.len() as u64)
            && libr::obj_put(b, k1, bb) == Ok(bb.len() as u64)
            && matches!(libr::obj_get(b, k1), Ok(v) if v == bb)
            && matches!(libr::obj_stat(b, k1), Ok((_, sz, nv, _)) if sz == bb.len() as u64 && nv == 2);
        c.ok("versioni: catena + latest", v14);
        // 15-16. Snapshot + rollback (la pinnata diventa NUOVA head: A,B,C,B').
        let cc = b"versione-C";
        let s = libr::snap_create(b).ok();
        c.ok("snap create", s.is_some());
        let v16 = match s {
            Some(sid) => {
                libr::obj_put(b, k1, cc) == Ok(cc.len() as u64)
                    && libr::snap_rollback(b, k1, sid).is_ok()
                    && matches!(libr::obj_get(b, k1), Ok(v) if v == bb)
                    && matches!(libr::obj_stat(b, k1), Ok((_, _, nv, _)) if nv == 4)
            }
            None => false,
        };
        c.ok("rollback ripristina pinnata", v16);
        // 17. Delete dello snapshot: live intatto.
        let v17 = match s {
            Some(sid) => {
                libr::snap_delete(sid).is_ok()
                    && matches!(libr::obj_stat(b, k1), Ok((_, _, nv, _)) if nv == 4)
                    && libr::snap_delete(sid).is_err()
            }
            None => false,
        };
        c.ok("snap delete non tocca live", v17);
        // 18. Retention: 10 PUT oltre le 4 versioni → trim a 8, latest ok.
        let mut v18 = true;
        for i in 0..10u8 {
            let d = [b'D', b'0' + i];
            if libr::obj_put(b, k1, &d) != Ok(2) {
                v18 = false;
            }
        }
        v18 = v18
            && matches!(libr::obj_get(b, k1), Ok(v) if v == [b'D', b'9'])
            && matches!(libr::obj_stat(b, k1), Ok((_, sz, nv, _)) if sz == 2 && nv == 8);
        c.ok("retention trim a 8", v18);
        // 19. Delete oggetto: GET/STAT rifiutati.
        let v19 = libr::obj_delete(b, k1).is_ok()
            && libr::obj_get(b, k1).is_err()
            && libr::obj_stat(b, k1).is_err()
            && libr::obj_delete(b, k1).is_err();
        c.ok("delete oggetto", v19);
        // 20. Clone di bucket: count + dati.
        let (cb, ka, kb) = (&b"csrc"[..], &b"a"[..], &b"b"[..]);
        let v20 = libr::obj_put(cb, ka, b"uno") == Ok(3)
            && libr::obj_put(cb, kb, b"due!") == Ok(4)
            && match libr::snap_create(cb) {
                Ok(sid) => {
                    libr::snap_clone(sid, b"cdst") == Ok(2)
                        && matches!(libr::obj_get(b"cdst", ka), Ok(v) if v == b"uno")
                        && matches!(libr::obj_get(b"cdst", kb), Ok(v) if v == b"due!")
                        && libr::snap_delete(sid).is_ok()
                }
                Err(_) => false,
            };
        c.ok("clone bucket", v20);
        // 21. STAT ritorna id + GET_ID/STAT_ID round-trip.
        let v21 = match libr::obj_stat(b"cdst", ka) {
            Ok((id, sz, nv, _)) if id > 0 && sz == 3 && nv == 1 => {
                matches!(libr::obj_get_id(id), Ok(v) if v == b"uno")
                    && matches!(libr::obj_stat_id(id), Ok((s2, n2, _)) if s2 == 3 && n2 == 1)
                    && libr::obj_get_id(id + 1000000).is_err()
            }
            _ => false,
        };
        c.ok("stat id + get_id/stat_id", v21);
    }

    // 22-27. Volume on-disk debug (56.2a, partizione `/dev/sdc1`
    // formattata da `arca create`: header-ext + root vuota; immagini
    // rigenerate a ogni run, quindi allocazioni deterministiche).
    {
        // 22. open + open assente rifiutata.
        let v22 = libr::arca_open("/dev/sdc1").is_ok() && libr::arca_open("/dev/sdZ").is_err();
        c.ok("vol open + assente", v22);
        // 23. alloc due blocchi distinti mai-zero.
        let b1 = libr::arca_alloc().ok();
        let b2 = libr::arca_alloc().ok();
        let v23 = match (b1, b2) {
            (Some(a), Some(b)) => a >= 1 && b >= 1 && a != b,
            _ => false,
        };
        c.ok("alloc distinti mai-zero", v23);
        // 24. write/read round-trip 3560 B (checksum verificata dal server).
        let mut pat = [0u8; libr::ARCA_NODE_PAYLOAD_LEN];
        for (i, b) in pat.iter_mut().enumerate() {
            *b = ((i * 7) % 251) as u8;
        }
        let v24 = match (b1, b2) {
            (Some(a), Some(b)) => {
                libr::arca_write_node(a, &pat).is_ok()
                    && matches!(libr::arca_read_node(a), Ok(v) if v == pat)
                    && libr::arca_write_node(b, &pat).is_ok()
                    && matches!(libr::arca_read_node(b), Ok(v) if v == pat)
            }
            _ => false,
        };
        c.ok("write/read nodi", v24);
        // 25. stat: high_water avanzata di 2 (format parte da 2), 2 live.
        let v25 = matches!(libr::arca_stat_vol(), Ok((high, live, _)) if high == 4 && live == 2);
        c.ok("stat volume", v25);
        // 26. free + realloc LIFO dallo stesso blocco.
        let v26 = match b1 {
            Some(a) => {
                libr::arca_free(a).is_ok()
                    && matches!(libr::arca_alloc(), Ok(b) if b == a)
                    && matches!(libr::arca_stat_vol(), Ok((_, live, _)) if live == 2)
            }
            _ => false,
        };
        c.ok("free + realloc LIFO", v26);
        // 27. rifiuti: double-free, free(0), free ignoto, read(0).
        let v27 = match b1 {
            Some(a) => {
                libr::arca_free(a).is_ok()
                    && libr::arca_free(a).is_err()
                    && libr::arca_free(0).is_err()
                    && libr::arca_free(99999).is_err()
                    && libr::arca_read_node(0).is_err()
            }
            _ => false,
        };
        c.ok("rifiuti allocatore", v27);
    }

    if c.pass == c.total {
        println!("[testsarca] PASS {}/{}", c.pass, c.total);
    } else {
        println!("[testsarca] FAIL {}/{}", c.pass, c.total);
    }
    println!("[testsarca] all tests done");
    let _ = libr::send(libr::CHANNEL_PARENT, libr::TEST_DONE, 0, 0);
    libr::exit(if c.pass == c.total { 0 } else { 1 });
}

/// Decodifica 64 hex in `[u8; 32]` (const in test).
fn hex32(s: &str) -> [u8; 32] {
    let b = s.as_bytes();
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        let hi = hexval(b[i * 2]);
        let lo = hexval(b[i * 2 + 1]);
        out[i] = (hi << 4) | lo;
        i += 1;
    }
    out
}

fn hexval(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => 0,
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    if let Some(loc) = info.location() {
        println!("[testsarca] panic @ {}:{}", loc.file(), loc.line());
    } else {
        println!("[testsarca] panic");
    }
    libr::exit(1)
}
