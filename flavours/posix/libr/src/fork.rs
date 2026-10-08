//! `fork` (ADR-0024/ADR-0041, Fase 58.3): personalita' POSIX sopra il
//! meccanismo figlio-copia del kernel (`SYS_SPAWN_COPY`). Il meccanismo resta
//! in `civis` (mmap/shm/COW); `fork` esiste per POSIX e non si estende al
//! modello nativo (ADR-0025).

use civis::*;
use civis::syscall_numbers::SYS_SPAWN_COPY;

/// Esito di `fork()` (Fase 34): nel padre il pid del figlio + il canale di
/// nascita (stesso id da entrambi i lati; il padre lo usa numerico, il figlio
/// come canale 0 = `CHANNEL_PARENT`); nel figlio solo il canale di nascita
/// (il pid del figlio lo sa il padre, il figlio sa di essere il figlio).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForkResult {
    /// Padre: pid del figlio + canale di nascita verso di lui.
    Parent { pid: u64, chan: u64 },
    /// Figlio: canale di nascita verso il padre (= canale 0).
    Child { chan: u64 },
}

/// `fork()`: duplica il chiamante in COW (address space condiviso, copie
/// private al primo write). Il figlio riprende come ritorno dalla syscall con
/// 0; priorita' e `req_next` ereditati (e divergono), niente canali/fd/ring/
/// porte/CBS ereditati (solo nascita). `NoMemory` se non c'e' un PID libero o
/// l'OOM colpisce il walk (Fase 39). Nel ramo figlio avvelena automaticamente
/// l'FS (`post_spawn_copy_child`): le op FS ritornano `Err` invece di aliasare
/// i ring.
#[inline]
pub fn fork() -> Result<ForkResult, Error> {
    let (rax, rdi, _, _, _) = unsafe { syscall4_out(SYS_SPAWN_COPY, 0, 0, 0, 0) };
    if rax < 0 {
        return Err(Error::NoMemory);
    }
    if rax == 0 {
        civis::fs::session::post_spawn_copy_child();
        Ok(ForkResult::Child { chan: CHANNEL_PARENT })
    } else {
        Ok(ForkResult::Parent { pid: rax as u64, chan: rdi })
    }
}
