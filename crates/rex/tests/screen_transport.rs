//! Real `olo-rex-link` QUIC screen stream tests (`QuicLink::open_screen`):
//! - `follow` first record, `--protocol` parity / validator contract, and
//!   disallowed-argument refusal before a single UDP packet or stream is sent;
//! - dropping an open `follow` or live `stream NAME` (against a pure-Rust
//!   stand-in `olo-session` compositor) ends the helper's child process on the
//!   host within 1 second (waited on via Linux `pidfd_open` + `poll`, zero
//!   sleeps and never `pgrep -f`);
//! - plain-stream (terminal) round-trip latency with and without a full-rate
//!   `stream NAME` running beside it on the same `QuicLink` (10 samples each,
//!   both directly on loopback and over a bandwidth-shaped UDP relay that
//!   exercises QUIC stream priority `-1` vs `0`).

use std::fs;
use std::io::{BufRead as _, BufReader as StdBufReader, Write as _};
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use olo_rex_client::quic::{QuicLink, TestTempDir, announced, find_test_helper, hex, make_test_quic_cert, new_token};
use olo_rex_link::screen_wire::{
    ClientMsg, FollowEvent, MAX_JSON_LINE, MSG_FRAME, PROTO, ServerMsg, decode_server_msg, encode_client_msg, max_server_payload_for, read_follow_event, validate_quic_screen_args,
};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UdpSocket;

fn find_helper() -> Option<PathBuf> {
    find_test_helper(&std::env::current_exe().unwrap())
}

struct ScratchDir(TestTempDir);

impl ScratchDir {
    fn new(tag: &str) -> Self {
        // Exclusive `mkdtemp(3)` under `/var/tmp` with a short prefix so
        // AF_UNIX socket paths stay well under Linux's 108-byte `sun_path` limit
        // and concurrent runs can never collide (Test Finding 11).
        let dir = TestTempDir::new(&format!("olo-st-{tag}"));
        for sub in ["home", "run/olo-screens", "state", "home/.config", "desktop"] {
            fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        Self(dir)
    }

    fn path(&self) -> &Path {
        self.0.path()
    }
}

/// Spawn `olo-rex-link quic -- cat` isolated in `scratch`.
struct QuicHelper {
    child: tokio::process::Child,
    input: Option<tokio::process::ChildStdin>,
    to: SocketAddr,
    print: String,
    token: [u8; olo_rex_link::quic_wire::TOKEN],
}

impl QuicHelper {
    async fn start(bin: &Path, scratch: &Path) -> Self {
        let token = new_token().unwrap();
        let mut child = tokio::process::Command::new(bin)
            .args(["quic", "--", "cat"])
            .env("HOME", scratch.join("home"))
            .env("XDG_RUNTIME_DIR", scratch.join("run"))
            .env("XDG_STATE_HOME", scratch.join("state"))
            .env("XDG_CONFIG_HOME", scratch.join("home/.config"))
            .env("OLO_HOST_DESKTOP", scratch.join("desktop"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        input.write_all(format!("{}\n", hex(&token)).as_bytes()).await.unwrap();
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(10), BufReader::new(child.stdout.take().unwrap()).read_line(&mut line)).await.expect("helper announce timeout").unwrap();
        let (port, print) = announced(&line).unwrap_or_else(|| panic!("unexpected announce: {line:?}"));
        Self { child, input: Some(input), to: (Ipv4Addr::LOCALHOST, port).into(), print, token }
    }

    fn pid(&self) -> i32 {
        self.child.id().expect("helper running") as i32
    }
}

/// Direct children of `parent_pid` whose `/proc/<pid>/cmdline` contains all
/// `needles` (each matched as a NUL-separated argv element). Never scans the
/// system or uses `pgrep -f`.
fn direct_child_with_args(parent_pid: i32, needles: &[&str]) -> Option<i32> {
    let raw = fs::read_to_string(format!("/proc/{parent_pid}/task/{parent_pid}/children")).ok()?;
    for tok in raw.split_whitespace() {
        let Ok(pid) = tok.parse::<i32>() else { continue };
        let Ok(cmdline) = fs::read(format!("/proc/{pid}/cmdline")) else { continue };
        let argv: Vec<&[u8]> = cmdline.split(|&b| b == 0).filter(|s| !s.is_empty()).collect();
        if needles.iter().all(|n| argv.contains(&n.as_bytes())) {
            return Some(pid);
        }
    }
    None
}

/// Open a Linux `pidfd` for `pid` (RAII closed on drop).
struct PidFd(i32);

impl PidFd {
    fn open(pid: i32) -> Option<Self> {
        // SAFETY: `pidfd_open(pid, 0)` returns a new file descriptor or -1.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0 as libc::c_uint) as i32 };
        (fd >= 0).then_some(Self(fd))
    }

    /// Block in `poll(2)` until the pinned process exits or absolute `deadline`
    /// expires (never restarting a relative budget after worker scheduling).
    fn wait_exit_until(&self, deadline: Instant) -> Option<Instant> {
        let mut pfd = libc::pollfd { fd: self.0, events: libc::POLLIN, revents: 0 };
        loop {
            let rem = deadline.checked_duration_since(Instant::now())?;
            let ms = rem.as_millis().min(i32::MAX as u128).max(1) as i32;
            // SAFETY: valid owned pidfd and pollfd struct.
            let r = unsafe { libc::poll(&mut pfd, 1, ms) };
            if r > 0 && (pfd.revents & libc::POLLIN) != 0 {
                return Some(Instant::now());
            }
            if r == 0 {
                return None;
            }
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                return None;
            }
        }
    }
}

impl Drop for PidFd {
    fn drop(&mut self) {
        // SAFETY: exclusively owned fd.
        unsafe {
            libc::close(self.0);
        }
    }
}

/// Wait on `pidfd` on a blocking thread bounded by the absolute `deadline`
/// both inside the worker and around the join (Test Finding 7).
async fn wait_pidfd_exit(pidfd: PidFd, deadline: Instant) -> Option<Instant> {
    let tok_deadline = tokio::time::Instant::from_std(deadline);
    tokio::time::timeout_at(tok_deadline, tokio::task::spawn_blocking(move || pidfd.wait_exit_until(deadline))).await.ok()?.ok().flatten()
}

/// Pure-Rust stand-in `olo-session` compositor process invoked by [`FakeScreen`]
/// with `arg0("olo-session")` and `OLO_SCREEN=<name>`, so `SO_PEERCRED` on the
/// compositor socket belongs to the pinned session PID.
#[test]
#[ignore]
fn fake_screen_session() {
    let sock = PathBuf::from(std::env::var("OLO_FAKE_SOCK").expect("OLO_FAKE_SOCK"));
    let size = std::env::var("OLO_FAKE_SIZE").unwrap_or_else(|_| "640x360".to_owned());
    let (w_s, h_s) = size.split_once('x').unwrap();
    let (w, h): (u32, u32) = (w_s.parse().unwrap(), h_s.parse().unwrap());
    let _ = fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).expect("fake screen bind");

    let seq = Arc::new(AtomicU64::new(1));
    let subscribers: Arc<std::sync::Mutex<Vec<UnixStream>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));

    let (seq_a, subs_a, stop_a) = (seq.clone(), subscribers.clone(), stop.clone());
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            if stop_a.load(Ordering::Relaxed) {
                break;
            }
            let (seq_c, subs_c) = (seq_a.clone(), subs_a.clone());
            std::thread::spawn(move || {
                let Ok(mut write_s) = conn.try_clone() else { return };
                let mut reader = StdBufReader::new(conn);
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    let Ok(req) = serde_json::from_str::<serde_json::Value>(line.trim()) else { break };
                    match req["type"].as_str() {
                        Some("subscribe_to") => {
                            if write_s.write_all(b"{\"type\":\"ok\"}\n").is_err() {
                                break;
                            }
                            if let Ok(sub) = write_s.try_clone() {
                                subs_c.lock().unwrap().push(sub);
                            }
                        }
                        Some("screenshot") => {
                            if let Some(path) = req["path"].as_str() {
                                let s = seq_c.load(Ordering::Relaxed);
                                let pixels = (w as usize) * (h as usize);
                                let mut raw = Vec::with_capacity(8 + pixels * 4);
                                raw.extend_from_slice(&w.to_le_bytes());
                                raw.extend_from_slice(&h.to_le_bytes());
                                // Structured-plus-changing RGBA pattern so zstd frames are
                                // realistic (~100–250 KiB per full-frame delta, matching a
                                // scrolling browser page) without taking seconds on a
                                // bandwidth-shaped link.
                                let mut state = s.wrapping_mul(0x9e3779b97f4a7c15).wrapping_add(1);
                                for y in 0..h {
                                    let band = (y + s as u32 * 13) & 31;
                                    for x in 0..w {
                                        let p = if band < 12 {
                                            state ^= state << 13;
                                            state ^= state >> 7;
                                            state ^= state << 17;
                                            (state as u32) | 0xff00_0000
                                        } else {
                                            0xff00_0000 | (((x.wrapping_add(s as u32)) & 0xff) << 16) | (((y.wrapping_add(s as u32 * 3)) & 0xff) << 8) | ((band * 7) & 0xff)
                                        };
                                        raw.extend_from_slice(&p.to_le_bytes());
                                    }
                                }
                                let tmp = format!("{path}.part");
                                if fs::write(&tmp, &raw).is_ok() && fs::rename(&tmp, path).is_ok() {
                                    let _ = write_s.write_all(b"{\"type\":\"ok\"}\n");
                                } else {
                                    let _ = write_s.write_all(b"{\"type\":\"error\",\"message\":\"write failed\"}\n");
                                }
                            }
                        }
                        _ => {
                            let _ = write_s.write_all(b"{\"type\":\"ok\"}\n");
                        }
                    }
                    line.clear();
                }
            });
        }
    });

    let mut out = std::io::stdout();
    writeln!(out, "ready").unwrap();
    out.flush().unwrap();

    let send_paint = |seq: &Arc<AtomicU64>, subscribers: &Arc<std::sync::Mutex<Vec<UnixStream>>>| {
        seq.fetch_add(1, Ordering::Relaxed);
        let mut subs = subscribers.lock().unwrap();
        subs.retain_mut(|s| s.write_all(b"{\"type\":\"event\",\"event\":{\"type\":\"painted\",\"output\":\"HEADLESS-1\"}}\n").is_ok());
    };

    let flooding = Arc::new(AtomicBool::new(false));
    let stdin = std::io::stdin();
    let mut reader = StdBufReader::new(stdin.lock());
    let mut cmd = String::new();
    while reader.read_line(&mut cmd).unwrap_or(0) > 0 {
        let t = cmd.trim();
        if t == "paint" {
            send_paint(&seq, &subscribers);
        } else if let Some(ms_s) = t.strip_prefix("flood ") {
            if let Ok(ms) = ms_s.parse::<u64>() {
                flooding.store(true, Ordering::SeqCst);
                let (fl, sq, sb, st) = (flooding.clone(), seq.clone(), subscribers.clone(), stop.clone());
                std::thread::spawn(move || {
                    while fl.load(Ordering::Relaxed) && !st.load(Ordering::Relaxed) {
                        sq.fetch_add(1, Ordering::Relaxed);
                        {
                            let mut subs = sb.lock().unwrap();
                            subs.retain_mut(|s| s.write_all(b"{\"type\":\"event\",\"event\":{\"type\":\"painted\",\"output\":\"HEADLESS-1\"}}\n").is_ok());
                        }
                        std::thread::sleep(Duration::from_millis(ms.max(5)));
                    }
                });
            }
        } else if t == "stop" {
            flooding.store(false, Ordering::SeqCst);
        }
        cmd.clear();
    }
    stop.store(true, Ordering::SeqCst);
    flooding.store(false, Ordering::SeqCst);
    let _ = fs::remove_file(&sock);
}

struct FakeScreen {
    child: Child,
    stdin: Option<std::process::ChildStdin>,
}

impl FakeScreen {
    fn start(scratch: &Path, name: &str, w: u32, h: u32) -> Self {
        let rt = scratch.join("run/olo-screens");
        fs::create_dir_all(&rt).unwrap();
        let sock = rt.join(format!("{name}.sock"));
        let exe = std::env::current_exe().unwrap();
        let mut child = Command::new(exe)
            .arg0("olo-session")
            .args(["--ignored", "--exact", "fake_screen_session", "--nocapture"])
            .env("OLO_SCREEN", name)
            .env("OLO_FAKE_SOCK", &sock)
            .env("OLO_FAKE_SIZE", format!("{w}x{h}"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let mut stdout = StdBufReader::new(child.stdout.take().unwrap());
        let mut ready = String::new();
        loop {
            ready.clear();
            let n = stdout.read_line(&mut ready).unwrap();
            assert!(n > 0, "fake_screen_session exited before announcing ready");
            if ready.trim() == "ready" {
                break;
            }
        }
        let meta = serde_json::json!({
            "name": name,
            "wayland_display": format!("olo-screen-{name}"),
            "olo_socket": sock,
            "pid": child.id() as i32,
            "width": w,
            "height": h,
            "scale": 1.0,
            "sound": false,
            "started": 1_700_000_000u64,
        });
        fs::write(rt.join(format!("{name}.json")), meta.to_string()).unwrap();
        Self { child, stdin }
    }

    fn flood(&mut self, interval_ms: u64) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "flood {interval_ms}").unwrap();
        stdin.flush().unwrap();
    }

    fn stop_flood(&mut self) {
        if let Some(stdin) = self.stdin.as_mut() {
            let _ = writeln!(stdin, "stop");
            let _ = stdin.flush();
        }
    }
}

impl Drop for FakeScreen {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Async read of one binary framed `ServerMsg` (`[tag:1][len:4 LE][payload]`),
/// validating tag & tag-specific payload bounds BEFORE allocating, reading
/// incrementally, and treating EOF as clean only before byte 0 (Test Finding 5).
async fn read_server_msg_async<R: tokio::io::AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Option<(ServerMsg, usize)>> {
    let mut tag = [0u8; 1];
    match r.read(&mut tag).await? {
        0 => return Ok(None),
        1 => {}
        _ => unreachable!(),
    }
    let mut len_bytes = [0u8; 4];
    r.read_exact(&mut len_bytes).await?;
    let len = u32::from_le_bytes(len_bytes);
    let max_len = max_server_payload_for(tag[0])?;
    if len > max_len {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("server msg 0x{:02x} length {len} exceeds {max_len}", tag[0])));
    }
    let mut payload = Vec::with_capacity((len as usize).min(64 * 1024));
    let mut remaining = len as usize;
    let mut scratch = [0u8; 16 * 1024];
    while remaining > 0 {
        let want = remaining.min(scratch.len());
        let n = r.read(&mut scratch[..want]).await?;
        if n == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "truncated server msg payload"));
        }
        payload.extend_from_slice(&scratch[..n]);
        remaining -= n;
    }
    let msg = decode_server_msg(tag[0], &payload)?;
    Ok(Some((msg, 5 + len as usize)))
}

/// Read one bounded newline-delimited follow line (`<= MAX_JSON_LINE`, Test Finding 5).
async fn read_bounded_line<R: tokio::io::AsyncBufRead + Unpin>(r: &mut R) -> std::io::Result<String> {
    let mut line = String::new();
    let mut limited = r.take((MAX_JSON_LINE + 1) as u64);
    let n = limited.read_line(&mut line).await?;
    if n > MAX_JSON_LINE {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "follow line exceeded MAX_JSON_LINE"));
    }
    Ok(line)
}

/// Transparent UDP forwarder owned by a single cancellable task (no detached
/// child tasks, Test Finding 10) that counts client→server packets and
/// optionally enforces a byte-rate cap (`bytes_per_sec`) with a bounded
/// tail-drop queue.
struct UdpRelay {
    listen: SocketAddr,
    s2c_bytes: Arc<AtomicU64>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl UdpRelay {
    async fn start(upstream: SocketAddr, bytes_per_sec: Option<f64>) -> Self {
        let client_sock = Arc::new(UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap());
        let listen = client_sock.local_addr().unwrap();
        let up_sock = Arc::new(UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap());
        up_sock.connect(upstream).await.unwrap();
        let s2c_bytes = Arc::new(AtomicU64::new(0));
        let peer: Arc<tokio::sync::Mutex<Option<SocketAddr>>> = Arc::new(tokio::sync::Mutex::new(None));

        let (cs1, us1, peer1) = (client_sock.clone(), up_sock.clone(), peer.clone());
        let (cs2, us2, down_bytes, peer2) = (client_sock, up_sock, s2c_bytes.clone(), peer);
        let task = tokio::spawn(async move {
            let up = async move {
                let mut buf = vec![0u8; 65536];
                if let Some(rate) = bytes_per_sec {
                    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(20);
                    let recv_loop = async {
                        while let Ok((n, from)) = cs1.recv_from(&mut buf).await {
                            *peer1.lock().await = Some(from);
                            let _ = tx.try_send(buf[..n].to_vec());
                        }
                    };
                    let pacer = async {
                        let mut next_send = tokio::time::Instant::now();
                        while let Some(pkt) = rx.recv().await {
                            let now = tokio::time::Instant::now();
                            next_send = next_send.max(now) + Duration::from_secs_f64(pkt.len() as f64 / rate);
                            if next_send > now {
                                tokio::time::sleep_until(next_send).await;
                            }
                            let _ = us1.send(&pkt).await;
                        }
                    };
                    tokio::join!(recv_loop, pacer);
                } else {
                    while let Ok((n, from)) = cs1.recv_from(&mut buf).await {
                        *peer1.lock().await = Some(from);
                        let _ = us1.send(&buf[..n]).await;
                    }
                }
            };
            let down = async move {
                let mut buf = vec![0u8; 65536];
                if let Some(rate) = bytes_per_sec {
                    let (tx, mut rx) = tokio::sync::mpsc::channel::<(Vec<u8>, SocketAddr)>(20);
                    let recv_loop = async {
                        while let Ok(n) = us2.recv(&mut buf).await {
                            if let Some(dst) = *peer2.lock().await {
                                let _ = tx.try_send((buf[..n].to_vec(), dst));
                            }
                        }
                    };
                    let pacer = async {
                        let mut next_send = tokio::time::Instant::now();
                        while let Some((pkt, dst)) = rx.recv().await {
                            let now = tokio::time::Instant::now();
                            next_send = next_send.max(now) + Duration::from_secs_f64(pkt.len() as f64 / rate);
                            if next_send > now {
                                tokio::time::sleep_until(next_send).await;
                            }
                            if cs2.send_to(&pkt, dst).await.is_ok() {
                                down_bytes.fetch_add(pkt.len() as u64, Ordering::Relaxed);
                            }
                        }
                    };
                    tokio::join!(recv_loop, pacer);
                } else {
                    while let Ok(n) = us2.recv(&mut buf).await {
                        if let Some(dst) = *peer2.lock().await
                            && cs2.send_to(&buf[..n], dst).await.is_ok()
                        {
                            down_bytes.fetch_add(n as u64, Ordering::Relaxed);
                        }
                    }
                }
            };
            tokio::join!(up, down);
        });
        Self { listen, s2c_bytes, task: Some(task) }
    }

    async fn stop(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for UdpRelay {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quic_open_screen_follow_protocol_and_refused_args_before_wire() {
    let Some(bin) = find_helper() else { return };
    let scratch = ScratchDir::new("follow");
    let mut helper = QuicHelper::start(&bin, scratch.path()).await;
    let relay = UdpRelay::start(helper.to, None).await;
    let stranger_relay = UdpRelay::start(helper.to, None).await;

    let link = QuicLink::connect(relay.listen, &helper.print, helper.token).await.expect("quic connect");
    link.hello().await.expect("hello");

    // 1. `open_screen(["follow"])` returns the follow stream's first Snapshot record.
    let stream = link.open_screen(&["follow".to_owned()]).await.expect("open_screen follow");
    let mut reader = BufReader::new(stream);
    let first_line = tokio::time::timeout(Duration::from_secs(5), read_bounded_line(&mut reader)).await.expect("follow snapshot timeout").unwrap();
    assert!(!first_line.is_empty(), "expected initial FollowEvent::Snapshot line");
    let event = read_follow_event(&mut first_line.as_bytes()).unwrap().expect("decoded follow event");
    assert!(matches!(event, FollowEvent::Snapshot { proto: PROTO, .. }), "got {event:?}");

    // Pin the exact `olo-rex-link screen follow` child PID under our helper and
    // verify dropping the stream ends it within one absolute 1s deadline (Test Finding 8).
    let follow_pid = direct_child_with_args(helper.pid(), &["screen", "follow"]).expect("follow child running under quic helper");
    let pidfd = PidFd::open(follow_pid).expect("pidfd_open follow child");
    let t0 = Instant::now();
    let deadline = t0 + Duration::from_secs(1);
    drop(reader);
    wait_pidfd_exit(pidfd, deadline).await.expect("follow child did not exit within 1s of QUIC stream drop");
    eprintln!("MEASURE quic drop follow child exited in {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);

    // 2. `--protocol`: if `rex-link`'s validator allows it, assert exact output
    //    parity with `olo-rex-link screen --protocol`; otherwise confirm the
    //    validator's refusal is surfaced verbatim before any byte is sent.
    let expected_proto = Command::new(&bin).args(["screen", "--protocol"]).env("OLO_HOST_DESKTOP", scratch.path().join("desktop")).output().unwrap();
    assert!(expected_proto.status.success());
    let proto_args = vec!["--protocol".to_owned()];
    match validate_quic_screen_args(&proto_args) {
        Ok(()) => {
            let mut s = link.open_screen(&proto_args).await.expect("open_screen --protocol");
            let mut out = Vec::new();
            tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await.unwrap().unwrap();
            assert_eq!(out, expected_proto.stdout, "QUIC --protocol output must match CLI output");
        }
        Err(validator_err) => {
            let err = link.open_screen(&proto_args).await.err().expect("--protocol refused by current rex-link validator");
            assert_eq!(err, validator_err.to_string());
        }
    }

    // 3. Arguments the validator refuses are rejected with its exact words
    //    BEFORE a single QUIC stream is allocated or wire frame is sent
    //    (checked via local `opened_streams()` + wire frames + `hello()` barrier —
    //    Test Finding 6).
    let wrong = new_token().unwrap();
    let stranger = QuicLink::connect(stranger_relay.listen, &helper.print, wrong).await.expect("stranger handshake");
    assert!(stranger.alive());
    link.hello().await.expect("sync before refused calls");
    let link_opened_before = link.opened_streams();
    let stranger_opened_before = stranger.opened_streams();
    let link_frames_before = link.sent_stream_frames();
    let stranger_frames_before = stranger.sent_stream_frames();

    for bad in [
        vec![],
        vec!["remove".to_owned()],
        vec!["follow".to_owned(), "extra".to_owned()],
        vec!["stream".to_owned(), "../bad".to_owned()],
        vec!["history".to_owned(), "qa".to_owned(), "--clear".to_owned()],
        vec!["export".to_owned(), "qa".to_owned(), "--out".to_owned(), "/tmp/x".to_owned()],
    ] {
        let want = validate_quic_screen_args(&bad).unwrap_err().to_string();
        assert_eq!(link.open_screen(&bad).await.err().as_deref(), Some(want.as_str()), "for {bad:?}");
        assert_eq!(stranger.open_screen(&bad).await.err().as_deref(), Some(want.as_str()), "stranger for {bad:?}");
    }
    assert_eq!(link.opened_streams(), link_opened_before, "disallowed open_screen args allocated a QUIC stream on `link`!");
    assert_eq!(stranger.opened_streams(), stranger_opened_before, "disallowed open_screen args allocated a QUIC stream on `stranger`!");
    assert_eq!(link.sent_stream_frames(), link_frames_before, "disallowed open_screen args sent a QUIC stream frame on `link`!");
    assert_eq!(stranger.sent_stream_frames(), stranger_frames_before, "disallowed open_screen args sent a QUIC stream frame on `stranger`!");
    // Synchronize via a known operation (`hello`) and re-verify `opened_streams`
    // grew by EXACTLY 1 (for `hello` alone) while `stranger` stays alive untouched!
    link.hello().await.expect("sync after refused calls");
    assert_eq!(link.opened_streams(), link_opened_before + 1, "only the barrier hello() should have allocated a stream");
    assert_eq!(stranger.opened_streams(), stranger_opened_before);
    assert_eq!(stranger.sent_stream_frames(), stranger_frames_before);
    assert!(stranger.alive(), "disallowed open_screen args touched the wrong-token link!");
    // Sanity check: a real stream on `stranger` DOES send bytes and closes the link.
    assert!(tokio::time::timeout(Duration::from_secs(5), stranger.hello()).await.unwrap().is_err());
    assert!(!stranger.alive());

    link.close_now();
    stranger.close_now();
    drop(link);
    drop(stranger);
    drop(helper.input.take());
    // Let the relay deliver CONNECTION_CLOSE before stopping it so the helper
    // exits immediately instead of waiting for QUIC idle timeout.
    let status = tokio::time::timeout(Duration::from_secs(10), helper.child.wait()).await.unwrap().unwrap();
    assert!(status.success());
    relay.stop().await;
    stranger_relay.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quic_drop_open_screen_stream_ends_host_process_within_one_second() {
    let Some(bin) = find_helper() else { return };
    let scratch = ScratchDir::new("drop-stream");
    let mut fake = FakeScreen::start(scratch.path(), "qa", 320, 200);
    let mut helper = QuicHelper::start(&bin, scratch.path()).await;
    let link = QuicLink::connect(helper.to, &helper.print, helper.token).await.expect("quic connect");

    let mut stream = link.open_screen(&["stream".to_owned(), "qa".to_owned()]).await.expect("open_screen stream qa");
    // Read the initial keyframe so `olo-rex-link screen stream qa` is fully up
    // and connected to our stand-in compositor.
    let (msg, _) = tokio::time::timeout(Duration::from_secs(5), read_server_msg_async(&mut stream)).await.expect("initial frame timeout").unwrap().expect("initial frame");
    assert!(matches!(msg, ServerMsg::Frame { seq: 1, .. }), "expected initial frame, got {msg:?}");

    // Start flooding paints, wait until the tag byte of frame 2 arrives (and
    // verify it really IS `MSG_FRAME` — Test Finding 7), then time drop + child
    // exit against ONE absolute 1-second deadline started BEFORE drop.
    fake.flood(20);
    let mut next_frame_byte = [0u8; 1];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut next_frame_byte)).await.expect("frame 2 byte timeout").expect("frame 2 byte");
    assert_eq!(next_frame_byte[0], MSG_FRAME, "expected prefetched second message to be MSG_FRAME");
    let child_pid = direct_child_with_args(helper.pid(), &["screen", "stream", "qa"]).expect("screen stream child running under quic helper");
    let pidfd = PidFd::open(child_pid).expect("pidfd_open screen stream child");
    let t0 = Instant::now();
    let deadline = t0 + Duration::from_secs(1);
    std::thread::spawn(move || drop(stream)).join().unwrap();
    wait_pidfd_exit(pidfd, deadline).await.expect("screen stream child did not exit within 1s of QUIC stream drop");
    eprintln!("MEASURE quic drop active `screen stream qa` child exited in {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);

    fake.stop_flood();
    drop(link);
    drop(helper.input.take());
    let status = tokio::time::timeout(Duration::from_secs(10), helper.child.wait()).await.unwrap().unwrap();
    assert!(status.success());
}

/// Finding 13 & Test Finding 1: controlled Quinn peer advertising EXACTLY 1
/// bidirectional stream credit. Holding that 1 credit deterministically exhausts
/// stream credits so `open_screen_within` times out cleanly while keeping the
/// live `QuicLink` healthy; releasing the credit immediately restores
/// `open_screen` on the same connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quic_open_screen_times_out_when_bidi_stream_credits_exhausted() {
    use olo_rex_link::quic_wire as wire;

    let (cert_der, key_der, print) = make_test_quic_cert();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut tls = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![rustls::pki_types::CertificateDer::from(cert_der)], rustls::pki_types::PrivateKeyDer::Pkcs8(key_der.into()))
        .unwrap();
    tls.alpn_protocols = vec![wire::ALPN.to_vec()];
    let mut server_cfg = quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap()));
    let mut transport = quinn::TransportConfig::default();
    transport.max_concurrent_bidi_streams(1u32.into());
    server_cfg.transport_config(Arc::new(transport));

    let server = quinn::Endpoint::server(server_cfg, (Ipv4Addr::LOCALHOST, 0).into()).unwrap();
    let addr = server.local_addr().unwrap();
    let srv = tokio::spawn(async move {
        let Some(inc) = server.accept().await else { return };
        let Ok(conn) = inc.await else { return };
        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
            tokio::spawn(async move {
                let mut head = [0u8; wire::TOKEN + 1];
                if recv.read_exact(&mut head).await.is_err() {
                    return;
                }
                match head[wire::TOKEN] {
                    wire::PLAIN => {
                        let _ = send.write_all(b"H").await;
                        let mut sink = [0u8; 64];
                        while let Ok(Some(_)) = recv.read(&mut sink).await {}
                        let _ = send.finish();
                    }
                    wire::SCREEN => {
                        let mut lb = [0u8; 2];
                        if recv.read_exact(&mut lb).await.is_err() {
                            return;
                        }
                        let mut body = vec![0u8; u16::from_le_bytes(lb) as usize];
                        if recv.read_exact(&mut body).await.is_err() {
                            return;
                        }
                        let _ = send.write_all(b"{\"proto\":1}\n").await;
                        let _ = send.finish();
                    }
                    _ => {}
                }
            });
        }
    });

    let token = new_token().unwrap();
    let link = QuicLink::connect(addr, &print, token).await.expect("quic connect");

    // Occupy the peer's sole bidirectional stream credit deterministically.
    let mut held = link.open(false).await.expect("open sole plain stream");
    let mut ack = [0u8; 1];
    tokio::time::timeout(Duration::from_secs(2), held.read_exact(&mut ack)).await.unwrap().unwrap();
    assert_eq!(&ack, b"H");

    let t0 = Instant::now();
    let err = link.open_screen_within(&["follow".to_owned()], Duration::from_millis(200)).await.err().expect("open_screen must time out when the sole stream credit is held");
    let elapsed = t0.elapsed();
    assert!(err.contains("timed out opening QUIC screen stream"), "expected pre-dispatch OpenTimeout error, got: {err}");
    assert!(elapsed < Duration::from_secs(2), "timeout took too long: {elapsed:?}");
    assert!(link.alive(), "stream-credit timeout must leave the healthy QUIC link alive");

    // Releasing the sole held stream immediately restores `open_screen` on the same link.
    drop(held);
    let mut s = link.open_screen(&["follow".to_owned()]).await.expect("open_screen recovers once sole credit frees");
    let mut reader = BufReader::new(&mut s);
    let line = tokio::time::timeout(Duration::from_secs(5), read_bounded_line(&mut reader)).await.unwrap().unwrap();
    assert_eq!(line, "{\"proto\":1}\n");
    drop(s);
    link.close_now();
    drop(link);
    srv.abort();
}

async fn time_plain_round_trips(plain: &mut std::pin::Pin<Box<dyn olo_rex_client::transport::AsyncReadWrite + Send>>, count: usize, spacing: Duration) -> Vec<Duration> {
    let mut times = Vec::with_capacity(count);
    for i in 0..count {
        let msg = format!("ping-{i:02}\n");
        let start = Instant::now();
        plain.write_all(msg.as_bytes()).await.unwrap();
        let mut back = vec![0u8; msg.len()];
        tokio::time::timeout(Duration::from_secs(5), plain.read_exact(&mut back)).await.expect("plain echo timeout").unwrap();
        assert_eq!(back, msg.as_bytes());
        times.push(start.elapsed());
        if !spacing.is_zero() {
            tokio::time::sleep(spacing).await;
        }
    }
    times
}

fn summarize_ms(samples: &[Duration]) -> (f64, f64, f64) {
    let mut s = samples.to_vec();
    s.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    (ms(s[0]), ms(s[s.len() / 2]), ms(s[s.len() - 1]))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quic_plain_stream_not_held_up_by_full_rate_screen_stream() {
    let Some(bin) = find_helper() else { return };
    let scratch = ScratchDir::new("latency");
    // 480×270 changing screen (~200–250 KiB compressed delta per frame at 10 fps)
    // saturates a 512 KiB/s shaped UDP relay while delivering multiple complete
    // frames during the 10 probe round trips.
    let mut fake = FakeScreen::start(scratch.path(), "qa", 480, 270);
    let mut helper = QuicHelper::start(&bin, scratch.path()).await;

    // 1. Direct loopback link (unconstrained) + 2. Bandwidth-shaped UDP link
    //    (512 KiB/s) so QUIC send-queue priority (-1 vs 0) actively arbitrates.
    for (label, rate) in [("loopback", None), ("shaped-512KiB/s", Some(512.0 * 1024.0))] {
        let relay = UdpRelay::start(helper.to, rate).await;
        let link = QuicLink::connect(relay.listen, &helper.print, helper.token).await.expect("quic connect");
        let mut plain = link.open(false).await.expect("plain stream");
        // Warm up `cat` process spawn on the plain stream.
        let _ = time_plain_round_trips(&mut plain, 1, Duration::ZERO).await;

        let baseline = time_plain_round_trips(&mut plain, 10, Duration::from_millis(15)).await;

        let screen = link.open_screen(&["stream".to_owned(), "qa".to_owned()]).await.expect("open_screen stream");
        let (mut s_read, mut s_write) = tokio::io::split(screen);
        let frames = Arc::new(AtomicUsize::new(0));
        let bytes = Arc::new(AtomicU64::new(0));
        let stop_drain = Arc::new(AtomicBool::new(false));
        let (fr, by, st) = (frames.clone(), bytes.clone(), stop_drain.clone());
        let drain = tokio::spawn(async move {
            while !st.load(Ordering::Relaxed) {
                match tokio::time::timeout(Duration::from_secs(3), read_server_msg_async(&mut s_read)).await {
                    Ok(Ok(Some((ServerMsg::Frame { seq, .. }, wire_bytes)))) => {
                        fr.fetch_add(1, Ordering::Relaxed);
                        by.fetch_add(wire_bytes as u64, Ordering::Relaxed);
                        let (tag, payload) = encode_client_msg(&ClientMsg::Ack { seq }).unwrap();
                        let mut hdr = [0u8; 5];
                        hdr[0] = tag;
                        hdr[1..5].copy_from_slice(&(payload.len() as u32).to_le_bytes());
                        if s_write.write_all(&hdr).await.is_err() || s_write.write_all(&payload).await.is_err() {
                            break;
                        }
                    }
                    Ok(Ok(Some(_))) => {}
                    _ => break,
                }
            }
        });

        fake.flood(20);
        // Wait until the screen stream has delivered >= 2 frames so we measure
        // strictly while it is streaming at full rate.
        let ready_deadline = Instant::now() + Duration::from_secs(5);
        while frames.load(Ordering::Relaxed) < 2 {
            assert!(Instant::now() < ready_deadline, "{label}: screen stream did not deliver 2 frames in time");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let frames_before = frames.load(Ordering::Relaxed);
        let bytes_before = bytes.load(Ordering::Relaxed);
        let wire_before = relay.s2c_bytes.load(Ordering::Relaxed);

        let under_load = time_plain_round_trips(&mut plain, 10, Duration::from_millis(30)).await;

        let frames_during = frames.load(Ordering::Relaxed) - frames_before;
        let bytes_during = bytes.load(Ordering::Relaxed) - bytes_before;
        let wire_during = relay.s2c_bytes.load(Ordering::Relaxed) - wire_before;
        fake.stop_flood();
        stop_drain.store(true, Ordering::Relaxed);
        drain.abort();
        let _ = drain.await;

        let (b_min, b_med, b_max) = summarize_ms(&baseline);
        let (l_min, l_med, l_max) = summarize_ms(&under_load);
        eprintln!(
            "MEASURE {label}: plain RTT baseline min/med/max = {b_min:.2}/{b_med:.2}/{b_max:.2} ms; \
             beside full-rate screen stream ({frames_during} complete frames, {} KiB frame bytes, {} KiB UDP wire bytes during probes) min/med/max = {l_min:.2}/{l_med:.2}/{l_max:.2} ms",
            bytes_during / 1024,
            wire_during / 1024,
        );

        assert!(wire_during > 64 * 1024, "{label}: expected screen stream to push >64 KiB over UDP while timing plain RTT (got {wire_during} B)");
        if rate.is_none() {
            assert!(l_med < 25.0 && l_max < 100.0, "{label}: plain stream held up by screen stream (med {l_med:.2} ms, max {l_max:.2} ms)");
        } else {
            // Assert BOTH median AND worst individual tail echo (Test Finding 7).
            assert!(l_med < 250.0 && l_max < 500.0, "{label}: plain stream RTT held up on shaped link (med {l_med:.2} ms, max {l_max:.2} ms)");
        }
        drop(plain);
        link.close_now();
        drop(link);
        // Brief flush so CONNECTION_CLOSE crosses the relay before we stop it.
        tokio::time::sleep(Duration::from_millis(50)).await;
        relay.stop().await;
    }

    drop(helper.input.take());
    let status = tokio::time::timeout(Duration::from_secs(10), helper.child.wait()).await.unwrap().unwrap();
    assert!(status.success());
}
