//! `arca` host tool (Fase 54, P5): per ora solo `create` — scrive un
//! `arca.img` con superblock ArcaFS (LBA0 + shadow LBA1), resto zero.
//! Offset/magic/checksum sono la single source `syscall-numbers`: questo
//! tool non li duplica (il round-trip create→mount→stat e' il test che li
//! tiene d'accordo). Le op guest (`list`/`get`/...) vivono in
//! `userland/arca`; il tool completo arriva in A7.
//!
//! Uso: `arca create <path> [--uuid HEX] [--size-mib N]`.

use std::env;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::process::exit;

use syscall_numbers as sn;

fn usage() -> ! {
    eprintln!("uso: arca create <path> [--uuid HEX16] [--size-mib N]  (default 1 MiB)");
    exit(2);
}

/// Costruisce i 128 byte di superblock (LE esplicito, checksum FNV-1a).
fn build_super(uuid: u64, generation: u64, mountpoint: &str) -> [u8; sn::ARCA_SUPER_LEN] {
    let mut sb = [0u8; sn::ARCA_SUPER_LEN];
    sb[sn::ARCA_OFF_MAGIC..sn::ARCA_OFF_MAGIC + 4].copy_from_slice(sn::ARCA_MAGIC);
    sb[sn::ARCA_OFF_VERSION..sn::ARCA_OFF_VERSION + 4]
        .copy_from_slice(&sn::ARCA_VERSION.to_le_bytes());
    sb[sn::ARCA_OFF_BLOCK_SIZE..sn::ARCA_OFF_BLOCK_SIZE + 4]
        .copy_from_slice(&sn::ARCA_BLOCK_SIZE.to_le_bytes());
    sb[sn::ARCA_OFF_UUID..sn::ARCA_OFF_UUID + 8].copy_from_slice(&uuid.to_le_bytes());
    sb[sn::ARCA_OFF_GEN..sn::ARCA_OFF_GEN + 8].copy_from_slice(&generation.to_le_bytes());
    // root/refcount/alloc restano 0 (volume vuoto in P5).
    let mp = mountpoint.as_bytes();
    let n = mp.len().min(63);
    sb[sn::ARCA_OFF_MOUNT..sn::ARCA_OFF_MOUNT + n].copy_from_slice(&mp[..n]);
    sb[sn::ARCA_OFF_AUTO] = 0;
    sb[sn::ARCA_OFF_FLAGS] = 0;
    let checksum = sn::image_hash(&sb[..sn::ARCA_OFF_CHECK]);
    sb[sn::ARCA_OFF_CHECK..sn::ARCA_OFF_CHECK + 8].copy_from_slice(&checksum.to_le_bytes());
    sb
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 || args[1] != "create" {
        usage();
    }
    let path = &args[2];
    let mut uuid: u64 = 1;
    let mut size_mib: u64 = 1;
    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--uuid" => {
                i += 1;
                uuid = match args.get(i).and_then(|s| u64::from_str_radix(s, 16).ok()) {
                    Some(v) => v,
                    None => {
                        eprintln!("--uuid: atteso esadecimale (es. 4152434100000001)");
                        exit(2);
                    }
                };
            }
            "--size-mib" => {
                i += 1;
                size_mib = match args.get(i).and_then(|s| s.parse().ok()) {
                    Some(v) if v > 0 => v,
                    _ => {
                        eprintln!("--size-mib: atteso intero > 0");
                        exit(2);
                    }
                };
            }
            other => {
                eprintln!("argomento ignoto: {}", other);
                usage();
            }
        }
        i += 1;
    }

    let sb = build_super(uuid, 1, "");
    let size = size_mib * 1024 * 1024;
    let mut f = match OpenOptions::new().create(true).write(true).truncate(true).open(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("create {}: {}", path, e);
            exit(1);
        }
    };
    if f.write_all(&sb).is_err() || f.seek(SeekFrom::Start(512)).is_err() || f.write_all(&sb).is_err() {
        eprintln!("scrittura superblock fallita");
        exit(1);
    }
    // Resto dell'immagine a zero (sparse): porta la dimensione a `size`.
    if f.set_len(size).is_err() {
        eprintln!("set_len {} fallita", size);
        exit(1);
    }
    println!(
        "arca: creato {} ({} MiB), uuid={:016X}, gen=1, block_size={}",
        path,
        size_mib,
        uuid,
        sn::ARCA_BLOCK_SIZE
    );
    let _ = f.flush();
}
