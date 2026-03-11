#![unstable(reason = "not public", issue = "none", feature = "fd")]

use crate::fmt;
use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut};
use crate::sys::wasmos;

pub struct Pipe {
    fd: i32,
}

pub fn pipe() -> io::Result<(Pipe, Pipe)> {
    let mut fds = [0; 2];
    wasmos::pipe(&mut fds).map_err(wasmos::io_error)?;
    Ok((Pipe { fd: fds[0] }, Pipe { fd: fds[1] }))
}

impl Pipe {
    pub(crate) fn from_fd(fd: i32) -> Pipe {
        Pipe { fd }
    }

    pub(crate) fn raw_fd(&self) -> i32 {
        self.fd
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        wasmos::dup(self.fd)
            .map(Pipe::from_fd)
            .map_err(wasmos::io_error)
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        wasmos::read(self.fd, buf).map_err(wasmos::io_error)
    }

    pub fn read_buf(&self, cursor: BorrowedCursor<'_>) -> io::Result<()> {
        crate::io::default_read_buf(|buf| self.read(buf), cursor)
    }

    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        crate::io::default_read_vectored(|buf| self.read(buf), bufs)
    }

    pub fn is_read_vectored(&self) -> bool {
        false
    }

    pub fn read_to_end(&self, buf: &mut Vec<u8>) -> io::Result<usize> {
        let start = buf.len();
        let mut chunk = [0u8; 8192];
        loop {
            let read = self.read(&mut chunk)?;
            if read == 0 {
                return Ok(buf.len() - start);
            }
            buf.extend_from_slice(&chunk[..read]);
        }
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        wasmos::write(self.fd, buf).map_err(wasmos::io_error)
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        crate::io::default_write_vectored(|buf| self.write(buf), bufs)
    }

    pub fn is_write_vectored(&self) -> bool {
        false
    }

    pub fn diverge(&self) -> ! {
        panic!("diverge called on supported WasmOS pipe")
    }
}

impl io::Read for &Pipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        Pipe::read(self, buf)
    }
}

impl io::Write for &Pipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Pipe::write(self, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        let _ = wasmos::close(self.fd);
    }
}

impl fmt::Debug for Pipe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pipe").field("fd", &self.fd).finish()
    }
}
