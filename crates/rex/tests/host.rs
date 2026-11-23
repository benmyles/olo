//! Against a real host, where one is given (never in an ordinary run:
//! every test here is ignored): `OLO_REX_TEST_HOST=ssh://USER@HOST`, with
//! an agent (`SSH_AUTH_SOCK`) whose keys let USER and root in, and a
//! `HOME` whose `.ssh/known_hosts` knows the host. It runs commands
//! there, asks how the host stands as the account and as root, and reads
//! the login server's accounts; it changes nothing on the host. What it
//! prints names neither the host nor the account.
//!
//!   HOME=SCRATCH OLO_REX_TEST_HOST=ssh://USER@HOST cargo test -p olo-rex-client --test host -- --ignored --nocapture

use std::sync::Arc;

use olo_rex_client::{
    SshAskpass, SshSessionPool, SshTarget,
    provision::{self, OverSsh, Runner as _},
};
use zeroize::Zeroizing;

/// Trusts no host it doesn't know and answers no question.
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

fn host() -> OverSsh {
    let address = std::env::var("OLO_REX_TEST_HOST").expect("OLO_REX_TEST_HOST=ssh://USER@HOST");
    let target = SshTarget::parse(&address).expect("an ssh:// address");
    assert!(target.user.as_deref().is_some_and(|u| u != "root"), "the address names an account that isn't root");
    OverSsh { pool: Arc::new(SshSessionPool::default()), target, root_key: None, askpass: Some(Arc::new(Nobody)) }
}

fn block<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(f)
}

/// (With the account's name out of it.)
fn shown(steps: &[provision::Step], user: &str) -> Vec<String> {
    steps.iter().map(|s| format!("{}: {}", if s.root { "root" } else { "user" }, s.shown().replace(user, "USER"))).collect()
}

#[test]
#[ignore = "needs a real host: OLO_REX_TEST_HOST"]
fn a_command_runs_there_and_what_it_writes_comes_line_by_line() {
    let mut host = host();
    let mut lines = Vec::new();
    let ran = block(host.run(false, "printf 'one\\ntwo\\n'; echo three >&2; printf 'no end'; exit 3", &mut |l| lines.push(l.to_owned()))).unwrap();
    assert_eq!(ran.status, Some(3));
    assert!(!ran.ok());
    let mut sorted = lines.clone();
    sorted.sort();
    assert_eq!(sorted, ["no end", "one", "three", "two"], "{lines:?}");
    assert_eq!(ran.output.lines().count(), 4);
    // As root, with the agent's keys; and who it is there.
    let ran = block(host.run(true, "id -u", &mut |_| {})).unwrap();
    assert_eq!((ran.status, ran.output.trim()), (Some(0), "0"));
    let ran = block(host.run(false, "id -u", &mut |_| {})).unwrap();
    assert!(ran.ok() && ran.output.trim() != "0");
}

#[test]
#[ignore = "needs a real host: OLO_REX_TEST_HOST"]
fn the_host_says_how_it_stands_and_what_it_needs() {
    let mut host = host();
    let user = host.target.user.clone().unwrap();
    let facts = block(provision::probe(&mut host, false)).unwrap();
    assert_eq!((facts.os.as_str(), facts.user.as_str()), ("Linux", user.as_str()));
    assert!(facts.uid >= 1000 && facts.rex.is_some());
    println!(
        "as the account: distro {}, rex {:?} ({:?}), packaged {}, a bare copy {}, systemd {}, mode {:?}, listed {:?}, its own server stays {}",
        facts.distro,
        facts.rex,
        facts.version,
        facts.packaged,
        facts.bare,
        facts.systemd,
        facts.mode(),
        facts.listed(),
        facts.own_server_stays()
    );
    for mode in provision::ServerMode::ALL {
        match provision::plan(mode, &facts) {
            Ok(steps) => println!("  for {}: {:?}{}", mode.name(), shown(&steps, &user), if provision::ends_sessions(&steps) { " (ends sessions)" } else { "" }),
            Err(why) => println!("  for {}: can't ({})", mode.name(), why.replace(&user, "USER")),
        }
    }
    // As root: who it is, and the accounts as root reads them.
    let root = block(provision::probe(&mut host, true)).unwrap();
    assert_eq!((root.uid, root.user.as_str()), (0, "root"));
    let accounts = block(provision::read_accounts(&mut host)).unwrap();
    println!("as root: {} accounts listed, {} defaults; the account is among them: {}", accounts.accounts().len(), accounts.defaults().len(), accounts.accounts().iter().any(|a| a.name == user));
}

#[test]
#[ignore = "needs a real host: OLO_REX_TEST_HOST"]
fn root_refused_is_said_to_be_refused_and_no_question_is_asked() {
    // A key nobody has: the agent offers none, and no password is asked for.
    let mut host = OverSsh { root_key: Some("SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into()), ..host() };
    let failed = block(host.run(true, "id -u", &mut |_| {})).unwrap_err();
    assert!(failed.refused, "{}", failed.why.len());
    // The account is still let in beside it (another connection).
    assert!(block(host.run(false, "true", &mut |_| {})).unwrap().ok());
}

#[test]
#[ignore = "needs a real host: OLO_REX_TEST_HOST"]
fn the_agents_keys_are_told_apart() {
    let status = block(olo_rex_client::inspect_ssh_agent(None));
    assert!(status.reachable && status.key_count > 0 && status.keys.len() == status.key_count);
    assert!(status.keys.iter().all(|k| k.fingerprint.starts_with("SHA256:") && !k.kind.is_empty()));
    let kinds: std::collections::BTreeSet<&str> = status.keys.iter().map(|k| k.kind.as_str()).collect();
    println!("the agent has {} keys, of kinds {kinds:?}; each has a fingerprint", status.key_count);
    // The one of them that lets root in is found by its fingerprint.
    let mut let_in = 0;
    for key in &status.keys {
        let mut host = OverSsh { root_key: Some(key.fingerprint.clone()), ..host() };
        if block(host.run(true, "true", &mut |_| {})).is_ok_and(|ran| ran.ok()) {
            let_in += 1;
        }
    }
    println!("{let_in} of them let root in there");
    assert!(let_in >= 1);
}

/// The host's server as root sees it ([`olo_rex_client::manage::look`]):
/// read, nothing changed; and its login server's log. (What it prints
/// names neither the host nor anybody on it: counts and states only.)
#[test]
#[ignore = "needs a real host: OLO_REX_TEST_HOST"]
fn the_server_is_read_as_root_and_nothing_is_changed() {
    use olo_rex_client::manage::{self, Service};
    let mut host = host();
    let before = block(provision::probe(&mut host, true)).expect("root is let in with the agent's keys");
    let server = block(manage::look(&mut host, true)).unwrap();
    println!(
        "mode {:?}; login {:?}, how long {}; shared {:?}; {} people, {} in the rex group; installed {}, newer {}; accounts {}, {} not listed",
        server.mode(),
        server.state(Service::Login),
        if server.up(Service::Login).is_some() { "known" } else { "unknown" },
        server.state(Service::Shared),
        server.people.len(),
        server.members.len(),
        server.installed.is_some(),
        server.newer.is_some(),
        server.accounts.as_ref().map_or("unread".to_owned(), |a| a.accounts().len().to_string()),
        server.not_listed().len(),
    );
    assert!(server.facts.packaged && server.facts.systemd);
    assert!(server.mode().is_some(), "a server runs there for everybody");
    assert!(!server.people.is_empty(), "somebody has an account there");
    assert!(server.installed.is_some());
    // Every action says what it would run, and none was run.
    for action in [manage::Action::Restart(Service::Login), manage::Action::Stop(Service::Shared), manage::Action::Update] {
        assert!(!action.shown(&server).is_empty(), "{action:?}");
    }
    println!(
        "the repository was asked: {}; a newer version: {}; sessions through the login server: {:?}; failed: {}",
        server.checked,
        server.newer.is_some(),
        server.sessions_on(Service::Login),
        server.failed(Service::Login)
    );
    assert!(server.checked, "apt read Rex's own source again");
    assert!(server.sessions_on(Service::Login).is_some(), "the account's own server said how many sessions it has");
    let log = block(manage::journal(&mut host, Service::Login, manage::LogRange::SinceStart)).unwrap();
    assert!(log.first().is_some_and(|day| day.len() == 10 && day.as_bytes()[4] == b'-'), "the log begins with its day");
    assert!(log.iter().skip(1).all(|l| l.len() == 10 || l.as_bytes().get(2) == Some(&b':')), "each line begins with its time");
    println!("the login server's log: {} lines", log.len());
    let after = block(provision::probe(&mut host, true)).unwrap();
    assert_eq!((before.login, before.shared, before.own), (after.login, after.shared, after.own));
}
