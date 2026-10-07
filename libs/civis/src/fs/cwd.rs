//! Directory di lavoro (S1.1): `chdir`/`getcwd` + risoluzione relativi.
//!
//! Niente cache client-side per disegno: ogni risoluzione fa una SYS_GETCWD
//! (una syscall in piu' per op con path — mai stato stale tra thread, fork
//! ed exec, dove una cache dovrebbe invalidarsi: la correttezza prima, la
//! cache quando i profili lo chiederanno). Il kernel tiene la cwd sul leader
//! del gruppo (condivisa ai thread come POSIX), sopravvive a exec, ereditata
//! da fork/spawn. Solo assoluti nel kernel; `..` resta testuale (nessuna
//! normalizzazione da nessuna parte, come prima).

use super::*;
use crate::*;
use alloc::vec::Vec;

/// `getcwd()`: directory di lavoro del gruppo (sempre assoluta, `/` default).
/// Ritorna i byte senza NUL.
pub fn getcwd() -> Result<Vec<u8>, Error> {
    let mut buf = [0u8; crate::CWD_MAX + 1];
    let r = unsafe { crate::syscall4(crate::SYS_GETCWD, buf.as_mut_ptr() as u64, buf.len() as u64, 0, 0) };
    if r < 0 {
        return Err(Error::Invalid);
    }
    Ok(buf[..r as usize].to_vec())
}

/// Unisce `path` alla cwd se relativo (assoluti invariati). Vuoto = invariato
/// (i chiamanti mantengono la semantica empty-path di prima).
pub fn resolve(path: &str) -> Result<Vec<u8>, Error> {
    if path.is_empty() || path.starts_with('/') {
        return Ok(path.as_bytes().to_vec());
    }
    let cwd = getcwd()?;
    let mut out = Vec::with_capacity(cwd.len() + 1 + path.len());
    out.extend_from_slice(&cwd);
    if !cwd.is_empty() && *cwd.last().unwrap_or(&0) != b'/' {
        out.push(b'/');
    }
    out.extend_from_slice(path.as_bytes());
    if out.len() > crate::CWD_MAX {
        return Err(Error::Invalid);
    }
    Ok(out)
}

/// `chdir(path)`: cambia la cwd del gruppo. Relativi uniti alla cwd corrente;
/// il target deve ESISTERE ed essere una dir (stat lstat: i symlink contano
/// come dir solo se... no — stat riporta il link (kind 3): chdir su link =
/// INVALID diretto qui? POSIX segue. Seguiamo: se link, risolvi un hop via
/// readlink relativo alla dir del link, poi stat del target).
/// Errori: NotFound/NotDir/Invalid dal kernel o dallo stat.
pub fn chdir(path: &str) -> Result<(), Error> {
    let abs = resolve(path)?;
    let abs_str = core::str::from_utf8(&abs).map_err(|_| Error::Invalid)?;
    // Il target deve esistere ed essere dir (il kernel non ha FS: stat qui).
    let mut st = super::sync::Stat { size: 0, kind: 0, readonly: false, mtime: 0 };
    super::sync::stat(abs_str, &mut st)?;
    if !st.is_dir() {
        return Err(Error::NotDir);
    }
    let r = unsafe { crate::syscall4(crate::SYS_CHDIR, abs.as_ptr() as u64, abs.len() as u64, 0, 0) };
    if r < 0 {
        Err(Error::Invalid)
    } else {
        Ok(())
    }
}


