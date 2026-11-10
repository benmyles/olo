//! Version 1 JSON-lines API over an owner-only Unix socket. Commands use one
//! request/reply. Subscriptions replay durable events then follow live events;
//! a slow subscriber disconnects and can replay from its last sequence.
use crate::{Command, Engine};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    io::{BufRead, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
};

pub fn data_dir() -> PathBuf {
    std::env::var_os("OLO_CODER_HOME").map(PathBuf::from).unwrap_or_else(|| olo_xdg::state_dir().join("coder"))
}
pub fn socket(dir: &Path) -> PathBuf {
    dir.join("service.sock")
}
pub async fn serve(dir: PathBuf) -> Result<()> {
    serve_with_upgrade(dir, None).await.map(|_| ())
}
/// Drain active turns before replacing the executable. Existing approvals and
/// cancellation still work; new turns wait until clients reconnect.
pub async fn serve_with_upgrade(dir: PathBuf, mut upgrade: Option<tokio::sync::oneshot::Receiver<()>>) -> Result<bool> {
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).mode(0o600).open(dir.join("service.lock"))?;
    use std::os::fd::AsRawFd;
    ensure!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0, "Coder service already running");
    let path = socket(&dir);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let e = Engine::open(dir)?;
    let mut upgrading = false;
    loop {
        tokio::select! {
            _=async { match upgrade.as_mut() { Some(signal) => { let _ = signal.await; }, None => std::future::pending::<()>().await } }, if !upgrading => {
                upgrading = true;
                e.upgrading.store(true, std::sync::atomic::Ordering::Release);
            },
            _=e.quiesce(), if upgrading => break,
            _=e.shutdown.cancelled()=>{upgrading = false; break},
            accepted=listener.accept()=>{let(stream,_)=accepted?;let e=e.clone();tokio::spawn(async move{if let Err(error)=connection(stream,e).await{tracing_error(&error);}});}
        }
    }
    // Give cancelled tools time to journal outcomes before the runtime exits.
    e.quiesce().await;
    e.shutdown.cancel();
    std::fs::remove_file(path)?;
    drop(lock);
    Ok(upgrading)
}
fn tracing_error(e: &anyhow::Error) {
    if !e.to_string().contains("Broken pipe") {
        eprintln!("olo-coder connection: {e:#}");
    }
}
async fn send(stream: &mut tokio::net::unix::OwnedWriteHalf, value: &Value) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(10), stream.write_all(format!("{value}\n").as_bytes())).await??;
    Ok(())
}
async fn connection(stream: UnixStream, e: Arc<Engine>) -> Result<()> {
    let peer = stream.peer_cred()?;
    ensure!(peer.uid() == unsafe { libc::geteuid() }, "peer is not the service owner");
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut line = Vec::new();
    // read_until is bounded by explicit chunks, so a client cannot make an unbounded line.
    loop {
        let b = reader.fill_buf().await?;
        if b.is_empty() {
            break;
        }
        let n = b.iter().position(|b| *b == b'\n').map_or(b.len(), |n| n + 1);
        ensure!(line.len() + n <= 8 * 1024 * 1024, "request too large");
        line.extend_from_slice(&b[..n]);
        let end = b[n - 1] == b'\n';
        reader.consume(n);
        if end {
            break;
        }
    }
    if line.is_empty() {
        return Ok(());
    }
    let request: Value = serde_json::from_slice(&line)?;
    ensure!(request.get("version").is_none_or(|v| v == 1), "unsupported protocol version");
    if request["subscribe_sessions"] == true {
        let filter: crate::SessionFilter = request.get("filter").map(|v| serde_json::from_value(v.clone())).transpose()?.unwrap_or_default();
        let mut live = e.events.subscribe();
        loop {
            let catalog = e.store.lock().unwrap().browse(&filter)?;
            let profiles = e.model_profiles.lock().unwrap().clone();
            let peers = e.peers.lock().unwrap().clone();
            send(&mut write, &json!({"catalog":catalog,"profiles":profiles,"peers":peers})).await?;
            loop {
                tokio::select! {
                    _=e.shutdown.cancelled()=>return Ok(()), _=reader.read_u8()=>return Ok(()),
                    event=live.recv()=>match event {
                        Ok(event) if event.kind=="peers_changed"=>{send(&mut write,&json!({"peers":event.data["peers"]})).await?;},
                        Ok(event) if event.kind=="model_profiles_changed"=>{send(&mut write,&json!({"profiles":event.data["profiles"]})).await?;},
                        Ok(event) if event.seq>0 && matches!(event.kind.as_str(),"created"|"forked"|"organized"|"running"|"idle"|"approval"|"paused"|"error"|"stop"|"message_queued"|"message_cancelled"|"message_edited"|"message_moved"|"messages_delivered"|"configured") => break,
                        Err(_)=>break, _=>{}
                    }
                }
            }
            // One short debounce only after a change, never an idle timer.
            tokio::select! { _=e.shutdown.cancelled()=>return Ok(()), _=tokio::time::sleep(std::time::Duration::from_millis(100))=>{} }
            while live.try_recv().is_ok() {}
        }
    } else if request["subscribe"] == true {
        let session = request["session"].as_str().context("subscription requires session")?;
        let mut live = e.events.subscribe();
        let mut cursor = request["after"].as_i64().unwrap_or(-1);
        if cursor < 0 {
            cursor = e.store.lock().unwrap().tail_cursor(session, 500)?;
        }
        loop {
            let events = e.store.lock().unwrap().events(session, cursor, 1000)?;
            if events.is_empty() {
                break;
            }
            for event in events {
                cursor = cursor.max(event.seq);
                send(&mut write, &serde_json::to_value(event)?).await?;
            }
        }
        send(&mut write, &json!({"ready":true,"after":cursor})).await?;
        loop {
            tokio::select! {_=e.shutdown.cancelled()=>break,_=reader.read_u8()=>break,result=live.recv()=>match result{
                Ok(event)if event.session==session&&(event.seq==0||event.seq>cursor)=>{cursor=cursor.max(event.seq);send(&mut write,&serde_json::to_value(event)?).await?;},
                Ok(_)=>{},Err(_)=>{send(&mut write,&json!({"reconnect":true,"after":cursor})).await?;break;}
            }}
        }
    } else {
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let cmd = request.get("command").unwrap_or(&request);
        let result = match serde_json::from_value::<Command>(cmd.clone()) {
            Ok(c) => e.command(c).await,
            Err(error) => Err(error.into()),
        };
        let reply = match result {
            Ok(v) => json!({"version":1,"id":id,"result":v}),
            Err(err) => json!({"version":1,"id":id,"error":format!("{err:#}")}),
        };
        send(&mut write, &reply).await?;
    }
    Ok(())
}
/// Blocking client, used only on a frontend's worker thread or a CLI invocation.
pub fn call(dir: &Path, command: &Command) -> Result<Value> {
    let mut stream = std::os::unix::net::UnixStream::connect(socket(dir)).context("connect to Coder service")?;
    use std::os::fd::AsRawFd;
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    ensure!(
        unsafe { libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) } == 0 && cred.uid == unsafe { libc::geteuid() },
        "Coder service is not owned by this user"
    );
    stream.set_read_timeout(Some(std::time::Duration::from_secs(45)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(45)))?;
    writeln!(stream, "{}", json!({"version":1,"command":command}))?;
    let mut line = String::new();
    use std::io::Read;
    std::io::BufReader::new(stream.take(128 * 1024 * 1024 + 1)).read_line(&mut line)?;
    ensure!(line.len() <= 128 * 1024 * 1024, "response exceeds 128 MB");
    let v: Value = serde_json::from_str(&line)?;
    if let Some(e) = v.get("error") {
        anyhow::bail!("{}", e.as_str().unwrap_or("command failed"));
    }
    Ok(v["result"].clone())
}
pub fn follow(dir: &Path, session: &str, after: i64, mut event: impl FnMut(Value) -> bool) -> Result<()> {
    let mut stream = std::os::unix::net::UnixStream::connect(socket(dir))?;
    writeln!(stream, "{}", json!({"subscribe":true,"session":session,"after":after}))?;
    for line in std::io::BufReader::new(stream).lines() {
        let value: Value = serde_json::from_str(&line?)?;
        ensure!(value["reconnect"] != true, "event subscription needs reconnect; resume from sequence {}", value["after"]);
        if !event(value) {
            return Ok(());
        }
    }
    anyhow::bail!("event stream disconnected before completion; the session may still be running")
}
