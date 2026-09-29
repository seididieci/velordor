//! Volume ArcaFS on-disk (Fase 56.2a: formato + allocatore + nodi opachi).
//!
//! Blocco = 3584 B = 7 settori (1 op `DISK_*` esatta), partition-relative:
//! blocco N = settori N*7..N*7+6. Blocco 0 = superblock (LBA0) + shadow
//! (LBA1) + header-estensione (settori 2-6). Il blocco 0 non si alloca mai.
//! Layout e checksum vivono in `arcafs::format` (condivisi col tool host):
//! qui solo I/O + freelist + guardia live.
//!
//! Memoria: niente cache nodi in 56.2a (ogni op = I/O diretto); `live' e'
//! una guardia RAM anti-double-free, non uno store (lo store in-RAM e'
//! eliminato: il motore parla solo blocchi).

use super::*;
use crate::fat32::BlockSource;
use alloc::collections::BTreeSet;
use arcafs::format::{
    self, HeaderExt, ARCA_BLOCK_SECTORS, ARCA_NODE_PAYLOAD_LEN, ARCA_XHDROFF, ARCA_XHDRLEN,
};

/// Byte di un blocco (ARCA_BLOCK_SIZE = 3584).
pub const BLOCK_BYTES: usize = 3584;

fn block_lba(n: u64) -> Option<u64> {
    n.checked_mul(ARCA_BLOCK_SECTORS as u64)
}

/// Volume aperto: handle nodo + header-estensione + guardia allocazioni.
pub struct ArcaVolume {
    disk: IpcDisk,
    xh: HeaderExt,
    live: BTreeSet<u64>,
}

impl ArcaVolume {
    /// Legge un blocco intero. None su errore IO/overflow.
    fn read_block(&self, n: u64, out: &mut [u8; BLOCK_BYTES]) -> bool {
        match block_lba(n) {
            Some(lba) => self.disk.read_sectors(lba, ARCA_BLOCK_SECTORS, &mut out[..]),
            None => false,
        }
    }

    /// Scrive un blocco intero. False su errore IO/overflow.
    fn write_block(&self, n: u64, data: &[u8; BLOCK_BYTES]) -> bool {
        match block_lba(n) {
            Some(lba) => self.disk.write_sectors(lba, ARCA_BLOCK_SECTORS, &data[..]),
            None => false,
        }
    }

    /// Legge i 5 settori dell'header-estensione (blocco 0 da ARCA_XHDROFF).
    fn read_xh_raw(&self, out: &mut [u8; ARCA_XHDRLEN]) -> bool {
        let mut sec = [0u8; 512];
        let mut done = 0usize;
        let mut lba = (ARCA_XHDROFF / 512) as u64;
        while done < ARCA_XHDRLEN {
            if !self.disk.read_sector(lba, &mut sec) {
                return false;
            }
            let take = (ARCA_XHDRLEN - done).min(512);
            out[done..done + take].copy_from_slice(&sec[..take]);
            done += take;
            lba += 1;
        }
        true
    }

    /// Scrive l'header-estensione (write-through: chiamata a ogni alloc/free).
    /// La coda dell'ultimo settore resta a zero (riservata).
    fn write_xh_raw(&self, xh: &[u8; ARCA_XHDRLEN]) -> bool {
        let mut sec = [0u8; 512];
        let mut done = 0usize;
        let mut lba = (ARCA_XHDROFF / 512) as u64;
        while done < ARCA_XHDRLEN {
            let take = (ARCA_XHDRLEN - done).min(512);
            sec.fill(0);
            sec[..take].copy_from_slice(&xh[done..done + take]);
            if !self.disk.write_sector(lba, &sec) {
                return false;
            }
            done += take;
            lba += 1;
        }
        true
    }

    /// Scrive l'header-estensione corrente (dopo alloc/free).
    fn store_xh(&self) -> bool {
        self.write_xh_raw(&format::xhdr_encode(&self.xh))
    }

    /// Apre un volume formattato: superblock valido + header-estensione
    /// valida. La guardia `live' parte vuota (i blocchi allocati prima di
    /// questa apertura sono noti solo alla freelist).
    pub fn open(handle: u32) -> Option<Self> {
        let disk = IpcDisk::new(handle);
        let mut sec = [0u8; 512];
        if !disk.read_sector(0, &mut sec) {
            return None;
        }
        format::superblock_verify(&sec)?;
        let v = Self {
            disk,
            xh: HeaderExt { free_head: 0, high_water: 0, next_id: 0, next_snap: 0, flags: 0 },
            live: BTreeSet::new(),
        };
        let mut raw = [0u8; ARCA_XHDRLEN];
        if !v.read_xh_raw(&mut raw) {
            return None;
        }
        let xh = format::xhdr_decode(&raw)?;
        Some(Self { xh, ..v })
    }

    /// Formatta: header-ext vergine + nodo root vuoto al blocco 1 + ROOT nel
    /// superblock (checksum ricalcolato). Il superblock deve gia' esistere
    /// (`arca create`); qui si riempiono solo i campi A2. high_water = 2.
    /// (56.2c: recovery; per ora solo bootstrap dei volumi di test.)
    pub fn format(handle: u32) -> Option<Self> {
        let mut v = Self {
            disk: IpcDisk::new(handle),
            xh: HeaderExt { free_head: 0, high_water: 2, next_id: 1, next_snap: 1, flags: 0 },
            live: BTreeSet::new(),
        };
        let mut sec = [0u8; 512];
        if !v.disk.read_sector(0, &mut sec) {
            return None;
        }
        format::superblock_set_root(&mut sec, 1)?;
        if !v.disk.write_sector(0, &sec) {
            return None;
        }
        if !v.store_xh() {
            return None;
        }
        let mut root = [0u8; BLOCK_BYTES];
        let payload = [0u8; ARCA_NODE_PAYLOAD_LEN];
        format::node_fill(&mut root, format::ARCA_NODE_TYPE_RAW, 0, &payload);
        if !v.write_block(1, &root) {
            return None;
        }
        v.live.insert(1);
        Some(v)
    }

    /// Alloca un blocco: pop dalla freelist o high_water++. Mai il blocco 0.
    /// Scrive header-ext (write-through) e marca live (guardia double-alloc).
    pub fn alloc(&mut self) -> Option<u64> {
        let n = if self.xh.free_head != 0 {
            let head = self.xh.free_head;
            let mut blk = [0u8; BLOCK_BYTES];
            if !self.read_block(head, &mut blk) {
                return None;
            }
            self.xh.free_head = u64::from_le_bytes(blk[..8].try_into().ok()?);
            head
        } else {
            let n = self.xh.high_water;
            if n == 0 {
                return None; // high_water 0 = blocco 0, mai allocabile
            }
            self.xh.high_water += 1;
            n
        };
        if n == 0 || !self.live.insert(n) {
            return None; // blocco 0 o double-alloc (guardia RAM)
        }
        if !self.store_xh() {
            self.live.remove(&n);
            return None;
        }
        Some(n)
    }

    /// Libera un blocco: push in freelist (next nei primi 8 B) + unmark live.
    /// Blocco 0, mai-allocato o double-free → false (guardia RAM, mai IO).
    pub fn free(&mut self, n: u64) -> bool {
        if n == 0 || !self.live.remove(&n) {
            return false;
        }
        let mut blk = [0u8; BLOCK_BYTES];
        blk[..8].copy_from_slice(&self.xh.free_head.to_le_bytes());
        if !self.write_block(n, &blk) {
            self.live.insert(n);
            return false;
        }
        self.xh.free_head = n;
        if !self.store_xh() {
            return false;
        }
        true
    }

    /// Scrive un nodo opaco (header magic+type+gen + payload + checksum).
    /// Blocco 0 protetto; il blocco deve essere live (allocato qui).
    pub fn write_node(&mut self, n: u64, ty: u8, generation: u64, payload: &[u8; ARCA_NODE_PAYLOAD_LEN]) -> bool {
        if n == 0 || !self.live.contains(&n) {
            return false;
        }
        let mut blk = [0u8; BLOCK_BYTES];
        format::node_fill(&mut blk, ty, generation, payload);
        self.write_block(n, &blk)
    }

    /// Legge e verifica un nodo: magic + checksum. Il payload va in `out`.
    /// Blocco 0 protetto; type/gen riportati per il chiamante (56.2b).
    pub fn read_node(&self, n: u64, out: &mut [u8; ARCA_NODE_PAYLOAD_LEN]) -> Option<(u8, u64)> {
        if n == 0 {
            return None;
        }
        let mut blk = [0u8; BLOCK_BYTES];
        if !self.read_block(n, &mut blk) {
            return None;
        }
        let (ty, generation) = format::node_verify(&blk)?;
        out.copy_from_slice(&blk[arcafs::format::ARCA_NODE_PAYLOAD..arcafs::format::ARCA_NODE_PAYLOAD + ARCA_NODE_PAYLOAD_LEN]);
        Some((ty, generation))
    }

    /// (high_water, live_count, free_head) per il debug STAT.
    pub fn stats(&self) -> (u64, u64, u64) {
        (self.xh.high_water, self.live.len() as u64, self.xh.free_head)
    }
}
