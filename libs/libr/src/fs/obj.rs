//! Object store nativo ArcaFS (Fase 55, A1): `R_OBJ_GET`/`R_OBJ_PUT`.
//!
//! Stateless: il server non tiene stato tra le chiamate GET successive.
//! Chunking automatico: GET > RING_MAX_PAYLOAD → loop client con offset++.

use super::ring::{req_ring_write, resp_ring_read_payload, resp_ring_consume, RING_MAX_PAYLOAD};
use crate::{fs_notify_result, fs_reply_check, Error, FS_NOTIFY, R_OBJ_GET, R_OBJ_PUT};
use crate::{
    R_SNAP_CREATE, R_SNAP_DELETE, R_SNAP_ROLLBACK, R_SNAP_CLONE, R_OBJ_GET_ID,
    R_OBJ_STAT_ID, R_OBJ_DELETE, R_OBJ_STAT,
};
use crate::{OBJ_BUCKET_MAX, OBJ_KEY_MAX};
use alloc::vec;
use alloc::vec::Vec;

/// GET un oggetto da ArcaFS (stateless, chunking automatico).
/// Ritorna il blob completo o Error (`Invalid` a bucket/chiave oltre bound,
/// mai troncamento silenzioso).
pub fn obj_get(bucket: &[u8], key: &[u8]) -> Result<Vec<u8>, Error> {
    let mut offset = 0usize;
    let mut result = Vec::new();

    loop {
        let payload = match build_get_payload(bucket, key) {
            Some(p) => p,
            None => return Err(Error::Invalid),
        };
        let frame = || req_ring_write(R_OBJ_GET, payload.len() as u64, offset as u64, &payload);
        if !frame() {
            return Err(Error::RingFull);
        }

        match fs_notify_result(FS_NOTIFY, frame) {
            Some((size, _, payload_len)) => {
                // Sentinella ERR_* → errore di dominio, CONSUMA il frame (16B)
                // prima di uscire: senza, il frame stale resta nel response ring
                // e la successiva op FS legge spazzatura (desync).
                let total = match fs_reply_check(size) {
                    Ok(v) => v as usize,
                    Err(e) => {
                        resp_ring_consume(16);
                        return Err(e);
                    }
                };
                // Blob vuoto o EOF: niente payload, esci.
                let remaining = total.saturating_sub(offset);
                if remaining == 0 {
                    resp_ring_consume(16);
                    break;
                }
                let to_read = remaining.min(payload_len).min(RING_MAX_PAYLOAD);
                if to_read == 0 {
                    // Frame senza payload ma size non-zero: impossibile, esci
                    // consumando per non desincronizzare.
                    resp_ring_consume(16);
                    break;
                }
                let mut buf = vec![0u8; to_read];
                resp_ring_read_payload(&mut buf, to_read);
                result.extend_from_slice(&buf);
                offset += to_read;
                if offset >= total {
                    break;
                }
            }
            None => return Err(Error::NotReady),
        }
    }

    Ok(result)
}

/// PUT un oggetto in ArcaFS. Ritorna size scritta o Error (`Invalid` a
/// bucket/chiave oltre bound, mai troncamento silenzioso).
pub fn obj_put(bucket: &[u8], key: &[u8], data: &[u8]) -> Result<u64, Error> {
    // Valida i nomi una volta sola (il PUT vuoto salta il loop: senza,
    // nomi oltre bound passerebbero con Ok(0)).
    if build_obj_prefix(bucket, key).is_none() {
        return Err(Error::Invalid);
    }
    let total_len = data.len();
    let mut written = 0usize;

    while written < total_len {
        let want = (total_len - written).min(RING_MAX_PAYLOAD);
        let payload = match build_put_payload(bucket, key, &data[written..written + want]) {
            Some(p) => p,
            None => return Err(Error::Invalid),
        };

        if !req_ring_write(R_OBJ_PUT, payload.len() as u64, written as u64, &payload) {
            return if written > 0 { Ok(written as u64) } else { Err(Error::RingFull) };
        }

        match fs_notify_result(FS_NOTIFY, || {
            req_ring_write(R_OBJ_PUT, payload.len() as u64, written as u64, &payload)
        }) {
            Some((size, _, _)) => {
                resp_ring_consume(16);
                let r = match fs_reply_check(size) {
                    Ok(v) => v,
                    Err(e) => {
                        if written > 0 {
                            return Ok(written as u64);
                        }
                        return Err(e);
                    }
                };
                written += (r as usize).min(want);
            }
            None => {
                if written > 0 {
                    return Ok(written as u64);
                }
                return Err(Error::NotReady);
            }
        }
    }

    Ok(total_len as u64)
}

/// Prefisso comune `[bucket_len:1][bucket]\0[key_len:1][key]\0`: None oltre
/// i bound (`OBJ_BUCKET_MAX`/`OBJ_KEY_MAX` — sul wire la lunghezza sta in
/// 1 byte, oltre e' inesprimibile: si rifiuta, mai `as u8` troncante).
fn build_obj_prefix(bucket: &[u8], key: &[u8]) -> Option<Vec<u8>> {
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

/// Costruisce il payload per GET: prefisso + niente dati.
fn build_get_payload(bucket: &[u8], key: &[u8]) -> Option<Vec<u8>> {
    build_obj_prefix(bucket, key)
}

/// Costruisce il payload per PUT: prefisso + chunk dati.
fn build_put_payload(bucket: &[u8], key: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    let mut p = build_obj_prefix(bucket, key)?;
    p.extend_from_slice(data);
    Some(p)
}

// ── Versioni + snapshot (Fase 56.1) ─────────────────────────────────
// Convenzione comune: richiesta con retry di scrittura frame (stesso
// pattern di obj_put), frame risposta da 16 B consumato qui; i payload
// dedicati (STAT) letti dal chiamante subito dopo, prima di qualunque
// altra op sul ring (1-in-volo, Fase 13).

/// Invia una richiesta object/snap e ritorna (w0, w1) della reply.
/// Il frame risposta (header 16 B, niente payload dedicato) e' consumato.
fn obj_request(tag: u32, w0: u64, w1: u64, payload: &[u8]) -> Result<(u64, u64), Error> {
    let frame = || req_ring_write(tag, w0, w1, payload);
    if !frame() {
        return Err(Error::RingFull);
    }
    match fs_notify_result(FS_NOTIFY, frame) {
        Some((a, b, _)) => {
            resp_ring_consume(16);
            Ok((a, b))
        }
        None => Err(Error::NotReady),
    }
}

/// Richiesta scalare: valore in w0 o sentinella (mappata qui in dominio).
fn obj_scalar(tag: u32, payload: &[u8]) -> Result<u64, Error> {
    let (a, _) = obj_request(tag, payload.len() as u64, 0, payload)?;
    fs_reply_check(a)
}

/// Snapshot del bucket → snap_id.
pub fn snap_create(bucket: &[u8]) -> Result<u64, Error> {
    if bucket.len() > OBJ_BUCKET_MAX {
        return Err(Error::Invalid);
    }
    let mut p = Vec::with_capacity(1 + bucket.len());
    p.push(bucket.len() as u8);
    p.extend_from_slice(bucket);
    obj_scalar(R_SNAP_CREATE, &p)
}

/// Elimina uno snapshot (GC delle copie pinnate).
pub fn snap_delete(snap_id: u64) -> Result<(), Error> {
    obj_scalar(R_SNAP_DELETE, &snap_id.to_le_bytes()).map(|_| ())
}

/// Rollback per-chiave dallo snapshot → nuova head size.
pub fn snap_rollback(bucket: &[u8], key: &[u8], snap_id: u64) -> Result<u64, Error> {
    let mut p = Vec::with_capacity(8 + 1 + bucket.len() + 1 + 1 + key.len() + 1);
    p.extend_from_slice(&snap_id.to_le_bytes());
    p.extend_from_slice(&build_obj_prefix(bucket, key).ok_or(Error::Invalid)?);
    obj_scalar(R_SNAP_ROLLBACK, &p)
}

/// Clona il bucket pinnato in `dst` (nuovi id) → oggetti clonati.
pub fn snap_clone(snap_id: u64, dst: &[u8]) -> Result<u64, Error> {
    if dst.len() > OBJ_BUCKET_MAX {
        return Err(Error::Invalid);
    }
    let mut p = Vec::with_capacity(8 + 1 + dst.len());
    p.extend_from_slice(&snap_id.to_le_bytes());
    p.push(dst.len() as u8);
    p.extend_from_slice(dst);
    obj_scalar(R_SNAP_CLONE, &p)
}

/// GET per object_id (chunking automatico come `obj_get`).
pub fn obj_get_id(id: u64) -> Result<Vec<u8>, Error> {
    let mut offset = 0usize;
    let mut result = Vec::new();
    let payload = id.to_le_bytes();
    loop {
        let frame = || req_ring_write(R_OBJ_GET_ID, payload.len() as u64, offset as u64, &payload);
        if !frame() {
            return Err(Error::RingFull);
        }
        match fs_notify_result(FS_NOTIFY, frame) {
            Some((size, _, payload_len)) => {
                let total = match fs_reply_check(size) {
                    Ok(v) => v as usize,
                    Err(e) => {
                        resp_ring_consume(16);
                        return Err(e);
                    }
                };
                let remaining = total.saturating_sub(offset);
                if remaining == 0 {
                    resp_ring_consume(16);
                    break;
                }
                let to_read = remaining.min(payload_len).min(RING_MAX_PAYLOAD);
                if to_read == 0 {
                    resp_ring_consume(16);
                    break;
                }
                let mut buf = vec![0u8; to_read];
                resp_ring_read_payload(&mut buf, to_read);
                result.extend_from_slice(&buf);
                offset += to_read;
                if offset >= total {
                    break;
                }
            }
            None => return Err(Error::NotReady),
        }
    }
    Ok(result)
}

/// Stat per (bucket,key): (id, size head, versioni, mtime head).
pub fn obj_stat(bucket: &[u8], key: &[u8]) -> Result<(u64, u64, u64, u64), Error> {
    let prefix = build_obj_prefix(bucket, key).ok_or(Error::Invalid)?;
    let frame = || req_ring_write(R_OBJ_STAT, prefix.len() as u64, 0, &prefix);
    if !frame() {
        return Err(Error::RingFull);
    }
    match fs_notify_result(FS_NOTIFY, frame) {
        Some((id, size, _)) => {
            // Prima i registri (a errore il frame e' vuoto: leggerlo
            // sarebbe oltre l'header, nel frame altrui — desync).
            let ok = fs_reply_check(id).and(fs_reply_check(size));
            match ok {
                Ok(_) => {
                    let mut f = [0u8; 16];
                    resp_ring_read_payload(&mut f, 16);
                    let nv = u64::from_le_bytes(f[..8].try_into().map_err(|_| Error::Invalid)?);
                    let mtime = u64::from_le_bytes(f[8..].try_into().map_err(|_| Error::Invalid)?);
                    // id/size gia' validati sopra (valori, non sentinelle).
                    Ok((id, size, nv, mtime))
                }
                Err(e) => {
                    resp_ring_consume(16);
                    Err(e)
                }
            }
        }
        None => Err(Error::NotReady),
    }
}

/// Stat per object_id: (size head, versioni, mtime head).
pub fn obj_stat_id(id: u64) -> Result<(u64, u64, u64), Error> {
    let payload = id.to_le_bytes();
    let frame = || req_ring_write(R_OBJ_STAT_ID, payload.len() as u64, 0, &payload);
    if !frame() {
        return Err(Error::RingFull);
    }
    match fs_notify_result(FS_NOTIFY, frame) {
        Some((size, nv, _)) => {
            let ok = fs_reply_check(size).and(fs_reply_check(nv));
            match ok {
                Ok(_) => {
                    let mut f = [0u8; 8];
                    resp_ring_read_payload(&mut f, 8);
                    let mtime = u64::from_le_bytes(f);
                    Ok((size, nv, mtime))
                }
                Err(e) => {
                    resp_ring_consume(16);
                    Err(e)
                }
            }
        }
        None => Err(Error::NotReady),
    }
}

/// Cancella nome + catena viva (gli snapshot restano validi).
pub fn obj_delete(bucket: &[u8], key: &[u8]) -> Result<(), Error> {
    let prefix = build_obj_prefix(bucket, key).ok_or(Error::Invalid)?;
    obj_scalar(R_OBJ_DELETE, &prefix).map(|_| ())
}
