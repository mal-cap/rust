use crate::os::fd::{AsFd, AsRawFd};

pub fn is_terminal(fd: &impl AsFd) -> bool {
    matches!(fd.as_fd().as_raw_fd(), 0..=2)
}
