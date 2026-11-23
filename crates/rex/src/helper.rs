//! Olo's helper on a host (`olo-rex-link`, HACKING, Rex, "A helper on
//! the host"): where it lives there and how it gets there.
//!
//! It is one file under the account's own home,
//! `.local/share/olo/rex-link/olo-rex-link-ID-MACHINE`, where ID names
//! the build and MACHINE is what `uname -m` says there: the app looks
//! for the build it carries, two computers with different Olos use the
//! same host side by side, and a home shared by hosts of two kinds
//! holds a helper for each. It is put there over the SSH connection
//! the host is reached by, as the account that connects: no root,
//! nothing started, nothing on the host's PATH; a file that does
//! nothing until the app runs it (`ssh::dial_command`, which also says
//! what runs where the app's own build isn't there).

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::provision::{Failed, shell_word};
use crate::ssh::{SshAskpass, SshSessionPool, SshTarget};

/// The helper's folder, under the account's home on the host.
pub const DIR: &str = ".local/share/olo/rex-link";

/// The helper's file there for the build `id`, as a shell on the host
/// names it (its machine is the host's to say).
pub fn file(id: &str) -> String {
    format!("\"$HOME\"/{DIR}/olo-rex-link-{id}-$(uname -m)")
}

/// A build of the helper, for one kind of host.
#[derive(Clone, Copy, Debug)]
pub struct Build {
    /// What `uname -s` says there, and what `uname -m` may say.
    pub system: &'static str,
    pub machines: &'static [&'static str],
    pub bytes: &'static [u8],
}

/// The build for a host, by what its `uname -sm` said.
pub fn build_for<'a>(uname: &str, builds: &'a [Build]) -> Option<&'a Build> {
    let mut words = uname.split_whitespace();
    let (system, machine) = (words.next()?, words.next()?);
    builds.iter().find(|b| b.system == system && b.machines.contains(&machine) && !b.bytes.is_empty())
}

/// Asks the host what it is, and whether it has the helper of this
/// build and can run it: `uname -sm`, then `here` if so.
fn asks(id: &str) -> String {
    format!("sh -c {}", shell_word(&format!("uname -sm; L={}; if [ -f \"$L\" ] && [ -O \"$L\" ] && \"$L\" --version > /dev/null 2>&1; then echo here; fi", file(id))))
}

/// Takes the helper on its input and puts it in its place: whole (its
/// length is checked), able to run there (it is asked its version: a
/// home that runs nothing, or a host of another kind, says so here),
/// and in one move, so a connection never finds half a file. The
/// folder is the account's alone. Builds nobody has installed for a
/// month are removed with it.
fn takes(id: &str, len: usize) -> String {
    format!(
        "sh -c {}",
        shell_word(&format!(
            "set -e; d=\"$HOME\"/{DIR}; n=olo-rex-link-{id}-$(uname -m); mkdir -p \"$d\"; chmod 700 \"$d\"; t=\"$d/.new.$$\"; trap 'rm -f \"$t\"' EXIT; cat > \"$t\"; \
             if [ \"$(wc -c < \"$t\" | tr -d ' ')\" != {len} ]; then echo 'it came short'; exit 3; fi; chmod 755 \"$t\"; \
             if ! \"$t\" --version > /dev/null 2>&1; then echo 'it does not run here'; exit 4; fi; mv -f \"$t\" \"$d/$n\"; \
             find \"$d\" -type f -name 'olo-rex-link-*' ! -name \"$n\" -mtime +30 -exec rm -f {{}} + 2> /dev/null || true; echo installed"
        ))
    )
}

/// How the helper came to stand on a host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stands {
    /// It was there already.
    Already,
    /// It was put there now: so many bytes, in so long.
    Installed { bytes: usize, took: Duration },
    /// The app carries no build for such a host (what its `uname -sm`
    /// said).
    NoBuild(String),
}

/// Sees that the host has the helper of build `id`, putting it there
/// where it hasn't. A control connection opened afterwards goes through
/// it ([`SshSessionPool::set_helper`] with `id`).
pub async fn ensure(pool: &SshSessionPool, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>, id: &str, builds: &[Build]) -> Result<Stands, Failed> {
    let failed = |e: crate::error::RexError| Failed { refused: matches!(e, crate::error::RexError::SshRefused(_)), why: e.to_string() };
    let asked = pool.exec(target, askpass.clone(), &asks(id), &mut |_| {}).await.map_err(failed)?;
    let mut lines = asked.output.lines();
    let uname = lines.next().unwrap_or_default().trim().to_owned();
    if !asked.ok() || uname.is_empty() {
        return Err(format!("the host didn't say what it is ({})", asked.output.lines().last().unwrap_or("no answer").trim()).into());
    }
    if lines.any(|l| l.trim() == "here") {
        return Ok(Stands::Already);
    }
    let Some(build) = build_for(&uname, builds) else { return Ok(Stands::NoBuild(uname)) };
    let began = Instant::now();
    let put = pool.put(target, askpass, &takes(id, build.bytes.len()), build.bytes).await.map_err(failed)?;
    if !put.ok() || put.output.lines().last().map(str::trim) != Some("installed") {
        return Err(format!("the helper couldn't be put on the host ({})", put.output.lines().last().unwrap_or("no answer").trim()).into());
    }
    Ok(Stands::Installed { bytes: build.bytes.len(), took: began.elapsed() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    /// A scratch home on disk (/tmp is memory here), removed with it.
    struct Home(std::path::PathBuf);

    impl Home {
        fn new(name: &str) -> Home {
            let dir = std::path::PathBuf::from(format!("/var/tmp/olo-rex-helper-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Home(dir)
        }

        /// A command run as the host would run it, with `input` on its
        /// input: its status and what it wrote.
        fn run(&self, command: &str, input: &[u8]) -> (i32, String) {
            let mut child = Command::new("sh").arg("-c").arg(command).env("HOME", &self.0).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
            let mut stdin = child.stdin.take().unwrap();
            let _ = stdin.write_all(input);
            drop(stdin);
            let out = child.wait_with_output().unwrap();
            (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).trim().to_owned())
        }

        fn files(&self) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(self.0.join(DIR)).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
            names.sort();
            names
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// (A stand-in for the helper that runs anywhere: a script.)
    const RUNS: &[u8] = b"#!/bin/sh\necho olo-rex-link 1\n";

    fn machine() -> String {
        String::from_utf8(Command::new("uname").arg("-m").output().unwrap().stdout).unwrap().trim().to_owned()
    }

    #[test]
    fn the_helper_is_put_in_its_place_whole_and_found_there_after() {
        let (home, m) = (Home::new("put"), machine());
        let (status, said) = home.run(&asks("0a1b"), b"");
        assert_eq!(status, 0);
        assert_eq!(said.lines().count(), 1, "what the host is, and nothing of a helper: {said:?}");
        assert_eq!(home.run(&takes("0a1b", RUNS.len()), RUNS), (0, "installed".to_owned()));
        assert_eq!(home.files(), [format!("olo-rex-link-0a1b-{m}")]);
        use std::os::unix::fs::PermissionsExt;
        let there = home.0.join(DIR);
        assert_eq!(std::fs::metadata(&there).unwrap().permissions().mode() & 0o777, 0o700, "the folder is the account's alone");
        let file = there.join(format!("olo-rex-link-0a1b-{m}"));
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::read(&file).unwrap(), RUNS);
        assert_eq!(home.run(&asks("0a1b"), b"").1.lines().last(), Some("here"));
        // Another build beside it: both stay (two computers, two Olos).
        assert_eq!(home.run(&takes("ffff", RUNS.len()), RUNS).1, "installed");
        assert_eq!(home.files(), [format!("olo-rex-link-0a1b-{m}"), format!("olo-rex-link-ffff-{m}")]);
        assert_ne!(home.run(&asks("9999"), b"").1.lines().last(), Some("here"));
        // One that is there and doesn't run isn't "here": it is put again.
        std::fs::write(&file, b"#!/bin/sh\nexit 1\n").unwrap();
        assert_ne!(home.run(&asks("0a1b"), b"").1.lines().last(), Some("here"));
        assert_eq!(home.run(&takes("0a1b", RUNS.len()), RUNS).1, "installed");
        assert_eq!(home.run(&asks("0a1b"), b"").1.lines().last(), Some("here"));
    }

    #[test]
    fn a_helper_that_came_short_or_doesnt_run_is_not_left_there() {
        let home = Home::new("short");
        // Cut off on its way.
        let (status, said) = home.run(&takes("0a1b", RUNS.len()), &RUNS[..10]);
        assert_eq!((status, said.as_str()), (3, "it came short"));
        assert_eq!(home.files(), Vec::<String>::new());
        // Not a program this host runs.
        let (status, said) = home.run(&takes("0a1b", 4), &[0x7f, b'E', b'L', b'F']);
        assert_eq!((status, said.as_str()), (4, "it does not run here"));
        assert_eq!(home.files(), Vec::<String>::new());
    }

    #[test]
    fn builds_nobody_installed_for_a_month_go_when_a_new_one_comes() {
        let (home, m) = (Home::new("old"), machine());
        assert_eq!(home.run(&takes("old1", RUNS.len()), RUNS).1, "installed");
        assert_eq!(home.run(&takes("new1", RUNS.len()), RUNS).1, "installed");
        // (The first, made forty days old.)
        let old = std::fs::File::options().write(true).open(home.0.join(DIR).join(format!("olo-rex-link-old1-{m}"))).unwrap();
        old.set_modified(std::time::SystemTime::now() - Duration::from_secs(40 * 86400)).unwrap();
        assert_eq!(home.run(&takes("new2", RUNS.len()), RUNS).1, "installed");
        assert_eq!(home.files(), [format!("olo-rex-link-new1-{m}"), format!("olo-rex-link-new2-{m}")]);
    }

    #[test]
    fn a_host_gets_the_build_for_what_it_is_or_none() {
        let builds = [
            Build { system: "Linux", machines: &["x86_64", "amd64"], bytes: b"x" },
            Build { system: "Linux", machines: &["aarch64", "arm64"], bytes: b"a" },
            Build { system: "Linux", machines: &["riscv64"], bytes: b"" },
        ];
        assert_eq!(build_for("Linux x86_64", &builds).map(|b| b.bytes), Some(&b"x"[..]));
        assert_eq!(build_for("Linux aarch64\n", &builds).map(|b| b.bytes), Some(&b"a"[..]));
        assert!(build_for("Darwin arm64", &builds).is_none());
        assert!(build_for("Linux riscv64", &builds).is_none(), "a build that wasn't made is none");
        assert!(build_for("", &builds).is_none());
    }
}
