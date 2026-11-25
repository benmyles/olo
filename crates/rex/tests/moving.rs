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
//! looks (the source's sized), the source's sessions captured and each
//! pane's agent and session found, the key, a way from the source, the
//! destination's own files kept aside, the first pass (the home in parts
//! sent in parallel, a folder of /var/tmp with it), the panes read again,
//! the source stopped, the second pass, the source's lingering off, the
//! destination made right (keys, paths, labels, shell, link, units and
//! crontab), the sessions made again there with each pane's command and
//! each agent's message, the key taken away. Then checks what came out:
//! every pane there, each agent in the session it was in (two Claude Codes
//! in one folder each in its own), the message at its prompt, the work
//! list showing, /var/tmp's folder, a linked dotfile made right, the source
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

        // Look: each host, then what the source's home holds that goes.
        let to = migrate::Facts::read(&ran(&mut dst, &migrate::look_command(id)).await).expect("the destination's look");
        let plain = migrate::Facts::read(&ran(&mut src, &migrate::look_command(id)).await).expect("the source's look");
        let excluded = migrate::excluded(&plain, &to, false);
        let from = migrate::Facts::read(&ran(&mut src, &migrate::look_sized_command(id, &excluded)).await).expect("the source's look, sized");
        step(&format!("looked: {} ({}) to {} ({}); {} files, {} in /var/tmp", from.system, from.machine, to.system, to.machine, from.home_files, from.var_tmp_count()));
        assert!(migrate::refusals(&from, &to, true, "the destination").is_empty(), "{:?}", migrate::refusals(&from, &to, true, "the destination"));
        let var_tmp = from.var_tmp_count() > 0;
        assert!(var_tmp && from.var_tmp_count() >= 5000, "the account's entries in /var/tmp: count={}", from.var_tmp_count());
        migrate::ensure_rsync(&mut src.runner, &from).await.expect("rsync on source");
        migrate::ensure_rsync(&mut dst.runner, &to).await.expect("rsync on destination");

        // Each session read, each pane's agent and session found (two
        // Claude Codes in one folder told apart by their own records).
        async fn read_panes(src: &mut Host, sctrl: &ControlClient, id: &str) -> (Vec<migrate::CapturedSession>, Vec<migrate::Pane>) {
            let sessions = sctrl.session_list().await.expect("the source's sessions").payload;
            let mut captured = Vec::new();
            for s in &sessions {
                captured.push(migrate::capture(sctrl, &s.session_id).await.expect("a session captured"));
            }
            let pids: Vec<u32> = captured.iter().flat_map(|c| c.panes.iter().filter_map(|p| p.pid)).collect();
            let screens: HashMap<u32, Vec<String>> = captured.iter().flat_map(|c| c.panes.iter()).filter_map(|p| Some((p.pid?, migrate::fingerprint(&p.screen)))).collect();
            let panes = migrate::Pane::read_all(&ran(src, &migrate::look_pane_command(id, &pids, &screens)).await).expect("the panes' look");
            (captured, panes)
        }
        let sctrl = control(&src).await;
        let dctrl = control(&dst).await;
        let (captured, panes) = read_panes(&mut src, &sctrl, id).await;
        step(&format!("{} sessions, found: {:?}", captured.len(), panes.iter().filter_map(|p| p.agent.as_ref().map(|a| (a.agent.clone(), a.how.clone()))).collect::<Vec<_>>()));

        // The key, a way from the source; the destination's own files kept aside.
        let public = migrate::make_key(&mut src.runner, id, &from, id).await.expect("the key");
        migrate::allow(&mut dst.runner, id, id, &public, var_tmp).await.expect("the key let in");
        let verified = dst.runner.pool.verified(&dst.runner.target).expect("the destination's key, as its connection checked it");
        let (user, at) = peer.split_once('@').expect("USER@HOST:PORT");
        let (peer_host, peer_port) = at.rsplit_once(':').expect("HOST:PORT");
        let way = migrate::Way { user: user.into(), host: peer_host.into(), port: peer_port.parse().unwrap(), host_key: verified.key.clone() };
        let way = migrate::reach(&mut src.runner, id, &from, &to, id, &[way]).await.expect("the source reaches the destination");
        let kept = migrate::keep_destination(&mut dst.runner, id, id, &excluded).await.expect("the destination's own files kept aside");
        step(&format!("the source reaches the destination with the move's key; {kept} of the destination's files kept aside"));

        async fn finish(src: &mut Host, id: &str, from: &migrate::Facts, pass: u32) -> serde_json::Value {
            let until = Instant::now() + Duration::from_secs(600);
            loop {
                let st = migrate::pass_status(&mut src.runner, id, from, id).await.expect("the copy's status");
                if st["pass"] == pass && st["running"] == false && matches!(st["phase"].as_str(), Some("done" | "failed")) {
                    return st;
                }
                assert!(Instant::now() < until, "the copy didn't end: {st}");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        migrate::start_pass(&mut src.runner, id, &from, &to, id, &way, 1, &excluded, var_tmp).await.expect("pass 1 started");
        let p1 = finish(&mut src, id, &from, 1).await;
        assert_eq!(p1["phase"], "done", "{p1}");
        assert!(p1["parts"].as_u64().unwrap_or(0) > 2, "the home sent in parts: {p1}");
        step(&format!("pass 1: {} files of {} in {} parts, {} bytes on the wire", p1["files"], p1["total_files"], p1["parts"], p1["sent"]));
        // Delete a file on the source after Pass 1 so Pass 2 proves `forget` removes it on the destination without touching destination's own files.
        ran(&mut src, "rm -f ~/ephemeral.tmp /var/tmp/olo-rig-scratch/ephemeral.tmp").await;

        // Read again just before the stop (the source went on working), then
        // stopped, the second pass, its lingering off, its Quadlets masked.
        let (captured, panes) = read_panes(&mut src, &sctrl, id).await;
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
        // (Each agent pane's session, by its pane: two of one agent apart.)
        let session_of: HashMap<String, String> = captured
            .iter()
            .flat_map(|c| c.panes.iter())
            .filter_map(|cp| Some((cp.block_id.clone(), panes.iter().find(|p| Some(p.pid) == cp.pid)?.agent.as_ref()?.session.clone()?)))
            .collect();
        let claudes: Vec<&String> = captured.iter().flat_map(|c| c.panes.iter()).filter(|cp| cp.label.starts_with("claude")).filter_map(|cp| session_of.get(&cp.block_id)).collect();
        assert_eq!(claudes.len(), 2, "two Claude Code panes, each with its session: {session_of:?}");
        assert_ne!(claudes[0], claudes[1], "two Claude Code panes in one folder, each its own session");
        drop(sctrl);
        // Mirror `hub.disconnect_host_and_wait`: close the SSH pool completely
        // before running `stop_source`, and open a separate SSH connection to
        // verify `stop_source` never kills SSH sessions.
        src.runner.pool.close(&src.runner.target).await;
        let other_ssh = Arc::new(SshSessionPool::default());
        let other_target = src.runner.target.clone();
        let other_handle = tokio::spawn({
            let other_ssh = other_ssh.clone();
            let other_target = other_target.clone();
            async move { other_ssh.exec(&other_target, Some(Arc::new(Nobody)), "sleep 8 && echo ssh-survived", &mut |_| {}).await }
        });
        tokio::time::sleep(Duration::from_millis(500)).await;
        let left = migrate::stop_source(&mut src.runner, id, &from, id, &agents).await.expect("the source stopped");
        assert!(left.is_empty(), "left running: {left:?}");
        let other_ran = other_handle.await.expect("other ssh task").expect("other ssh session survived stop_source");
        assert!(other_ran.ok() && other_ran.output.contains("ssh-survived"), "other SSH session should NOT be stopped: {other_ran:?}");
        other_ssh.close(&other_target).await;
        let crontab = migrate::saved_crontab(&mut src.runner, &from, id).await.expect("its crontab read");
        step(&format!("the source stopped{}", if crontab.is_some() { ", its crontab saved" } else { "" }));
        migrate::allow(&mut dst.runner, id, id, &public, var_tmp).await.expect("the key's day put off");
        migrate::start_pass(&mut src.runner, id, &from, &to, id, &way, 2, &excluded, var_tmp).await.expect("pass 2 started");
        let p2 = finish(&mut src, id, &from, 2).await;
        assert_eq!(p2["phase"], "done", "{p2}");
        step(&format!("pass 2: {} files, {} on the wire, {} gone", p2["files"], p2["sent"], p2["report"]["gone"]));
        migrate::linger_off(&mut src.runner, &from).await.expect("its lingering off");
        migrate::mask_quadlets(&mut src.runner, &from).await.expect("its Quadlets masked");

        // The destination made right: keys, paths, labels, shell, link,
        // units and crontab, the path; its agents' programs checked.
        let added = migrate::merge_keys(&mut src.runner, &mut dst.runner, id, id).await.expect("the source's keys let in there");
        let fixed = migrate::fix_destination(&mut dst.runner, id, &from, &to, id, var_tmp).await.expect("the destination made right");
        let label = migrate::relabel(&mut dst.runner, &to).await.expect("its labels");
        let shell = migrate::login_shell(&mut dst.runner, &from, &to).await.expect("its login shell");
        let linked = migrate::compat_link(&mut dst.runner, &from, &to).await.expect("the old home's path");
        let failed = migrate::units_and_crontab(&mut dst.runner, &from, &to, id, crontab.as_deref()).await.expect("its units and crontab");
        migrate::path_on(&mut dst.runner).await.expect("~/.local/bin on its shells' path");
        let ran_agents: Vec<String> = panes.iter().filter_map(|p| p.agent.as_ref().map(|a| a.agent.clone())).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        let broken = migrate::agents_broken(&mut dst.runner, &ran_agents).await.expect("its agents' programs");
        step(&format!(
            "made right: {added} keys added, {} rewritten, {} projects, {} relinked; labels {label:?}, shell {shell:?}, linked {linked}, units failed {failed:?}, agents broken {broken:?}",
            fixed["rewritten"].as_array().map_or(0, Vec::len),
            fixed["projects"].as_array().map_or(0, Vec::len),
            fixed["relinked"].as_array().map_or(0, Vec::len)
        ));
        assert!(added >= 1, "the source's own sign-in key let in there");
        assert!(broken.is_empty(), "the stand-ins run there: {broken:?}");

        // The sessions made again (a folder that isn't there: the home),
        // each pane's command, each agent's message.
        let cwds: Vec<String> = runs.values().map(|r| r.cwd.clone()).filter(|c| !c.is_empty()).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        let there = migrate::dirs_there(&mut dst.runner, &cwds).await.expect("the panes' folders there");
        let mut made_all = HashMap::new();
        for c in &captured {
            let (sid, made) = migrate::rebuild(&dctrl, c, &runs, &there).await.expect("a session made again");
            for (old, new) in made {
                made_all.insert(old, (sid.clone(), new));
            }
        }
        step(&format!("{} panes made again there", made_all.len()));
        let told = migrate::Told { from_label: "the old host", to_label: "the new host", from: from.clone(), to: to.clone(), when: "just now", linked, var_tmp: true, ..migrate::Told::default() };
        let message = migrate::message(&told);
        for (old, (sid, new)) in &made_all {
            let Some(run) = runs.get(old) else { continue };
            let Some(command) = &run.command else { continue };
            screen_with(&dctrl, sid, new, "$", 15).await;
            let text = migrate::command_text(command);
            // (What isn't run again is left typed as a comment: Enter never
            // runs it by mistake.)
            let typed = if run_it.get(old).copied().unwrap_or(false) { format!("{text}\r") } else { format!("# {text}") };
            dctrl.terminal_write(sid, new, &typed).await.expect("its command typed");
        }
        for (old, (sid, new)) in &made_all {
            let Some(pane) = captured.iter().flat_map(|c| c.panes.iter()).find(|p| &p.block_id == old) else { continue };
            let Some(agent) = pane.pid.and_then(|pid| panes.iter().find(|p| p.pid == pid)).and_then(|p| p.agent.as_ref()) else { continue };
            let screen = screen_with(&dctrl, sid, new, &format!("{}>", agent.agent), 20).await;
            assert!(screen.contains(&format!("{}>", agent.agent)), "{} never came to its prompt:\n{screen}", agent.agent);
            dctrl.terminal_write(sid, new, &message).await.expect("its message typed");
        }
        step("each pane's command and each agent's message typed");

        // What came out: each agent in its own session, the message at its
        // prompt; the work list; /var/tmp's folder; the linked dotfile made
        // right; the source stopped.
        for (old, (sid, new)) in &made_all {
            let Some(pane) = captured.iter().flat_map(|c| c.panes.iter()).find(|p| &p.block_id == old) else { continue };
            let screen = dctrl.terminal_format(sid, new, "text").await.map(|r| r.payload).unwrap_or_default();
            match session_of.get(old) {
                Some(was) => {
                    assert!(screen.contains(&format!("session {was}")), "{} isn't back in its session {was}:\n{screen}", pane.label);
                    assert!(screen.contains("[Olo] This session was moved"), "{}'s message isn't at its prompt:\n{screen}", pane.label);
                    assert!(screen.contains(&format!("in {}", to.home)), "{} isn't in the new home:\n{screen}", pane.label);
                    println!("  {}: back in its own session, the message at its prompt", pane.label);
                }
                None if pane.label == "todo" => {
                    let screen = screen_with(&dctrl, sid, new, "second thing", 10).await;
                    assert!(screen.contains("first thing") && screen.contains("second thing"), "the work list isn't showing:\n{screen}");
                    println!("  the work list: showing");
                }
                None => {}
            }
        }
        let there_now = ran(
            &mut dst,
            "cat /var/tmp/olo-rig-scratch/notes.txt; echo; cat /var/tmp/olo-rig-loose.txt; echo; cat ~/dest-preexisting.txt; echo; [ ! -e ~/ephemeral.tmp ] && echo home-gone; [ ! -e /var/tmp/olo-rig-scratch/ephemeral.tmp ] && echo vt-gone; echo vtcount=$(ls -d /var/tmp/olo-bulk-* | wc -l); readlink ~/.gitconfig; cat ~/dotfiles/gitconfig",
        )
        .await;
        assert!(there_now.contains("scratch notes"), "/var/tmp's folder came: {there_now}");
        assert!(there_now.contains("loose scratch"), "/var/tmp's loose file came: {there_now}");
        assert!(there_now.contains("stay put"), "destination's pre-existing file kept: {there_now}");
        assert!(there_now.contains("home-gone") && there_now.contains("vt-gone"), "vanished Pass 1 files forgotten: {there_now}");
        assert!(there_now.contains("vtcount=5000"), "all 5000 /var/tmp dirs arrived: {there_now}");
        assert!(there_now.contains(&format!("{}/dotfiles/gitconfig", to.home)) && there_now.contains(&format!("{}/.gitignore_global", to.home)), "the linked dotfile made right: {there_now}");
        println!("  /var/tmp's 5000+ folders & loose file there; destination's own file kept; vanished files forgotten; linked dotfile made right");
        let state = ran(&mut src, "sh -c 'export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}; echo socket=$(systemctl --user is-enabled rex.socket 2>&1); echo server=$(systemctl --user is-active rex.service 2>&1);'").await;
        assert!(state.contains("socket=disabled") && state.contains("server=inactive"), "the source isn't stopped: {state}");
        println!("  the source: Rex's socket disabled, its server down (and nothing of the account's left running, its drain said)");
        migrate::clean_up(&mut src.runner, &mut dst.runner, id, &from, &to, id).await.expect("the key taken away");
        step("done: the key taken away, both move folders gone");
    });
}
