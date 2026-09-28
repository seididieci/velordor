//! usertestsarca — test ArcaFS P5 (Fase 54): BLAKE2s + content_hash + volume.
//!
//! Assert (8 con il drive ArcaFS presente; 3 core senza, run manuale):
//!   1. vettori BLAKE2s (vuoto/abc/lungo, valori noti)
//!   2. ramfs: hash BLAKE2s via `R_GET_HASH` == ricalcolo indipendente
//!   3. tamper: contenuto diverso -> hash diverso
//!   4. un disco espone il superblock ACFS (scan per magic, mai per lettera)
//!   5. mount `/arca` del volume ArcaFS (stub P5) riesce
//!   6. `open` sul mount stub rifiutato (mai dati inventati)
//!   7. `readdir` sul mount stub rifiutato
//!   8. umount `/arca` riesce (cleanup)
//! Con `ARCA_IMG=1` (gate) il drive c'e' sempre: 8/8. Senza, il core
//! (1-3) resta PASS — n/n adattivo, mai FAIL per drive assente.

#![no_std]
#![no_main]

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

/// Cerca un disco il cui LBA0 e' un superblock ArcaFS valido. Ritorna il nome
/// breve (es. "sdc") in un buffer, o None. Scan per magic (lettera-agnostico).
fn find_arca() -> Option<[u8; 3]> {
    // Fino a 8 dischi (bound difensivo; QEMU ne ha 3 con ARCA_IMG=1).
    for i in 0..8u64 {
        let name: [u8; 3] = [b's', b'd', b'a' + i as u8];
        let mut pbuf = [0u8; 16];
        pbuf[..5].copy_from_slice(b"/dev/");
        pbuf[5..8].copy_from_slice(&name);
        let path = core::str::from_utf8(&pbuf[..8]).unwrap_or("");
        let Ok(fd) = libr::open(path, 0) else {
            continue;
        };
        let mut sec = [0u8; 512];
        let n = libr::read_fs(fd, &mut sec, 512);
        let _ = libr::close(fd);
        if n != Ok(512) {
            continue;
        }
        let sb = &sec[..libr::ARCA_SUPER_LEN];
        if sb[libr::ARCA_OFF_MAGIC..libr::ARCA_OFF_MAGIC + 4] != *libr::ARCA_MAGIC {
            continue;
        }
        let u32le = |o: usize| u32::from_le_bytes([sb[o], sb[o + 1], sb[o + 2], sb[o + 3]]);
        let u64le = |o: usize| {
            u64::from_le_bytes([
                sb[o], sb[o + 1], sb[o + 2], sb[o + 3], sb[o + 4], sb[o + 5], sb[o + 6],
                sb[o + 7],
            ])
        };
        if u32le(libr::ARCA_OFF_VERSION) == libr::ARCA_VERSION
            && u32le(libr::ARCA_OFF_BLOCK_SIZE) == libr::ARCA_BLOCK_SIZE
            && libr::image_hash(&sb[..libr::ARCA_OFF_CHECK]) == u64le(libr::ARCA_OFF_CHECK)
        {
            return Some(name);
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

    // 4-8. Volume ArcaFS (solo se il terzo drive ArcaFS e' presente).
    match find_arca() {
        Some(name) => {
            c.ok("volume ACFS trovato", true);
            let dev = core::str::from_utf8(&name).unwrap_or("sdc");
            let mut src = [0u8; 16];
            src[..5].copy_from_slice(b"/dev/");
            src[5..8].copy_from_slice(&name);
            let src = core::str::from_utf8(&src[..8]).unwrap_or("/dev/sdc");

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
