//! How Terminal and olo-termd talk: frames over a Unix socket, each a
//! header (length, kind, shell id) and a body: JSON for requests and
//! replies, raw bytes for a shell's input and output. The PTY of a shell
//! Terminal attaches to comes with the reply (SCM_RIGHTS), so Terminal can
//! see what's running in it.

use std::{
    collections::VecDeque,
    io::{self, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::net::UnixStream,
    },
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// Lines of scrollback kept per shell.
pub const SCROLLBACK: usize = 10_000;

// Terminal → olo-termd.
/// `Hello`: who's asking, and the environment for its shells.
pub const HELLO: u8 = 1;
/// The shells there are: answered with SHELLS.
pub const LIST: u8 = 2;
/// `Spawn` a shell with this (new) id: answered with ATTACHED or EXITED.
pub const SPAWN: u8 = 3;
/// Attach to a shell at a `Grid` size: answered with ATTACHED or EXITED.
pub const ATTACH: u8 = 4;
/// Bytes for the shell.
pub const INPUT: u8 = 5;
/// A new `Grid`.
pub const RESIZE: u8 = 6;
/// Forget the scrollback.
pub const CLEAR: u8 = 7;
/// Hang up the shell (its window was closed).
pub const CLOSE: u8 = 8;
/// Terminal is quitting: leave the shells be, and don't start it again.
pub const BYE: u8 = 9;
/// How olo-termd is: answered with REPORT.
pub const STATUS: u8 = 10;
/// Re-execute in place, keeping the shells.
pub const RESTART: u8 = 11;
/// Take over a shell (`Adopt`) with this (new) id, its PTY attached: one
/// Terminal ran itself before olo-termd. Answered like SPAWN.
pub const ADOPT: u8 = 12;

// olo-termd → Terminal.
/// `Vec<ShellInfo>`.
pub const SHELLS: u8 = 64;
/// The shell's pid (i32, little-endian), then its screen and scrollback
/// as output to replay; carries the PTY.
pub const ATTACHED: u8 = 65;
/// What the shell wrote.
pub const OUTPUT: u8 = 66;
/// The shell is gone: `Option<i32>`, its exit status.
pub const EXITED: u8 = 67;
/// Another Terminal attached to the shell.
pub const DETACHED: u8 = 68;
/// `Status`.
pub const REPORT: u8 = 69;
/// The session is ending: the shells are about to be hung up, each kept
/// to be restored (`Spawn::restore`) next time.
pub const ENDING: u8 = 70;

/// The agent screen this Terminal (or olo-termd) is that screen's own
/// on: started there, not the person's Terminal moved there for a while
/// (`olo_kit_base::moving`), whose shells stay with theirs.
pub fn screen() -> Option<String> {
    std::env::var("OLO_SCREEN").ok().filter(|s| !s.is_empty()).filter(|_| olo_kit_base::moving::home().is_none())
}

/// Where Terminal and olo-termd keep what lasts between sessions (the
/// windows, the shells): the person's state folder, or a screen's own,
/// beside its history and Chrome's profile and gone with it. A Terminal
/// started on an agent screen shares the person's home, and once opened
/// their windows' folders there and wrote its own windows over theirs.
pub fn state_dir() -> PathBuf {
    let state = olo_xdg::state_dir();
    match screen() {
        Some(name) => state.join("screens").join(name).join("terminal"),
        None => state.join("terminal"),
    }
}

/// Where olo-termd keeps shells between sessions (only for us:
/// scrollback can hold anything).
pub fn kept_dir() -> PathBuf {
    state_dir()
}

/// Shell `id`'s lines, kept (zlib).
pub fn kept_path(id: u64) -> PathBuf {
    kept_dir().join(format!("shell-{id:016x}.z"))
}

/// Length (u32, of what follows it), kind (u8; bit 7 set when the frame
/// carries a file descriptor), shell id (u64).
const HEADER: usize = 13;
const CARRIES_FD: u8 = 0x80;
/// The longest frame accepted.
const MAX: usize = 256 << 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub pid: i32,
    pub env: Vec<(String, String)>,
}

/// A shell's size: characters, and a character cell in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grid {
    pub cols: u16,
    pub rows: u16,
    pub cell_w: u16,
    pub cell_h: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spawn {
    /// A program to run instead of the login shell.
    pub command: Option<Vec<String>>,
    pub cwd: Option<PathBuf>,
    pub grid: Grid,
    /// In place of this shell from an earlier session: what it showed
    /// (kept when that session ended) comes first.
    #[serde(default)]
    pub restore: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Adopt {
    pub pid: i32,
    pub grid: Grid,
    pub title: Option<String>,
    /// Its screen so far, as output to replay.
    pub replay: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellInfo {
    pub id: u64,
    pub pid: i32,
    /// The Terminal attached to it, if any (its pid).
    pub client: Option<i32>,
    pub grid: Grid,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub pid: i32,
    /// When this olo-termd started (seconds since the epoch).
    pub started: u64,
    /// Terminals connected.
    pub clients: usize,
    pub shells: Vec<ShellInfo>,
}

#[derive(Debug)]
pub struct Frame {
    pub kind: u8,
    pub id: u64,
    pub data: Vec<u8>,
    pub fd: Option<OwnedFd>,
}

impl Frame {
    pub fn new(kind: u8, id: u64, data: Vec<u8>) -> Frame {
        Frame { kind, id, data, fd: None }
    }

    pub fn json(kind: u8, id: u64, value: &impl Serialize) -> Frame {
        Frame::new(kind, id, serde_json::to_vec(value).unwrap_or_default())
    }

    pub fn parse<T: for<'a> Deserialize<'a>>(&self) -> Option<T> {
        serde_json::from_slice(&self.data).ok()
    }
}

pub fn runtime_dir() -> PathBuf {
    olo_xdg::runtime_dir()
}

/// The display whose olo-termd has the shells: this one, or, for the
/// person's Terminal moved to an agent screen (`olo_kit_base::moving`),
/// theirs, where its shells stay.
pub fn display() -> String {
    let home = olo_kit_base::moving::home().and_then(|h| h.set.into_iter().find(|(k, _)| k == "WAYLAND_DISPLAY").map(|(_, v)| v));
    home.or_else(|| std::env::var("WAYLAND_DISPLAY").ok()).filter(|d| !d.is_empty()).unwrap_or_else(|| "wayland-0".into())
}

/// Where olo-termd listens (one per display).
pub fn socket_path() -> PathBuf {
    olo_xdg::legacy::runtime_file(&runtime_dir(), &format!("olo-termd-{}.sock", display()))
}

/// Held by the running olo-termd.
pub fn lock_path() -> PathBuf {
    olo_xdg::legacy::runtime_file(&runtime_dir(), &format!("olo-termd-{}.lock", display()))
}

/// The desktop session's pid file, if this display has one.
pub fn session_pid_path() -> PathBuf {
    olo_xdg::legacy::runtime_file(&runtime_dir(), &format!("olo-session-{}.pid", display()))
}

pub fn wayland_socket() -> PathBuf {
    let d = display();
    if Path::new(&d).is_absolute() { PathBuf::from(d) } else { runtime_dir().join(d) }
}

/// Send one frame (and the descriptor it carries).
pub fn write(s: &UnixStream, f: &Frame) -> io::Result<()> {
    let mut head = [0u8; HEADER];
    head[0..4].copy_from_slice(&((HEADER - 4 + f.data.len()) as u32).to_le_bytes());
    head[4] = f.kind | if f.fd.is_some() { CARRIES_FD } else { 0 };
    head[5..13].copy_from_slice(&f.id.to_le_bytes());
    let mut s = s;
    match &f.fd {
        Some(fd) => {
            let sent = send_fd(s, &head, fd.as_raw_fd())?;
            s.write_all(&head[sent..])?;
        }
        None => s.write_all(&head)?,
    }
    s.write_all(&f.data)
}

/// Send `bytes` with `fd` attached; how much of `bytes` went.
fn send_fd(s: &UnixStream, bytes: &[u8], fd: RawFd) -> io::Result<usize> {
    // SAFETY: a msghdr over our buffers, with room for one SCM_RIGHTS.
    unsafe {
        let space = libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) as usize;
        let mut control = vec![0u8; space];
        let mut iov = libc::iovec { iov_base: bytes.as_ptr() as *mut _, iov_len: bytes.len() };
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = space as _;
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;
        std::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<RawFd>(), fd);
        loop {
            let n = libc::sendmsg(s.as_raw_fd(), &msg, libc::MSG_NOSIGNAL);
            if n >= 0 {
                return Ok(n as usize);
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }
}

/// Reads frames, collecting the descriptors that come with them.
pub struct Reader {
    s: UnixStream,
    buf: Vec<u8>,
    fds: VecDeque<OwnedFd>,
    chunk: Vec<u8>,
}

impl Reader {
    pub fn new(s: UnixStream) -> Reader {
        Reader { s, buf: Vec::new(), fds: VecDeque::new(), chunk: vec![0; 256 * 1024] }
    }

    /// The next frame; None when the other end has gone.
    pub fn next(&mut self) -> io::Result<Option<Frame>> {
        loop {
            if self.buf.len() >= HEADER {
                let len = u32::from_le_bytes(self.buf[0..4].try_into().unwrap()) as usize;
                if !(HEADER - 4..=MAX).contains(&len) {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "bad frame length"));
                }
                if self.buf.len() >= 4 + len {
                    let kind = self.buf[4];
                    let id = u64::from_le_bytes(self.buf[5..13].try_into().unwrap());
                    let data = self.buf[HEADER..4 + len].to_vec();
                    self.buf.drain(..4 + len);
                    let fd = if kind & CARRIES_FD != 0 { self.fds.pop_front() } else { None };
                    return Ok(Some(Frame { kind: kind & !CARRIES_FD, id, data, fd }));
                }
            }
            let n = self.receive()?;
            if n == 0 {
                return Ok(None);
            }
            self.buf.extend_from_slice(&self.chunk[..n]);
        }
    }

    fn receive(&mut self) -> io::Result<usize> {
        // SAFETY: recvmsg into our buffers; descriptors received are ours.
        unsafe {
            let space = libc::CMSG_SPACE((8 * std::mem::size_of::<RawFd>()) as u32) as usize;
            let mut control = vec![0u8; space];
            let mut iov = libc::iovec { iov_base: self.chunk.as_mut_ptr().cast(), iov_len: self.chunk.len() };
            let mut msg: libc::msghdr = std::mem::zeroed();
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = space as _;
            let n = loop {
                let n = libc::recvmsg(self.s.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC);
                if n >= 0 {
                    break n as usize;
                }
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::Interrupted {
                    return Err(e);
                }
            };
            let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
            while !cmsg.is_null() {
                if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                    let data = libc::CMSG_DATA(cmsg).cast::<RawFd>();
                    let count = ((*cmsg).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / std::mem::size_of::<RawFd>();
                    for i in 0..count {
                        self.fds.push_back(OwnedFd::from_raw_fd(std::ptr::read_unaligned(data.add(i))));
                    }
                }
                cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
            }
            Ok(n)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_and_descriptors_arrive_in_order() {
        let (a, b) = UnixStream::pair().unwrap();
        let file = std::fs::File::open("/proc/self/exe").unwrap();
        let writer = std::thread::spawn(move || {
            write(&a, &Frame::new(OUTPUT, 7, b"hello".to_vec())).unwrap();
            let mut f = Frame::new(ATTACHED, 9, vec![1; 300_000]);
            f.fd = Some(file.into());
            write(&a, &f).unwrap();
            write(&a, &Frame::json(EXITED, 9, &Some(3))).unwrap();
        });
        let mut r = Reader::new(b);
        let f = r.next().unwrap().unwrap();
        assert_eq!((f.kind, f.id, f.data.as_slice(), f.fd.is_none()), (OUTPUT, 7, &b"hello"[..], true));
        let f = r.next().unwrap().unwrap();
        assert_eq!((f.kind, f.id, f.data.len()), (ATTACHED, 9, 300_000));
        let fd = f.fd.expect("descriptor");
        let link = std::fs::read_link(format!("/proc/self/fd/{}", fd.as_raw_fd())).unwrap();
        assert_eq!(link, std::fs::read_link("/proc/self/exe").unwrap());
        let f = r.next().unwrap().unwrap();
        assert_eq!(f.parse::<Option<i32>>(), Some(Some(3)));
        writer.join().unwrap();
        assert!(r.next().unwrap().is_none());
    }
}
