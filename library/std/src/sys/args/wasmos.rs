#![forbid(unsafe_op_in_unsafe_fn)]

pub use super::common::Args;
use crate::ffi::OsString;
use crate::sys::wasmos;
use crate::vec;
use crate::vec::Vec;

pub fn args() -> Args {
    Args::new(load_args().unwrap_or_default())
}

fn load_args() -> Option<Vec<OsString>> {
    let argc = wasmos::getargc().ok()?;
    let mut args = Vec::with_capacity(argc);
    let mut scratch = [0u8; 4096];

    for index in 0..argc {
        let len = wasmos::getarg(index, &mut scratch).ok()?;
        let bytes = if len <= scratch.len() {
            scratch[..len].to_vec()
        } else {
            let mut full = vec![0u8; len];
            let retry = wasmos::getarg(index, &mut full).ok()?;
            full.truncate(retry);
            full
        };
        args.push(unsafe { OsString::from_encoded_bytes_unchecked(bytes) });
    }

    Some(args)
}
