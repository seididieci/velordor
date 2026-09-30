//! ArcaFS provider stub (56.2b).
//!
//! Lo store versionato vive nei blocchi via `btree_drv` (B+tree COW +
//! commit), non piu' in RAM: il backend mem 56.1 era transitorio per
//! dichiarazione (`arcafs.md` §17) e l'oracolo di confronto e' nei test
//! host `arcafs` (`MemStore`, 9 test), non in un doppio backend guest che
//! costerebbe ~19 KiB di `cardo.bin` oltre `SPAWN_IMAGE_MAX`.
//! Qui resta solo il tipo per `negotiate()`/vista POSIX (stub: `open`
//! rifiuta; la dir persistente arriva in 56.3). Senza volume legato gli op
//! nativi danno errore loud e init ripiega su FAT (dual-mode N0 invariato).

use super::*;
use crate::provider::{EntrySink, LocalFs, LocalFsDyn, Meta, StatVfs};

/// Handle ArcaFS (vista POSIX ancora stub — `open` rifiuta; il tipo esiste
/// perche' `AnyHandle::Arca` lo richiede; la dir persistente arriva in 56.3).
#[derive(Clone, Copy, PartialEq)]
pub struct ArcaHandle;

/// Istanza ArcaFS (stub per il mount: generazione/uuid/offset diagnostici;
/// i dati vivono nel motore disco legato via `R_ARCA_DEBUG`).
pub struct ArcaFs {
    /// Generation del superblock montato (diagnostica, come uuid).
    pub generation: u64,
    /// UUID volume (identita' globale con `object_id`).
    pub uuid: u64,
    /// Offset LBA della partizione (0 = whole-disk; traduzione nel driver).
    pub partition_offset: u64,
}

impl ArcaFs {
    pub fn stub(generation: u64, uuid: u64, partition_offset: u64) -> Self {
        Self { generation, uuid, partition_offset }
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
