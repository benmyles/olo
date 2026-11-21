//! Olo's opt-in build of NVIDIA's open modules. The three prevention
//! patches address a failed runtime suspend. All five modules are
//! installed together for the next start. Nothing here opens a GPU,
//! loads or unloads a driver, or changes a device in the running kernel.
//! Two programs do the kernel toolchain's work: make compiles the
//! modules; Fedora's /usr/sbin/depmod indexes one release under the
//! selected root. The index format belongs to kmod: a wrong index is
//! a driver that cannot load. Verify both indexes before promotion.
mod boot;
mod build;
mod index;
mod manage;
mod modules;
mod patches;
#[cfg(test)]
mod real_build;
mod status;
mod system;
#[cfg(test)]
mod tests;
mod watch;

use anyhow::{Result, bail, ensure};
use index::Depmod;
use std::process::ExitCode;
use system::System;

const USAGE: &str = "usage: olo-nvidia status [--json] | build [--kernel RELEASE] [--set prevention] | withdraw [--kernel RELEASE | --all] | check";

fn run(s: &System, args: &[String]) -> Result<()> {
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    if matches!(args.first(), Some(&("build" | "withdraw" | "check" | "watch"))) {
        ensure!(s.root != std::path::Path::new("/") && unsafe { libc::geteuid() } != 0, "NVIDIA module changes are available only in ordinary-user test roots until boot deployment is supported");
    }
    match args.as_slice() {
        ["status"] | ["status", "--json"] => {
            let info = status::status(s)?;
            if args.len() > 1 {
                println!("{}", serde_json::to_string(&info)?);
            } else {
                status::print(&info);
            }
        }
        ["check"] => {
            let _guard = s.lock()?;
            manage::check(s, &Depmod, |r| manage::build(s, r, false, &Depmod))?;
        }
        ["watch"] => watch::watch(s)?,
        ["unit", "service"] => print!("{}", watch::SERVICE),
        ["unit", "watch"] => print!("{}", watch::WATCH_SERVICE),
        ["withdraw", "--all"] => {
            let _guard = s.lock()?;
            system::remove_file(&s.state().join("prevention.enabled"))?;
            let mut errors = vec![];
            for r in s.releases()? {
                if (s.kernel(&r).join(modules::OURS).try_exists()? || s.manifest(&r).try_exists()?)
                    && let Err(e) = manage::withdraw(s, &r, &Depmod)
                {
                    errors.push(format!("{r}: {e:#}"));
                }
            }
            ensure!(errors.is_empty(), "{}", errors.join("\n"));
            println!("Withdrew all Olo NVIDIA sets and stopped following kernel updates.");
        }
        [cmd @ ("build" | "withdraw"), rest @ ..] => {
            let mut release = None;
            let mut set = None;
            for pair in rest.chunks(2) {
                match pair {
                    ["--kernel", r] if release.is_none() => {
                        system::valid_release(r)?;
                        release = Some((*r).to_owned());
                    }
                    ["--set", "prevention"] if *cmd == "build" && set.is_none() => set = Some(()),
                    _ => bail!(USAGE),
                }
            }
            let r = match release {
                Some(r) => r,
                None => s.running()?,
            };
            let _guard = s.lock()?;
            if *cmd == "build" {
                manage::build(s, &r, true, &Depmod)?;
                println!("Five prevention modules installed for {r}'s next start.");
            } else {
                manage::withdraw(s, &r, &Depmod)?;
                println!("The distribution's five NVIDIA modules are selected for {r}.");
            }
        }
        ["--help"] => println!("{USAGE}"),
        _ => bail!(USAGE),
    }
    Ok(())
}
fn main() -> ExitCode {
    olo_crash::install();
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match System::here().and_then(|s| run(&s, &args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("olo-nvidia: {e:#}");
            ExitCode::FAILURE
        }
    }
}
