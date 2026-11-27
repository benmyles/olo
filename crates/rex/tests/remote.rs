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

fn find_screen_helper(exe: &Path) -> Option<std::path::PathBuf> {
    olo_rex_client::quic::find_test_helper(exe)
}

fn outer(case: &str) -> bool {
    let exe = std::env::current_exe().unwrap();
    if exe == Path::new("/suite") {
        assert_eq!(std::env::var("REX_SERVER_SOCKET").unwrap(), "/fixture/rex/rex.sock");
        return false;
    }
    let mut cmd = Command::new("/usr/bin/python3");
    cmd.args(["-I", concat!(env!("CARGO_MANIFEST_DIR"), "/tests/remote/fixture.py")]).arg(&exe).arg(case).env_remove("REX_SESSION").env_remove("REX_BLOCK").env_remove("SSH_AUTH_SOCK");
    if case.starts_with("ssh_screen") {
        if let Some(helper) = find_screen_helper(&exe) {
            cmd.env("OLO_REX_LINK_BIN", helper);
        } else {
            cmd.env_remove("OLO_REX_LINK_BIN");
        }
    }
    let result = cmd.status().unwrap();
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
    // (`rex` as an SSH command finds it there: on its path, else in
    // ~/.local/bin, where Rex's own installer puts an account's.)
    let dialled = fs::read_to_string("/fixture/dial-calls")?;
    assert!(dialled.trim().starts_with("sh -c 'R=$(command -v rex 2> /dev/null) || R=\"$HOME\"/.local/bin/rex; exec \"$R\" __internal dial-stdio"), "{dialled}");
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

/// Recursively collect descendants of `root_pid` inside this private PID
/// namespace via `/proc/<pid>/task/<pid>/children` (never `pgrep -f`).
fn descendant_with_args(root_pid: i32, needles: &[&str]) -> Option<i32> {
    let mut stack = vec![root_pid];
    while let Some(pid) = stack.pop() {
        let Ok(kids) = children(pid) else { continue };
        for kid in kids {
            if let Ok(cmdline) = fs::read(format!("/proc/{kid}/cmdline")) {
                let argv: Vec<&[u8]> = cmdline.split(|&b| b == 0).filter(|s| !s.is_empty()).collect();
                if needles.iter().all(|n| argv.contains(&n.as_bytes())) {
                    return Some(kid);
                }
            }
            stack.push(kid);
        }
    }
    None
}

struct PidFd(i32);

impl PidFd {
    fn open(pid: i32) -> Option<Self> {
        // SAFETY: `pidfd_open(pid, 0)` returns a new fd or -1.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0 as libc::c_uint) as i32 };
        (fd >= 0).then_some(Self(fd))
    }

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

async fn wait_pidfd(pidfd: PidFd, deadline: Instant) -> Option<Instant> {
    let tok_deadline = tokio::time::Instant::from_std(deadline);
    tokio::time::timeout_at(tok_deadline, tokio::task::spawn_blocking(move || pidfd.wait_exit_until(deadline))).await.ok()?.ok().flatten()
}

/// Stand-in `olo-session` compositor inside the fixture namespace so
/// `olo-rex-link screen stream qa` can run end-to-end over real `sshd` and QUIC.
#[test]
#[ignore]
fn fake_screen_session() {
    use std::io::{BufRead as _, BufReader as StdBufReader, Write as _};
    use std::os::unix::net::UnixListener;

    let sock = std::path::PathBuf::from(std::env::var("OLO_FAKE_SOCK").expect("OLO_FAKE_SOCK"));
    let _ = fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).expect("fake screen bind");
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
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
                        }
                        Some("screenshot") => {
                            if let Some(path) = req["path"].as_str() {
                                let w: u32 = std::env::var("OLO_FAKE_W").ok().and_then(|s| s.parse().ok()).unwrap_or(128);
                                let h: u32 = std::env::var("OLO_FAKE_H").ok().and_then(|s| s.parse().ok()).unwrap_or(64);
                                let pixels = (w * h * 4) as usize;
                                let mut raw = Vec::with_capacity(8 + pixels);
                                raw.extend_from_slice(&w.to_le_bytes());
                                raw.extend_from_slice(&h.to_le_bytes());
                                if w > 128 {
                                    // High-entropy pixels so compressed OLOT keyframe exceeds
                                    // russh's 100-packet (~3.2 MiB) channel buffer in 1 frame.
                                    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
                                    for _ in 0..pixels {
                                        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                                        raw.push((state >> 56) as u8);
                                    }
                                } else {
                                    raw.resize(8 + pixels, 0xaa);
                                }
                                let tmp = format!("{path}.part");
                                let _ = fs::write(&tmp, &raw);
                                let _ = fs::rename(&tmp, path);
                                let _ = write_s.write_all(b"{\"type\":\"ok\"}\n");
                                if let Ok(marker) = std::env::var("OLO_FAKE_SERVED_MARKER") {
                                    let _ = fs::write(marker, b"served\n");
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
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    let _ = fs::remove_file(&sock);
}

struct FixtureScreen {
    child: std::process::Child,
    _stdin: std::process::ChildStdin,
}

impl FixtureScreen {
    fn start(name: &str) -> Self {
        Self::start_sized(name, 128, 64)
    }

    fn start_sized(name: &str, w: u32, h: u32) -> Self {
        Self::start_sized_with_marker(name, w, h, None)
    }

    fn start_sized_with_marker(name: &str, w: u32, h: u32, served_marker: Option<&Path>) -> Self {
        use std::io::{BufRead as _, BufReader as StdBufReader};
        use std::os::unix::process::CommandExt as _;
        let rt = Path::new("/fixture/run/olo-screens");
        fs::create_dir_all(rt).unwrap();
        let sock = rt.join(format!("{name}.sock"));
        let mut cmd = Command::new("/suite");
        cmd.arg0("olo-session")
            .args(["--ignored", "--exact", "fake_screen_session", "--nocapture"])
            .env("OLO_SCREEN", name)
            .env("OLO_FAKE_SOCK", &sock)
            .env("OLO_FAKE_W", w.to_string())
            .env("OLO_FAKE_H", h.to_string())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        if let Some(marker) = served_marker {
            cmd.env("OLO_FAKE_SERVED_MARKER", marker);
        }
        let mut child = cmd.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut stdout = StdBufReader::new(child.stdout.take().unwrap());
        let mut ready = String::new();
        loop {
            ready.clear();
            let n = stdout.read_line(&mut ready).unwrap();
            assert!(n > 0, "fixture fake_screen_session exited before announcing ready");
            if ready.trim() == "ready" {
                break;
            }
        }
        let meta = json!({
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
        Self { child, _stdin: stdin }
    }
}

impl Drop for FixtureScreen {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Read one bounded binary `ServerMsg` (uses wire crate's
/// `max_server_payload_for`, reads incrementally — Test Finding 5).
async fn read_one_server_msg<R: tokio::io::AsyncRead + Unpin>(r: &mut R) -> Result<olo_rex_link::screen_wire::ServerMsg> {
    let mut head = [0u8; 5];
    timeout(Duration::from_secs(5), r.read_exact(&mut head)).await??;
    let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]);
    let max_len = olo_rex_link::screen_wire::max_server_payload_for(head[0])?;
    if len > max_len {
        return Err(format!("server msg 0x{:02x} length {len} exceeds {max_len}", head[0]).into());
    }
    let mut payload = Vec::with_capacity((len as usize).min(64 * 1024));
    let mut remaining = len as usize;
    let mut scratch = [0u8; 16 * 1024];
    while remaining > 0 {
        let want = remaining.min(scratch.len());
        let n = timeout(Duration::from_secs(5), r.read(&mut scratch[..want])).await??;
        if n == 0 {
            return Err("truncated server msg payload".into());
        }
        payload.extend_from_slice(&scratch[..n]);
        remaining -= n;
    }
    Ok(olo_rex_link::screen_wire::decode_server_msg(head[0], &payload)?)
}

/// Read one bounded binary `HistoryMsg` (uses wire crate's
/// `max_history_payload_for`, reads incrementally — Test Finding 5).
async fn read_one_history_msg<R: tokio::io::AsyncRead + Unpin>(r: &mut R) -> Result<(u8, olo_rex_link::screen_wire::HistoryMsg)> {
    let mut head = [0u8; 5];
    timeout(Duration::from_secs(5), r.read_exact(&mut head)).await??;
    let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]);
    let max_len = olo_rex_link::screen_wire::max_history_payload_for(head[0])?;
    if len > max_len {
        return Err(format!("history msg 0x{:02x} length {len} exceeds {max_len}", head[0]).into());
    }
    let mut payload = Vec::with_capacity((len as usize).min(64 * 1024));
    let mut remaining = len as usize;
    let mut scratch = [0u8; 16 * 1024];
    while remaining > 0 {
        let want = remaining.min(scratch.len());
        let n = timeout(Duration::from_secs(5), r.read(&mut scratch[..want])).await??;
        if n == 0 {
            return Err("truncated history msg payload".into());
        }
        payload.extend_from_slice(&scratch[..n]);
        remaining -= n;
    }
    Ok((head[0], olo_rex_link::screen_wire::decode_history_msg(head[0], &payload)?))
}

/// Read one bounded follow NDJSON line (`<= MAX_JSON_LINE`, Test Finding 5).
async fn read_bounded_follow_line<R: tokio::io::AsyncBufRead + Unpin>(r: &mut R) -> Result<String> {
    use tokio::io::AsyncBufReadExt as _;
    let max = olo_rex_link::screen_wire::MAX_JSON_LINE;
    let mut line = String::new();
    let mut limited = r.take((max + 1) as u64);
    let n = timeout(Duration::from_secs(5), limited.read_line(&mut line)).await??;
    if n > max {
        return Err("follow line exceeded MAX_JSON_LINE".into());
    }
    Ok(line)
}

/// Open a FIFO with `O_RDONLY | O_NONBLOCK` and read one PID line asynchronously
/// under `within`, never stranding a blocking worker thread if the writer fails
/// before opening the FIFO (Test Finding 9).
async fn read_pid_from_fifo_async(path: &Path, within: Duration) -> Result<i32> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let file = fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path)?;
    // SAFETY: `file` is exclusively owned by `AsyncFd` and remains open for its lifetime.
    let afd = unsafe { tokio::io::unix::AsyncFd::register(file)? };
    let read_fut = async {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 64];
        loop {
            let mut guard = afd.readable().await?;
            match guard.try_io(|inner| {
                let raw = std::os::fd::AsRawFd::as_raw_fd(inner.get_ref());
                // SAFETY: valid owned fd and writable buffer.
                let n = unsafe { libc::read(raw, chunk.as_mut_ptr().cast(), chunk.len()) };
                if n < 0 { Err(std::io::Error::last_os_error()) } else { Ok(n as usize) }
            }) {
                Ok(Ok(0)) => {
                    if !buf.is_empty() {
                        break;
                    }
                    // No writer has opened the FIFO yet (Linux reports EOF/POLLHUP
                    // on O_NONBLOCK O_RDONLY until a writer attaches). Yield briefly.
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                Ok(Ok(n)) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.contains(&b'\n') || buf.len() >= 32 {
                        break;
                    }
                }
                Ok(Err(e)) => return Err(e.into()),
                Err(_would_block) => continue,
            }
        }
        let s = String::from_utf8_lossy(&buf);
        let pid: i32 = s.trim().parse()?;
        Ok::<i32, Box<dyn std::error::Error>>(pid)
    };
    timeout(within, read_fut).await?
}

case!(ssh_screen_fallback_protocol_follow_and_drop, {
    use olo_rex_link::screen_wire::{self, FollowEvent, HistoryMsg, PROTO, ServerMsg};
    use tokio::io::BufReader;

    let sshd_pid: i32 = fs::read_to_string("/fixture/sshd.pid")?.trim().parse()?;
    let pool = Arc::new(SshSessionPool::default());
    pool.set_helper(Some("test"));
    let target = SshTarget::parse("fixture")?;
    let ask: Option<Arc<dyn SshAskpass>> = Some(Ask::new(true, None));

    // 1. `--protocol` over SSH fallback matches `olo-rex-link screen --protocol`.
    let expected_proto = Command::new("/fixture/olo-rex-link").args(["screen", "--protocol"]).env("OLO_HOST_DESKTOP", "/fixture/desktop").output()?;
    assert!(expected_proto.status.success());
    let mut proto_stream = pool.open_screen_stream(&target, ask.clone(), &["--protocol".to_owned()]).await?;
    assert!(!pool.linked(&target).await, "OLO_REX_QUIC=no must use SSH fallback");
    let mut proto_out = Vec::new();
    timeout(Duration::from_secs(5), proto_stream.read_to_end(&mut proto_out)).await??;
    assert_eq!(proto_out, expected_proto.stdout);

    // 2. `follow` over SSH fallback emits initial Snapshot, and dropping the
    //    stream (from a non-Tokio thread) finishes drop + child exit within one
    //    absolute 1s deadline (Test Finding 8).
    let follow_stream = pool.open_screen_stream(&target, ask.clone(), &["follow".to_owned()]).await?;
    let mut follow_reader = BufReader::new(follow_stream);
    let first_line = read_bounded_follow_line(&mut follow_reader).await?;
    assert!(!first_line.is_empty());
    let ev = screen_wire::read_follow_event(&mut first_line.as_bytes())?.expect("follow snapshot");
    assert!(matches!(ev, FollowEvent::Snapshot { proto: PROTO, .. }), "got {ev:?}");

    let follow_pid = descendant_with_args(sshd_pid, &["screen", "follow"]).expect("follow process under sshd");
    let pidfd = PidFd::open(follow_pid).expect("pidfd follow");
    let t0 = Instant::now();
    let deadline = t0 + Duration::from_secs(1);
    std::thread::spawn(move || drop(follow_reader)).join().unwrap();
    wait_pidfd(pidfd, deadline).await.expect("SSH follow helper did not exit within 1s of drop");
    eprintln!("MEASURE ssh fallback drop `follow` exited in {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);

    // 3. Live `stream qa` over SSH fallback delivers initial frame, and drop
    //    ends the host helper within one absolute 1s deadline.
    let _fake = FixtureScreen::start("qa");
    let mut live_stream = pool.open_screen_stream(&target, ask.clone(), &["stream".to_owned(), "qa".to_owned()]).await?;
    let first_msg = read_one_server_msg(&mut live_stream).await?;
    assert!(matches!(first_msg, ServerMsg::Frame { seq: 1, .. }), "got {first_msg:?}");
    let stream_pid = descendant_with_args(sshd_pid, &["screen", "stream", "qa"]).expect("stream process under sshd");
    let pidfd = PidFd::open(stream_pid).expect("pidfd stream");
    let t0 = Instant::now();
    let deadline = t0 + Duration::from_secs(1);
    drop(live_stream);
    wait_pidfd(pidfd, deadline).await.expect("SSH stream helper did not exit within 1s of drop");
    eprintln!("MEASURE ssh fallback drop `stream qa` exited in {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);

    // 4. `history qa` over SSH fallback maps to `history-stream qa` (matching QUIC)
    //    and yields Actions + Index + SnapshotDone (bounded incremental reader).
    let mut hist_stream = pool.open_screen_stream(&target, ask.clone(), &["history".to_owned(), "qa".to_owned()]).await?;
    for expected_tag in [screen_wire::MSG_HISTORY_ACTIONS, screen_wire::MSG_HISTORY_INDEX, screen_wire::MSG_HISTORY_SNAPSHOT_DONE] {
        let (tag, msg) = read_one_history_msg(&mut hist_stream).await?;
        assert_eq!(tag, expected_tag);
        if expected_tag == screen_wire::MSG_HISTORY_SNAPSHOT_DONE {
            assert!(matches!(msg, HistoryMsg::SnapshotDone { .. }));
        }
    }
    let hist_pid = descendant_with_args(sshd_pid, &["screen", "history-stream", "qa"]).expect("history-stream process under sshd");
    let pidfd = PidFd::open(hist_pid).expect("pidfd history");
    let t0 = Instant::now();
    let deadline = t0 + Duration::from_secs(1);
    drop(hist_stream);
    wait_pidfd(pidfd, deadline).await.expect("SSH history-stream helper did not exit within 1s of drop");

    // 4b. Explicit `shutdown()` followed by drop leaves sibling stream intact.
    let mut sibling = pool.exec_io(&target, ask.clone(), "exec cat").await?;
    let mut graceful = pool.open_screen_stream(&target, ask.clone(), &["follow".to_owned()]).await?;
    let mut g_reader = BufReader::new(&mut graceful);
    assert!(!read_bounded_follow_line(&mut g_reader).await?.is_empty());
    drop(g_reader);
    graceful.shutdown().await?;
    graceful.shutdown().await?;
    let mut tail = Vec::new();
    timeout(Duration::from_secs(5), graceful.read_to_end(&mut tail)).await??;
    assert!(tail.is_empty());
    drop(graceful);
    sibling.write_all(b"alive").await?;
    let mut sb = [0u8; 5];
    timeout(Duration::from_secs(5), sibling.read_exact(&mut sb)).await??;
    assert_eq!(&sb, b"alive");
    drop(sibling);

    // 5. Missing helper surfaces a clear error (not "dial-stdio").
    fs::remove_dir_all("/fixture/home/.local/share/olo/rex-link")?;
    let mut missing = pool.open_screen_stream(&target, ask, &["follow".to_owned()]).await?;
    let mut buf = [0u8; 16];
    let err = timeout(Duration::from_secs(5), missing.read(&mut buf)).await?.unwrap_err().to_string();
    assert!(err.contains("screens-capable"), "expected screens helper error, got: {err}");
    Ok(())
});

case!(ssh_screen_over_quic_link, {
    use olo_rex_link::screen_wire::{self, FollowEvent, PROTO, ServerMsg};
    use tokio::io::BufReader;

    let sshd_pid: i32 = fs::read_to_string("/fixture/sshd.pid")?.trim().parse()?;
    let pool = Arc::new(SshSessionPool::default());
    pool.set_helper(Some("test"));
    let target = SshTarget::parse("fixture")?;
    let ask: Option<Arc<dyn SshAskpass>> = Some(Ask::new(true, None));

    // `OLO_REX_QUIC=yes`: `open_screen_stream(["follow"])` brings up the QUIC
    // link via sshd and opens the stream over QUIC (child MUST descend from the
    // `olo-rex-link quic` keeper PID, not directly from sshd — Test Finding 3).
    let follow_stream = pool.open_screen_stream(&target, ask.clone(), &["follow".to_owned()]).await?;
    assert!(pool.linked(&target).await, "expected QUIC link to be up");
    let keeper_pid = descendant_with_args(sshd_pid, &["quic", "--"]).expect("QUIC keeper under sshd");
    let mut follow_reader = BufReader::new(follow_stream);
    let first_line = read_bounded_follow_line(&mut follow_reader).await?;
    assert!(!first_line.is_empty());
    let ev = screen_wire::read_follow_event(&mut first_line.as_bytes())?.expect("follow snapshot");
    assert!(matches!(ev, FollowEvent::Snapshot { proto: PROTO, .. }), "got {ev:?}");

    let follow_pid = descendant_with_args(keeper_pid, &["screen", "follow"]).expect("follow child MUST descend from QUIC keeper");
    let pidfd = PidFd::open(follow_pid).expect("pidfd quic follow");
    let t0 = Instant::now();
    let deadline = t0 + Duration::from_secs(1);
    drop(follow_reader);
    wait_pidfd(pidfd, deadline).await.expect("QUIC follow child did not exit within 1s of drop");
    eprintln!("MEASURE ssh+quic drop `follow` exited in {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);

    // `--protocol` is routed over SSH explicitly, returns exact CLI output, and
    // NEVER poisons the QUIC link.
    let expected_proto = Command::new("/fixture/olo-rex-link").args(["screen", "--protocol"]).env("OLO_HOST_DESKTOP", "/fixture/desktop").output()?;
    assert!(expected_proto.status.success());
    let mut proto_stream = pool.open_screen_stream(&target, ask.clone(), &["--protocol".to_owned()]).await?;
    let mut proto_out = Vec::new();
    timeout(Duration::from_secs(5), proto_stream.read_to_end(&mut proto_out)).await??;
    assert_eq!(proto_out, expected_proto.stdout);
    assert!(pool.linked(&target).await, "`--protocol` must not mark the QUIC link down");

    // Finding 5: a valid 256-range export whose JSON exceeds QUIC's 4,096-byte
    // selector limit MUST NOT take the healthy UDP link out of use!
    let mut huge_parts = String::new();
    for i in 0..256u32 {
        if i > 0 {
            huge_parts.push(',');
        }
        let a = 1_000_000 + i * 2;
        huge_parts.push_str(&format!("{a}-{}", a + 1));
    }
    let huge_export = vec!["export".to_owned(), "qa".to_owned(), "--part".to_owned(), huge_parts];
    assert!(screen_wire::validate_quic_screen_args(&huge_export).is_ok(), "must be syntactically valid");
    assert!(screen_wire::encode_quic_screen_args(&huge_export).is_err(), "must exceed 4096-byte QUIC selector cap");
    let huge_stream = pool.open_screen_stream(&target, ask.clone(), &huge_export).await?;
    drop(huge_stream);
    assert!(pool.linked(&target).await, "valid oversized export MUST NOT demote healthy QUIC link!");

    // Live `stream qa` over the live QUIC link through `SshSessionPool` (descends from keeper).
    let _fake = FixtureScreen::start("qa");
    let mut live_stream = pool.open_screen_stream(&target, ask.clone(), &["stream".to_owned(), "qa".to_owned()]).await?;
    let first_msg = read_one_server_msg(&mut live_stream).await?;
    assert!(matches!(first_msg, ServerMsg::Frame { seq: 1, .. }), "got {first_msg:?}");
    let stream_pid = descendant_with_args(keeper_pid, &["screen", "stream", "qa"]).expect("stream child MUST descend from QUIC keeper");
    let pidfd = PidFd::open(stream_pid).expect("pidfd quic stream");
    let t0 = Instant::now();
    let deadline = t0 + Duration::from_secs(1);
    drop(live_stream);
    wait_pidfd(pidfd, deadline).await.expect("QUIC stream child did not exit within 1s of drop");
    eprintln!("MEASURE ssh+quic drop `stream qa` exited in {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);

    // Finding 6: discarding a dead QUIC link sends SSH EOF + CLOSE on its
    // keeper channel so the host `olo-rex-link quic` process exits within 1s
    // while the original SSH connection stays alive and usable!
    let mut sibling = pool.exec_io(&target, ask, "exec cat").await?;
    let keeper_fd = PidFd::open(keeper_pid).expect("pidfd quic keeper");
    let t0 = Instant::now();
    let deadline = t0 + Duration::from_secs(1);
    pool.discard_dead_link_for_test(&target).await;
    assert!(!pool.linked(&target).await);
    wait_pidfd(keeper_fd, deadline).await.expect("discarded QUIC keeper process stayed stranded on host!");
    eprintln!("MEASURE finding6 discarded QUIC keeper exited in {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
    // Original SSH connection is still live and usable!
    sibling.write_all(b"still-up").await?;
    let mut sb = [0u8; 8];
    timeout(Duration::from_secs(5), sibling.read_exact(&mut sb)).await??;
    assert_eq!(&sb, b"still-up");
    drop(sibling);
    Ok(())
});

case!(ssh_exec_endings, {
    let pool = Arc::new(SshSessionPool::default());
    let target = SshTarget::parse("fixture")?;
    let ask: Option<Arc<dyn SshAskpass>> = Some(Ask::new(true, None));

    // 1. A command that prints nothing and exits 0 reads as a clean end (EOF).
    let mut empty = pool.exec_io(&target, ask.clone(), "true").await?;
    let mut out = Vec::new();
    timeout(Duration::from_secs(5), empty.read_to_end(&mut out)).await??;
    assert!(out.is_empty(), "expected clean 0-byte EOF, got {out:?}");

    // 2. A command that exits 3 having written to stderr reads as an error
    //    carrying both its stderr words and its exit status.
    let mut failed = pool.exec_io(&target, ask.clone(), "echo 'no room left on host' >&2; exit 3").await?;
    let mut buf = Vec::new();
    let err = timeout(Duration::from_secs(5), failed.read_to_end(&mut buf)).await?.expect_err("exit 3 must fail").to_string();
    assert!(err.contains('3') && err.contains("no room left on host"), "expected status 3 and stderr words, got: {err}");

    // 3. A stream dropped BEFORE its first stdout byte (command still starting
    //    and silent) ends the remote process within one absolute 1s deadline
    //    (nonblocking async FIFO read — Test Findings 8 & 9).
    let fifo = Path::new("/fixture/silent.fifo");
    let _ = fs::remove_file(fifo);
    let c_fifo = std::ffi::CString::new(fifo.to_str().unwrap())?;
    assert_eq!(unsafe { libc::mkfifo(c_fifo.as_ptr(), 0o600) }, 0, "mkfifo failed: {}", std::io::Error::last_os_error());
    let silent_stream = pool.exec_io(&target, ask, "echo $$ > /fixture/silent.fifo; exec cat").await?;
    let remote_pid = read_pid_from_fifo_async(fifo, Duration::from_secs(5)).await?;
    let pidfd = PidFd::open(remote_pid).expect("pidfd_open silent remote process");
    let t0 = Instant::now();
    let deadline = t0 + Duration::from_secs(1);
    std::thread::spawn(move || drop(silent_stream)).join().unwrap();
    wait_pidfd(pidfd, deadline).await.expect("silent remote process did not exit within 1s of pre-first-byte drop");
    eprintln!("MEASURE ssh drop before first byte exited in {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
    Ok(())
});

// Finding 9: open a pooled SSH connection on runtime A, open a silent command
// on a short-lived runtime B using the SAME pool, shut down runtime B
// completely, then drop the stream from a plain OS thread -> teardown runs on
// runtime A (the SSH driver's runtime), remote `cat` exits within 1s, and the
// connection holds 0 leaked channels.
case!(ssh_r3_drop_after_opener_runtime_shutdown, {
    let pool = Arc::new(SshSessionPool::default());
    let target = SshTarget::parse("fixture")?;
    let ask: Option<Arc<dyn SshAskpass>> = Some(Ask::new(true, None));

    // Establish the pooled SSH connection on Runtime A (the test's current runtime).
    pool.reach(&target, ask.clone()).await?;

    let fifo = Path::new("/fixture/rt9.fifo");
    let _ = fs::remove_file(fifo);
    let c_fifo = std::ffi::CString::new(fifo.to_str().unwrap())?;
    assert_eq!(unsafe { libc::mkfifo(c_fifo.as_ptr(), 0o600) }, 0);

    // Open the stream on a completely separate short-lived Runtime B on another thread
    // (using `spawn_blocking` so Runtime A keeps driving its SSH connection!),
    // return the stream, and SHUT DOWN Runtime B before dropping the stream!
    let (pool_b, target_b, ask_b) = (pool.clone(), target.clone(), ask.clone());
    let stream = tokio::task::spawn_blocking(move || {
        let rt_b = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let s = rt_b.block_on(pool_b.exec_io(&target_b, ask_b, "echo $$ > /fixture/rt9.fifo; exec cat")).unwrap();
        // Shut down Runtime B completely while `s` is still alive!
        drop(rt_b);
        s
    })
    .await?;

    let remote_pid = read_pid_from_fifo_async(fifo, Duration::from_secs(5)).await?;
    let pidfd = PidFd::open(remote_pid).expect("pidfd rt9");
    let t0 = Instant::now();
    let deadline = t0 + Duration::from_secs(1);
    std::thread::spawn(move || drop(stream)).join().unwrap();
    wait_pidfd(pidfd, deadline).await.expect("remote process did not exit after opener runtime B shut down!");
    eprintln!("MEASURE finding9 drop after opener runtime shutdown exited in {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);

    // Verify 0 channels leaked on Runtime A's SSH connection.
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut counts = pool.open_channel_count(&target).await?;
    while counts != (0, 0) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(15)).await;
        counts = pool.open_channel_count(&target).await?;
    }
    assert_eq!(counts, (0, 0), "leaked channels after drop on dead opener runtime: {counts:?}");
    Ok(())
});

#[tokio::test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
async fn ssh_bench_throughput() -> Result {
    if outer("ssh_bench_throughput") {
        return Ok(());
    }
    const TOTAL: usize = 256 * 1024 * 1024;
    let pool = Arc::new(SshSessionPool::default());
    let target = SshTarget::parse("fixture")?;
    let ask: Option<Arc<dyn SshAskpass>> = Some(Ask::new(true, None));
    // Warm up SSH auth once so handshake isn't in the throughput window.
    let mut warm = pool.exec_io(&target, ask.clone(), "echo ok").await?;
    let mut wbuf = [0u8; 8];
    let _ = warm.read(&mut wbuf).await?;
    drop(warm);

    let mut stream = pool.exec_io(&target, ask, "head -c 268435456 /dev/zero").await?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut got = 0usize;
    let start = Instant::now();
    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        got += n;
    }
    let elapsed = start.elapsed();
    assert_eq!(got, TOTAL, "expected {TOTAL} bytes, got {got}");
    let mib_s = (TOTAL as f64 / (1024.0 * 1024.0)) / elapsed.as_secs_f64();
    eprintln!("BENCH throughput 256 MiB: {:.4} s ({:.1} MiB/s)", elapsed.as_secs_f64(), mib_s);
    Ok(())
}

#[tokio::test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
async fn ssh_bench_rtt() -> Result {
    if outer("ssh_bench_rtt") {
        return Ok(());
    }
    let pool = Arc::new(SshSessionPool::default());
    let target = SshTarget::parse("fixture")?;
    let ask: Option<Arc<dyn SshAskpass>> = Some(Ask::new(true, None));
    let mut stream = pool.exec_io(&target, ask, "exec cat").await?;
    let payload = [b'x'; 64];
    let mut reply = [0u8; 64];
    // Warm up channel & remote `cat`.
    for _ in 0..10 {
        stream.write_all(&payload).await?;
        stream.read_exact(&mut reply).await?;
    }
    let mut samples = Vec::with_capacity(1000);
    for _ in 0..1000 {
        let start = Instant::now();
        stream.write_all(&payload).await?;
        stream.read_exact(&mut reply).await?;
        assert_eq!(reply, payload);
        samples.push(start.elapsed());
    }
    samples.sort();
    let us = |d: Duration| d.as_secs_f64() * 1_000_000.0;
    eprintln!("BENCH rtt 1000x64B: min {:.1} us, med {:.1} us, p99 {:.1} us, max {:.1} us", us(samples[0]), us(samples[samples.len() / 2]), us(samples[990]), us(samples[samples.len() - 1]),);
    Ok(())
}

case!(ssh_r3_late_failure_not_clean_eof, {
    let pool = Arc::new(SshSessionPool::default());
    let target = SshTarget::parse("fixture")?;
    let ask: Option<Arc<dyn SshAskpass>> = Some(Ask::new(true, None));

    // 1. Non-empty PNG-like prefix followed by stderr + exit 1 MUST return Err
    //    (never clean EOF that could publish a truncated cached file).
    let mut partial = pool.exec_io(&target, ask.clone(), "printf '\\211PNGpartial-data'; echo 'disk read failed' >&2; exit 1").await?;
    let mut got = Vec::new();
    let res = timeout(Duration::from_secs(5), partial.read_to_end(&mut got)).await?;
    let err = res.expect_err("non-zero exit after partial stdout must fail, not return clean EOF").to_string();
    assert_eq!(got, b"\x89PNGpartial-data", "prefix bytes must be delivered before the error");
    assert!(err.contains('1') && err.contains("disk read failed"), "expected status 1 and stderr, got: {err}");

    // 2. Non-empty prefix followed by signal termination MUST return Err.
    let mut killed = pool.exec_io(&target, ask.clone(), "printf 'half-written'; echo 'fatal stderr' >&2; kill -9 $$").await?;
    let mut kbuf = Vec::new();
    let kerr = timeout(Duration::from_secs(5), killed.read_to_end(&mut kbuf)).await?.expect_err("signal after partial stdout must fail").to_string();
    assert_eq!(kbuf, b"half-written");
    assert!(kerr.contains("KILL") && kerr.contains("fatal stderr"), "expected KILL + stderr, got: {kerr}");

    // 3. Losing the SSH connection after the first packet MUST return Err, not clean EOF.
    let mut dropped = pool.exec_io(&target, ask, "printf 'first-chunk'; sleep 30").await?;
    let mut first = [0u8; 11];
    timeout(Duration::from_secs(5), dropped.read_exact(&mut first)).await??;
    assert_eq!(&first, b"first-chunk");
    pool.close(&target).await;
    let mut tail = Vec::new();
    let derr = timeout(Duration::from_secs(5), dropped.read_to_end(&mut tail)).await?.expect_err("SSH disconnect after partial stdout must fail, not return clean EOF");
    assert_eq!(derr.kind(), std::io::ErrorKind::UnexpectedEof, "expected UnexpectedEof, got: {derr:?}");
    Ok(())
});

async fn ping_term_rtt(term: &mut std::pin::Pin<Box<dyn olo_rex_client::transport::AsyncReadWrite + Send>>) -> Result<(Duration, Duration)> {
    let mut rtts = Vec::with_capacity(10);
    for i in 0u8..10 {
        let msg = [b'T', i, b'\n'];
        let mut reply = [0u8; 3];
        let t0 = Instant::now();
        timeout(Duration::from_secs(2), term.write_all(&msg)).await.map_err(|_| "terminal write timed out")??;
        timeout(Duration::from_secs(2), term.read_exact(&mut reply)).await.map_err(|_| "terminal echo timed out")??;
        assert_eq!(reply, msg);
        rtts.push(t0.elapsed());
    }
    rtts.sort();
    Ok((rtts[rtts.len() / 2], rtts[rtts.len() - 1]))
}

case!(ssh_r3_finished_stream_kept, {
    let pool = Arc::new(SshSessionPool::default());
    let target = SshTarget::parse("fixture")?;
    let ask: Option<Arc<dyn SshAskpass>> = Some(Ask::new(true, None));

    // Terminal echo stream on the pooled SSH connection.
    let mut term = pool.exec_io(&target, ask.clone(), "exec cat").await?;
    let (base_med, base_max) = ping_term_rtt(&mut term).await?;

    // Open a dial stream whose peer prints "ready\n", closes stdout (EOF),
    // and keeps reading a large upload. Client reads EOF and KEEPS the stream.
    let mut half = pool.open_rex_channel(&target, ask.clone(), false).await?;
    let mut banner = Vec::new();
    timeout(Duration::from_secs(5), half.read_to_end(&mut banner)).await??;
    assert_eq!(banner, b"ready\n");

    // Write enough data (> 100 server window adjustments) to fill the 100-slot
    // russh channel buffer if the finished reader retained its receiver, then
    // shut down stdin so the server sends ExitStatus + Close while `half` stays alive.
    let chunk = vec![b'U'; 32 * 1024];
    for _ in 0..256 {
        timeout(Duration::from_secs(5), half.write_all(&chunk)).await??;
    }
    timeout(Duration::from_secs(5), half.shutdown()).await??;
    // Also hold a completed `exec_io` stream read to EOF and kept alive.
    let mut done_cmd = pool.exec_io(&target, ask, "echo done").await?;
    let mut done_out = Vec::new();
    timeout(Duration::from_secs(5), done_cmd.read_to_end(&mut done_out)).await??;
    assert_eq!(done_out, b"done\n");

    // Sibling terminal on the SAME SSH connection must keep echoing cleanly.
    let (with_med, with_max) = ping_term_rtt(&mut term).await?;
    eprintln!(
        "MEASURE finding2 terminal RTT: baseline med {:.0} us max {:.0} us | beside finished-and-kept stream med {:.0} us max {:.0} us",
        base_med.as_secs_f64() * 1e6,
        base_max.as_secs_f64() * 1e6,
        with_med.as_secs_f64() * 1e6,
        with_max.as_secs_f64() * 1e6,
    );
    drop((half, done_cmd, term));
    Ok(())
});

case!(ssh_screen_unread_isolation, {
    use olo_rex_link::screen_wire::ServerMsg;

    let sshd_pid: i32 = fs::read_to_string("/fixture/sshd.pid")?.trim().parse()?;
    let pool = Arc::new(SshSessionPool::default());
    pool.set_helper(Some("test"));
    let target = SshTarget::parse("fixture")?;
    let ask: Option<Arc<dyn SshAskpass>> = Some(Ask::new(true, None));

    // Terminal echo stream on the primary pooled SSH connection.
    let mut term = pool.exec_io(&target, ask.clone(), "exec cat").await?;
    let mut base = Vec::with_capacity(10);
    for i in 0u8..10 {
        let msg = [b'E', i, b'\n'];
        let mut reply = [0u8; 3];
        let t0 = Instant::now();
        timeout(Duration::from_secs(2), term.write_all(&msg)).await??;
        timeout(Duration::from_secs(2), term.read_exact(&mut reply)).await??;
        assert_eq!(reply, msg);
        base.push(t0.elapsed());
    }
    base.sort();

    // Start a large-keyframe screen stream over SSH fallback (1600×900 high-entropy
    // keyframe > 5 MiB, exceeding russh's 100-packet ~3.2 MiB channel queue in a
    // single frame) and synchronize deterministically on `served_marker` + live
    // child PID before probing terminal echo (Test Finding 2)!
    let served_marker = Path::new("/fixture/frame1-served");
    let _ = fs::remove_file(served_marker);
    let fake = FixtureScreen::start_sized_with_marker("qa", 1600, 900, Some(served_marker));
    let mut screen = pool.open_screen_stream(&target, ask, &["stream".to_owned(), "qa".to_owned()]).await?;
    assert_eq!(pool.open_connection_count(&target), (1, 0, 1), "screen stream should use 1 dedicated screen connection");

    let wait_deadline = Instant::now() + Duration::from_secs(10);
    while !served_marker.exists() && Instant::now() < wait_deadline {
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    assert!(served_marker.exists(), "fake compositor did not serve the 1600x900 screenshot");
    // Give the helper + sshd a beat to push the > 5 MiB frame into russh's unread buffer,
    // and assert the helper child is STILL running (never exited early!).
    tokio::time::sleep(Duration::from_millis(250)).await;
    let stream_pid = descendant_with_args(sshd_pid, &["screen", "stream", "qa"]).expect("screen stream helper MUST still be alive while unread");

    // Terminal echo MUST still respond promptly while `screen` stays completely unread!
    let mut with_unread = Vec::with_capacity(10);
    for i in 0u8..10 {
        let msg = [b'F', i, b'\n'];
        let mut reply = [0u8; 3];
        let t0 = Instant::now();
        timeout(Duration::from_secs(2), term.write_all(&msg)).await.map_err(|_| "terminal write stalled behind unread screen stream")??;
        timeout(Duration::from_secs(2), term.read_exact(&mut reply)).await.map_err(|_| "terminal echo stalled behind unread screen stream")??;
        assert_eq!(reply, msg);
        with_unread.push(t0.elapsed());
    }
    with_unread.sort();
    // Prove the screen helper stayed alive throughout and actually delivered frame 1!
    assert_eq!(descendant_with_args(sshd_pid, &["screen", "stream", "qa"]), Some(stream_pid), "screen helper exited during probes");
    let frame1 = read_one_server_msg(&mut screen).await?;
    assert!(matches!(frame1, ServerMsg::Frame { seq: 1, .. }), "expected live frame 1 after unread probes, got {frame1:?}");

    eprintln!(
        "MEASURE finding3 terminal RTT: baseline med {:.0} us max {:.0} us | beside unread flooding screen med {:.0} us max {:.0} us",
        base[base.len() / 2].as_secs_f64() * 1e6,
        base[base.len() - 1].as_secs_f64() * 1e6,
        with_unread[with_unread.len() / 2].as_secs_f64() * 1e6,
        with_unread[with_unread.len() - 1].as_secs_f64() * 1e6,
    );

    // Finding 9: dropping the last screen stream immediately closes the idle
    // dedicated screen connection (returns to `(1, 0, 0)`).
    drop(screen);
    let close_deadline = Instant::now() + Duration::from_secs(2);
    while pool.open_connection_count(&target) != (1, 0, 0) && Instant::now() < close_deadline {
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    assert_eq!(pool.open_connection_count(&target), (1, 0, 0), "idle screen connection must close when last stream drops");
    drop((term, fake));
    Ok(())
});

case!(ssh_r3_client_close_no_channel_leak, {
    let pool = Arc::new(SshSessionPool::default());
    let target = SshTarget::parse("fixture")?;
    let ask: Option<Arc<dyn SshAskpass>> = Some(Ask::new(true, None));

    // Keep one sibling echo channel open on the connection so we verify the
    // exact delta (baseline = 1, after 5 client-initiated closes = 1).
    let mut sibling = pool.exec_io(&target, ask.clone(), "exec cat").await?;
    sibling.write_all(b"ok\n").await?;
    let mut b = [0u8; 3];
    sibling.read_exact(&mut b).await?;
    assert_eq!(&b, b"ok\n");
    let before = pool.open_channel_count(&target).await?;
    assert_eq!(before, (1, 1), "expected 1 sibling channel before, got {before:?}");

    // Open 5 long-lived commands that ignore stdin EOF (`sleep 60`), wait for
    // each to start (`echo go`), then drop from the client side BEFORE the peer closes.
    for _ in 0..5 {
        let mut s = pool.exec_io(&target, ask.clone(), "echo go; exec sleep 60").await?;
        let mut go = [0u8; 3];
        timeout(Duration::from_secs(5), s.read_exact(&mut go)).await??;
        assert_eq!(&go, b"go\n");
        drop(s);
    }
    // Wait briefly for the spawned drop tasks to process CLOSE on the SSH driver
    // and verify the connection's channel tables return to exactly (1, 1).
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut after = pool.open_channel_count(&target).await?;
    while after != (1, 1) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
        after = pool.open_channel_count(&target).await?;
    }
    assert_eq!(after, (1, 1), "client-initiated close leaked russh channels: before={before:?} after={after:?}");
    // Sibling still works on the same connection.
    sibling.write_all(b"hi\n").await?;
    sibling.read_exact(&mut b).await?;
    assert_eq!(&b, b"hi\n");
    drop(sibling);
    Ok(())
});

/// Configurable in-process `russh::server` used by `ssh_final_*` integration
/// tests inside isolated `bwrap` namespaces (with `/fixture/home/.ssh`).
struct FinalMockConfig {
    name: &'static str,
    allow_pubkey: bool,
    password: Option<&'static str>,
    max_sessions_per_conn: usize,
    max_connections: usize,
    connections_seen: Arc<std::sync::atomic::AtomicUsize>,
}

struct FinalMockHandler {
    cfg: Arc<FinalMockConfig>,
    active_sessions: std::sync::atomic::AtomicUsize,
}

impl russh::server::Handler for FinalMockHandler {
    type Error = russh::Error;

    async fn auth_publickey(&mut self, _user: &str, _key: russh::server::VerifiedPublicKey<'_>) -> std::result::Result<russh::server::Auth, Self::Error> {
        if self.cfg.allow_pubkey {
            Ok(russh::server::Auth::Accept)
        } else {
            let mut methods = russh::MethodSet::empty();
            if self.cfg.password.is_some() {
                methods.push(russh::MethodKind::Password);
            }
            Ok(russh::server::Auth::Reject { proceed_with_methods: Some(methods), partial_success: false })
        }
    }

    async fn auth_none(&mut self, _user: &str) -> std::result::Result<russh::server::Auth, Self::Error> {
        let mut methods = russh::MethodSet::empty();
        if self.cfg.allow_pubkey {
            methods.push(russh::MethodKind::PublicKey);
        }
        if self.cfg.password.is_some() {
            methods.push(russh::MethodKind::Password);
        }
        Ok(russh::server::Auth::Reject { proceed_with_methods: Some(methods), partial_success: false })
    }

    async fn auth_password(&mut self, _user: &str, password: &str) -> std::result::Result<russh::server::Auth, Self::Error> {
        if self.cfg.password == Some(password) { Ok(russh::server::Auth::Accept) } else { Ok(russh::server::Auth::Reject { proceed_with_methods: None, partial_success: false }) }
    }

    async fn channel_open_session(
        &mut self,
        _channel: russh::Channel<russh::server::Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut russh::server::Session,
    ) -> std::result::Result<(), Self::Error> {
        let cur = self.active_sessions.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.cfg.max_sessions_per_conn > 0 && cur >= self.cfg.max_sessions_per_conn {
            self.active_sessions.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            reply.reject(russh::ChannelOpenFailure::ResourceShortage).await;
            return Ok(());
        }
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(&mut self, channel: russh::ChannelId, data: &[u8], session: &mut russh::server::Session) -> std::result::Result<(), Self::Error> {
        let cmd = String::from_utf8_lossy(data).into_owned();
        session.channel_success(channel)?;
        let handle = session.handle();
        let name = self.cfg.name;
        if cmd.contains("hold") {
            // Stay open until client closes.
            tokio::spawn(async move {
                let _ = handle.data(channel, bytes::Bytes::from(format!("held:{name}\n"))).await;
            });
        } else {
            tokio::spawn(async move {
                let _ = handle.data(channel, bytes::Bytes::from(format!("server:{name}\n"))).await;
                let _ = handle.eof(channel).await;
                let _ = handle.exit_status_request(channel, 0).await;
                let _ = handle.close(channel).await;
            });
        }
        Ok(())
    }

    async fn channel_close(&mut self, _channel: russh::ChannelId, _session: &mut russh::server::Session) -> std::result::Result<(), Self::Error> {
        self.active_sessions.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

async fn start_final_mock_ssh(cfg: Arc<FinalMockConfig>) -> (std::net::SocketAddr, String, tokio::task::JoinHandle<()>) {
    let seed = olo_rex_client::quic::new_token().unwrap();
    let key = russh::keys::PrivateKey::from(russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&seed));
    let pub_openssh = key.public_key().to_openssh().unwrap().split_whitespace().take(2).collect::<Vec<_>>().join(" ");
    let mut scfg = russh::server::Config::default();
    scfg.keys.push(key);
    let scfg = Arc::new(scfg);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else { break };
            let n = cfg.connections_seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if cfg.max_connections > 0 && n > cfg.max_connections {
                drop(tcp);
                continue;
            }
            let scfg = scfg.clone();
            let handler = FinalMockHandler { cfg: cfg.clone(), active_sessions: std::sync::atomic::AtomicUsize::new(0) };
            tokio::spawn(async move {
                let _ = russh::server::run_stream(scfg, tcp, handler).await;
            });
        }
    });
    (addr, pub_openssh, task)
}

#[derive(Debug)]
struct CountingAskpass {
    password: Option<String>,
    prompts: Arc<std::sync::atomic::AtomicUsize>,
    host_confirms: Arc<std::sync::atomic::AtomicUsize>,
}

impl SshAskpass for CountingAskpass {
    fn confirm_host_key(&self, _host: &str, _port: u16, _fp: &str, _kt: &str, changed: bool) -> bool {
        self.host_confirms.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        !changed
    }

    fn prompt_secret(&self, _title: &str, _prompt: &str, _echo: bool) -> Option<zeroize::Zeroizing<String>> {
        self.prompts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.password.clone().map(zeroize::Zeroizing::new)
    }
}

// Finding 6: rewriting an SSH alias in `~/.ssh/config` while the primary
// connection is live MUST NOT route secondary screen connections to another
// already-trusted machine or overwrite `pool.verified(target)`.
case!(ssh_final_f6_alias_rewrite_pinned_to_primary, {
    use std::sync::atomic::Ordering;

    let cfg_a = Arc::new(FinalMockConfig {
        name: "machine-a",
        allow_pubkey: true,
        password: None,
        max_sessions_per_conn: 10,
        max_connections: 0,
        connections_seen: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    });
    let cfg_b = Arc::new(FinalMockConfig {
        name: "machine-b",
        allow_pubkey: true,
        password: None,
        max_sessions_per_conn: 10,
        max_connections: 0,
        connections_seen: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    });
    let (addr_a, key_a, srv_a) = start_final_mock_ssh(cfg_a.clone()).await;
    let (addr_b, key_b, srv_b) = start_final_mock_ssh(cfg_b.clone()).await;

    // Pre-trust BOTH servers in `/fixture/home/.ssh/known_hosts`.
    let known_hosts = format!("[127.0.0.1]:{} {key_a}\n[127.0.0.1]:{} {key_b}\n", addr_a.port(), addr_b.port());
    fs::write("/fixture/home/.ssh/known_hosts", known_hosts)?;
    fs::write("/fixture/home/.ssh/config", format!("Host alias-a\n HostName 127.0.0.1\n Port {}\n User scratch\n IdentityFile /fixture/identity\n", addr_a.port()))?;

    let pool = Arc::new(SshSessionPool::default());
    let target = SshTarget::parse("alias-a")?;
    let ask: Option<Arc<dyn SshAskpass>> = Some(Ask::new(true, None));

    pool.reach(&target, ask.clone()).await?;
    let verified_before = pool.verified(&target).expect("primary verified key");
    assert_eq!(verified_before.port, addr_a.port());
    assert_eq!(verified_before.key, key_a);
    assert_eq!(cfg_a.connections_seen.load(Ordering::SeqCst), 1);

    // Rewrite alias `alias-a` to point to already-trusted `machine-b` while primary is live!
    fs::write("/fixture/home/.ssh/config", format!("Host alias-a\n HostName 127.0.0.1\n Port {}\n User scratch\n IdentityFile /fixture/identity\n", addr_b.port()))?;

    let mut s = pool.open_screen_stream(&target, ask, &["follow".to_owned()]).await?;
    let mut out = String::new();
    timeout(Duration::from_secs(5), s.read_to_string(&mut out)).await??;
    assert_eq!(out, "server:machine-a\n", "screen stream MUST reach pinned machine-a, not rewritten alias machine-b!");
    assert_eq!(cfg_b.connections_seen.load(Ordering::SeqCst), 0, "machine-b must NEVER be dialed");
    assert_eq!(cfg_a.connections_seen.load(Ordering::SeqCst), 2, "machine-a should serve the dedicated screen connection");
    assert_eq!(pool.verified(&target), Some(verified_before), "pool.verified() must stay pinned to machine-a");

    drop(s);
    pool.close(&target).await;
    srv_a.abort();
    srv_b.abort();
    Ok(())
});

// Finding 7: secondary screen connection NEVER calls askpass (password-only
// host gets 0 extra prompts and falls back to primary), and a 1-connection
// gateway falls back cleanly to primary instead of failing.
case!(ssh_final_f7_no_askpass_and_single_conn_fallback, {
    use std::sync::atomic::Ordering;

    // Case A: password-only server (rejects all public keys).
    {
        let cfg = Arc::new(FinalMockConfig {
            name: "pw-only",
            allow_pubkey: false,
            password: Some("secret-pw"),
            max_sessions_per_conn: 10,
            max_connections: 0,
            connections_seen: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        let (addr, key, srv) = start_final_mock_ssh(cfg.clone()).await;
        fs::write("/fixture/home/.ssh/known_hosts", format!("[127.0.0.1]:{} {key}\n", addr.port()))?;
        let prompts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let host_confirms = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ask: Option<Arc<dyn SshAskpass>> = Some(Arc::new(CountingAskpass { password: Some("secret-pw".into()), prompts: prompts.clone(), host_confirms: host_confirms.clone() }));

        let pool = Arc::new(SshSessionPool::default());
        let target = SshTarget::parse(&format!("ssh://scratch@127.0.0.1:{}", addr.port()))?;
        pool.reach(&target, ask.clone()).await?;
        assert_eq!(prompts.load(Ordering::SeqCst), 1, "primary login takes 1 password prompt");

        // Opening two screen streams MUST trigger ZERO extra password prompts and succeed via primary!
        for _ in 0..2 {
            let mut s = pool.open_screen_stream(&target, ask.clone(), &["follow".to_owned()]).await?;
            let mut out = String::new();
            timeout(Duration::from_secs(5), s.read_to_string(&mut out)).await??;
            assert_eq!(out, "server:pw-only\n");
        }
        assert_eq!(prompts.load(Ordering::SeqCst), 1, "screen fallback MUST NEVER prompt askpass a second time!");
        assert_eq!(host_confirms.load(Ordering::SeqCst), 0);
        assert_eq!(pool.open_connection_count(&target), (1, 0, 0));
        pool.close(&target).await;
        srv.abort();
    }

    // Case B: 1-connection gateway (refuses any 2nd TCP connection).
    {
        let cfg = Arc::new(FinalMockConfig {
            name: "one-conn",
            allow_pubkey: true,
            password: None,
            max_sessions_per_conn: 10,
            max_connections: 1,
            connections_seen: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        let (addr, key, srv) = start_final_mock_ssh(cfg.clone()).await;
        fs::write("/fixture/home/.ssh/known_hosts", format!("[127.0.0.1]:{} {key}\n", addr.port()))?;
        fs::write("/fixture/home/.ssh/config", format!("Host oneconn\n HostName 127.0.0.1\n Port {}\n User scratch\n IdentityFile /fixture/identity\n", addr.port()))?;

        let pool = Arc::new(SshSessionPool::default());
        let target = SshTarget::parse("oneconn")?;
        let ask: Option<Arc<dyn SshAskpass>> = Some(Ask::new(true, None));
        pool.reach(&target, ask.clone()).await?;

        // Both screen opens MUST succeed by falling back to primary (second one skips probing).
        for _ in 0..2 {
            let mut s = pool.open_screen_stream(&target, ask.clone(), &["follow".to_owned()]).await?;
            let mut out = String::new();
            timeout(Duration::from_secs(5), s.read_to_string(&mut out)).await??;
            assert_eq!(out, "server:one-conn\n");
        }
        // Only 1 failed secondary probe was ever attempted (gen disabled after first failure).
        assert_eq!(cfg.connections_seen.load(Ordering::SeqCst), 2, "must not repeatedly dial a 1-connection host");
        pool.close(&target).await;
        srv.abort();
    }
    Ok(())
});

// Findings 8 & 9:
// - Host A stalled mid-connect MUST NOT block Host B's screen opens or `close(B)`,
//   and `close(A)` cancels Host A's pending open immediately;
// - Unified 5-connection budget (`primary + further + screens <= 5`) and
//   immediate teardown of idle screen connections when their last stream drops.
case!(ssh_final_f8_f9_no_cross_host_lock_and_five_conn_budget, {
    use std::sync::atomic::Ordering;

    // 1. Finding 8: blackhole listener for Host A (accepts TCP, never sends SSH banner).
    let blackhole = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let blackhole_addr = blackhole.local_addr()?;
    let bh_task = tokio::spawn(async move {
        let mut kept = Vec::new();
        while let Ok((sock, _)) = blackhole.accept().await {
            kept.push(sock);
        }
    });

    // Live mock server for Host B.
    let cfg_b = Arc::new(FinalMockConfig {
        name: "host-b",
        allow_pubkey: true,
        password: None,
        max_sessions_per_conn: 1,
        max_connections: 0,
        connections_seen: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    });
    let (addr_b, key_b, srv_b) = start_final_mock_ssh(cfg_b.clone()).await;
    fs::write("/fixture/home/.ssh/known_hosts", format!("[127.0.0.1]:{} {key_b}\n", addr_b.port()))?;
    fs::write(
        "/fixture/home/.ssh/config",
        format!(
            "Host stall-a\n HostName 127.0.0.1\n Port {}\n User scratch\n ConnectTimeout 30\n\
             Host fast-b\n HostName 127.0.0.1\n Port {}\n User scratch\n IdentityFile /fixture/identity\n",
            blackhole_addr.port(),
            addr_b.port(),
        ),
    )?;

    let pool = Arc::new(SshSessionPool::default());
    let target_a = SshTarget::parse("stall-a")?;
    let target_b = SshTarget::parse("fast-b")?;
    let ask: Option<Arc<dyn SshAskpass>> = Some(Ask::new(true, None));

    // Start Host A screen open in background (stalls inside connect).
    let (pool_a, ta, ask_a) = (pool.clone(), target_a.clone(), ask.clone());
    let task_a = tokio::spawn(async move { pool_a.open_screen_stream(&ta, ask_a, &["follow".to_owned()]).await });
    tokio::time::sleep(Duration::from_millis(60)).await;

    // While Host A is stalled, Host B screen open AND close(B) MUST finish in < 1 s!
    let t0 = Instant::now();
    let mut sb = timeout(Duration::from_secs(1), pool.open_screen_stream(&target_b, ask.clone(), &["follow".to_owned()])).await??;
    let mut out_b = String::new();
    timeout(Duration::from_secs(2), sb.read_to_string(&mut out_b)).await??;
    assert_eq!(out_b, "server:host-b\n");
    drop(sb);
    timeout(Duration::from_secs(1), pool.close(&target_b)).await?;
    assert!(t0.elapsed() < Duration::from_secs(2), "Host A stalled Host B!");

    // Calling `close(A)` MUST cancel Host A's pending open promptly (not wait 30 s)!
    let t_close_a = Instant::now();
    pool.close(&target_a).await;
    let res_a = timeout(Duration::from_secs(1), task_a).await??;
    assert!(res_a.is_err(), "closed Host A pending open must error");
    assert!(t_close_a.elapsed() < Duration::from_secs(1));

    // 2. Finding 9: unified 5-connection budget (`primary + further + screens <= 5`)
    //    and immediate idle screen teardown when the last screen stream drops!
    // Reset Host B counters and use `MaxSessions = 1` per connection.
    cfg_b.connections_seen.store(0, Ordering::SeqCst);
    let mut held_ord = Vec::new();
    for _ in 0..3 {
        let mut s = pool.exec_io(&target_b, ask.clone(), "hold").await?;
        let mut line = [0u8; 12];
        timeout(Duration::from_secs(2), s.read_exact(&mut line)).await??;
        held_ord.push(s);
    }
    assert_eq!(pool.open_connection_count(&target_b), (1, 2, 0));

    // Open 2 held screen streams -> 2 dedicated screen connections -> total = 5 (budget full!).
    let mut held_scr = Vec::new();
    for _ in 0..2 {
        let mut s = pool.open_screen_stream(&target_b, ask.clone(), &["stream".to_owned(), "hold".to_owned()]).await?;
        let mut line = [0u8; 12];
        timeout(Duration::from_secs(2), s.read_exact(&mut line)).await??;
        held_scr.push(s);
    }
    assert_eq!(pool.open_connection_count(&target_b), (1, 2, 2), "total connections must be 1+2+2 = 5");
    assert_eq!(cfg_b.connections_seen.load(Ordering::SeqCst), 5);

    // A 6th concurrent stream MUST NOT exceed the 5-connection budget!
    let sixth = pool.open_screen_stream(&target_b, ask.clone(), &["stream".to_owned(), "hold".to_owned()]).await;
    assert!(sixth.is_err(), "6th concurrent stream on 5 full 1-slot connections must be refused");
    assert_eq!(cfg_b.connections_seen.load(Ordering::SeqCst), 5, "MUST NEVER open more than 5 connections to one target");

    // Dropping both screen streams MUST immediately close both idle screen connections!
    drop(held_scr);
    let deadline = Instant::now() + Duration::from_secs(2);
    while pool.open_connection_count(&target_b) != (1, 2, 0) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    assert_eq!(pool.open_connection_count(&target_b), (1, 2, 0), "idle screen connections must close immediately when their last stream drops");

    drop(held_ord);
    pool.close(&target_b).await;
    srv_b.abort();
    bh_task.abort();
    Ok(())
});
