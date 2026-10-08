//! Errori OS Velordo (S1.3): niente errno (syscall con codici in-band).
//! Forma minima: errno fittizio 0, mai interrupted, tutto Other.

use crate::{fmt, io};

pub fn errno() -> io::RawOsError {
    0
}

pub fn is_interrupted(_code: io::RawOsError) -> bool {
    false
}

pub fn decode_error_kind(_code: io::RawOsError) -> io::ErrorKind {
    io::ErrorKind::Other
}

pub fn format_error(errno: io::RawOsError, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "Velordo OS error {errno}")
}
