//! Real carriers in a private mount, pid and loopback network namespace.
//! The test process and all the remote programs have only the fixture's home.
use std::{
    fs,
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use olo_rex_client::*;
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};
use zeroize::Zeroizing;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
type Events = tokio::sync::mpsc::UnboundedReceiver<ControlEventEnvelope>;

fn outer(case: &str) -> bool {
    let exe = std::env::current_exe().unwrap();
    if exe == Path::new("/suite") {
        assert_eq!(std::env::var("REX_SERVER_SOCKET").unwrap(), "/fixture/rex/rex.sock");
        return false;
    }
    let result = Command::new("/usr/bin/python3")
        .args(["-I", concat!(env!("CARGO_MANIFEST_DIR"), "/tests/remote/fixture.py")])
        .arg(exe)
        .arg(case)
        .env_remove("REX_SESSION")
        .env_remove("REX_BLOCK")
        .env_remove("SSH_AUTH_SOCK")
        .status()
        .unwrap();
    assert!(result.success(), "{case} in its private namespace: {result}");
    true
}

macro_rules! case {
    ($name:ident, $body:expr) => {
        #[tokio::test]
        async fn $name() -> Result {
            if outer(stringify!($name)) {
                return Ok(());
            }
            $body
        }
    };
}

#[derive(Debug)]
struct Ask {
    trust: bool,
    key_prompt_delay: Duration,
    secret: Option<String>,
    keys: Mutex<Vec<bool>>,
    prompts: Mutex<Vec<String>>,
}

impl Ask {
    fn new(trust: bool, secret: Option<&str>) -> Arc<Self> {
        Arc::new(Self { trust, key_prompt_delay: Duration::ZERO, secret: secret.map(str::to_string), keys: Mutex::default(), prompts: Mutex::default() })
    }
}

impl SshAskpass for Ask {
    fn confirm_host_key(&self, _: &str, _: u16, fingerprint: &str, _: &str, changed: bool) -> bool {
        std::thread::sleep(self.key_prompt_delay);
        assert!(fingerprint.starts_with("SHA256:"));
        self.keys.lock().unwrap().push(changed);
        self.trust
    }

    fn prompt_secret(&self, title: &str, _: &str, echo: bool) -> Option<Zeroizing<String>> {
        assert!(!echo);
        self.prompts.lock().unwrap().push(title.to_owned());
        self.secret.clone().map(Zeroizing::new)
    }
}

fn endpoint(name: &str, ask: Option<Arc<dyn SshAskpass>>) -> RexEndpoint {
    RexEndpoint::parse(name, Arc::default(), ask).unwrap()
}

async fn connect(ep: &RexEndpoint, token: Option<&str>) -> Result<(ControlClient, Events)> {
    let (stream, host) = ep.connect_stream().await?;
    Ok(ControlClient::connect(stream, &host, ClientIdentity::a_test(), token).await?)
}

async fn session(ctrl: &ControlClient) -> Result<(String, String)> {
    let r = ctrl.session_create(Some("scratch"), Some(json!({"command":["/bin/sh"],"cwd":"/fixture/home"}))).await?.payload;
    ctrl.session_attach(r["session_id"].as_str().unwrap()).await?;
    Ok((r["session_id"].as_str().unwrap().into(), r["initial_windows"][0]["block_ids"][0].as_str().unwrap().into()))
}

async fn version(ep: &RexEndpoint) -> Result {
    let (ctrl, _) = connect(ep, None).await?;
    assert_eq!(ctrl.server_version().await?.payload.api_revision, REX_API_REVISION);
    ctrl.close();
    Ok(())
}

case!(http_and_discovery, {
    assert_eq!(local_socket_candidates(), [Path::new("/fixture/rex/rex.sock")]);
    for addr in ["local", "unix:///fixture/rex/rex.sock", "http://127.0.0.1:7297"] {
        version(&endpoint(addr, None)).await?;
    }
    Ok(())
});

case!(ssh_direct_exec_and_known_host, {
    let known = Path::new("/fixture/home/.ssh/known_hosts");
    let declined = Ask::new(false, None);
    assert!(connect(&endpoint("fixture", Some(declined.clone())), None).await.is_err());
    assert_eq!(*declined.keys.lock().unwrap(), [false]);
    assert!(!known.exists());
    let first = Ask::new(true, None);
    version(&endpoint("ssh://fixture/fixture/rex/rex.sock", Some(first.clone()))).await?;
    assert_eq!(*first.keys.lock().unwrap(), [false]);
    assert!(known.exists());
    assert!(!Path::new("/fixture/dial-calls").exists(), "explicit socket used the exec fallback");
    version(&endpoint("fixture", None)).await?;
    assert_eq!(fs::read_to_string("/fixture/dial-calls")?.trim(), "rex __internal dial-stdio");
    Ok(())
});

case!(ssh_changed_key, {
    let known = Path::new("/fixture/home/.ssh/known_hosts");
    let wrong = format!("[127.0.0.1]:2222 {}", fs::read_to_string("/fixture/identity.pub")?);
    fs::write(known, &wrong)?;
    let r = connect(&endpoint("fixture", Some(Ask::new(true, None))), None).await;
    let error = r.err().ok_or("accepted a changed host key")?.to_string();
    assert!(error.to_lowercase().contains("changed"), "{error}");
    assert_eq!(fs::read_to_string(known)?, wrong);
    Ok(())
});

case!(ssh_protected_key, {
    let ask = Ask::new(true, Some("test-passphrase"));
    version(&endpoint("protected", Some(ask.clone()))).await?;
    assert_eq!(*ask.prompts.lock().unwrap(), ["SSH Key Passphrase"]);
    let cancelled = Ask::new(true, None);
    let err = connect(&endpoint("protected", Some(cancelled)), None).await.err().ok_or("cancelled passphrase accepted")?.to_string();
    assert!(err.contains("cancelled"));
    assert!(connect(&endpoint("protected", Some(Ask::new(true, Some("wrong")))), None).await.is_err());
    Ok(())
});

case!(ssh_agent, {
    let sock = Path::new("/fixture/agent.sock");
    let status = inspect_ssh_agent(Some(sock)).await;
    assert!(status.reachable);
    assert_eq!(status.key_count, 1);
    let mut ep = endpoint("ssh://scratch@127.0.0.1:2222", Some(Ask::new(true, None)));
    let RexEndpoint::Ssh { target, .. } = &mut ep else { unreachable!() };
    target.agent_socket = Some(sock.into());
    version(&ep).await
});

case!(ssh_proxy_command, {
    // Only the proxy can reach sshd at this port.
    fs::write("/fixture/home/.ssh/config", "Host proxied\n HostName 127.0.0.1\n Port 2223\n User scratch\n IdentityFile /fixture/identity\n ProxyCommand /usr/bin/ncat %h 2222\n")?;
    version(&endpoint("proxied", Some(Ask::new(true, None)))).await
});

case!(ssh_proxy_jump, {
    fs::write("/fixture/home/.ssh/config", "Host jump-test\n HostName 127.0.0.1\n Port 2222\n User scratch\n IdentityFile /fixture/identity\n ProxyJump fixture\n")?;
    let err = connect(&endpoint("jump-test", Some(Ask::new(true, None))), None).await.err().ok_or("ignored configured jump")?.to_string();
    assert!(err.contains("ProxyJump"), "{err}");
    Ok(())
});

case!(ssh_unreachable, {
    for addr in ["ssh://127.0.0.1:2223", "ssh://absent.invalid"] {
        assert!(timeout(Duration::from_secs(4), connect(&endpoint(addr, None), None)).await?.is_err());
    }
    let listener = TcpListener::bind("127.0.0.1:2223").await?;
    let ep = endpoint("ssh://127.0.0.1:2223", None);
    let start = Instant::now();
    let err = timeout(Duration::from_secs(4), connect(&ep, None)).await?.err().unwrap().to_string();
    assert!(err.contains("timed out"), "{err}");
    eprintln!("MEASURE silent SSH banner: {:?}", start.elapsed());
    drop(listener);
    Ok(())
});

case!(remote_forwards, {
    let ep = endpoint("ssh://fixture/fixture/rex/rex.sock", Some(Ask::new(true, None)));
    let (ctrl, _) = connect(&ep, None).await?;
    let (sid, _) = session(&ctrl).await?;
    let echo = TcpListener::bind("127.0.0.1:0").await?;
    let port = echo.local_addr()?.port();
    tokio::spawn(async move {
        while let Ok((mut peer, _)) = echo.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = peer.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    let fwd = start_local_forward(ctrl.clone(), ep.clone(), sid.clone(), "127.0.0.1".into(), port, 0).await?;
    assert!(start_local_forward(ctrl.clone(), ep.clone(), sid.clone(), "127.0.0.1".into(), port, fwd.local_addr.port()).await.is_err());
    for n in 0..100u8 {
        let mut peer = TcpStream::connect(fwd.local_addr).await?;
        let payload = vec![n; if n == 99 { 8 * 1024 * 1024 } else { 17 }];
        let (mut r, mut w) = peer.split();
        let mut seen = Vec::new();
        timeout(Duration::from_secs(8), async {
            tokio::try_join!(
                async {
                    w.write_all(&payload).await?;
                    w.shutdown().await
                },
                async { r.read_to_end(&mut seen).await.map(|_| ()) }
            )
        })
        .await??;
        assert_eq!(seen.len(), payload.len());
        assert!(seen == payload);
    }
    let mut active = TcpStream::connect(fwd.local_addr).await?;
    active.write_all(b"x").await?;
    assert_eq!(active.read_u8().await?, b'x');
    let addr = fwd.local_addr;
    let token = ctrl.reconnect_token().unwrap().to_owned();
    signal(&ssh_children()?, libc::SIGKILL)?;
    assert_eq!(timeout(Duration::from_secs(2), active.read(&mut [0])).await??, 0);
    let (ctrl, _) = connect(&ep, Some(&token)).await?;
    ctrl.session_bootstrap("all", None, None, &[]).await?;
    fwd.reconnect(ctrl.clone(), ep.clone()).await?;
    let mut active = TcpStream::connect(addr).await?;
    active.write_all(b"y").await?;
    assert_eq!(active.read_u8().await?, b'y');
    fwd.reconnect(ctrl.clone(), ep.clone()).await?;
    active.write_all(b"z").await?;
    assert_eq!(active.read_u8().await?, b'z');
    fwd.stop();
    assert_eq!(timeout(Duration::from_secs(2), active.read(&mut [0])).await??, 0);
    assert!(TcpStream::connect(addr).await.is_err());
    let refused = start_local_forward(ctrl.clone(), ep.clone(), sid.clone(), "127.0.0.1".into(), 1, 0).await?;
    let mut peer = TcpStream::connect(refused.local_addr).await?;
    assert_eq!(timeout(Duration::from_secs(3), peer.read(&mut [0])).await??, 0);
    // Left as Rex leaves its forwards across an upgrade in place (its
    // process ends; the block stays on the server, owned by nobody), and
    // taken up by another client on the same port: no second block.
    let left = start_local_forward(ctrl.clone(), ep.clone(), sid.clone(), "127.0.0.1".into(), port, 0).await?;
    let (block, here) = (left.block_id.clone(), left.local_addr);
    left.leave();
    let freed = Instant::now();
    while TcpStream::connect(here).await.is_ok() {
        assert!(freed.elapsed() < Duration::from_secs(2), "the left forward still listens");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (other, _) = connect(&ep, None).await?;
    other.session_bootstrap("all", None, None, &[]).await?;
    let again = resume_local_forward(other.clone(), ep.clone(), sid.clone(), block.clone(), "127.0.0.1".into(), port, here.port()).await?;
    assert_eq!(again.local_addr, here);
    let mut peer = TcpStream::connect(here).await?;
    peer.write_all(b"again").await?;
    let mut seen = [0u8; 5];
    timeout(Duration::from_secs(3), peer.read_exact(&mut seen)).await??;
    assert_eq!(&seen, b"again");
    // Stopped, it closes its block: there is nothing to take up then.
    again.stop();
    assert_eq!(timeout(Duration::from_secs(2), peer.read(&mut [0])).await??, 0);
    let gone = Instant::now();
    loop {
        match resume_local_forward(other.clone(), ep.clone(), sid.clone(), block.clone(), "127.0.0.1".into(), port, 0).await {
            Err(_) => break,
            Ok(still) => {
                still.leave();
                assert!(gone.elapsed() < Duration::from_secs(2), "a closed forward's block was taken up again");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    Ok(())
});

async fn credentials() -> Result {
    let name = "ssh://scratch@127.0.0.1:2222";
    let cancelled = Ask::new(true, None);
    let err = connect(&endpoint(name, Some(cancelled.clone())), None).await.err().ok_or("accepted cancelled login")?.to_string();
    assert!(err.contains("cancelled"), "{err}");
    assert_eq!(cancelled.prompts.lock().unwrap().len(), 1);
    let wrong = Ask::new(true, Some("wrong"));
    assert!(connect(&endpoint(name, Some(wrong.clone())), None).await.is_err());
    assert_eq!(wrong.prompts.lock().unwrap().len(), 3);
    let right = Ask::new(true, Some("fixture-password"));
    version(&endpoint(name, Some(right.clone()))).await?;
    assert_eq!(right.prompts.lock().unwrap().len(), 1);
    Ok(())
}

case!(ssh_password, credentials().await);
case!(ssh_interactive, credentials().await);
case!(ssh_missing_rex, {
    let err = connect(&endpoint("fixture", Some(Ask::new(true, None))), None).await.err().ok_or("connected without a remote program")?.to_string();
    assert!(err.contains("install Rex"), "{err}");
    Ok(())
});
case!(ssh_closes_after_auth, {
    let err = connect(&endpoint("fixture", Some(Ask::new(true, None))), None).await.err().ok_or("server closed its session")?.to_string();
    assert!(err.contains("42"), "{err}");
    Ok(())
});

#[test]
fn endpoints() {
    let cases = [
        ("  ", "Local"),
        (" Local ", "Local"),
        ("unix:///a b", "unix:///a b"),
        ("/folder/a.sock", "unix:///folder/a.sock"),
        ("host", "ssh://host"),
        ("ssh://a+b@host:2222/", "ssh://a+b@host:2222"),
        ("host.tail.ts.net", "https://host.tail.ts.net"),
        ("host:443", "http://host:443"),
        ("http://host/", "http://host:7297"),
        ("http://host:80", "http://host:80"),
        ("https://host:443/", "https://host"),
        ("ssh://u@[::1]:22/sock", "ssh://u@[::1]:22/sock"),
        ("[::1]:7297", "http://[::1]:7297"),
        ("::1", "https://[::1]"),
        ("[::1]", "https://[::1]"),
        ("https://[2001:db8::1]", "https://[2001:db8::1]"),
        ("user@dev", "ssh://user@dev"),
        ("user+tag@dev", "ssh://user+tag@dev"),
    ];
    for (raw, display) in cases {
        let parsed = endpoint(raw, None);
        assert_eq!(parsed.display(), display, "{raw}");
        assert_eq!(endpoint(display, None).display(), display);
    }
    for raw in [
        "unix://",
        "ssh://",
        "http://",
        "https://",
        "ssh://[::1]typo",
        "host:bad",
        "ssh://user@:22",
        "http://user@host",
        "odd scheme",
        "ssh://@host",
        "https://host/path",
        "https://host?x",
        "host:65536",
        "host:0",
        "ssh://host:",
        "ftp://host",
    ] {
        assert!(RexEndpoint::parse(raw, Arc::default(), None).is_err(), "{raw}");
    }
    for port in 1..=u16::MAX {
        let raw = format!("ssh://name@[::1]:{port}");
        assert_eq!(SshTarget::parse(&raw).unwrap().display(), raw);
    }
}

#[tokio::test]
async fn negotiated_revisions() -> Result {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{
        accept_hdr_async,
        tungstenite::{
            Message,
            handshake::server::{Request, Response},
        },
    };
    for (server, chosen, min, ok) in [(18, 18, 1, true), (20, 19, 1, true), (20, 20, 20, false), (19, 0, 0, false)] {
        let (client, peer) = tokio::io::duplex(8192);
        let server_task = tokio::spawn(async move {
            let mut ws = accept_hdr_async(peer, |_: &Request, mut response: Response| {
                response.headers_mut().insert("Sec-WebSocket-Protocol", CONTROL_SUBPROTOCOL.parse().unwrap());
                Ok(response)
            })
            .await
            .unwrap();
            ws.next().await.unwrap().unwrap();
            ws.send(Message::Text(
                json!({"type":"welcome","protocol":CONTROL_SUBPROTOCOL,"limits":{"max_message_bytes":1048576},
                 "api_server_revision":server,"api_revision":chosen,"api_min_revision":min})
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        });
        let got = ControlClient::connect(client, "localhost", ClientIdentity::a_test(), None).await;
        assert_eq!(got.is_ok(), ok, "server {server} selected {chosen}");
        server_task.await?;
    }
    Ok(())
}

/// A probe finds a server that has gone silent without closing, in
/// seconds: the link held open and nothing answering (an in-memory
/// server that reads on and never writes again). And a server that is
/// there answers it, and the connection stays; a probe asked for again
/// while one is out sends nothing more.
#[tokio::test]
async fn a_probe_finds_a_silent_server() -> Result {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{
        accept_hdr_async,
        tungstenite::{
            Message,
            handshake::server::{Request, Response},
        },
    };
    for answers in [true, false] {
        let (client, peer) = tokio::io::duplex(8192);
        let (pings, mut seen) = tokio::sync::mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            let mut ws = accept_hdr_async(peer, |_: &Request, mut response: Response| {
                response.headers_mut().insert("Sec-WebSocket-Protocol", CONTROL_SUBPROTOCOL.parse().unwrap());
                Ok(response)
            })
            .await
            .unwrap();
            ws.next().await.unwrap().unwrap();
            ws.send(Message::Text(
                json!({"type":"welcome","protocol":CONTROL_SUBPROTOCOL,"limits":{"max_message_bytes":1048576},"api_server_revision":19,"api_revision":19,"api_min_revision":1}).to_string().into(),
            ))
            .await
            .unwrap();
            // (Reading on, as a peer whose host is frozen doesn't: the
            // socket stays open either way. A pong goes back by itself
            // when this end is polled, unless it is told not to answer.)
            while let Some(Ok(frame)) = ws.next().await {
                if matches!(frame, Message::Ping(_)) {
                    let _ = pings.send(());
                    if answers {
                        let _ = ws.flush().await;
                    } else {
                        // Never polled again: nothing of its own goes out.
                        std::future::pending::<()>().await;
                    }
                }
            }
        });
        let (control, _events) = ControlClient::connect(client, "localhost", ClientIdentity::a_test(), None).await?;
        control.probe();
        control.probe();
        seen.recv().await.unwrap();
        // (By the clock: a second over the probe's five for the silent
        // one to be found, and for the other to have outlived them.)
        let gone = tokio::time::timeout(Duration::from_secs(6), control.disconnected()).await.is_ok();
        assert_eq!(gone, !answers, "a server that {} must {}", if answers { "answers" } else { "is silent" }, if answers { "be kept" } else { "be found gone within the probe's five seconds" });
        if answers {
            // One ping for the two calls; and the connection still serves.
            assert!(seen.try_recv().is_err(), "a second probe was sent while one was out");
        }
        server.abort();
    }
    Ok(())
}

const CHUNK: usize = 65_536;

fn sample_chunk(data: &mut [u8], offset: usize) {
    for (i, byte) in data.iter_mut().enumerate() {
        let n = (offset + i) as u64;
        *byte = (n.wrapping_mul(0x9e3779b97f4a7c15) >> 37) as u8;
    }
}

fn uploaded_file(root: &Path, name: &str) -> Option<std::path::PathBuf> {
    for e in fs::read_dir(root).ok()?.flatten() {
        if e.file_type().ok()?.is_dir() {
            if let Some(found) = uploaded_file(&e.path(), name) {
                return Some(found);
            }
        } else if e.file_name() == name {
            return Some(e.path());
        }
    }
    None
}

fn sha256(path: &Path) -> Result<String> {
    let out = Command::new("sha256sum").arg(path).output()?;
    assert!(out.status.success());
    Ok(String::from_utf8(out.stdout)?[..64].to_owned())
}

case!(remote_transfers, {
    use std::io::Write;
    let ep = endpoint("ssh://fixture/fixture/rex/rex.sock", Some(Ask::new(true, None)));
    let (ctrl, _) = connect(&ep, None).await?;
    let (sid, bid) = session(&ctrl).await?;
    let mut chunk = vec![0; CHUNK];
    for size in [0, 1, CHUNK, 100 * 1024 * 1024] {
        let started = Instant::now();
        let name = format!("payload-{size}.bin");
        let upload = ctrl.terminal_upload_begin(&sid, &bid, &format!("upload:{}", uuid::Uuid::new_v4()), &name, size as u64, None).await?.payload;
        let mut stream = open_bytes_stream(&ep, &upload.ticket).await?;
        let expected = Path::new("/fixture/expected");
        let mut file = fs::File::create(expected)?;
        for offset in (0..size).step_by(CHUNK) {
            let n = CHUNK.min(size - offset);
            sample_chunk(&mut chunk[..n], offset);
            file.write_all(&chunk[..n])?;
            stream.write_all(&chunk[..n]).await?;
        }
        drop(file);
        stream.finish_write().await?;
        assert!(timeout(Duration::from_secs(5), stream.read_chunk()).await??.is_none());
        let uploaded = uploaded_file(Path::new("/fixture"), &name).ok_or_else(|| format!("completed upload file {name} missing"))?;
        let expected_hash = sha256(expected)?;
        assert_eq!(sha256(&uploaded)?, expected_hash);
        let dl = ctrl.terminal_download_begin(&sid, &bid, &format!("download:{}", uuid::Uuid::new_v4()), uploaded.to_str().unwrap(), None).await?.payload;
        assert_eq!(dl.size, size as u64);
        let mut down = open_bytes_stream(&ep, dl.ticket.as_deref().ok_or("download ticket missing")?).await?;
        let destination = Path::new("/fixture/download-part");
        let mut file = fs::File::create(destination)?;
        let mut bytes = 0;
        while let Some(part) = down.read_chunk().await? {
            file.write_all(&part)?;
            bytes += part.len();
        }
        drop(file);
        assert_eq!(bytes, size);
        assert_eq!(sha256(destination)?, expected_hash);
        eprintln!("MEASURE {size} bytes each way, SHA-256 {} in {:.2?}", &expected_hash[..12], started.elapsed());
    }
    Ok(())
});

async fn terminal(ep: &RexEndpoint, ctrl: &ControlClient, sid: &str, bid: &str) -> Result<TerminalDataStream<RexStream>> {
    let ticket = ctrl.data_connect(sid, bid, "terminal", DATA_TERMINAL_SUBPROTOCOL).await?.payload.ticket;
    let (s, h) = ep.connect_stream().await?;
    Ok(TerminalDataStream::connect_hello(s, &h, &ticket).await?)
}

fn hwm_kib() -> Result<u64> {
    let status = fs::read_to_string("/proc/self/status")?;
    Ok(status.lines().find(|line| line.starts_with("VmHWM:")).unwrap().split_whitespace().nth(1).unwrap().parse()?)
}

case!(remote_output, {
    let ep = endpoint("ssh://fixture/fixture/rex/rex.sock", Some(Ask::new(true, None)));
    let (ctrl, _) = connect(&ep, None).await?;
    let (sid, bid) = session(&ctrl).await?;
    let mut term = terminal(&ep, &ctrl, &sid, &bid).await?;
    let TerminalStreamFrame::Bytes(first) = term.next_frame().await? else { panic!("snapshot absent") };
    assert!(first.starts_with(b"GHOSTSNP"));
    let before = hwm_kib()?;
    let started = Instant::now();
    // Terminal output is exactly 50 MiB of a single byte. Octal escapes in
    // the command keep its own echo distinct from the output being counted.
    ctrl.terminal_write(&sid, &bid, "/usr/bin/head -c 52428800 /dev/zero | /usr/bin/tr '\\000' '\\176'; printf '\\005'\n").await?;
    let mut total = 0;
    loop {
        let TerminalStreamFrame::Bytes(part) = term.next_frame().await? else { return Err("terminal closed during flood".into()) };
        total += part.iter().filter(|b| **b == b'~').count();
        if part.contains(&5) {
            break;
        }
    }
    assert_eq!(total, 50 * 1024 * 1024);
    let after = hwm_kib()?;
    assert!(after.saturating_sub(before) < 48 * 1024, "peak RSS grew from {before} to {after} KiB");
    eprintln!("MEASURE 50 MiB terminal in {:.2?}, peak RSS {before} -> {after} KiB", started.elapsed());
    ctrl.terminal_write(&sid, &bid, "seq 1 10000; printf '\\006'\n").await?;
    loop {
        let TerminalStreamFrame::Bytes(part) = term.next_frame().await? else { return Err("lost terminal before history filled".into()) };
        if part.contains(&6) {
            break;
        }
    }
    term.close_clean().await;
    let mut fresh = terminal(&ep, &ctrl, &sid, &bid).await?;
    let TerminalStreamFrame::Bytes(frame) = fresh.next_frame().await? else { return Err("long scrollback snapshot missing".into()) };
    assert!(frame.starts_with(b"GHOSTSNP"));
    let mut bytes = frame;
    let mut offset = 10;
    let mut history = 0;
    loop {
        while bytes.len() < offset + 10 {
            let TerminalStreamFrame::Bytes(part) = fresh.next_frame().await? else { return Err("snapshot truncated".into()) };
            bytes.extend(part);
        }
        let tag = u16::from_le_bytes(bytes[offset..offset + 2].try_into()?);
        let len = u32::from_le_bytes(bytes[offset + 2..offset + 6].try_into()?) as usize;
        offset += 10;
        assert!(offset + len < 64 * 1024 * 1024);
        while bytes.len() < offset + len {
            let TerminalStreamFrame::Bytes(part) = fresh.next_frame().await? else { return Err("snapshot record truncated".into()) };
            bytes.extend(part);
        }
        if tag == 2 {
            history += u64::from_le_bytes(bytes[offset + 4..offset + 12].try_into()?);
        }
        offset += len;
        if tag == 6 {
            break;
        }
    }
    assert!(history >= 1000, "{history} scrollback rows");
    eprintln!("MEASURE complete snapshot {offset} bytes, {history} history rows");
    Ok(())
});

case!(remote_slow, {
    let ep = endpoint("ssh://fixture/fixture/rex/rex.sock", Some(Ask::new(true, None)));
    let (ctrl, _) = connect(&ep, None).await?;
    let started = Instant::now();
    for _ in 0..3 {
        ctrl.server_status().await?;
    }
    assert!(started.elapsed() >= Duration::from_millis(550));
    let (sid, bid) = session(&ctrl).await?;
    let mut term = terminal(&ep, &ctrl, &sid, &bid).await?;
    ctrl.terminal_write(&sid, &bid, "printf '\\007'\n").await?;
    loop {
        let TerminalStreamFrame::Bytes(part) = term.next_frame().await? else { return Err("terminal closed on slow link".into()) };
        if part.contains(&7) {
            break;
        }
    }
    eprintln!("MEASURE netem 100 ms each direction, 2% loss: requests + session + terminal in {:.2?}", started.elapsed());
    Ok(())
});

fn children(pid: i32) -> Result<Vec<i32>> {
    Ok(fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))?.split_whitespace().map(str::parse).collect::<std::result::Result<_, _>>()?)
}

fn ssh_children() -> Result<Vec<i32>> {
    let parent: i32 = fs::read_to_string("/fixture/sshd.pid")?.trim().parse()?;
    let mut sessions = children(parent)?;
    for pid in sessions.clone() {
        sessions.extend(children(pid).unwrap_or_default());
    }
    assert!(!sessions.is_empty());
    Ok(sessions)
}

fn signal(pids: &[i32], sig: i32) -> Result {
    for &pid in pids.iter().rev() {
        assert!(pid > 1);
        if unsafe { libc::kill(pid, sig) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error.into());
            }
        }
    }
    Ok(())
}

async fn wait_for_event(events: &mut Events, matches: impl Fn(&ControlEvent) -> bool) -> Result<u64> {
    timeout(Duration::from_secs(3), async {
        while let Some(ev) = events.recv().await {
            if matches(&ev.event) {
                return Ok(ev.seq);
            }
        }
        Err("control stream closed before event".into())
    })
    .await?
}

async fn loss(kind: &str) -> Result {
    use std::io::Write;
    fs::OpenOptions::new().append(true).open("/fixture/home/.ssh/config")?.write_all(b" ServerAliveInterval 1\n ServerAliveCountMax 2\n")?;
    let ep = endpoint("ssh://fixture/fixture/rex/rex.sock", Some(Ask::new(true, None)));
    let mut restarted = None;
    let mut server_pid = fs::read_to_string("/fixture/rex.pid")?.trim().parse()?;
    for activity in ["idle", "streaming", "request"] {
        let (ctrl, mut events) = connect(&ep, None).await?;
        let (sid, bid) = session(&ctrl).await?;
        let mut term = terminal(&ep, &ctrl, &sid, &bid).await?;
        let TerminalStreamFrame::Bytes(first) = term.next_frame().await? else { return Err("missing snapshot".into()) };
        assert!(first.starts_with(b"GHOSTSNP"));
        let token = ctrl.reconnect_token().unwrap().to_owned();
        let mut pending = None;
        if activity == "streaming" {
            ctrl.terminal_write(&sid, &bid, "for i in 1 2 3 4 5 6; do printf '\\005'; sleep .1; done\n").await?;
            loop {
                let TerminalStreamFrame::Bytes(bytes) = term.next_frame().await? else { return Err("early close".into()) };
                if bytes.contains(&5) {
                    break;
                }
            }
        } else if activity == "request" {
            ctrl.client_keymap(&[KeymapAction { name: "wait".into(), kind: Some("client".into()), ..Default::default() }], &[], &[]).await?;
            let c = ctrl.clone();
            pending = Some(tokio::spawn(async move { c.call_raw("client.perform", Some(json!({"client_id":c.client_id(),"action":"wait","timeout_ms":30000}))).await }));
            wait_for_event(&mut events, |e| matches!(e, ControlEvent::ClientPerformRequested(_))).await?;
        }
        let ssh = ssh_children()?;
        let start = Instant::now();
        match kind {
            "sshd" => signal(&ssh, libc::SIGKILL)?,
            "blackhole" => signal(&ssh, libc::SIGSTOP)?,
            "rex" => signal(&[server_pid], libc::SIGKILL)?,
            _ => unreachable!(),
        }
        timeout(Duration::from_secs(7), async { while events.recv().await.is_some() {} }).await?;
        assert!(events.recv().await.is_none());
        if let Some(waiter) = pending {
            assert!(matches!(waiter.await?, Err(RexError::Closed(_))));
        }
        loop {
            match timeout(Duration::from_secs(2), term.next_frame()).await? {
                Ok(TerminalStreamFrame::Closed { .. }) | Err(_) => break,
                Ok(TerminalStreamFrame::Bytes(_)) => {}
            }
        }
        eprintln!("MEASURE {kind}/{activity}: noticed in {:.2?}", start.elapsed());
        let offset = term.bytes_received;
        let data_token = term.welcome.reconnect_token;
        drop(term);
        if kind == "blackhole" {
            signal(&ssh, libc::SIGCONT)?;
        }
        if kind == "rex" {
            if let Some(mut child) = restarted.take() {
                std::process::Child::wait(&mut child)?;
            }
            let addr = "unix:///fixture/rex/rex.sock";
            let child = Command::new("/usr/bin/rex")
                .args(["-S", addr, "server", "run", "--listen", addr, "--state-dir", "/fixture/rex/state", "--restore=false"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()?;
            server_pid = child.id() as i32;
            restarted = Some(child);
            timeout(Duration::from_secs(4), async {
                while tokio::net::UnixStream::connect("/fixture/rex/rex.sock").await.is_err() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await?;
        }
        ctrl.disconnected().await;
        assert!(ctrl.same_connection(&ctrl.clone()));
        let (again, _) = connect(&ep, Some(&token)).await?;
        assert!(!ctrl.same_connection(&again));
        let resumed = again.welcome.client.as_ref().unwrap().resumed;
        let boot = again.session_bootstrap("all", None, None, &[]).await?.payload;
        if kind == "rex" {
            assert!(!resumed && boot.attach_all.sessions.is_empty());
        } else {
            assert!(resumed, "control token failed for {kind}/{activity}");
            assert!(boot.attach_all.sessions.iter().any(|v| v.session_id == sid));
            let (stream, host) = ep.connect_stream().await?;
            let mut resumed_stream = match TerminalDataStream::connect_resume(stream, &host, &data_token, offset).await {
                Ok(stream) => {
                    assert_eq!(stream.bytes_received, offset);
                    eprintln!("MEASURE {kind}/{activity}: resumed terminal at {offset}");
                    stream
                }
                Err(RexError::DataRejected(reason)) => {
                    eprintln!("MEASURE {kind}/{activity}: fresh terminal after {reason:?}");
                    terminal(&ep, &again, &sid, &bid).await?
                }
                Err(e) => return Err(e.into()),
            };
            again.terminal_write(&sid, &bid, "printf '\\006'\n").await?;
            loop {
                let TerminalStreamFrame::Bytes(bytes) = resumed_stream.next_frame().await? else { return Err("resumed stream closed".into()) };
                if bytes.contains(&6) {
                    break;
                }
            }
        }
        again.close();
    }
    if let Some(mut child) = restarted {
        child.kill()?;
        child.wait()?;
    }
    Ok(())
}

case!(remote_sshd_loss, loss("sshd").await);
case!(remote_rex_loss, loss("rex").await);
case!(remote_blackhole, loss("blackhole").await);

case!(https_carrier, {
    version(&endpoint("https://localhost:9443", None)).await?;
    let err = connect(&endpoint("https://127.0.0.1:9443", None), None).await.err().ok_or("certificate accepted for the wrong name")?.to_string();
    assert!(err.contains("TLS handshake"), "{err}");
    Ok(())
});

case!(remote_resources, {
    let ep = endpoint("proxied", Some(Ask::new(true, None)));
    let runtime = tokio::runtime::Handle::current();
    let fds = || fs::read_dir("/proc/self/fd").unwrap().count();
    let tasks = || runtime.metrics().num_alive_tasks();
    let pid = std::process::id();
    let children_of_client = || children(pid as i32).unwrap().len();
    // Warm up Tokio's process driver (SIGCHLD uses a process-wide fd).
    let mut warmup = tokio::process::Command::new("/bin/true").spawn()?;
    warmup.wait().await?;
    let before = (fds(), tasks(), children_of_client());
    for _ in 0..20 {
        let (ctrl, mut events) = connect(&ep, None).await?;
        ctrl.server_status().await?;
        signal(&ssh_children()?, libc::SIGKILL)?;
        timeout(Duration::from_secs(2), async { while events.recv().await.is_some() {} }).await?;
        timeout(Duration::from_secs(2), async {
            while tasks() > before.1 || children_of_client() > before.2 || fds() > before.0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| format!("after close, fd/task/child {:?} vs {before:?}", (fds(), tasks(), children_of_client())))?;
    }
    eprintln!("MEASURE 20 forced ProxyCommand reconnects, fd/task/child {before:?} -> {:?}", (fds(), tasks(), children_of_client()));
    Ok(())
});

case!(remote_partial_transfers, {
    let ep = endpoint("ssh://fixture/fixture/rex/rex.sock", Some(Ask::new(true, None)));
    let (ctrl, _) = connect(&ep, None).await?;
    let (sid, bid) = session(&ctrl).await?;
    const SIZE: u64 = 100 * 1024 * 1024;
    let mut partial = vec![0; CHUNK];
    sample_chunk(&mut partial, 0);
    for cut in [false, true] {
        let name = if cut { "broken.bin" } else { "cancelled.bin" };
        let u = ctrl.terminal_upload_begin(&sid, &bid, &format!("upload:{}", uuid::Uuid::new_v4()), name, SIZE, None).await?.payload;
        let mut upload = open_bytes_stream(&ep, &u.ticket).await?;
        for _ in 0..SIZE as usize / 2 / CHUNK {
            upload.write_all(&partial).await?;
        }
        if cut {
            signal(&ssh_children()?, libc::SIGKILL)?;
            assert!(timeout(Duration::from_secs(3), upload.read_chunk()).await?.is_err());
        } else {
            // Close the WebSocket before the advertised length/EOF.
            upload.into_inner().close(None).await?;
        }
        if let Some(path) = uploaded_file(Path::new("/fixture"), name) {
            assert!(path.metadata()?.len() < SIZE);
        }
    }

    let (ctrl, _) = connect(&ep, None).await?;
    ctrl.session_attach(&sid).await?;
    let screen = ctrl.terminal_format(&sid, &bid, "text").await?.payload;
    assert!(!screen.contains("cancelled.bin") && !screen.contains("broken.bin"), "partial upload was delivered to the terminal");
    let source = fs::File::create("/fixture/large.bin")?;
    source.set_len(SIZE)?;
    for cut in [false, true] {
        let info = ctrl.terminal_download_begin(&sid, &bid, &format!("download:{}", uuid::Uuid::new_v4()), "/fixture/large.bin", None).await?.payload;
        let mut stream = open_bytes_stream(&ep, info.ticket.as_deref().unwrap()).await?;
        let mut got = 0;
        while got < info.size / 2 {
            got += stream.read_chunk().await?.ok_or("early EOF")?.len() as u64;
        }
        if cut {
            signal(&ssh_children()?, libc::SIGKILL)?;
            loop {
                match timeout(Duration::from_secs(3), stream.read_chunk()).await? {
                    Ok(Some(part)) => got += part.len() as u64,
                    Ok(None) => return Err("cut download marked complete".into()),
                    Err(_) => break,
                }
            }
        } else {
            stream.into_inner().close(None).await?;
        }
        assert!(got < info.size);
    }
    Ok(())
});

case!(ssh_stalled_agent, {
    let path = Path::new("/fixture/stalled-agent.sock");
    let _listener = tokio::net::UnixListener::bind(path)?;
    let status = timeout(Duration::from_secs(6), inspect_ssh_agent(Some(path))).await?;
    assert!(status.error.unwrap().contains("timed out"));
    Ok(())
});

case!(remote_sequence_and_input, {
    let ep = endpoint("ssh://fixture/fixture/rex/rex.sock", Some(Ask::new(true, None)));
    let (ctrl, mut events) = connect(&ep, None).await?;
    let (sid, bid) = session(&ctrl).await?;
    let (observer, _) = connect(&endpoint("unix:///fixture/rex/rex.sock", None), None).await?;
    observer.session_attach(&sid).await?;
    let mut expected = String::new();
    let mut ctrl = ctrl;
    for round in 0..2 {
        let mut seqs = Vec::new();
        for i in 0..4 {
            let text = format!("{round}-{i}");
            ctrl.terminal_write(&sid, &bid, &format!("echo {text} >> /fixture/keys\n")).await?;
            expected.push_str(&text);
            expected.push('\n');
            ctrl.session_set_label(&sid, &text).await?;
            seqs.push(wait_for_event(&mut events, |ev| matches!(ev,ControlEvent::SessionLabelChanged{label,..} if label==&text)).await?);
        }
        assert!(seqs.windows(2).all(|pair| pair[0] < pair[1]));
        let token = ctrl.reconnect_token().unwrap().to_owned();
        signal(&ssh_children()?, libc::SIGKILL)?;
        while events.recv().await.is_some() {}
        observer.session_set_label(&sid, "during outage").await?;
        (ctrl, events) = connect(&ep, Some(&token)).await?;
        let refresh = ctrl.session_bootstrap("all", Some(&sid), None, &[]).await?;
        assert_eq!(refresh.payload.selected_session.unwrap().session.label, "during outage");
        timeout(Duration::from_secs(2), async {
            while fs::read_to_string("/fixture/keys").ok().as_ref() != Some(&expected) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
    }
    Ok(())
});

case!(ssh_shared_authentication, {
    let ask = Ask::new(true, None);
    let ep = endpoint("ssh://fixture/fixture/rex/rex.sock", Some(ask.clone()));
    let mut connections = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let ep = ep.clone();
        connections.spawn(async move { version(&ep).await.map_err(|e| e.to_string()) });
    }
    while let Some(result) = connections.join_next().await {
        result??;
    }
    assert_eq!(*ask.keys.lock().unwrap(), [false]);
    let log = fs::read_to_string("/fixture/sshd.log")?;
    assert_eq!(log.matches("Accepted publickey for scratch").count(), 1);
    Ok(())
});

case!(ssh_prompt_time, {
    let mut ask = Ask::new(true, None);
    Arc::get_mut(&mut ask).unwrap().key_prompt_delay = Duration::from_millis(2300);
    version(&endpoint("fixture", Some(ask))).await
});
