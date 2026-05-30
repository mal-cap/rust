use crate::fmt;
use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut};
use crate::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4, ToSocketAddrs};
use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, RawFd};
use crate::sys::fd::FileDesc;
use crate::sys::{AsInner, FromInner, IntoInner, unsupported, wasmos};
use crate::time::Duration;
use crate::vec;

fn ipv6_unsupported<T>() -> io::Result<T> {
    Err(io::const_error!(
        io::ErrorKind::Unsupported,
        "IPv6 is not supported on WasmOS yet",
    ))
}

fn unsupported_socket_option<T>() -> io::Result<T> {
    Err(io::const_error!(
        io::ErrorKind::Unsupported,
        "socket option not supported on WasmOS yet",
    ))
}

fn wait_socket(fd: i32, events: i16) -> io::Result<()> {
    loop {
        let mut pollfd = wasmos::PollFd {
            fd,
            events,
            revents: 0,
        };
        match wasmos::poll(crate::slice::from_mut(&mut pollfd), -1) {
            Ok(_) => return Ok(()),
            Err(errno) if errno == wasmos::EINTR => continue,
            Err(errno) => return Err(wasmos::io_error(errno)),
        }
    }
}

fn format_addr(addr: &SocketAddr) -> io::Result<String> {
    match addr {
        SocketAddr::V4(addr) => Ok(addr.to_string()),
        SocketAddr::V6(_) => ipv6_unsupported(),
    }
}

fn dns_resolve_blocking(host: &str, port: u16) -> io::Result<SocketAddr> {
    let mut scratch = [0u8; 64];
    for _ in 0..50 {
        match wasmos::dns_resolve(host, &mut scratch) {
            Ok(len) => {
                let value = crate::str::from_utf8(&scratch[..len]).map_err(|_| {
                    io::const_error!(io::ErrorKind::InvalidData, "DNS returned non-UTF-8")
                })?;
                let ip = value.parse::<Ipv4Addr>().map_err(|_| {
                    io::const_error!(io::ErrorKind::InvalidData, "DNS returned invalid IPv4")
                })?;
                return Ok(SocketAddr::new(IpAddr::V4(ip), port));
            }
            Err(errno) if errno == wasmos::EAGAIN => wasmos::sleep_ms(10),
            Err(errno) if errno == wasmos::ENOENT => {
                return Err(io::Error::new(io::ErrorKind::NotFound, "DNS resolution failed"));
            }
            Err(errno) => return Err(wasmos::io_error(errno)),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "DNS resolution timed out on WasmOS",
    ))
}

#[derive(Debug)]
pub struct Socket(FileDesc);

impl AsInner<FileDesc> for Socket {
    fn as_inner(&self) -> &FileDesc {
        &self.0
    }
}

impl IntoInner<FileDesc> for Socket {
    fn into_inner(self) -> FileDesc {
        self.0
    }
}

impl FromInner<FileDesc> for Socket {
    fn from_inner(inner: FileDesc) -> Socket {
        Socket(inner)
    }
}

impl AsFd for Socket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl AsRawFd for Socket {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl IntoRawFd for Socket {
    fn into_raw_fd(self) -> RawFd {
        self.0.into_raw_fd()
    }
}

impl FromRawFd for Socket {
    unsafe fn from_raw_fd(raw_fd: RawFd) -> Self {
        unsafe { Self(FileDesc::from_raw_fd(raw_fd)) }
    }
}

pub struct TcpStream {
    inner: Socket,
    peer: Option<SocketAddr>,
    local: Option<SocketAddr>,
}

impl TcpStream {
    pub fn connect<A: ToSocketAddrs>(addr: A) -> io::Result<TcpStream> {
        let mut last_err = io::Error::new(io::ErrorKind::InvalidInput, "no addresses to connect to");
        for target in addr.to_socket_addrs()? {
            let target_str = match format_addr(&target) {
                Ok(value) => value,
                Err(err) => {
                    last_err = err;
                    continue;
                }
            };
            let fd = wasmos::socket(wasmos::AF_INET, wasmos::SOCK_STREAM, 0)
                .map_err(wasmos::io_error)?;
            match wasmos::connect(fd, &target_str) {
                Ok(()) => {
                    return Ok(TcpStream {
                        inner: unsafe { Socket::from_raw_fd(fd) },
                        peer: Some(target),
                        local: None,
                    });
                }
                Err(errno) => {
                    let _ = wasmos::close(fd);
                    last_err = wasmos::io_error(errno);
                }
            }
        }
        Err(last_err)
    }

    pub fn connect_timeout(addr: &SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
        if timeout.is_zero() {
            return Err(io::const_error!(
                io::ErrorKind::InvalidInput,
                "cannot set a 0 duration timeout",
            ));
        }
        TcpStream::connect(*addr)
    }

    pub fn set_read_timeout(&self, _: Option<Duration>) -> io::Result<()> {
        unsupported_socket_option()
    }

    pub fn set_write_timeout(&self, _: Option<Duration>) -> io::Result<()> {
        unsupported_socket_option()
    }

    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        unsupported_socket_option()
    }

    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        unsupported_socket_option()
    }

    pub fn peek(&self, _: &mut [u8]) -> io::Result<usize> {
        unsupported()
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            match wasmos::recv(self.inner.as_raw_fd(), buf, 0) {
                Ok(len) => return Ok(len),
                Err(errno) if errno == wasmos::EAGAIN => {
                    wait_socket(self.inner.as_raw_fd(), wasmos::POLLIN)?
                }
                Err(errno) if errno == wasmos::EINTR => continue,
                Err(errno) => return Err(wasmos::io_error(errno)),
            }
        }
    }

    pub fn read_buf(&self, cursor: BorrowedCursor<'_>) -> io::Result<()> {
        crate::io::default_read_buf(|buf| self.read(buf), cursor)
    }

    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        crate::io::default_read_vectored(|buf| self.read(buf), bufs)
    }

    pub fn is_read_vectored(&self) -> bool {
        false
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        loop {
            match wasmos::send(self.inner.as_raw_fd(), buf, 0) {
                Ok(len) => return Ok(len),
                Err(errno) if errno == wasmos::EAGAIN => {
                    wait_socket(self.inner.as_raw_fd(), wasmos::POLLOUT)?
                }
                Err(errno) if errno == wasmos::EINTR => continue,
                Err(errno) => return Err(wasmos::io_error(errno)),
            }
        }
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        crate::io::default_write_vectored(|buf| self.write(buf), bufs)
    }

    pub fn is_write_vectored(&self) -> bool {
        false
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.peer.ok_or_else(|| {
            io::const_error!(io::ErrorKind::Unsupported, "peer address not available on WasmOS")
        })
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        self.local.ok_or_else(|| {
            io::const_error!(io::ErrorKind::Unsupported, "local address not available on WasmOS")
        })
    }

    pub fn shutdown(&self, _: Shutdown) -> io::Result<()> {
        unsupported()
    }

    pub fn duplicate(&self) -> io::Result<TcpStream> {
        Ok(TcpStream {
            inner: Socket::from_inner(self.inner.as_inner().duplicate()?),
            peer: self.peer,
            local: self.local,
        })
    }

    pub fn set_linger(&self, _: Option<Duration>) -> io::Result<()> {
        unsupported_socket_option()
    }

    pub fn linger(&self) -> io::Result<Option<Duration>> {
        unsupported_socket_option()
    }

    pub fn set_nodelay(&self, _: bool) -> io::Result<()> {
        unsupported_socket_option()
    }

    pub fn nodelay(&self) -> io::Result<bool> {
        unsupported_socket_option()
    }

    pub fn set_ttl(&self, _: u32) -> io::Result<()> {
        unsupported_socket_option()
    }

    pub fn ttl(&self) -> io::Result<u32> {
        unsupported_socket_option()
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        unsupported_socket_option()
    }

    pub fn set_nonblocking(&self, _: bool) -> io::Result<()> {
        unsupported_socket_option()
    }

    pub fn socket(&self) -> &Socket {
        &self.inner
    }

    pub fn into_socket(self) -> Socket {
        self.inner
    }
}

impl FromInner<Socket> for TcpStream {
    fn from_inner(socket: Socket) -> TcpStream {
        TcpStream {
            inner: socket,
            peer: None,
            local: None,
        }
    }
}

impl IntoInner<Socket> for TcpStream {
    fn into_inner(self) -> Socket {
        self.inner
    }
}

impl fmt::Debug for TcpStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TcpStream")
            .field("fd", &self.inner.as_raw_fd())
            .field("peer", &self.peer)
            .field("local", &self.local)
            .finish()
    }
}

pub struct TcpListener {
    inner: Socket,
    local: SocketAddr,
}

impl TcpListener {
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<TcpListener> {
        let mut last_err = io::Error::new(io::ErrorKind::InvalidInput, "no addresses to bind to");
        for target in addr.to_socket_addrs()? {
            let target_str = match format_addr(&target) {
                Ok(value) => value,
                Err(err) => {
                    last_err = err;
                    continue;
                }
            };
            let fd = wasmos::socket(wasmos::AF_INET, wasmos::SOCK_STREAM, 0)
                .map_err(wasmos::io_error)?;
            match wasmos::bind(fd, &target_str) {
                Ok(()) => {
                    if let Err(errno) = wasmos::listen(fd, 128) {
                        let _ = wasmos::close(fd);
                        last_err = wasmos::io_error(errno);
                        continue;
                    }
                    return Ok(TcpListener {
                        inner: unsafe { Socket::from_raw_fd(fd) },
                        local: target,
                    });
                }
                Err(errno) => {
                    let _ = wasmos::close(fd);
                    last_err = wasmos::io_error(errno);
                }
            }
        }
        Err(last_err)
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local)
    }

    pub fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let fd = loop {
            match wasmos::accept(self.inner.as_raw_fd()) {
                Ok(fd) => break fd,
                Err(errno) if errno == wasmos::EAGAIN => {
                    wait_socket(self.inner.as_raw_fd(), wasmos::POLLIN)?
                }
                Err(errno) if errno == wasmos::EINTR => continue,
                Err(errno) => return Err(wasmos::io_error(errno)),
            }
        };
        let peer = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));
        Ok((
            TcpStream {
                inner: unsafe { Socket::from_raw_fd(fd) },
                peer: None,
                local: Some(self.local),
            },
            peer,
        ))
    }

    pub fn duplicate(&self) -> io::Result<TcpListener> {
        Ok(TcpListener {
            inner: Socket::from_inner(self.inner.as_inner().duplicate()?),
            local: self.local,
        })
    }

    pub fn set_ttl(&self, _: u32) -> io::Result<()> {
        unsupported_socket_option()
    }

    pub fn ttl(&self) -> io::Result<u32> {
        unsupported_socket_option()
    }

    pub fn set_only_v6(&self, _: bool) -> io::Result<()> {
        ipv6_unsupported()
    }

    pub fn only_v6(&self) -> io::Result<bool> {
        ipv6_unsupported()
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        unsupported_socket_option()
    }

    pub fn set_nonblocking(&self, _: bool) -> io::Result<()> {
        unsupported_socket_option()
    }

    pub fn socket(&self) -> &Socket {
        &self.inner
    }

    pub fn into_socket(self) -> Socket {
        self.inner
    }
}

impl FromInner<Socket> for TcpListener {
    fn from_inner(socket: Socket) -> TcpListener {
        TcpListener {
            inner: socket,
            local: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)),
        }
    }
}

impl IntoInner<Socket> for TcpListener {
    fn into_inner(self) -> Socket {
        self.inner
    }
}

impl fmt::Debug for TcpListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TcpListener")
            .field("fd", &self.inner.as_raw_fd())
            .field("local", &self.local)
            .finish()
    }
}

pub struct UdpSocket {
    inner: Socket,
}

impl UdpSocket {
    pub fn bind<A: ToSocketAddrs>(_: A) -> io::Result<UdpSocket> {
        unsupported()
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        unsupported()
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        unsupported()
    }

    pub fn recv_from(&self, _: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        unsupported()
    }

    pub fn peek_from(&self, _: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        unsupported()
    }

    pub fn send_to(&self, _: &[u8], _: &SocketAddr) -> io::Result<usize> {
        unsupported()
    }

    pub fn duplicate(&self) -> io::Result<UdpSocket> {
        Ok(UdpSocket {
            inner: Socket::from_inner(self.inner.as_inner().duplicate()?),
        })
    }

    pub fn set_read_timeout(&self, _: Option<Duration>) -> io::Result<()> {
        unsupported()
    }

    pub fn set_write_timeout(&self, _: Option<Duration>) -> io::Result<()> {
        unsupported()
    }

    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        unsupported()
    }

    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        unsupported()
    }

    pub fn set_broadcast(&self, _: bool) -> io::Result<()> {
        unsupported()
    }

    pub fn broadcast(&self) -> io::Result<bool> {
        unsupported()
    }

    pub fn set_multicast_loop_v4(&self, _: bool) -> io::Result<()> {
        unsupported()
    }

    pub fn multicast_loop_v4(&self) -> io::Result<bool> {
        unsupported()
    }

    pub fn set_multicast_ttl_v4(&self, _: u32) -> io::Result<()> {
        unsupported()
    }

    pub fn multicast_ttl_v4(&self) -> io::Result<u32> {
        unsupported()
    }

    pub fn set_multicast_loop_v6(&self, _: bool) -> io::Result<()> {
        unsupported()
    }

    pub fn multicast_loop_v6(&self) -> io::Result<bool> {
        unsupported()
    }

    pub fn join_multicast_v4(&self, _: &Ipv4Addr, _: &Ipv4Addr) -> io::Result<()> {
        unsupported()
    }

    pub fn join_multicast_v6(&self, _: &crate::net::Ipv6Addr, _: u32) -> io::Result<()> {
        unsupported()
    }

    pub fn leave_multicast_v4(&self, _: &Ipv4Addr, _: &Ipv4Addr) -> io::Result<()> {
        unsupported()
    }

    pub fn leave_multicast_v6(&self, _: &crate::net::Ipv6Addr, _: u32) -> io::Result<()> {
        unsupported()
    }

    pub fn set_ttl(&self, _: u32) -> io::Result<()> {
        unsupported()
    }

    pub fn ttl(&self) -> io::Result<u32> {
        unsupported()
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        unsupported()
    }

    pub fn set_nonblocking(&self, _: bool) -> io::Result<()> {
        unsupported()
    }

    pub fn recv(&self, _: &mut [u8]) -> io::Result<usize> {
        unsupported()
    }

    pub fn peek(&self, _: &mut [u8]) -> io::Result<usize> {
        unsupported()
    }

    pub fn send(&self, _: &[u8]) -> io::Result<usize> {
        unsupported()
    }

    pub fn connect<A: ToSocketAddrs>(&self, _: A) -> io::Result<()> {
        unsupported()
    }

    pub fn socket(&self) -> &Socket {
        &self.inner
    }

    pub fn into_socket(self) -> Socket {
        self.inner
    }
}

impl FromInner<Socket> for UdpSocket {
    fn from_inner(socket: Socket) -> UdpSocket {
        UdpSocket { inner: socket }
    }
}

impl IntoInner<Socket> for UdpSocket {
    fn into_inner(self) -> Socket {
        self.inner
    }
}

impl fmt::Debug for UdpSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UdpSocket").field("fd", &self.inner.as_raw_fd()).finish()
    }
}

pub struct LookupHost {
    entries: vec::IntoIter<SocketAddr>,
}

impl Iterator for LookupHost {
    type Item = SocketAddr;

    fn next(&mut self) -> Option<SocketAddr> {
        self.entries.next()
    }
}

pub fn lookup_host(host: &str, port: u16) -> io::Result<LookupHost> {
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        return Ok(LookupHost {
            entries: vec![SocketAddr::new(IpAddr::V4(ip), port)].into_iter(),
        });
    }
    let addr = dns_resolve_blocking(host, port)?;
    Ok(LookupHost {
        entries: vec![addr].into_iter(),
    })
}
