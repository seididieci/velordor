//! Hook di personalita' (ADR-0041, Fase 58.3): il punto in cui il meccanismo
//! neutro cede il controllo alla personalita' senza conoscerla.
//!
//! `civis` non riferisce mai POSIX: `print::flush` (meccanismo, usato anche dai
//! server nativi) deve pero' instradare l'output sul file quando la personalita'
//! POSIX ha attivato un redirect. L'instradamento vive in `flavours/posix/libr`
//! (`stdio::route_out`) e viene installato al bordo, nell'entry POSIX.
//! Senza hook installato il routing ritorna `false` (= seriale, nativo invariato).

use core::sync::atomic::{AtomicUsize, Ordering};

/// Funzione di instradamento stdout della personalita' (`&[u8]` -> "gestito").
pub type RouteOut = fn(&[u8]) -> bool;

static ROUTE_OUT: AtomicUsize = AtomicUsize::new(0);

/// Installa la funzione di routing della personalita' (chiamata dalla
/// personalita' prima di saltare a `main`). Un solo hook per processo.
pub fn set_route_out(f: RouteOut) {
    ROUTE_OUT.store(f as usize, Ordering::Relaxed);
}

/// Instrada `bytes` via hook, se installato. `false` = nessuna personalita'
/// (o hook assente): il chiamante usa il sink di default (seriale).
pub fn route_out(bytes: &[u8]) -> bool {
    let p = ROUTE_OUT.load(Ordering::Relaxed);
    if p == 0 {
        return false;
    }
    let f: RouteOut = unsafe { core::mem::transmute::<usize, RouteOut>(p) };
    f(bytes)
}
