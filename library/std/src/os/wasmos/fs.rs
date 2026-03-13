//! WasmOS-specific extensions to primitives in the `std::fs` module.

#![stable(feature = "metadata_ext", since = "1.1.0")]

use crate::fs::Metadata;
#[allow(deprecated)]
use crate::os::wasmos::raw;
use crate::sys::AsInner;

#[stable(feature = "metadata_ext", since = "1.1.0")]
pub trait MetadataExt {
    #[stable(feature = "metadata_ext", since = "1.1.0")]
    #[deprecated(since = "1.8.0", note = "other methods of this trait are now preferred")]
    #[allow(deprecated)]
    fn as_raw_stat(&self) -> &raw::stat;

    #[stable(feature = "metadata_ext2", since = "1.8.0")]
    fn st_dev(&self) -> u64;
    #[stable(feature = "metadata_ext2", since = "1.8.0")]
    fn st_ino(&self) -> u64;
    #[stable(feature = "metadata_ext2", since = "1.8.0")]
    fn st_mode(&self) -> u32;
    #[stable(feature = "metadata_ext2", since = "1.8.0")]
    fn st_nlink(&self) -> u64;
    #[stable(feature = "metadata_ext2", since = "1.8.0")]
    fn st_uid(&self) -> u32;
    #[stable(feature = "metadata_ext2", since = "1.8.0")]
    fn st_gid(&self) -> u32;
    #[stable(feature = "metadata_ext2", since = "1.8.0")]
    fn st_rdev(&self) -> u64;
    #[stable(feature = "metadata_ext2", since = "1.8.0")]
    fn st_size(&self) -> u64;
    #[stable(feature = "metadata_ext2", since = "1.8.0")]
    fn st_atime(&self) -> i64;
    #[stable(feature = "metadata_ext2", since = "1.8.0")]
    fn st_atime_nsec(&self) -> i64;
    #[stable(feature = "metadata_ext2", since = "1.8.0")]
    fn st_mtime(&self) -> i64;
    #[stable(feature = "metadata_ext2", since = "1.8.0")]
    fn st_mtime_nsec(&self) -> i64;
    #[stable(feature = "metadata_ext2", since = "1.8.0")]
    fn st_ctime(&self) -> i64;
    #[stable(feature = "metadata_ext2", since = "1.8.0")]
    fn st_ctime_nsec(&self) -> i64;
    #[stable(feature = "metadata_ext2", since = "1.8.0")]
    fn st_blksize(&self) -> u64;
    #[stable(feature = "metadata_ext2", since = "1.8.0")]
    fn st_blocks(&self) -> u64;
}

#[stable(feature = "metadata_ext", since = "1.1.0")]
impl MetadataExt for Metadata {
    #[allow(deprecated)]
    fn as_raw_stat(&self) -> &raw::stat {
        unsafe { &*(self.as_inner() as *const _ as *const raw::stat) }
    }

    fn st_dev(&self) -> u64 {
        0
    }

    fn st_ino(&self) -> u64 {
        self.as_raw_stat().st_ino
    }

    fn st_mode(&self) -> u32 {
        self.as_raw_stat().st_mode
    }

    fn st_nlink(&self) -> u64 {
        self.as_raw_stat().st_nlink.into()
    }

    fn st_uid(&self) -> u32 {
        self.as_raw_stat().st_uid
    }

    fn st_gid(&self) -> u32 {
        self.as_raw_stat().st_gid
    }

    fn st_rdev(&self) -> u64 {
        self.as_raw_stat().st_rdev
    }

    fn st_size(&self) -> u64 {
        self.as_raw_stat().st_size
    }

    fn st_atime(&self) -> i64 {
        (self.as_raw_stat().st_atime_ns / 1_000_000_000) as i64
    }

    fn st_atime_nsec(&self) -> i64 {
        (self.as_raw_stat().st_atime_ns % 1_000_000_000) as i64
    }

    fn st_mtime(&self) -> i64 {
        (self.as_raw_stat().st_mtime_ns / 1_000_000_000) as i64
    }

    fn st_mtime_nsec(&self) -> i64 {
        (self.as_raw_stat().st_mtime_ns % 1_000_000_000) as i64
    }

    fn st_ctime(&self) -> i64 {
        (self.as_raw_stat().st_ctime_ns / 1_000_000_000) as i64
    }

    fn st_ctime_nsec(&self) -> i64 {
        (self.as_raw_stat().st_ctime_ns % 1_000_000_000) as i64
    }

    fn st_blksize(&self) -> u64 {
        4096
    }

    fn st_blocks(&self) -> u64 {
        self.st_size().div_ceil(512)
    }
}
