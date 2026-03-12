use crate::io;
use crate::sys::wasmos;

pub fn errno() -> i32 {
    0
}

pub fn is_interrupted(errno: i32) -> bool {
    errno == wasmos::EINTR
}

pub fn decode_error_kind(errno: i32) -> io::ErrorKind {
    match errno {
        x if x == wasmos::ENOENT => io::ErrorKind::NotFound,
        x if x == wasmos::EINTR => io::ErrorKind::Interrupted,
        x if x == wasmos::EAGAIN => io::ErrorKind::WouldBlock,
        x if x == wasmos::EINVAL => io::ErrorKind::InvalidInput,
        x if x == wasmos::EBADF => io::ErrorKind::NotConnected,
        x if x == wasmos::ECHILD => io::ErrorKind::InvalidInput,
        x if x == wasmos::ETIMEDOUT => io::ErrorKind::TimedOut,
        x if x == wasmos::ENOSYS => io::ErrorKind::Unsupported,
        _ => io::ErrorKind::Uncategorized,
    }
}

pub fn error_string(errno: i32) -> String {
    match errno {
        0 => "operation successful",
        x if x == wasmos::ENOENT => "no such file or directory",
        x if x == wasmos::EINTR => "interrupted system call",
        x if x == wasmos::EAGAIN => "resource temporarily unavailable",
        x if x == wasmos::EINVAL => "invalid argument",
        x if x == wasmos::EBADF => "bad file descriptor",
        x if x == wasmos::ECHILD => "no child processes",
        x if x == wasmos::ETIMEDOUT => "timed out",
        x if x == wasmos::ENOSYS => "function not implemented",
        _ => "unknown error",
    }
    .into()
}
