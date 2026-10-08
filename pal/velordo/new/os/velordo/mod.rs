//! Estensioni `std::os::velordo` (S1.3, bonifica POSIX): solo ffi per
//! OsString/OsStr, definizione propria byte-based (i path Velordo sono
//! byte-string; nessun riuso del modulo `unix`).

#![unstable(feature = "velordo_std", issue = "none")]
#![doc(cfg(target_os = "velordo"))]
#![forbid(unsafe_op_in_unsafe_fn)]

#[path = "os_str.rs"]
pub mod ffi;
