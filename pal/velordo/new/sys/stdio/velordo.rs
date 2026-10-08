//! Console Velordo (S1.3): stdout/stderr su SYS_WRITE (fd 1/2 → seriale),
//! stdin EOF. Forma di `stdio/unsupported.rs` con write reale.

use crate::sys::pal::write_fd as pal_write;
use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut};

pub struct Stdin;
pub struct Stdout;
pub type Stderr = Stdout;

impl Stdin {
    pub const fn new() -> Stdin {
        Stdin
    }
}

impl io::Read for Stdin {
    #[inline]
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Ok(0) // EOF: niente stdin interattivo in v1
    }

    #[inline]
    fn read_buf(&mut self, _cursor: BorrowedCursor<'_, u8>) -> io::Result<()> {
        Ok(())
    }

    #[inline]
    fn read_vectored(&mut self, _bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        Ok(0)
    }

    #[inline]
    fn is_read_vectored(&self) -> bool {
        false
    }

    #[inline]
    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        if !buf.is_empty() { Err(io::Error::READ_EXACT_EOF) } else { Ok(()) }
    }

    #[inline]
    fn read_buf_exact(&mut self, cursor: BorrowedCursor<'_, u8>) -> io::Result<()> {
        if cursor.capacity() != 0 { Err(io::Error::READ_EXACT_EOF) } else { Ok(()) }
    }

    #[inline]
    fn read_to_end(&mut self, _buf: &mut Vec<u8>) -> io::Result<usize> {
        Ok(0)
    }

    #[inline]
    fn read_to_string(&mut self, _buf: &mut String) -> io::Result<usize> {
        Ok(0)
    }
}

impl Stdout {
    pub const fn new() -> Stdout {
        Stdout
    }
}

impl io::Write for Stdout {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(pal_write(1, buf))
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        let mut n = 0;
        for b in bufs {
            n += pal_write(1, b);
        }
        Ok(n)
    }

    #[inline]
    fn is_write_vectored(&self) -> bool {
        true
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        // SYS_WRITE puo' scrivere corto: loop fino a tutto (mai perdita
        // silenziosa a meta' riga).
        let mut done = 0;
        while done < buf.len() {
            let n = pal_write(1, &buf[done..]);
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::WriteZero, "short write"));
            }
            done += n;
        }
        Ok(())
    }

    #[inline]
    fn write_all_vectored(&mut self, bufs: &mut [IoSlice<'_>]) -> io::Result<()> {
        for b in bufs {
            self.write_all(b)?;
        }
        Ok(())
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(()) // seriale senza buffering: niente da scaricare
    }
}

pub const STDIN_BUF_SIZE: usize = 0;

pub fn is_ebadf(_err: &io::Error) -> bool {
    true
}

pub fn panic_output() -> Option<Vec<u8>> {
    // Panic su seriale via Stderr diretto (vedi rt::panic_output): qui
    // niente (il writer lo prende rt da Stderr::new). None = default.
    None
}
