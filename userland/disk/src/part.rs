//! Tabella partizioni MBR/GPT (Fase 16 + Fase 55, A1).
//!
//! MBR: voci primarie (4 a `0x1BE`, 16 byte l'una): tipo, LBA iniziale e
//! dimensione. Niente catene extended/logical (futuro documentato).
//! GPT: header EFI a LBA1 + 128 entry da 128B; guard protective-MBR `0xEE`
//! (Fase 55: CRC32 saltato, backup header ignorato per A1).

use alloc::vec::Vec;
use super::*;

/// Voce di partizione primaria.
pub struct PartInfo {
    /// Byte tipo MBR (0x0B/0x0C = FAT32, 0x83 = Linux, ...).
    pub ptype: u8,
    /// Primo LBA della partizione.
    pub start: u32,
    /// Settori della partizione.
    pub sectors: u32,
}

/// Tipo GPT ArcaFS (placeholder, da registrare con UUID alias prima del rilascio).
/// Non testiamo il match su disco in A1 — costante commentata per riferimento.
// pub const ARCAFS_TYPE_GUID: [u8; 16] = [ /* TODO: registrare */ ];

/// Partizione GPT (coordinate fisiche LBA48, Fase 55).
pub struct PartLoc {
    /// Primo settore della partizione (LBA48).
    pub start: u64,
    /// Numero di settori.
    pub sectors: u64,
}

/// Firma MBR a fine settore.
const MBR_SIG0: u8 = 0x55;
const MBR_SIG1: u8 = 0xAA;

/// Magic GPT "EFI-part" (8 byte a offset 0 del superblock GPT).
const GPT_MAGIC: [u8; 8] = [0x45, 0x46, 0x49, 0x2d, 0x70, 0x61, 0x72, 0x74];

/// Tipo protective-MBR (byte 446 del settore 0): segnale "questo disco e' GPT".
const PARTITION_TYPE_EFI: u8 = 0xEE;

/// Parsa le voci primarie dal settore 0. Ritorna le partizioni non vuote
/// (tipo != 0 e size != 0), al massimo 4.
pub fn parse_mbr(sector0: &[u8; 512], out: &mut Vec<PartInfo>) {
    if sector0[510] != MBR_SIG0 || sector0[511] != MBR_SIG1 {
        return;
    }
    for i in 0..4 {
        let base = 0x1BE + i * 16;
        let ptype = sector0[base + 4];
        let start = u32::from_le_bytes([
            sector0[base + 8],
            sector0[base + 9],
            sector0[base + 10],
            sector0[base + 11],
        ]);
        let sectors = u32::from_le_bytes([
            sector0[base + 12],
            sector0[base + 13],
            sector0[base + 14],
            sector0[base + 15],
        ]);
        if ptype != 0 && sectors != 0 {
            out.push(PartInfo { ptype, start, sectors });
        }
    }
}

/// Parsa il GPT header a LBA1. Ritorna Some se presente (protective-MBR `0xEE`),
/// None se disco non-GPT. Il caller usa i campi per scansionare le entry.
pub fn parse_gpt_header(sector1: &[u8; 512]) -> Option<(u64, u64, u32, u32)> {
    // Magic "EFI-part" a offset 0
    if sector1[0..8] != GPT_MAGIC {
        return None;
    }
    // Offset 44: current LBA (4 byte LE) — deve essere 1
    let current_lba = u32::from_le_bytes([sector1[44], sector1[45], sector1[46], sector1[47]]);
    if current_lba != 1 {
        return None;
    }
    // Offset 56: first usable LBA (8 byte LE)
    let first_usable = u64::from_le_bytes([
        sector1[56], sector1[57], sector1[58], sector1[59],
        sector1[60], sector1[61], sector1[62], sector1[63],
    ]);
    // Offset 64: last usable LBA (8 byte LE)
    let last_usable = u64::from_le_bytes([
        sector1[64], sector1[65], sector1[66], sector1[67],
        sector1[68], sector1[69], sector1[70], sector1[71],
    ]);
    // Offset 112: partition entries array offset (4 byte LE)
    let entries_offset = u32::from_le_bytes([sector1[112], sector1[113], sector1[114], sector1[115]]);
    // Offset 120: numero di entry GPT (4 byte LE)
    let entries_count = u32::from_le_bytes([sector1[120], sector1[121], sector1[122], sector1[123]]);

    Some((first_usable, last_usable, entries_offset, entries_count))
}

/// Parsa le entry GPT da un disco. Legge LBA0 (protective-MBR), LBA1 (header),
/// poi scansiona le entry a partire dall'offset dell'header.
/// Ritorna le partizioni valide (first <= last, sectors > 0).
pub fn parse_gpt(disk: &block::AtaDisk, lba0: &[u8; 512], out: &mut Vec<PartLoc>) {
    // Guard protective-MBR: byte 446 = 0xEE → disco GPT (non MBR legacy)
    if lba0[446] != PARTITION_TYPE_EFI {
        return;
    }

    // Leggi LBA1 (GPT header)
    let mut sec1 = [0u8; 512];
    if !disk.read_sector(1, &mut sec1) {
        return;
    }

    let (first_usable, last_usable, entries_offset, entries_count) = match parse_gpt_header(&sec1) {
        Some(v) => v,
        None => return,
    };

    // Scansiona le entry GPT (max 128)
    for i in 0..entries_count.min(128) {
        let entry_base = (entries_offset as u64 + i as u64) * 512;
        if entry_base == 1 {
            continue; // Skip LBA1 che contiene l'header
        }

        let mut sec = [0u8; 512];
        if !disk.read_sector(entry_base, &mut sec) {
            continue;
        }

        // Type GUID: 16 byte a offset 0 dell'entry
        // (A1: non testiamo il match con ARCAFS_TYPE_GUID — placeholder)

        // First LBA: 8 byte LE a offset 48
        let first_lba = u64::from_le_bytes([
            sec[48], sec[49], sec[50], sec[51],
            sec[52], sec[53], sec[54], sec[55],
        ]);

        // Last LBA: 8 byte LE a offset 56
        let last_lba = u64::from_le_bytes([
            sec[56], sec[57], sec[58], sec[59],
            sec[60], sec[61], sec[62], sec[63],
        ]);

        // Validazione: first <= last, sectors > 0
        if first_lba <= last_lba && first_lba >= first_usable && last_lba <= last_usable {
            let sectors = last_lba - first_lba + 1;
            out.push(PartLoc { start: first_lba, sectors });
        }
    }
}

/// Parsa MBR e GPT da un disco. Ritorna le partizioni trovate (MBR o GPT, mai entrambi).
/// Se byte 446 di LBA0 == `0xEE` → GPT; altrimenti → MBR legacy (o disco non partizionato).
pub fn parse_partitions(disk: &block::AtaDisk, lba0: &[u8; 512]) -> PartitionResult {
    // Controlla se e' un disco GPT (protective-MBR)
    if lba0[446] == PARTITION_TYPE_EFI {
        let mut parts = Vec::new();
        parse_gpt(disk, lba0, &mut parts);
        return PartitionResult::Gpt(parts);
    }

    // Altrimenti MBR legacy (o disco non partizionato)
    let mut parts = Vec::new();
    parse_mbr(lba0, &mut parts);
    PartitionResult::Mbr(parts)
}

/// Risultato del parse: MBR o GPT.
pub enum PartitionResult {
    Mbr(Vec<PartInfo>),
    Gpt(Vec<PartLoc>),
}
