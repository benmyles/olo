//! A Rex server of a test's own: its socket, process tree, home and
//! network namespace (bwrap), under `OLO_REX_TEST_DIR` or `/var/tmp`.
//! Shared by the tests that need the real server: this crate's
//! `live.rs`, and the helper's (`crates/rex-link/tests/server.rs`, which
//! runs Olo's helper in front of it).
#![allow(dead_code)]

use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

use olo_rex_client::*;
use serde_json::json;
use tokio::{
    sync::mpsc,
    time::{sleep, timeout},
};
use uuid::Uuid;

pub type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
pub type Events = mpsc::UnboundedReceiver<ControlEventEnvelope>;

pub struct Scratch {
    pub child: Child,
    pub dir: PathBuf,
    pub sock: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

impl Scratch {
    pub async fn start() -> Result<Option<Self>> {
        let rex = find_rex_binary();
        if !rex.is_file() && !std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).any(|d| d.join(&rex).is_file()) {
            eprintln!("skipping live Rex contract test: rex is not installed");
            return Ok(None);
        }
        if !std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).any(|dir| dir.join("bwrap").is_file()) {
            eprintln!("skipping live Rex contract test: bwrap is not installed");
            return Ok(None);
        }
        let root = std::env::var_os("OLO_REX_TEST_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/var/tmp"));
        let dir = root.join(format!("rex-{}", &Uuid::new_v4().simple().to_string()[..12]));
        let home = dir.join("home");
        let runtime = dir.join("run");
        let sock = dir.join("rex.sock");
        fs::DirBuilder::new().mode(0o700).create(&dir)?;
        for sub in [&home, &runtime, &dir.join("tmp")] {
            fs::DirBuilder::new().mode(0o700).create(sub)?;
        }
        // Rex also takes a per-endpoint startup lock below /run/user/UID.
        // Map that and /tmp to scratch disk. No host /home, /run or network.
        let addr = format!("unix://{}", sock.display());
        let uid = unsafe { libc::geteuid() };
        let child = Command::new("bwrap")
            .args([
                "--die-with-parent",
                "--unshare-net",
                "--unshare-pid",
                "--new-session",
                "--ro-bind",
                "/usr",
                "/usr",
                "--ro-bind",
                "/etc",
                "/etc",
                "--ro-bind",
                "/bin",
                "/bin",
                "--ro-bind",
                "/lib",
                "/lib",
                "--ro-bind-try",
                "/lib64",
                "/lib64",
                "--dev",
                "/dev",
                "--proc",
                "/proc",
                "--bind",
            ])
            .arg(&dir)
            .arg(&dir)
            .args(["--dir", "/run/user", "--bind"])
            .arg(&runtime)
            .arg(format!("/run/user/{uid}"))
            .arg("--bind")
            .arg(dir.join("tmp"))
            .arg("/tmp")
            .arg("--chdir")
            .arg(&home)
            .args(["--clearenv", "--setenv", "PATH", "/usr/local/bin:/usr/bin:/bin", "--setenv", "SHELL", "/bin/sh", "--setenv", "HOME"])
            .arg(&home)
            .args(["--setenv", "XDG_RUNTIME_DIR"])
            .arg(&runtime)
            .args(["--setenv", "XDG_STATE_HOME"])
            .arg(dir.join("state"))
            .args(["--setenv", "REX_SERVER_SOCKET"])
            .arg(&sock)
            .arg("--")
            .arg(rex)
            .args(["-S", &addr, "server", "run", "--listen", &addr, "--state-dir"])
            .arg(dir.join("rex-state"))
            .arg("--restore=false")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(dir.join("server.log"))?)
            .spawn()?;
        let mut server = Self { child, dir, sock };
        timeout(Duration::from_secs(5), async {
            loop {
                if tokio::net::UnixStream::connect(&server.sock).await.is_ok() {
                    return Ok::<(), std::io::Error>(());
                }
                if let Some(status) = server.child.try_wait()? {
                    return Err(std::io::Error::other(format!("scratch Rex exited {status}: {}", fs::read_to_string(server.dir.join("server.log"))?)));
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .map_err(|_| "scratch Rex did not start in five seconds")??;
        Ok(Some(server))
    }

    pub async fn connect(&self) -> Result<(ControlClient, Events)> {
        let stream = tokio::net::UnixStream::connect(&self.sock).await?;
        Ok(ControlClient::connect_unix(stream, ClientIdentity::olo_rex(None)).await?)
    }

    pub async fn session(&self, ctrl: &ControlClient, label: &str) -> Result<(String, String, String)> {
        let r = ctrl.session_create(Some(label), Some(json!({"command":["/bin/sh"],"cwd": self.dir.join("home")}))).await?.payload;
        Ok((r["session_id"].as_str().unwrap().into(), r["initial_windows"][0]["window_id"].as_str().unwrap().into(), r["initial_windows"][0]["block_ids"][0].as_str().unwrap().into()))
    }
}
