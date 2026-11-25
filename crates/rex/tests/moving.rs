//! An account moved from one real host to another, whole (never in an
//! ordinary run: ignored; the rig's containers give it two hosts,
//! tools/dev/rex-slow, `UbuntuHost` and `FedoraHost` on a network of their
//! own, each set up for Rex by its host test):
//!
//!   OLO_REX_TEST_HOST=ssh://ubuntu@127.0.0.1:P1   (the source, from here)
//!   OLO_REX_TEST_DEST=ssh://fedora@127.0.0.1:P2   (the destination, from here)
//!   OLO_REX_TEST_PEER=fedora@NAME:22              (the destination, from the source)
//!   OLO_REX_TEST_HELPER_FILE=…/olo-rex-link        (built with `move`, for their machine)
//!
//! with an agent whose keys let both accounts in and a HOME whose
//! known_hosts knows both. The move as the app drives it, step by step:
//! looks, the source's sessions captured and each pane's agent and session
//! found, the key, a way from the source, the first pass, the source
//! stopped, the second pass, the source's lingering off, the destination
//! made right, its units and crontab, the sessions made again there with
//! each pane's command and each agent's message, the key taken away. Then
//! checks what came out: every pane there, each agent in the session it
//! was in, the message at its prompt, the work list showing, the source
//! stopped. What it prints names neither host nor account.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use olo_rex_client::{
    ClientIdentity, ControlClient, RexEndpoint, SshAskpass, SshSessionPool, SshTarget, helper, migrate,
    provision::{self, OverSsh, Runner as _},
};
use zeroize::Zeroizing;

#[derive(Debug)]
struct Nobody;

impl SshAskpass for Nobody {
    fn confirm_host_key(&self, _: &str, _: u16, _: &str, _: &str, _: bool) -> bool {
        false
    }

    fn prompt_secret(&self, _: &str, _: &str, _: bool) -> Option<Zeroizing<String>> {
        None
    }
}

struct Host {
    address: String,
    runner: OverSsh,
}

fn host(var: &str) -> Host {
    let address = std::env::var(var).unwrap_or_else(|_| panic!("{var}=ssh://USER@HOST:PORT"));
    let target = SshTarget::parse(&address).expect("an ssh:// address");
    let runner = OverSsh { pool: Arc::new(SshSessionPool::default()), target, root_key: None, root_by: provision::RootBy::Sudo, askpass: Some(Arc::new(Nobody)) };
    Host { address, runner }
}

async fn ran(h: &mut Host, command: &str) -> String {
    let r = h.runner.run(false, command, &mut |_| {}).await.unwrap_or_else(|f| panic!("{command}: {}", f.why));
    assert_eq!(r.status, Some(0), "{command}: {}", r.output);
    r.output
}

async fn control(h: &Host) -> ControlClient {
    let endpoint = RexEndpoint::parse(&h.address, h.runner.pool.clone(), Some(Arc::new(Nobody))).expect("an address");
    let (stream, name) = endpoint.connect_control_stream().await.expect("the host's Rex server is reached");
    ControlClient::connect(stream, &name, ClientIdentity::a_test(), None).await.expect("the control connection").0
}

/// The pane's screen, once it shows `want` (up to `secs`).
async fn screen_with(ctrl: &ControlClient, sid: &str, bid: &str, want: &str, secs: u64) -> String {
    let until = Instant::now() + Duration::from_secs(secs);
    loop {
        let screen = match ctrl.terminal_format(sid, bid, "text").await {
            Ok(r) => r.payload,
            Err(e) => format!("(its screen couldn't be read: {e})"),
        };
        if screen.contains(want) || Instant::now() > until {
            return screen;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[test]
#[ignore = "two real hosts: the rig's (OLO_REX_TEST_HOST, _DEST, _PEER, _HELPER_FILE)"]
fn an_account_moves_whole() {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap().block_on(async {
        let (mut src, mut dst) = (host("OLO_REX_TEST_HOST"), host("OLO_REX_TEST_DEST"));
        let peer = std::env::var("OLO_REX_TEST_PEER").expect("OLO_REX_TEST_PEER=USER@HOST:PORT");
        let file = std::env::var("OLO_REX_TEST_HELPER_FILE").expect("OLO_REX_TEST_HELPER_FILE");
        let bytes: &'static [u8] = Box::leak(std::fs::read(&file).expect("the helper's file").into_boxed_slice());
        let builds = [helper::Build { system: "Linux", machines: &["x86_64", "amd64", "aarch64", "arm64"], bytes }];
        let id = "move-test";
        for h in [&src, &dst] {
            helper::ensure(&h.runner.pool, &h.runner.target, Some(Arc::new(Nobody)), id, &builds).await.expect("the helper is put on the host");
            // (Rex's server reached through Olo's helper there, as the app
            // reaches it.)
            h.runner.pool.set_helper(Some(id));
        }
        let began = Instant::now();
        let step = |what: &str| println!("{:>6.1} s  {what}", began.elapsed().as_secs_f64());

        // Look.
        let from = migrate::Facts::read(&ran(&mut src, &migrate::look_command(id)).await).expect("the source's look");
        let to = migrate::Facts::read(&ran(&mut dst, &migrate::look_command(id)).await).expect("the destination's look");
        step(&format!("looked: {} ({}) to {} ({})", from.system, from.machine, to.system, to.machine));
        assert!(migrate::refusals(&from, &to, 0, 0).is_empty(), "{:?}", migrate::refusals(&from, &to, 0, 0));
        let (sctrl, dctrl) = (control(&src).await, control(&dst).await);
        let sessions = sctrl.session_list().await.expect("the source's sessions").payload;
        let mut captured = Vec::new();
        for s in &sessions {
            captured.push(migrate::capture(&sctrl, &s.session_id).await.expect("a session captured"));
        }
        let pids: Vec<u32> = captured.iter().flat_map(|c| c.panes.iter().filter_map(|p| p.pid)).collect();
        let panes = migrate::Pane::read_all(&ran(&mut src, &migrate::look_pane_command(id, &pids)).await).expect("the panes' look");
        let path = |p: &str| -> String {
            match p.strip_prefix(&from.home) {
                Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{}{rest}", to.home),
                _ => p.to_owned(),
            }
        };
        let mut runs = HashMap::new();
        let mut agents = Vec::new();
        let mut run_it = HashMap::new();
        for c in &captured {
            let (r, a, it) = migrate::pane_runs(c, &panes, &path);
            runs.extend(r);
            agents.extend(a);
            run_it.extend(it);
        }
        let sessions_before: HashMap<String, String> = panes.iter().filter_map(|p| p.agent.as_ref()).filter_map(|a| Some((a.agent.clone(), a.session.clone()?))).collect();
        step(&format!(
            "{} sessions, {} panes, agents {:?} (found: {:?})",
            captured.len(),
            runs.len(),
            agents.len(),
            panes.iter().filter_map(|p| p.agent.as_ref().map(|a| (a.agent.clone(), a.how.clone()))).collect::<Vec<_>>()
        ));

        // The key, and a way from the source.
        let public = migrate::make_key(&mut src.runner, id, &from, id).await.expect("the key");
        migrate::allow(&mut dst.runner, id, id, &public).await.expect("the key let in");
        let verified = dst.runner.pool.verified(&dst.runner.target).expect("the destination's key, as its connection checked it");
        let (user, at) = peer.split_once('@').expect("USER@HOST:PORT");
        let (peer_host, peer_port) = at.rsplit_once(':').expect("HOST:PORT");
        let way = migrate::Way { user: user.into(), host: peer_host.into(), port: peer_port.parse().unwrap(), host_key: verified.key.clone() };
        let way = migrate::reach(&mut src.runner, id, &from, &to, id, &[way]).await.expect("the source reaches the destination");
        step("the source reaches the destination with the move's key");
        let excluded = migrate::excluded(&from, &to, false);

        async fn finish(src: &mut Host, id: &str, from: &migrate::Facts) -> serde_json::Value {
            let until = Instant::now() + Duration::from_secs(600);
            loop {
                let st = migrate::pass_status(&mut src.runner, id, from, id).await.expect("the copy's status");
                if st["running"] == false && matches!(st["phase"].as_str(), Some("done" | "failed")) {
                    return st;
                }
                assert!(Instant::now() < until, "the copy didn't end: {st}");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        migrate::start_pass(&mut src.runner, id, &from, &to, id, &way, 1, &excluded).await.expect("pass 1 started");
        let p1 = finish(&mut src, id, &from).await;
        assert_eq!(p1["phase"], "done", "{p1}");
        step(&format!("pass 1: {} files, {} bytes on the wire", p1["files"], p1["sent"]));

        // The source stopped, the second pass, lingering off.
        drop(sctrl);
        let left = migrate::stop_source(&mut src.runner, id, &from, id, &agents).await.expect("the source stopped");
        assert!(left.is_empty(), "left running: {left:?}");
        let crontab = migrate::saved_crontab(&mut src.runner, &from, id).await.expect("its crontab read");
        step(&format!("the source stopped{}", if crontab.is_some() { ", its crontab saved" } else { "" }));
        migrate::start_pass(&mut src.runner, id, &from, &to, id, &way, 2, &excluded).await.expect("pass 2 started");
        let p2 = finish(&mut src, id, &from).await;
        assert_eq!(p2["phase"], "done", "{p2}");
        step(&format!("pass 2: {} files, {} on the wire, {} gone", p2["files"], p2["sent"], p2["report"]["gone"]));
        migrate::linger_off(&mut src.runner, &from).await.expect("its lingering off");

        // The destination made right, its units and crontab.
        let fixed = migrate::fix_destination(&mut dst.runner, id, &from, &to, id).await.expect("the destination made right");
        step(&format!(
            "made right: {} rewritten, {} projects, {} relinked; mentions {:?}",
            fixed["rewritten"].as_array().map_or(0, Vec::len),
            fixed["projects"].as_array().map_or(0, Vec::len),
            fixed["relinked"].as_array().map_or(0, Vec::len),
            fixed["mentions"]
        ));
        migrate::units_and_crontab(&mut dst.runner, &from, &to, id, crontab.as_deref()).await.expect("its units and crontab");
        migrate::path_on(&mut dst.runner).await.expect("~/.local/bin on its shells' path");

        // The sessions made again, each pane's command, each agent's message.
        let mut made_all = HashMap::new();
        let mut new_sessions = Vec::new();
        for c in &captured {
            let (sid, made) = migrate::rebuild(&dctrl, c, &runs).await.expect("a session made again");
            new_sessions.push(sid.clone());
            for (old, new) in made {
                made_all.insert(old, (sid.clone(), new));
            }
        }
        step(&format!("{} sessions made again there, {} panes", new_sessions.len(), made_all.len()));
        let told = migrate::Told { from_label: "the old host", to_label: "the new host", from: from.clone(), to: to.clone(), when: "just now", linked: false, ..migrate::Told::default() };
        let message = migrate::message(&told);
        for (old, (sid, new)) in &made_all {
            let Some(run) = runs.get(old) else { continue };
            let Some(command) = &run.command else { continue };
            // (Its shell ready first: the prompt's mark.)
            screen_with(&dctrl, sid, new, "$", 15).await;
            let text = migrate::command_text(command);
            let enter = if run_it.get(old).copied().unwrap_or(false) { "\r" } else { "" };
            dctrl.terminal_write(sid, new, &format!("{text}{enter}")).await.expect("its command typed");
        }
        for (old, (sid, new)) in &made_all {
            let Some(pane) = captured.iter().flat_map(|c| c.panes.iter()).find(|p| &p.block_id == old) else { continue };
            let Some(agent) = pane.pid.and_then(|pid| panes.iter().find(|p| p.pid == pid)).and_then(|p| p.agent.as_ref()) else { continue };
            let screen = screen_with(&dctrl, sid, new, &format!("{}>", agent.agent), 20).await;
            assert!(screen.contains(&format!("{}>", agent.agent)), "{} never came to its prompt:\n{screen}", agent.agent);
            dctrl.terminal_write(sid, new, &message).await.expect("its message typed");
        }
        step("each pane's command and each agent's message typed");

        // What came out.
        for (old, (sid, new)) in &made_all {
            let Some(pane) = captured.iter().flat_map(|c| c.panes.iter()).find(|p| &p.block_id == old) else { continue };
            let agent = pane.pid.and_then(|pid| panes.iter().find(|p| p.pid == pid)).and_then(|p| p.agent.clone());
            let screen = dctrl.terminal_format(sid, new, "text").await.map(|r| r.payload).unwrap_or_default();
            match agent {
                Some(a) => {
                    let was = sessions_before.get(&a.agent).cloned().unwrap_or_default();
                    assert!(screen.contains(&format!("session {was}")), "{} isn't back in its session {was}:\n{screen}", a.agent);
                    assert!(screen.contains("[Olo] This session was moved"), "{}'s message isn't at its prompt:\n{screen}", a.agent);
                    assert!(screen.contains(&format!("in {}", to.home)), "{} isn't in the new home:\n{screen}", a.agent);
                    println!("  {}: back in its session, the message at its prompt", a.agent);
                }
                None if pane.label == "todo" => {
                    let screen = screen_with(&dctrl, sid, new, "second thing", 10).await;
                    assert!(screen.contains("first thing") && screen.contains("second thing"), "the work list isn't showing:\n{screen}");
                    println!("  the work list: showing");
                }
                None => {}
            }
        }
        // The source stopped and kept so: Rex's socket disabled (nothing
        // starts its server again), its server down (its drain said nothing
        // of the account's was left).
        // (Not by reaching it: a dial there may start a server of its own.)
        let state = ran(&mut src, "sh -c 'export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}; echo socket=$(systemctl --user is-enabled rex.socket 2>&1); echo server=$(systemctl --user is-active rex.service 2>&1);'").await;
        assert!(state.contains("socket=disabled") && state.contains("server=inactive"), "the source isn't stopped: {state}");
        println!("  the source: Rex's socket disabled, its server down (and nothing of the account's left running, its drain said)");
        migrate::clean_up(&mut src.runner, &mut dst.runner, id, &from, id).await.expect("the key taken away");
        step("done: the key taken away, the source's folder gone");
    });
}
