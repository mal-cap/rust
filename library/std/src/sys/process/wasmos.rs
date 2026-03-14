#![unstable(feature = "process_internals", issue = "none")]

use super::CommandEnvs;
use super::env::CommandEnv;
use crate::boxed::Box;
pub use crate::ffi::OsString as EnvKey;
use crate::ffi::{OsStr, OsString};
use crate::num::NonZero;
use crate::os::fd::AsRawFd;
use crate::path::{Path, PathBuf};
use crate::process::StdioPipes;
use crate::sys::fd::FileDesc;
use crate::sys::fs::File;
use crate::sys::pipe::{Pipe, pipe};
use crate::sys::{wasmos, os};
use crate::vec;
use crate::vec::Vec;
use crate::{fmt, fs, io};

static mut OUTPUT_CAPTURE_SEQ: u32 = 0;

const FDOP_CLOSE: i32 = 1;
const FDOP_DUP2: i32 = 2;
const FDOP_OPEN: i32 = 3;
const FDOP_CHDIR: i32 = 4;
const SIGKILL: i32 = 9;
const STDIN_FD: i32 = 0;
const STDOUT_FD: i32 = 1;
const STDERR_FD: i32 = 2;

#[repr(C)]
struct PosixSpawnFileActionsWire {
    _pad0: [u32; 2],
    actions: u32,
    _pad: [u32; 16],
}

#[repr(C)]
struct PosixSpawnFdopWire {
    next: u32,
    _prev: u32,
    cmd: i32,
    fd: i32,
    srcfd: i32,
    oflag: i32,
    mode: u32,
}

pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    env: CommandEnv,
    cwd: Option<OsString>,
    stdin: Option<Stdio>,
    stdout: Option<Stdio>,
    stderr: Option<Stdio>,
    uid: Option<u32>,
    gid: Option<u32>,
    groups: Option<Box<[u32]>>,
    pgroup: Option<i32>,
    chroot: Option<OsString>,
    setsid: bool,
    pre_exec: Vec<Box<dyn FnMut() -> io::Result<()> + Send + Sync>>,
}

#[derive(Debug)]
pub enum Stdio {
    Inherit,
    Null,
    MakePipe,
    ParentStdout,
    ParentStderr,
    Fd(FileDesc),
    InheritFile(File),
    Pipe(Pipe),
}

impl Command {
    pub fn new(program: &OsStr) -> Command {
        Command {
            program: program.to_owned(),
            args: vec![program.to_owned()],
            env: Default::default(),
            cwd: None,
            stdin: None,
            stdout: None,
            stderr: None,
            uid: None,
            gid: None,
            groups: None,
            pgroup: None,
            chroot: None,
            setsid: false,
            pre_exec: Vec::new(),
        }
    }

    pub fn set_arg_0(&mut self, arg: &OsStr) {
        self.args[0] = arg.to_owned();
    }

    pub fn arg(&mut self, arg: &OsStr) {
        self.args.push(arg.to_owned());
    }

    pub fn env_mut(&mut self) -> &mut CommandEnv {
        &mut self.env
    }

    pub fn cwd(&mut self, dir: &OsStr) {
        self.cwd = Some(dir.to_owned());
    }

    pub fn uid(&mut self, id: u32) {
        self.uid = Some(id);
    }

    pub fn gid(&mut self, id: u32) {
        self.gid = Some(id);
    }

    pub fn groups(&mut self, groups: &[u32]) {
        self.groups = Some(Box::from(groups));
    }

    pub fn pgroup(&mut self, pgroup: i32) {
        self.pgroup = Some(pgroup);
    }

    pub fn chroot(&mut self, dir: &Path) {
        self.chroot = Some(dir.as_os_str().to_owned());
        if self.cwd.is_none() {
            self.cwd = Some(OsString::from("/"));
        }
    }

    pub fn setsid(&mut self, setsid: bool) {
        self.setsid = setsid;
    }

    pub unsafe fn pre_exec(&mut self, f: Box<dyn FnMut() -> io::Result<()> + Send + Sync>) {
        self.pre_exec.push(f);
    }

    pub fn stdin(&mut self, stdin: Stdio) {
        self.stdin = Some(stdin);
    }

    pub fn stdout(&mut self, stdout: Stdio) {
        self.stdout = Some(stdout);
    }

    pub fn stderr(&mut self, stderr: Stdio) {
        self.stderr = Some(stderr);
    }

    pub fn get_program(&self) -> &OsStr {
        &self.program
    }

    pub fn get_args(&self) -> CommandArgs<'_> {
        let mut iter = self.args.iter();
        iter.next();
        CommandArgs { iter }
    }

    pub fn get_envs(&self) -> CommandEnvs<'_> {
        self.env.iter()
    }

    pub fn get_env_clear(&self) -> bool {
        self.env.does_clear()
    }

    pub fn get_current_dir(&self) -> Option<&Path> {
        self.cwd.as_ref().map(Path::new)
    }

    pub fn spawn(
        &mut self,
        default: Stdio,
        needs_stdin: bool,
    ) -> io::Result<(Process, StdioPipes)> {
        self.ensure_supported_extensions()?;
        let program = self.resolve_program()?;
        self.validate_cwd()?;

        let stdin = if let Some(stdin) = self.stdin.take() {
            stdin
        } else if needs_stdin {
            default
        } else {
            Stdio::Null
        };
        let stdout = self.stdout.take().unwrap_or(Stdio::Inherit);
        let stderr = self.stderr.take().unwrap_or(Stdio::Inherit);

        let stdin_pipe = if matches!(stdin, Stdio::MakePipe) { Some(pipe()?) } else { None };
        let stdout_pipe = if matches!(stdout, Stdio::MakePipe) { Some(pipe()?) } else { None };
        let stderr_pipe = if matches!(stderr, Stdio::MakePipe) { Some(pipe()?) } else { None };

        let env_pairs = self.env.capture();
        let argv_bufs = self
            .args
            .iter()
            .map(|arg| nul_terminated(arg.as_os_str()))
            .collect::<Vec<_>>();
        let mut argv_ptrs = argv_bufs.iter().map(|arg| arg.as_ptr() as u32).collect::<Vec<_>>();
        argv_ptrs.push(0);

        let env_bufs = env_pairs
            .iter()
            .map(|(key, value)| {
                let mut entry = Vec::with_capacity(
                    key.as_encoded_bytes().len() + value.as_encoded_bytes().len() + 2,
                );
                entry.extend_from_slice(key.as_encoded_bytes());
                entry.push(b'=');
                entry.extend_from_slice(value.as_encoded_bytes());
                entry.push(0);
                entry
            })
            .collect::<Vec<_>>();
        let mut env_ptrs = env_bufs.iter().map(|entry| entry.as_ptr() as u32).collect::<Vec<_>>();
        env_ptrs.push(0);

        let mut action_head = 0u32;
        let mut action_nodes = Vec::<Box<[u8]>>::new();
        for action in [
            self.cwd_action(),
            stdin_action(&stdin, stdin_pipe.as_ref()),
            stdout_action(&stdout, stdout_pipe.as_ref()),
            stderr_action(&stderr, stderr_pipe.as_ref()),
        ]
        .into_iter()
        .flatten()
        {
            let node = build_action_node(action, action_head);
            action_head = node.as_ptr() as u32;
            action_nodes.push(node);
        }

        let file_actions = PosixSpawnFileActionsWire {
            _pad0: [0, 0],
            actions: action_head,
            _pad: [0; 16],
        };
        let file_actions_ptr = if action_head == 0 {
            0
        } else {
            (&file_actions as *const PosixSpawnFileActionsWire) as u32
        };

        let child_pid = match wasmos::posix_spawn(
            program.as_os_str(),
            argv_ptrs.as_ptr(),
            env_ptrs.as_ptr(),
            0,
            file_actions_ptr,
        ) {
            Ok(pid) => pid,
            Err(errno) => {
                drop(stdin_pipe);
                drop(stdout_pipe);
                drop(stderr_pipe);
                return Err(wasmos::io_error(errno));
            }
        };

        // Apply process group if requested
        if let Some(pgid) = self.pgroup {
            let _ = wasmos::setpgid(child_pid, pgid as u32);
        }
        if self.setsid {
            // posix_spawn model: parent sets a new process group equal to child's pid
            // as a best-effort approximation of setsid semantics.
            let _ = wasmos::setpgid(child_pid, child_pid);
        }

        let stdin_parent = stdin_pipe.map(|(child_end, parent_end)| {
            drop(child_end);
            parent_end
        });
        let stdout_parent = stdout_pipe.map(|(parent_end, child_end)| {
            drop(child_end);
            parent_end
        });
        let stderr_parent = stderr_pipe.map(|(parent_end, child_end)| {
            drop(child_end);
            parent_end
        });

        Ok((
            Process { pid: child_pid },
            StdioPipes {
                stdin: stdin_parent,
                stdout: stdout_parent,
                stderr: stderr_parent,
            },
        ))
    }

    fn resolve_program(&self) -> io::Result<PathBuf> {
        let raw = self.program.as_encoded_bytes();
        if raw.contains(&b'/') {
            let path = PathBuf::from(self.program.clone());
            fs::metadata(&path)?;
            return Ok(path);
        }

        let Some(path_var) = crate::env::var_os("PATH") else {
            return Err(io::const_error!(io::ErrorKind::NotFound, "program not found on PATH"));
        };

        for dir in os::split_paths(&path_var) {
            let candidate = dir.join(&self.program);
            if let Ok(meta) = fs::metadata(&candidate)
                && meta.is_file()
            {
                return Ok(candidate);
            }
        }

        Err(io::const_error!(io::ErrorKind::NotFound, "program not found on PATH"))
    }

    fn validate_cwd(&self) -> io::Result<()> {
        let Some(dir) = self.cwd.as_ref() else {
            return Ok(());
        };
        let meta = fs::metadata(Path::new(dir))?;
        if meta.is_dir() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "current_dir is not a directory",
            ))
        }
    }

    fn ensure_supported_extensions(&self) -> io::Result<()> {
        if self.uid.is_some()
            || self.gid.is_some()
            || self.groups.is_some()
            || self.chroot.is_some()
            || !self.pre_exec.is_empty()
        {
            return Err(io::const_error!(
                io::ErrorKind::Unsupported,
                "unix process extensions are not supported on WasmOS yet"
            ));
        }
        Ok(())
    }

    fn cwd_action(&self) -> Option<SpawnAction> {
        self.cwd
            .as_ref()
            .map(|dir| SpawnAction::Chdir { path: nul_terminated(dir.as_os_str()) })
    }
}

pub fn output(cmd: &mut Command) -> io::Result<(ExitStatus, Vec<u8>, Vec<u8>)> {
    cmd.ensure_supported_extensions()?;
    let program = cmd.resolve_program()?;
    cmd.validate_cwd()?;

    let stdin = cmd.stdin.take().unwrap_or(Stdio::Null);

    let capture_id = next_output_capture_id();
    let stdout_path = capture_path("stdout", capture_id);
    let stderr_path = capture_path("stderr", capture_id);
    fs::File::create(&stdout_path)?;
    fs::File::create(&stderr_path)?;

    let argv_bufs = cmd
        .args
        .iter()
        .map(|arg| nul_terminated(arg.as_os_str()))
        .collect::<Vec<_>>();
    let mut argv_ptrs = argv_bufs.iter().map(|arg| arg.as_ptr() as u32).collect::<Vec<_>>();
    argv_ptrs.push(0);

    let env_pairs = cmd.env.capture();
    let env_bufs = env_pairs
        .iter()
        .map(|(key, value)| {
            let mut entry = Vec::with_capacity(
                key.as_encoded_bytes().len() + value.as_encoded_bytes().len() + 2,
            );
            entry.extend_from_slice(key.as_encoded_bytes());
            entry.push(b'=');
            entry.extend_from_slice(value.as_encoded_bytes());
            entry.push(0);
            entry
        })
        .collect::<Vec<_>>();
    let mut env_ptrs = env_bufs.iter().map(|entry| entry.as_ptr() as u32).collect::<Vec<_>>();
    env_ptrs.push(0);

    let mut action_head = 0u32;
    let mut action_nodes = Vec::<Box<[u8]>>::new();
    for action in [
        cmd.cwd_action(),
        stdin_action(&stdin, None),
        Some(SpawnAction::Open {
            target_fd: STDOUT_FD,
            flags: wasmos::O_WRONLY | wasmos::O_CREAT | wasmos::O_TRUNC,
            path: nul_terminated(stdout_path.as_os_str()),
        }),
        Some(SpawnAction::Open {
            target_fd: STDERR_FD,
            flags: wasmos::O_WRONLY | wasmos::O_CREAT | wasmos::O_TRUNC,
            path: nul_terminated(stderr_path.as_os_str()),
        }),
    ]
    .into_iter()
    .flatten()
    {
        let node = build_action_node(action, action_head);
        action_head = node.as_ptr() as u32;
        action_nodes.push(node);
    }

    let file_actions = PosixSpawnFileActionsWire {
        _pad0: [0, 0],
        actions: action_head,
        _pad: [0; 16],
    };
    let child_pid = wasmos::posix_spawn(
        program.as_os_str(),
        argv_ptrs.as_ptr(),
        env_ptrs.as_ptr(),
        0,
        (&file_actions as *const PosixSpawnFileActionsWire) as u32,
    )
    .map_err(wasmos::io_error)?;

    let mut raw_status = 0;
    wasmos::waitpid(child_pid as i32, &mut raw_status).map_err(wasmos::io_error)?;
    let stdout = fs::read(&stdout_path)?;
    let stderr = fs::read(&stderr_path)?;
    let _ = fs::remove_file(&stdout_path);
    let _ = fs::remove_file(&stderr_path);
    Ok((ExitStatus::new(raw_status), stdout, stderr))
}

fn stdin_action(stdio: &Stdio, pipe: Option<&(Pipe, Pipe)>) -> Option<SpawnAction> {
    match stdio {
        Stdio::Inherit => None,
        Stdio::Null => Some(SpawnAction::Open {
            target_fd: STDIN_FD,
            flags: wasmos::O_RDONLY,
            path: nul_terminated(OsStr::new("/dev/null")),
        }),
        Stdio::MakePipe => {
            let (child_end, parent_end) = pipe.expect("stdin pipe must exist");
            Some(SpawnAction::Pipe {
                child_fd: child_end.raw_fd(),
                target_fd: STDIN_FD,
                parent_fd: parent_end.raw_fd(),
            })
        }
        Stdio::Fd(fd) => Some(SpawnAction::Dup {
            src_fd: fd.as_raw_fd(),
            target_fd: STDIN_FD,
        }),
        Stdio::InheritFile(file) => Some(SpawnAction::Dup {
            src_fd: file.raw_fd(),
            target_fd: STDIN_FD,
        }),
        Stdio::Pipe(pipe) => Some(SpawnAction::Dup {
            src_fd: pipe.raw_fd(),
            target_fd: STDIN_FD,
        }),
        Stdio::ParentStdout | Stdio::ParentStderr => None,
    }
}

fn stdout_action(stdio: &Stdio, pipe: Option<&(Pipe, Pipe)>) -> Option<SpawnAction> {
    match stdio {
        Stdio::Inherit => None,
        Stdio::Null => Some(SpawnAction::Open {
            target_fd: STDOUT_FD,
            flags: wasmos::O_WRONLY,
            path: nul_terminated(OsStr::new("/dev/null")),
        }),
        Stdio::MakePipe => {
            let (parent_end, child_end) = pipe.expect("stdout pipe must exist");
            Some(SpawnAction::Pipe {
                child_fd: child_end.raw_fd(),
                target_fd: STDOUT_FD,
                parent_fd: parent_end.raw_fd(),
            })
        }
        Stdio::ParentStdout => Some(SpawnAction::Dup {
            src_fd: STDOUT_FD,
            target_fd: STDOUT_FD,
        }),
        Stdio::ParentStderr => Some(SpawnAction::Dup {
            src_fd: STDERR_FD,
            target_fd: STDOUT_FD,
        }),
        Stdio::Fd(fd) => Some(SpawnAction::Dup {
            src_fd: fd.as_raw_fd(),
            target_fd: STDOUT_FD,
        }),
        Stdio::InheritFile(file) => Some(SpawnAction::Dup {
            src_fd: file.raw_fd(),
            target_fd: STDOUT_FD,
        }),
        Stdio::Pipe(pipe) => Some(SpawnAction::Dup {
            src_fd: pipe.raw_fd(),
            target_fd: STDOUT_FD,
        }),
    }
}

fn stderr_action(stdio: &Stdio, pipe: Option<&(Pipe, Pipe)>) -> Option<SpawnAction> {
    match stdio {
        Stdio::Inherit => None,
        Stdio::Null => Some(SpawnAction::Open {
            target_fd: STDERR_FD,
            flags: wasmos::O_WRONLY,
            path: nul_terminated(OsStr::new("/dev/null")),
        }),
        Stdio::MakePipe => {
            let (parent_end, child_end) = pipe.expect("stderr pipe must exist");
            Some(SpawnAction::Pipe {
                child_fd: child_end.raw_fd(),
                target_fd: STDERR_FD,
                parent_fd: parent_end.raw_fd(),
            })
        }
        Stdio::ParentStdout => Some(SpawnAction::Dup {
            src_fd: STDOUT_FD,
            target_fd: STDERR_FD,
        }),
        Stdio::ParentStderr => Some(SpawnAction::Dup {
            src_fd: STDERR_FD,
            target_fd: STDERR_FD,
        }),
        Stdio::Fd(fd) => Some(SpawnAction::Dup {
            src_fd: fd.as_raw_fd(),
            target_fd: STDERR_FD,
        }),
        Stdio::InheritFile(file) => Some(SpawnAction::Dup {
            src_fd: file.raw_fd(),
            target_fd: STDERR_FD,
        }),
        Stdio::Pipe(pipe) => Some(SpawnAction::Dup {
            src_fd: pipe.raw_fd(),
            target_fd: STDERR_FD,
        }),
    }
}

enum SpawnAction {
    Pipe {
        child_fd: i32,
        target_fd: i32,
        parent_fd: i32,
    },
    Dup {
        src_fd: i32,
        target_fd: i32,
    },
    Open {
        target_fd: i32,
        flags: u32,
        path: Vec<u8>,
    },
    Chdir {
        path: Vec<u8>,
    },
}

#[derive(PartialEq, Eq, Clone, Copy, Debug, Default)]
pub struct ExitStatus(i32);

impl ExitStatus {
    fn new(raw_status: i32) -> ExitStatus {
        ExitStatus((raw_status >> 8) & 0xff)
    }

    pub fn exit_ok(&self) -> Result<(), ExitStatusError> {
        if self.0 == 0 { Ok(()) } else { Err(ExitStatusError(*self)) }
    }

    pub fn code(&self) -> Option<i32> {
        Some(self.0)
    }

    pub fn signal(&self) -> Option<i32> {
        None
    }

    pub fn core_dumped(&self) -> bool {
        false
    }

    pub fn stopped_signal(&self) -> Option<i32> {
        None
    }

    pub fn continued(&self) -> bool {
        false
    }

    pub fn into_raw(self) -> i32 {
        self.0 << 8
    }
}

impl From<i32> for ExitStatus {
    fn from(raw: i32) -> Self {
        ExitStatus::new(raw)
    }
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "exit status: {}", self.0)
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct ExitStatusError(ExitStatus);

impl Into<ExitStatus> for ExitStatusError {
    fn into(self) -> ExitStatus {
        self.0
    }
}

impl ExitStatusError {
    pub fn code(self) -> Option<NonZero<i32>> {
        NonZero::new(self.0.0)
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct ExitCode(i32);

impl ExitCode {
    pub const SUCCESS: ExitCode = ExitCode(0);
    pub const FAILURE: ExitCode = ExitCode(1);

    pub fn as_i32(&self) -> i32 {
        self.0
    }
}

impl From<u8> for ExitCode {
    fn from(code: u8) -> Self {
        Self(code as i32)
    }
}

pub struct Process {
    pid: u32,
}

impl Process {
    pub fn id(&self) -> u32 {
        self.pid
    }

    pub fn send_signal(&self, signal: i32) -> io::Result<()> {
        wasmos::kill(self.pid, signal).map_err(wasmos::io_error)
    }

    pub fn kill(&mut self) -> io::Result<()> {
        wasmos::kill(self.pid, SIGKILL).map_err(wasmos::io_error)
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        let mut raw_status = 0;
        wasmos::waitpid(self.pid as i32, &mut raw_status).map_err(wasmos::io_error)?;
        Ok(ExitStatus::new(raw_status))
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let mut raw_status = 0;
        match wasmos::waitpid_nohang(self.pid as i32, &mut raw_status) {
            Ok(0) => Ok(None),
            Ok(_) => Ok(Some(ExitStatus::new(raw_status))),
            Err(errno) => Err(wasmos::io_error(errno)),
        }
    }
}

impl Command {
    pub fn exec(&mut self, _default: Stdio) -> io::Error {
        let program = match self.resolve_program() {
            Ok(p) => p,
            Err(e) => return e,
        };

        let argv_bufs = self
            .args
            .iter()
            .map(|arg| nul_terminated(arg.as_os_str()))
            .collect::<Vec<_>>();
        let mut argv_ptrs = argv_bufs.iter().map(|arg| arg.as_ptr() as u32).collect::<Vec<_>>();
        argv_ptrs.push(0);

        let env_pairs = self.env.capture();
        let env_bufs = env_pairs
            .iter()
            .map(|(key, value)| {
                let mut entry = Vec::with_capacity(
                    key.as_encoded_bytes().len() + value.as_encoded_bytes().len() + 2,
                );
                entry.extend_from_slice(key.as_encoded_bytes());
                entry.push(b'=');
                entry.extend_from_slice(value.as_encoded_bytes());
                entry.push(0);
                entry
            })
            .collect::<Vec<_>>();
        let mut env_ptrs = env_bufs.iter().map(|entry| entry.as_ptr() as u32).collect::<Vec<_>>();
        env_ptrs.push(0);

        match wasmos::execve(program.as_os_str(), argv_ptrs.as_ptr(), env_ptrs.as_ptr()) {
            Ok(()) => unreachable!("execve returned Ok"),
            Err(errno) => wasmos::io_error(errno),
        }
    }
}

pub struct CommandArgs<'a> {
    iter: crate::slice::Iter<'a, OsString>,
}

impl<'a> Iterator for CommandArgs<'a> {
    type Item = &'a OsStr;

    fn next(&mut self) -> Option<&'a OsStr> {
        self.iter.next().map(|arg| &**arg)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a> ExactSizeIterator for CommandArgs<'a> {
    fn len(&self) -> usize {
        self.iter.len()
    }

    fn is_empty(&self) -> bool {
        self.iter.is_empty()
    }
}

impl<'a> fmt::Debug for CommandArgs<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter.clone()).finish()
    }
}

pub type ChildPipe = Pipe;

pub fn read_output(
    out: ChildPipe,
    stdout: &mut Vec<u8>,
    err: ChildPipe,
    stderr: &mut Vec<u8>,
) -> io::Result<()> {
    let mut out = Some(out);
    let mut err = Some(err);
    let mut scratch = [0u8; 8192];

    while out.is_some() || err.is_some() {
        let mut pollfds = Vec::new();
        if let Some(pipe) = out.as_ref() {
            pollfds.push(wasmos::PollFd {
                fd: pipe.raw_fd(),
                events: wasmos::POLLIN,
                revents: 0,
            });
        }
        if let Some(pipe) = err.as_ref() {
            pollfds.push(wasmos::PollFd {
                fd: pipe.raw_fd(),
                events: wasmos::POLLIN,
                revents: 0,
            });
        }

        wasmos::poll(&mut pollfds, -1).map_err(wasmos::io_error)?;

        let mut index = 0usize;
        if let Some(pipe) = out.as_ref() {
            let revents = pollfds[index].revents;
            if revents & (wasmos::POLLIN | wasmos::POLLHUP | wasmos::POLLERR) != 0 {
                let read = pipe.read(&mut scratch)?;
                if read == 0 {
                    out = None;
                } else {
                    stdout.extend_from_slice(&scratch[..read]);
                }
            }
            index += 1;
        }
        if let Some(pipe) = err.as_ref() {
            let revents = pollfds[index].revents;
            if revents & (wasmos::POLLIN | wasmos::POLLHUP | wasmos::POLLERR) != 0 {
                let read = pipe.read(&mut scratch)?;
                if read == 0 {
                    err = None;
                } else {
                    stderr.extend_from_slice(&scratch[..read]);
                }
            }
        }
    }

    Ok(())
}

pub fn getpid() -> u32 {
    wasmos::getpid()
}

pub fn getppid() -> u32 {
    wasmos::getppid()
}

impl From<ChildPipe> for Stdio {
    fn from(pipe: ChildPipe) -> Stdio {
        Stdio::Pipe(pipe)
    }
}

impl From<io::Stdout> for Stdio {
    fn from(_: io::Stdout) -> Stdio {
        Stdio::ParentStdout
    }
}

impl From<io::Stderr> for Stdio {
    fn from(_: io::Stderr) -> Stdio {
        Stdio::ParentStderr
    }
}

impl From<File> for Stdio {
    fn from(file: File) -> Stdio {
        Stdio::InheritFile(file)
    }
}

impl fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            let mut ds = f.debug_struct("Command");
            ds.field("program", &self.program).field("args", &self.args);
            if !self.env.is_unchanged() {
                ds.field("env", &self.env);
            }
            if self.cwd.is_some() {
                ds.field("cwd", &self.cwd);
            }
            if self.stdin.is_some() {
                ds.field("stdin", &self.stdin);
            }
            if self.stdout.is_some() {
                ds.field("stdout", &self.stdout);
            }
            if self.stderr.is_some() {
                ds.field("stderr", &self.stderr);
            }
            ds.finish()
        } else {
            write!(f, "{:?}", self.program)?;
            for arg in self.get_args() {
                write!(f, " {:?}", arg)?;
            }
            Ok(())
        }
    }
}

fn nul_terminated(value: &OsStr) -> Vec<u8> {
    let bytes = value.as_encoded_bytes();
    let mut out = Vec::with_capacity(bytes.len() + 1);
    out.extend_from_slice(bytes);
    out.push(0);
    out
}

fn build_action_node(action: SpawnAction, next: u32) -> Box<[u8]> {
    let has_path_payload = matches!(action, SpawnAction::Open { .. } | SpawnAction::Chdir { .. });
    let mut bytes = match action {
        SpawnAction::Pipe {
            child_fd,
            target_fd,
            parent_fd,
        } => {
            let actions = [
                PosixSpawnFdopWire {
                    next: 0,
                    _prev: 0,
                    cmd: FDOP_DUP2,
                    fd: target_fd,
                    srcfd: child_fd,
                    oflag: 0,
                    mode: 0,
                },
                PosixSpawnFdopWire {
                    next: 0,
                    _prev: 0,
                    cmd: FDOP_CLOSE,
                    fd: child_fd,
                    srcfd: 0,
                    oflag: 0,
                    mode: 0,
                },
                PosixSpawnFdopWire {
                    next,
                    _prev: 0,
                    cmd: FDOP_CLOSE,
                    fd: parent_fd,
                    srcfd: 0,
                    oflag: 0,
                    mode: 0,
                },
            ];
            let mut out = Vec::with_capacity(actions.len() * core::mem::size_of::<PosixSpawnFdopWire>());
            for wire in actions.into_iter().rev() {
                out.extend_from_slice(as_bytes(&wire));
            }
            out
        }
        SpawnAction::Dup { src_fd, target_fd } => {
            let wire = PosixSpawnFdopWire {
                next,
                _prev: 0,
                cmd: FDOP_DUP2,
                fd: target_fd,
                srcfd: src_fd,
                oflag: 0,
                mode: 0,
            };
            as_bytes(&wire).to_vec()
        }
        SpawnAction::Open {
            target_fd,
            flags,
            path,
        } => {
            let wire = PosixSpawnFdopWire {
                next,
                _prev: 0,
                cmd: FDOP_OPEN,
                fd: target_fd,
                srcfd: 0,
                oflag: flags as i32,
                mode: 0,
            };
            let mut out = Vec::with_capacity(core::mem::size_of::<PosixSpawnFdopWire>() + path.len());
            out.extend_from_slice(as_bytes(&wire));
            out.extend_from_slice(&path);
            out
        }
        SpawnAction::Chdir { path } => {
            let wire = PosixSpawnFdopWire {
                next,
                _prev: 0,
                cmd: FDOP_CHDIR,
                fd: 0,
                srcfd: 0,
                oflag: 0,
                mode: 0,
            };
            let mut out = Vec::with_capacity(core::mem::size_of::<PosixSpawnFdopWire>() + path.len());
            out.extend_from_slice(as_bytes(&wire));
            out.extend_from_slice(&path);
            out
        }
    };

    if has_path_payload {
        return bytes.into_boxed_slice();
    }

    let node_ptr = bytes.as_mut_ptr() as u32;
    let mut offset = 0usize;
    while offset + core::mem::size_of::<PosixSpawnFdopWire>() <= bytes.len() {
        let next_ptr = if offset + core::mem::size_of::<PosixSpawnFdopWire>() < bytes.len() {
            node_ptr + (offset + core::mem::size_of::<PosixSpawnFdopWire>()) as u32
        } else {
            next
        };
        bytes[offset..offset + 4].copy_from_slice(&next_ptr.to_le_bytes());
        offset += core::mem::size_of::<PosixSpawnFdopWire>();
    }

    bytes.into_boxed_slice()
}

fn as_bytes<T>(value: &T) -> &[u8] {
    unsafe { core::slice::from_raw_parts((value as *const T).cast::<u8>(), core::mem::size_of::<T>()) }
}

fn capture_path(stream: &str, capture_id: u32) -> PathBuf {
    let mut path = String::from("/tmp/wasmos-std-");
    push_decimal(&mut path, getpid());
    path.push('-');
    push_decimal(&mut path, capture_id);
    path.push('-');
    path.push_str(stream);
    path.push_str(".tmp");
    PathBuf::from(path)
}

fn next_output_capture_id() -> u32 {
    unsafe {
        OUTPUT_CAPTURE_SEQ = OUTPUT_CAPTURE_SEQ.wrapping_add(1);
        OUTPUT_CAPTURE_SEQ
    }
}

fn push_decimal(buf: &mut String, mut value: u32) {
    if value == 0 {
        buf.push('0');
        return;
    }

    let mut digits = [0u8; 10];
    let mut len = 0usize;
    while value != 0 {
        digits[len] = (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    while len != 0 {
        len -= 1;
        buf.push((b'0' + digits[len]) as char);
    }
}
