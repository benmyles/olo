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

struct Link {
    up: watch::Sender<bool>,
    duplicate_perform: Arc<std::sync::atomic::AtomicBool>,
    omit_hosts: Arc<std::sync::atomic::AtomicBool>,
    pongs: Arc<AtomicUsize>,
}

impl Link {
    async fn start(dir: &Path) -> Self {
        let listener = UnixListener::bind(dir.join("rex.sock")).unwrap();
        let backend = dir.join("backend.sock");
        let (up, watch_link) = watch::channel(true);
        let link = Self { up, duplicate_perform: Default::default(), omit_hosts: Default::default(), pongs: Default::default() };
        let (duplicate, omit, pongs) = (link.duplicate_perform.clone(), link.omit_hosts.clone(), link.pongs.clone());
        tokio::spawn(async move {
            while let Ok((app, _)) = listener.accept().await {
                let mut state = watch_link.clone();
                if !*state.borrow_and_update() {
                    continue;
                }
                let Ok(server) = UnixStream::connect(&backend).await else { continue };
                let (duplicate, omit, pongs) = (duplicate.clone(), omit.clone(), pongs.clone());
                tokio::spawn(async move {
                    let (mut ar, mut aw) = app.into_split();
                    let (mut sr, mut sw) = server.into_split();
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
                            tokio::io::copy(&mut sr, &mut aw).await?;
                            return Ok(());
                        }
                        loop {
                            let mut h = [0u8; 2];
                            sr.read_exact(&mut h).await?;
                            let mut wire = h.to_vec();
                            let len = match h[1] & 127 {
                                126 => {
                                    let n = sr.read_u16().await?;
                                    wire.extend(n.to_be_bytes());
                                    n as usize
                                }
                                127 => {
                                    let n = sr.read_u64().await?;
                                    wire.extend(n.to_be_bytes());
                                    usize::try_from(n).unwrap()
                                }
                                n => n as usize,
                            };
                            if len > 2 * 1024 * 1024 {
                                return Err(io::Error::other("oversized fixture frame"));
                            }
                            let start = wire.len();
                            wire.resize(start + len, 0);
                            sr.read_exact(&mut wire[start..]).await?;
                            if h[0] & 15 == 10 {
                                pongs.fetch_add(1, Ordering::SeqCst);
                            }
                            if h[0] & 15 == 1 && (omit.load(Ordering::SeqCst) || duplicate.load(Ordering::SeqCst)) {
                                let message: Value = serde_json::from_slice(&wire[start..])?;
                                let kind = message.pointer("/payload/type").and_then(Value::as_str);
                                if kind == Some("hosts_changed") && omit.swap(false, Ordering::SeqCst) {
                                    continue;
                                }
                                if kind == Some("client_perform_requested") && duplicate.swap(false, Ordering::SeqCst) {
                                    aw.write_all(&wire).await?;
                                }
                            }
                            aw.write_all(&wire).await?;
                        }
                    };
                    tokio::select! {_=state.changed()=>{},_=tokio::io::copy(&mut ar,&mut sw)=>{},_=replies=>{}}
                });
            }
        });
        link
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
