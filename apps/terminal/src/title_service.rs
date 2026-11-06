//! The coding agents' tab title tool as a service of the session
//! (`olo-terminal --mcp-service`, one of olo-session's `REAL_SHELL`): an
//! MCP server over loopback HTTP (MCP's streamable HTTP, answered with
//! plain JSON) at [`url`], which Terminal names in the agents' settings
//! (agent_title.rs). Claude Code starts a stdio server once and never
//! again once it has gone (its tool lost for the rest of the session);
//! an HTTP one it reconnects to by itself, as Codex does.
//!
//! It keeps no sessions (no `Mcp-Session-Id`): every request stands
//! alone, so whichever copy of the service answers it, before an upgrade
//! or after, answers it the same. Each response closes its connection,
//! so none is left open to be cut. Upgraded (olo-session's SIGURG), it
//! stops taking connections, lets the requests under way finish (a few
//! milliseconds; a slow sender is held to `READ_LIMIT`), and becomes the
//! new version in place with the listening socket handed over
//! (`LISTENER_ENV`): connections that come meanwhile wait in the
//! kernel's queue, so nobody is refused. Should it stop for any other
//! reason, olo-session starts it again, and the agents reconnect.
//!
//! Only this user's programs may set a title: for the tool, the process
//! at the other end of the connection is found in the kernel's table of
//! TCP sockets (its user, and which process holds it); and a browser's
//! request (an `Origin`, or a `Host` that isn't loopback) is refused.
//! That process (the agent) and its ancestors find its tab, as the stdio
//! server's own ancestors did.

use olo_sync::Mutex;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    os::fd::{AsRawFd, FromRawFd, RawFd},
    path::Path,
    sync::{Arc, Condvar},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{agent_context, agent_title};

/// What the service is run with.
pub const FLAG: &str = "--mcp-service";
/// Its loopback port (Token Pool has 17891 and 17892).
pub const PORT: u16 = 17893;
/// Another port, for a test.
const PORT_ENV: &str = "OLO_TERMINAL_MCP_PORT";
/// The listening socket's descriptor, handed to the new version.
const LISTENER_ENV: &str = "OLO_TERMINAL_MCP_LISTENER";
const PATH: &str = "/mcp";
/// The longest a request may take to come (head and body).
const READ_LIMIT: Duration = Duration::from_secs(10);
/// The most a request's head and body may be.
const MOST_HEAD: usize = 16 * 1024;
const MOST_BODY: usize = 1024 * 1024;

pub fn port() -> u16 {
    std::env::var(PORT_ENV).ok().and_then(|p| p.parse().ok()).unwrap_or(PORT)
}

/// Where the agents reach it.
pub fn url() -> String {
    format!("http://127.0.0.1:{}{PATH}", port())
}

/// The listening socket: the one the version before handed over, else a
/// new one.
fn listen(port: u16) -> std::io::Result<TcpListener> {
    if let Some(fd) = std::env::var(LISTENER_ENV).ok().and_then(|v| v.parse::<RawFd>().ok()) {
        // SAFETY: read at start, before any other thread looks at the
        // environment; not handed on to what we start.
        unsafe { std::env::remove_var(LISTENER_ENV) };
        // SAFETY: the descriptor was handed to us (left open across the
        // exec) and is taken once; checked below to be our socket.
        let l = unsafe { TcpListener::from_raw_fd(fd) };
        // SAFETY: fcntl on a descriptor we own: our children mustn't hold it.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        if l.local_addr().is_ok_and(|a| a.port() == port) {
            return Ok(l);
        }
    }
    TcpListener::bind(("127.0.0.1", port))
}

/// Requests under way, and their end told.
#[derive(Default)]
struct UnderWay {
    count: Mutex<usize>,
    done: Condvar,
}

impl UnderWay {
    fn begin(&self) {
        *self.count.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    }

    fn end(&self) {
        let mut n = self.count.lock().unwrap_or_else(|e| e.into_inner());
        *n -= 1;
        if *n == 0 {
            self.done.notify_all();
        }
    }

    /// Waits for none to be under way (at most `limit`).
    fn wait_idle(&self, limit: Duration) {
        let deadline = Instant::now() + limit;
        let mut n = self.count.lock().unwrap_or_else(|e| e.into_inner());
        while *n > 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            n = self.done.wait_timeout(n, left).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

/// `olo-terminal --mcp-service`: serves till it's ended, becoming each
/// new version as it's installed.
pub fn serve() -> std::process::ExitCode {
    let port = port();
    let listener = match listen(port) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("olo-terminal: the tab title service can't listen on 127.0.0.1:{port}: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    if let Err(e) = listener.set_nonblocking(true) {
        eprintln!("olo-terminal: {e}");
        return std::process::ExitCode::FAILURE;
    }
    let upgrade = olo_kit_base::restore::upgrade_signal();
    let under_way = Arc::new(UnderWay::default());
    loop {
        let mut fds =
            [libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: upgrade.as_ref().map_or(-1, |u| u.as_raw_fd()), events: libc::POLLIN, revents: 0 }];
        // SAFETY: two pollfds of ours; asleep till a connection or an upgrade.
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            eprintln!("olo-terminal: the tab title service: {}", std::io::Error::last_os_error());
            return std::process::ExitCode::FAILURE;
        }
        if fds[1].revents & libc::POLLIN != 0
            && let Some(u) = upgrade.as_ref()
        {
            let mut b = [0u8; 16];
            // SAFETY: reading the upgrade signal's pipe into a buffer of ours.
            unsafe { libc::read(u.as_raw_fd(), b.as_mut_ptr().cast(), b.len()) };
            // What's under way finishes; new connections wait in the queue.
            under_way.wait_idle(READ_LIMIT + Duration::from_secs(1));
            let fd = listener.as_raw_fd();
            // SAFETY: fcntl on our own socket: left open across the exec.
            unsafe { libc::fcntl(fd, libc::F_SETFD, 0) };
            let e = olo_kit_base::restore::exec_new_version_with(&[(LISTENER_ENV, fd.to_string())]);
            // SAFETY: as above; it couldn't, so this version carries on.
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
            eprintln!("olo-terminal: the tab title service couldn't become the new version: {e}");
            continue;
        }
        if fds[0].revents & libc::POLLIN == 0 {
            continue;
        }
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    under_way.begin();
                    let mine = under_way.clone();
                    let spawned = std::thread::Builder::new().name("request".into()).spawn(move || {
                        let _ = answer_connection(stream);
                        mine.end();
                    });
                    // (No thread: the connection closes unanswered, and the
                    // agent tries again.)
                    if spawned.is_err() {
                        under_way.end();
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                // (A connection gone before it was taken, or no descriptors
                // left for a moment: the next one is tried.)
                Err(_) => break,
            }
        }
    }
}

/// A request as read.
#[derive(Debug, PartialEq)]
struct Request {
    method: String,
    path: String,
    /// Names in lower case.
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

/// An answer: its status and JSON body, if any.
#[derive(Debug, PartialEq)]
struct Response {
    status: u16,
    body: Option<Value>,
}

impl Response {
    fn empty(status: u16) -> Response {
        Response { status, body: None }
    }

    fn error(status: u16, message: &str) -> Response {
        Response { status, body: Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32600, "message": message}})) }
    }
}

/// Read one request from `r`: its head (up to a blank line), then its
/// body (`Content-Length`, or chunked).
fn read_request(r: &mut impl Read) -> Result<Request, Response> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MOST_HEAD {
            return Err(Response::error(431, "the request's head is too big"));
        }
        let n = r.read(&mut chunk).map_err(|_| Response::error(408, "the request didn't come"))?;
        if n == 0 {
            return Err(Response::error(400, "the request ended early"));
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = std::str::from_utf8(&buf[..head_end]).map_err(|_| Response::error(400, "the request's head isn't text"))?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split(' ');
    let (method, path) = (first.next().unwrap_or_default().to_owned(), first.next().unwrap_or_default().to_owned());
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_owned())).collect();
    let mut rest = buf[head_end + 4..].to_vec();
    let mut req = Request { method, path, headers, body: Vec::new() };
    let mut more = |rest: &mut Vec<u8>| -> Result<(), Response> {
        let n = r.read(&mut chunk).map_err(|_| Response::error(408, "the request didn't come"))?;
        if n == 0 {
            return Err(Response::error(400, "the request ended early"));
        }
        rest.extend_from_slice(&chunk[..n]);
        Ok(())
    };
    if req.header("transfer-encoding").is_some_and(|t| t.eq_ignore_ascii_case("chunked")) {
        loop {
            let line_end = loop {
                if let Some(i) = rest.windows(2).position(|w| w == b"\r\n") {
                    break i;
                }
                more(&mut rest)?;
            };
            let size = std::str::from_utf8(&rest[..line_end])
                .ok()
                .and_then(|l| usize::from_str_radix(l.split(';').next().unwrap_or_default().trim(), 16).ok())
                .ok_or_else(|| Response::error(400, "a bad chunk"))?;
            rest.drain(..line_end + 2);
            if size == 0 {
                break;
            }
            if req.body.len() + size > MOST_BODY {
                return Err(Response::error(413, "the request is too big"));
            }
            while rest.len() < size + 2 {
                more(&mut rest)?;
            }
            req.body.extend_from_slice(&rest[..size]);
            rest.drain(..size + 2);
        }
    } else {
        let length: usize = req.header("content-length").map_or(Ok(0), str::parse).map_err(|_| Response::error(400, "a bad Content-Length"))?;
        if length > MOST_BODY {
            return Err(Response::error(413, "the request is too big"));
        }
        while rest.len() < length {
            more(&mut rest)?;
        }
        rest.truncate(length);
        req.body = rest;
    }
    Ok(req)
}

/// Whether `host` (a `Host` or an `Origin`'s host and port) is loopback:
/// an agent's, not a page's that a name was pointed here for.
fn loopback(host: &str) -> bool {
    let host = host.rsplit_once(':').filter(|(_, p)| p.bytes().all(|b| b.is_ascii_digit())).map_or(host, |(h, _)| h);
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

/// The answer to `req`, `set` doing the tool's work.
fn respond(req: &Request, set: &mut agent_title::Set) -> Response {
    // A browser says where the page is from; an agent doesn't.
    if req.header("origin").is_some_and(|o| !loopback(o.trim_start_matches("http://"))) {
        return Response::error(403, "not for web pages");
    }
    if !req.header("host").is_some_and(loopback) {
        return Response::error(403, "only on loopback");
    }
    if req.path.split('?').next() != Some(PATH) {
        return Response::error(404, "not here");
    }
    match req.method.as_str() {
        "POST" => {}
        // No stream from the server, and no sessions to end.
        "GET" | "DELETE" => return Response::empty(405),
        _ => return Response::empty(405),
    }
    let Ok(msg) = serde_json::from_slice::<Value>(&req.body) else {
        return Response { status: 400, body: Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}})) };
    };
    // One message, or (the protocol before 2025-06-18) a batch of them.
    let replies: Vec<Value> = match &msg {
        Value::Array(all) => all.iter().filter_map(|m| agent_title::answer(m, set)).collect(),
        one => agent_title::answer(one, set).into_iter().collect(),
    };
    match (msg.is_array(), replies.len()) {
        // Notifications and replies only: taken.
        (_, 0) => Response::empty(202),
        (false, _) => Response { status: 200, body: replies.into_iter().next() },
        (true, _) => Response { status: 200, body: Some(Value::Array(replies)) },
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        _ => "Error",
    }
}

fn write_response(w: &mut impl Write, r: &Response) -> std::io::Result<()> {
    let body = r.body.as_ref().map(|b| b.to_string()).unwrap_or_default();
    let mut head = format!("HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n", r.status, reason(r.status), body.len());
    if r.body.is_some() {
        head.push_str("Content-Type: application/json\r\n");
    }
    if r.status == 405 {
        head.push_str("Allow: POST\r\n");
    }
    head.push_str("\r\n");
    // (In one write: a second small one would wait on the first's
    // acknowledgement.)
    head.push_str(&body);
    w.write_all(head.as_bytes())?;
    w.flush()
}

/// One connection: its request answered, and closed.
fn answer_connection(mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(READ_LIMIT))?;
    stream.set_write_timeout(Some(READ_LIMIT))?;
    let response = match read_request(&mut stream) {
        Err(r) => r,
        Ok(req) => {
            // Who asked, only for the tool itself (what the server is and
            // has is no secret): this user's program, and which.
            let mut set = |title: &str, cwd: Option<&Path>| {
                // SAFETY: getuid can't fail.
                let uid = unsafe { libc::getuid() };
                let inode = peer(&stream).filter(|&(u, _)| u == uid).map(|(_, inode)| inode).ok_or("Only this user's programs may set a tab's title.")?;
                let pid = holder(inode, uid);
                let lineage = pid.map(agent_context::lineage).unwrap_or_default();
                let own = pid.and_then(|p| std::fs::read_link(format!("/proc/{p}/cwd")).ok());
                agent_title::set_for(&lineage, title, cwd, own.as_deref())
            };
            respond(&req, &mut set)
        }
    };
    write_response(&mut stream, &response)
}

/// The kernel's table's way of writing an IPv4 address and port.
fn table_addr(a: SocketAddr) -> Option<String> {
    let SocketAddr::V4(a) = a else { return None };
    Some(format!("{:08X}:{:04X}", u32::from_le_bytes(a.ip().octets()), a.port()))
}

/// The user at the other end of `stream` (a loopback connection to us)
/// and its socket: the one in the kernel's table whose own end is our
/// peer's and whose other end is ours (its inode, to find who holds it).
fn peer(stream: &TcpStream) -> Option<(u32, u64)> {
    let (theirs, ours) = (table_addr(stream.peer_addr().ok()?)?, table_addr(stream.local_addr().ok()?)?);
    let table = std::fs::read_to_string("/proc/net/tcp").ok()?;
    let (uid, inode) = table.lines().skip(1).find_map(|line| {
        let f: Vec<&str> = line.split_whitespace().collect();
        (f.len() > 9 && f[1] == theirs && f[2] == ours).then(|| Some((f[7].parse::<u32>().ok()?, f[9].parse::<u64>().ok()?))).flatten()
    })?;
    Some((uid, inode))
}

/// What agents' processes are called: looked in first (one has asked,
/// nearly always), before every other process of the user's.
const AGENTS: [&str; 5] = ["claude", "codex", "node", "bun", "code"];

/// The process (of user `uid`) holding socket `inode`.
fn holder(inode: u64, uid: u32) -> Option<i32> {
    let want = format!("socket:[{inode}]");
    let holds = |pid: i32| std::fs::read_dir(format!("/proc/{pid}/fd")).is_ok_and(|fds| fds.flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|l| l.as_os_str() == want.as_str())));
    let mut others = Vec::new();
    for e in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
        if std::fs::metadata(e.path()).map(|m| std::os::unix::fs::MetadataExt::uid(&m)).ok() != Some(uid) {
            continue;
        }
        let comm = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default();
        if AGENTS.contains(&comm.trim()) {
            if holds(pid) {
                return Some(pid);
            }
        } else {
            others.push(pid);
        }
    }
    others.into_iter().find(|&pid| holds(pid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn request(text: &str) -> Request {
        read_request(&mut text.as_bytes()).unwrap()
    }

    fn post(body: &str) -> Request {
        request(&format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:17893\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()))
    }

    fn nothing(_: &str, _: Option<&Path>) -> Result<String, String> {
        Ok("done".into())
    }

    #[test]
    fn reads_a_request_by_its_length_or_its_chunks() {
        let r = post(r#"{"a":1}"#);
        assert_eq!((r.method.as_str(), r.path.as_str(), r.body.as_slice()), ("POST", "/mcp", br#"{"a":1}"#.as_slice()));
        assert_eq!(r.header("content-type"), Some("application/json"));
        let chunked = request("POST /mcp HTTP/1.1\r\nHost: localhost:17893\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n{\"a\r\n4;x=y\r\n\":1}\r\n0\r\n\r\n");
        assert_eq!(chunked.body, br#"{"a":1}"#);
        // Cut short, or too big: refused.
        assert_eq!(read_request(&mut "POST /mcp HTTP/1.1\r\nContent-Length: 10\r\n\r\n{}".as_bytes()).unwrap_err().status, 400);
        assert_eq!(read_request(&mut format!("POST /mcp HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MOST_BODY + 1).as_bytes()).unwrap_err().status, 413);
    }

    #[test]
    fn answers_as_an_mcp_server_with_no_sessions() {
        let mut set = nothing;
        let init = respond(&post(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#), &mut set);
        assert_eq!(init.status, 200);
        assert_eq!(init.body.as_ref().unwrap()["result"]["serverInfo"]["name"], "olo-terminal");
        // A notification is taken, with nothing to say.
        assert_eq!(respond(&post(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#), &mut set), Response::empty(202));
        let list = respond(&post(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#), &mut set);
        assert_eq!(list.body.unwrap()["result"]["tools"][0]["name"], "set_tab_title");
        // A batch, the older protocol's.
        let batch = respond(&post(r#"[{"jsonrpc":"2.0","id":3,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/x"}]"#), &mut set);
        assert_eq!(batch.body.unwrap().as_array().map(Vec::len), Some(1));
        // No stream from the server, no session to end.
        let mut get = post("");
        get.method = "GET".into();
        assert_eq!(respond(&get, &mut set).status, 405);
        assert_eq!(respond(&post("not json"), &mut set).status, 400);
    }

    #[test]
    fn calls_the_tool() {
        let mut got = Vec::new();
        let mut set = |t: &str, cwd: Option<&Path>| -> Result<String, String> {
            got.push((t.to_owned(), cwd.map(Path::to_path_buf)));
            Ok("titled".into())
        };
        let r = respond(&post(r#"{"jsonrpc":"2.0","id":"a","method":"tools/call","params":{"name":"set_tab_title","arguments":{"title":"Fix login","cwd":"/src"}}}"#), &mut set);
        assert_eq!(r.body.unwrap()["result"]["content"][0]["text"], "titled");
        assert_eq!(got, [("Fix login".to_owned(), Some(PathBuf::from("/src")))]);
    }

    #[test]
    fn refuses_web_pages_and_other_hosts() {
        let mut set = nothing;
        let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let page = request(&format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:17893\r\nOrigin: https://example.com\r\nContent-Length: {}\r\n\r\n{ping}", ping.len()));
        assert_eq!(respond(&page, &mut set).status, 403);
        // A name pointed at loopback (DNS rebinding): its Host gives it away.
        let rebound = request(&format!("POST /mcp HTTP/1.1\r\nHost: evil.example:17893\r\nContent-Length: {}\r\n\r\n{ping}", ping.len()));
        assert_eq!(respond(&rebound, &mut set).status, 403);
        let local_page = request(&format!("POST /mcp HTTP/1.1\r\nHost: localhost:17893\r\nOrigin: http://localhost:17893\r\nContent-Length: {}\r\n\r\n{ping}", ping.len()));
        assert_eq!(respond(&local_page, &mut set).status, 200);
        let mut elsewhere = post(ping);
        elsewhere.path = "/other".into();
        assert_eq!(respond(&elsewhere, &mut set).status, 404);
    }

    /// The process at the other end of a loopback connection is found:
    /// this test's own, which connected.
    #[test]
    fn finds_who_connected() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let _c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let (s, _) = l.accept().unwrap();
        let started = Instant::now();
        let (uid, inode) = peer(&s).expect("in the table");
        let table = started.elapsed();
        // SAFETY: getuid can't fail.
        assert_eq!(uid, unsafe { libc::getuid() });
        assert_eq!(holder(inode, uid), Some(std::process::id() as i32));
        eprintln!("the peer's user in {table:?}, its process in {:?} (not an agent: every process looked at)", started.elapsed() - table);
    }

    /// Over a real connection: an answer, and the connection closed.
    #[test]
    fn answers_over_a_connection() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let server = std::thread::spawn(move || answer_connection(l.accept().unwrap().0));
        let mut c = TcpStream::connect(addr).unwrap();
        let body = r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#;
        write!(c, "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Length: {}\r\n\r\n{body}", addr.port(), body.len()).unwrap();
        let mut got = String::new();
        c.read_to_string(&mut got).unwrap();
        server.join().unwrap().unwrap();
        assert!(got.starts_with("HTTP/1.1 200 OK\r\n") && got.contains("Connection: close\r\n"), "{got}");
        let body: Value = serde_json::from_str(got.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body, json!({"jsonrpc": "2.0", "id": 7, "result": {}}));
    }
}
