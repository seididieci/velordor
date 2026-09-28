//! ArcaFS provider (Fase 54, P5: stub — A1 lo riempie).
//!
//! In P5 il volume esiste (superblock scritto da `arca create`, riconosciuto
//! da `negotiate()` su magic+versione+checksum) ma non e' ancora leggibile:
//! ogni op ritorna un rifiuto tipizzato (mai hang, mai dati inventati).
//! La vista nativa (`R_OBJ_*`, A1) e il seal (A2) vivono qui in futuro;
//! `R_GET_HASH` resta valido anche allora (fallback via trait `read`).

use super::*;
use crate::provider::{EntrySink, LocalFs, LocalFsDyn, Meta, StatVfs};

/// Handle ArcaFS (P5: mai costruito — `open` rifiuta sempre; il tipo esiste
/// perche' `AnyHandle::Arca` lo richiede).
#[derive(Clone, Copy, PartialEq)]
pub struct ArcaHandle;

/// Istanza ArcaFS montata (P5: solo prova di avvenuto mount — generation e
/// uuid letti dal superblock a scopo diagnostico, mai usati per I/O).
pub struct ArcaFs {
    /// Generation del superblock montato (LBA0, o shadow LBA1 in futuro).
    pub generation: u64,
    /// UUID volume (identita' globale con `object_id` in A1).
    pub uuid: u64,
}

impl ArcaFs {
    pub fn stub(generation: u64, uuid: u64) -> Self {
        Self { generation, uuid }
    }
}

impl LocalFs for ArcaFs {
    type Handle = ArcaHandle;

    fn open(&mut self, _rel: &str, _flags: u32) -> Result<Self::Handle, u64> {
        Err(crate::ERR_NOTFOUND)
    }
    fn read(&mut self, _h: Self::Handle, _off: usize, _buf: &mut [u8]) -> Result<usize, u64> {
        Err(crate::ERR_NOTFOUND)
    }
    fn write(&mut self, _h: Self::Handle, _off: usize, _buf: &[u8], _append: bool) -> Result<usize, u64> {
        Err(crate::ERR_READONLY)
    }
    fn close(&mut self, _h: Self::Handle) {}
    fn readdir(&mut self, _rel: &str, _out: &mut dyn EntrySink) -> Result<usize, u64> {
        Err(crate::ERR_NOTFOUND)
    }
    fn stat(&mut self, _rel: &str) -> Result<Meta, u64> {
        Err(crate::ERR_NOTFOUND)
    }
    fn mkdir(&mut self, _rel: &str) -> Result<(), u64> {
        Err(crate::ERR_READONLY)
    }
    fn remove(&mut self, _rel: &str) -> Result<(), u64> {
        Err(crate::ERR_READONLY)
    }
    fn statvfs(&mut self, _rel: &str) -> Result<StatVfs, u64> {
        Err(crate::ERR_NOTFOUND)
    }
}

impl LocalFsDyn for ArcaFs {
    fn open_dyn(&mut self, rel: &str, flags: u32) -> Result<crate::provider::AnyHandle, u64> {
        <Self as LocalFs>::open(self, rel, flags).map(|_| crate::provider::AnyHandle::Arca(ArcaHandle))
    }
    fn read_dyn(&mut self, h: crate::provider::AnyHandle, off: usize, buf: &mut [u8]) -> Result<usize, u64> {
        match h {
            crate::provider::AnyHandle::Arca(ah) => <Self as LocalFs>::read(self, ah, off, buf),
            _ => Err(crate::ERR_INVALID),
        }
    }
    fn write_dyn(&mut self, h: crate::provider::AnyHandle, off: usize, buf: &[u8], append: bool) -> Result<usize, u64> {
        match h {
            crate::provider::AnyHandle::Arca(ah) => <Self as LocalFs>::write(self, ah, off, buf, append),
            _ => Err(crate::ERR_INVALID),
        }
    }
    fn readdir_dyn(&mut self, rel: &str, out: &mut dyn EntrySink) -> Result<usize, u64> {
        <Self as LocalFs>::readdir(self, rel, out)
    }
    fn stat_dyn(&mut self, rel: &str) -> Result<Meta, u64> {
        <Self as LocalFs>::stat(self, rel)
    }
    fn mkdir_dyn(&mut self, rel: &str) -> Result<(), u64> {
        <Self as LocalFs>::mkdir(self, rel)
    }
    fn remove_dyn(&mut self, rel: &str) -> Result<(), u64> {
        <Self as LocalFs>::remove(self, rel)
    }
    fn statvfs_dyn(&mut self, rel: &str) -> Result<StatVfs, u64> {
        <Self as LocalFs>::statvfs(self, rel)
    }
}
