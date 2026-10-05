use super::*;

// ── Fase 39 (P0, fondamenta posix) + 40.3 (skeleton supervisionato) ───
// t53: (a) Posix registrato e supervisionato: lookup riesce, pid noto e
// figlio di init (lineage di supervisione, mai squat); (b) tabella to_errno
// totale e fissata; (c) gate di registrazione sul nuovo slot 8 via helper
// non-figlio-di-init (stesso probe di t50, che resta intatto: qui si esercita
// service_from_disc(8) + braccio nome "posix").
//
// Nota storica: in Fase 39 (a) asseriva lookup/pid = NotFound (nessun server).
// Dalla 40.3 il skeleton gira supervisionato: l'assenza sarebbe un FAIL.

/// t53 — fondamenta posix: registry, errore nativo, gate.
pub fn t_posix_foundation() -> bool {
    helpers::drain_stray();
    // (a) Server Posix su e supervisionato: lookup riesce subito (niente
    // bound da attendere: init lo spawna prima della suite) e il pid e'
    // figlio di init (stessa lineage degli altri servizi).
    let _ = match civis::service_lookup(civis::Service::Posix) {
        Ok(chan) => chan,
        Err(e) => {
            println!("[usertests] t53: lookup Posix = Err({:?}) (atteso Ok)", e);
            return false;
        }
    };
    let pid = match civis::service_pid(civis::Service::Posix) {
        Ok(p) => p,
        Err(e) => {
            println!("[usertests] t53: service_pid Posix = Err({:?}) (atteso Ok)", e);
            return false;
        }
    };
    match civis::ps_info(pid as u32) {
        Some(e) if e.parent == Some(1) => {}
        other => {
            println!("[usertests] t53: posix pid={} parent illegittimo: {:?}", pid, other.map(|e| e.parent));
            return false;
        }
    }
    // (b) UNICA traduzione nativo→errno: tabella totale e fissata. Se una
    // variante futura nasce senza braccio qui, non compila (match totale).
    let table: [(civis::Error, i64); 17] = [
        (civis::Error::NotReady, libr::posix::EIO),
        (civis::Error::Pending, libr::posix::EAGAIN),
        (civis::Error::RingFull, libr::posix::EAGAIN),
        (civis::Error::ServerDied, libr::posix::EIO),
        (civis::Error::Denied, libr::posix::EACCES),
        (civis::Error::NoMemory, libr::posix::ENOMEM),
        (civis::Error::Busy, libr::posix::EBUSY),
        (civis::Error::Invalid, libr::posix::EINVAL),
        (civis::Error::Failed, libr::posix::EIO),
        (civis::Error::NotFound, libr::posix::ENOENT),
        (civis::Error::NotDir, libr::posix::ENOTDIR),
        (civis::Error::IsDir, libr::posix::EISDIR),
        (civis::Error::Exists, libr::posix::EEXIST),
        (civis::Error::ReadOnly, libr::posix::EROFS),
        (civis::Error::TooBig, libr::posix::EFBIG),
        (civis::Error::Empty, libr::posix::EAGAIN),
        (civis::Error::Closed, libr::posix::EPIPE),
    ];
    for (e, want) in table {
        let got = libr::posix::to_errno(e);
        if got != want {
            println!("[usertests] t53: to_errno({:?}) = {} (atteso {})", e, got, want);
            return false;
        }
    }
    // (c) Gate sul nuovo slot: helper non-figlio-di-init prova kill ostile +
    // register Init + register Posix — tutti e tre rifiutati (ok=true).
    let (b_chan, b_pid) = match helpers::spawn_cfg(
        "/test/testcli.bin", "utcli", 16, helpers::M_KILLME, 0,
    ) {
        Some(x) => x,
        None => {
            println!("[usertests] t53: spawn vittima FAILED");
            return false;
        }
    };
    let (h_chan, _) = match helpers::spawn_cfg(
        "/test/testcli.bin", "utcli", 16, helpers::M_HARDEN, b_pid,
    ) {
        Some(x) => x,
        None => {
            let _ = civis::kill(b_pid as i64, 0);
            let _ = helpers::wait_exit(b_chan);
            println!("[usertests] t53: spawn harden FAILED");
            return false;
        }
    };
    let (ok, detail) = helpers::recv_done(&[h_chan]);
    let victim_alive = civis::ps_info(b_pid as u32).is_some();
    let _ = civis::kill(b_pid as i64, 0);
    let _ = helpers::wait_exit(b_chan);
    if !ok || !victim_alive {
        println!("[usertests] t53: harden FAIL (ok={}, detail={}, victim_alive={})", ok, detail, victim_alive);
        return false;
    }
    true
}
