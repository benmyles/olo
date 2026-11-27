//! The app's side of a tab's children (`olo agent`), against a Rex
//! server of the test's own and the host's agents service as the app
//! carries it (`olo-rex-link agent`): children started **before any
//! desktop was there** are shown the moment the app connects; one
//! started meanwhile appears; what a child may ask of this computer is
//! the service's to say; `rex do olo.agent.…` is passed to the service;
//! and the window's Stop asks it.
use super::*;
use std::process::{Child, Command, Stdio};
use tokio::time::{sleep, timeout};

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn rex(sock: &Path, method: &str, payload: Value) -> Value {
    let out = Command::new("/usr/bin/rex").args(["-S", &format!("unix://{}", sock.display()), "api", "call", method, &payload.to_string()]).output().unwrap();
    assert!(out.status.success(), "rex api call {method}: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

/// `olo agent ARGS` on this host, from a pane: no app in it at all.
fn olo_agent(helper: &Path, block: &str, args: &[&str]) -> Value {
    let out = Command::new(helper).arg("agent").args(args).env("REX_BLOCK", block).output().unwrap();
    assert!(out.status.success(), "olo agent {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

async fn until<T>(what: &str, mut read: impl FnMut() -> Option<T>) -> T {
    timeout(Duration::from_secs(15), async {
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

#[test]
fn the_app_shows_and_asks_what_a_hosts_service_says() {
    const CASE: &str = "OLO_REX_AGENTS_CASE";
    if !Path::new("/usr/bin/rex").is_file() {
        eprintln!("skipping: /usr/bin/rex unavailable");
        return;
    }
    // (Run again in an account of its own: a home, a runtime folder and
    // a Rex server that are nobody's but this test's.)
    if std::env::var_os(CASE).is_none() {
        let dir = PathBuf::from(format!("/var/tmp/rx-agents-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&dir).unwrap();
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "hub::agents_tests::the_app_shows_and_asks_what_a_hosts_service_says", "--nocapture"])
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
            .env("OLO_REX_AGENTS_DIR", dir.join("agents"))
            .env(CASE, "1")
            .current_dir(&dir);
        if let Some(path) = std::env::var_os("LD_LIBRARY_PATH") {
            cmd.env("LD_LIBRARY_PATH", path);
        }
        let output = cmd.output().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    let dir = PathBuf::from(std::env::var_os("HOME").unwrap());
    let sock = dir.join("rex.sock");
    let at = format!("unix://{}", sock.display());
    let _server = Server(
        Command::new("/usr/bin/rex")
            .args(["-S", &at, "server", "run", "--listen", &at, "--restore=false", "--state-dir"])
            .arg(dir.join("state"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let began = Instant::now();
    while !sock.exists() {
        assert!(began.elapsed() < Duration::from_secs(8), "the scratch Rex server didn't begin to listen");
        std::thread::sleep(Duration::from_millis(20));
    }
    let sid = rex(&sock, "session.create", json!({ "label": "lead" }))["session_id"].as_str().unwrap().to_owned();
    let lead = rex(&sock, "session.new_window", json!({ "session_id": sid, "window_label": "lead", "layout": { "block": { "flavor": "com.superlogical.terminal.shell" } } }));
    let lead_block = lead["block_ids"][0].as_str().unwrap().to_owned();
    let helper = crate::activity_watch::local_helper_path().expect("the helper the app carries");

    // Before any desktop: a child started on the host, by the host.
    let early = olo_agent(&helper, &lead_block, &["start", "--command", "--name", "early", "--json", "--", "sleep", "300"]);
    assert_eq!(early["desktop"], "none");

    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let hub = RexHub::start(Settings::default(), TerminalTheme::default(), None, None, |_| {});
    rt.block_on(async {
        // The app connects: the child is there, under the lead's tab.
        let shown = until("the child to be shown", || {
            let st = hub.state.lock().unwrap();
            st.agents_told.contains("local").then(|| st.subagents.entries.iter().find(|e| e.host_key == "local" && e.name.as_deref() == Some("early")).cloned()).flatten()
        })
        .await;
        assert_eq!(Some(shown.parent_window_id.as_str()), lead["window_id"].as_str());
        assert_eq!(Some(shown.window_id.as_str()), early["window_id"].as_str());
        until("the tab tree to have it as a child", || {
            let st = hub.state.lock().unwrap();
            let live: HashSet<String> = st.hosts.get("local")?.sessions.get(&sid)?.windows.iter().map(|w| w.window_id.clone()).collect();
            st.subagents.is_child_window("local", &sid, &shown.window_id, &live).then_some(())
        })
        .await;

        // What it may ask of this computer: nothing, said at once from what the service said.
        let perform = |action: &str, block: &str, pid: Option<u32>| {
            let (inner, action, sid, block) = (hub.inner.clone(), action.to_owned(), sid.clone(), block.to_owned());
            async move { inner.handle_client_perform("local", action, Some(json!({ "args": ["whoami"], "title": "t", "body": "b" })), Some(sid), Some(block), pid).await }
        };
        let refused = perform("olo.op", &shown.block_id, None).await.expect("an answer");
        assert_eq!((&refused["ok"], &refused["data"]["code"]), (&json!(false), &json!(69)), "{refused}");
        assert!(refused["stderr"].as_str().unwrap().contains("--desktop none"), "{refused}");
        // Every action there is, not `olo.`'s alone.
        let refused = perform("agent.auto_approve_toggle", &shown.block_id, None).await.expect("an answer");
        assert_eq!(refused["ok"], false, "{refused}");

        // One started this moment, asked about before the app may have been told: the service is asked.
        let late = olo_agent(&helper, &lead_block, &["start", "--command", "--name", "late", "--desktop", "screens=qa-1", "--json", "--", "sleep", "300"]);
        let refused = perform("olo.op", late["block_id"].as_str().unwrap(), None).await.expect("an answer");
        assert_eq!(refused["data"]["code"], 69, "{refused}");
        assert!(refused["stderr"].as_str().unwrap().contains("screens=qa-1"), "{refused}");
        until("the new child to be shown", || hub.state.lock().unwrap().subagents.entries.iter().any(|e| e.name.as_deref() == Some("late")).then_some(())).await;
        // A process inside a child's tab that names the lead's pane is still that child.
        let early_pid = olo_agent(&helper, &lead_block, &["status", "early", "--json"])["pid"].as_u64().expect("its process") as u32;
        let refused = perform("olo.op", &lead_block, Some(early_pid)).await.expect("an answer");
        assert_eq!(refused["data"]["code"], 69, "a child that says it is the lead's pane: {refused}");
        // The lead's own pane is nobody's child: not refused here.
        let allowed = perform("olo.notify", &lead_block, Some(std::process::id())).await.expect("an answer");
        assert_eq!(allowed["ok"], true, "{allowed}");

        // `rex do olo.agent.list`: the service's answer.
        let listed = hub
            .inner
            .handle_client_perform("local", "olo.agent.list".into(), Some(json!({ "args": ["--json"], "cwd": dir })), Some(sid.clone()), Some(lead_block.clone()), None)
            .await
            .expect("an answer");
        assert_eq!(listed["ok"], true, "{listed}");
        let said: Value = serde_json::from_str(listed["stdout"].as_str().unwrap().trim()).unwrap();
        let names: Vec<&str> = said["subagents"].as_array().unwrap().iter().filter_map(|s| s["name"].as_str()).collect();
        assert_eq!(names, ["early", "late"], "{said}");
        // …and in its structured form.
        let status = hub
            .inner
            .handle_client_perform("local", "olo.agent.status".into(), Some(json!({ "target": "late", "json": true })), Some(sid.clone()), Some(lead_block.clone()), None)
            .await
            .expect("an answer");
        assert_eq!(status["data"]["desktop"], "screens=qa-1", "{status}");
        // A mistake's status is the service's.
        let wrong =
            hub.inner.handle_client_perform("local", "olo.agent.status".into(), Some(json!({ "args": ["nobody"] })), Some(sid.clone()), Some(lead_block.clone()), None).await.expect("an answer");
        assert_eq!((&wrong["ok"], &wrong["data"]["code"]), (&json!(false), &json!(1)), "{wrong}");

        // The window's Stop: the service closes the tab, and says so.
        hub.stop_subagent("local", &sid, &shown.window_id, true, false);
        until("its end to be shown", || hub.state.lock().unwrap().subagents.entries.iter().find(|e| e.name.as_deref() == Some("early")).and_then(|e| e.exited_at)).await;
        until("its tab to be gone", || {
            let st = hub.state.lock().unwrap();
            (!st.hosts.get("local")?.sessions.get(&sid)?.windows.iter().any(|w| w.window_id == shown.window_id)).then_some(())
        })
        .await;
        let _ = olo_agent(&helper, &lead_block, &["stop", "late", "--force", "--json"]);
    });
}

/// A minder's agents are the ones it minds, whoever stands under its
/// row in a list: stopped before it, and nobody else's with them.
#[test]
fn a_minders_agents_are_found_by_who_minds_them() {
    let agent = |id: &str, minded_by: Option<&str>, session: &str| crate::subagents::NativeSubagent {
        id: id.to_owned(),
        name: Some(id.to_owned()),
        host_key: "h".into(),
        session_id: session.into(),
        window_id: format!("win-{id}"),
        block_id: format!("block-{id}"),
        parent_window_id: "win-lead".into(),
        parent_block_id: None,
        cli: crate::subagents::AgentCli::Claude,
        model: "m".into(),
        effort: "e".into(),
        title: id.to_owned(),
        role: None,
        cwd: None,
        resume_id: None,
        cli_session_id: None,
        transcript_path: None,
        desktop_policy: crate::subagents::DesktopPolicy::None,
        stream_mode: false,
        started_at: 1,
        exited_at: None,
        exit_code: None,
        last_pid: None,
        created_rev: 0,
        seen_live: true,
        exited_by_sync: false,
        session_confidence: None,
        add_dirs: Vec::new(),
        command: Vec::new(),
        minds: None,
        minded_by: minded_by.map(str::to_owned),
        shared_minder: false,
    };
    let mut gone = agent("gone", Some("m1"), "s");
    gone.exited_at = Some(2);
    let entries = vec![
        agent("m1", None, "s"),
        agent("a", Some("m1"), "s"),
        agent("m2", None, "s"),
        agent("b", Some("m2"), "s"),
        agent("c", Some("m1"), "s"),
        agent("loose", None, "s"),
        gone,
        agent("elsewhere", Some("m1"), "other"),
    ];
    // Its own two, in the list's order, then itself. Not the other minder's, not one nobody minds, not one
    // that has ended, not another session's.
    assert_eq!(minder_and_its_agents(&entries, "h", "s", "win-m1"), ["win-a", "win-c", "win-m1"]);
    assert_eq!(minder_and_its_agents(&entries, "h", "s", "win-m2"), ["win-b", "win-m2"]);
    // A tab the records don't have (closed meanwhile): itself alone, as asked.
    assert_eq!(minder_and_its_agents(&entries, "h", "s", "win-none"), ["win-none"]);
}
