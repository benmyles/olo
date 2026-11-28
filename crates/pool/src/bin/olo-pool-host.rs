//! `olo-pool-host`: Olo's Token Pool on a Rex host (`olo_token_pool::
//! host`). Put in the account's home by the desktop's pool, built for
//! the host (statically, without the crate's `desktop` feature), and
//! run as the account's own service:
//!
//! - `olo-pool-host serve`: the pool (its gateway on loopback, the
//!   clients' proxy beside it, the accounts the desktop carried), until
//!   it is stopped. Claude Code and Codex there are pointed at it as it
//!   first starts, and Codex's daemon kept on its settings.
//! - `olo-pool-host ctl`: one line of JSON from its input put to the
//!   service, and the answer written (what the desktop's pool runs over
//!   SSH).
//! - `olo-pool-host put-back`: Claude Code and Codex there as they were
//!   before the pool (as it is removed).
//! - `olo-pool-host forget`: what the pool kept on the host, removed.
//! - `olo-pool-host keep`: the service's own process once it has handed
//!   over to a newer version (`host::hand_over`), following it.
//! - `olo-pool-host --version`.

use std::io::Read as _;

use olo_token_pool::{clients, codex_daemon, gateway, host, intercept, model::Provider};

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") => {
            println!("olo-pool-host {}", host::VERSION);
            std::process::ExitCode::SUCCESS
        }
        Some("ctl") => {
            let mut line = String::new();
            if std::io::stdin().read_to_string(&mut line).is_err() {
                return std::process::ExitCode::FAILURE;
            }
            match host::ask_here(&line) {
                Ok(said) => {
                    print!("{said}");
                    std::process::ExitCode::SUCCESS
                }
                Err(why) => {
                    eprintln!("olo-pool-host: {why}");
                    std::process::ExitCode::from(3)
                }
            }
        }
        Some("put-back") => {
            let mut ok = true;
            for p in Provider::ALL {
                if let Err(e) = clients::put_back(p) {
                    eprintln!("olo-pool-host: {e}");
                    ok = false;
                }
            }
            if ok { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE }
        }
        // (As the pool is removed from the host: what it kept there,
        // its sealed accounts and their key among it.)
        Some("forget") => {
            let config = olo_xdg::config_dir();
            for name in
                ["pool.vault", "pool.key", "pool.lock", "pool-ca.pem", "pool-ca.key", "pool-ca-bundle.pem", "pool-agy-ca.pem", "pool-agy-ca.key", "pool-clients.json", "pool-agy.env", "pool-agy.sh"]
            {
                let _ = std::fs::remove_file(config.join(name));
            }
            let _ = std::fs::remove_file(olo_xdg::state_dir().join("pool-activity.json"));
            let _ = std::fs::remove_file(host::socket());
            olo_token_pool::capture::remove_all();
            std::process::ExitCode::SUCCESS
        }
        Some("serve") => serve(),
        Some("keep") => keep(),
        _ => {
            eprintln!("olo-pool-host serve | ctl | put-back | forget | keep | --version");
            std::process::ExitCode::from(2)
        }
    }
}

/// The service's own process once it has handed over to a newer version
/// (`host::hand_over`), in place (systemd follows its pid): it follows its
/// children (the copy, and the newer ones each hands over to, given to it as
/// their subreaper when the one before ends), passes the service's signals
/// on to them, and ends once none is left, as the last of them ended, so
/// systemd stops and restarts the service as it did. Asleep otherwise.
fn keep() -> std::process::ExitCode {
    // SAFETY: prctl, sigprocmask, signalfd, poll, read and waitpid with valid
    // arguments, on this process and its own children; kill on its children.
    unsafe {
        libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for s in [libc::SIGCHLD, libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::sigaddset(&mut set, s);
        }
        libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        let signals = libc::signalfd(-1, &set, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK);
        let me = std::process::id();
        let children = || -> Vec<i32> { std::fs::read_to_string(format!("/proc/{me}/task/{me}/children")).unwrap_or_default().split_whitespace().filter_map(|p| p.parse().ok()).collect() };
        let mut last = 0;
        loop {
            loop {
                let mut status = 0;
                if libc::waitpid(-1, &mut status, libc::WNOHANG) <= 0 {
                    break;
                }
                last = status;
            }
            let follow = children();
            if follow.is_empty() || signals < 0 {
                break;
            }
            let mut polled = libc::pollfd { fd: signals, events: libc::POLLIN, revents: 0 };
            while libc::poll(&mut polled, 1, -1) < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {}
            loop {
                let mut info: libc::signalfd_siginfo = std::mem::zeroed();
                let size = std::mem::size_of::<libc::signalfd_siginfo>();
                if libc::read(signals, (&raw mut info).cast(), size) != size as isize {
                    break;
                }
                if let s @ (libc::SIGTERM | libc::SIGINT | libc::SIGHUP) = info.ssi_signo as i32 {
                    for &p in &follow {
                        libc::kill(p, s);
                    }
                }
            }
        }
        if libc::WIFEXITED(last) {
            std::process::ExitCode::from(libc::WEXITSTATUS(last) as u8)
        } else if libc::WIFSIGNALED(last) {
            std::process::ExitCode::from((128 + libc::WTERMSIG(last)) as u8)
        } else {
            std::process::ExitCode::SUCCESS
        }
    }
}

fn serve() -> std::process::ExitCode {
    // (Ended by a signal, what is kept is written first: the vault is
    // saved a batch at a time. Blocked here, before any thread starts,
    // and waited for on the main one.)
    let mut signals: libc::sigset_t = unsafe { std::mem::zeroed() };
    // SAFETY: a signal set of this function's own, made empty, then two
    // signals added, and blocked for this thread and those it starts.
    unsafe {
        libc::sigemptyset(&mut signals);
        libc::sigaddset(&mut signals, libc::SIGTERM);
        libc::sigaddset(&mut signals, libc::SIGINT);
        libc::pthread_sigmask(libc::SIG_BLOCK, &signals, std::ptr::null_mut());
    }
    let shared = gateway::Shared::launch(gateway::port(), || {});
    shared.set_hosted();
    if let Some(why) = shared.pool.lock().unwrap().error.clone().filter(|e| e.starts_with("Can’t listen")) {
        eprintln!("olo-pool-host: {why}");
        return std::process::ExitCode::FAILURE;
    }
    if let Err(e) = host::serve(shared.clone()) {
        eprintln!("olo-pool-host: {e}");
        return std::process::ExitCode::FAILURE;
    }
    // Capture installed Claude Code / Codex clients for each account on this
    // host so quota, reset-credit and claim requests go out as the clients
    // themselves send them (without a capture, Anthropic refuses cedar_ember
    // reads as the wrong surface).
    olo_token_pool::capture::follow(shared.clone(), std::time::Duration::from_secs(15));
    // Claude Code, Codex and Antigravity on this host use the pool (their
    // settings as they were are kept, for `put-back`), and Codex's daemon,
    // which outlives its windows, is kept on them.
    // Wait for CA + TLS proxy initialization before pointing clients, and only
    // point providers whose proxy path initialized cleanly (so a companion-CA
    // write error never leaves Antigravity pointed at a missing cert while
    // Claude Code and Codex keep working).
    let ready = shared.wait_proxy_ready(std::time::Duration::from_secs(10));
    let proxy = intercept::port();
    let point_clients = |ready: &gateway::ProxyReady| {
        if let Some(why) = &ready.agy_error {
            eprintln!("olo-pool-host: Antigravity proxy unavailable: {why}");
        }
        if ready.proxy_ok {
            for p in Provider::ALL {
                if p == Provider::Antigravity && !ready.agy_ok {
                    continue;
                }
                if !clients::pointed(p, proxy)
                    && let Err(e) = clients::set(p, true, proxy)
                {
                    eprintln!("olo-pool-host: {e}");
                }
            }
        } else if let Some(why) = &ready.error {
            eprintln!("olo-pool-host: proxy not ready; skipping client configuration: {why}");
        }
    };
    if ready.finished {
        point_clients(&ready);
    } else {
        // CA initialization is still running; finish client setup once it
        // completes rather than abandoning configuration on slow startups.
        let s = shared.clone();
        let _ = std::thread::Builder::new().name("pool-host-clients".into()).spawn(move || {
            let later = s.wait_proxy_ready(std::time::Duration::from_secs(300));
            if later.finished {
                if let Some(why) = &later.agy_error {
                    eprintln!("olo-pool-host: Antigravity proxy unavailable: {why}");
                }
                if later.proxy_ok {
                    for p in Provider::ALL {
                        if p == Provider::Antigravity && !later.agy_ok {
                            continue;
                        }
                        if !clients::pointed(p, proxy)
                            && let Err(e) = clients::set(p, true, proxy)
                        {
                            eprintln!("olo-pool-host: {e}");
                        }
                    }
                } else if let Some(why) = &later.error {
                    eprintln!("olo-pool-host: proxy not ready; skipping client configuration: {why}");
                }
            }
        });
    }
    codex_daemon::keep_following(std::time::Duration::from_secs(10));
    let mut got: libc::c_int = 0;
    // SAFETY: waits for one of the two signals blocked above.
    unsafe { libc::sigwait(&signals, &mut got) };
    // Flush pending writes BEFORE retiring drain (`save_once` skips writes once retired).
    shared.flush();
    shared.drain.retire();
    let _ = std::fs::remove_file(host::socket());
    std::process::ExitCode::SUCCESS
}
