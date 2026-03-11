#![forbid(unsafe_op_in_unsafe_fn)]

pub use super::common::Env;
use crate::ffi::{OsStr, OsString};
use crate::io;
use crate::sys::wasmos;
use crate::vec;
use crate::vec::Vec;

pub fn env() -> Env {
    Env::new(load_env())
}

pub fn getenv(name: &OsStr) -> Option<OsString> {
    let mut scratch = [0u8; 4096];
    let len = wasmos::getenv(name, &mut scratch).ok()?;
    let bytes = if len <= scratch.len() {
        scratch[..len].to_vec()
    } else {
        let mut full = vec![0u8; len];
        let retry = wasmos::getenv(name, &mut full).ok()?;
        full.truncate(retry);
        full
    };
    Some(unsafe { OsString::from_encoded_bytes_unchecked(bytes) })
}

pub unsafe fn setenv(name: &OsStr, value: &OsStr) -> io::Result<()> {
    wasmos::setenv(name, value).map_err(wasmos::io_error)
}

pub unsafe fn unsetenv(name: &OsStr) -> io::Result<()> {
    wasmos::unsetenv(name).map_err(wasmos::io_error)
}

fn load_env() -> Vec<(OsString, OsString)> {
    let mut scratch = vec![0u8; 4096];
    let len = match wasmos::listenv(&mut scratch) {
        Ok(len) => len,
        Err(_) => return Vec::new(),
    };
    if len > scratch.len() {
        scratch.resize(len, 0);
        let Ok(retry) = wasmos::listenv(&mut scratch) else {
            return Vec::new();
        };
        scratch.truncate(retry);
    } else {
        scratch.truncate(len);
    }

    let mut vars = Vec::new();
    for entry in scratch.split(|byte| *byte == b'\n') {
        if entry.is_empty() {
            continue;
        }
        let Some(eq) = entry[1..].iter().position(|byte| *byte == b'=').map(|idx| idx + 1) else {
            continue;
        };
        vars.push((
            unsafe { OsString::from_encoded_bytes_unchecked(entry[..eq].to_vec()) },
            unsafe { OsString::from_encoded_bytes_unchecked(entry[eq + 1..].to_vec()) },
        ));
    }
    vars
}
