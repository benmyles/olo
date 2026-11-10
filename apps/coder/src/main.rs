mod client;
mod form;
mod gui;
mod transcript;
mod tui;
use anyhow::{Context, Result};
use olo_coder_core::{Command, rpc};
use std::{io::Write, path::PathBuf};
fn main() {
    olo_crash::install();
    if let Err(e) = run() {
        eprintln!("olo-coder: {e:#}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut dir = rpc::data_dir();
    let mut cwd = std::env::current_dir()?;
    let mut session = None;
    let mut mode = if std::io::IsTerminal::is_terminal(&std::io::stdin()) { "tui" } else { "gui" };
    let mut payload = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                println!(
                    "Olo Coder\n\nolo-coder [--gui | --tui] [--workspace DIR] [--session ID]\nolo-coder --rpc JSON\nolo-coder --events ID [--after SEQUENCE]\nolo-coder --exec PROMPT [--session ID]\nolo-coder --service\n\n--data DIR overrides OLO_CODER_HOME. With no session, a new one is created.\nThe service survives closing either interface. /help lists shared commands.\nUse --rpc '{{\"op\":\"shutdown\"}}' to stop the service deliberately."
                );
                return Ok(());
            }
            "--gui" => mode = "gui",
            "--tui" => mode = "tui",
            "--service" => mode = "service",
            "--data" | "--workspace" | "--session" | "--rpc" | "--events" | "--exec" | "--after" => {
                let key = args[i].as_str();
                i += 1;
                let value = args.get(i).context("option needs a value")?;
                match key {
                    "--data" => dir = value.into(),
                    "--workspace" => cwd = PathBuf::from(value),
                    "--session" => session = Some(value.clone()),
                    "--rpc" => {
                        mode = "rpc";
                        payload = Some(value.clone());
                    }
                    "--events" => {
                        mode = "events";
                        session = Some(value.clone());
                    }
                    "--exec" => {
                        mode = "exec";
                        payload = Some(value.clone());
                    }
                    "--after" => payload = Some(value.clone()),
                    _ => {}
                }
            }
            other => anyhow::bail!("unknown option {other}; use --help"),
        };
        i += 1;
    }
    if mode == "service" {
        let (upgrade, signal) = tokio::sync::oneshot::channel();
        olo_kit_base::restore::on_upgrade(move || {
            olo_kit_base::restore::mark_waiting(true);
            let _ = upgrade.send(());
        });
        let upgraded = tokio::runtime::Builder::new_multi_thread().enable_all().worker_threads(4).build()?.block_on(rpc::serve_with_upgrade(dir, Some(signal)));
        olo_kit_base::restore::mark_waiting(false);
        if upgraded? {
            return Err(olo_kit_base::restore::exec_new_version().into());
        }
        return Ok(());
    }
    if !matches!(mode, "gui" | "tui") {
        ensure_service(&dir)?;
    }
    match mode {
        "gui" => gui::run(dir, cwd, session),
        "tui" => tui::run(dir, cwd, session)?,
        "rpc" => {
            let command: Command = serde_json::from_str(payload.as_deref().context("missing command")?)?;
            println!("{}", serde_json::to_string_pretty(&rpc::call(&dir, &command)?)?);
        }
        "events" => rpc::follow(&dir, &session.context("missing session")?, payload.and_then(|s| s.parse().ok()).unwrap_or(0), |v| {
            println!("{v}");
            true
        })?,
        "exec" => {
            let session = match session {
                Some(s) => s,
                None => rpc::call(&dir, &Command::Create { workspace: cwd, config: None })?["id"].as_str().context("no session id")?.into(),
            };
            let cursor = rpc::call(&dir, &Command::Cursor { session: session.clone() })?["after"].as_i64().unwrap_or(0);
            rpc::call(&dir, &Command::Send { session: session.clone(), text: payload.context("missing prompt")? })?;
            let mut stopped = None;
            rpc::follow(&dir, &session, cursor, |v| {
                println!("{v}");
                let _ = std::io::stdout().flush();
                match v["kind"].as_str() {
                    Some("idle") => false,
                    Some(kind @ ("error" | "paused" | "approval")) => {
                        stopped = Some(format!("session {session}: {kind}: {}", v["data"]));
                        false
                    }
                    _ => true,
                }
            })?;
            if let Some(reason) = stopped {
                anyhow::bail!("{reason}");
            }
        }
        _ => {}
    }
    Ok(())
}
fn ensure_service(dir: &std::path::Path) -> Result<()> {
    if std::os::unix::net::UnixStream::connect(rpc::socket(dir)).is_ok() {
        return Ok(());
    }
    use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
    std::fs::create_dir_all(dir)?;
    let log = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(dir.join("service.log"))?;
    let exe = std::env::current_exe()?;
    let exe = PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--service").arg("--data").arg(dir).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(log);
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    // Startup only, bounded. There is no timer once connected.
    for _ in 0..1500 {
        if std::os::unix::net::UnixStream::connect(rpc::socket(dir)).is_ok() {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            anyhow::bail!("service exited {status}; see {}/service.log", dir.display());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    anyhow::bail!("service did not start; see {}/service.log", dir.display())
}
