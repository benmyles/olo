//! The coding agents' tab title service (`olo-terminal --mcp-service`)
//! becomes each new version without a gap: requests sent while it's
//! upgraded again and again are all answered, by the same process.

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// A ping over a connection of its own, as an agent sends each request;
/// its answer, or what went wrong.
fn ping(port: u16, id: u64) -> Result<(), String> {
    let mut c = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("connect: {e}"))?;
    c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let body = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"ping"}}"#);
    write!(c, "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).map_err(|e| format!("send: {e}"))?;
    let mut got = String::new();
    c.read_to_string(&mut got).map_err(|e| format!("read: {e}"))?;
    if got.starts_with("HTTP/1.1 200 ") && got.contains(&format!("\"id\":{id}")) { Ok(()) } else { Err(format!("answered {got:?}")) }
}

#[test]
fn upgrades_without_refusing_anyone() {
    let dir = std::path::PathBuf::from(format!("/var/tmp/olo-title-service-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_olo-terminal"))
        .arg("--mcp-service")
        .env("OLO_TERMINAL_MCP_PORT", port.to_string())
        .env("HOME", &dir)
        .env("XDG_RUNTIME_DIR", &dir)
        .env("XDG_STATE_HOME", dir.join("state"))
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id() as i32;
    let deadline = Instant::now() + Duration::from_secs(10);
    while ping(port, 0).is_err() {
        assert!(Instant::now() < deadline, "the service never answered");
        std::thread::sleep(Duration::from_millis(20));
    }

    // Agents asking all the while, from four threads.
    let stop = Arc::new(AtomicBool::new(false));
    let answered = Arc::new(AtomicU64::new(0));
    let askers: Vec<_> = (0..4u64)
        .map(|t| {
            let (stop, answered) = (stop.clone(), answered.clone());
            std::thread::spawn(move || {
                let mut failed = Vec::new();
                let mut n = 0;
                while !stop.load(Ordering::Relaxed) {
                    n += 1;
                    match ping(port, t * 1_000_000 + n) {
                        Ok(()) => {
                            answered.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => failed.push(e),
                    }
                }
                failed
            })
        })
        .collect();
    // Ten upgrades (olo-session's SIGURG), each once the last had gone
    // through (the new version takes the signal again once it's up).
    let mut upgrades = 0;
    for _ in 0..10 {
        let before = answered.load(Ordering::Relaxed);
        // SAFETY: a signal to our own child.
        unsafe { libc::kill(pid, libc::SIGURG) };
        upgrades += 1;
        std::thread::sleep(Duration::from_millis(150));
        assert!(answered.load(Ordering::Relaxed) > before, "nothing answered after upgrade {upgrades}");
    }
    stop.store(true, Ordering::Relaxed);
    let failed: Vec<String> = askers.into_iter().flat_map(|a| a.join().unwrap()).collect();
    // It became the new version in place: the same process, started again
    // with the socket handed over.
    let env = std::fs::read(format!("/proc/{pid}/environ")).unwrap();
    let handed = env.split(|&b| b == 0).any(|v| v.starts_with(b"OLO_TERMINAL_MCP_LISTENER="));
    let alive = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(failed.is_empty(), "{} of {} requests failed across the upgrades: {:?}", failed.len(), failed.len() as u64 + answered.load(Ordering::Relaxed), &failed[..failed.len().min(5)]);
    assert!(alive, "the service ended");
    assert!(handed, "it never became the new version");
    eprintln!("{} requests answered across {upgrades} upgrades", answered.load(Ordering::Relaxed));
}
