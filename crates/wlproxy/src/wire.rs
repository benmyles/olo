//! The Wayland wire format, and a non-blocking Unix socket that carries it
//! with file descriptors alongside.

use std::collections::VecDeque;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

use wayland_backend::protocol::{AllowNull, ArgumentType};

/// Object ids from here up are made by the server.
pub const SERVER_START: u32 = 0xff00_0000;
/// libwayland's limit on descriptors in one sendmsg.
const MAX_FDS: usize = 28;
/// A message is at most this long (the header's size field).
const MAX_MSG: usize = 4096;

#[derive(Debug)]
pub enum Arg {
    Int(i32),
    Uint(u32),
    Fixed(i32),
    /// Without its terminating NUL; `None` is a null string.
    Str(Option<Vec<u8>>),
    Object(u32),
    NewId(u32),
    Array(Vec<u8>),
    Fd(OwnedFd),
}

impl Arg {
    fn dup(&self) -> io::Result<Arg> {
        Ok(match self {
            Arg::Int(v) => Arg::Int(*v),
            Arg::Uint(v) => Arg::Uint(*v),
            Arg::Fixed(v) => Arg::Fixed(*v),
            Arg::Str(v) => Arg::Str(v.clone()),
            Arg::Object(v) => Arg::Object(*v),
            Arg::NewId(v) => Arg::NewId(*v),
            Arg::Array(v) => Arg::Array(v.clone()),
            Arg::Fd(fd) => Arg::Fd(fd.try_clone()?),
        })
    }
}

#[derive(Debug)]
pub struct Msg {
    pub id: u32,
    pub op: u16,
    pub args: Vec<Arg>,
}

impl Msg {
    pub fn new(id: u32, op: u16, args: Vec<Arg>) -> Msg {
        Msg { id, op, args }
    }

    /// A copy, its descriptors duplicated.
    pub fn dup(&self) -> io::Result<Msg> {
        Ok(Msg { id: self.id, op: self.op, args: self.args.iter().map(Arg::dup).collect::<io::Result<_>>()? })
    }

    pub fn uint(&self, i: usize) -> u32 {
        match self.args.get(i) {
            Some(Arg::Uint(v) | Arg::Object(v) | Arg::NewId(v)) => *v,
            Some(Arg::Int(v) | Arg::Fixed(v)) => *v as u32,
            _ => 0,
        }
    }

    pub fn int(&self, i: usize) -> i32 {
        self.uint(i) as i32
    }

    pub fn str(&self, i: usize) -> &[u8] {
        match self.args.get(i) {
            Some(Arg::Str(Some(s))) => s,
            _ => &[],
        }
    }

    /// The id this message creates, if any.
    pub fn new_id(&self) -> Option<u32> {
        self.args.iter().find_map(|a| match a {
            Arg::NewId(id) => Some(*id),
            _ => None,
        })
    }

    fn encoded_len(&self) -> usize {
        8 + self
            .args
            .iter()
            .map(|a| match a {
                Arg::Str(Some(s)) => 4 + pad(s.len() + 1),
                Arg::Array(a) => 4 + pad(a.len()),
                Arg::Fd(_) => 0,
                _ => 4,
            })
            .sum::<usize>()
    }

    /// Append the message to `out` and its descriptors to `fds`.
    fn encode(self, out: &mut Vec<u8>, fds: &mut Vec<OwnedFd>) {
        let len = self.encoded_len();
        out.extend_from_slice(&self.id.to_ne_bytes());
        out.extend_from_slice(&(((len as u32) << 16) | self.op as u32).to_ne_bytes());
        for a in self.args {
            match a {
                Arg::Int(v) | Arg::Fixed(v) => out.extend_from_slice(&v.to_ne_bytes()),
                Arg::Uint(v) | Arg::Object(v) | Arg::NewId(v) => out.extend_from_slice(&v.to_ne_bytes()),
                Arg::Str(None) => out.extend_from_slice(&0u32.to_ne_bytes()),
                Arg::Str(Some(s)) => {
                    out.extend_from_slice(&((s.len() + 1) as u32).to_ne_bytes());
                    out.extend_from_slice(&s);
                    out.resize(out.len() + pad(s.len() + 1) - s.len(), 0);
                }
                Arg::Array(a) => {
                    out.extend_from_slice(&(a.len() as u32).to_ne_bytes());
                    out.extend_from_slice(&a);
                    out.resize(out.len() + pad(a.len()) - a.len(), 0);
                }
                Arg::Fd(fd) => fds.push(fd),
            }
        }
    }
}

fn pad(n: usize) -> usize {
    (n + 3) & !3
}

/// The header at the front of `buf`, if a whole message is there:
/// (object, opcode, length).
pub fn header(buf: &[u8]) -> Result<Option<(u32, u16, usize)>, &'static str> {
    if buf.len() < 8 {
        return Ok(None);
    }
    let id = u32::from_ne_bytes(buf[0..4].try_into().unwrap());
    let word = u32::from_ne_bytes(buf[4..8].try_into().unwrap());
    let len = (word >> 16) as usize;
    if len < 8 || !len.is_multiple_of(4) {
        return Err("bad message length");
    }
    Ok((buf.len() >= len).then_some((id, (word & 0xffff) as u16, len)))
}

/// Decode a message's arguments (`body` is after the header).
pub fn decode(id: u32, op: u16, mut body: &[u8], sig: &[ArgumentType], fds: &mut VecDeque<OwnedFd>) -> Result<Msg, &'static str> {
    let mut args = Vec::with_capacity(sig.len());
    let word = |body: &mut &[u8]| -> Result<u32, &'static str> {
        if body.len() < 4 {
            return Err("message too short");
        }
        let v = u32::from_ne_bytes(body[..4].try_into().unwrap());
        *body = &body[4..];
        Ok(v)
    };
    for t in sig {
        args.push(match t {
            ArgumentType::Int => Arg::Int(word(&mut body)? as i32),
            ArgumentType::Uint => Arg::Uint(word(&mut body)?),
            ArgumentType::Fixed => Arg::Fixed(word(&mut body)? as i32),
            ArgumentType::Object(_) => Arg::Object(word(&mut body)?),
            ArgumentType::NewId => Arg::NewId(word(&mut body)?),
            ArgumentType::Str(null) => {
                let len = word(&mut body)? as usize;
                if len == 0 {
                    if *null == AllowNull::No {
                        return Err("null string");
                    }
                    Arg::Str(None)
                } else {
                    if body.len() < pad(len) {
                        return Err("message too short");
                    }
                    let s = body[..len - 1].to_vec();
                    body = &body[pad(len)..];
                    Arg::Str(Some(s))
                }
            }
            ArgumentType::Array => {
                let len = word(&mut body)? as usize;
                if body.len() < pad(len) {
                    return Err("message too short");
                }
                let a = body[..len].to_vec();
                body = &body[pad(len)..];
                Arg::Array(a)
            }
            ArgumentType::Fd => Arg::Fd(fds.pop_front().ok_or("missing file descriptor")?),
        });
    }
    if !body.is_empty() {
        return Err("message too long");
    }
    Ok(Msg { id, op, args })
}

/// One end of a Wayland connection.
pub struct Conn {
    pub sock: UnixStream,
    pub inbuf: Vec<u8>,
    pub infds: VecDeque<OwnedFd>,
    out: VecDeque<u8>,
    /// Descriptors to send, each with where its message starts (counted
    /// in bytes ever queued).
    outfds: VecDeque<(usize, OwnedFd)>,
    queued: usize,
    sent: usize,
}

impl Conn {
    pub fn new(sock: UnixStream) -> io::Result<Conn> {
        sock.set_nonblocking(true)?;
        Ok(Conn { sock, inbuf: Vec::new(), infds: VecDeque::new(), out: VecDeque::new(), outfds: VecDeque::new(), queued: 0, sent: 0 })
    }

    pub fn fd(&self) -> RawFd {
        self.sock.as_raw_fd()
    }

    /// Bytes waiting to go out.
    pub fn backlog(&self) -> usize {
        self.out.len()
    }

    pub fn send(&mut self, msg: Msg) {
        let mut bytes = Vec::with_capacity(msg.encoded_len());
        let mut fds = Vec::new();
        msg.encode(&mut bytes, &mut fds);
        for fd in fds {
            self.outfds.push_back((self.queued, fd));
        }
        self.queued += bytes.len();
        self.out.extend(bytes);
    }

    /// Raw bytes (the hello before the Wayland stream).
    pub fn send_raw(&mut self, bytes: &[u8]) {
        self.queued += bytes.len();
        self.out.extend(bytes);
    }

    /// Write what the socket takes now. Each descriptor goes out with or
    /// before the bytes of its message, never after.
    pub fn flush(&mut self) -> io::Result<()> {
        while !self.out.is_empty() {
            let nfds = self.outfds.len().min(MAX_FDS);
            // (A message with more descriptors than one sendmsg takes
            // can't be split; send it whole.)
            let limit = match self.outfds.get(nfds) {
                Some((at, _)) if *at > self.sent => at - self.sent,
                _ => self.out.len(),
            };
            let (a, b) = self.out.as_slices();
            let first = &a[..a.len().min(limit)];
            let second = &b[..b.len().min(limit - first.len())];
            let raw: Vec<RawFd> = self.outfds.iter().take(nfds).map(|(_, fd)| fd.as_raw_fd()).collect();
            let n = sendmsg(self.fd(), &[first, second], &raw)?;
            let Some(n) = n else { break };
            self.outfds.drain(..nfds);
            self.out.drain(..n);
            self.sent += n;
        }
        Ok(())
    }

    /// Read what has arrived; false at the end of the stream.
    pub fn fill(&mut self) -> io::Result<bool> {
        loop {
            let mut buf = [0u8; 16 * 1024];
            match recvmsg(self.fd(), &mut buf, &mut self.infds)? {
                Some(0) => return Ok(false),
                Some(n) => self.inbuf.extend_from_slice(&buf[..n]),
                None => return Ok(true),
            }
            if self.infds.len() > 1024 || self.inbuf.len() > 1 << 20 {
                return Err(io::Error::other("peer sends faster than we can read"));
            }
        }
    }

    /// Read exactly `n` raw bytes, if they've all come (the hello's reply).
    pub fn take_raw(&mut self, n: usize) -> Option<Vec<u8>> {
        (self.inbuf.len() >= n).then(|| self.inbuf.drain(..n).collect())
    }

    /// The next whole message's header, if one is there.
    pub fn peek(&self) -> Result<Option<(u32, u16, usize)>, &'static str> {
        let h = header(&self.inbuf)?;
        if let Some((_, _, len)) = h
            && len > MAX_MSG
        {
            return Err("message too long");
        }
        Ok(h)
    }
}

/// sendmsg with descriptors; `None` when the socket is full.
fn sendmsg(fd: RawFd, bufs: &[&[u8]], fds: &[RawFd]) -> io::Result<Option<usize>> {
    let iov: Vec<libc::iovec> = bufs.iter().filter(|b| !b.is_empty()).map(|b| libc::iovec { iov_base: b.as_ptr() as *mut _, iov_len: b.len() }).collect();
    let space = if fds.is_empty() { 0 } else { (unsafe { libc::CMSG_SPACE(std::mem::size_of_val(fds) as u32) }) as usize };
    let mut cmsg = vec![0u64; space.div_ceil(8)];
    // SAFETY: msghdr over our buffers, which outlive the call.
    unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = iov.as_ptr() as *mut _;
        msg.msg_iovlen = iov.len() as _;
        if !fds.is_empty() {
            msg.msg_control = cmsg.as_mut_ptr().cast();
            msg.msg_controllen = space as _;
            let c = libc::CMSG_FIRSTHDR(&msg);
            (*c).cmsg_level = libc::SOL_SOCKET;
            (*c).cmsg_type = libc::SCM_RIGHTS;
            (*c).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(fds) as u32) as _;
            std::ptr::copy_nonoverlapping(fds.as_ptr(), libc::CMSG_DATA(c).cast::<RawFd>(), fds.len());
        }
        loop {
            let n = libc::sendmsg(fd, &msg, libc::MSG_NOSIGNAL | libc::MSG_DONTWAIT);
            if n >= 0 {
                return Ok(Some(n as usize));
            }
            let e = io::Error::last_os_error();
            match e.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => return Ok(None),
                _ => return Err(e),
            }
        }
    }
}

/// recvmsg, collecting descriptors; `None` when nothing is waiting.
fn recvmsg(fd: RawFd, buf: &mut [u8], fds: &mut VecDeque<OwnedFd>) -> io::Result<Option<usize>> {
    let mut cmsg = [0u64; 256];
    // SAFETY: msghdr over our buffers; the descriptors received are ours.
    unsafe {
        let mut iov = libc::iovec { iov_base: buf.as_mut_ptr().cast(), iov_len: buf.len() };
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cmsg.as_mut_ptr().cast();
        msg.msg_controllen = std::mem::size_of_val(&cmsg) as _;
        let n = loop {
            let n = libc::recvmsg(fd, &mut msg, libc::MSG_CMSG_CLOEXEC | libc::MSG_DONTWAIT);
            if n >= 0 {
                break n as usize;
            }
            let e = io::Error::last_os_error();
            match e.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => return Ok(None),
                _ => return Err(e),
            }
        };
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(c).cast::<RawFd>();
                let count = ((*c).cmsg_len as usize - (data as usize - c as usize)) / std::mem::size_of::<RawFd>();
                for i in 0..count {
                    fds.push_back(OwnedFd::from_raw_fd(std::ptr::read_unaligned(data.add(i))));
                }
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
        if msg.msg_flags & libc::MSG_CTRUNC != 0 {
            return Err(io::Error::other("descriptors lost (control data truncated)"));
        }
        Ok(Some(n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let (a, b) = UnixStream::pair().unwrap();
        let mut a = Conn::new(a).unwrap();
        let mut b = Conn::new(b).unwrap();
        let file = std::fs::File::open("/dev/null").unwrap();
        let sig = [ArgumentType::Uint, ArgumentType::Str(AllowNull::Yes), ArgumentType::Fd, ArgumentType::Array, ArgumentType::Str(AllowNull::Yes), ArgumentType::NewId];
        for i in 0..40 {
            a.send(Msg::new(
                3,
                1,
                vec![Arg::Uint(i), Arg::Str(Some(b"hello".to_vec())), Arg::Fd(file.try_clone().unwrap().into()), Arg::Array(vec![1, 2, 3]), Arg::Str(None), Arg::NewId(7)],
            ));
        }
        a.flush().unwrap();
        assert_eq!(a.backlog(), 0);
        assert!(b.fill().unwrap());
        for i in 0..40 {
            let (id, op, len) = b.peek().unwrap().unwrap();
            let body: Vec<u8> = b.inbuf[8..len].to_vec();
            b.inbuf.drain(..len);
            let m = decode(id, op, &body, &sig, &mut b.infds).unwrap();
            assert_eq!((m.id, m.op, m.uint(0), m.str(1), m.new_id()), (3, 1, i, &b"hello"[..], Some(7)));
            assert!(matches!(&m.args[3], Arg::Array(a) if a == &[1, 2, 3]));
            assert!(matches!(&m.args[4], Arg::Str(None)));
        }
        assert!(b.infds.is_empty());
        assert!(b.inbuf.is_empty());
    }
}
