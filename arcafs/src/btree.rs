//! B+tree COW per ArcaFS 56.2b (puro, `no_std`+`alloc`, niente I/O).
//!
//! Logica di indice condivisa tra guest (cardo) e test host: il codec dei
//! nodi e gli algoritmi split/merge vivono UNA volta sola qui; lo storage
//! (blocchi 3584 B con checksum in `format`) e' iniettato via [`BlockStore`].
//! `userland/fs` implementa il trait sopra `ArcaVolume` (+ cache LRU);
//! i test host usano `MemStore` (sotto, `#[cfg(test)]`).
//!
//! Alberi (tre radici indipendenti, stesso store):
//! - primary (tag 0): chiave composita `[id:8][seq:8]` → record versione.
//!   Ogni PUT = un record nuovo (mai overwrite); la head e' il seq max;
//!   il trim retention cancella i seq min oltre `retain`.
//! - secondary (tag 1): chiave `(bucket,key)` → `(uuid,size,mtime,head_seq)`.
//!   Stat denormalizzata: `stat` senza toccare il primary.
//! - refcount (tag 2): chiave `[id:8][seq:8]` → count u64 (pin snapshot).
//!
//! Nodi: header payload `[tag:1][nrec:2 LE][_rsv:1]` poi record; INTERNAL con
//! figli + separatori minimi. Valori grandi (> soglia inline) in overflow
//! linkati (nodi RAW `[next:8][chunk:3552]`). Mai tabelle fisse.

use super::format::{
    self, ARCA_NODE_PAYLOAD_LEN, ARCA_NODE_TYPE_INTERNAL, ARCA_NODE_TYPE_LEAF,
    ARCA_NODE_TYPE_RAW,
};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// Tag di albero dentro il payload (primo byte).
pub const TREE_PRIMARY: u8 = 0;
pub const TREE_SECONDARY: u8 = 1;
pub const TREE_REFCOUNT: u8 = 2;
/// Tag tabella snapshot persistente (56.2c): foglie `(key,val)` generiche con
/// chiave `snap/<sid:8 LE>` (riesce il codec `Leaf`, niente formato nuovo).
/// Il blocco meta vive fuori dagli alberi, puntato dal superblock.
pub const TREE_META: u8 = 3;

/// Soglia inline valori primary: oltre si va in overflow linkato.
pub const INLINE_MAX: usize = 512;
/// Chunk dati per blocco overflow (3560 - 8 di next).
pub const OV_CHUNK: usize = 3552;

/// Storage astratto: blocchi numerati (0 = mai valido, come `ArcaVolume`).
/// `read_node` verifica checksum e riporta `(type, gen)`; `write_node`
/// ricalcola header+checksum. Semantica identica a `volume.rs`.
pub trait BlockStore {
    fn read_node(&self, blk: u64, out: &mut [u8; ARCA_NODE_PAYLOAD_LEN])
        -> Option<(u8, u64)>;
    fn write_node(
        &mut self,
        blk: u64,
        ty: u8,
        gen: u64,
        payload: &[u8; ARCA_NODE_PAYLOAD_LEN],
    ) -> bool;
    fn alloc(&mut self) -> Option<u64>;
    fn free(&mut self, blk: u64) -> bool;
}

// ── Codec primitivo (LE esplicito, mai panico su dati disco) ─────────────

fn rd16(b: &[u8], o: usize) -> Option<u16> {
    let s = b.get(o..o + 2)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}
fn rd64(b: &[u8], o: usize) -> Option<u64> {
    let s = b.get(o..o + 8)?;
    Some(u64::from_le_bytes([
        s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
    ]))
}
fn wr16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn wr64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Chiave secondary serializzata `[blen:1][bucket][0][klen:1][key][0]`.
pub fn seckey_encode(bucket: &[u8], key: &[u8]) -> Option<Vec<u8>> {
    if bucket.len() > super::proto::OBJ_BUCKET_MAX
        || key.len() > super::proto::OBJ_KEY_MAX
    {
        return None;
    }
    let mut v = Vec::with_capacity(3 + bucket.len() + key.len());
    v.push(bucket.len() as u8);
    v.extend_from_slice(bucket);
    v.push(0);
    v.push(key.len() as u8);
    v.extend_from_slice(key);
    v.push(0);
    Some(v)
}

/// Parsa una chiave secondary, ritorna `(bucket, key, resto)`.
pub fn seckey_parse(b: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    // Stesso formato di `wire::obj_prefix` (condiviso via riesportazione).
    let (bk, k, rest) = super::wire::parse_obj_prefix(b)?;
    Some((bk, k, rest))
}

// ── Nodi ─────────────────────────────────────────────────────────────────
// Header payload: [tag:1][nrec:2 LE][rsv:1], record da offset 4.

const HDR_LEN: usize = 4;

fn node_nrec(p: &[u8]) -> Option<usize> {
    let n = rd16(p, 1)? as usize;
    if p.get(0).is_none() || p.len() < HDR_LEN {
        return None;
    }
    Some(n)
}
fn node_set_nrec(p: &mut [u8; ARCA_NODE_PAYLOAD_LEN], n: usize) -> Option<()> {
    if n > u16::MAX as usize {
        return None;
    }
    p[1..3].copy_from_slice(&(n as u16).to_le_bytes());
    Some(())
}

/// Record primary encodato:
/// `[id:8][seq:8][mtime:8][kind:1]` + inline `[dlen:2][data]` | overflow `[ov:8][size:8]`.
#[derive(Clone, Debug, PartialEq)]
pub struct PrimRec {
    pub id: u64,
    pub seq: u64,
    pub mtime: u64,
    /// A versione pinnata? No: pin vive nel refcount; qui solo dati.
    pub data_ov: Option<(u64, u64)>, // (ov_head, size) se overflow
    pub data_inline: Vec<u8>,        // vuoto se overflow
}

fn prim_rec_encode(r: &PrimRec) -> Vec<u8> {
    let mut v = Vec::new();
    wr64(&mut v, r.id);
    wr64(&mut v, r.seq);
    wr64(&mut v, r.mtime);
    match r.data_ov {
        Some((ov, size)) => {
            v.push(1);
            wr64(&mut v, ov);
            wr64(&mut v, size);
        }
        None => {
            v.push(0);
            wr16(&mut v, r.data_inline.len() as u16);
            v.extend_from_slice(&r.data_inline);
        }
    }
    v
}

fn prim_rec_parse(b: &[u8]) -> Option<(PrimRec, usize)> {
    let id = rd64(b, 0)?;
    let seq = rd64(b, 8)?;
    let mtime = rd64(b, 16)?;
    let kind = *b.get(24)?;
    if kind == 0 {
        let dlen = rd16(b, 25)? as usize;
        let data = b.get(27..27 + dlen)?.to_vec();
        Some((
            PrimRec { id, seq, mtime, data_ov: None, data_inline: data },
            27 + dlen,
        ))
    } else if kind == 1 {
        let ov = rd64(b, 25)?;
        let size = rd64(b, 33)?;
        Some((
            PrimRec { id, seq, mtime, data_ov: Some((ov, size)), data_inline: Vec::new() },
            41,
        ))
    } else {
        None
    }
}

/// Record secondary:
/// `seckey + [uuid:8][size:8][mtime:8][head_seq:8]` (32 B fissi in coda).
#[derive(Clone, Debug, PartialEq)]
pub struct SecRec {
    pub bucket: Vec<u8>,
    pub key: Vec<u8>,
    pub uuid: u64,
    pub size: u64,
    pub mtime: u64,
    pub head_seq: u64,
}

fn sec_rec_encode(r: &SecRec) -> Option<Vec<u8>> {
    let mut v = seckey_encode(&r.bucket, &r.key)?;
    wr64(&mut v, r.uuid);
    wr64(&mut v, r.size);
    wr64(&mut v, r.mtime);
    wr64(&mut v, r.head_seq);
    Some(v)
}

fn sec_rec_parse(b: &[u8]) -> Option<(SecRec, usize)> {
    let (bk, k, rest) = seckey_parse(b)?;
    let off = b.len() - rest.len();
    if rest.len() < 32 {
        return None;
    }
    let uuid = rd64(rest, 0)?;
    let size = rd64(rest, 8)?;
    let mtime = rd64(rest, 16)?;
    let head_seq = rd64(rest, 24)?;
    Some((
        SecRec {
            bucket: bk.to_vec(), key: k.to_vec(), uuid, size, mtime, head_seq,
        },
        off + 32,
    ))
}

/// Record refcount: `[id:8][seq:8][count:8]` (24 B fissi).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RefRec {
    pub id: u64,
    pub seq: u64,
    pub count: u64,
}

fn ref_rec_encode(r: &RefRec) -> [u8; 24] {
    let mut v = [0u8; 24];
    v[..8].copy_from_slice(&r.id.to_le_bytes());
    v[8..16].copy_from_slice(&r.seq.to_le_bytes());
    v[16..].copy_from_slice(&r.count.to_le_bytes());
    Some(v).map(|x| x).unwrap()
}

fn ref_rec_parse(b: &[u8]) -> Option<(RefRec, usize)> {
    if b.len() < 24 {
        return None;
    }
    Some((
        RefRec { id: rd64(b, 0)?, seq: rd64(b, 8)?, count: rd64(b, 16)? },
        24,
    ))
}

/// Chiave primaria composita `[id:8][seq:8]` (ordinamento (id,seq)).
fn prim_key(id: u64, seq: u64) -> [u8; 16] {
    let mut k = [0u8; 16];
    k[..8].copy_from_slice(&id.to_le_bytes());
    k[8..].copy_from_slice(&seq.to_le_bytes());
    k
}

// ── Foglie: lista ordinata di (chiave, record) ───────────────────────────

/// Foglia generica: chiavi opache ordinate + payload record opachi.
/// Il codec specifico (prim/sec/ref) serializza sopra.
struct Leaf {
    keys: Vec<Vec<u8>>,
    vals: Vec<Vec<u8>>,
}

fn leaf_parse(p: &[u8], tag: u8) -> Option<Leaf> {
    if *p.first()? != tag {
        return None;
    }
    let n = node_nrec(p)?;
    let mut off = HDR_LEN;
    let mut keys = Vec::with_capacity(n.min(256));
    let mut vals = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        let klen = rd16(p, off)? as usize;
        off += 2;
        let k = p.get(off..off + klen)?.to_vec();
        off += klen;
        let vlen = rd16(p, off)? as usize;
        off += 2;
        let v = p.get(off..off + vlen)?.to_vec();
        off += vlen;
        keys.push(k);
        vals.push(v);
    }
    Some(Leaf { keys, vals })
}

/// Codifica foglia in un payload heap (mai 3.5K stack/return — regola §18).
fn leaf_encode(tag: u8, leaf: &Leaf) -> Option<Box<[u8; ARCA_NODE_PAYLOAD_LEN]>> {
    let mut p = format::boxed_node();
    p[0] = tag;
    node_set_nrec(&mut p, leaf.keys.len())?;
    let mut off = HDR_LEN;
    for (k, v) in leaf.keys.iter().zip(leaf.vals.iter()) {
        if k.len() > u16::MAX as usize || v.len() > u16::MAX as usize {
            return None;
        }
        let need = 2 + k.len() + 2 + v.len();
        if off + need > ARCA_NODE_PAYLOAD_LEN {
            return None;
        }
        p[off..off + 2].copy_from_slice(&(k.len() as u16).to_le_bytes());
        off += 2;
        p[off..off + k.len()].copy_from_slice(k);
        off += k.len();
        p[off..off + 2].copy_from_slice(&(v.len() as u16).to_le_bytes());
        off += 2;
        p[off..off + v.len()].copy_from_slice(v);
        off += v.len();
    }
    Some(p)
}

/// Posizione di `key` in foglia (binary search): `(found_idx | insert_idx)`.
fn leaf_pos(leaf: &Leaf, key: &[u8]) -> Result<usize, usize> {
    leaf.keys.binary_search_by(|k| k.as_slice().cmp(key))
}

// ── Interni: figli + separatori ──────────────────────────────────────────
// Layout: `[tag:1][nchild:2 LE][rsv:1]` poi `[child:8 × n]` poi
// `(n-1) × [slen:2][sep:slen]`.

struct Internal {
    children: Vec<u64>,
    seps: Vec<Vec<u8>>, // len = children.len()-1
}

fn internal_parse(p: &[u8], tag: u8) -> Option<Internal> {
    if *p.first()? != tag {
        return None;
    }
    let n = node_nrec(p)?; // riuso nrec = nchild
    if n == 0 || n > 512 {
        return None;
    }
    let mut off = HDR_LEN;
    let mut children = Vec::with_capacity(n.min(64));
    for _ in 0..n {
        children.push(rd64(p, off)?);
        off += 8;
    }
    let mut seps = Vec::with_capacity(n.saturating_sub(1).min(64));
    for _ in 0..n.saturating_sub(1) {
        let sl = rd16(p, off)? as usize;
        off += 2;
        seps.push(p.get(off..off + sl)?.to_vec());
        off += sl;
    }
    Some(Internal { children, seps })
}

/// Come `leaf_encode` per gli interni (payload heap, regola §18).
fn internal_encode(tag: u8, it: &Internal) -> Option<Box<[u8; ARCA_NODE_PAYLOAD_LEN]>> {
    if it.children.is_empty()
        || it.children.len() > 512
        || it.seps.len() + 1 != it.children.len()
    {
        return None;
    }
    let mut p = format::boxed_node();
    p[0] = tag;
    node_set_nrec(&mut p, it.children.len())?;
    let mut off = HDR_LEN;
    for c in it.children.iter() {
        if off + 8 > ARCA_NODE_PAYLOAD_LEN {
            return None;
        }
        p[off..off + 8].copy_from_slice(&c.to_le_bytes());
        off += 8;
    }
    for s in it.seps.iter() {
        if s.len() > u16::MAX as usize || off + 2 + s.len() > ARCA_NODE_PAYLOAD_LEN {
            return None;
        }
        p[off..off + 2].copy_from_slice(&(s.len() as u16).to_le_bytes());
        off += 2;
        p[off..off + s.len()].copy_from_slice(s);
        off += s.len();
    }
    Some(p)
}

/// Figlio da seguire per `key`: primo separatore > key → sx, altrimenti dx.
fn internal_pick(it: &Internal, key: &[u8]) -> usize {
    let mut i = 0;
    while i < it.seps.len() && it.seps[i].as_slice() <= key {
        i += 1;
    }
    i
}

// ── Overflow: catena RAW `[next:8][chunk:3552]` ───────────────────────────

fn ov_write<S: BlockStore>(
    store: &mut S,
    gen: u64,
    data: &[u8],
) -> Option<(u64, u64)> {
    if data.is_empty() {
        return None;
    }
    let mut head = 0u64;
    let mut prev = 0u64;
    let mut rest = data;
    let size = data.len() as u64;
    while !rest.is_empty() {
        let take = rest.len().min(OV_CHUNK);
        let blk = store.alloc()?;
        if blk == 0 {
            return None;
        }
        // Buffer heap (regola §18: 3.5K stack qui + catena write sotto).
        let mut payload = format::boxed_node();
        payload[..8].copy_from_slice(&0u64.to_le_bytes()); // next, patchato sotto
        payload[8..8 + take].copy_from_slice(&rest[..take]);
        if !store.write_node(blk, ARCA_NODE_TYPE_RAW, gen, &payload) {
            return None;
        }
        if prev != 0 {
            // Patch next del precedente: rileggi, aggiorna, riscrivi.
            let mut pb = format::boxed_node();
            store.read_node(prev, &mut pb)?;
            pb[..8].copy_from_slice(&blk.to_le_bytes());
            if !store.write_node(prev, ARCA_NODE_TYPE_RAW, gen, &pb) {
                return None;
            }
        } else {
            head = blk;
        }
        prev = blk;
        rest = &rest[take..];
    }
    Some((head, size))
}

fn ov_read<S: BlockStore>(store: &S, head: u64, size: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(size.min(1 << 20));
    let mut cur = head;
    let mut left = size;
    // Guardia anti-loop su catena corrotta: mai più blocchi dei byte/8.
    let mut guard = size / 8 + 16;
    // Buffer heap (regola §18, vedi `ov_write`).
    let mut payload = format::boxed_node();
    while cur != 0 && left > 0 {
        if guard == 0 {
            return None;
        }
        guard -= 1;
        let (ty, _) = store.read_node(cur, &mut payload)?;
        if ty != ARCA_NODE_TYPE_RAW {
            return None;
        }
        let next = rd64(&payload[..], 0)?;
        let take = left.min(OV_CHUNK);
        out.extend_from_slice(payload.get(8..8 + take)?);
        left -= take;
        cur = next;
    }
    if left != 0 || out.len() != size {
        return None;
    }
    Some(out)
}

fn ov_free<S: BlockStore>(store: &mut S, head: u64) {
    let mut cur = head;
    let mut guard = 1 << 20; // mai loop infinito su catena corrotta
    let mut payload = format::boxed_node();
    while cur != 0 && guard > 0 {
        guard -= 1;
        let next = match store.read_node(cur, &mut payload) {
            Some((ARCA_NODE_TYPE_RAW, _)) => rd64(&payload[..], 0).unwrap_or(0),
            _ => 0,
        };
        store.free(cur);
        cur = next;
    }
}

// ── Snapshot (tabella in RAM nel motore; persistita dal driver al commit) ─

/// Snapshot per-bucket: pin delle head al momento della creazione.
/// `entries`: (seckey, uuid, seq pinnato). Il pin vive nel refcount su disco.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub id: u64,
    pub bucket: Vec<u8>,
    pub tick: u64,
    pub entries: Vec<(Vec<u8>, u64, u64)>,
}

/// Motore B+tree COW: tre radici + allocazione id/snapshot + tabella snapshot.
/// `S` = store blocchi (volume reale o `MemStore` nei test host).
pub struct BTree<S: BlockStore> {
    pub store: S,
    pub gen: u64,
    pub root_primary: u64,
    pub root_secondary: u64,
    pub root_refcount: u64,
    /// Blocco tabella snapshot persistente (0 = nessuna). Scritto dal commit
    /// insieme alle radici (mai generazioni diverse); i blocchi meta vecchi
    /// restano orfani per la GC (mai free eager cross-handle: la guardia live
    /// del volume non li conosce dopo un restart).
    pub meta_root: u64,
    pub next_id: u64,
    pub next_snap: u64,
    pub snaps: BTreeMap<u64, Snapshot>,
    /// Indice inverso id → seckey (F2: id mai riusati). Persistito dal driver.
    pub by_id: BTreeMap<u64, Vec<u8>>,
}

impl<S: BlockStore> BTree<S> {
    /// Crea un indice vuoto: tre foglie vuote + id/snap da 1.
    pub fn new(mut store: S, gen: u64) -> Option<Self> {
        let rp = store.alloc()?;
        let rs = store.alloc()?;
        let rr = store.alloc()?;
        if rp == 0 || rs == 0 || rr == 0 {
            return None;
        }
        let mut me = Self {
            store, gen, root_primary: rp, root_secondary: rs, root_refcount: rr,
            meta_root: 0, next_id: 1, next_snap: 1, snaps: BTreeMap::new(),
            by_id: BTreeMap::new(),
        };
        me.fmt_empty(rp, TREE_PRIMARY)?;
        me.fmt_empty(rs, TREE_SECONDARY)?;
        me.fmt_empty(rr, TREE_REFCOUNT)?;
        Some(me)
    }

    /// Costruisce il motore su radici esistenti (bind 56.2b: i blocchi sono
    /// gia' allocati e formattati, il driver li passa dopo averli validati).
    /// `next_id/next_snap` partono da 1: il driver li sincronizza subito dopo
    /// dall'header-ext (fonte persistente) e chiama `rebuild_by_id`.
    #[inline(never)]
    pub fn new_with_roots(
        store: S,
        gen: u64,
        root_primary: u64,
        root_secondary: u64,
        root_refcount: u64,
    ) -> Option<Self> {
        if root_primary == 0 || root_secondary == 0 || root_refcount == 0 {
            return None; // blocco 0 mai valido, come `ArcaVolume`
        }
        Some(Self {
            store, gen, root_primary, root_secondary, root_refcount,
            meta_root: 0, next_id: 1, next_snap: 1, snaps: BTreeMap::new(),
            by_id: BTreeMap::new(),
        })
    }

    /// Restituisce lo store (test host: simula restart riusando i blocchi).
    #[inline(never)]
    pub fn into_store(self) -> S {
        self.store
    }

    /// Ricostruisce l'indice inverso id → seckey passeggiando il secondary
    /// (bind dopo kill/restart: la tabella in-RAM e' persa, i dati no).
    /// La tabella snapshot NON si ricostruisce (pin persi: documentato 56.2c).
    #[inline(never)]
    pub fn rebuild_by_id(&mut self) -> Option<()> {
        self.by_id.clear();
        let root = self.root_secondary;
        let mut stack = Vec::from([root]);
        let mut guard = 1 << 16;
        while let Some(blk) = stack.pop() {
            if guard == 0 {
                return None;
            }
            guard -= 1;
            let (ty, tag, _, p) = self.node_read(blk)?;
            if tag != TREE_SECONDARY {
                continue;
            }
            if ty == ARCA_NODE_TYPE_LEAF {
                if let Some(leaf) = leaf_parse(&p[..], TREE_SECONDARY) {
                    for (k, v) in leaf.keys.iter().zip(leaf.vals.iter()) {
                        if let Some((sec, _)) = sec_rec_parse(v) {
                            self.by_id.insert(sec.uuid, k.clone());
                        }
                    }
                }
            } else if let Some(it) = internal_parse(&p[..], TREE_SECONDARY) {
                stack.extend(it.children.iter().copied());
            }
        }
        Some(())
    }

    // ── Tabella snapshot persistente (56.2c, blocco meta TREE_META) ─────
    // Chiave `snap/<sid:8 LE>` (13 B); valore `[blen:1][bucket][tick:8][n:4]`
    // poi n × `[uuid:8][seq:8][sklen:2][seckey]`. Tutto a lunghezze esplicite,
    // verificato al load: meta corrotta = tabella ignorata loud (dati
    // intatti), mai mount rifiutato per la retention.

    /// Serializza la tabella snapshot in coppie (key, val). `None` solo oltre
    /// i bound nomi (mai troncamenti).
    #[inline(never)]
    fn snaps_encode(&self) -> Option<Vec<(Vec<u8>, Vec<u8>)>> {
        let mut out = Vec::with_capacity(self.snaps.len());
        for (sid, snap) in self.snaps.iter() {
            let mut k = Vec::with_capacity(13);
            k.extend_from_slice(b"snap/");
            k.extend_from_slice(&sid.to_le_bytes());
            let mut v = Vec::new();
            if snap.bucket.len() > super::proto::OBJ_BUCKET_MAX {
                return None;
            }
            v.push(snap.bucket.len() as u8);
            v.extend_from_slice(&snap.bucket);
            v.extend_from_slice(&snap.tick.to_le_bytes());
            if snap.entries.len() > u32::MAX as usize {
                return None;
            }
            v.extend_from_slice(&(snap.entries.len() as u32).to_le_bytes());
            for (sk, uuid, seq) in snap.entries.iter() {
                v.extend_from_slice(&uuid.to_le_bytes());
                v.extend_from_slice(&seq.to_le_bytes());
                if sk.len() > u16::MAX as usize {
                    return None;
                }
                v.extend_from_slice(&(sk.len() as u16).to_le_bytes());
                v.extend_from_slice(sk);
            }
            out.push((k, v));
        }
        Some(out)
    }

    /// Sostituisce la tabella con le coppie lette dal blocco meta. Qualunque
    /// malformazione → `None` e tabella INVARIATA (il chiamante monta senza
    /// snapshot, loud). `next_snap` resta al chiamante (header-ext).
    #[inline(never)]
    pub fn snaps_decode(&mut self, entries: &[(Vec<u8>, Vec<u8>)]) -> Option<()> {
        let mut table = BTreeMap::new();
        for (k, v) in entries.iter() {
            if k.len() != 13 || k[..5] != *b"snap/" {
                return None;
            }
            let mut sidb = [0u8; 8];
            sidb.copy_from_slice(&k[5..13]);
            let sid = u64::from_le_bytes(sidb);
            if sid == 0 {
                return None;
            }
            let blen = *v.first()? as usize;
            if blen > super::proto::OBJ_BUCKET_MAX {
                return None;
            }
            let bucket = v.get(1..1 + blen)?.to_vec();
            let rest = v.get(1 + blen..)?;
            if rest.len() < 12 {
                return None;
            }
            let mut tickb = [0u8; 8];
            tickb.copy_from_slice(&rest[..8]);
            let tick = u64::from_le_bytes(tickb);
            let n = u32::from_le_bytes([rest[8], rest[9], rest[10], rest[11]]) as usize;
            let mut cur = &rest[12..];
            let mut ents = Vec::with_capacity(n.min(1024));
            for _ in 0..n {
                if cur.len() < 18 {
                    return None;
                }
                let mut u = [0u8; 8];
                let mut s = [0u8; 8];
                u.copy_from_slice(&cur[..8]);
                s.copy_from_slice(&cur[8..16]);
                let sklen = u16::from_le_bytes([cur[16], cur[17]]) as usize;
                cur = cur.get(18..)?;
                let sk = cur.get(..sklen)?.to_vec();
                cur = cur.get(sklen..)?;
                // La seckey deve parsare (bucket/key entro bound): niente
                // chiavi fantasma nella tabella; uuid 0 mai valido (F2).
                let (_, _, _) = seckey_parse(&sk)?;
                if u == [0; 8] {
                    return None;
                }
                ents.push((sk, u64::from_le_bytes(u), u64::from_le_bytes(s)));
            }
            if !cur.is_empty() {
                return None; // trailing: formato futuro, rifiuta loud oggi
            }
            if table.insert(sid, Snapshot { id: sid, bucket, tick, entries: ents }).is_some() {
                return None; // sid duplicato: corrotto
            }
        }
        self.snaps = table;
        Some(())
    }

    /// Scrive la tabella nel blocco meta (allocato fresco): aggiorna
    /// `meta_root`, il vecchio resta orfano per la GC (mai free eager
    /// cross-handle). Blocco singolo: oltre capacita' → `None` loud
    /// (scala gate: irraggiungibile; multi-blocco fuori scope 56).
    #[inline(never)]
    pub fn meta_store(&mut self) -> Option<()> {
        let entries = self.snaps_encode()?;
        let leaf = Leaf {
            keys: entries.iter().map(|(k, _)| k.clone()).collect(),
            vals: entries.iter().map(|(_, v)| v.clone()).collect(),
        };
        let enc = leaf_encode(TREE_META, &leaf)?;
        let nb = self.store.alloc()?;
        if nb == 0 {
            return None;
        }
        if !self.store.write_node(nb, ARCA_NODE_TYPE_LEAF, self.gen, &enc) {
            self.store.free(nb);
            return None;
        }
        self.meta_root = nb;
        Some(())
    }

    /// Carica la tabella da `meta_root` (0 = nessuna: tabella vuota, Ok).
    /// Verifica tipo LEAF + tag META; foglia malformata o tabella malformata
    /// → `None` (il chiamante monta senza snapshot, loud).
    #[inline(never)]
    pub fn meta_load(&mut self) -> Option<()> {
        if self.meta_root == 0 {
            self.snaps.clear();
            return Some(());
        }
        let (ty, tag, _, p) = self.node_read(self.meta_root)?;
        if ty != ARCA_NODE_TYPE_LEAF || tag != TREE_META {
            return None;
        }
        let leaf = leaf_parse(&p[..], TREE_META)?;
        let pairs: Vec<(Vec<u8>, Vec<u8>)> =
            leaf.keys.into_iter().zip(leaf.vals.into_iter()).collect();
        self.snaps_decode(&pairs)
    }

    // ── Reachability per orphan-GC (56.2c) ──────────────────────────────
    // Blocchi raggiungibili = nodi dei 3 alberi + catene overflow dei valori
    // primary. I pin snapshot non sganciano mai i record dal primary: la walk
    // li trova comunque (la tabella serve a rollback/clone, non alla GC).

    /// Tutti i blocchi raggiungibili dalle radici (ordinati, dedupati).
    /// Walk iterativa con guardia visited (puntatori corrotti: mai loop).
    #[inline(never)]
    pub fn reachable_blocks(&self) -> Option<Vec<u64>> {
        let mut out = Vec::new();
        for (root, tag) in [
            (self.root_primary, TREE_PRIMARY),
            (self.root_secondary, TREE_SECONDARY),
            (self.root_refcount, TREE_REFCOUNT),
        ] {
            self.walk_tree(root, tag, &mut out)?;
        }
        // Catene overflow dei valori primary (ogni record di ogni foglia).
        // DISCRIMINA per `ty`: il trial-parse di un interno su byte di foglia
        // puo' riuscire spurio e iniettare figli spazzatura (osservato: walk
        // fallita su albero valido). Mai trial-parse senza tipo.
        let mut stack = alloc::vec![self.root_primary];
        let mut seen: Vec<u64> = Vec::new();
        while let Some(blk) = stack.pop() {
            if blk == 0 || seen.contains(&blk) {
                continue;
            }
            seen.push(blk);
            let (ty, tg, _, p) = self.node_read(blk)?;
            if tg != TREE_PRIMARY {
                continue;
            }
            if ty == ARCA_NODE_TYPE_INTERNAL {
                let it = internal_parse(&p[..], TREE_PRIMARY)?;
                stack.extend(it.children.iter().copied());
                continue;
            }
            if ty != ARCA_NODE_TYPE_LEAF {
                return None;
            }
            let leaf = leaf_parse(&p[..], TREE_PRIMARY)?;
            for v in leaf.vals.iter() {
                let (rec, _) = prim_rec_parse(v)?;
                if let Some((ov, _)) = rec.data_ov {
                    self.walk_ov(ov, &mut out)?;
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        Some(out)
    }

    /// Walk di un albero: accumula i blocchi nodo in `out` (senza dedup qui).
    #[inline(never)]
    fn walk_tree(&self, root: u64, tag: u8, out: &mut Vec<u64>) -> Option<()> {
        let mut stack = alloc::vec![root];
        let mut guard = 1usize << 20;
        while let Some(blk) = stack.pop() {
            if guard == 0 {
                return None;
            }
            guard -= 1;
            if blk == 0 || out.contains(&blk) {
                continue;
            }
            out.push(blk);
            let (ty, tg, _, p) = self.node_read(blk)?;
            if tg != tag {
                return None; // tag inatteso: struttura corrotta, loud
            }
            if ty == ARCA_NODE_TYPE_INTERNAL {
                let it = internal_parse(&p[..], tag)?;
                stack.extend(it.children.iter().copied());
            } else if ty != ARCA_NODE_TYPE_LEAF {
                return None;
            }
        }
        Some(())
    }

    /// Walk di una catena overflow: accumula i blocchi in `out`.
    #[inline(never)]
    fn walk_ov(&self, head: u64, out: &mut Vec<u64>) -> Option<()> {
        let mut cur = head;
        let mut guard = 1usize << 20;
        let mut payload = format::boxed_node();
        while cur != 0 {
            if guard == 0 {
                return None;
            }
            guard -= 1;
            if out.contains(&cur) {
                return None; // ciclo in catena: corrotto, loud
            }
            out.push(cur);
            let (ty, _) = self.store.read_node(cur, &mut payload)?;
            if ty != ARCA_NODE_TYPE_RAW {
                return None;
            }
            cur = rd64(&payload[..], 0)?;
        }
        Some(())
    }

    /// Formatta le tre radici come foglie vuote (init volume fresco 56.2b:
    /// i blocchi sono gia' allocati dal driver, qui solo la formattazione).
    #[inline(never)]
    pub fn init_empty_roots(&mut self) -> Option<()> {
        let (rp, rs, rr) = (self.root_primary, self.root_secondary, self.root_refcount);
        self.fmt_empty(rp, TREE_PRIMARY)?;
        self.fmt_empty(rs, TREE_SECONDARY)?;
        self.fmt_empty(rr, TREE_REFCOUNT)?;
        Some(())
    }

    #[inline(never)]
    fn fmt_empty(&mut self, blk: u64, tag: u8) -> Option<()> {
        let mut p = format::boxed_node();
        p[0] = tag;
        // nrec = 0 e resto zero: foglia vuota (boxed_node azzera gia').
        if self.store.write_node(blk, ARCA_NODE_TYPE_LEAF, self.gen, &p) {
            Some(())
        } else {
            None
        }
    }

    /// Legge un nodo: `(type, tag, gen, payload)`. None se illeggibile.
    /// Payload in `Box` heap (mai 3.5K stack/return per-valore — regola §18:
    /// sotto stanno altri frame con buffer, l'annidamento sfonda i 16 KiB).
    #[inline(never)]
    fn node_read(&self, blk: u64) -> Option<(u8, u8, u64, Box<[u8; ARCA_NODE_PAYLOAD_LEN]>)> {
        let mut p = format::boxed_node();
        let (ty, generation) = self.store.read_node(blk, &mut p)?;
        let tag = *p.first()?;
        if ty != ARCA_NODE_TYPE_LEAF && ty != ARCA_NODE_TYPE_INTERNAL {
            return None;
        }
        Some((ty, tag, generation, p))
    }

    /// Lookup generico: foglia + posizione per `key` nell'albero `root/tag`.
    #[inline(never)]
    fn btree_lookup(&self, root: u64, tag: u8, key: &[u8]) -> Option<(u64, Leaf, usize, bool)> {
        let mut cur = root;
        let mut guard = 64;
        loop {
            if guard == 0 {
                return None;
            }
            guard -= 1;
            let (ty, _tg, _gen, p) = self.node_read(cur)?;
            if ty == ARCA_NODE_TYPE_LEAF {
                let leaf = leaf_parse(&p[..], tag)?;
                return match leaf_pos(&leaf, key) {
                    Ok(i) => Some((cur, leaf, i, true)),
                    Err(i) => Some((cur, leaf, i, false)),
                };
            }
            let it = internal_parse(&p[..], tag)?;
            if it.children.is_empty() {
                return None;
            }
            cur = *it.children.get(internal_pick(&it, key))?;
        }
    }

    /// Path radice→foglia per `key`: lista (nodo, indice-figlio-scelto).
    #[inline(never)]
    fn btree_path(&self, root: u64, tag: u8, key: &[u8]) -> Option<Vec<(u64, usize)>> {
        let mut path = Vec::new();
        let mut cur = root;
        let mut guard = 64;
        loop {
            if guard == 0 {
                return None;
            }
            guard -= 1;
            let (ty, _tg, _gen, p) = self.node_read(cur)?;
            if ty == ARCA_NODE_TYPE_LEAF {
                return Some(path);
            }
            let it = internal_parse(&p[..], tag)?;
            let pick = internal_pick(&it, key);
            let nxt = *it.children.get(pick)?;
            path.push((cur, pick));
            cur = nxt;
        }
    }

    /// Alloca un blocco e ci scrive il payload: `(blocco, ())` o `None`.
    /// I blocchi allocati qui e mai linkati vanno liberati dal chiamante
    /// a errore (igiene freelist: niente leak sul path di fallimento).
    #[inline(never)]
    fn emit(&mut self, ty: u8, payload: &[u8; ARCA_NODE_PAYLOAD_LEN]) -> Option<u64> {
        let b = self.store.alloc()?;
        if b == 0 {
            return None;
        }
        if !self.store.write_node(b, ty, self.gen, payload) {
            self.store.free(b);
            return None;
        }
        Some(b)
    }

    /// Scrive una foglia (split se necessario): `(sx, Option<(sep, dx)>)`.
    /// Entrambi i blocchi sono nuovi (COW); il vecchio va liberato dal
    /// chiamante SOLO a commit riuscito (altrimenti la generazione vecchia
    /// resta valida: vedi ordine write §3).
    #[inline(never)]
    fn write_leaf_split(&mut self, tag: u8, leaf: &Leaf) -> Option<(u64, Option<(Vec<u8>, u64)>)> {
        if let Some(enc) = leaf_encode(tag, leaf) {
            return Some((self.emit(ARCA_NODE_TYPE_LEAF, &enc)?, None));
        }
        if leaf.keys.len() < 2 {
            return None; // singolo record > nodo: chiave/valore troppo grande
        }
        let mid = leaf.keys.len() / 2;
        let (lk, lv) = (leaf.keys[..mid].to_vec(), leaf.vals[..mid].to_vec());
        let (rk, rv) = (leaf.keys[mid..].to_vec(), leaf.vals[mid..].to_vec());
        let (left, right) = (Leaf { keys: lk, vals: lv }, Leaf { keys: rk, vals: rv });
        // Emit sequenziali in blocchi separati: MAI due payload 3.5K vivi
        // nello stesso frame (7K transienti + catena IO = #PF — regola §18).
        let lb = {
            let le = leaf_encode(tag, &left)?;
            self.emit(ARCA_NODE_TYPE_LEAF, &le)?
        };
        match {
            let re = leaf_encode(tag, &right)?;
            self.emit(ARCA_NODE_TYPE_LEAF, &re)
        } {
            Some(rb) => Some((lb, Some((right.keys[0].clone(), rb)))),
            None => {
                self.store.free(lb);
                None
            }
        }
    }

    /// Riscrive i padri COW dal basso verso l'alto: `child_new` rimpiazza il
    /// figlio in `path[last]`; `up` (split del livello sotto) inserisce
    /// `(sep → rblk)` nel padre. Ritorna la nuova root (nuova se split).
    #[inline(never)]
    fn rewrite_parents(
        &mut self,
        path: &[(u64, usize)],
        tag: u8,
        mut child_new: u64,
        mut up: Option<(Vec<u8>, u64)>,
    ) -> Option<u64> {
        if path.is_empty() {
            // Root-foglia: se ha splittato, nuova root interna.
            if let Some((sep, rb)) = up {
                let it = Internal { children: alloc::vec![child_new, rb], seps: alloc::vec![sep] };
                let enc = internal_encode(tag, &it)?;
                return self.emit(ARCA_NODE_TYPE_INTERNAL, &enc);
            }
            return Some(child_new);
        }
        for (parent_blk, idx) in path.iter().rev() {
            let (_, _, _, pp) = self.node_read(*parent_blk)?;
            let mut it = internal_parse(&pp[..], tag)?;
            if *idx >= it.children.len() {
                return None; // path stale: mai riscrivere, fallisci loud
            }
            it.children[*idx] = child_new;
            if let Some((sep, rb)) = up.take() {
                it.seps.insert(*idx.min(&it.seps.len()), sep);
                it.children.insert(idx + 1, rb);
            }
            if let Some(enc) = internal_encode(tag, &it) {
                child_new = self.emit(ARCA_NODE_TYPE_INTERNAL, &enc)?;
                up = None;
                continue;
            }
            // Split interno per conteggio figli; il separatore mediano sale.
            let n = it.children.len();
            if n < 3 {
                return None; // due figli che non ci stanno: separatori enormi
            }
            let mid = n / 2;
            let r_children = it.children.split_off(mid);
            // Seps: sx tiene [..mid-1], il mediano sale, dx tiene [mid..].
            let promoted = it.seps.remove(mid - 1);
            let r_seps = it.seps.split_off(mid - 1);
            let (left, right) = (
                Internal { children: it.children, seps: it.seps },
                Internal { children: r_children, seps: r_seps },
            );
            // Come sopra: emit sequenziali, mai 2×3.5K vivi in frame.
            let lb = {
                let le = internal_encode(tag, &left)?;
                self.emit(ARCA_NODE_TYPE_INTERNAL, &le)?
            };
            match {
                let re = internal_encode(tag, &right)?;
                self.emit(ARCA_NODE_TYPE_INTERNAL, &re)
            } {
                Some(rb) => {
                    child_new = lb;
                    up = Some((promoted, rb));
                }
                None => {
                    self.store.free(lb);
                    return None;
                }
            }
        }
        // Split arrivato in radice: nuova root.
        if let Some((sep, rb)) = up {
            let it = Internal { children: alloc::vec![child_new, rb], seps: alloc::vec![sep] };
            let enc = internal_encode(tag, &it)?;
            return self.emit(ARCA_NODE_TYPE_INTERNAL, &enc);
        }
        Some(child_new)
    }

    /// Insert COW di (key,val) nell'albero `root/tag`; ritorna la nuova root.
    /// Chiave esistente = sostituzione valore (upsert).
    /// Foglia target per `key`: figlio dell'ultimo interno del path (o la
    /// root se l'albero e' una foglia singola). `btree_path` NON include la
    /// foglia: `path.last()` e' l'interno padre, mai la foglia stessa.
    #[inline(never)]
    fn leaf_of_path(&self, root: u64, tag: u8, path: &[(u64, usize)]) -> Option<u64> {
        match path.last() {
            Some((blk, idx)) => {
                let (_, _, _, p) = self.node_read(*blk)?;
                let it = internal_parse(&p[..], tag)?;
                Some(*it.children.get(*idx)?)
            }
            None => Some(root),
        }
    }

    #[inline(never)]
    fn btree_insert(
        &mut self,
        root: u64,
        tag: u8,
        key: Vec<u8>,
        val: Vec<u8>,
    ) -> Option<u64> {
        let path = self.btree_path(root, tag, &key)?;
        let leaf_blk = self.leaf_of_path(root, tag, &path)?;
        let (_, _, _, p) = self.node_read(leaf_blk)?;
        let mut leaf = leaf_parse(&p[..], tag)?;
        match leaf_pos(&leaf, &key) {
            Ok(i) => leaf.vals[i] = val,
            Err(i) => {
                leaf.keys.insert(i, key);
                leaf.vals.insert(i, val);
            }
        }
        let (child_new, up) = self.write_leaf_split(tag, &leaf)?;
        self.rewrite_parents(&path, tag, child_new, up)
    }

    /// Remove COW di `key` dall'albero `root/tag`: `(nuova_root, rimosso?)`.
    /// Merge SOLO a foglia vuota (correttezza: mai foglie vuote non-root);
    /// il merge generale per bassa fill e il borrow sono tuning futuri
    /// (non toccano la correttezza del lookup, solo l'efficienza spazio).
    #[inline(never)]
    fn btree_remove(&mut self, root: u64, tag: u8, key: &[u8]) -> Option<(u64, bool)> {
        let path = self.btree_path(root, tag, key)?;
        let leaf_blk = self.leaf_of_path(root, tag, &path)?;
        let (_, _, _, p) = self.node_read(leaf_blk)?;
        let mut leaf = leaf_parse(&p[..], tag)?;
        let i = match leaf_pos(&leaf, key) {
            Err(_) => return Some((root, false)), // no-op: ZERO scritture
            Ok(i) => i,
        };
        leaf.keys.remove(i);
        leaf.vals.remove(i);
        if path.is_empty() {
            // Root-foglia: anche vuota resta valida (albero vuoto).
            let enc = leaf_encode(tag, &leaf)?;
            let nb = self.emit(ARCA_NODE_TYPE_LEAF, &enc)?;
            return Some((nb, true));
        }
        if !leaf.keys.is_empty() {
            let (child_new, up) = self.write_leaf_split(tag, &leaf)?;
            debug_assert!(up.is_none(), "remove non cresce: split impossibile");
            let nr = self.rewrite_parents(&path, tag, child_new, up)?;
            return Some((nr, true));
        }
        // Foglia svuotata: fondila col fratello e rimuovi il separatore.
        self.remove_merge_empty(root, tag, &path)
    }

    /// Fonde una foglia svuotata col fratello (sx se esiste, altrimenti dx),
    /// rimuove il separatore dal padre e riscrive COW verso l'alto. Se la
    /// radice resta con un figlio solo, collassa (altezza -1).
    #[inline(never)]
    fn remove_merge_empty(
        &mut self,
        _root: u64,
        tag: u8,
        path: &[(u64, usize)],
    ) -> Option<(u64, bool)> {
        let (parent_blk, idx) = *path.last()?;
        let (_, _, _, pp) = self.node_read(parent_blk)?;
        let mut it = internal_parse(&pp[..], tag)?;
        if it.children.len() < 2 {
            return None; // padre degenerato: mai riscrivere
        }
        // Fratello: sx se esiste, altrimenti dx. Il figlio vuoto sparisce.
        let (sib_idx, sep_idx) = if idx > 0 { (idx - 1, idx - 1) } else { (1, 0) };
        let sib_blk = *it.children.get(sib_idx)?;
        let (_, _, _, sp) = self.node_read(sib_blk)?;
        let sib = leaf_parse(&sp[..], tag)?;
        if sib.keys.is_empty() {
            return None; // due foglie vuote adiacenti: corruzione, fallisci loud
        }
        let nb = self.emit(ARCA_NODE_TYPE_LEAF, &*leaf_encode(tag, &sib)?)?;
        // Il padre perde il figlio vuoto (idx) e il separatore adiacente.
        let lost = if sib_idx < idx { idx } else { idx };
        it.children.remove(lost);
        if sep_idx < it.seps.len() {
            it.seps.remove(sep_idx);
        }
        // Punta il fratello al nuovo blocco.
        let sib_pos = if sib_idx < idx { sib_idx } else { sib_idx - 1 };
        if sib_pos < it.children.len() {
            it.children[sib_pos] = nb;
        } else {
            return None;
        }
        if path.len() == 1 {
            // Il padre e' la radice.
            if it.children.len() == 1 {
                // Collasso: lafoglia fratello diventa radice (libera il padre
                // solo al commit: qui resta orfano per la generazione vecchia).
                return Some((nb, true));
            }
            let enc = internal_encode(tag, &it)?;
            let nr = self.emit(ARCA_NODE_TYPE_INTERNAL, &enc)?;
            return Some((nr, true));
        }
        // Padre non-root con 1 figlio: degenerazione profonda — non supportata
        // in 56.2b (richiede merge interno a cascata; a scala gate i merge
        // partono da foglie con padri ramificati: mai osservato). Fallisci
        // loud senza scrivere nulla di linkato (nb resta orfano temporaneo,
        // recuperato dalla orphan-GC 56.2c).
        if it.children.len() < 2 {
            self.store.free(nb);
            return None;
        }
        let enc = internal_encode(tag, &it)?;
        let pb_new = self.emit(ARCA_NODE_TYPE_INTERNAL, &enc)?;
        let nr = self.rewrite_parents(&path[..path.len() - 1], tag, pb_new, None)?;
        Some((nr, true))
    }

    // ── Refcount per-versione (albero tag 2, chiave [id:8][seq:8]) ──────

    #[inline(never)]
    fn ref_get(&self, id: u64, seq: u64) -> u64 {
        let mut k = [0u8; 16];
        k[..8].copy_from_slice(&id.to_le_bytes());
        k[8..].copy_from_slice(&seq.to_le_bytes());
        match self.btree_lookup(self.root_refcount, TREE_REFCOUNT, &k) {
            Some((_, leaf, i, true)) => ref_rec_parse(&leaf.vals[i])
                .map(|(r, _)| r.count)
                .unwrap_or(0),
            _ => 0,
        }
    }

    #[inline(never)]
    fn ref_set(&mut self, id: u64, seq: u64, count: u64) -> Option<()> {
        let mut k = [0u8; 16];
        k[..8].copy_from_slice(&id.to_le_bytes());
        k[8..].copy_from_slice(&seq.to_le_bytes());
        let r = RefRec { id, seq, count };
        let nr = self.btree_insert(self.root_refcount, TREE_REFCOUNT, k.to_vec(), ref_rec_encode(&r).to_vec())?;
        self.root_refcount = nr;
        Some(())
    }

    #[inline(never)]
    fn ref_inc(&mut self, id: u64, seq: u64) -> Option<()> {
        let c = self.ref_get(id, seq);
        self.ref_set(id, seq, c + 1)
    }

    /// Decrementa; a zero rimuove il record E libera i blocchi valore.
    /// Ritorna `true` se il conteggio ha raggiunto zero (chiamante: trim).
    #[inline(never)]
    fn ref_dec_free(&mut self, id: u64, seq: u64) -> Option<bool> {
        let c = self.ref_get(id, seq);
        if c <= 1 {
            // Ultimo pin (o mai pinnato): cancella il record refcount.
            let mut k = [0u8; 16];
            k[..8].copy_from_slice(&id.to_le_bytes());
            k[8..].copy_from_slice(&seq.to_le_bytes());
            let (nr, _) = self.btree_remove(self.root_refcount, TREE_REFCOUNT, &k)?;
            self.root_refcount = nr;
            Some(true)
        } else {
            self.ref_set(id, seq, c - 1)?;
            Some(false)
        }
    }

    // ── Valori primary (inline / overflow) ───────────────────────────────

    /// Scrive un valore primary: `(PrimRec senza id/seq/mtime, overflow_allocati)`.
    #[inline(never)]
    fn value_write(&mut self, data: &[u8]) -> Option<(Option<(u64, u64)>, Vec<u8>)> {
        if data.len() <= INLINE_MAX {
            return Some((None, data.to_vec()));
        }
        let (ov, size) = ov_write(&mut self.store, self.gen, data)?;
        Some((Some((ov, size)), Vec::new()))
    }

    /// Legge un valore primary (inline od overflow).
    #[inline(never)]
    fn value_read(&self, r: &PrimRec) -> Option<Vec<u8>> {
        match r.data_ov {
            Some((ov, size)) => {
                let sz: usize = size.try_into().ok()?;
                ov_read(&self.store, ov, sz)
            }
            None => Some(r.data_inline.clone()),
        }
    }

    /// Libera i blocchi overflow di un valore (il record lo cancella il chiamante).
    #[inline(never)]
    fn value_free(&mut self, r: &PrimRec) {
        if let Some((ov, _)) = r.data_ov {
            ov_free(&mut self.store, ov);
        }
    }

    /// Head di un oggetto: record al seq noto (i seq live sono densi
    /// `[lo..head]` per costruzione di `put_chunk`: trim dal basso, append
    /// in alto, mai buchi in cima). Lookup diretto, mai scansioni O(store).
    #[inline(never)]
    fn head_rec(&self, id: u64, seq: u64) -> Option<PrimRec> {
        if seq == 0 {
            return None;
        }
        let pk = prim_key(id, seq);
        let (_, leaf, i, found) = self.btree_lookup(self.root_primary, TREE_PRIMARY, &pk)?;
        if !found {
            return None;
        }
        prim_rec_parse(&leaf.vals[i]).map(|(r, _)| r)
    }

    /// Tutte le seq presenti per `id` (per trim/delete): vettore ordinato.
    #[inline(never)]
    fn seqs_of(&self, id: u64, head_seq: u64) -> Vec<u64> {
        let mut out = Vec::new();
        // Dalla head a ritroso finche' i record esistono (catena senza buchi:
        // i seq sono densi per costruzione di `put_chunk`).
        let mut s = head_seq;
        loop {
            let pk = prim_key(id, s);
            match self.btree_lookup(self.root_primary, TREE_PRIMARY, &pk) {
                Some((_, _, _, true)) => {
                    out.push(s);
                    if s == 0 {
                        break;
                    }
                    s -= 1;
                }
                _ => break,
            }
            if out.len() > 1 << 16 {
                break; // guardia: mai loop infinito su store corrotto
            }
        }
        out.reverse();
        out
    }

    // ── API oggetto (stessa semantica 56.1, backend blocchi) ─────────────

    /// PUT chunk a `offset` (0 = nuova versione da zero, >0 = patch della
    /// head): crea SEMPRE una versione nuova. Ritorna i byte accettati.
    /// `now` = wall-clock dal chiamante (cardo `wall::wall_secs`).
    #[inline(never)]
    pub fn put_chunk(
        &mut self,
        bucket: &[u8],
        key: &[u8],
        offset: usize,
        data: &[u8],
        now: u64,
        retain: usize,
    ) -> Option<usize> {
        let sk = seckey_encode(bucket, key)?;
        let n = data.len();
        // Base: clone della head se patch, altrimenti da zero.
        let (uuid, base, head_seq) = match self.btree_lookup(self.root_secondary, TREE_SECONDARY, &sk) {
            Some((_, leaf, i, true)) => {
                let (sec, _) = sec_rec_parse(&leaf.vals[i])?;
                let head = self.head_rec(sec.uuid, sec.head_seq)?;
                let blob = self.value_read(&head)?;
                (sec.uuid, if offset == 0 { Vec::new() } else { blob }, sec.head_seq)
            }
            _ => {
                let id = self.next_id;
                self.next_id += 1;
                self.by_id.insert(id, sk.clone());
                (id, Vec::new(), 0)
            }
        };
        let mut next = base;
        let end = offset.checked_add(n)?;
        if next.len() < end {
            if end > (1 << 24) {
                return None; // bound anti-OOM: 16 MiB per oggetto in 56.2b
            }
            next.resize(end, 0);
        }
        next.get_mut(offset..end)?.copy_from_slice(data);
        let seq = head_seq + 1;
        let (data_ov, data_inline) = self.value_write(&next)?;
        let rec = PrimRec { id: uuid, seq, mtime: now, data_ov, data_inline };
        let pk = prim_key(uuid, seq);
        let nr = self.btree_insert(self.root_primary, TREE_PRIMARY, pk.to_vec(), prim_rec_encode(&rec))?;
        self.root_primary = nr;
        // Upsert secondary (uuid stabile, head avanzata).
        let sec = SecRec {
            bucket: bucket.to_vec(), key: key.to_vec(), uuid,
            size: next.len() as u64, mtime: now, head_seq: seq,
        };
        let nr2 = self.btree_insert(self.root_secondary, TREE_SECONDARY, sk, sec_rec_encode(&sec)?)?;
        self.root_secondary = nr2;
        self.trim_retain(uuid, seq, retain)?;
        Some(n)
    }

    /// Trim retention: oltre `retain` versioni, le piu' vecchie non pinnate
    /// (refcount 0) vengono cancellate e liberate; quelle pinnate restano.
    #[inline(never)]
    fn trim_retain(&mut self, id: u64, head_seq: u64, retain: usize) -> Option<()> {
        let seqs = self.seqs_of(id, head_seq);
        if seqs.len() <= retain {
            return Some(());
        }
        let drop_n = seqs.len() - retain;
        for seq in seqs.iter().take(drop_n) {
            if self.ref_get(id, *seq) > 0 {
                continue; // pinnata da snapshot: resta
            }
            // Leggi il valore per liberare l'overflow, poi cancella.
            let pk = prim_key(id, *seq);
            let val = match self.btree_lookup(self.root_primary, TREE_PRIMARY, &pk) {
                Some((_, leaf, i, true)) => prim_rec_parse(&leaf.vals[i]).map(|(r, _)| r),
                _ => None,
            };
            let (nr, removed) = self.btree_remove(self.root_primary, TREE_PRIMARY, &pk)?;
            self.root_primary = nr;
            if removed {
                if let Some(rec) = val {
                    self.value_free(&rec);
                }
                // Pulisci il refcount (era 0: rimuovi il record).
                let _ = self.ref_dec_free(id, *seq);
            }
        }
        Some(())
    }

    /// Head di (bucket,key): blob completo o `None` (assente/malformato).
    #[inline(never)]
    pub fn get(&self, bucket: &[u8], key: &[u8]) -> Option<Vec<u8>> {
        let sk = seckey_encode(bucket, key)?;
        let (_, leaf, i, found) = self.btree_lookup(self.root_secondary, TREE_SECONDARY, &sk)?;
        if !found {
            return None;
        }
        let (sec, _) = sec_rec_parse(&leaf.vals[i])?;
        let head = self.head_rec(sec.uuid, sec.head_seq)?;
        self.value_read(&head)
    }

    /// Head per object_id (via indice inverso → secondary → primary).
    /// Oggetto cancellato (secondary assente) = `None`, mai dati inventati.
    #[inline(never)]
    pub fn get_id(&self, id: u64) -> Option<Vec<u8>> {
        let sk = self.by_id.get(&id)?.clone();
        let (_, leaf, i, found) = self.btree_lookup(self.root_secondary, TREE_SECONDARY, &sk)?;
        if !found {
            return None;
        }
        let (sec, _) = sec_rec_parse(&leaf.vals[i])?;
        if sec.uuid != id {
            return None; // indice inverso stale: mai dati altrui
        }
        let head = self.head_rec(id, sec.head_seq)?;
        self.value_read(&head)
    }

    /// Stat per (bucket,key): (id, size head, versioni contate, mtime head).
    #[inline(never)]
    pub fn stat(&self, bucket: &[u8], key: &[u8]) -> Option<(u64, u64, u64, u64)> {
        let sk = seckey_encode(bucket, key)?;
        let (_, leaf, i, found) = self.btree_lookup(self.root_secondary, TREE_SECONDARY, &sk)?;
        if !found {
            return None;
        }
        let (sec, _) = sec_rec_parse(&leaf.vals[i])?;
        let head = self.head_rec(sec.uuid, sec.head_seq)?;
        let nv = self.seqs_of(sec.uuid, sec.head_seq).len() as u64;
        let size = match head.data_ov {
            Some((_, s)) => s,
            None => head.data_inline.len() as u64,
        };
        Some((sec.uuid, size, nv, head.mtime))
    }

    /// Stat per object_id: (size head, versioni contate, mtime head).
    /// Oggetto cancellato = `None` (mai dati inventati).
    #[inline(never)]
    pub fn stat_id(&self, id: u64) -> Option<(u64, u64, u64)> {
        let sk = self.by_id.get(&id)?.clone();
        let (_, leaf, i, found) = self.btree_lookup(self.root_secondary, TREE_SECONDARY, &sk)?;
        if !found {
            return None;
        }
        let (sec, _) = sec_rec_parse(&leaf.vals[i])?;
        if sec.uuid != id {
            return None;
        }
        let head = self.head_rec(id, sec.head_seq)?;
        let nv = self.seqs_of(id, sec.head_seq).len() as u64;
        let size = match head.data_ov {
            Some((_, s)) => s,
            None => head.data_inline.len() as u64,
        };
        Some((size, nv, head.mtime))
    }

    /// Cancella nome + catena viva (gli snapshot pinnati restano validi:
    /// i pin vivono nel refcount, i dati overflow pinnati non si liberano).
    /// Le versioni pinnate perdono il nome ma i blocchi restano finche'
    /// lo snapshot vive (orphan visibili alla GC 56.2c se lo snapshot muore
    /// dopo: documentato, non nascosto).
    #[inline(never)]
    pub fn delete(&mut self, bucket: &[u8], key: &[u8]) -> Option<bool> {
        let sk = seckey_encode(bucket, key)?;
        let (sec, head_seq) = match self.btree_lookup(self.root_secondary, TREE_SECONDARY, &sk) {
            Some((_, leaf, i, true)) => {
                let (s, _) = sec_rec_parse(&leaf.vals[i])?;
                (s.uuid, s.head_seq)
            }
            _ => return Some(false),
        };
        let (nr, removed) = self.btree_remove(self.root_secondary, TREE_SECONDARY, &sk)?;
        self.root_secondary = nr;
        if !removed {
            return Some(false);
        }
        self.by_id.remove(&sec);
        // Catena viva: cancella le versioni non pinnate, sgancia le pinnate.
        for seq in self.seqs_of(sec, head_seq) {
            if self.ref_get(sec, seq) > 0 {
                continue; // pinnata: il blocco resta per lo snapshot
            }
            let pk = prim_key(sec, seq);
            let val = match self.btree_lookup(self.root_primary, TREE_PRIMARY, &pk) {
                Some((_, leaf, i, true)) => prim_rec_parse(&leaf.vals[i]).map(|(r, _)| r),
                _ => None,
            };
            let (nrp, _) = self.btree_remove(self.root_primary, TREE_PRIMARY, &pk)?;
            self.root_primary = nrp;
            if let Some(rec) = val {
                self.value_free(&rec);
            }
            let _ = self.ref_dec_free(sec, seq);
        }
        Some(true)
    }

    // ── Snapshot / clone / rollback (pin = refcount su disco) ──────────

    /// Tutte le seckey di un bucket (scansione secondary: O(bucket), il
    /// listing paginato 56.3 userà range-scan + cursore su questo stesso path).
    #[inline(never)]
    fn keys_of_bucket(&self, bucket: &[u8], root: u64) -> Vec<(Vec<u8>, SecRec)> {
        // Walk completa del secondary (a scala gate decine di chiavi: niente
        // cursore, walk semplice). Il listing paginato 56.3 usera' range-scan
        // sullo stesso path.
        let mut out = Vec::new();
        let mut stack = Vec::from([root]);
        let mut guard = 1 << 16;
        while let Some(blk) = stack.pop() {
            if guard == 0 {
                break;
            }
            guard -= 1;
            let (ty, tag, _, p) = match self.node_read(blk) {
                Some(n) => n,
                None => continue,
            };
            if tag != TREE_SECONDARY {
                continue; // nodo estraneo: skip difensivo, mai panico
            }
            if ty == ARCA_NODE_TYPE_LEAF {
                if let Some(leaf) = leaf_parse(&p[..], TREE_SECONDARY) {
                    for (k, v) in leaf.keys.iter().zip(leaf.vals.iter()) {
                        if let Some((sec, _)) = sec_rec_parse(v) {
                            if sec.bucket == bucket {
                                out.push((k.clone(), sec));
                            }
                        }
                    }
                }
            } else if let Some(it) = internal_parse(&p[..], TREE_SECONDARY) {
                stack.extend(it.children.iter().copied());
            }
        }
        out
    }

    /// Snapshot del bucket: pinna la head di OGNI chiave (refcount inc).
    /// Ritorna l'id snapshot.
    #[inline(never)]
    pub fn snap_create(&mut self, bucket: &[u8], tick: u64) -> Option<u64> {
        if bucket.len() > super::proto::OBJ_BUCKET_MAX {
            return None;
        }
        let root = self.root_secondary;
        let entries_in: Vec<(Vec<u8>, SecRec)> = self.keys_of_bucket(bucket, root);
        let mut entries = Vec::with_capacity(entries_in.len());
        for (sk, sec) in entries_in.iter() {
            self.ref_inc(sec.uuid, sec.head_seq)?;
            entries.push((sk.clone(), sec.uuid, sec.head_seq));
        }
        let id = self.next_snap;
        self.next_snap += 1;
        self.snaps.insert(id, Snapshot { id, bucket: bucket.to_vec(), tick, entries });
        Some(id)
    }

    /// Elimina uno snapshot: decrementa i pin (le versioni a zero restano
    /// finche' la retention/trim le raccoglie; i blocchi si liberano al trim
    /// o alla delete — mai dangling, mai double-free).
    #[inline(never)]
    pub fn snap_delete(&mut self, snap_id: u64) -> bool {
        let snap = match self.snaps.remove(&snap_id) {
            Some(s) => s,
            None => return false,
        };
        for (_, uuid, seq) in snap.entries.iter() {
            let _ = self.ref_dec_free(*uuid, *seq);
        }
        true
    }

    /// Rollback per-chiave: la versione pinnata diventa una NUOVA head
    /// (clonata, storia mai troncata). Solo stesso bucket.
    #[inline(never)]
    pub fn snap_rollback(
        &mut self,
        bucket: &[u8],
        key: &[u8],
        snap_id: u64,
        now: u64,
        retain: usize,
    ) -> Option<u64> {
        let snap = self.snaps.get(&snap_id)?;
        if snap.bucket != bucket {
            return None;
        }
        let sk = seckey_encode(bucket, key)?;
        let (_, uuid_pin, seq_pin) = snap.entries.iter().find(|(k, _, _)| *k == sk)?;
        let pinned = self.head_rec(*uuid_pin, *seq_pin)?;
        let data = self.value_read(&pinned)?;
        // Scrivi come nuova head della chiave live (crea l'oggetto se la
        // chiave e' stata cancellata dopo lo snapshot, con NUOVO uuid).
        let (uuid, head_seq) = match self.btree_lookup(self.root_secondary, TREE_SECONDARY, &sk) {
            Some((_, leaf, i, true)) => {
                let (sec, _) = sec_rec_parse(&leaf.vals[i])?;
                (sec.uuid, sec.head_seq)
            }
            _ => {
                let id = self.next_id;
                self.next_id += 1;
                self.by_id.insert(id, sk.clone());
                (id, 0)
            }
        };
        let seq = head_seq + 1;
        let (data_ov, data_inline) = self.value_write(&data)?;
        let rec = PrimRec { id: uuid, seq, mtime: now, data_ov, data_inline };
        let pk = prim_key(uuid, seq);
        let nr = self.btree_insert(self.root_primary, TREE_PRIMARY, pk.to_vec(), prim_rec_encode(&rec))?;
        self.root_primary = nr;
        let sec = SecRec {
            bucket: bucket.to_vec(), key: key.to_vec(), uuid,
            size: data.len() as u64, mtime: now, head_seq: seq,
        };
        let nr2 = self.btree_insert(self.root_secondary, TREE_SECONDARY, sk, sec_rec_encode(&sec)?)?;
        self.root_secondary = nr2;
        self.trim_retain(uuid, seq, retain)?;
        Some(data.len() as u64)
    }

    /// Clone di bucket: ogni entry dello snapshot diventa oggetto NUOVO
    /// (nuovi uuid) in `dst`. Ritorna gli oggetti clonati.
    #[inline(never)]
    pub fn snap_clone(&mut self, snap_id: u64, dst: &[u8], now: u64, retain: usize) -> Option<u64> {
        if dst.len() > super::proto::OBJ_BUCKET_MAX {
            return None;
        }
        let snap = self.snaps.get(&snap_id)?.clone();
        let mut n = 0u64;
        for (sk_pin, uuid_pin, seq_pin) in snap.entries.iter() {
            // Chiave destinazione = stesso key, bucket dst.
            let (_, key, _) = seckey_parse(sk_pin)?;
            let pinned = self.head_rec(*uuid_pin, *seq_pin)?;
            let data = self.value_read(&pinned)?;
            let sk_dst = seckey_encode(dst, key)?;
            let (uuid, head_seq) = match self.btree_lookup(self.root_secondary, TREE_SECONDARY, &sk_dst) {
                Some((_, leaf, i, true)) => {
                    let (sec, _) = sec_rec_parse(&leaf.vals[i])?;
                    (sec.uuid, sec.head_seq)
                }
                _ => {
                    let id = self.next_id;
                    self.next_id += 1;
                    self.by_id.insert(id, sk_dst.clone());
                    (id, 0)
                }
            };
            let seq = head_seq + 1;
            let (data_ov, data_inline) = self.value_write(&data)?;
            let rec = PrimRec { id: uuid, seq, mtime: now, data_ov, data_inline };
            let pk = prim_key(uuid, seq);
            let nr = self.btree_insert(self.root_primary, TREE_PRIMARY, pk.to_vec(), prim_rec_encode(&rec))?;
            self.root_primary = nr;
            let sec = SecRec {
                bucket: dst.to_vec(), key: key.to_vec(), uuid,
                size: data.len() as u64, mtime: now, head_seq: seq,
            };
            let nr2 = self.btree_insert(self.root_secondary, TREE_SECONDARY, sk_dst, sec_rec_encode(&sec)?)?;
            self.root_secondary = nr2;
            self.trim_retain(uuid, seq, retain)?;
            n += 1;
        }
        Some(n)
    }
}

/// Store in memoria per i test host: blocchi numerati da 1 (0 = mai valido).
#[cfg(test)]
pub struct MemStore {
    pub blocks: Vec<Option<([u8; ARCA_NODE_PAYLOAD_LEN], u8, u64)>>,
    pub free_list: Vec<u64>,
}

#[cfg(test)]
impl MemStore {
    pub fn new() -> Self {
        Self { blocks: alloc::vec![None], free_list: Vec::new() }
    }
    pub fn live_count(&self) -> usize {
        self.blocks.iter().filter(|b| b.is_some()).count()
    }
}

#[cfg(test)]
impl BlockStore for MemStore {
    fn read_node(&self, blk: u64, out: &mut [u8; ARCA_NODE_PAYLOAD_LEN]) -> Option<(u8, u64)> {
        let (p, ty, gen) = self.blocks.get(blk as usize)?.as_ref()?;
        out.copy_from_slice(p);
        Some((*ty, *gen))
    }
    fn write_node(&mut self, blk: u64, ty: u8, gen: u64, payload: &[u8; ARCA_NODE_PAYLOAD_LEN]) -> bool {
        if blk == 0 || blk as usize >= self.blocks.len() {
            return false;
        }
        // Il tipo e' parte del contratto: MemStore lo registra come il disco.
        self.blocks[blk as usize] = Some((*payload, ty, gen));
        true
    }
    fn alloc(&mut self) -> Option<u64> {
        if let Some(b) = self.free_list.pop() {
            return Some(b);
        }
        let b = self.blocks.len() as u64;
        self.blocks.push(None);
        Some(b)
    }
    fn free(&mut self, blk: u64) -> bool {
        if blk == 0 || blk as usize >= self.blocks.len() {
            return false;
        }
        if self.blocks[blk as usize].is_none() {
            return false; // double-free: rifiuta loud come `ArcaVolume`
        }
        self.blocks[blk as usize] = None;
        self.free_list.push(blk);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> BTree<MemStore> {
        BTree::new(MemStore::new(), 1).expect("new")
    }

    #[test]
    fn roundtrip_piccolo() {
        let mut t = engine();
        assert_eq!(t.put_chunk(b"b", b"k1", 0, b"ciao", 100, 8), Some(4));
        assert_eq!(t.get(b"b", b"k1"), Some(alloc::vec![b'c', b'i', b'a', b'o']));
        let (id, sz, nv, mt) = t.stat(b"b", b"k1").expect("stat");
        assert!(id >= 1 && sz == 4 && nv == 1 && mt == 100);
        // GET_ID via indice inverso.
        let sk = seckey_encode(b"b", b"k1").unwrap();
        let uuid = t.by_id.iter().find(|(_, v)| **v == sk).map(|(k, _)| *k).unwrap();
        assert_eq!(uuid, id);
        assert_eq!(t.get_id(uuid), Some(alloc::vec![b'c', b'i', b'a', b'o']));
        assert_eq!(t.get_id(uuid + 1000000), None);
        assert_eq!(t.get(b"b", b"nope"), None);
    }

    #[test]
    fn versioni_catena_latest_e_retention() {
        let mut t = engine();
        t.put_chunk(b"v", b"k", 0, b"A", 1, 8).unwrap();
        t.put_chunk(b"v", b"k", 0, b"B", 2, 8).unwrap();
        assert_eq!(t.get(b"v", b"k"), Some(alloc::vec![b'B']));
        let (_, _, nv, _) = t.stat(b"v", b"k").unwrap();
        assert_eq!(nv, 2);
        for i in 0..10u8 {
            t.put_chunk(b"v", b"k", 0, &[b'D', b'0' + i], 10 + i as u64, 8).unwrap();
        }
        // 12 versioni totali, trim a 8, latest D9.
        assert_eq!(t.get(b"v", b"k"), Some(alloc::vec![b'D', b'9']));
        let (_, sz, nv, _) = t.stat(b"v", b"k").unwrap();
        assert_eq!((sz, nv), (2, 8));
    }

    #[test]
    fn patch_offset_clone_head() {
        let mut t = engine();
        t.put_chunk(b"v", b"k", 0, b"hello", 1, 8).unwrap();
        // Patch a offset 1: nuova versione "hELLo", storia 2.
        assert_eq!(t.put_chunk(b"v", b"k", 1, b"ELL", 2, 8), Some(3));
        assert_eq!(t.get(b"v", b"k"), Some(b"hELLo".to_vec()));
        let (_, _, nv, _) = t.stat(b"v", b"k").unwrap();
        assert_eq!(nv, 2);
    }

    #[test]
    fn snapshot_rollback_delete_clone() {
        let mut t = engine();
        t.put_chunk(b"b", b"k1", 0, b"A", 1, 8).unwrap();
        t.put_chunk(b"b", b"k1", 0, b"B", 2, 8).unwrap();
        let sid = t.snap_create(b"b", 3).expect("snap");
        t.put_chunk(b"b", b"k1", 0, b"C", 4, 8).unwrap();
        // Rollback: la pinnata B diventa nuova head (storia A,B,C,B').
        assert!(t.snap_rollback(b"b", b"k1", sid, 5, 8).is_some());
        assert_eq!(t.get(b"b", b"k1"), Some(alloc::vec![b'B']));
        let (_, _, nv, _) = t.stat(b"b", b"k1").unwrap();
        assert_eq!(nv, 4);
        // Delete snapshot: live intatto, seconda delete fallisce.
        assert!(t.snap_delete(sid));
        let (_, _, nv2, _) = t.stat(b"b", b"k1").unwrap();
        assert_eq!(nv2, 4);
        assert!(!t.snap_delete(sid));
        // Clone di un nuovo snapshot in dst.
        t.put_chunk(b"cs", b"a", 0, b"uno", 6, 8).unwrap();
        t.put_chunk(b"cs", b"b", 0, b"due!", 7, 8).unwrap();
        let s2 = t.snap_create(b"cs", 8).unwrap();
        assert_eq!(t.snap_clone(s2, b"cd", 9, 8), Some(2));
        assert_eq!(t.get(b"cd", b"a"), Some(alloc::vec![b'u', b'n', b'o']));
        assert_eq!(t.get(b"cd", b"b"), Some(alloc::vec![b'd', b'u', b'e', b'!']));
        // Clone assegna uuid NUOVI (niente aliasing d'identita').
        let (id_src, _, _, _) = t.stat(b"cs", b"a").unwrap();
        let (id_dst, _, _, _) = t.stat(b"cd", b"a").unwrap();
        assert_ne!(id_src, id_dst);
        // Delete oggetto: GET/STAT rifiutati, re-delete falsa.
        assert_eq!(t.delete(b"b", b"k1"), Some(true));
        assert_eq!(t.get(b"b", b"k1"), None);
        assert_eq!(t.stat(b"b", b"k1"), None);
        assert_eq!(t.delete(b"b", b"k1"), Some(false));
    }

    #[test]
    fn refcount_pin_sopravvive_a_delete_e_trim() {
        let mut t = engine();
        t.put_chunk(b"b", b"k", 0, b"PIN", 1, 8).unwrap();
        let sid = t.snap_create(b"b", 2).unwrap();
        // Delete live: lo snapshot pinna ancora i blocchi (refcount 1).
        assert_eq!(t.delete(b"b", b"k"), Some(true));
        assert_eq!(t.get(b"b", b"k"), None);
        // Rollback ricrea la chiave dallo snapshot (nuovo uuid).
        assert!(t.snap_rollback(b"b", b"k", sid, 3, 8).is_some());
        assert_eq!(t.get(b"b", b"k"), Some(alloc::vec![b'P', b'I', b'N']));
        // Trim aggressivo con pin: la versione pinnata non si libera.
        assert!(t.snap_delete(sid));
    }

    #[test]
    fn bulk_split_e_merge() {
        let mut t = engine();
        // 120 chiavi da ~70 B: forza split multi-livello della secondary.
        for i in 0..120u64 {
            let k = alloc::format!("chiave-{:03}", i);
            let v = alloc::vec![(i % 251) as u8; 64];
            assert!(t.put_chunk(b"d", k.as_bytes(), 0, &v, i, 8).is_some(), "put {}", i);
        }
        // Albero cresciuto oltre la foglia singola.
        assert!(t.store.live_count() > 4, "live={}", t.store.live_count());
        for i in 0..120u64 {
            let k = alloc::format!("chiave-{:03}", i);
            let v = alloc::vec![(i % 251) as u8; 64];
            assert_eq!(t.get(b"d", k.as_bytes()), Some(v), "get {}", i);
        }
        // Delete di 80 chiavi: merge a foglie vuote, le restanti leggibili.
        for i in 0..80u64 {
            let k = alloc::format!("chiave-{:03}", i);
            assert_eq!(t.delete(b"d", k.as_bytes()), Some(true), "del {}", i);
        }
        for i in 80..120u64 {
            let k = alloc::format!("chiave-{:03}", i);
            let v = alloc::vec![(i % 251) as u8; 64];
            assert_eq!(t.get(b"d", k.as_bytes()), Some(v), "survivor {}", i);
        }
        for i in 0..80u64 {
            let k = alloc::format!("chiave-{:03}", i);
            assert_eq!(t.get(b"d", k.as_bytes()), None, "gone {}", i);
        }
    }

    #[test]
    fn overflow_blob_grande_roundtrip() {
        let mut t = engine();
        let big: Vec<u8> = (0..3000u32).map(|i| (i * 7 % 251) as u8).collect();
        assert_eq!(t.put_chunk(b"b", b"big", 0, &big, 1, 8), Some(3000));
        assert_eq!(t.get(b"b", b"big"), Some(big));
        let (_, sz, nv, _) = t.stat(b"b", b"big").unwrap();
        assert_eq!((sz, nv), (3000, 1));
        // Seconda versione grande: la prima resta in storia (due catene ov).
        let big2: Vec<u8> = (0..3000u32).map(|i| (i * 13 % 251) as u8).collect();
        t.put_chunk(b"b", b"big", 0, &big2, 2, 8).unwrap();
        assert_eq!(t.get(b"b", b"big"), Some(big2));
    }

    #[test]
    fn chiavi_lunghe_e_bound() {
        let mut t = engine();
        let long_k = alloc::vec![b'x'; 200];
        assert!(t.put_chunk(b"b", &long_k, 0, b"v", 1, 8).is_some());
        assert_eq!(t.get(b"b", &long_k), Some(alloc::vec![b'v']));
        // Oltre bound: rifiuto loud, mai troncamento.
        let too_long_b = alloc::vec![b'y'; 17];
        let too_long_k = alloc::vec![b'z'; 256];
        assert_eq!(t.put_chunk(&too_long_b, b"k", 0, b"v", 1, 8), None);
        assert_eq!(t.put_chunk(b"b", &too_long_k, 0, b"v", 1, 8), None);
        assert_eq!(t.snap_create(&too_long_b, 1), None);
        assert_eq!(t.get(&too_long_b, b"k"), None);
    }

    #[test]
    fn meta_roundtrip_e_rollback_post_reload() {
        let mut t = engine();
        t.put_chunk(b"b", b"k", 0, b"v1", 1, 8).unwrap();
        t.put_chunk(b"b", b"k", 0, b"v2", 2, 8).unwrap();
        let sid = t.snap_create(b"b", 3).unwrap();
        t.meta_store().unwrap();
        let (rp, rs, rr, mr, nid, nsn) =
            (t.root_primary, t.root_secondary, t.root_refcount, t.meta_root, t.next_id, t.next_snap);
        assert!(mr != 0);
        // Restart simulato: tabella persa, blocchi riusati.
        let store = t.into_store();
        let mut t2 = BTree::new_with_roots(store, 1, rp, rs, rr).unwrap();
        t2.meta_root = mr;
        t2.next_id = nid;
        t2.next_snap = nsn;
        assert!(t2.snaps.is_empty());
        t2.rebuild_by_id().unwrap();
        t2.meta_load().unwrap();
        // Lo snapshot ricaricato e' usabile (rollback alla pinnata v2).
        assert!(t2.snap_rollback(b"b", b"k", sid, 4, 8).is_some());
        assert_eq!(t2.get(b"b", b"k"), Some(alloc::vec![b'v', b'2']));
        // Meta corrotta: tabella ignorata loud, dati intatti.
        let bad = t2.store.alloc().unwrap();
        {
            let mut p = [0u8; ARCA_NODE_PAYLOAD_LEN];
            p[0] = 9; // tag ignoto
            t2.store.write_node(bad, ARCA_NODE_TYPE_LEAF, 1, &p);
        }
        t2.meta_root = bad;
        assert!(t2.meta_load().is_none());
        assert_eq!(t2.get(b"b", b"k"), Some(alloc::vec![b'v', b'2']));
    }

    #[test]
    fn reachable_copre_tutto_e_trova_orfani() {
        let mut t = engine();
        t.put_chunk(b"b", b"k1", 0, b"uno", 1, 8).unwrap();
        t.put_chunk(b"b", b"k2", 0, b"due", 2, 8).unwrap();
        let big: Vec<u8> = (0..2000u32).map(|i| (i % 251) as u8).collect();
        t.put_chunk(b"b", b"big", 0, &big, 3, 8).unwrap();
        let sid = t.snap_create(b"b", 4).unwrap();
        assert!(sid > 0);
        // COW orfana le vecchie radici a ogni op (per disegno: le raccoglie
        // la GC): le radici CORRENTI + catena ov sono raggiungibili, la prima
        // radice (superata dai rewrite) no.
        let r = t.reachable_blocks().unwrap();
        for b in [t.root_primary, t.root_secondary, t.root_refcount] {
            assert!(r.contains(&b), "radice {} irraggiungibile?!", b);
        }
        assert!(!r.contains(&1), "vecchia root ancora raggiungibile?!");
        assert!(r.len() < t.store.live_count(), "nessun orfano COW?!");
        // Orfano staged (alloc mai linkato): non raggiungibile.
        let orf = t.store.alloc().unwrap();
        let r2 = t.reachable_blocks().unwrap();
        assert!(!r2.contains(&orf));
        assert!(r2.len() == r.len());
    }

    #[test]
    fn rollback_bucket_mismatch_e_id_ignoto() {
        let mut t = engine();
        t.put_chunk(b"a", b"k", 0, b"v", 1, 8).unwrap();
        let sid = t.snap_create(b"a", 2).unwrap();
        assert_eq!(t.snap_rollback(b"altro", b"k", sid, 3, 8), None);
        assert_eq!(t.snap_rollback(b"a", b"k", sid + 999, 3, 8), None);
        assert!(!t.snap_delete(sid + 999));
    }
}