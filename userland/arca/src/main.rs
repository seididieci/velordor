//! arca — tool guest ArcaFS (Fase 54, P5: `list` + `stat`).
//!
//! Lancio dalla shell (`run /fat/bin/arca.bin list`). `list` stampa la
//! topologia dei dischi (via `libr::disk_list/disk_info`, relay R_DISK_*);
//! `stat <nodo>` legge il LBA0 del nodo (`/dev/<nodo>`) e riconosce il
//! superblock ArcaFS (magic + versione + block-size + checksum FNV — stessi
//! offset del tool host e di cardo, single source in `syscall-numbers`).
//!
//! Il mount nativo e `get`/`put`/`rm` arrivano in A1/A7; qui solo lettura e
//! diagnostica. Output su seriale (come gli altri programmi da disco).

#![no_std]
#![no_main]

use libr;
use libr::println;

/// Costruisce "/dev/<dev>" in un buffer stack (no alloc). `None` se non ci
/// sta (nomi nodo cortissimi per costruzione).
fn dev_path<'a>(dev: &str, buf: &'a mut [u8; 32]) -> Option<&'a str> {
    let prefix = b"/dev/";
    if dev.is_empty() || prefix.len() + dev.len() > buf.len() {
        return None;
    }
    buf[..prefix.len()].copy_from_slice(prefix);
    buf[prefix.len()..prefix.len() + dev.len()].copy_from_slice(dev.as_bytes());
    core::str::from_utf8(&buf[..prefix.len() + dev.len()]).ok()
}

/// Legge il LBA0 del nodo `dev` (`/dev/<dev>`) e, se e' un superblock
/// ArcaFS valido, ritorna `(uuid, generation)`.
fn read_super(dev: &str) -> Option<(u64, u64)> {
    let mut pbuf = [0u8; 32];
    let path = dev_path(dev, &mut pbuf)?;
    let fd = libr::open(path, 0).ok()?;
    let mut sec = [0u8; 512];
    let n = libr::read_fs(fd, &mut sec, 512);
    let _ = libr::close(fd);
    if n != Ok(512) {
        return None;
    }
    let sb = &sec[..libr::ARCA_SUPER_LEN];
    if sb[libr::ARCA_OFF_MAGIC..libr::ARCA_OFF_MAGIC + 4] != *libr::ARCA_MAGIC {
        return None;
    }
    let u32le = |o: usize| u32::from_le_bytes([sb[o], sb[o + 1], sb[o + 2], sb[o + 3]]);
    let u64le = |o: usize| {
        u64::from_le_bytes([
            sb[o], sb[o + 1], sb[o + 2], sb[o + 3], sb[o + 4], sb[o + 5], sb[o + 6], sb[o + 7],
        ])
    };
    if u32le(libr::ARCA_OFF_VERSION) != libr::ARCA_VERSION {
        return None;
    }
    if u32le(libr::ARCA_OFF_BLOCK_SIZE) != libr::ARCA_BLOCK_SIZE {
        return None;
    }
    if libr::image_hash(&sb[..libr::ARCA_OFF_CHECK]) != u64le(libr::ARCA_OFF_CHECK) {
        return None;
    }
    Some((u64le(libr::ARCA_OFF_UUID), u64le(libr::ARCA_OFF_GEN)))
}

libr::entry!(real_main);
fn real_main(sp: u64) -> ! {
    // Gli `&str` degli argv prendono in prestito `args`: il match vive nello
    // stesso scope (args resta vivo fino alla fine del blocco).
    let args = libr::args_from_stack(sp);
    let cmd = args
        .as_ref()
        .and_then(|a| a.get(1))
        .and_then(|b| core::str::from_utf8(b).ok())
        .unwrap_or("");
    let arg = args
        .as_ref()
        .and_then(|a| a.get(2))
        .and_then(|b| core::str::from_utf8(b).ok())
        .unwrap_or("");
    match cmd {
        "list" => cmd_list(),
        "stat" => cmd_stat(arg),
        "" => {
            println!("arca: uso: arca list | arca stat <nodo>");
            libr::exit(1);
        }
        other => {
            println!("arca: comando ignoto: {}", other);
            libr::exit(1);
        }
    }
    libr::exit(0);
}

fn cmd_list() {
    match libr::disk_list() {
        Ok(disks) => {
            println!("[arca] {} dischi", disks.len());
            for (i, (sectors, flags)) in disks.iter().enumerate() {
                let name = (b'a' + i as u8) as char;
                println!(
                    "[arca] sd{}: settori={} lba48={} trim={} udma={} ssd={}",
                    name,
                    sectors,
                    if flags & 1 != 0 { "si" } else { "no" },
                    if flags & 2 != 0 { "si" } else { "no" },
                    match (flags >> 8) & 0xF {
                        0xF => "PIO",
                        _ => "UDMA",
                    },
                    if (flags >> 16) & 0xFFFF == 1 { "si" } else { "no" },
                );
            }
        }
        Err(_) => println!("[arca] disk_list FAILED"),
    }
}

fn cmd_stat(node: &str) {
    if node.is_empty() {
        println!("arca: stat: manca il nodo (es. sda)");
        return;
    }
    let mut pbuf = [0u8; 32];
    let shown = dev_path(node, &mut pbuf).unwrap_or("/dev/<nodo>");
    match read_super(node) {
        Some((uuid, generation)) => println!(
            "[arca] {}: ArcaFS uuid={:016X} generation={} block_size={}",
            shown,
            uuid,
            generation,
            libr::ARCA_BLOCK_SIZE
        ),
        None => println!("[arca] {}: non e' un volume ArcaFS", shown),
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    println!("[arca] panic");
    libr::exit(1)
}
