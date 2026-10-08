//! Estensioni `std::os::velordo` (S1.3): solo ffi per OsString/OsStr
//! (riuso del modulo unix, fatto apposta per questo).

#![unstable(feature = "velordo_std", issue = "none")]
#![doc(cfg(target_os = "velordo"))]
#![forbid(unsafe_op_in_unsafe_fn)]

#[path = "../unix/ffi/os_str.rs"]
pub mod ffi;
