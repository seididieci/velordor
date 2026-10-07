// Directory di lavoro per-processo (S1.1): vive sul LEADER del gruppo (i
// thread la condividono come POSIX), sopravvive a exec (persona, non
// immagine), ereditata da fork/spawn. Il kernel e' byte-opaco: niente
// normalizzazione (`..` testuale), solo assoluto + bound 256.
use super::entry::current_id;

/// chdir(path_ptr, len): imposta la cwd del gruppo. Solo path assoluti
/// (il client unisce i relativi alla cwd cached); oltre CWD_MAX, vuota o
/// non assoluta = -1. Non verifica l'esistenza (il client fa stat prima:
// il kernel non ha FS).
pub(super) fn sys_chdir(path_ptr: u64, len: usize) -> i64 {
    if len == 0 || len > syscall_numbers::CWD_MAX {
        return -1;
    }
    if !crate::arc::vmm_user::is_user_range(path_ptr, len) {
        return -1;
    }
    let bytes = unsafe { core::slice::from_raw_parts(path_ptr as *const u8, len) };
    if bytes.first() != Some(&b'/') {
        return -1; // solo assoluti (i relativi li unisce civis)
    }
    crate::ordo::sched::set_cwd(current_id() as usize, bytes)
}

/// getcwd(buf_ptr, cap): copia cwd + NUL, ritorna len senza NUL. Cap corta
/// (len + 1) = -1 (il chiamante rialloca, pattern boot_cmdline).
pub(super) fn sys_getcwd(buf_ptr: u64, cap: usize) -> i64 {
    let mut tmp = [0u8; syscall_numbers::CWD_MAX];
    let len = match crate::ordo::sched::get_cwd(current_id() as usize, &mut tmp) {
        Some(n) => n,
        None => return -1,
    };
    if cap < len + 1 {
        return -1;
    }
    if !crate::arc::vmm_user::is_user_range(buf_ptr, len + 1) {
        return -1;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf_ptr as *mut u8, len);
        core::ptr::write((buf_ptr as *mut u8).add(len), 0);
    }
    len as i64
}
