//! Exercise the app's own connection loop. A private Unix relay can cut its link without
//! restarting Rex, or duplicate / omit one real server event before it reaches the app.
use super::*;
use std::{
    io,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UnixListener, UnixStream},
    sync::watch,
    time::{sleep, timeout},
};

struct Server {
    child: Child,
    dir: PathBuf,
}

impl Server {
    fn start(dir: &Path) -> Self {
        let addr = format!("unix://{}", dir.join("backend.sock").display());
        let child = Command::new("/usr/bin/rex")
            .args(["-S", &addr, "server", "run", "--listen", &addr, "--restore=false", "--state-dir"])
            .arg(dir.join("state"))
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self { child, dir: dir.into() }
    }
    fn kill(&mut self) {
        let _ = self.child.kill();
        self.child.wait().unwrap();
    }
    fn restart(&mut self) {
        self.kill();
        *self = Self::start(&self.dir);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The server's ends of the connections a cut left open ([`Link::half_open`]).
type Held = Arc<std::sync::Mutex<Vec<(tokio::net::unix::OwnedReadHalf, tokio::net::unix::OwnedWriteHalf)>>>;

/// What the link does to the app's requests and the terminals' streams
/// as they pass, for a test to set and read.
#[derive(Default)]
struct Steer {
    /// So many terminal streams opened to take over a held one (their
    /// first frame a RESUME) are ended as they begin.
    cut_resumes: AtomicUsize,
    /// So many terminal streams bring their welcome and then nothing.
    stall_streams: AtomicUsize,
    /// The next size request is held until `release` (what the app
    /// sends after it waits behind it, as on a slow link).
    hold_next_resize: std::sync::atomic::AtomicBool,
    holding: std::sync::atomic::AtomicBool,
    release: tokio::sync::Notify,
    /// Each size request the app sent (columns, rows), and whether one
    /// was held as it came.
    resizes: std::sync::Mutex<Vec<(u64, u64, bool)>>,
    /// Terminal streams through the link now, and those a cut ended.
    terminals_live: AtomicUsize,
    terminals_cut: AtomicUsize,
    /// Claims of panes' sizes the app sent (`session.own_block`).
    owns: AtomicUsize,
    /// Terminal streams opened through the link, all told.
    terminals_opened: AtomicUsize,
    /// So many terminal streams bring their welcome and then a frame with
    /// nothing in it every so often, and nothing else.
    drip_streams: AtomicUsize,
    /// So many terminal streams bring their welcome, and their first
    /// screen only once `screen_release` is said (`screens_held`: how
    /// many wait so now).
    hold_screens: AtomicUsize,
    screens_held: AtomicUsize,
    screen_release: tokio::sync::Notify,
    /// What a terminal's stream brings at most, in bytes a second (none:
    /// as fast as it comes): a narrow link.
    throttle: std::sync::atomic::AtomicU64,
    /// The answer to the app's next claim of a pane (`session.own_block`)
    /// is held (`held_answer`: its request's id, until it comes;
    /// `answers_held`: held now) until `answer_release` is said, and what
    /// the server sends after it goes by meanwhile.
    hold_own_answer: std::sync::atomic::AtomicBool,
    held_answer: std::sync::Mutex<Option<String>>,
    answers_held: AtomicUsize,
    answer_release: tokio::sync::Notify,
}

/// One of so many (`count`) taken, if any is left: one at a time, as
/// several streams may ask at once (each read it before either took it,
/// and the second's taking wrapped it round to all).
fn one_of(count: &AtomicUsize) -> bool {
    count.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok()
}

/// The length a frame's header says, read: what a frame of the fixture
/// may be at most (a header that says more is an error of the fixture's,
/// never an allocation of what it says).
async fn frame_length(r: &mut tokio::net::unix::OwnedReadHalf, second: u8, wire: &mut Vec<u8>) -> io::Result<usize> {
    let len = match second & 127 {
        126 => {
            let n = r.read_u16().await?;
            wire.extend(n.to_be_bytes());
            usize::from(n)
        }
        127 => {
            let n = r.read_u64().await?;
            wire.extend(n.to_be_bytes());
            usize::try_from(n).map_err(|_| io::Error::other("a fixture frame longer than memory"))?
        }
        n => usize::from(n),
    };
    if len > 2 * 1024 * 1024 {
        return Err(io::Error::other("oversized fixture frame"));
    }
    Ok(len)
}

/// One WebSocket frame as a client sends it (masked): its bytes as they
/// came, and its payload unmasked.
async fn client_frame(r: &mut tokio::net::unix::OwnedReadHalf) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut wire = vec![r.read_u8().await?, r.read_u8().await?];
    let len = frame_length(r, wire[1], &mut wire).await?;
    let mut mask = [0u8; 4];
    if wire[1] & 0x80 != 0 {
        r.read_exact(&mut mask).await?;
        wire.extend(mask);
    }
    let start = wire.len();
    wire.resize(start + len, 0);
    r.read_exact(&mut wire[start..]).await?;
    let payload = wire[start..].iter().enumerate().map(|(i, b)| b ^ mask[i % 4]).collect();
    Ok((wire, payload))
}

/// A server frame (unmasked) as it came.
async fn server_frame(r: &mut tokio::net::unix::OwnedReadHalf) -> io::Result<Vec<u8>> {
    let mut wire = vec![r.read_u8().await?, r.read_u8().await?];
    let len = frame_length(r, wire[1], &mut wire).await?;
    let start = wire.len();
    wire.resize(start + len, 0);
    r.read_exact(&mut wire[start..]).await?;
    Ok(wire)
}

/// The size a request asks for, where it is one (its method's JSON).
fn size_asked(payload: &[u8]) -> Option<(u64, u64)> {
    let message: Value = serde_json::from_slice(payload).ok()?;
    (message.get("method")?.as_str()? == "com.superlogical.terminal.resize").then_some(())?;
    let args = message.pointer("/payload/args")?;
    Some((args.get("columns")?.as_u64()?, args.get("rows")?.as_u64()?))
}

struct Link {
    up: watch::Sender<bool>,
    duplicate_perform: Arc<std::sync::atomic::AtomicBool>,
    omit_hosts: Arc<std::sync::atomic::AtomicBool>,
    pongs: Arc<AtomicUsize>,
    /// A cut is a network's change, not a cable's end: the app's side of
    /// each connection ends at once and the server's stays open and
    /// unread (`held`), as a server's TCP connection to an address that
    /// went stays established until its own timeouts end it.
    half_open: Arc<std::sync::atomic::AtomicBool>,
    held: Held,
    /// Each change ends every terminal's stream through the link, both
    /// sides, and nothing else: a way here that broke while the control
    /// connection stands.
    streams_cut: watch::Sender<u64>,
    /// No terminal's stream gets through while set (each is ended as
    /// the server answers its upgrade).
    refuse_streams: Arc<std::sync::atomic::AtomicBool>,
    steer: Arc<Steer>,
}

impl Link {
    async fn start(dir: &Path) -> Self {
        let listener = UnixListener::bind(dir.join("rex.sock")).unwrap();
        let backend = dir.join("backend.sock");
        let (up, watch_link) = watch::channel(true);
        let (streams_cut, watch_streams) = watch::channel(0u64);
        let link = Self {
            up,
            duplicate_perform: Default::default(),
            omit_hosts: Default::default(),
            pongs: Default::default(),
            half_open: Default::default(),
            held: Default::default(),
            streams_cut,
            refuse_streams: Default::default(),
            steer: Default::default(),
        };
        let (duplicate, omit, pongs, half_open, held, refuse, steer) =
            (link.duplicate_perform.clone(), link.omit_hosts.clone(), link.pongs.clone(), link.half_open.clone(), link.held.clone(), link.refuse_streams.clone(), link.steer.clone());
        tokio::spawn(async move {
            while let Ok((app, _)) = listener.accept().await {
                let mut state = watch_link.clone();
                if !*state.borrow_and_update() {
                    continue;
                }
                let mut cut_streams = watch_streams.clone();
                cut_streams.borrow_and_update();
                let Ok(server) = UnixStream::connect(&backend).await else { continue };
                let (duplicate, omit, pongs, half_open, held, refuse, steer) = (duplicate.clone(), omit.clone(), pongs.clone(), half_open.clone(), held.clone(), refuse.clone(), steer.clone());
                tokio::spawn(async move {
                    let (mut ar, mut aw) = app.into_split();
                    let (mut sr, mut sw) = server.into_split();
                    let stream = std::sync::atomic::AtomicBool::new(false);
                    // What the app sends: its upgrade's request, then frames.
                    // A size request may be held (and what comes after it
                    // with it, in order); a stream that takes over a held
                    // one may be ended as it begins.
                    let requests = async {
                        let mut header = Vec::new();
                        while !header.ends_with(b"\r\n\r\n") {
                            if header.len() > 8192 {
                                return Err(io::Error::other("oversized fixture header"));
                            }
                            header.push(ar.read_u8().await?);
                        }
                        let said = String::from_utf8_lossy(&header).into_owned();
                        sw.write_all(&header).await?;
                        if said.contains("rex.data.terminal.v1") {
                            let (wire, payload) = client_frame(&mut ar).await?;
                            if payload.first() == Some(&0x02) && one_of(&steer.cut_resumes) {
                                return Err(io::Error::other("a takeover cut"));
                            }
                            sw.write_all(&wire).await?;
                            tokio::io::copy(&mut ar, &mut sw).await?;
                            return Ok(());
                        }
                        if !said.contains("rex.control.v1") {
                            tokio::io::copy(&mut ar, &mut sw).await?;
                            return Ok(());
                        }
                        let (frames_tx, mut frames) = tokio::sync::mpsc::unbounded_channel::<(Vec<u8>, bool)>();
                        let read = async {
                            loop {
                                let (wire, payload) = client_frame(&mut ar).await?;
                                let mut hold = false;
                                if let Some(own) = serde_json::from_slice::<Value>(&payload).ok().filter(|m| m.get("method").and_then(Value::as_str) == Some("session.own_block")) {
                                    steer.owns.fetch_add(1, Ordering::SeqCst);
                                    // (Its answer to be held, where the test asked so.)
                                    if steer.hold_own_answer.swap(false, Ordering::SeqCst) {
                                        *steer.held_answer.lock().unwrap() = own.get("id").and_then(Value::as_str).map(str::to_owned);
                                    }
                                }
                                if let Some((cols, rows)) = size_asked(&payload) {
                                    steer.resizes.lock().unwrap().push((cols, rows, steer.holding.load(Ordering::SeqCst)));
                                    hold = steer.hold_next_resize.swap(false, Ordering::SeqCst);
                                    if hold {
                                        steer.holding.store(true, Ordering::SeqCst);
                                    }
                                }
                                if frames_tx.send((wire, hold)).is_err() {
                                    return Ok::<(), io::Error>(());
                                }
                            }
                        };
                        let write = async {
                            while let Some((wire, hold)) = frames.recv().await {
                                if hold {
                                    steer.release.notified().await;
                                    steer.holding.store(false, Ordering::SeqCst);
                                }
                                sw.write_all(&wire).await?;
                            }
                            Ok::<(), io::Error>(())
                        };
                        tokio::select! { r = read => r, r = write => r }
                    };
                    // Only the server's unmasked control messages are inspected. Data streams
                    // and requests pass unchanged until the test cuts this connection.
                    let replies = async {
                        let mut header = Vec::new();
                        while !header.ends_with(b"\r\n\r\n") {
                            if header.len() > 8192 {
                                return Err(io::Error::other("oversized fixture header"));
                            }
                            header.push(sr.read_u8().await?);
                        }
                        aw.write_all(&header).await?;
                        if !String::from_utf8_lossy(&header).contains("rex.control.v1") {
                            let terminal = String::from_utf8_lossy(&header).contains("rex.data.terminal.v1");
                            if terminal && refuse.load(Ordering::SeqCst) {
                                return Err(io::Error::other("streams refused"));
                            }
                            stream.store(terminal, Ordering::SeqCst);
                            if terminal {
                                steer.terminals_live.fetch_add(1, Ordering::SeqCst);
                                steer.terminals_opened.fetch_add(1, Ordering::SeqCst);
                                // (A welcome, then nothing: a way that stopped.)
                                if one_of(&steer.stall_streams) {
                                    aw.write_all(&server_frame(&mut sr).await?).await?;
                                    std::future::pending::<()>().await;
                                }
                                // (A welcome, then frames with nothing in them,
                                // and never a screen.)
                                if one_of(&steer.drip_streams) {
                                    aw.write_all(&server_frame(&mut sr).await?).await?;
                                    loop {
                                        sleep(Duration::from_millis(300)).await;
                                        aw.write_all(&[0x82, 0x00]).await?;
                                    }
                                }
                                // (A welcome, then its first screen once the
                                // test lets it come: of a stream the server
                                // took, its answer a welcome, not a refusal.)
                                if steer.hold_screens.load(Ordering::SeqCst) > 0 {
                                    let answer = server_frame(&mut sr).await?;
                                    aw.write_all(&answer).await?;
                                    if answer.get(2) == Some(&0x81) && one_of(&steer.hold_screens) {
                                        steer.screens_held.fetch_add(1, Ordering::SeqCst);
                                        steer.screen_release.notified().await;
                                        steer.screens_held.fetch_sub(1, Ordering::SeqCst);
                                    }
                                }
                                // (No more a second than the link takes, where it is narrow.)
                                loop {
                                    let frame = server_frame(&mut sr).await?;
                                    aw.write_all(&frame).await?;
                                    let rate = steer.throttle.load(Ordering::SeqCst);
                                    if rate > 0 {
                                        sleep(Duration::from_secs_f64(frame.len() as f64 / rate as f64)).await;
                                    }
                                }
                            }
                            tokio::io::copy(&mut sr, &mut aw).await?;
                            return Ok(());
                        }
                        // (Read and written apart: an answer held on its way
                        // lets what comes after it go by, as on a link that
                        // brought them in another order.)
                        let (frames_tx, mut frames) = tokio::sync::mpsc::unbounded_channel::<(Vec<u8>, bool)>();
                        let read = async {
                            loop {
                                let mut h = [0u8; 2];
                                sr.read_exact(&mut h).await?;
                                let mut wire = h.to_vec();
                                let len = frame_length(&mut sr, h[1], &mut wire).await?;
                                let start = wire.len();
                                wire.resize(start + len, 0);
                                sr.read_exact(&mut wire[start..]).await?;
                                if h[0] & 15 == 10 {
                                    pongs.fetch_add(1, Ordering::SeqCst);
                                }
                                let mut hold = false;
                                let held_id = steer.held_answer.lock().unwrap().clone();
                                if h[0] & 15 == 1 && (omit.load(Ordering::SeqCst) || duplicate.load(Ordering::SeqCst) || held_id.is_some()) {
                                    let message: Value = serde_json::from_slice(&wire[start..])?;
                                    let kind = message.pointer("/payload/type").and_then(Value::as_str);
                                    if kind == Some("hosts_changed") && omit.swap(false, Ordering::SeqCst) {
                                        continue;
                                    }
                                    if kind == Some("client_perform_requested") && duplicate.swap(false, Ordering::SeqCst) && frames_tx.send((wire.clone(), false)).is_err() {
                                        return Ok(());
                                    }
                                    if message.get("type").and_then(Value::as_str) == Some("response") && held_id.is_some() && message.get("id").and_then(Value::as_str) == held_id.as_deref() {
                                        steer.held_answer.lock().unwrap().take();
                                        hold = true;
                                    }
                                }
                                if frames_tx.send((wire, hold)).is_err() {
                                    return Ok(());
                                }
                            }
                        };
                        let write = async {
                            let mut held: Vec<Vec<u8>> = Vec::new();
                            loop {
                                tokio::select! {
                                    biased;
                                    _ = steer.answer_release.notified(), if !held.is_empty() => {
                                        for wire in held.drain(..) {
                                            aw.write_all(&wire).await?;
                                        }
                                        steer.answers_held.store(0, Ordering::SeqCst);
                                    }
                                    frame = frames.recv() => {
                                        let Some((wire, hold)) = frame else { return Ok(()) };
                                        if hold {
                                            held.push(wire);
                                            steer.answers_held.fetch_add(1, Ordering::SeqCst);
                                        } else {
                                            aw.write_all(&wire).await?;
                                        }
                                    }
                                }
                            }
                        };
                        tokio::select! { r = read => r, r = write => r }
                    };
                    let streams_cut = async {
                        while cut_streams.changed().await.is_ok() {
                            if stream.load(Ordering::SeqCst) {
                                return;
                            }
                        }
                        std::future::pending::<()>().await
                    };
                    let (cut, by_streams_cut) = tokio::select! {_=state.changed()=>(true, false),_=streams_cut=>(false, true),_=requests=>(false, false),_=replies=>(false, false)};
                    let terminal = stream.load(Ordering::SeqCst);
                    if cut && half_open.load(Ordering::SeqCst) {
                        held.lock().unwrap().push((sr, sw));
                    } else {
                        drop((sr, sw));
                    }
                    drop((ar, aw));
                    if terminal {
                        steer.terminals_live.fetch_sub(1, Ordering::SeqCst);
                        if by_streams_cut {
                            steer.terminals_cut.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                });
            }
        });
        link
    }

    /// Every terminal's stream through the link is ended, both ways: how
    /// many there were.
    fn cut_streams(&self) -> usize {
        let live = self.steer.terminals_live.load(Ordering::SeqCst);
        self.streams_cut.send_modify(|n| *n += 1);
        live
    }

    /// The server's ends a half-open cut kept are let go: the server
    /// hears them end (its timeouts, at last).
    fn release(&self) -> usize {
        let mut held = self.held.lock().unwrap();
        let n = held.len();
        held.clear();
        n
    }
}

async fn until<T>(what: &str, mut read: impl FnMut() -> Option<T>) -> T {
    timeout(Duration::from_secs(12), async {
        loop {
            if let Some(v) = read() {
                break v;
            }
            sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

fn control(hub: &RexHub) -> Option<ControlClient> {
    hub.state.lock().unwrap().hosts["local"].control.clone()
}

async fn session(hub: &RexHub, ctrl: &ControlClient) -> BlockKey {
    let r = ctrl.session_create(Some("scratch"), None).await.unwrap();
    let sid = r.payload["session_id"].as_str().unwrap();
    until("subscribed terminal and snapshot", || {
        let st = hub.state.lock().unwrap();
        st.blocks.iter().find_map(|(k, b)| (k.session_id == sid && b.lock().unwrap().input_ready).then(|| k.clone()))
    })
    .await
}

async fn echo() -> SocketAddr {
    let l = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    addr
}

async fn through(port: u16, marker: &[u8]) {
    timeout(Duration::from_secs(5), async {
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(marker).await.unwrap();
        let mut out = vec![0; marker.len()];
        s.read_exact(&mut out).await.unwrap();
        assert_eq!(out, marker);
    })
    .await
    .expect("forward did not return the echo");
}

async fn forwards(hub: &Arc<RexHub>, link: &Link, server: &mut Server) {
    let ctrl = until("first connection", || control(hub)).await;
    let key = session(hub, &ctrl).await;
    let mut ports = Vec::new();
    for i in 0..2 {
        let addr = echo().await;
        let port = hub.inner.ensure_ephemeral_forward("local", &key.session_id, "127.0.0.1", addr.port()).await.unwrap();
        through(port, &[i]).await;
        ports.push(port);
    }

    link.up.send(false).unwrap();
    until("offline", || control(hub).is_none().then_some(())).await;
    assert!(hub.state.lock().unwrap().forwards.iter().all(|f| f.state == ForwardState::Waiting));
    link.up.send(true).unwrap();
    until("both restored", || hub.state.lock().unwrap().forwards.iter().all(|f| f.state == ForwardState::Active).then_some(())).await;
    assert!(!ctrl.same_connection(&control(hub).unwrap()));
    for p in &ports {
        through(*p, b"new connection").await;
    }

    // Restart without restoration: neither original tunnel block exists on the new server.
    link.up.send(false).unwrap();
    server.restart();
    until("offline after kill", || control(hub).is_none().then_some(())).await;
    link.up.send(true).unwrap();
    until("each missing tunnel reported", || {
        let st = hub.state.lock().unwrap();
        (st.forwards.len() == 2 && st.forwards.iter().all(|f| matches!(&f.state,ForwardState::Failed(reason) if !reason.is_empty()))).then_some(())
    })
    .await;
    assert!(hub.state.lock().unwrap().hosts["local"].session_summaries.is_empty());
    eprintln!("Two echo forwards served the same ports after a link cut. Restarting without either session reported both failures.");
}

async fn events(hub: &Arc<RexHub>, link: &Link, server: &mut Server, clips: &AtomicUsize) {
    let ctrl = until("first connection", || control(hub)).await;
    let key = session(hub, &ctrl).await;
    let ep = RexEndpoint::Unix(server.dir.join("backend.sock"));
    let (s, host) = ep.connect_stream().await.unwrap();
    let (observer, _events) = ControlClient::connect(s, &host, olo_rex_client::ClientIdentity::olo_rex(Some("fixture".into())), None).await.unwrap();

    link.duplicate_perform.store(true, Ordering::SeqCst);
    observer.call_raw("client.perform", Some(json!({"client_id":ctrl.client_id(),"action":"olo.clipboard.set","args":{"text":"duplicate proof"}}))).await.unwrap();
    link.omit_hosts.store(true, Ordering::SeqCst);
    let mut record = HostRecord { label: "Fixture".into(), endpoints: vec![format!("unix://{}", server.dir.join("rex.sock").display())], ..Default::default() };
    let put = observer.hosts_put(&record).await.unwrap().payload;
    record.id = Some(put["host"]["id"].as_str().unwrap().into());
    until("omitted real hosts_changed", || (!link.omit_hosts.load(Ordering::SeqCst)).then_some(())).await;
    ctrl.session_set_label(&key.session_id, "after gap").await.unwrap();
    until("host list recovered by refresh", || hub.state.lock().unwrap().hosts["local"].synced_hosts.iter().any(|r| r.id == record.id).then_some(())).await;
    assert_eq!(clips.load(Ordering::SeqCst), 1);
    assert!(!link.duplicate_perform.load(Ordering::SeqCst));

    link.up.send(false).unwrap();
    until("offline", || control(hub).is_none().then_some(())).await;
    record.label = "Changed while disconnected".to_owned();
    observer.hosts_put(&record).await.unwrap();
    link.up.send(true).unwrap();
    until("fresh bootstrap after reconnect", || hub.state.lock().unwrap().hosts["local"].synced_hosts.iter().any(|r| r.label == "Changed while disconnected").then_some(())).await;
    assert_eq!(clips.load(Ordering::SeqCst), 1);
    let resumed = control(hub).unwrap();
    observer.call_raw("client.perform", Some(json!({"client_id":resumed.client_id(),"action":"olo.clipboard.set","args":{"text":"after reconnect"}}))).await.unwrap();
    assert_eq!(clips.load(Ordering::SeqCst), 2);
    server.restart();
    until("fresh server state", || {
        let st = hub.state.lock().unwrap();
        let h = &st.hosts["local"];
        (h.control.as_ref().is_some_and(|c| !c.same_connection(&ctrl)) && h.session_summaries.is_empty()).then_some(())
    })
    .await;
    eprintln!("One duplicated action was applied once. One omitted host update was recovered by refresh. Reconnect fetched offline changes and restart cleared the old sessions.");
}

async fn input(hub: &Arc<RexHub>, link: &Link, server: &mut Server) {
    let ctrl = until("connection", || control(hub)).await;
    let key = session(hub, &ctrl).await;
    let dir = &server.dir;
    let logfile = dir.join("keys");
    // The shell records bytes after canonical input commits a line. The log is wholly private.
    hub.send_input(&key, format!("stty -echo; cat > {}\r", shell_quote(&logfile.to_string_lossy())).into_bytes());
    until("input recorder", || logfile.exists().then_some(())).await;
    for b in b"abc\r" {
        hub.send_input(&key, vec![*b]);
    }
    until("first ordered line", || (std::fs::read(&logfile).unwrap() == b"abc\n").then_some(())).await;
    // Pause responses so only the first RPC can be in flight. The other writes must die with its queue.
    assert_eq!(unsafe { libc::kill(server.child.id() as i32, libc::SIGSTOP) }, 0);
    hub.send_input(&key, b"X".to_vec());
    for _ in 0..32 {
        hub.send_input(&key, b"unwritten".to_vec());
    }
    link.up.send(false).unwrap();
    until("old queue closed", || control(hub).is_none().then_some(())).await;
    hub.send_input(&key, b"blind".to_vec());
    assert_eq!(unsafe { libc::kill(server.child.id() as i32, libc::SIGCONT) }, 0);
    link.up.send(true).unwrap();
    until("same pane restored", || hub.writer_for_key(&key, None)).await;
    assert!(hub.writer_for_key(&key, Some(&ctrl)).is_none()); // Clipboard request from before the drop.
    for b in b"def\r" {
        hub.send_input(&key, vec![*b]);
    }
    let bytes = until("second ordered line", || {
        let b = std::fs::read(&logfile).unwrap();
        b.ends_with(b"def\n").then_some(b)
    })
    .await;
    assert!(bytes == b"abc\ndef\n" || bytes == b"abc\nXdef\n", "unexpected input {bytes:?}");

    link.up.send(false).unwrap();
    server.restart();
    until("restart gap", || control(hub).is_none().then_some(())).await;
    hub.send_input(&key, b"old-pane".to_vec());
    link.up.send(true).unwrap();
    let fresh = until("new server", || control(hub)).await;
    let replacement = session(hub, &fresh).await;
    assert_ne!(replacement, key);
    assert!(hub.writer_for_key(&key, None).is_none());
    assert!(hub.writer_for_key(&replacement, None).is_some());
    eprintln!("Pre- and post-gap bytes arrived once, in order. Unwritten and offline bytes disappeared; a replacement pane cannot receive them.");
}

async fn key_probe(hub: &Arc<RexHub>, link: &Link, server: &Server) {
    let local = until("local connection", || control(hub)).await;
    let local_pane = session(hub, &local).await;
    // A real HTTP transport on loopback to the same disposable Unix-socket Rex server.
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let socket = server.dir.join("rex.sock");
    tokio::spawn(async move {
        while let Ok((mut tcp, _)) = listener.accept().await {
            let mut unix = UnixStream::connect(&socket).await.unwrap();
            tokio::spawn(async move {
                let _ = tokio::io::copy_bidirectional(&mut tcp, &mut unix).await;
            });
        }
    });
    hub.add_and_connect_host("loopback".into(), "Remote fixture".into(), format!("http://{address}"), false);
    let remote_pane = BlockKey { host_key: "loopback".into(), ..local_pane.clone() };
    let remote = until("remote pane screen", || {
        let st = hub.state.lock().unwrap();
        if !st.blocks.get(&remote_pane)?.lock().unwrap().input_ready {
            return None;
        }
        st.hosts["loopback"].control.clone()
    })
    .await;
    let modified_key =
        || RexKeyEvent { key: "key_a".into(), mods: "ctrl".into(), consumed_mods: String::new(), text: String::new(), unshifted_codepoint: 97, composing: false, action: "press".into() };

    let marker = server.dir.join("unix-key");
    hub.send_rex_key(&local_pane, modified_key());
    hub.send_input(&local_pane, format!("printf ok > {}\r", shell_quote(&marker.to_string_lossy())).into_bytes());
    until("local keys written", || marker.exists().then_some(())).await;
    assert_eq!(link.pongs.load(Ordering::SeqCst), 0);

    hub.send_input(&remote_pane, b"x".to_vec());
    until("probe for remote text", || (link.pongs.load(Ordering::SeqCst) == 1).then_some(())).await;
    remote.session_view(&remote_pane.session_id).await.unwrap();
    hub.send_rex_key(&remote_pane, modified_key());
    until("probe for remote modified key", || (link.pongs.load(Ordering::SeqCst) == 2).then_some(())).await;
    remote.session_view(&remote_pane.session_id).await.unwrap();

    assert_eq!(unsafe { libc::kill(server.child.id() as i32, libc::SIGSTOP) }, 0);
    until("silent fixture", || std::fs::read_to_string(format!("/proc/{}/status", server.child.id())).ok()?.lines().any(|l| l.starts_with("State:\tT")).then_some(())).await;
    let start = Instant::now();
    hub.send_input(&local_pane, b"y".to_vec());
    hub.send_input(&remote_pane, b"y".to_vec());
    timeout(Duration::from_secs(6), async {
        remote.disconnected().await;
        until("app shows remote disconnected", || matches!(&hub.state.lock().unwrap().hosts["loopback"].conn_state, HostConnState::Reconnecting { .. }).then_some(())).await;
    })
    .await
    .expect("remote key failed to expose a silent link");
    assert!(control(hub).is_some_and(|c| c.same_connection(&local)));
    eprintln!("Unix keys sent zero probes. Both remote key paths elicited a Pong. Silent remote reported disconnected in {:.3} s.", start.elapsed().as_secs_f64());
    assert_eq!(unsafe { libc::kill(server.child.id() as i32, libc::SIGCONT) }, 0);
}

/// A client of the test's own, straight to the server (past the link):
/// what it writes reaches a pane however the app's link stands.
async fn observer(server: &Server, session_id: &str) -> ControlClient {
    let (s, host) = RexEndpoint::Unix(server.dir.join("backend.sock")).connect_stream().await.unwrap();
    let (observer, _events) = ControlClient::connect(s, &host, olo_rex_client::ClientIdentity::a_test(), None).await.unwrap();
    observer.session_attach(session_id).await.unwrap();
    observer
}

/// Whether a pane, as the app has it, shows `text` (it comes there only
/// through a stream of the pane's).
fn shows(hub: &RexHub, key: &BlockKey, text: &str) -> bool {
    let st = hub.state.lock().unwrap();
    let Some(block) = st.blocks.get(key) else { return false };
    let g = block.lock().unwrap();
    let (_, rows) = g.terminal.size();
    (0..u32::from(rows)).any(|row| g.terminal.viewport_row_text(row).contains(text))
}

/// Wi-Fi left for Ethernet (Ben, 2026-11-28: "after i switch from wifi
/// to ethernet and rex reconnects, most of the subagents come back but
/// the parent ones don't seem to show anything"). The app's ends of its
/// connections go at once and the server's stay open, as a server's
/// connection to an address that went does until its own timeouts. The
/// app connects again as the same client (its token), and the server
/// refuses every new stream of a pane whose last one it still holds
/// ("AlreadyConnected"): each pane was tried once and left blank.
async fn half_open(hub: &Arc<RexHub>, link: &Link, server: &Server) {
    let ctrl = until("first connection", || control(hub)).await;
    let key = session(hub, &ctrl).await;
    ctrl.session_new_split(&key.session_id, Some(&key.block_id), "right", None).await.unwrap();
    let other = until("second pane's screen", || {
        let st = hub.state.lock().unwrap();
        st.blocks.iter().find_map(|(k, b)| (k.session_id == key.session_id && *k != key && b.lock().unwrap().input_ready).then(|| k.clone()))
    })
    .await;
    let observer = observer(server, &key.session_id).await;
    let client = ctrl.client_id().map(str::to_owned);
    link.half_open.store(true, Ordering::SeqCst);
    link.up.send(false).unwrap();
    until("offline", || control(hub).is_none().then_some(())).await;
    link.up.send(true).unwrap();
    let again = until("connected again", || control(hub)).await;
    assert_eq!(again.client_id().map(str::to_owned), client, "connected again as the same client");
    assert!(link.held.lock().unwrap().len() >= 3, "the server's ends of the control connection and both streams are still open");
    let started = Instant::now();
    for k in [&key, &other] {
        observer.terminal_write(&k.session_id, &k.block_id, "echo came''-back\r").await.unwrap();
    }
    for k in [&key, &other] {
        until("a pane whose last stream the server still held shows again", || shows(hub, k, "came-back").then_some(())).await;
    }
    eprintln!(
        "Both panes showed again {:.0} ms after the app was connected again, with the server's {} ends of the old connections still open.",
        started.elapsed().as_secs_f64() * 1000.0,
        link.release()
    );
}

/// The network changes under a host's connection that nothing is sent
/// on meanwhile: one whose way went with it was found gone only by its
/// keepalive, minutes on (or by a key typed to it).
async fn network_change(hub: &Arc<RexHub>, server: &Server) {
    until("local connection", || control(hub)).await;
    // A real HTTP transport on loopback to the same server, as `key_probe`'s.
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let socket = server.dir.join("backend.sock");
    tokio::spawn(async move {
        while let Ok((mut tcp, _)) = listener.accept().await {
            let mut unix = UnixStream::connect(&socket).await.unwrap();
            tokio::spawn(async move {
                let _ = tokio::io::copy_bidirectional(&mut tcp, &mut unix).await;
            });
        }
    });
    hub.add_and_connect_host("loopback".into(), "Remote fixture".into(), format!("http://{address}"), false);
    let remote = until("remote connection", || hub.state.lock().unwrap().hosts.get("loopback")?.control.clone()).await;
    assert_eq!(unsafe { libc::kill(server.child.id() as i32, libc::SIGSTOP) }, 0);
    until("silent fixture", || std::fs::read_to_string(format!("/proc/{}/status", server.child.id())).ok()?.lines().any(|l| l.starts_with("State:\tT")).then_some(())).await;
    let start = Instant::now();
    olo_net::pretend_change(true, None);
    let found = timeout(Duration::from_secs(8), remote.disconnected()).await;
    assert_eq!(unsafe { libc::kill(server.child.id() as i32, libc::SIGCONT) }, 0);
    found.expect("a network change asks a host connected over it whether it still is");
    eprintln!("A silent host was found gone {:.3} s after the network changed.", start.elapsed().as_secs_f64());
}

/// The size the server's terminal has now (its PTY's), and its owner.
async fn host_size(observer: &ControlClient, key: &BlockKey) -> (u64, u64, Option<String>) {
    let size = observer.call_raw("com.superlogical.terminal.size", Some(json!({"session_id": key.session_id, "block_id": key.block_id, "args": {}}))).await.unwrap().payload;
    (size["columns"].as_u64().unwrap_or(0), size["rows"].as_u64().unwrap_or(0), size["owner"].as_str().map(str::to_owned))
}

/// Until the server's terminal is `cols` × `rows`: what it was last.
async fn host_comes_to(observer: &ControlClient, key: &BlockKey, cols: u64, rows: u64) -> (u64, u64, Option<String>) {
    let mut last = host_size(observer, key).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while (last.0, last.1) != (cols, rows) && Instant::now() < deadline {
        sleep(Duration::from_millis(50)).await;
        last = host_size(observer, key).await;
    }
    last
}

/// A window dragged larger: its pane's view wants a new size at each
/// step. Each was asked of the host as it came, all on their way at
/// once, and the server does two at once in either order: the terminal
/// could end at a size of the middle of the drag, and the view, already
/// at the last, never asked again. The first size is held on its way
/// (a slow link) while the drag goes on: nothing more may be sent
/// before its answer, and then the last size alone.
async fn sizes_in_a_row(hub: &Arc<RexHub>, link: &Link, server: &Server) {
    let ctrl = until("first connection", || control(hub)).await;
    let key = session(hub, &ctrl).await;
    let observer = observer(server, &key.session_id).await;
    let steer = &link.steer;
    steer.resizes.lock().unwrap().clear();
    steer.hold_next_resize.store(true, Ordering::SeqCst);
    hub.resize_block(&key, 100, 30, 8, 16);
    until("the first size held on its way", || steer.holding.load(Ordering::SeqCst).then_some(())).await;
    for cols in 101..124 {
        hub.resize_block(&key, cols, 30, 8, 16);
    }
    // (Anything the drag sent while the first was held has reached the
    // link by now; nothing should have: this waits only on a failure.)
    let sent_meanwhile = timeout(Duration::from_millis(500), async {
        while steer.resizes.lock().unwrap().len() < 2 {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    steer.release.notify_one();
    let now = host_comes_to(&observer, &key, 123, 30).await;
    let asked: Vec<(u64, u64, bool)> = steer.resizes.lock().unwrap().clone();
    assert!(!sent_meanwhile, "nothing more was sent while the first size was on its way: {asked:?}");
    assert_eq!(asked, [(100, 30, false), (123, 30, false)], "the first size, then the last alone");
    assert_eq!((now.0, now.1), (123, 30), "the host's terminal is at the last size the view wanted");
    eprintln!("A drag of 24 steps, its first held on its way, sent 100 then 123 alone, and the terminal is 123×30.");
}

/// A pane whose size another client holds (another desktop's view, an
/// older agents service) is taken when its tab is in front of a window
/// the person brings forward or that changes size while forward (moved
/// to a larger display), and given that window's size.
async fn tab_in_front_taken(hub: &Arc<RexHub>, server: &Server) {
    let ctrl = until("first connection", || control(hub)).await;
    let key = session(hub, &ctrl).await;
    let other = observer(server, &key.session_id).await;
    other.call_raw("session.own_block", Some(json!({"session_id": key.session_id, "block_ids": [key.block_id]}))).await.unwrap();
    other.terminal_resize(&key.session_id, &key.block_id, 90, 20, Some(8), Some(16)).await.unwrap();
    let held = |hub: &RexHub| hub.state.lock().unwrap().blocks.get(&key).is_some_and(|b| b.lock().unwrap().other_size_owner);
    until("the app knows another client holds the pane's size", || held(hub).then_some(())).await;
    assert!(!hub.resize_block(&key, 150, 40, 8, 16), "another client's size isn't asked of the host");
    assert_eq!(host_size(&other, &key).await.0, 90);
    assert!(hub.take_tab_in_front(&key.host_key, &key.session_id), "the tab in front has a pane to take");
    until("the pane taken", || (!held(hub)).then_some(())).await;
    hub.resize_block(&key, 150, 40, 8, 16);
    let now = host_comes_to(&other, &key, 150, 40).await;
    assert_eq!(now, (150, 40, ctrl.client_id().map(str::to_owned)), "the host's terminal is this view's size, and the app's to size");
    assert!(!hub.take_tab_in_front(&key.host_key, &key.session_id), "nothing more to take");
    eprintln!("A pane another client held at 90×20 was taken with its tab in front and given 150×40.");
}

/// After the connection is made again, a pane another client holds
/// knows so from the session, before (or without) its stream: it was
/// taken as this client's to size until its stream said otherwise.
async fn owner_from_the_session(hub: &Arc<RexHub>, link: &Link, server: &Server) {
    let ctrl = until("first connection", || control(hub)).await;
    let key = session(hub, &ctrl).await;
    let other = observer(server, &key.session_id).await;
    other.call_raw("session.own_block", Some(json!({"session_id": key.session_id, "block_ids": [key.block_id]}))).await.unwrap();
    link.refuse_streams.store(true, Ordering::SeqCst);
    link.up.send(false).unwrap();
    until("offline", || control(hub).is_none().then_some(())).await;
    link.up.send(true).unwrap();
    until("connected again", || control(hub)).await;
    let held = until("the pane, made again", || {
        let st = hub.state.lock().unwrap();
        st.blocks.get(&key).map(|b| {
            let b = b.lock().unwrap();
            (b.other_size_owner, b.input_ready)
        })
    })
    .await;
    assert_eq!(held, (true, false), "known to be another's to size, with no stream to say so");
    assert!(!hub.resize_block(&key, 150, 40, 8, 16), "so its size isn't asked of the host");
    link.refuse_streams.store(false, Ordering::SeqCst);
    eprintln!("A pane made again with no stream knew another client held its size.");
}

/// A pane's stream ends while its host's connection stands (the way of
/// its stream broke, not the control connection's): it was never had
/// again, and the pane showed nothing more.
async fn stream_lost(hub: &Arc<RexHub>, link: &Link, server: &Server) {
    let ctrl = until("first connection", || control(hub)).await;
    let key = session(hub, &ctrl).await;
    let observer = observer(server, &key.session_id).await;
    let live = link.cut_streams();
    assert!(live >= 1, "the pane's stream passes the link");
    // (Written once the link has ended the old stream, both ways: it can
    // show only through a new one.)
    until("the old stream ended", || (link.steer.terminals_cut.load(Ordering::SeqCst) >= live).then_some(())).await;
    observer.terminal_write(&key.session_id, &key.block_id, "echo back''-again\r").await.unwrap();
    until("the pane, attached again, shows what came after", || shows(hub, &key, "back-again").then_some(())).await;
    assert!(control(hub).is_some_and(|c| c.same_connection(&ctrl)), "the control connection stood throughout");
    eprintln!("A pane whose stream was cut while its host stayed connected was attached again.");
}

/// Wi-Fi left for Ethernet, and the first try to take the held stream
/// over is cut on its way too (Astra R1 #1, Neon R1 #1): what the old
/// stream left was spent on it, and the pane stayed blank until the
/// server's old end timed out. It is kept for the next try.
async fn takeover_cut(hub: &Arc<RexHub>, link: &Link, server: &Server) {
    let ctrl = until("first connection", || control(hub)).await;
    let key = session(hub, &ctrl).await;
    let observer = observer(server, &key.session_id).await;
    link.half_open.store(true, Ordering::SeqCst);
    link.steer.cut_resumes.store(1, Ordering::SeqCst);
    link.up.send(false).unwrap();
    until("offline", || control(hub).is_none().then_some(())).await;
    link.up.send(true).unwrap();
    until("connected again", || control(hub)).await;
    until("the first takeover cut", || (link.steer.cut_resumes.load(Ordering::SeqCst) == 0).then_some(())).await;
    observer.terminal_write(&key.session_id, &key.block_id, "echo taken''-over\r").await.unwrap();
    until("the pane, its held stream taken over at the second try, shows again", || shows(hub, &key, "taken-over").then_some(())).await;
    eprintln!("A takeover cut on its way was tried again with what the old stream left; {} held ends let go.", link.release());
}

/// A stream whose way stops just after its welcome (Astra R1 #9): the
/// pane waited on it for ever, blank and taking no keys. Had again once
/// its first screen brings nothing for a while (here 1.5 s).
async fn first_screen_stalled(hub: &Arc<RexHub>, link: &Link, server: &Server) {
    let ctrl = until("first connection", || control(hub)).await;
    let key = session(hub, &ctrl).await;
    let observer = observer(server, &key.session_id).await;
    link.steer.stall_streams.store(1, Ordering::SeqCst);
    link.cut_streams();
    until("a stream that brings its welcome and stops", || (link.steer.stall_streams.load(Ordering::SeqCst) == 0).then_some(())).await;
    observer.terminal_write(&key.session_id, &key.block_id, "echo past''-the-stall\r").await.unwrap();
    until("the pane, had again past the stalled stream, shows what came", || shows(hub, &key, "past-the-stall").then_some(())).await;
    eprintln!("A stream that stopped after its welcome was given up and the pane had again.");
}

/// A pane whose tries ran out (its streams refused; here three tries)
/// stayed blank once its way was back: nothing tried it again but a
/// change of its session (Astra R1 #18). Choosing it now does.
async fn tries_spent(hub: &Arc<RexHub>, link: &Link, server: &Server) {
    let ctrl = until("first connection", || control(hub)).await;
    let key = session(hub, &ctrl).await;
    let observer = observer(server, &key.session_id).await;
    link.refuse_streams.store(true, Ordering::SeqCst);
    link.cut_streams();
    let left = |hub: &RexHub| hub.state.lock().unwrap().blocks.get(&key).is_some_and(|b| !b.lock().unwrap().data_connected);
    until("the pane's tries spent", || left(hub).then_some(())).await;
    link.refuse_streams.store(false, Ordering::SeqCst);
    observer.terminal_write(&key.session_id, &key.block_id, "echo chosen''-again\r").await.unwrap();
    hub.focus_block(&key.host_key, &key.session_id, &key.block_id);
    until("the pane, chosen, had again", || shows(hub, &key, "chosen-again").then_some(())).await;
    eprintln!("A pane whose tries ran out was had again when chosen.");
}

/// A window dragged while another client holds its tab's size: each
/// step asked to take it, each a request the host answered in turn
/// before anything typed after (Astra R1 #7, Neon R1 #2). Asked once.
async fn claimed_once(hub: &Arc<RexHub>, link: &Link, server: &Server) {
    let ctrl = until("first connection", || control(hub)).await;
    let key = session(hub, &ctrl).await;
    let other = observer(server, &key.session_id).await;
    other.call_raw("session.own_block", Some(json!({"session_id": key.session_id, "block_ids": [key.block_id]}))).await.unwrap();
    let held = |hub: &RexHub| hub.state.lock().unwrap().blocks.get(&key).is_some_and(|b| b.lock().unwrap().other_size_owner);
    until("the app knows another client holds the pane's size", || held(hub).then_some(())).await;
    link.steer.owns.store(0, Ordering::SeqCst);
    for _ in 0..30 {
        hub.take_tab_in_front(&key.host_key, &key.session_id);
    }
    until("the pane taken", || (!held(hub)).then_some(())).await;
    // (What else the drag queued has gone out by now: a key typed after
    // it goes out behind any claim still queued.)
    let marker = server.dir.join("typed");
    hub.send_input(&key, format!("printf ok > {}\r", shell_quote(&marker.to_string_lossy())).into_bytes());
    until("what was typed after the drag", || marker.exists().then_some(())).await;
    assert_eq!(link.steer.owns.load(Ordering::SeqCst), 1, "one claim for thirty steps");
    eprintln!("Thirty steps of a drag sent one claim of the pane.");
}

/// This process's open descriptors and threads (the hub runs in it).
fn held_now() -> (usize, usize) {
    let count = |dir: &str| std::fs::read_dir(dir).map(Iterator::count).unwrap_or(0);
    (count("/proc/self/fd"), count("/proc/self/task"))
}

/// Five networks left in a row, each pane's held stream taken over and
/// had again each time (its first try cut, three times of the five):
/// nothing is left behind of any of it, descriptors or threads
/// (LEAKS=0).
async fn nothing_left_behind(hub: &Arc<RexHub>, link: &Link, server: &Server) {
    let ctrl = until("first connection", || control(hub)).await;
    let key = session(hub, &ctrl).await;
    ctrl.session_new_split(&key.session_id, Some(&key.block_id), "right", None).await.unwrap();
    let panes = |hub: &RexHub| hub.state.lock().unwrap().blocks.iter().filter(|(k, b)| k.session_id == key.session_id && b.lock().unwrap().input_ready).count();
    until("both panes' screens", || (panes(hub) == 2).then_some(())).await;
    // (The two panes, known once: read after each connection was made
    // again, they could be none yet, its sessions not had, and a round
    // went by without a pane shown: Astra R2, Neon R2 #5.)
    let keys: Vec<BlockKey> = hub.state.lock().unwrap().blocks.keys().filter(|k| k.session_id == key.session_id).cloned().collect();
    assert_eq!(keys.len(), 2, "both panes");
    let observer = observer(server, &key.session_id).await;
    let settled = |hub: &RexHub, link: &Link| panes(hub) == 2 && link.steer.terminals_live.load(Ordering::SeqCst) == 2 && link.held.lock().unwrap().is_empty();
    until("settled before: two streams, nothing held", || settled(hub, link).then_some(())).await;
    let before = held_now();
    for round in 0..5 {
        link.half_open.store(true, Ordering::SeqCst);
        link.steer.cut_resumes.store(usize::from(round % 2 == 0), Ordering::SeqCst);
        link.up.send(false).unwrap();
        until("offline", || control(hub).is_none().then_some(())).await;
        link.up.send(true).unwrap();
        until("connected again", || control(hub)).await;
        let marker = format!("round''-{round}");
        let shown = format!("round-{round}");
        // (Each shows what was written after the cut while the server
        // still holds its old stream: had through its takeover, before
        // any held end is let go of.)
        for k in &keys {
            observer.terminal_write(&k.session_id, &k.block_id, &format!("echo {marker}\r")).await.unwrap();
        }
        for k in &keys {
            until("a pane back", || shows(hub, k, &shown).then_some(())).await;
        }
        link.half_open.store(false, Ordering::SeqCst);
        link.release();
        until("settled: two streams, nothing held", || settled(hub, link).then_some(())).await;
    }
    // (Until they are back where they were: a thread of tokio's blocking
    // pool, if one was started meanwhile, ends 10 s after its last work.)
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut after = held_now();
    while (after.0 > before.0 || after.1 > before.1) && Instant::now() < deadline {
        sleep(Duration::from_millis(50)).await;
        after = held_now();
    }
    eprintln!("Descriptors and threads before five networks left: {before:?}; after: {after:?}.");
    assert!(after.0 <= before.0, "no descriptor left behind: {before:?} then {after:?}");
    assert!(after.1 <= before.1, "no thread left behind: {before:?} then {after:?}");
}

/// A pane whose stream goes on pouring what its terminal wrote long ago,
/// its shell at its prompt again (a flood on a narrow link), is given a
/// new stream, whose first screen is the present (`said_over`: caught
/// up). That stream's first screen was given the old one's time, long
/// run out: on a pane shown longer than that, the new stream was ended
/// as soon as its welcome came, its screen still on its way; and keys
/// typed meanwhile went in against the old screen (Astra R2 #5, Neon R2
/// #1). Now the new screen is waited for, no key taken until it comes.
async fn caught_up(hub: &Arc<RexHub>, link: &Link, server: &Server) {
    let ctrl = until("first connection", || control(hub)).await;
    let key = session(hub, &ctrl).await;
    let observer = observer(server, &key.session_id).await;
    // (A shell that edits its own line: at its prompt, it is what reads, `shell_reads`.)
    // (Typed once its prompt is there: what was typed before it went.)
    observer.terminal_write(&key.session_id, &key.block_id, "exec bash --norc --noprofile\r").await.unwrap();
    until("bash at its prompt", || shows(hub, &key, "bash-").then_some(())).await;
    let opened = link.steer.terminals_opened.load(Ordering::SeqCst);
    link.steer.hold_screens.store(1, Ordering::SeqCst);
    link.steer.throttle.store(80 * 1024, Ordering::SeqCst);
    // (Much written there at once, and the shell at its prompt again: what
    // comes over the narrow link for a minute after is the past.)
    observer.terminal_write(&key.session_id, &key.block_id, "seq 1 400000\r").await.unwrap();
    until("caught up: the new stream's first screen held on its way", || (link.steer.screens_held.load(Ordering::SeqCst) == 1).then_some(())).await;
    link.steer.throttle.store(0, Ordering::SeqCst);
    // (The catch-up's streams, a refused hello tried again among them: the
    // server may not have had the old one's end yet.)
    let caught = link.steer.terminals_opened.load(Ordering::SeqCst);
    assert!(caught > opened, "a new stream: the catch-up's");
    let ready = || hub.state.lock().unwrap().blocks[&key].lock().unwrap().input_ready;
    // (Held well within its first screen's time: it is waited for, and
    // nothing typed goes in meanwhile.)
    let given_up = timeout(Duration::from_millis(1000), async {
        while link.steer.terminals_opened.load(Ordering::SeqCst) == caught && link.steer.screens_held.load(Ordering::SeqCst) == 1 {
            sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .is_ok();
    assert!(!given_up, "the new stream is waited for, not ended as it began");
    assert!(!ready() && hub.writer_for_key(&key, None).is_none(), "no key goes in against the old screen while the new one comes");
    link.steer.screen_release.notify_one();
    until("its new screen, and keys again", || ready().then_some(())).await;
    until("the present shown", || shows(hub, &key, "400000").then_some(())).await;
    assert_eq!(link.steer.terminals_opened.load(Ordering::SeqCst), caught, "and no other stream");
    eprintln!("Caught up with {} new stream(s), the last one's first screen waited for, no key taken meanwhile.", caught - opened);
}

/// A stream that brings its welcome and then only frames with nothing
/// in them is had again once its first screen's time runs out (Astra R2
/// #7: each empty frame gave it its whole time again, and one every so
/// often kept the pane blank for ever).
async fn empty_frames(hub: &Arc<RexHub>, link: &Link, server: &Server) {
    let ctrl = until("first connection", || control(hub)).await;
    let key = session(hub, &ctrl).await;
    let observer = observer(server, &key.session_id).await;
    let opened = link.steer.terminals_opened.load(Ordering::SeqCst);
    link.steer.drip_streams.store(1, Ordering::SeqCst);
    assert_eq!(link.cut_streams(), 1);
    until("the stream of empty frames", || (link.steer.terminals_opened.load(Ordering::SeqCst) > opened).then_some(())).await;
    observer.terminal_write(&key.session_id, &key.block_id, "echo dripped''-past\r").await.unwrap();
    until("had again past it, its screen shown", || shows(hub, &key, "dripped-past").then_some(())).await;
    assert_eq!(link.steer.terminals_opened.load(Ordering::SeqCst), opened + 2, "once more, its first screen's time out");
}

/// Streams that never show the pane, however long each lasts, are
/// failures in a row: the pane is left after its tries (Astra R2 #7:
/// each lasting longer than a stream that lasted, the count began again
/// at each, and they were tried for ever).
async fn never_shown(hub: &Arc<RexHub>, link: &Link) {
    let ctrl = until("first connection", || control(hub)).await;
    let key = session(hub, &ctrl).await;
    let block = hub.state.lock().unwrap().blocks[&key].clone();
    let opened = link.steer.terminals_opened.load(Ordering::SeqCst);
    link.steer.stall_streams.store(1000, Ordering::SeqCst);
    assert_eq!(link.cut_streams(), 1);
    until("left, its tries out", || (!block.lock().unwrap().data_connected).then_some(())).await;
    let tried = link.steer.terminals_opened.load(Ordering::SeqCst) - opened;
    assert!((1..=3).contains(&tried), "tried {tried} times");
    let more = timeout(Duration::from_secs(3), async {
        while link.steer.terminals_opened.load(Ordering::SeqCst) - opened == tried {
            sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .is_ok();
    assert!(!more, "and not again");
    link.steer.stall_streams.store(0, Ordering::SeqCst);
}

/// Rex claims a pane; before the host's answer comes, another desktop
/// takes it back, and the word of that comes first. The answer, come
/// last, gave the pane back to Rex all the same (Astra R2 #11, Neon R2
/// #3): Rex then sent sizes the host kept as wishes, and skipped the
/// claim it needed. Now the answer is taken as the host numbered it.
async fn claim_answered_late(hub: &Arc<RexHub>, link: &Link, server: &Server) {
    let ctrl = until("first connection", || control(hub)).await;
    let key = session(hub, &ctrl).await;
    let other = observer(server, &key.session_id).await;
    let block = hub.state.lock().unwrap().blocks[&key].clone();
    let said = || {
        let g = block.lock().unwrap();
        (g.other_size_owner, g.owner_seq, g.owning)
    };
    let first = other.session_own_block(&key.session_id, &key.block_id).await.unwrap().seq;
    until("the pane the other's", || (said().0 && said().1 >= first).then_some(())).await;
    link.steer.hold_own_answer.store(true, Ordering::SeqCst);
    assert!(hub.take_tab_in_front("local", &key.session_id), "claimed");
    until("its answer held on its way", || (link.steer.answers_held.load(Ordering::SeqCst) == 1).then_some(())).await;
    let again = other.session_own_block(&key.session_id, &key.block_id).await.unwrap().seq;
    until("the other's later word had", || (said().0 && said().1 >= again).then_some(())).await;
    link.steer.answer_release.notify_one();
    until("the claim's answer had", || (!said().2).then_some(())).await;
    assert!(said().0, "the other's, as the host last said: the answer, numbered before that, stands aside");
    let (_, _, owner) = host_size(&other, &key).await;
    assert_eq!(owner.as_deref(), other.client_id(), "the host agrees");
}

/// The streams opened beforehand for a connection are its own (Astra R2
/// #3): kept under the host's key, a connection the host had replaced
/// left them for the next one's panes, which sent their tickets down an
/// old endpoint's streams.
async fn spares_kept(hub: &Arc<RexHub>, server: &Server) {
    let ctrl = until("first connection", || control(hub)).await;
    let key = session(hub, &ctrl).await;
    let other = observer(server, &key.session_id).await;
    let spare = || -> Spare { tokio::spawn(async { Err(olo_rex_client::RexError::Closed("a spare of the test's".into())) }) };
    hub.inner.keep_spares("old-endpoint", &other, vec![spare(), spare()]);
    assert!(hub.inner.take_spare("old-endpoint", &ctrl).is_none(), "another connection's: none of them");
    hub.inner.let_go_of_spares("old-endpoint", &ctrl);
    assert!(hub.inner.take_spare("old-endpoint", &other).is_some(), "not let go of by another; taken by its own");
    hub.inner.let_go_of_spares("old-endpoint", &other);
    assert!(hub.inner.take_spare("old-endpoint", &other).is_none(), "let go of by its own");
}

/// The network goes (no way out left) under a host's connection: it was
/// asked only once a network came back (Astra R1 #12: the hook returned
/// at once while offline). Asked now too.
async fn network_gone(hub: &Arc<RexHub>, server: &Server) {
    until("local connection", || control(hub)).await;
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let socket = server.dir.join("backend.sock");
    tokio::spawn(async move {
        while let Ok((mut tcp, _)) = listener.accept().await {
            let mut unix = UnixStream::connect(&socket).await.unwrap();
            tokio::spawn(async move {
                let _ = tokio::io::copy_bidirectional(&mut tcp, &mut unix).await;
            });
        }
    });
    hub.add_and_connect_host("loopback".into(), "Remote fixture".into(), format!("http://{address}"), false);
    let remote = until("remote connection", || hub.state.lock().unwrap().hosts.get("loopback")?.control.clone()).await;
    assert_eq!(unsafe { libc::kill(server.child.id() as i32, libc::SIGSTOP) }, 0);
    until("silent fixture", || std::fs::read_to_string(format!("/proc/{}/status", server.child.id())).ok()?.lines().any(|l| l.starts_with("State:\tT")).then_some(())).await;
    olo_net::pretend_change(false, None);
    let found = timeout(Duration::from_secs(8), remote.disconnected()).await;
    assert_eq!(unsafe { libc::kill(server.child.id() as i32, libc::SIGCONT) }, 0);
    found.expect("a network going asks a host connected over it whether it still is");
    eprintln!("A silent host was found gone after the network went.");
}

fn fixture(case: &str) {
    if !Path::new("/usr/bin/rex").is_file() {
        eprintln!("skipping live app reconnect test: /usr/bin/rex unavailable");
        return;
    }
    if std::env::var("OLO_REX_RECONNECT_CASE").as_deref() != Ok(case) {
        let tmp = std::env::var_os("TMPDIR").map(PathBuf::from).filter(|p| p.starts_with("/var/tmp")).unwrap_or_else(|| "/var/tmp".into());
        let dir = tmp.join(format!("rx-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&dir).unwrap();
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", &format!("hub::reconnect_tests::{case}"), "--nocapture"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("SHELL", "/bin/sh")
            .env("HOME", &dir)
            .env("XDG_RUNTIME_DIR", &dir)
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("XDG_CACHE_HOME", dir.join("cache"))
            .env("XDG_STATE_HOME", dir.join("state-home"))
            .env("TMPDIR", &dir)
            .env("REX_SERVER_SOCKET", dir.join("rex.sock"))
            .env("OLO_REX_RECONNECT_CASE", case)
            .current_dir(&dir);
        if let Some(path) = std::env::var_os("LD_LIBRARY_PATH") {
            cmd.env("LD_LIBRARY_PATH", path);
        }
        // (Shorter waits where the case is about them.)
        match case {
            "a_first_screen_that_never_comes_is_had_again" => {
                cmd.env("OLO_REX_FIRST_SCREEN_QUIET_MS", "1500");
            }
            "a_pane_whose_tries_ran_out_is_had_again_when_chosen" => {
                cmd.env("OLO_REX_ATTACH_TRIES", "3");
            }
            "a_pane_caught_up_waits_for_its_new_screen" | "a_stream_of_empty_frames_is_had_again" => {
                cmd.env("OLO_REX_FIRST_SCREEN_QUIET_MS", "1500");
            }
            "tries_that_never_show_the_pane_run_out" => {
                cmd.env("OLO_REX_FIRST_SCREEN_QUIET_MS", "1500").env("OLO_REX_ATTACH_TRIES", "3").env("OLO_REX_STREAM_LASTED_MS", "1000");
            }
            _ => {}
        }
        let output = cmd.output().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    let dir = PathBuf::from(std::env::var_os("HOME").unwrap());
    assert!(dir.starts_with("/var/tmp") && dir.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.strip_prefix("rx-").is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())));
    assert_eq!(std::env::var_os("REX_SERVER_SOCKET"), Some(dir.join("rex.sock").into_os_string()));
    assert_eq!(std::env::var_os("XDG_RUNTIME_DIR").as_deref(), Some(dir.as_os_str()));
    let mut server = Server::start(&dir);
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let link = rt.block_on(Link::start(&dir));
    let clips = Arc::new(AtomicUsize::new(0));
    let seen = clips.clone();
    let hub = RexHub::start(Settings::default(), TerminalTheme::default(), None, None, move |n| {
        if matches!(n, HubNotice::SetClipboard(_)) {
            seen.fetch_add(1, Ordering::SeqCst);
        }
    });
    rt.block_on(async {
        timeout(Duration::from_secs(48), async {
            match case {
                "forward_restoration" => forwards(&hub, &link, &mut server).await,
                "event_recovery" => events(&hub, &link, &mut server, &clips).await,
                "input_during_disconnect" => input(&hub, &link, &mut server).await,
                "probe_on_remote_keys_only" => key_probe(&hub, &link, &server).await,
                "streams_held_after_a_network_change" => half_open(&hub, &link, &server).await,
                "a_stream_lost_while_connected" => stream_lost(&hub, &link, &server).await,
                "a_network_change_asks_at_once" => network_change(&hub, &server).await,
                "sizes_in_a_row_end_at_the_last" => sizes_in_a_row(&hub, &link, &server).await,
                "the_tab_in_front_is_taken" => tab_in_front_taken(&hub, &server).await,
                "a_panes_owner_is_known_from_its_session" => owner_from_the_session(&hub, &link, &server).await,
                "a_takeover_cut_on_its_way_is_tried_again" => takeover_cut(&hub, &link, &server).await,
                "a_first_screen_that_never_comes_is_had_again" => first_screen_stalled(&hub, &link, &server).await,
                "a_pane_whose_tries_ran_out_is_had_again_when_chosen" => tries_spent(&hub, &link, &server).await,
                "a_claim_is_sent_once_while_a_window_is_dragged" => claimed_once(&hub, &link, &server).await,
                "a_network_gone_asks_at_once" => network_gone(&hub, &server).await,
                "nothing_is_left_behind_across_networks" => nothing_left_behind(&hub, &link, &server).await,
                "a_pane_caught_up_waits_for_its_new_screen" => caught_up(&hub, &link, &server).await,
                "a_stream_of_empty_frames_is_had_again" => empty_frames(&hub, &link, &server).await,
                "tries_that_never_show_the_pane_run_out" => never_shown(&hub, &link).await,
                "a_claim_answered_after_a_later_owner_change_stands_aside" => claim_answered_late(&hub, &link, &server).await,
                "streams_opened_beforehand_are_their_connections_alone" => spares_kept(&hub, &server).await,
                _ => unreachable!(),
            }
        })
        .await
        .expect("app reconnect test exceeded its deadline")
    });
}

#[test]
fn forward_restoration() {
    fixture("forward_restoration")
}
#[test]
fn event_recovery() {
    fixture("event_recovery")
}
#[test]
fn input_during_disconnect() {
    fixture("input_during_disconnect")
}

#[test]
fn probe_on_remote_keys_only() {
    fixture("probe_on_remote_keys_only")
}

#[test]
fn streams_held_after_a_network_change() {
    fixture("streams_held_after_a_network_change")
}

#[test]
fn a_stream_lost_while_connected() {
    fixture("a_stream_lost_while_connected")
}

#[test]
fn a_network_change_asks_at_once() {
    fixture("a_network_change_asks_at_once")
}

#[test]
fn sizes_in_a_row_end_at_the_last() {
    fixture("sizes_in_a_row_end_at_the_last")
}

#[test]
fn the_tab_in_front_is_taken() {
    fixture("the_tab_in_front_is_taken")
}

#[test]
fn a_panes_owner_is_known_from_its_session() {
    fixture("a_panes_owner_is_known_from_its_session")
}

#[test]
fn a_takeover_cut_on_its_way_is_tried_again() {
    fixture("a_takeover_cut_on_its_way_is_tried_again")
}

#[test]
fn a_first_screen_that_never_comes_is_had_again() {
    fixture("a_first_screen_that_never_comes_is_had_again")
}

#[test]
fn a_pane_whose_tries_ran_out_is_had_again_when_chosen() {
    fixture("a_pane_whose_tries_ran_out_is_had_again_when_chosen")
}

#[test]
fn a_claim_is_sent_once_while_a_window_is_dragged() {
    fixture("a_claim_is_sent_once_while_a_window_is_dragged")
}

#[test]
fn a_network_gone_asks_at_once() {
    fixture("a_network_gone_asks_at_once")
}

#[test]
fn nothing_is_left_behind_across_networks() {
    fixture("nothing_is_left_behind_across_networks")
}

#[test]
fn a_pane_caught_up_waits_for_its_new_screen() {
    fixture("a_pane_caught_up_waits_for_its_new_screen")
}

#[test]
fn a_stream_of_empty_frames_is_had_again() {
    fixture("a_stream_of_empty_frames_is_had_again")
}

#[test]
fn tries_that_never_show_the_pane_run_out() {
    fixture("tries_that_never_show_the_pane_run_out")
}

#[test]
fn a_claim_answered_after_a_later_owner_change_stands_aside() {
    fixture("a_claim_answered_after_a_later_owner_change_stands_aside")
}

#[test]
fn streams_opened_beforehand_are_their_connections_alone() {
    fixture("streams_opened_beforehand_are_their_connections_alone")
}
