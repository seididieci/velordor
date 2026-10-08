//! userbench — micro-benchmark throughput client→block (Fase 23).
//!
//! Misura il percorso dati END-TO-END con il TSC (calibrato sul PIT), senza
//! cache che nascondano il collo di bottiglia (non ne esistono ancora: ogni
//! op attraversa IPC + cardo + block + PIO). Piattaforma di riferimento:
//! KVM (`scripts/bench.sh`, N run con media: i tempi TCG non sono reali).
//!
//! Op misurate (costo crescente del percorso):
//!   b1 zero_1B .............. read 1 B da /dev/zero (solo IPC, niente disco)
//!   b2 sda_512B_seq ......... 200 read sequenziali da /dev/sda (IPC + PIO)
//!   b3 fat_small_orc ........ open+read+close di /fat/HELLO.TXT (find + IPC + PIO)
//!   b4 ramfs_4K_write/read .. write/read 4 KiB su ramfs (FS+IPC, niente disco)
//!   b5 fat_4K_oow ........... open+overwrite+close 4 KiB su /fat (write+FLUSH)
//!   b6 bulk (Fase 53, P4 misura bulk): sweep round-trip-vs-dimensione
//!     4K/16K/64K — ramfs R/W singole (hot=cold in RAM) + FAT R/W hot/cold
//!     (cold = file distinti + spoiler unico: ogni iter dati mai visti).
//!     Metodologia uniforme open/op/close per iter a offset 0 (steady state,
//!     niente crescita file: b4/b5 restano gli anchor storici grow-walk/oow).
//!
//! Outputegrepabile: righe `[bench] <nome> iters=<n> cyc_op=<c> kb_s=<k>`
//! (c = cicli TSC per op, k = KiB/s). Fine: TEST_DONE a init + exit.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use civis;
use civis::{println, OPEN_CREATE, OPEN_TRUNC};

/// Spoiler cache (Fase 53, P4): 300 settori sequenziali da /dev/sda letti e
/// scartati — oltre le 256 entry CLOCK di block: tutto l'evicted (dati +
/// metadati del file sotto misura). Freddo deterministico senza restart,
/// senza protocollo, senza pagine extra. ~0.4 s a chiamata (KVM).
fn spoil_cache() -> bool {
    let Ok(fd) = civis::open("/dev/sda", 0) else {
        return false;
    };
    let mut sec = [0u8; 512];
    let mut ok = true;
    // Letture sequenziali: la posizione per-fd avanza da sola (DEV relay).
    for _ in 0..300 {
        if civis::read_fs(fd, &mut sec, 512) != Ok(512) {
            ok = false;
            break;
        }
    }
    let _ = civis::close(fd);
    ok
}

/// Crea `path` con `size` byte di pattern (una write, chunking civis).
/// `OPEN_CREATE|OPEN_TRUNC`: idempotente tra righe che riusano lo stesso path con
/// size diverse (su FAT il remove non esiste: b5 lascia il file, qui lo si
/// re-tronca; l'immagine e' rigenerata a ogni run.sh comunque).
fn bulk_create(path: &str, size: usize, seed: u8) -> bool {
    let Ok(fd) = civis::open(path, OPEN_CREATE | civis::OPEN_TRUNC) else {
        return false;
    };
    let buf: Vec<u8> = (0..size).map(|i| ((seed as usize + i) % 251) as u8).collect();
    let n = civis::write_fs(fd, &buf, size);
    let _ = civis::close(fd);
    n == Ok(size)
}

/// Bulk ramfs (Fase 53): open/op/close per iter a offset 0 su /BULK.TMP
/// pre-cresciuto (steady state, niente realloc in misura). Una sola serie:
/// hot=cold in RAM (niente DISK, niente cache).
fn bulk_ramfs(wname: &str, rname: &str, size: usize, iters: u64, hz: u64) -> bool {
    if !bulk_create("/BULK.TMP", size, 0xA5) {
        println!("[bench] {}: FAIL (create)", wname);
        return false;
    }
    let wbuf: Vec<u8> = (0..size).map(|i| ((0xA5usize + i) % 251) as u8).collect();
    let mut ok = run(wname, iters, size as u64, hz, || {
        let Ok(fd) = civis::open("/BULK.TMP", 0) else {
            return false;
        };
        let n = civis::write_fs(fd, &wbuf, size);
        let _ = civis::close(fd);
        n == Ok(size)
    });
    if ok {
        let mut rbuf: Vec<u8> = alloc::vec![0u8; size];
        ok &= run(rname, iters, size as u64, hz, || {
            let Ok(fd) = civis::open("/BULK.TMP", 0) else {
                return false;
            };
            let n = civis::read_fs(fd, &mut rbuf, size);
            let _ = civis::close(fd);
            n == Ok(size)
        });
    }
    if civis::remove("/BULK.TMP").is_err() {
        println!("[bench] {}: FAIL (cleanup)", wname);
        ok = false;
    }
    ok
}

/// Bulk FAT overwrite hot (Fase 53, b5-style): /fat/BULK.TMP pre-cresciuto,
/// open+write+close per iter (warmup interno a run() scalda metadati+dati).
fn bulk_fat_write_hot(name: &str, size: usize, iters: u64, hz: u64) -> bool {
    if !bulk_create("/fat/BULK.TMP", size, 0x5A) {
        println!("[bench] {}: FAIL (create)", name);
        return false;
    }
    let wbuf: Vec<u8> = (0..size).map(|i| ((0x5Ausize + i) % 251) as u8).collect();
    let ok = run(name, iters, size as u64, hz, || {
        let Ok(fd) = civis::open("/fat/BULK.TMP", 0) else {
            return false;
        };
        let n = civis::write_fs(fd, &wbuf, size);
        let _ = civis::close(fd);
        n == Ok(size)
    });
    // Su FAT il remove non esiste (come b5): il file resta e la prossima riga
    // lo re-tronca via bulk_create; l'immagine e' rigenerata a ogni run.sh.
    ok
}

/// Nome 8.3 per i file cold (`tag` 5 char + 2 cifre: max 99 file).
/// `out` = "/fat/" (5) + tag (5) + 2 cifre + ".TMP" (4) = 16 byte esatti.
fn cold_path<'a>(tag: &str, i: u64, out: &'a mut [u8; 16]) -> &'a str {
    out[..5].copy_from_slice(b"/fat/");
    out[5..10].copy_from_slice(&tag.as_bytes()[..5]);
    out[10] = b'0' + ((i / 10) % 10) as u8;
    out[11] = b'0' + (i % 10) as u8;
    out[12..16].copy_from_slice(b".TMP");
    core::str::from_utf8(&out[..]).unwrap_or("/fat/BAD.TMP")
}

/// Bulk FAT overwrite cold (Fase 53): `iters` file distinti pre-cresciuti +
/// UN solo spoiler prima della misura — ogni iter tocca settori mai visti
/// (dati + catene FAT freddi; la dir padre va hot dopo iter 1, caveat
/// documentato). Niente spoiler nel path misurato: numeri op puri.
/// Le 5 warmup di run() consumano i primi 5 file, le misurate i successivi:
/// se ne creano `iters + 5`. Su FAT niente remove (come b5): i file restano
/// (nomi per riga distinti via tag), immagine rigenerata a ogni run.sh.
fn bulk_fat_write_cold(name: &str, size: usize, iters: u64, hz: u64) -> bool {
    let total = iters + 5;
    for i in 0..total {
        let mut p = [0u8; 16];
        if !bulk_create(cold_path("BWC4K", i, &mut p), size, 0x5A) {
            println!("[bench] {}: FAIL (create {})", name, i);
            return false;
        }
    }
    if !spoil_cache() {
        println!("[bench] {}: FAIL (spoil)", name);
        return false;
    }
    let wbuf: Vec<u8> = (0..size).map(|i| ((0x5Ausize + i) % 251) as u8).collect();
    let mut idx = 0u64;
    run(name, iters, size as u64, hz, || {
        let mut p = [0u8; 16];
        let path = cold_path("BWC4K", idx, &mut p);
        idx += 1;
        let Ok(fd) = civis::open(path, 0) else {
            return false;
        };
        let n = civis::write_fs(fd, &wbuf, size);
        let _ = civis::close(fd);
        n == Ok(size)
    })
}

/// Bulk FAT read hot (Fase 53): pre-cresciuto + pre-pass completo (tutto in
/// cache), open+read+close per iter.
fn bulk_fat_read_hot(name: &str, size: usize, iters: u64, hz: u64) -> bool {
    if !bulk_create("/fat/BULK.TMP", size, 0xA5) {
        println!("[bench] {}: FAIL (create)", name);
        return false;
    }
    // Pre-pass: scalda dati+metadati (le 5 warmup di run() da sole bastano,
    // ma il pre-pass esplicito rende hot deterministico da iter 1).
    {
        let Ok(fd) = civis::open("/fat/BULK.TMP", 0) else {
            println!("[bench] {}: FAIL (pre-pass open)", name);
            return false;
        };
        let mut tmp: Vec<u8> = alloc::vec![0u8; size];
        let n = civis::read_fs(fd, &mut tmp, size);
        let _ = civis::close(fd);
        if n != Ok(size) {
            println!("[bench] {}: FAIL (pre-pass read)", name);
            return false;
        }
    }
    let mut rbuf: Vec<u8> = alloc::vec![0u8; size];
    let ok = run(name, iters, size as u64, hz, || {
        let Ok(fd) = civis::open("/fat/BULK.TMP", 0) else {
            return false;
        };
        let n = civis::read_fs(fd, &mut rbuf, size);
        let _ = civis::close(fd);
        n == Ok(size)
    });
    // Su FAT il remove non esiste (come b5): il file resta e la prossima riga
    // lo re-tronca via bulk_create; l'immagine e' rigenerata a ogni run.sh.
    ok
}

/// Costo dello spoiler da solo (Fase 53): riferimento metodologico (prova che
/// 300 settori evictonano le 256 entry: tempo ~300 PIO; le righe cold NON lo
/// includono — usano file distinti + spoiler unico fuori misura).
fn bulk_spoil(name: &str, iters: u64, hz: u64) -> bool {
    run(name, iters, 300 * 512, hz, || spoil_cache())
}

/// Bulk FAT read cold (Fase 53): come write (file distinti + spoiler unico).
fn bulk_fat_read_cold(name: &str, size: usize, iters: u64, hz: u64) -> bool {
    let total = iters + 5;
    for i in 0..total {
        let mut p = [0u8; 16];
        if !bulk_create(cold_path("BRC4K", i, &mut p), size, 0xA5) {
            println!("[bench] {}: FAIL (create {})", name, i);
            return false;
        }
    }
    if !spoil_cache() {
        println!("[bench] {}: FAIL (spoil)", name);
        return false;
    }
    let mut rbuf: Vec<u8> = alloc::vec![0u8; size];
    let mut idx = 0u64;
    run(name, iters, size as u64, hz, || {
        let mut p = [0u8; 16];
        let path = cold_path("BRC4K", idx, &mut p);
        idx += 1;
        let Ok(fd) = civis::open(path, 0) else {
            return false;
        };
        let n = civis::read_fs(fd, &mut rbuf, size);
        let _ = civis::close(fd);
        n == Ok(size)
    })
}

/// Esegue `warm` iterazioni di riscaldamento poi `iters` misurate, stampa la
/// riga `[bench]`. `bytes` = byte utili per iter (per i KiB/s).
fn run(name: &str, iters: u64, bytes: u64, hz: u64, mut f: impl FnMut() -> bool) -> bool {
    for _ in 0..5 {
        if !f() {
            println!("[bench] {}: FAIL (warmup)", name);
            return false;
        }
    }
    let t0 = civis::rdtsc();
    let mut max_cyc = 0u64;
    for _ in 0..iters {
        let s = civis::rdtsc();
        if !f() {
            println!("[bench] {}: FAIL (iter)", name);
            return false;
        }
        let dt = civis::rdtsc().wrapping_sub(s);
        if dt > max_cyc {
            max_cyc = dt;
        }
    }
    let total = civis::rdtsc().wrapping_sub(t0);
    if total == 0 {
        println!("[bench] {}: FAIL (tsc fermo)", name);
        return false;
    }
    let cyc_op = total / iters;
    let kb_s = bytes * iters * hz / total / 1024;
    println!(
        "[bench] {} iters={} cyc_op={} max_cyc={} kb_s={}",
        name, iters, cyc_op, max_cyc, kb_s
    );
    true
}

civis::entry!(real_main);
fn real_main(_sp: u64) -> ! {
    println!("[bench] starting, pid={}", civis::getpid());
    let hz = civis::tsc_calibrate(20);
    println!("[bench] tsc_hz={}", hz);
    let mut ok = hz != 0;

    // b1: solo IPC — 1 B da /dev/zero (/dev/null da' EOF=0 per semantica
    // Unix: per misurare il round-trip IPC serve un device che risponde).
    if ok {
        match civis::open("/dev/zero", 0) {
            Err(e) => {
                println!("[bench] zero_1B: FAIL (open {:?})", e);
                ok = false;
            }
            Ok(fd) => {
                let mut one = [0u8; 1];
                ok &= run("zero_1B", 2000, 1, hz, || civis::read_fs(fd, &mut one, 1) == Ok(1));
                let _ = civis::close(fd);
            }
        }
    }

    // b2: catena completa — 200 settori sequenziali raw da /dev/sda.
    if ok {
        match civis::open("/dev/sda", 0) {
            Err(e) => {
                println!("[bench] sda_512B_seq: FAIL (open {:?})", e);
                ok = false;
            }
            Ok(fd) => {
                let mut sec = [0u8; 512];
                ok &= run("sda_512B_seq", 200, 512, hz, || civis::read_fs(fd, &mut sec, 512) == Ok(512));
                let _ = civis::close(fd);
            }
        }
    }

    // b3: file piccolo su FAT — open+read+close (find + IPC + PIO).
    if ok {
        let mut hello = [0u8; 32];
        ok &= run("fat_small_orc", 500, 25, hz, || {
            let Ok(fd) = civis::open("/fat/HELLO.TXT", 0) else {
                return false;
            };
            let n = civis::read_fs(fd, &mut hello, 25);
            let _ = civis::close(fd);
            n == Ok(25)
        });
    }

    // b4: ramfs 4 KiB — stack FS+IPC senza disco (write poi read).
    if ok {
        match civis::open("/BENCH.TMP", OPEN_CREATE) {
            Err(e) => {
                println!("[bench] ramfs_4K: FAIL (create {:?})", e);
                ok = false;
            }
            Ok(fd) => {
                let wbuf = [0xA5u8; 4096];
                ok &= run("ramfs_4K_write", 100, 4096, hz, || civis::write_fs(fd, &wbuf, 4096) == Ok(4096));
                let _ = civis::close(fd);
            }
        }
    }
    if ok {
        match civis::open("/BENCH.TMP", 0) {
            Err(e) => {
                println!("[bench] ramfs_4K_read: FAIL (open {:?})", e);
                ok = false;
            }
            Ok(fd) => {
                let mut rbuf = [0u8; 4096];
                ok &= run("ramfs_4K_read", 100, 4096, hz, || civis::read_fs(fd, &mut rbuf, 4096) == Ok(4096));
                let _ = civis::close(fd);
                if civis::remove("/BENCH.TMP").is_err() {
                    println!("[bench] ramfs cleanup: FAIL (remove)");
                    ok = false;
                }
            }
        }
    }

    // b5: overwrite 4 KiB su FAT — open+write+close (PIO + FLUSH per settore).
    // Il file resta nell'immagine (rigenerata a ogni run.sh): mai fixture altrui.
    if ok {
        match civis::open("/fat/BENCH.TMP", OPEN_CREATE) {
            Err(e) => {
                println!("[bench] fat_4K_oow: FAIL (create {:?})", e);
                ok = false;
            }
            Ok(fd) => {
                let _ = civis::close(fd);
                let wbuf = [0x5Au8; 4096];
                ok &= run("fat_4K_oow", 50, 4096, hz, || {
                    let Ok(fd) = civis::open("/fat/BENCH.TMP", 0) else {
                        return false;
                    };
                    let n = civis::write_fs(fd, &wbuf, 4096);
                    let _ = civis::close(fd);
                    n == Ok(4096)
                });
            }
        }
    }

    // b6: sweep bulk 4K/16K/64K (Fase 53, P4 misura bulk). ramfs: serie
    // singole (hot=cold in RAM); FAT: hot (cache) + cold (spoiler per iter).
    // Iter decrescenti con la size (rumore noto ±20-40% sulle brevi, §13).
    if ok {
        ok &= bulk_ramfs("bulk_ramfs_16K_write", "bulk_ramfs_16K_read", 16384, 25, hz);
    }
    if ok {
        ok &= bulk_ramfs("bulk_ramfs_64K_write", "bulk_ramfs_64K_read", 65536, 10, hz);
    }
    if ok {
        ok &= bulk_fat_write_hot("bulk_fat_4K_write_hot", 4096, 25, hz);
    }
    if ok {
        ok &= bulk_fat_read_hot("bulk_fat_4K_read_hot", 4096, 50, hz);
    }
    if ok {
        ok &= bulk_fat_write_hot("bulk_fat_16K_write_hot", 16384, 12, hz);
    }
    if ok {
        ok &= bulk_fat_read_hot("bulk_fat_16K_read_hot", 16384, 20, hz);
    }
    if ok {
        ok &= bulk_fat_write_hot("bulk_fat_64K_write_hot", 65536, 5, hz);
    }
    if ok {
        ok &= bulk_fat_read_hot("bulk_fat_64K_read_hot", 65536, 8, hz);
    }
    if ok {
        ok &= bulk_spoil("bulk_spoil_300sec", 15, hz);
    }
    if ok {
        ok &= bulk_fat_write_cold("bulk_fat_4K_write_cold", 4096, 15, hz);
    }
    if ok {
        ok &= bulk_fat_read_cold("bulk_fat_4K_read_cold", 4096, 15, hz);
    }
    if ok {
        ok &= bulk_fat_write_cold("bulk_fat_16K_write_cold", 16384, 8, hz);
    }
    if ok {
        ok &= bulk_fat_read_cold("bulk_fat_16K_read_cold", 16384, 8, hz);
    }
    if ok {
        ok &= bulk_fat_write_cold("bulk_fat_64K_write_cold", 65536, 5, hz);
    }
    if ok {
        ok &= bulk_fat_read_cold("bulk_fat_64K_read_cold", 65536, 5, hz);
    }

    if ok {
        println!("[bench] DONE ok=1");
    } else {
        println!("[bench] DONE ok=0");
    }
    let _ = civis::send(civis::CHANNEL_PARENT, civis::TEST_DONE, 0, 0); // init: bench finito
    civis::exit(if ok { 0 } else { 1 })
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    if let Some(loc) = info.location() {
        println!("[bench] panic @ {}:{}", loc.file(), loc.line());
    } else {
        println!("[bench] panic");
    }
    civis::exit(1)
}
