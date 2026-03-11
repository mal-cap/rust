use crate::ffi::OsStr;
use crate::io;

pub type Errno = i32;

pub const EBADF: Errno = 9;
pub const ENOSYS: Errno = 38;

pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const O_CREAT: u32 = 0x40;
pub const O_EXCL: u32 = 0x80;
pub const O_TRUNC: u32 = 0x200;
pub const O_APPEND: u32 = 0x400;

const SYS_OPEN: i32 = 20;
const SYS_CLOSE: i32 = 21;
const SYS_READ: i32 = 22;
const SYS_WRITE: i32 = 23;
const SYS_SEEK: i32 = 24;
const SYS_STAT: i32 = 25;
const SYS_FSTAT: i32 = 26;
const SYS_MKDIR: i32 = 27;
const SYS_UNLINK: i32 = 28;
const SYS_RMDIR: i32 = 29;
const SYS_READDIR: i32 = 30;
const SYS_RENAME: i32 = 31;
const SYS_DUP: i32 = 32;
const SYS_CHDIR: i32 = 36;
const SYS_GETCWD: i32 = 37;
const SYS_LINK: i32 = 38;
const SYS_SYMLINK: i32 = 39;
const SYS_GETARGC: i32 = 71;
const SYS_GETARG: i32 = 72;
const SYS_CHMOD: i32 = 73;
const SYS_FTRUNCATE: i32 = 77;
const SYS_FSYNC: i32 = 78;
const SYS_LSTAT: i32 = 79;
const SYS_READLINK: i32 = 80;

unsafe extern "C" {
    fn __wasmos_syscall(nr: i32, a0: i32, a1: i32, a2: i32, a3: i32, a4: i32, a5: i32) -> i64;
}

#[inline]
fn decode(packed: i64) -> Result<i32, Errno> {
    let value = packed as i32;
    let errno = (packed >> 32) as i32;
    if errno == 0 { Ok(value) } else { Err(errno) }
}

#[inline]
fn call(nr: i32, a0: i32, a1: i32, a2: i32, a3: i32, a4: i32, a5: i32) -> Result<i32, Errno> {
    // SAFETY: This is the sole syscall FFI boundary for the WasmOS target.
    decode(unsafe { __wasmos_syscall(nr, a0, a1, a2, a3, a4, a5) })
}

#[inline]
pub fn io_error(errno: Errno) -> io::Error {
    io::Error::from_raw_os_error(errno)
}

#[inline]
fn call_path<T>(path: &OsStr, tail: impl FnOnce(*const u8, usize) -> Result<T, Errno>) -> Result<T, Errno> {
    let bytes = path.as_encoded_bytes();
    tail(bytes.as_ptr(), bytes.len())
}

pub fn open(path: &OsStr, flags: u32) -> Result<i32, Errno> {
    call_path(path, |ptr, len| call(SYS_OPEN, ptr as i32, len as i32, flags as i32, 0, 0, 0))
}

pub fn close(fd: i32) -> Result<(), Errno> {
    call(SYS_CLOSE, fd, 0, 0, 0, 0, 0).map(|_| ())
}

pub fn read(fd: i32, buf: &mut [u8]) -> Result<usize, Errno> {
    call(SYS_READ, fd, buf.as_mut_ptr() as i32, buf.len() as i32, 0, 0, 0).map(|v| v as usize)
}

pub fn write(fd: i32, buf: &[u8]) -> Result<usize, Errno> {
    call(SYS_WRITE, fd, buf.as_ptr() as i32, buf.len() as i32, 0, 0, 0).map(|v| v as usize)
}

pub fn seek(fd: i32, offset: i32, whence: i32) -> Result<u64, Errno> {
    call(SYS_SEEK, fd, offset, whence, 0, 0, 0).map(|v| v as u32 as u64)
}

pub fn stat(path: &OsStr, buf: &mut [u8]) -> Result<(), Errno> {
    call_path(path, |ptr, len| {
        call(SYS_STAT, ptr as i32, len as i32, buf.as_mut_ptr() as i32, 0, 0, 0).map(|_| ())
    })
}

pub fn lstat(path: &OsStr, buf: &mut [u8]) -> Result<(), Errno> {
    call_path(path, |ptr, len| {
        call(SYS_LSTAT, ptr as i32, len as i32, buf.as_mut_ptr() as i32, 0, 0, 0).map(|_| ())
    })
}

pub fn fstat(fd: i32, buf: &mut [u8]) -> Result<(), Errno> {
    call(SYS_FSTAT, fd, buf.as_mut_ptr() as i32, 0, 0, 0, 0).map(|_| ())
}

pub fn mkdir(path: &OsStr) -> Result<(), Errno> {
    call_path(path, |ptr, len| call(SYS_MKDIR, ptr as i32, len as i32, 0, 0, 0, 0).map(|_| ()))
}

pub fn unlink(path: &OsStr) -> Result<(), Errno> {
    call_path(path, |ptr, len| call(SYS_UNLINK, ptr as i32, len as i32, 0, 0, 0, 0).map(|_| ()))
}

pub fn rmdir(path: &OsStr) -> Result<(), Errno> {
    call_path(path, |ptr, len| call(SYS_RMDIR, ptr as i32, len as i32, 0, 0, 0, 0).map(|_| ()))
}

pub fn readdir(fd: i32, buf: &mut [u8]) -> Result<usize, Errno> {
    call(SYS_READDIR, fd, buf.as_mut_ptr() as i32, buf.len() as i32, 0, 0, 0).map(|v| v as usize)
}

pub fn rename(old: &OsStr, new: &OsStr) -> Result<(), Errno> {
    let old_bytes = old.as_encoded_bytes();
    let new_bytes = new.as_encoded_bytes();
    call(
        SYS_RENAME,
        old_bytes.as_ptr() as i32,
        old_bytes.len() as i32,
        new_bytes.as_ptr() as i32,
        new_bytes.len() as i32,
        0,
        0,
    )
    .map(|_| ())
}

pub fn dup(fd: i32) -> Result<i32, Errno> {
    call(SYS_DUP, fd, 0, 0, 0, 0, 0)
}

pub fn getcwd(buf: &mut [u8]) -> Result<usize, Errno> {
    call(SYS_GETCWD, buf.as_mut_ptr() as i32, buf.len() as i32, 0, 0, 0, 0).map(|v| v as usize)
}

pub fn link(src: &OsStr, dst: &OsStr) -> Result<(), Errno> {
    let src_bytes = src.as_encoded_bytes();
    let dst_bytes = dst.as_encoded_bytes();
    call(
        SYS_LINK,
        src_bytes.as_ptr() as i32,
        src_bytes.len() as i32,
        dst_bytes.as_ptr() as i32,
        dst_bytes.len() as i32,
        0,
        0,
    )
    .map(|_| ())
}

pub fn symlink(target: &OsStr, link_path: &OsStr) -> Result<(), Errno> {
    let target_bytes = target.as_encoded_bytes();
    let link_bytes = link_path.as_encoded_bytes();
    call(
        SYS_SYMLINK,
        target_bytes.as_ptr() as i32,
        target_bytes.len() as i32,
        link_bytes.as_ptr() as i32,
        link_bytes.len() as i32,
        0,
        0,
    )
    .map(|_| ())
}

pub fn chmod(path: &OsStr, mode: u32) -> Result<(), Errno> {
    call_path(path, |ptr, len| call(SYS_CHMOD, ptr as i32, len as i32, mode as i32, 0, 0, 0).map(|_| ()))
}

pub fn ftruncate(fd: i32, len: u64) -> Result<(), Errno> {
    call(SYS_FTRUNCATE, fd, len as i32, (len >> 32) as i32, 0, 0, 0).map(|_| ())
}

pub fn fsync(fd: i32) -> Result<(), Errno> {
    call(SYS_FSYNC, fd, 0, 0, 0, 0, 0).map(|_| ())
}

pub fn readlink(path: &OsStr, buf: &mut [u8]) -> Result<usize, Errno> {
    call_path(path, |ptr, len| {
        call(
            SYS_READLINK,
            ptr as i32,
            len as i32,
            buf.as_mut_ptr() as i32,
            buf.len() as i32,
            0,
            0,
        )
        .map(|v| v as usize)
    })
}

pub fn getargc() -> Result<usize, Errno> {
    call(SYS_GETARGC, 0, 0, 0, 0, 0, 0).map(|v| v as usize)
}

pub fn getarg(index: usize, buf: &mut [u8]) -> Result<usize, Errno> {
    call(SYS_GETARG, index as i32, buf.as_mut_ptr() as i32, buf.len() as i32, 0, 0, 0)
        .map(|v| v as usize)
}
