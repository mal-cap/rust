use crate::ffi::{OsStr, OsString};
use crate::fmt;
use crate::fs::TryLockError;
use crate::hash::{Hash, Hasher};
use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut, SeekFrom};
use crate::mem::ManuallyDrop;
use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, RawFd};
use crate::path::{Path, PathBuf};
pub use crate::sys::fs::common::{Dir, copy, exists, remove_dir_all};
use crate::sys::fd::FileDesc;
use crate::sys::path;
use crate::sys::time::{SystemTime, UNIX_EPOCH};
use crate::sys::{
    AsInner, FromInner, IntoInner, unsupported, unsupported_err,
    wasmos::{self, StatBuf},
};
use crate::time::Duration;
use crate::vec;
use crate::vec::Vec;

const FT_REG: u8 = 0;
const FT_DIR: u8 = 1;
const FT_CHR: u8 = 2;
const FT_LNK: u8 = 4;
const FT_FIFO: u8 = 5;
const FT_SOCK: u8 = 6;

const DT_DIR: u8 = 4;
const DT_LNK: u8 = 10;

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct FileType {
    raw: u8,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FilePermissions {
    mode: u32,
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub struct FileAttr {
    raw: StatBuf,
}

pub struct ReadDir {
    entries: crate::vec::IntoIter<DirEntry>,
}

pub struct DirEntry {
    parent: PathBuf,
    name: OsString,
    file_type: FileType,
}

#[derive(Clone, Debug, Default)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
    mode: u32,
    custom_flags: i32,
}

#[derive(Copy, Clone, Default, Debug)]
pub struct FileTimes {
    accessed: Option<SystemTime>,
    modified: Option<SystemTime>,
}

#[derive(Debug)]
pub struct File {
    fd: i32,
}

#[derive(Debug)]
pub struct DirBuilder {
    mode: u32,
}

fn system_time_from_ns(ns: u64) -> io::Result<SystemTime> {
    UNIX_EPOCH
        .checked_add_duration(&Duration::from_nanos(ns))
        .ok_or_else(|| io::const_error!(io::ErrorKind::InvalidData, "timestamp out of range"))
}

fn stat_inner(path: &Path, follow: bool) -> io::Result<FileAttr> {
    let mut raw = StatBuf::default();
    let res = if follow {
        wasmos::stat(path.as_os_str(), &mut raw)
    } else {
        wasmos::lstat(path.as_os_str(), &mut raw)
    };
    res.map_err(wasmos::io_error)?;
    Ok(FileAttr { raw })
}

fn fstat_inner(fd: i32) -> io::Result<FileAttr> {
    let mut raw = StatBuf::default();
    wasmos::fstat(fd, &mut raw).map_err(wasmos::io_error)?;
    Ok(FileAttr { raw })
}

impl FileType {
    pub fn is(&self, mode: u32) -> bool {
        match mode {
            0o060000 => false,
            0o020000 => self.raw == FT_CHR,
            0o010000 => self.raw == FT_FIFO,
            0o140000 => self.raw == FT_SOCK,
            _ => false,
        }
    }

    pub fn is_dir(&self) -> bool {
        self.raw == FT_DIR
    }

    pub fn is_file(&self) -> bool {
        self.raw == FT_REG || self.raw == FT_CHR
    }

    pub fn is_symlink(&self) -> bool {
        self.raw == FT_LNK
    }
}

impl FilePermissions {
    pub fn mode(&self) -> u32 {
        self.mode
    }

    pub fn set_mode(&mut self, mode: u32) {
        self.mode = mode;
    }

    pub fn from_mode(mode: u32) -> FilePermissions {
        FilePermissions { mode }
    }

    pub fn readonly(&self) -> bool {
        self.mode & 0o222 == 0
    }

    pub fn set_readonly(&mut self, readonly: bool) {
        if readonly {
            self.mode &= !0o222;
        } else {
            self.mode |= 0o200;
        }
    }
}

impl FileAttr {
    pub fn size(&self) -> u64 {
        self.raw.size
    }

    pub fn perm(&self) -> FilePermissions {
        FilePermissions { mode: self.raw.mode }
    }

    pub fn file_type(&self) -> FileType {
        FileType { raw: self.raw.file_type as u8 }
    }

    pub fn modified(&self) -> io::Result<SystemTime> {
        system_time_from_ns(self.raw.mtime_ns)
    }

    pub fn accessed(&self) -> io::Result<SystemTime> {
        system_time_from_ns(self.raw.atime_ns)
    }

    pub fn created(&self) -> io::Result<SystemTime> {
        system_time_from_ns(self.raw.ctime_ns)
    }
}

impl fmt::Debug for FileAttr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileAttr")
            .field("size", &self.size())
            .field("perm", &self.perm())
            .field("file_type", &self.file_type())
            .finish()
    }
}

impl Hash for FilePermissions {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.mode.hash(state);
    }
}

impl FileTimes {
    pub fn set_accessed(&mut self, t: SystemTime) {
        self.accessed = Some(t);
    }

    pub fn set_modified(&mut self, t: SystemTime) {
        self.modified = Some(t);
    }
}

impl fmt::Debug for ReadDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReadDir(..)")
    }
}

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<io::Result<DirEntry>> {
        self.entries.next().map(Ok)
    }
}

impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.parent.join(Path::new(&self.name))
    }

    pub fn file_name(&self) -> OsString {
        self.name.clone()
    }

    pub fn file_name_os_str(&self) -> &OsStr {
        self.name.as_os_str()
    }

    pub fn metadata(&self) -> io::Result<FileAttr> {
        stat(&self.path())
    }

    pub fn file_type(&self) -> io::Result<FileType> {
        Ok(self.file_type)
    }

    pub fn ino(&self) -> u64 {
        self.metadata().map(|meta| meta.raw.inode_id as u64).unwrap_or(0)
    }
}

impl OpenOptions {
    pub fn new() -> OpenOptions {
        OpenOptions { mode: 0o666, ..OpenOptions::default() }
    }

    pub fn read(&mut self, read: bool) {
        self.read = read;
    }

    pub fn write(&mut self, write: bool) {
        self.write = write;
    }

    pub fn append(&mut self, append: bool) {
        self.append = append;
        if append {
            self.write = true;
        }
    }

    pub fn truncate(&mut self, truncate: bool) {
        self.truncate = truncate;
    }

    pub fn create(&mut self, create: bool) {
        self.create = create;
    }

    pub fn create_new(&mut self, create_new: bool) {
        self.create_new = create_new;
    }

    pub fn mode(&mut self, mode: u32) {
        self.mode = mode;
    }

    pub fn custom_flags(&mut self, flags: i32) {
        self.custom_flags = flags;
    }

    fn access_flags(&self) -> io::Result<u32> {
        match (self.read, self.write, self.append) {
            (true, false, false) => Ok(wasmos::O_RDONLY),
            (false, true, false) => Ok(wasmos::O_WRONLY),
            (true, true, false) => Ok(wasmos::O_RDWR),
            (false, _, true) => Ok(wasmos::O_WRONLY | wasmos::O_APPEND),
            (true, _, true) => Ok(wasmos::O_RDWR | wasmos::O_APPEND),
            (false, false, false) => {
                if self.create || self.create_new || self.truncate {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "creating or truncating a file requires write or append access",
                    ))
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "must specify at least one of read, write, or append access",
                    ))
                }
            }
        }
    }

    fn creation_flags(&self) -> io::Result<u32> {
        match (self.write, self.append) {
            (true, false) | (false, true) => {}
            (false, false) => {
                if self.create || self.create_new || self.truncate {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "creating or truncating a file requires write or append access",
                    ));
                }
            }
            (true, true) => {}
        }

        if self.append && self.truncate && !self.create_new {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "append and truncate are mutually exclusive unless create_new is set",
            ));
        }

        Ok(match (self.create, self.truncate, self.create_new) {
            (false, false, false) => 0,
            (true, false, false) => wasmos::O_CREAT,
            (false, true, false) => wasmos::O_TRUNC,
            (true, true, false) => wasmos::O_CREAT | wasmos::O_TRUNC,
            (_, _, true) => wasmos::O_CREAT | wasmos::O_EXCL,
        })
    }
}

impl File {
    pub(crate) fn raw_fd(&self) -> i32 {
        self.fd
    }

    pub fn open(path: &Path, opts: &OpenOptions) -> io::Result<File> {
        let flags = opts.access_flags()? | opts.creation_flags()? | (opts.custom_flags as u32);
        let fd = wasmos::open(path.as_os_str(), flags).map_err(wasmos::io_error)?;
        if flags & wasmos::O_CREAT != 0 {
            let _ = wasmos::chmod(path.as_os_str(), opts.mode);
        }
        Ok(File { fd })
    }

    pub fn file_attr(&self) -> io::Result<FileAttr> {
        fstat_inner(self.fd)
    }

    pub fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        let dup = self.duplicate()?;
        dup.seek(SeekFrom::Start(offset))?;
        dup.read(buf)
    }

    pub fn read_buf_at(&self, cursor: BorrowedCursor<'_>, offset: u64) -> io::Result<()> {
        let dup = self.duplicate()?;
        dup.seek(SeekFrom::Start(offset))?;
        dup.read_buf(cursor)
    }

    pub fn read_vectored_at(&self, bufs: &mut [IoSliceMut<'_>], offset: u64) -> io::Result<usize> {
        let dup = self.duplicate()?;
        dup.seek(SeekFrom::Start(offset))?;
        dup.read_vectored(bufs)
    }

    pub fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        let dup = self.duplicate()?;
        dup.seek(SeekFrom::Start(offset))?;
        dup.write(buf)
    }

    pub fn write_vectored_at(&self, bufs: &[IoSlice<'_>], offset: u64) -> io::Result<usize> {
        let dup = self.duplicate()?;
        dup.seek(SeekFrom::Start(offset))?;
        dup.write_vectored(bufs)
    }

    pub fn fsync(&self) -> io::Result<()> {
        wasmos::fsync(self.fd).map_err(wasmos::io_error)
    }

    pub fn datasync(&self) -> io::Result<()> {
        self.fsync()
    }

    pub fn lock(&self) -> io::Result<()> {
        unsupported()
    }

    pub fn lock_shared(&self) -> io::Result<()> {
        unsupported()
    }

    pub fn try_lock(&self) -> Result<(), TryLockError> {
        Err(TryLockError::Error(unsupported_err()))
    }

    pub fn try_lock_shared(&self) -> Result<(), TryLockError> {
        Err(TryLockError::Error(unsupported_err()))
    }

    pub fn unlock(&self) -> io::Result<()> {
        unsupported()
    }

    pub fn truncate(&self, size: u64) -> io::Result<()> {
        wasmos::ftruncate(self.fd, size).map_err(wasmos::io_error)
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        wasmos::read(self.fd, buf).map_err(wasmos::io_error)
    }

    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        crate::io::default_read_vectored(|buf| self.read(buf), bufs)
    }

    pub fn is_read_vectored(&self) -> bool {
        false
    }

    pub fn read_buf(&self, cursor: BorrowedCursor<'_>) -> io::Result<()> {
        crate::io::default_read_buf(|buf| self.read(buf), cursor)
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

    pub fn flush(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn seek(&self, pos: SeekFrom) -> io::Result<u64> {
        let (offset, whence) = match pos {
            SeekFrom::Start(offset) => (
                i32::try_from(offset)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "seek offset out of range"))?,
                0,
            ),
            SeekFrom::Current(offset) => (
                i32::try_from(offset)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "seek offset out of range"))?,
                1,
            ),
            SeekFrom::End(offset) => (
                i32::try_from(offset)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "seek offset out of range"))?,
                2,
            ),
        };
        wasmos::seek(self.fd, offset, whence).map_err(wasmos::io_error)
    }

    pub fn size(&self) -> Option<io::Result<u64>> {
        Some(self.file_attr().map(|attr| attr.size()))
    }

    pub fn tell(&self) -> io::Result<u64> {
        self.seek(SeekFrom::Current(0))
    }

    pub fn duplicate(&self) -> io::Result<File> {
        wasmos::dup(self.fd)
            .map(|fd| File { fd })
            .map_err(wasmos::io_error)
    }

    pub fn set_permissions(&self, perm: FilePermissions) -> io::Result<()> {
        wasmos::fchmod(self.fd, perm.mode).map_err(wasmos::io_error)
    }

    pub fn set_times(&self, _times: FileTimes) -> io::Result<()> {
        unsupported()
    }
}

impl Drop for File {
    fn drop(&mut self) {
        let _ = wasmos::close(self.fd);
    }
}

impl IntoInner<FileDesc> for File {
    fn into_inner(self) -> FileDesc {
        let fd = ManuallyDrop::new(self).fd;
        unsafe { FileDesc::from_raw_fd(fd) }
    }
}

impl FromInner<FileDesc> for File {
    fn from_inner(file_desc: FileDesc) -> Self {
        Self { fd: file_desc.into_raw_fd() }
    }
}

impl FromInner<u32> for FilePermissions {
    fn from_inner(mode: u32) -> Self {
        FilePermissions { mode }
    }
}

impl AsRawFd for File {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl IntoRawFd for File {
    fn into_raw_fd(self) -> RawFd {
        ManuallyDrop::new(self).fd
    }
}

impl FromRawFd for File {
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
        Self { fd }
    }
}

impl DirBuilder {
    pub fn new() -> DirBuilder {
        DirBuilder { mode: 0o777 }
    }

    pub fn set_mode(&mut self, mode: u32) {
        self.mode = mode;
    }

    pub fn mkdir(&self, path: &Path) -> io::Result<()> {
        wasmos::mkdir(path.as_os_str()).map_err(wasmos::io_error)?;
        let _ = wasmos::chmod(path.as_os_str(), self.mode);
        Ok(())
    }
}

pub fn readdir(path: &Path) -> io::Result<ReadDir> {
    let dir = File::open(path, &{
        let mut opts = OpenOptions::new();
        opts.read(true);
        opts
    })?;
    let mut buf = vec![0u8; 262_144];
    let len = wasmos::readdir(dir.fd, &mut buf).map_err(wasmos::io_error)?;
    let data = &buf[..len];
    let mut entries = Vec::new();
    let mut offset = 0usize;
    let parent = path.to_path_buf();

    while offset + 19 <= data.len() {
        let rec_len = u16::from_le_bytes([data[offset + 16], data[offset + 17]]) as usize;
        if rec_len == 0 || offset + rec_len > data.len() {
            break;
        }
        let name_start = offset + 19;
        let name_end = data[name_start..offset + rec_len]
            .iter()
            .position(|&byte| byte == 0)
            .map(|name_len| name_start + name_len)
            .unwrap_or(offset + rec_len);
        let name_bytes = &data[name_start..name_end];
        if name_bytes != b"." && name_bytes != b".." {
            let raw_type = match data[offset + 18] {
                DT_DIR => FT_DIR,
                DT_LNK => FT_LNK,
                _ => FT_REG,
            };
            entries.push(DirEntry {
                parent: parent.clone(),
                name: unsafe { OsString::from_encoded_bytes_unchecked(name_bytes.to_vec()) },
                file_type: FileType { raw: raw_type },
            });
        }
        offset += rec_len;
    }

    Ok(ReadDir { entries: entries.into_iter() })
}

pub fn unlink(path: &Path) -> io::Result<()> {
    wasmos::unlink(path.as_os_str()).map_err(wasmos::io_error)
}

pub fn chown(_path: &Path, _uid: u32, _gid: u32) -> io::Result<()> {
    unsupported()
}

pub fn fchown(_fd: RawFd, _uid: u32, _gid: u32) -> io::Result<()> {
    unsupported()
}

pub fn lchown(_path: &Path, _uid: u32, _gid: u32) -> io::Result<()> {
    unsupported()
}

pub fn chroot(_path: &Path) -> io::Result<()> {
    unsupported()
}

pub fn mkfifo(_path: &Path, _mode: u32) -> io::Result<()> {
    unsupported()
}

pub fn rename(old: &Path, new: &Path) -> io::Result<()> {
    wasmos::rename(old.as_os_str(), new.as_os_str()).map_err(wasmos::io_error)
}

pub fn set_perm(path: &Path, perm: FilePermissions) -> io::Result<()> {
    wasmos::chmod(path.as_os_str(), perm.mode).map_err(wasmos::io_error)
}

pub fn set_times(path: &Path, times: FileTimes) -> io::Result<()> {
    set_times_impl(path, times, 0)
}

pub fn set_times_nofollow(path: &Path, times: FileTimes) -> io::Result<()> {
    const AT_SYMLINK_NOFOLLOW: i32 = 0x100;
    set_times_impl(path, times, AT_SYMLINK_NOFOLLOW)
}

fn set_times_impl(path: &Path, times: FileTimes, flags: i32) -> io::Result<()> {
    const AT_FDCWD: i32 = -100;
    const UTIME_OMIT: i32 = 0x3ffffffe;

    let encode_timespec = |opt: Option<SystemTime>| -> [u8; 16] {
        let mut buf = [0u8; 16];
        match opt {
            None => {
                buf[8..12].copy_from_slice(&UTIME_OMIT.to_le_bytes());
            }
            Some(t) => {
                let ns = t
                    .sub_time(&UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64;
                let sec = (ns / 1_000_000_000) as i64;
                let nsec = (ns % 1_000_000_000) as i32;
                buf[0..8].copy_from_slice(&sec.to_le_bytes());
                buf[8..12].copy_from_slice(&nsec.to_le_bytes());
                // buf[12..16] is _pad, already zero
            }
        }
        buf
    };

    let mut times_buf = [0u8; 32];
    times_buf[0..16].copy_from_slice(&encode_timespec(times.accessed));
    times_buf[16..32].copy_from_slice(&encode_timespec(times.modified));

    wasmos::utimensat(AT_FDCWD, path.as_os_str(), &times_buf, flags).map_err(wasmos::io_error)
}

pub fn rmdir(path: &Path) -> io::Result<()> {
    wasmos::rmdir(path.as_os_str()).map_err(wasmos::io_error)
}

pub fn readlink(path: &Path) -> io::Result<PathBuf> {
    let mut buf = vec![0u8; 4096];
    let len = wasmos::readlink(path.as_os_str(), &mut buf).map_err(wasmos::io_error)?;
    buf.truncate(len);
    Ok(PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(buf) }))
}

pub fn symlink(original: &Path, link: &Path) -> io::Result<()> {
    wasmos::symlink(original.as_os_str(), link.as_os_str()).map_err(wasmos::io_error)
}

pub fn link(src: &Path, dst: &Path) -> io::Result<()> {
    wasmos::link(src.as_os_str(), dst.as_os_str()).map_err(wasmos::io_error)
}

pub fn stat(path: &Path) -> io::Result<FileAttr> {
    stat_inner(path, true)
}

pub fn lstat(path: &Path) -> io::Result<FileAttr> {
    stat_inner(path, false)
}

pub fn canonicalize(path: &Path) -> io::Result<PathBuf> {
    path::absolute(path)
}

pub(crate) fn debug_assert_fd_is_open(_fd: i32) {}
