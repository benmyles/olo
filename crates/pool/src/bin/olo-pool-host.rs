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
            for p in [Provider::Claude, Provider::Codex] {
                if let Err(e) = clients::put_back(p) {
                    eprintln!("olo-pool-host: {e}");
                    ok = false;
                }
            }
            if ok { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE }
        }
        Some("serve") => serve(),
        _ => {
            eprintln!("olo-pool-host serve | ctl | put-back | --version");
            std::process::ExitCode::from(2)
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
    // Claude Code and Codex on this host use the pool (their settings
    // as they were are kept, for `put-back`), and Codex's daemon, which
    // outlives its windows, is kept on them.
    let proxy = intercept::port();
    for p in [Provider::Claude, Provider::Codex] {
        if !clients::pointed(p, proxy)
            && let Err(e) = clients::set(p, true, proxy)
        {
            eprintln!("olo-pool-host: {e}");
        }
    }
    codex_daemon::keep_following(std::time::Duration::from_secs(10));
    let mut got: libc::c_int = 0;
    // SAFETY: waits for one of the two signals blocked above.
    unsafe { libc::sigwait(&signals, &mut got) };
    shared.flush();
    let _ = std::fs::remove_file(host::socket());
    std::process::ExitCode::SUCCESS
}
