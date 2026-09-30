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

use arcafs as af;

fn usage() -> ! {
    eprintln!("uso: arca create <path> [--uuid HEX16] [--size-mib N]  (default 1 MiB, uuid auto)");
    exit(2);
}

/// Inizializza un volume vuoto su `f` a `base` settori (0 = whole-disk):
/// header-estensione + nodo root al blocco 1 + ROOT nel superblock.
/// Fallisce loud (exit 1) su qualunque errore IO.
fn init_empty_volume(f: &mut std::fs::File, base: u64, uuid: u64) {
    use std::io::{Seek, SeekFrom, Write};
    // Root vuota al blocco 1 (relativo a `base`).
    let mut root = [0u8; 3584];
    af::format::node_fill(&mut root, af::format::ARCA_NODE_TYPE_RAW, 0, &[0u8; af::format::ARCA_NODE_PAYLOAD_LEN]);
    let root_off = (base + 7) * 512;
    if f.seek(SeekFrom::Start(root_off)).is_err() || f.write_all(&root).is_err() {
        eprintln!("scrittura root fallita");
        exit(1);
    }
    // Header-estensione a settori base+2..base+6.
    let xh = af::format::HeaderExt {
        free_head: 0,
        high_water: 2,
        next_id: 1,
        next_snap: 1,
        flags: 0,
    };
    let enc = af::format::xhdr_encode(&xh);
    let xh_off = base * 512 + af::format::ARCA_XHDROFF as u64;
    if f.seek(SeekFrom::Start(xh_off)).is_err() || f.write_all(&enc).is_err() {
        eprintln!("scrittura header-estensione fallita");
        exit(1);
    }
    // ROOT = 1 nel superblock (checksum ricalcolato).
    let mut sec0 = [0u8; 512];
    if f.seek(SeekFrom::Start(base * 512)).is_err()
        || std::io::Read::read_exact(f, &mut sec0).is_err()
        || af::format::superblock_set_root(&mut sec0, 1).is_none()
    {
        eprintln!("ROOT nel superblock fallito");
        exit(1);
    }
    if f.seek(SeekFrom::Start(base * 512)).is_err() || f.write_all(&sec0).is_err() {
        eprintln!("scrittura superblock con ROOT fallita");
        exit(1);
    }
    // Shadow sincronizzato (stessa generazione al format: crash-safe per
    // costruzione, il flip entra con il commit 56.2b).
    if f.seek(SeekFrom::Start((base + 1) * 512)).is_err() || f.write_all(&sec0).is_err() {
        eprintln!("scrittura shadow fallita");
        exit(1);
    }
    println!(
        "arca: volume inizializzato (root=1, uuid={:016X})",
        uuid,
    );
}

/// Genera un UUID volume a 64 bit dall'OS RNG (`/dev/urandom`, 8 byte).
/// Fallback senza entropia: `(nanos xor pid)` — mai collisione pratica su
/// volumi creati a mano, ma il path primario resta l'OS (documentato qui,
/// non nascosto). Zero dipendenze esterne (repo offline, niente registry).
fn auto_uuid() -> u64 {
    if let Ok(mut f) = OpenOptions::new().read(true).open("/dev/urandom") {
        let mut b = [0u8; 8];
        if std::io::Read::read_exact(&mut f, &mut b).is_ok() {
            let v = u64::from_le_bytes(b);
            if v != 0 {
                return v;
            }
        }
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15);
    (nanos ^ (std::process::id() as u64).wrapping_mul(0x100000001B3)).max(1)
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
/// BUG STORICO (Fase 55): lo start era ricalcolato a mano su 3 byte stile
/// CHS invece che u32 LE a off+8 — ora single source col guest
/// (`parse_mbr` in block/part.rs).
fn find_mbr_partition(buf: &[u8; 512]) -> Option<(u64, u64)> {
    // MBR partition table starts at offset 446 (0x1BE), 16 bytes per entry
    for i in 0..4 {
        let off = 446 + i * 16;
        let ptype = buf[off + 4];
        if ptype == 0 {
            continue; // entry vuota
        }
        let start = u32::from_le_bytes([buf[off + 8], buf[off + 9], buf[off + 10], buf[off + 11]]) as u64;
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

    // Prova GPT: tipo prima voce (byte 450, NON 446 che e' il boot flag) =
    // 0xEE → protective MBR. Header a LBA1, offset UEFI reali (stessi del
    // guest `parse_gpt_header`: magic 0, current 24, first usable 40,
    // last usable 48, entry LBA 72, count 80, size 84).
    if buf[450] == 0xEE {
        let mut f = match OpenOptions::new().read(true).open(&disk_path) {
            Ok(f) => f,
            Err(_) => return None,
        };
        let mut hdr = [0u8; 512];
        if f.seek(SeekFrom::Start(512)).is_err() || f.read_exact(&mut hdr).is_err() {
            return None;
        }
        if hdr[0..8] != *b"EFI PART" {
            return None;
        }
        let u64le = |o: usize| u64::from_le_bytes([
            hdr[o], hdr[o+1], hdr[o+2], hdr[o+3], hdr[o+4], hdr[o+5], hdr[o+6], hdr[o+7],
        ]);
        if u64le(24) != 1 {
            return None;
        }
        let first_usable = u64le(40);
        let last_usable = u64le(48);
        let entries_lba = u64le(72);
        let num_entries = u32::from_le_bytes([hdr[80], hdr[81], hdr[82], hdr[83]]) as u64;
        let entry_size = u32::from_le_bytes([hdr[84], hdr[85], hdr[86], hdr[87]]);
        if entry_size != 128 || num_entries == 0 {
            return None;
        }
        // Legge l'array entry (128 B l'una, anche a cavallo di settore).
        let total = (num_entries.min(128) * 128) as usize;
        let arr_off = entries_lba * 512;
        let arr_end = arr_off + total as u64;
        let disk_len = f.seek(SeekFrom::End(0)).unwrap_or(0);
        if arr_end > disk_len {
            return None;
        }
        let mut arr = vec![0u8; total];
        if f.seek(SeekFrom::Start(arr_off)).is_err() || f.read_exact(&mut arr).is_err() {
            return None;
        }
        for i in 0..num_entries.min(128) {
            let off = (i * 128) as usize;
            let e = &arr[off..off + 128];
            if e[0..16] == [0u8; 16] {
                continue; // entry vuota
            }
            // Entry UEFI: first LBA a +32, last LBA a +40 (u64 LE).
            let start = u64::from_le_bytes(e[32..40].try_into().unwrap());
            let last = u64::from_le_bytes(e[40..48].try_into().unwrap());
            if start == 0 || start > last || start < first_usable || last > last_usable {
                continue;
            }
            let size = last - start + 1;
            println!(
                "arca: partizione {} su {}: GPT start={} settori={}",
                path, disk_path, start, size
            );
            return Some((start, size));
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
    // UUID auto-generato (Fase 56.2a): l'unicita' del volume non puo'
    // dipendere dall'utente (default 1 = collisioni garantite tra volumi).
    // `--uuid` resta solo come override esplicito (test deterministici).
    let mut uuid: u64 = auto_uuid();
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

    let sb = af::format::superblock_build(uuid, 1, "");

    // Determina se e' una partizione o whole-disk
    let is_partition = path.trim_start_matches("/dev/")
        .as_bytes()
        .last()
        .map_or(false, |b| b.is_ascii_digit());

    if is_partition {
        // Partizione: trova start_lba dalla tabella del disco
        match find_partition_offset(path) {
            Some((start, size)) => {
                let mut f = match OpenOptions::new().read(true).write(true).open(path) {
                    Ok(f) => f,
                    Err(e) => {
                        eprintln!("create {}: {}", path, e);
                        exit(1);
                    }
                };
                // Scrivi superblock a start_lba * 512 + shadow a start+1
                // (partition-relative LBA0/LBA1, arcafs.md §16.4).
                let offset = start * 512;
                let shadow = (start + 1) * 512;
                let ok = f.seek(SeekFrom::Start(offset)).is_ok()
                    && f.write_all(&sb).is_ok()
                    && f.seek(SeekFrom::Start(shadow)).is_ok()
                    && f.write_all(&sb).is_ok();
                if !ok {
                    eprintln!("scrittura superblock a offset {} fallita", offset);
                    exit(1);
                }
                init_empty_volume(&mut f, start, uuid);
                println!(
                    "arca: creato {} (partizione start={} size={} MiB), uuid={:016X}, gen=1, block_size={}",
                    path,
                    start,
                    size * 512 / 1024 / 1024,
                    uuid,
                    af::format::ARCA_BLOCK_SIZE
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
        let mut f = match OpenOptions::new().create(true).read(true).write(true).truncate(true).open(path) {
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
        init_empty_volume(&mut f, 0, uuid);
        println!(
            "arca: creato {} ({} MiB), uuid={:016X}, gen=1, block_size={}",
            path,
            size_mib,
            uuid,
            af::format::ARCA_BLOCK_SIZE
        );
    }
    
}
