//! Estensioni ffi per `OsString`/`OsStr` su Velordo (bonifica POSIX).
//!
//! I path Velordo sono byte-string arbitrarie (ArcaFS/cardo non impongono
//! UTF-8: i nomi vivono come byte nei nodi), quindi `OsStr` e' una vista su
//! byte come sulle piattaforme byte-based — ma la definizione e' NOSTRA,
//! in `std::os::velordo`, senza dipendere dal modulo `unix`.

use crate::ffi::{OsStr, OsString};
use crate::mem;
use crate::sys::os_str::Buf;
use crate::sys::{AsInner, FromInner, IntoInner};

/// Platform-specific extensions to [`OsString`].
#[stable(feature = "rust1", since = "1.0.0")]
pub impl(self) trait OsStringExt {
    /// Creates an [`OsString`] from a byte vector.
    ///
    /// See the module documentation for an example.
    #[stable(feature = "rust1", since = "1.0.0")]
    fn from_vec(vec: Vec<u8>) -> Self;

    /// Yields the underlying byte vector of this [`OsString`].
    ///
    /// See the module documentation for an example.
    #[stable(feature = "rust1", since = "1.0.0")]
    fn into_vec(self) -> Vec<u8>;
}

#[stable(feature = "rust1", since = "1.0.0")]
impl OsStringExt for OsString {
    #[inline]
    fn from_vec(vec: Vec<u8>) -> OsString {
        FromInner::from_inner(Buf { inner: vec })
    }
    #[inline]
    fn into_vec(self) -> Vec<u8> {
        self.into_inner().inner
    }
}

/// Platform-specific extensions to [`OsStr`].
#[stable(feature = "rust1", since = "1.0.0")]
pub impl(self) trait OsStrExt {
    #[stable(feature = "rust1", since = "1.0.0")]
    /// Creates an [`OsStr`] from a byte slice.
    ///
    /// See the module documentation for an example.
    fn from_bytes(slice: &[u8]) -> &Self;

    /// Gets the underlying byte view of the [`OsStr`] slice.
    ///
    /// See the module documentation for an example.
    #[stable(feature = "rust1", since = "1.0.0")]
    fn as_bytes(&self) -> &[u8];
}

#[stable(feature = "rust1", since = "1.0.0")]
impl OsStrExt for OsStr {
    #[inline]
    fn from_bytes(slice: &[u8]) -> &OsStr {
        unsafe { mem::transmute(slice) }
    }
    #[inline]
    fn as_bytes(&self) -> &[u8] {
        &self.as_inner().inner
    }
}
