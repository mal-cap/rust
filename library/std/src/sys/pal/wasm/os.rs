use crate::env;
use crate::ffi::{OsStr, OsString};
use crate::path::{self, PathBuf};
use crate::sys::wasmos;
use crate::{fmt, iter, slice};

const PATH_SEPARATOR: u8 = b':';

pub fn getcwd() -> crate::io::Result<PathBuf> {
    let mut buf = vec![0u8; 256];
    loop {
        match wasmos::getcwd(&mut buf) {
            Ok(len) => {
                buf.truncate(len);
                return Ok(PathBuf::from(unsafe {
                    OsString::from_encoded_bytes_unchecked(buf)
                }));
            }
            Err(errno) if errno == wasmos::EINVAL => {
                buf.resize(buf.len() * 2, 0);
            }
            Err(errno) => return Err(wasmos::io_error(errno)),
        }
    }
}

pub fn chdir(path: &path::Path) -> crate::io::Result<()> {
    wasmos::chdir(path.as_os_str()).map_err(wasmos::io_error)
}

pub type SplitPaths<'a> = iter::Map<
    slice::Split<'a, u8, impl FnMut(&u8) -> bool + 'static>,
    impl FnMut(&[u8]) -> PathBuf + 'static,
>;

#[define_opaque(SplitPaths)]
pub fn split_paths(unparsed: &OsStr) -> SplitPaths<'_> {
    fn is_separator(&b: &u8) -> bool {
        b == PATH_SEPARATOR
    }

    fn into_pathbuf(part: &[u8]) -> PathBuf {
        PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(part.to_vec()) })
    }

    unparsed.as_encoded_bytes().split(is_separator).map(into_pathbuf)
}

#[derive(Debug)]
pub struct JoinPathsError;

pub fn join_paths<I, T>(paths: I) -> Result<OsString, JoinPathsError>
where
    I: Iterator<Item = T>,
    T: AsRef<OsStr>,
{
    let mut joined = Vec::new();
    for (index, path) in paths.enumerate() {
        let bytes = path.as_ref().as_encoded_bytes();
        if bytes.contains(&PATH_SEPARATOR) {
            return Err(JoinPathsError);
        }
        if index != 0 {
            joined.push(PATH_SEPARATOR);
        }
        joined.extend_from_slice(bytes);
    }
    Ok(unsafe { OsString::from_encoded_bytes_unchecked(joined) })
}

impl fmt::Display for JoinPathsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "path segment contains separator `{}`", PATH_SEPARATOR as char)
    }
}

impl crate::error::Error for JoinPathsError {}

pub fn current_exe() -> crate::io::Result<PathBuf> {
    let Some(arg0) = env::args_os().next() else {
        return Err(crate::io::const_error!(
            crate::io::ErrorKind::NotFound,
            "an executable path was not found because no arguments were provided through argv",
        ));
    };

    let path = PathBuf::from(arg0);
    if path.is_absolute() {
        return Ok(path);
    }

    getcwd().map(|cwd| cwd.join(path))
}

pub fn temp_dir() -> PathBuf {
    PathBuf::from("/tmp")
}

pub fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}
