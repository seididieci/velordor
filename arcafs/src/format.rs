//! Formato blocchi on-disk ArcaFS (Fase 56.2a). Funzioni pure di
//! encode/decode/verifica condivise tra guest e tool host: il layout e'
//! definito UNA volta sola qui.
//!
//! Blocco = 3584 B = 7 settori (1 op `DISK_*` esatta), partition-relative:
//! blocco N = settori N*7..N*7+6. Blocco 0 = superblock (LBA0) + shadow
//! (LBA1) + header-estensione (settori 2-6, i primi 1024 B restano intoccati
//! per non sfiorare mai superblock/shadow). Il blocco 0 non si alloca mai.

use syscall_numbers::image_hash;

/// Magic superblock + versione formato.
pub const ARCA_MAGIC: &[u8; 4] = b"ACFS";
pub const ARCA_VERSION: u32 = 1;
/// Block size negoziato (fisso v1) = 1 chunk DISK_* esatto.
pub const ARCA_BLOCK_SIZE: u32 = 3584;
/// Superblock: 128 B a LBA0 (+ shadow LBA1); checksum FNV-1a di [0..120].
pub const ARCA_SUPER_LEN: usize = 128;
pub const ARCA_OFF_MAGIC: usize = 0;
pub const ARCA_OFF_VERSION: usize = 4;
pub const ARCA_OFF_BLOCK_SIZE: usize = 8;
pub const ARCA_OFF_UUID: usize = 12;
pub const ARCA_OFF_GEN: usize = 20;
pub const ARCA_OFF_ROOT: usize = 28;
pub const ARCA_OFF_REFCOUNT: usize = 36;
pub const ARCA_OFF_ALLOC: usize = 44;
pub const ARCA_OFF_MOUNT: usize = 52;
pub const ARCA_OFF_AUTO: usize = 116;
pub const ARCA_OFF_FLAGS: usize = 117;
pub const ARCA_OFF_CHECK: usize = 120;
/// Settori per blocco.
pub const ARCA_BLOCK_SECTORS: usize = 7;
/// Byte offset dell'header-estensione nel blocco 0 (salta LBA0+LBA1).
pub const ARCA_XHDROFF: usize = 1024;
/// Magic header-estensione + versione.
pub const ARCA_XMAGIC: &[u8; 4] = b"AXHD";
pub const ARCA_XVER: u32 = 1;
/// Header-estensione (56 B): magic 0:4, ver 4:4, free_head 8:8, high_water
/// 16:8, next_id 24:8, next_snap 32:8, flags 40:8 (bit0 DIRTY, 56.2b),
/// check 48:8 (FNV-1a di [0..48]).
pub const ARCA_XHDRLEN: usize = 56;
pub const ARCA_XHOFF_FREE: usize = 8;
pub const ARCA_XHOFF_HIGH: usize = 16;
pub const ARCA_XHOFF_NEXTID: usize = 24;
pub const ARCA_XHOFF_NEXTSNAP: usize = 32;
pub const ARCA_XHOFF_FLAGS: usize = 40;
pub const ARCA_XHOFF_CHECK: usize = 48;
/// Nodo blocco (3584 B): magic "ANOD" 0:4, type 4:1 (RAW=0 opaco 56.2a;
/// LEAF=1/INTERNAL=2 riservati 56.2b), gen 8:8 (0 in 56.2a), payload
/// 16:3560, check 3576:8 (FNV-1a di [0..3576]).
pub const ARCA_NMAGIC: &[u8; 4] = b"ANOD";
pub const ARCA_NODE_TYPE_RAW: u8 = 0;
pub const ARCA_NODE_TYPE_LEAF: u8 = 1;
pub const ARCA_NODE_TYPE_INTERNAL: u8 = 2;
pub const ARCA_NODE_PAYLOAD: usize = 16;
pub const ARCA_NODE_PAYLOAD_LEN: usize = 3560;
pub const ARCA_NODE_CHECK: usize = 3576;

/// Header-estensione parsata.
pub struct HeaderExt {
    pub free_head: u64,
    pub high_water: u64,
    pub next_id: u64,
    pub next_snap: u64,
    pub flags: u64,
}

fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([
        b[o], b[o + 1], b[o + 2], b[o + 3], b[o + 4], b[o + 5], b[o + 6], b[o + 7],
    ])
}

fn put64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

/// Verifica superblock (magic+versione+block-size+checksum): `(gen, uuid)`
/// o `None`. Stessi 4 check di `probe_arca` (il mount resta dov'e').
pub fn superblock_verify(sec: &[u8]) -> Option<(u64, u64)> {
    let sb = sec.get(..ARCA_SUPER_LEN)?;
    if sb[ARCA_OFF_MAGIC..ARCA_OFF_MAGIC + 4] != *ARCA_MAGIC {
        return None;
    }
    let u32le = |o: usize| {
        u32::from_le_bytes([sb[o], sb[o + 1], sb[o + 2], sb[o + 3]])
    };
    if u32le(ARCA_OFF_VERSION) != ARCA_VERSION {
        return None;
    }
    if u32le(ARCA_OFF_BLOCK_SIZE) != ARCA_BLOCK_SIZE {
        return None;
    }
    if image_hash(&sb[..ARCA_OFF_CHECK]) != u64le(sb, ARCA_OFF_CHECK) {
        return None;
    }
    Some((u64le(sb, ARCA_OFF_GEN), u64le(sb, ARCA_OFF_UUID)))
}

/// Costruisce 128 B di superblock (LE esplicito, checksum FNV-1a).
/// Mossa dal tool host: stessa funzione per `create` e guest `format`.
pub fn superblock_build(uuid: u64, generation: u64, mountpoint: &str) -> [u8; ARCA_SUPER_LEN] {
    let mut sb = [0u8; ARCA_SUPER_LEN];
    sb[ARCA_OFF_MAGIC..ARCA_OFF_MAGIC + 4].copy_from_slice(ARCA_MAGIC);
    sb[ARCA_OFF_VERSION..ARCA_OFF_VERSION + 4].copy_from_slice(&ARCA_VERSION.to_le_bytes());
    sb[ARCA_OFF_BLOCK_SIZE..ARCA_OFF_BLOCK_SIZE + 4]
        .copy_from_slice(&ARCA_BLOCK_SIZE.to_le_bytes());
    sb[ARCA_OFF_UUID..ARCA_OFF_UUID + 8].copy_from_slice(&uuid.to_le_bytes());
    sb[ARCA_OFF_GEN..ARCA_OFF_GEN + 8].copy_from_slice(&generation.to_le_bytes());
    let mp = mountpoint.as_bytes();
    let n = mp.len().min(63);
    sb[ARCA_OFF_MOUNT..ARCA_OFF_MOUNT + n].copy_from_slice(&mp[..n]);
    sb[ARCA_OFF_AUTO] = 0;
    sb[ARCA_OFF_FLAGS] = 0;
    let checksum = image_hash(&sb[..ARCA_OFF_CHECK]);
    sb[ARCA_OFF_CHECK..ARCA_OFF_CHECK + 8].copy_from_slice(&checksum.to_le_bytes());
    sb
}

/// Imposta ROOT nel superblock e ricalcola il checksum (preserva il resto).
/// `None` se il settore non e' un superblock valido.
pub fn superblock_set_root(sec: &mut [u8; 512], root: u64) -> Option<()> {
    superblock_verify(sec)?;
    sec[ARCA_OFF_ROOT..ARCA_OFF_ROOT + 8].copy_from_slice(&root.to_le_bytes());
    let checksum = image_hash(&sec[..ARCA_OFF_CHECK]);
    sec[ARCA_OFF_CHECK..ARCA_OFF_CHECK + 8].copy_from_slice(&checksum.to_le_bytes());
    Some(())
}

/// Codifica header-estensione (56 B).
pub fn xhdr_encode(xh: &HeaderExt) -> [u8; ARCA_XHDRLEN] {
    let mut b = [0u8; ARCA_XHDRLEN];
    b[..4].copy_from_slice(ARCA_XMAGIC);
    b[4..8].copy_from_slice(&ARCA_XVER.to_le_bytes());
    put64(&mut b, ARCA_XHOFF_FREE, xh.free_head);
    put64(&mut b, ARCA_XHOFF_HIGH, xh.high_water);
    put64(&mut b, ARCA_XHOFF_NEXTID, xh.next_id);
    put64(&mut b, ARCA_XHOFF_NEXTSNAP, xh.next_snap);
    put64(&mut b, ARCA_XHOFF_FLAGS, xh.flags);
    let check = image_hash(&b[..ARCA_XHOFF_CHECK]);
    put64(&mut b, ARCA_XHOFF_CHECK, check);
    b
}

/// Decodifica+verifica header-estensione (magic+ver+checksum).
pub fn xhdr_decode(b: &[u8]) -> Option<HeaderExt> {
    let b = b.get(..ARCA_XHDRLEN)?;
    if b[..4] != *ARCA_XMAGIC {
        return None;
    }
    if u32::from_le_bytes([b[4], b[5], b[6], b[7]]) != ARCA_XVER {
        return None;
    }
    if image_hash(&b[..ARCA_XHOFF_CHECK]) != u64le(b, ARCA_XHOFF_CHECK) {
        return None;
    }
    Some(HeaderExt {
        free_head: u64le(b, ARCA_XHOFF_FREE),
        high_water: u64le(b, ARCA_XHOFF_HIGH),
        next_id: u64le(b, ARCA_XHOFF_NEXTID),
        next_snap: u64le(b, ARCA_XHOFF_NEXTSNAP),
        flags: u64le(b, ARCA_XHOFF_FLAGS),
    })
}

/// Riempie un blocco-nodo (header + payload + checksum).
pub fn node_fill(blk: &mut [u8; 3584], ty: u8, gen: u64, payload: &[u8; ARCA_NODE_PAYLOAD_LEN]) {
    blk[..4].copy_from_slice(ARCA_NMAGIC);
    blk[4] = ty;
    blk[8..16].copy_from_slice(&gen.to_le_bytes());
    blk[ARCA_NODE_PAYLOAD..ARCA_NODE_PAYLOAD + ARCA_NODE_PAYLOAD_LEN].copy_from_slice(payload);
    let check = image_hash(&blk[..ARCA_NODE_CHECK]);
    blk[ARCA_NODE_CHECK..].copy_from_slice(&check.to_le_bytes());
}

/// Verifica un blocco-nodo (magic + checksum): `(type, gen)` o `None`.
pub fn node_verify(blk: &[u8; 3584]) -> Option<(u8, u64)> {
    if blk[..4] != *ARCA_NMAGIC {
        return None;
    }
    if image_hash(&blk[..ARCA_NODE_CHECK]) != u64le(blk, ARCA_NODE_CHECK) {
        return None;
    }
    Some((blk[4], u64le(blk, 8)))
}
