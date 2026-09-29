//! Builder/parser puri dei frame ArcaFS (niente I/O, niente errori tipati:
//! ritornano `Option`, i chiamanti mappano nel loro errore di dominio).
//!
//! Usati da `libr` (wrapper client), userfs (handler server) e tool host:
//! il formato e' definito UNA volta sola.

use super::proto::{OBJ_BUCKET_MAX, OBJ_KEY_MAX};
use alloc::vec::Vec;

/// Prefisso comune `[bucket_len:1][bucket]\0[key_len:1][key]\0`: `None` oltre
/// i bound (sul wire la lunghezza sta in 1 byte: oltre e' inesprimibile,
/// si rifiuta, mai `as u8` troncante).
pub fn obj_prefix(bucket: &[u8], key: &[u8]) -> Option<Vec<u8>> {
    if bucket.len() > OBJ_BUCKET_MAX || key.len() > OBJ_KEY_MAX {
        return None;
    }
    let mut p = Vec::with_capacity(1 + bucket.len() + 1 + 1 + key.len() + 1);
    p.push(bucket.len() as u8);
    p.extend_from_slice(bucket);
    p.push(0);
    p.push(key.len() as u8);
    p.extend_from_slice(key);
    p.push(0);
    Some(p)
}

/// Parsa `[bucket_len:1][bucket]\0[key_len:1][key]\0` e ritorna
/// `(bucket, key, resto)`. Qualunque malformazione → `None` (mai panico su
/// slice, mai letture oltre il payload).
pub fn parse_obj_prefix(payload: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    let mut cursor = 0usize;
    let bucket_len = *payload.get(cursor)? as usize;
    cursor += 1;
    let bucket = payload.get(cursor..cursor + bucket_len)?;
    cursor += bucket_len;
    if *payload.get(cursor)? != 0 {
        return None;
    }
    cursor += 1;
    let key_len = *payload.get(cursor)? as usize;
    cursor += 1;
    let key = payload.get(cursor..cursor + key_len)?;
    cursor += key_len;
    if *payload.get(cursor)? != 0 {
        return None;
    }
    cursor += 1;
    let rest = payload.get(cursor..)?;
    Some((bucket, key, rest))
}
/// Costruisce `[len:1][bucket]`: `None` oltre bound. (Encode di un bucket
/// singolo per SNAP_CREATE/CLONE; il decode e' `parse_bucket_only`.)
pub fn bucket_only(bucket: &[u8]) -> Option<Vec<u8>> {
    if bucket.len() > OBJ_BUCKET_MAX {
        return None;
    }
    let mut p = Vec::with_capacity(1 + bucket.len());
    p.push(bucket.len() as u8);
    p.extend_from_slice(bucket);
    Some(p)
}

/// Parsa `[len:1][bytes]` esatti (niente trailing): bucket singolo.
/// `None` se lungo, con resto, o oltre bound.
pub fn parse_bucket_only(payload: &[u8]) -> Option<&[u8]> {
    let blen = *payload.first()? as usize;
    let bucket = payload.get(1..1 + blen)?;
    if payload.len() != 1 + blen || bucket.len() > OBJ_BUCKET_MAX {
        return None;
    }
    Some(bucket)
}

/// Parsa `[u64:8]` esatti (snapshot id, object id, block number).
pub fn parse_u64(payload: &[u8]) -> Option<u64> {
    let b = payload.first_chunk::<8>()?;
    if payload.len() != 8 {
        return None;
    }
    Some(u64::from_le_bytes(*b))
}

/// Divide `[u64:8][resto]`: id + coda (rollback/clone/debug con argomento).
pub fn split_id_rest(payload: &[u8]) -> Option<(u64, &[u8])> {
    let (head, rest) = payload.split_at_checked(8)?;
    Some((parse_u64(head)?, rest))
}
