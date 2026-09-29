//! ArcaFS provider (56.1: store nativo versionato in RAM).
//!
//! Ogni PUT crea una versione (mai overwrite); snapshot per-bucket con pin
//! dei dati (copie, non refcount — la condivisione COW degli extent arriva
//! con il B+tree on-disk in 56.2); GC = trim retention + drop a
//! snapshot-delete; `object_id` monotonico mai riusato (F2). La vista nativa
//! (`R_OBJ_*`, `R_SNAP_*`) vive qui; `R_GET_HASH` resta valido (fallback via
//! trait `read` sulla head).

use super::*;
use crate::provider::{EntrySink, LocalFs, LocalFsDyn, Meta, StatVfs};
use alloc::collections::BTreeMap;

// ArcaFS 56.1 — tipi versionati (sotto).

/// Versioni vive trattenute per oggetto (56.1): oltre si trimmano le piu'
/// vecchie non pinnate (gli snapshot tengono copie proprie, mai la catena).
const VERSION_RETAIN: usize = 8;

/// Handle ArcaFS (vista POSIX ancora stub — `open` rifiuta; il tipo esiste
/// perche' `AnyHandle::Arca` lo richiede; la dir persistente arriva in 56.3).
#[derive(Clone, Copy, PartialEq)]
pub struct ArcaHandle;

/// Una versione: blob immutabile + mtime di creazione (wall `Time`).
pub struct Version {
    pub data: Vec<u8>,
    pub mtime: u64,
}

/// Oggetto versionato: identita' stabile + catena (head = ultima).
pub struct Object {
    pub id: u64,
    pub versions: Vec<Version>,
}

/// Snapshot per-bucket (56.1): pin per-chiave con COPIE dei dati (niente
/// aliasing con la catena viva: DELETE/trim non invalidano mai uno snapshot;
/// il refcount sugli extent condivisi e' lavoro 56.2, non debito dimenticato).
pub struct Snapshot {
    pub id: u64,
    pub bucket: Vec<u8>,
    pub tick: u64,
    pub entries: Vec<(Vec<u8>, Vec<u8>)>, // (key, data) alla creazione
}

/// Istanza ArcaFS (56.1: store nativo versionato in RAM; la persistenza su
/// volume e' 56.2, i marker dir 56.3).
pub struct ArcaFs {
    /// Generation del superblock montato (diagnostica, come uuid).
    pub generation: u64,
    /// UUID volume (identita' globale con `object_id`).
    pub uuid: u64,
    /// Offset LBA della partizione (0 = whole-disk; traduzione nel driver).
    pub partition_offset: u64,
    /// Store: key flat → oggetto versionato.
    pub objects: BTreeMap<Vec<u8>, Object>,
    /// Indice inverso object_id → key flat (id mai riusati, F2).
    pub by_id: BTreeMap<u64, Vec<u8>>,
    /// Snapshot per id.
    pub snaps: BTreeMap<u64, Snapshot>,
    /// Prossimo object_id (parte da 1; 0 = mai valido).
    pub next_id: u64,
    /// Prossimo snapshot_id (parte da 1).
    pub next_snap: u64,
}

impl ArcaFs {
    pub fn stub(generation: u64, uuid: u64, partition_offset: u64) -> Self {
        Self {
            generation,
            uuid,
            partition_offset,
            objects: BTreeMap::new(),
            by_id: BTreeMap::new(),
            snaps: BTreeMap::new(),
            next_id: 1,
            next_snap: 1,
        }
    }

    /// Head di un oggetto: Ok(dati) o Err (`INVALID` oltre bound, `NOTFOUND`
    /// assente — mai dati inventati, mai troncamenti).
    pub fn get(&self, bucket: &[u8], key: &[u8]) -> Result<&Vec<u8>, u64> {
        let k = Self::make_key(bucket, key).ok_or(crate::ERR_INVALID)?;
        self.objects
            .get(&k)
            .and_then(|o| o.versions.last())
            .map(|v| &v.data)
            .ok_or(crate::ERR_NOTFOUND)
    }

    /// Head per object_id (stessi errori di `get`).
    pub fn get_id(&self, id: u64) -> Result<&Vec<u8>, u64> {
        let k = self.by_id.get(&id).ok_or(crate::ERR_NOTFOUND)?;
        self.objects
            .get(k)
            .and_then(|o| o.versions.last())
            .map(|v| &v.data)
            .ok_or(crate::ERR_NOTFOUND)
    }

    /// Stat per (bucket,key): (id, size head, versioni trattenute, mtime head).
    pub fn stat(&self, bucket: &[u8], key: &[u8]) -> Result<(u64, u64, u64, u64), u64> {
        let k = Self::make_key(bucket, key).ok_or(crate::ERR_INVALID)?;
        let o = self.objects.get(&k).ok_or(crate::ERR_NOTFOUND)?;
        let head = o.versions.last().ok_or(crate::ERR_NOTFOUND)?;
        Ok((o.id, head.data.len() as u64, o.versions.len() as u64, head.mtime))
    }

    /// Stat per object_id.
    pub fn stat_id(&self, id: u64) -> Result<(u64, u64, u64), u64> {
        let k = self.by_id.get(&id).ok_or(crate::ERR_NOTFOUND)?;
        let o = self.objects.get(k).ok_or(crate::ERR_NOTFOUND)?;
        let head = o.versions.last().ok_or(crate::ERR_NOTFOUND)?;
        Ok((head.data.len() as u64, o.versions.len() as u64, head.mtime))
    }

    /// Inserisce/aggiorna un oggetto: SEMPRE una nuova versione (mai
    /// overwrite — §1). Offset 0 = versione da zero (compat A1: re-PUT
    /// sostituisce la head visibile ma la storia resta); offset > 0 = clone
    /// della head con range patchata (COW in RAM, §5). Ritorna i byte
    /// accettati, None oltre bound. Il trim retention scatta a ogni push.
    pub fn put_chunk(&mut self, bucket: &[u8], key: &[u8], offset: usize, data: &[u8]) -> Option<usize> {
        let k = Self::make_key(bucket, key)?;
        let now = crate::wall::wall_secs();
        let n = data.len();
        if !self.objects.contains_key(&k) {
            let id = self.next_id;
            self.next_id += 1;
            self.by_id.insert(id, k.clone());
            self.objects.insert(k.clone(), Object { id, versions: Vec::new() });
        }
        let o = match self.objects.get_mut(&k) {
            Some(o) => o,
            None => return None, // impossibile: creato sopra
        };
        let mut base = if offset == 0 {
            Vec::new()
        } else {
            o.versions.last().map(|v| v.data.clone()).unwrap_or_else(Vec::new)
        };
        let end = offset + n;
        if base.len() < end {
            base.resize(end, 0);
        }
        base[offset..end].copy_from_slice(data);
        o.versions.push(Version { data: base, mtime: now });
        while o.versions.len() > VERSION_RETAIN {
            o.versions.remove(0);
        }
        Some(n)
    }

    /// Come `put_chunk` a offset 0 (nuova versione da zero).
    pub fn put(&mut self, bucket: &[u8], key: &[u8], data: &[u8]) -> Option<usize> {
        self.put_chunk(bucket, key, 0, data)
    }

    /// Cancella un oggetto (nome + catena viva). Gli snapshot tengono copie
    /// proprie: il delete non li invalida mai. Ritorna Err se assente.
    pub fn delete(&mut self, bucket: &[u8], key: &[u8]) -> Result<(), u64> {
        let k = Self::make_key(bucket, key).ok_or(crate::ERR_INVALID)?;
        match self.objects.remove(&k) {
            Some(o) => {
                self.by_id.remove(&o.id);
                Ok(())
            }
            None => Err(crate::ERR_NOTFOUND),
        }
    }

    /// Snapshot del bucket: pinna la head di OGNI chiave (copie). Ritorna
    /// l'id. Bucket oltre bound → None.
    pub fn snap_create(&mut self, bucket: &[u8]) -> Option<u64> {
        if bucket.len() > libr::OBJ_BUCKET_MAX {
            return None;
        }
        let id = self.next_snap;
        self.next_snap += 1;
        let mut entries = Vec::new();
        for (k, o) in self.objects.iter() {
            let (b, key) = Self::split_key(k)?;
            if b == bucket {
                if let Some(head) = o.versions.last() {
                    entries.push((key.to_vec(), head.data.clone()));
                }
            }
        }
        let tick = crate::wall::wall_secs();
        self.snaps.insert(id, Snapshot { id, bucket: bucket.to_vec(), tick, entries });
        Some(id)
    }

    /// Elimina uno snapshot (le copie pinnate vengono liberate qui: e' la GC
    /// 56.1 — niente refcount finche' gli extent non sono condivisi in 56.2).
    pub fn snap_delete(&mut self, id: u64) -> bool {
        self.snaps.remove(&id).is_some()
    }

    /// Rollback per-chiave: la versione pinnata dallo snapshot diventa una
    /// NUOVA head (clonata — la storia non si tronca mai, nemmeno al
    /// rollback). Lo snapshot deve coprire lo stesso bucket. Ritorna la
    /// nuova size.
    pub fn snap_rollback(&mut self, bucket: &[u8], key: &[u8], snap_id: u64) -> Result<u64, u64> {
        let data = match self.snaps.get(&snap_id) {
            Some(s) if s.bucket == bucket => match s.entries.iter().find(|(k, _)| k == key) {
                Some((_, d)) => d.clone(),
                None => return Err(crate::ERR_NOTFOUND),
            },
            Some(_) => return Err(crate::ERR_INVALID),
            None => return Err(crate::ERR_NOTFOUND),
        };
        let now = crate::wall::wall_secs();
        let k = Self::make_key(bucket, key).ok_or(crate::ERR_INVALID)?;
        if !self.objects.contains_key(&k) {
            let id = self.next_id;
            self.next_id += 1;
            self.by_id.insert(id, k.clone());
            self.objects.insert(k.clone(), Object { id, versions: Vec::new() });
        }
        let o = match self.objects.get_mut(&k) {
            Some(o) => o,
            None => return Err(crate::ERR_NOTFOUND), // impossibile
        };
        let size = data.len() as u64;
        o.versions.push(Version { data, mtime: now });
        while o.versions.len() > VERSION_RETAIN {
            o.versions.remove(0);
        }
        Ok(size)
    }

    /// Clone di bucket: ogni entry dello snapshot diventa un oggetto NUOVO
    /// (nuovi id) nel bucket destinazione. Ritorna gli oggetti clonati.
    pub fn snap_clone(&mut self, snap_id: u64, dst: &[u8]) -> Result<u64, u64> {
        if dst.len() > libr::OBJ_BUCKET_MAX {
            return Err(crate::ERR_INVALID);
        }
        let entries = match self.snaps.get(&snap_id) {
            Some(s) => s.entries.clone(),
            None => return Err(crate::ERR_NOTFOUND),
        };
        let now = crate::wall::wall_secs();
        let mut n = 0u64;
        for (key, data) in entries.iter() {
            let k = Self::make_key(dst, key).ok_or(crate::ERR_INVALID)?;
            if !self.objects.contains_key(&k) {
                let id = self.next_id;
                self.next_id += 1;
                self.by_id.insert(id, k.clone());
                self.objects.insert(k.clone(), Object { id, versions: Vec::new() });
            }
            let o = match self.objects.get_mut(&k) {
                Some(o) => o,
                None => return Err(crate::ERR_NOTFOUND), // impossibile
            };
            o.versions.push(Version { data: data.clone(), mtime: now });
            while o.versions.len() > VERSION_RETAIN {
                o.versions.remove(0);
            }
            n += 1;
        }
        Ok(n)
    }

    /// Scompone una key flat in (bucket, key). None se malformata (difesa:
    /// le key nascono solo da `make_key`, mai dal wire).
    fn split_key(k: &[u8]) -> Option<(&[u8], &[u8])> {
        let blen = *k.first()? as usize;
        let bucket = k.get(1..1 + blen)?;
        let key = k.get(1 + blen + 1 + 1..)?;
        // Layout make_key: [blen][bucket][0][klen][key] (senza terminatore).
        let klen = *k.get(1 + blen + 1)? as usize;
        if key.len() != klen {
            return None;
        }
        Some((bucket, key))
    }

    /// Costruisce la key flat per l'hash map. None oltre i bound condivisi
    /// (`OBJ_*_MAX` in `syscall-numbers` via `libr`): sul wire la lunghezza
    /// sta in 1 byte, oltre e' inesprimibile — si rifiuta, mai `as u8`.
    fn make_key(bucket: &[u8], key: &[u8]) -> Option<Vec<u8>> {
        if bucket.len() > libr::OBJ_BUCKET_MAX || key.len() > libr::OBJ_KEY_MAX {
            return None;
        }
        let mut k = Vec::with_capacity(1 + bucket.len() + 1 + 1 + key.len());
        k.push(bucket.len() as u8);
        k.extend_from_slice(bucket);
        k.push(0); // separator
        k.push(key.len() as u8);
        k.extend_from_slice(key);
        Some(k)
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
