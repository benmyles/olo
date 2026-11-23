//! Wire contracts against the installed server. Each test owns its socket,
//! process tree, home, and network namespace (including embedded Tailscale).
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

use olo_rex_client::*;
use serde_json::{Value, json};
use tokio::{
    sync::mpsc,
    time::{sleep, timeout},
};
use uuid::Uuid;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
type Events = mpsc::UnboundedReceiver<ControlEventEnvelope>;

struct Scratch {
    child: Child,
    dir: PathBuf,
    sock: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

impl Scratch {
    async fn start() -> Result<Option<Self>> {
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

    async fn connect(&self) -> Result<(ControlClient, Events)> {
        let stream = tokio::net::UnixStream::connect(&self.sock).await?;
        Ok(ControlClient::connect_unix(stream, ClientIdentity::olo_rex(None)).await?)
    }

    async fn session(&self, ctrl: &ControlClient, label: &str) -> Result<(String, String, String)> {
        let r = ctrl.session_create(Some(label), Some(json!({"command":["/bin/sh"],"cwd": self.dir.join("home")}))).await?.payload;
        Ok((r["session_id"].as_str().unwrap().into(), r["initial_windows"][0]["window_id"].as_str().unwrap().into(), r["initial_windows"][0]["block_ids"][0].as_str().unwrap().into()))
    }
}

async fn event(events: &mut Events, predicate: impl Fn(&ControlEvent) -> bool) -> Result<ControlEvent> {
    Ok(timeout(Duration::from_secs(4), async {
        while let Some(ev) = events.recv().await {
            if predicate(&ev.event) {
                return Ok(ev.event);
            }
        }
        Err("event stream closed")
    })
    .await
    .map_err(|_| "expected Rex event did not arrive")??)
}

#[tokio::test]
async fn terminal_snapshot_and_bytes_roundtrip() -> Result {
    let Some(server) = Scratch::start().await? else { return Ok(()) };
    let (ctrl, _events) = server.connect().await?;
    let ep = RexEndpoint::Unix(server.sock.clone());
    let test_file = server.dir.join("sample.txt");
    fs::write(&test_file, b"hello from olo rex download test\n")?;
    let created = ctrl.session_create(Some("it-test"), Some(json!({"shell":"none","command":["/bin/sh"],"exit":{"on_completion":false}}))).await?;
    let sid = created.payload["session_id"].as_str().unwrap().to_owned();
    let bid = created.payload["initial_windows"][0]["block_ids"][0].as_str().unwrap().to_owned();

    let boot = ctrl
        .session_bootstrap(
            "all",
            Some(&sid),
            None,
            &[DataTicketRequest { creator_name: "com.superlogical.terminal".into(), data_ref_id: "terminal".into(), subprotocol: Some(DATA_TERMINAL_SUBPROTOCOL.into()) }],
        )
        .await?;
    let ticket = boot.payload.data_tickets.as_ref().and_then(|t| t.first()).map(|t| t.ticket.clone()).expect("bootstrap data ticket");

    let (dstream, dhost) = ep.connect_stream().await?;
    let mut term_ws = TerminalDataStream::connect_hello(dstream, &dhost, &ticket).await?;
    assert_ne!(term_ws.welcome.channel_nonce, 0);

    // Receive initial GHOSTSNP snapshot and verify live bytes arrive
    let mut snap_buf = Vec::new();
    let mut got_ghostsnp = false;
    for _ in 0..10 {
        match tokio::time::timeout(std::time::Duration::from_secs(2), term_ws.next_frame()).await {
            Ok(Ok(TerminalStreamFrame::Bytes(b))) => {
                snap_buf.extend_from_slice(&b);
                if snap_buf.starts_with(b"GHOSTSNP") {
                    got_ghostsnp = true;
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(got_ghostsnp, "expected GHOSTSNP header on terminal data stream");

    // Verify rex.data.bytes.v1 download with 0-byte half-close
    let dl = ctrl.terminal_download_begin(&sid, &bid, &format!("download:{}", uuid::Uuid::new_v4()), test_file.to_str().unwrap(), None).await?;
    let dl_ticket = dl.payload.ticket.expect("download ticket");
    let mut bws = open_bytes_stream(&ep, &dl_ticket).await?;
    let mut downloaded = Vec::new();
    while let Some(chunk) = bws.read_chunk().await? {
        downloaded.extend_from_slice(&chunk);
    }
    assert_eq!(downloaded, b"hello from olo rex download test\n");

    ctrl.terminal_resize(&sid, &bid, 92, 33, Some(7), Some(16)).await?;
    ctrl.terminal_set_theme(&sid, &bid, "#ffffff", "#000000", None, &["1=ff1122".into()], "dark").await?;
    ctrl.terminal_write(&sid, &bid, "exit 7\n").await?;
    let mut demux = InlineApcDemuxer::new(term_ws.welcome.channel_nonce);
    let (mut resized, mut themed) = (false, false);
    timeout(Duration::from_secs(4), async {
        loop {
            let TerminalStreamFrame::Bytes(bytes) = term_ws.next_frame().await? else {
                return Err(RexError::Closed("expected terminal exit report".into()));
            };
            for item in demux.push(&bytes) {
                match item {
                    DemuxItem::Apc(InlineApc::Resize { cols: 92, rows: 33, cell_width_px: 7, .. }) => resized = true,
                    DemuxItem::Apc(InlineApc::Theme { scheme, .. }) if scheme.as_deref() == Some("dark") => themed = true,
                    DemuxItem::Apc(InlineApc::ChildExited { exit_code: 7 }) => return Ok(()),
                    _ => {}
                }
            }
        }
    })
    .await??;
    assert!(resized && themed);
    assert_eq!(ctrl.terminal_process(&sid, &bid).await?.payload.last_exit.unwrap().exit_code, 7);
    term_ws.close_clean().await;

    ctrl.session_destroy(&sid).await?;
    Ok(())
}

#[tokio::test]
async fn server_hosts_tailscale_and_stop() -> Result {
    let Some(server) = Scratch::start().await? else { return Ok(()) };
    let (ctrl, mut events) = server.connect().await?;
    assert_eq!(ctrl.api_revision(), REX_API_REVISION);
    assert!(ctrl.client_id().is_some() && ctrl.reconnect_token().is_some());
    let boot = ctrl.session_bootstrap("all", None, Some(&[]), &[]).await?.payload;
    assert!(boot.attach_all.sessions.is_empty());
    assert_eq!(boot.server_status.tailscale.unwrap().state, "disabled");
    let version = ctrl.server_version().await?.payload;
    assert!(!version.version.is_empty());
    assert_eq!(version.api_revision, 19);
    assert!(ctrl.server_config_reload().await?.payload["generation"].is_u64());
    event(&mut events, |e| matches!(e, ControlEvent::KeymapChanged { .. })).await?;

    let host = HostRecord { label: "Scratch".into(), endpoints: vec!["example.invalid:7000".into()], ..Default::default() };
    let put = ctrl.hosts_put(&host).await?.payload;
    let id = put["host"]["id"].as_str().unwrap();
    let list = ctrl.hosts_list().await?.payload;
    assert_eq!(list.hosts[0].id.as_deref(), Some(id));
    event(&mut events, |e| matches!(e, ControlEvent::HostsChanged{ hash } if hash == &list.hash)).await?;
    assert_eq!(ctrl.hosts_remove(id).await?.payload["removed"], true);

    assert!(matches!(ctrl.tailscale_enable(None,Some(false),Some(&["tag:audit".into()]),None).await,Err(RexError::Api(e)) if e.code=="bad_request"));
    let enabled = ctrl.tailscale_enable(Some("rex-audit"), Some(true), Some(&["tag:audit".into()]), None).await?.payload;
    assert!(matches!(enabled.state.as_str(), "starting" | "needs_login"));
    assert!(enabled.allow_tailnet_members);
    let ready = event(&mut events, |e| matches!(e, ControlEvent::TailscaleStatusChanged(s) if s.state == "needs_login")).await?;
    let ControlEvent::TailscaleStatusChanged(ready) = ready else { unreachable!() };
    assert!(ready.allow_tailnet_members);
    assert_eq!(ctrl.tailscale_login().await?.payload.state, "needs_login");
    assert_eq!(ctrl.tailscale_rename("rex-renamed").await?.payload.state, "needs_login");
    assert_eq!(ctrl.tailscale_logout().await?.payload.state, "disabled");
    assert_eq!(ctrl.tailscale_disable().await?.payload.state, "disabled");
    assert_eq!(ctrl.server_status().await?.payload.tailscale.unwrap().state, "disabled");
    ctrl.server_stop().await?;
    timeout(Duration::from_secs(3), async { while events.recv().await.is_some() {} }).await?;
    assert!(matches!(ctrl.server_status().await, Err(RexError::Closed(_))));
    Ok(())
}

#[tokio::test]
async fn workspace_layout_and_ownership() -> Result {
    let Some(server) = Scratch::start().await? else { return Ok(()) };
    let (ctrl, mut events) = server.connect().await?;
    ctrl.session_bootstrap("all", None, Some(&[]), &[]).await?;
    let (s, w, b) = server.session(&ctrl, "First").await?;
    let (other, _, _) = server.session(&ctrl, "Another").await?;
    ctrl.session_move(&other, Some(&s)).await?;
    assert_eq!(ctrl.session_list().await?.payload[0].session_id, other);
    let attached = ctrl.session_attach(&s).await?.payload;
    assert_eq!(attached.view.active_window().unwrap().window_id, w);
    ctrl.session_set_label(&s, "Renamed").await?;
    let w2 = ctrl.session_new_window(&s, Some("Two"), None).await?.payload["window_id"].as_str().unwrap().to_owned();
    ctrl.session_set_window_label(&s, &w, "One").await?;
    ctrl.session_move_window(&s, &w2, Some(&w)).await?;
    ctrl.session_focus_previous_window(&s).await?;
    ctrl.session_focus_next_window(&s).await?;

    // The anchor is in the other window. Ignoring it silently splits w2.
    let r = ctrl.session_new_split(&s, Some(&b), "down", None).await?.payload;
    assert_eq!(r["anchor_id"], b);
    let b2 = r["block_ids"][0].as_str().unwrap();
    let split_id = r["split_ids"][0].as_str().unwrap();
    ctrl.session_resize_pane(&s, split_id, 0.64).await?;
    let view = ctrl.session_view(&s).await?.payload;
    let (window, layer, _) = view.find_block(b2).unwrap();
    assert_eq!(window.window_id, w);
    let split = layer.layout.as_ref().unwrap().split.as_ref().unwrap();
    assert_eq!(split.ratio, 0.64);
    assert_eq!(split.direction, SplitDirection::Vertical);
    assert_eq!(split.after.block_id.as_deref(), Some(b2));
    ctrl.session_focus_window(&s, &w).await?;
    ctrl.session_set_block_label(&s, b2, "Split").await?;
    ctrl.session_focus_block(&s, &b).await?;
    ctrl.session_focus_direction(&s, "down").await?;
    ctrl.session_focus_previous_pane(&s).await?;
    ctrl.session_focus_next_pane(&s).await?;
    let history = ctrl.focus_history_reverse(&s, None).await?.payload;
    assert_eq!(history["reversed"], true);
    ctrl.focus_history_reverse(&s, history["cursor"].as_u64()).await?;
    ctrl.session_zoom_block(&s, Some(b2), Some(true)).await?;
    ctrl.session_zoom_block(&s, Some(b2), Some(false)).await?;
    ctrl.session_swap_blocks(&s, &b, b2).await?;
    ctrl.session_move_block(&s, b2, &b, "left").await?;
    for r in [ctrl.session_own_block(&s, &b).await?, ctrl.session_disown_block(&s, &b).await?] {
        assert_eq!(r.payload["blocks"].as_array().unwrap().len(), 1);
        assert_eq!(r.payload["blocks"][0]["block_id"], b);
    }
    let floated = ctrl.session_float_block(&s, b2, Some([0.1, 0.1, 0.6, 0.5])).await?.payload;
    let layer = ctrl.session_new_layer(&s, Some(&w), [0.1, 0.1, 0.4, 0.4], None).await?.payload;
    let layer_id = layer["layer_id"].as_str().unwrap();
    ctrl.session_resize_layer(&s, layer_id, [0.2, 0.2, 0.4, 0.4]).await?;
    ctrl.session_lower_layer(&s, layer_id).await?;
    ctrl.session_raise_layer(&s, layer_id).await?;
    ctrl.session_set_layer_state(&s, layer_id, "minimized", false).await?;
    let v = ctrl.session_view(&s).await?.payload;
    let l = v.find_block(layer["block_ids"][0].as_str().unwrap()).unwrap().1;
    assert_eq!((l.layer_kind(), l.layer_state()), (LayerKind::Floating, LayerState::Minimized));
    ctrl.session_close_layer(&s, layer_id).await?;
    ctrl.session_close_layer(&s, floated["layer_id"].as_str().unwrap()).await?;
    ctrl.session_close_window(&s, Some(&w2)).await?;
    ctrl.session_detach(&s).await?;
    ctrl.session_destroy(&s).await?;
    ctrl.session_destroy(&other).await?;
    let mut kinds = std::collections::BTreeSet::new();
    while let Some(e) = timeout(Duration::from_secs(3), events.recv()).await? {
        let name = match e.event {
            ControlEvent::SessionCreated(_) => "created",
            ControlEvent::SessionDestroyed { session_id } if session_id == other => break,
            ControlEvent::SessionMoved { .. } => "moved",
            ControlEvent::SessionViewChanged { revision, .. } if revision > 0 => "view",
            ControlEvent::SessionLabelChanged { label, .. } if label == "Renamed" => "label",
            ControlEvent::ActiveWindowChanged { window_id: Some(_), .. } => "active",
            ControlEvent::WindowCreated { .. } => "window created",
            ControlEvent::WindowClosed { .. } => "window closed",
            ControlEvent::WindowLabelChanged { label, .. } if label == "One" => "window label",
            ControlEvent::BlockCreated { .. } => "block created",
            ControlEvent::BlockClosed { .. } => "block closed",
            ControlEvent::BlockOwnerChanged { .. } => "owner",
            // (A client beginning to follow the session: this test's own.)
            ControlEvent::SessionClientConnected { session_id, .. } if session_id == s => "client connected",
            _ => continue,
        };
        kinds.insert(name);
    }
    assert_eq!(
        kinds,
        std::collections::BTreeSet::from([
            "created",
            "moved",
            "view",
            "label",
            "active",
            "window created",
            "window closed",
            "window label",
            "block created",
            "block closed",
            "owner",
            "client connected"
        ])
    );
    Ok(())
}

#[tokio::test]
async fn terminal_files_status_and_block_creators() -> Result {
    let Some(server) = Scratch::start().await? else { return Ok(()) };
    let (ctrl, mut events) = server.connect().await?;
    let (s, _, b) = server.session(&ctrl, "Terminal").await?;
    ctrl.session_bootstrap("all", Some(&s), None, &[]).await?;
    ctrl.terminal_resize(&s, &b, 100, 30, Some(8), Some(16)).await?;
    ctrl.terminal_set_theme(&s, &b, "#eeeeee", "#111111", Some("#eeeeee"), &["1=ff0000".into()], "dark").await?;
    assert!(ctrl.data_connect(&s, &b, "terminal", DATA_TERMINAL_SUBPROTOCOL).await?.payload.expires_in_ms > 0);
    ctrl.terminal_reset(&s, &b).await?;
    ctrl.terminal_clear(&s, &b).await?;
    assert_eq!(ctrl.terminal_ports(&s, &b).await?.payload.status, "ok");
    assert!(ctrl.terminal_process(&s, &b).await?.payload.child.is_some());
    ctrl.terminal_input_mode(&s, &b).await?;
    // Clipboard storage and status updates run independently on the server.
    // Read each value only after its own notification.
    ctrl.terminal_write(&s, &b, "printf '\\033]52;c;YXVk\\007'\n").await?;
    let clipboard = event(&mut events, |e| matches!(e, ControlEvent::BlockEvent { block_id, name, .. } if block_id == &b && name == "clipboard_written")).await?;
    let ControlEvent::BlockEvent { payload, .. } = clipboard else { unreachable!() };
    let id = payload["id"].as_u64().expect("stored clipboard id");
    assert_eq!(ctrl.terminal_clipboard_read(&s, &b, Some(id), 0).await?.payload.data, "aud");
    let cmd = "printf 'verified\\n\\033]7501;state=working\\007'\n";
    assert_eq!(ctrl.terminal_write(&s, &b, cmd).await?.payload["bytes_written"], cmd.len());
    event(&mut events, |e| matches!(e, ControlEvent::BlockEvent { name, payload, .. } if name == "program_status_changed" && payload["record"]["state"] == "working")).await?;
    assert_eq!(ctrl.terminal_program_status(&s, &b).await?.payload[0].priority(), ProgramPriority::Working);
    let formatted = ctrl.terminal_format(&s, &b, "text").await?.payload;
    assert!(formatted.contains("verified"));
    // A smaller negotiated budget requests a prefix of this same screen and
    // follows its blob ticket. Splitting on byte offsets must preserve it.
    let mut small = ctrl.clone();
    small.max_message_bytes = 1040;
    assert_eq!(small.terminal_format(&s, &b, "text").await?.payload, formatted);
    let first: RpcResult<Value> = ctrl.call_block("com.superlogical.terminal.format", &s, &b, Some(json!({"format":"text","max_bytes":1}))).await?;
    let rest = ctrl.terminal_blob_read(&s, &b, first.payload["id"].as_u64().unwrap(), first.payload["content"].as_str().unwrap().len()).await?.payload;
    assert!(rest.eof && rest.data.contains("verified"));
    assert!(matches!(ctrl.terminal_blob_read(&s,&b,u32::MAX.into(),0).await,Err(RexError::Api(e)) if e.code=="not_found"));
    let f = server.dir.join("fixture.txt");
    fs::write(&f, "file bytes")?;
    let list = ctrl.terminal_list_dir(&s, &b, server.dir.to_str().unwrap(), Some("fixture"), true).await?.payload;
    assert_eq!(list.entries[0].name, "fixture.txt");
    assert!(list.entries[0].file);
    assert_eq!(ctrl.terminal_list_dir(&s, &b, server.dir.join("absent").to_str().unwrap(), None, true).await?.payload.status, "missing");
    let upload = ctrl.terminal_upload_begin(&s, &b, &format!("upload:{}", Uuid::new_v4()), "fixture.txt", 4, Some(("two", 0, 2))).await?.payload;
    assert!(!upload.ticket.is_empty() && upload.expires_in_ms > 0);
    for (flavor, options) in [("com.superlogical.tunnel.dial", json!({"host":"127.0.0.1","port":1})), ("com.superlogical.onboarding.onboarding", json!({}))] {
        let block = ctrl.session_new_block(&s, flavor, Some(options), Some("Detached")).await?.payload;
        let id = block["block_id"].as_str().unwrap();
        if flavor.ends_with(".dial") {
            let info = ctrl.tunnel_info(&s, id).await?.payload;
            assert_eq!(info.target.as_deref(), Some("127.0.0.1:1"));
            assert_eq!(info.connections, 0);
        } else {
            assert_eq!(ctrl.onboarding_state(&s, id).await?.payload, json!({"generation":0}));
            assert_eq!(ctrl.onboarding_set_step(&s, id, "next").await?.payload["step"], "next");
            // The valid request fits the negotiated limit. Wrapping the step in
            // a session event takes more room; Rex sends event_dropped instead.
            let overhead =
                serde_json::to_vec(&json!({"type":"request","id":"99999","method":"com.superlogical.onboarding.set_step","payload":{"session_id":s,"block_id":id,"args":{"step":""}}}))?.len();
            ctrl.onboarding_set_step(&s, id, &"a".repeat(ctrl.max_message_bytes - overhead)).await?;
            event(&mut events, |e| matches!(e, ControlEvent::EventDropped { event_type, block_id: Some(b), block_event_name, .. } if event_type == "block_event" && b == id && block_event_name.as_deref() == Some("step_changed"))).await?;
        }
        ctrl.block_close(&s, id).await?;
    }
    ctrl.session_destroy(&s).await?;
    Ok(())
}

#[tokio::test]
async fn keymap_prompt_perform_and_callback() -> Result {
    let Some(server) = Scratch::start().await? else { return Ok(()) };
    let (ctrl, mut ev) = server.connect().await?;
    ctrl.session_bootstrap("all", None, Some(&[]), &[]).await?;
    let (other, mut other_ev) = server.connect().await?;
    event(&mut ev, |e| matches!(e, ControlEvent::ClientJoined(c) if Some(c.client_id.as_str()) == other.client_id())).await?;
    other.client_set_label("Other").await?;
    event(&mut ev, |e| matches!(e, ControlEvent::ClientChanged(c) if c.info.as_ref().and_then(|i| i.label.as_deref()) == Some("Other"))).await?;
    assert_eq!(ctrl.client_list().await?.payload.len(), 2);
    let actions = [KeymapAction { name: "audit.perform".into(), kind: Some("client".into()), title: Some("Perform".into()), ..Default::default() }];
    let binding = KeymapBinding { keys: vec!["ctrl+g".into()], action: "audit.perform".into(), ..Default::default() };
    let defaults = [KeymapLayer { name: "Default".into(), bindings: vec![binding.clone()], ..Default::default() }];
    let overrides = [KeymapLayer {
        name: "Custom".into(),
        bindings: vec![KeymapBinding { keys: vec!["ctrl+b".into()], ..binding }],
        removals: vec![KeymapRemoval { keys: vec!["ctrl+g".into()], ..Default::default() }],
    }];
    let keymap = ctrl.client_keymap(&actions, &defaults, &overrides).await?.payload;
    assert!(keymap.actions.unwrap().iter().any(|a| a.name == "com.superlogical.terminal.reset"));
    assert!(keymap.bindings.iter().any(|b| b.keys == ["ctrl+b"]));
    let (s, _, b) = server.session(&ctrl, "Keys").await?;
    ctrl.session_attach(&s).await?;
    let key = KeyEvent { key: "key_g".into(), mods: "ctrl".into(), consumed_mods: "".into(), text: "".into(), unshifted_codepoint: 103, composing: false, action: "press".into() };
    ctrl.client_key(&s, Some(&b), &key, None).await?;
    let dir = server.dir.join("home/.config/rex");
    fs::create_dir_all(&dir)?;
    fs::write(
        dir.join("init.lua"),
        r#"
rex.action{name="queue", title="Queue", run=function() rex.client.queue("audit.perform", {value="scratch"}) end}
rex.action{name="ask", title="Audit", run=function() return rex.ui.pick{ title="Audit", items={{id="one",title="One"}} } end}
rex.action{name="lookup", title="Lookup", run=function() return rex.server("scratch").call("session.list", {}) end}
"#,
    )?;
    assert_eq!(ctrl.server_config_reload().await?.payload["diagnostics"], json!([]));
    let queued = ctrl.client_action("queue", None, "api", None, None, None, None).await?.payload.client_actions;
    assert_eq!(queued[0].action, "audit.perform");
    assert_eq!(queued[0].args.as_ref().unwrap()["value"], "scratch");
    let (result, answered) = tokio::join!(ctrl.client_action("ask", None, "api", None, None, None, None), async {
        let ControlEvent::ClientPromptRequested(p) = event(&mut ev, |e| matches!(e, ControlEvent::ClientPromptRequested(_))).await? else { unreachable!() };
        assert_eq!(p.prompt.items.unwrap()[0].id, "one");
        let ControlEvent::ClientPromptRequested(p2) = event(&mut other_ev, |e| matches!(e, ControlEvent::ClientPromptRequested(_))).await? else { unreachable!() };
        ctrl.client_reply_ok(&p.call_id, json!({"ids":["one"]})).await?;
        event(&mut other_ev, |e| matches!(e, ControlEvent::ClientPromptEnded(q) if q.call_id == p2.call_id && q.reason == "answered")).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    });
    answered?;
    assert_eq!(result?.payload.result, Some(json!("one")));
    let servers = [("scratch".into(), "local-stub".into())];
    let (result, answered) = tokio::join!(ctrl.client_action("lookup", None, "api", None, None, Some(&servers), None), async {
        let ControlEvent::ClientCallRequested(p) = event(&mut ev, |e| matches!(e, ControlEvent::ClientCallRequested(_))).await? else { unreachable!() };
        assert_eq!(p.server, "local-stub");
        assert_eq!(p.method, "session.list");
        ctrl.client_reply_ok(&p.call_id, json!({"count":0})).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    });
    answered?;
    assert_eq!(result?.payload.result, Some(json!({"count":0})));
    let (result, answered) = tokio::join!(other.call_raw("client.perform", Some(json!({"client_id":ctrl.client_id(),"action":"audit.perform"}))), async {
        let ControlEvent::ClientPerformRequested(p) = event(&mut ev, |e| matches!(e, ControlEvent::ClientPerformRequested(_))).await? else { unreachable!() };
        assert_eq!(Some(p.requester.client_id.as_str()), other.client_id());
        ctrl.client_reply_err(&p.call_id, "refused", "scratch").await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    });
    answered?;
    assert!(matches!(result, Err(RexError::Api(e)) if e.code == "refused" && e.status == 403));
    other.close();
    event(&mut ev, |e| matches!(e, ControlEvent::ClientLeft { client_id } if Some(client_id.as_str()) == other.client_id())).await?;
    Ok(())
}

#[tokio::test]
async fn loss_releases_every_pending_caller() -> Result {
    let Some(mut server) = Scratch::start().await? else { return Ok(()) };
    let (ctrl, mut events) = server.connect().await?;
    ctrl.client_keymap(&[KeymapAction { name: "audit.wait".into(), kind: Some("client".into()), ..Default::default() }], &[], &[]).await?;
    let asks = |c: ControlClient, id: String| tokio::spawn(async move { c.call_raw("client.perform", Some(json!({"client_id":id,"action":"audit.wait","timeout_ms":30000}))).await });
    let one = asks(ctrl.clone(), ctrl.client_id().unwrap().into());
    let two = asks(ctrl.clone(), ctrl.client_id().unwrap().into());
    event(&mut events, |e| matches!(e, ControlEvent::ClientPerformRequested(_))).await?;
    server.child.kill()?;
    timeout(Duration::from_secs(2), async {
        for task in [one, two] {
            assert!(matches!(task.await.unwrap(), Err(RexError::Closed(_))));
        }
        while events.recv().await.is_some() {}
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn glance_follows_requests_socket_and_agent_events() -> Result {
    use olo_protocol::glance::{self, Request, Requests, Timeline};
    let Some(server) = Scratch::start().await? else { return Ok(()) };
    let dir = server.dir.join("deskbar");
    fs::create_dir(&dir)?;
    let requests = glance::requests_path(&dir, "rex");
    let widget = |id| Request { widget: id, kind: "agents".into(), options: Default::default(), size: Default::default() };
    glance::write_json(&requests, &Requests { widgets: vec![widget(7)] })?;
    let socket = server.dir.join("new-listener");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_olo-rex-glance"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", server.dir.join("home"))
        .env("XDG_RUNTIME_DIR", server.dir.join("run"))
        .env("XDG_STATE_HOME", server.dir.join("state"))
        .env("REX_SERVER_SOCKET", &socket)
        .env(glance::DIR_ENV, &dir)
        .kill_on_drop(true)
        .spawn()?;
    async fn wait_title(path: PathBuf, title: &str) -> Result {
        timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(bytes) = fs::read(&path)
                    && let Ok(t) = serde_json::from_slice::<Timeline>(&bytes)
                    && t.entries.last().is_some_and(|e| e.glance.title == title)
                {
                    return;
                }
                sleep(Duration::from_millis(15)).await;
            }
        })
        .await
        .map_err(|_| format!("Glance did not publish {title:?} in {}", path.display()))?;
        Ok(())
    }
    wait_title(glance::content_path(&dir, 7), "Rex Server Offline").await?;
    std::os::unix::fs::symlink(&server.sock, &socket)?;
    wait_title(glance::content_path(&dir, 7), "No active sessions").await?;
    let (ctrl, _events) = server.connect().await?;
    let (s, _, b) = server.session(&ctrl, "Glance").await?;
    ctrl.session_attach(&s).await?;
    wait_title(glance::content_path(&dir, 7), "1 session").await?;
    ctrl.terminal_write(&s, &b, "printf '\\033]7501;state=blocked\\007'\n").await?;
    wait_title(glance::content_path(&dir, 7), "1 waiting").await?;
    glance::write_json(&requests, &Requests { widgets: vec![widget(8)] })?;
    wait_title(glance::content_path(&dir, 8), "1 waiting").await?;
    ctrl.server_stop().await?;
    wait_title(glance::content_path(&dir, 8), "Rex Server Offline").await?;
    glance::write_json(&requests, &Requests::default())?;
    assert!(timeout(Duration::from_secs(2), child.wait()).await??.success());
    Ok(())
}

/// What a program copies by OSC 52 is told to the clients: with its
/// text where it is short, by its number and size alone where it is
/// long, to be read a piece at a time (the app does: hub.rs,
/// `clipboard_written`).
#[tokio::test]
async fn what_a_program_copies_is_told_short_and_read_long() -> Result {
    let Some(server) = Scratch::start().await? else { return Ok(()) };
    let (ctrl, mut events) = server.connect().await?;
    let (sid, _, bid) = server.session(&ctrl, "clip").await?;
    ctrl.session_bootstrap("all", Some(&sid), None, &[]).await?;
    for len in [15usize, 300_003] {
        // (Base64 of so many "a": "YWFh" for every three.)
        ctrl.terminal_write(&sid, &bid, &format!("printf '\\033]52;c;%s\\a' {}\n", "YWFh".repeat(len / 3))).await?;
        let ControlEvent::BlockEvent { payload, .. } = event(&mut events, |e| matches!(e, ControlEvent::BlockEvent { name, .. } if name == "clipboard_written")).await? else { unreachable!() };
        assert_eq!(payload["bytes"].as_u64(), Some(len as u64), "{payload}");
        let id = payload["id"].as_u64().expect("its number");
        let mut text = String::new();
        loop {
            let piece = ctrl.terminal_clipboard_read(&sid, &bid, Some(id), text.len()).await?.payload;
            assert_eq!((piece.id, piece.total), (id, len));
            text.push_str(&piece.data);
            if piece.eof {
                break;
            }
            assert!(!piece.data.is_empty());
        }
        assert!(text.len() == len && text.bytes().all(|b| b == b'a'));
        match payload.get("text").and_then(|t| t.as_str()) {
            Some(said) => assert_eq!(said, text, "what is short comes with the word of it"),
            None => assert!(len > 4096, "only what is long comes without its text: {len}"),
        }
    }
    Ok(())
}
