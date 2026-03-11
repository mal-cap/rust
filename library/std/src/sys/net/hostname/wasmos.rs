use crate::env;
use crate::ffi::OsString;
use crate::io::Result;

pub fn hostname() -> Result<OsString> {
    Ok(env::var_os("HOSTNAME").unwrap_or_else(|| OsString::from("wasmos")))
}
