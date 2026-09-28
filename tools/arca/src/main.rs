//! `arca` host tool (Fase 54, P5): per ora solo `create` — scrive un
//! superblock ArcaFS con offset automatico: whole-disk → LBA0, partizione →
//! `start_lba` dalla tabella MBR/GPT del disco.
//! Offset/magic/checksum sono la single source `syscall-numbers`: questo
//! tool non li duplica (il round-trip create→mount→stat e' il test che li
//! tiene d'accordo). Le op guest (`list`/`get`/...) vivono in
//! `userland/arca`; il tool completo arriva in A7.
//!
//! Uso: `arca create <path> [--uuid HEX] [--size-mib N]`.
//! `<path>` puo' essere un file immagine (LBA0) o un device/partizione:
//! `/dev/sdX` → whole-disk (LBA0), `/dev/sdXn` → partizione, il tool legge
//! la tabella MBR/GPT del disco per trovare `start_lba` e scrive li'.

use std::env;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
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

/// Estrae il nome del disco base da un path di partizione.
/// `/dev/sda1` → `sda`, `/dev/nvme0n1p2` → `nvme0n1`, `/dev/sdb` → `sdb`.
fn base_disk_name(path: &str) -> Option<&str> {
    let name = path.trim_start_matches("/dev/");
    // MBR/GPT naming: sd[a-z][1-9] o nvmeNnM[p][1-9]
    if name.starts_with("sd") && name.len() > 2 {
        let rest = &name[2..];
        if let Some(idx) = rest.bytes().position(|b| b.is_ascii_digit()) {
            return Some(&name[..2 + idx]);
        }
    }
    // NVMe: nvmeNnM o nvmeNnMpM
    if name.starts_with("nvme") {
        let mut parts = name.split('p');
        if let Some(base) = parts.next() {
            for p in parts {
                if p.bytes().all(|b| b.is_ascii_digit()) && !p.is_empty() {
                    return Some(base);
                }
            }
            // No partition number → whole nvme device
            return Some(name);
        }
    }
    None
}

/// Legge il primo settore (512 B) del disco per rilevare MBR/GPT.
fn read_mbr_gpt(disk_path: &str, buf: &mut [u8; 512]) -> bool {
    let mut f = match OpenOptions::new().read(true).open(disk_path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    match f.read_exact(buf) {
        Ok(_) => true,
        Err(_) => false,
    }
}

/// Cerca una partizione attiva nel MBR (registro 446–511).
/// Ritorna `Some((start_lba, size))` della prima entry con type != 0.
fn find_mbr_partition(buf: &[u8; 512]) -> Option<(u64, u64)> {
    // MBR partition table starts at offset 446 (0x1BE), 16 bytes per entry
    for i in 0..4 {
        let off = 446 + i * 16;
        let ptype = buf[off + 4];
        if ptype == 0 {
            continue; // entry vuota
        }
        // CHL → LBA: settori 0-2 di offset+8 (3 bytes, little-endian)
        let start = u32::from(buf[off + 8]) as u64
            | ((u32::from(buf[off + 9]) & 0x3F) as u64) << 8
            | ((u32::from(buf[off + 10])) as u64) << 16;
        let size = u32::from_le_bytes([
            buf[off + 12],
            buf[off + 13],
            buf[off + 14],
            buf[off + 15],
        ]) as u64;
        if start > 0 && size > 0 {
            return Some((start, size));
        }
    }
    None
}

/// Trova l'offset LBA della partizione nel path dato.
fn find_partition_offset(path: &str) -> Option<(u64, u64)> {
    let disk_name = base_disk_name(path)?;
    // Costruisce il path del disco base: /dev/sda1 → /dev/sda
    let disk_path = format!("/dev/{}", disk_name);

    let mut buf = [0u8; 512];
    if !read_mbr_gpt(&disk_path, &mut buf) {
        eprintln!("arca: impossibile leggere {} (prova come file immagine)", disk_path);
        return None;
    }

    // Prova MBR prima
    if let Some((start, size)) = find_mbr_partition(&buf) {
        println!(
            "arca: partizione {} su {}: MBR start={} settori={}",
            path, disk_path, start, size
        );
        return Some((start, size));
    }

    // Prova GPT: byte 446 = 0xEE → protective MBR. La tabella e' a LBA1.
    if buf[446] == 0xEE {
        // Leggi LBA1 (la prima partizione GPT)
        let mut f = match OpenOptions::new().read(true).open(&disk_path) {
            Ok(f) => f,
            Err(_) => return None,
        };
        if f.seek(SeekFrom::Start(512)).is_ok() && f.read_exact(&mut buf).is_ok() {
            // GPT header a LBA1: offset 44 = number of partition entries
            let num_entries = u32::from_le_bytes([buf[44], buf[45], buf[46], buf[47]]) as usize;
            // Offset del primo entry nella tabella (di solito 512)
            let entry_off = u32::from_le_bytes([buf[48], buf[49], buf[50], buf[51]]);
            // Ogni entry e' 128 byte
            for i in 0..num_entries {
                let off = (entry_off + i as u32 * 128) as usize;
                if off + 16 > 512 {
                    break;
                }
                // Tipo GUID: primi 8 byte dell'entry
                let type_low = u64::from_le_bytes([
                    buf[off], buf[off+1], buf[off+2], buf[off+3],
                    buf[off+4], buf[off+5], buf[off+6], buf[off+7],
                ]);
                if type_low == 0 {
                    continue; // entry vuota
                }
                // Start LBA: bytes 8-15 dell'entry
                let start = u64::from_le_bytes([
                    buf[off + 8], buf[off + 9], buf[off + 10], buf[off + 11],
                    buf[off + 12], buf[off + 13], buf[off + 14], buf[off + 15],
                ]);
                // Numero di settori: bytes 16-23
                let size = u64::from_le_bytes([
                    buf[off + 16], buf[off + 17], buf[off + 18], buf[off + 19],
                    buf[off + 20], buf[off + 21], buf[off + 22], buf[off + 23],
                ]);
                if start > 0 && size > 0 {
                    println!(
                        "arca: partizione {} su {}: GPT start={} settori={}",
                        path, disk_path, start, size
                    );
                    return Some((start, size));
                }
            }
        }
    }

    eprintln!("arca: nessuna partizione trovata su {}", disk_path);
    None
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

    // Determina se e' una partizione o whole-disk
    let is_partition = path.trim_start_matches("/dev/")
        .as_bytes()
        .last()
        .map_or(false, |b| b.is_ascii_digit());

    if is_partition {
        // Partizione: trova start_lba dalla tabella del disco
        match find_partition_offset(path) {
            Some((start, size)) => {
                let mut f = match OpenOptions::new().write(true).open(path) {
                    Ok(f) => f,
                    Err(e) => {
                        eprintln!("create {}: {}", path, e);
                        exit(1);
                    }
                };
                // Scrivi superblock a start_lba * 512
                let offset = start * 512;
                if f.seek(SeekFrom::Start(offset)).is_err() || f.write_all(&sb).is_err() {
                    eprintln!("scrittura superblock a offset {} fallita", offset);
                    exit(1);
                }
                println!(
                    "arca: creato {} (partizione start={} size={} MiB), uuid={:016X}, gen=1, block_size={}",
                    path,
                    start,
                    size * 512 / 1024 / 1024,
                    uuid,
                    sn::ARCA_BLOCK_SIZE
                );
            }
            None => {
                eprintln!("arca: impossibile determinare offset partizione per {}", path);
                exit(1);
            }
        }
    } else {
        // Whole-disk o file immagine: LBA0
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
    }
    
}
