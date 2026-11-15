//! What the daemon has no call for: a .repo file written into
//! /etc/yum.repos.d, or one taken away. As root: OS Updates' own program
//! run again through pkexec (`olo-updates --helper`; polkit asks for the
//! password with Olo's agent), handed one request as a JSON line on its
//! input, answering with one. It does only those two things, in that one
//! folder, and checks what it's handed itself: a file's name is a plain
//! name ending .repo, its text a .repo file's (every repository saying
//! where it's from), an existing file never written over; a file taken
//! away is a .repo file directly in the folder, not a link.
//!
//! Only from the person's own session at the computer, as Olo Doctor's
//! helper. `OLO_UPDATES_HELPER=direct` does it as this user in the folder
//! `OLO_UPDATES_REPOS_DIR` names instead (tests: `dnf-fake --repos` reads
//! it); `=off` never.

use std::{
    io::{BufRead, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde::{Deserialize, Serialize};

pub const REPOS_DIR: &str = "/etc/yum.repos.d";

#[derive(Debug, Serialize, Deserialize)]
enum Request {
    Write { name: String, text: String },
    Remove { path: String },
}

#[derive(Debug, Serialize, Deserialize)]
enum Reply {
    Done(String),
    Failed(String),
}

enum Way {
    Pkexec,
    Direct(PathBuf),
    Off(String),
}

fn way() -> Way {
    match std::env::var("OLO_UPDATES_HELPER").as_deref() {
        Ok("direct") => match std::env::var("OLO_UPDATES_REPOS_DIR") {
            Ok(d) if !d.is_empty() => Way::Direct(PathBuf::from(d)),
            _ => Way::Off("OLO_UPDATES_HELPER=direct needs OLO_UPDATES_REPOS_DIR.".into()),
        },
        Ok("off") => Way::Off("Writing repositories is turned off here (OLO_UPDATES_HELPER).".into()),
        _ if olo_system::dnf::real() && olo_system::env::at_the_computer() => Way::Pkexec,
        _ => Way::Off("Repositories are added and removed only from the session at the computer.".into()),
    }
}

/// The folder repository files are written to and taken from: the
/// system's, or a test's (`OLO_UPDATES_HELPER=direct`).
pub fn repos_dir() -> PathBuf {
    match way() {
        Way::Direct(dir) => dir,
        _ => PathBuf::from(REPOS_DIR),
    }
}

/// Write `/etc/yum.repos.d/NAME` with `text`; where it went.
pub fn write_repo(name: &str, text: &str) -> Result<String, String> {
    ask(Request::Write { name: name.to_owned(), text: text.to_owned() })
}

/// Take away the .repo file at `path`.
pub fn remove_repo(path: &str) -> Result<(), String> {
    ask(Request::Remove { path: path.to_owned() }).map(|_| ())
}

fn ask(request: Request) -> Result<String, String> {
    match way() {
        Way::Off(why) => Err(why),
        Way::Direct(dir) => carry_out(&dir, request),
        Way::Pkexec => {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            // An upgrade replaced the program under us: its new self is at
            // the same path.
            let exe = PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
            let mut child = Command::new("pkexec")
                .arg("--disable-internal-agent")
                .arg(&exe)
                .arg("--helper")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("pkexec couldn’t be started: {e}"))?;
            let mut line = serde_json::to_string(&request).map_err(|e| e.to_string())?;
            line.push('\n');
            if let Some(mut input) = child.stdin.take() {
                input.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
            }
            let mut answer = String::new();
            if let Some(out) = child.stdout.take() {
                let _ = std::io::BufReader::new(out).read_line(&mut answer);
            }
            let status = child.wait().map_err(|e| e.to_string())?;
            match serde_json::from_str::<Reply>(answer.trim()) {
                Ok(Reply::Done(s)) => Ok(s),
                Ok(Reply::Failed(e)) => Err(e),
                // pkexec: 126 refused, 127 not authorized or cancelled.
                Err(_) if matches!(status.code(), Some(126 | 127)) => Err("Not allowed (authentication was refused or cancelled).".into()),
                Err(_) => Err("The helper didn’t answer.".into()),
            }
        }
    }
}

/// Whether `name` is a plain .repo file's name.
fn good_name(name: &str) -> bool {
    name.ends_with(".repo") && name.len() <= 200 && !name.starts_with('.') && name.chars().all(|c| c.is_ascii_alphanumeric() || "._:+-@".contains(c))
}

/// Do what's asked, in `dir`.
fn carry_out(dir: &Path, request: Request) -> Result<String, String> {
    match request {
        Request::Write { name, text } => {
            if !good_name(&name) {
                return Err(format!("“{name}” isn’t a .repo file’s name."));
            }
            crate::repos::parse_repo_file(&text)?;
            let path = dir.join(&name);
            if path.symlink_metadata().is_ok() {
                return Err(format!("{} is there already.", path.display()));
            }
            let tmp = dir.join(format!(".{name}.olo-new"));
            {
                use std::os::unix::fs::OpenOptionsExt;
                let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o644).open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
                f.write_all(text.as_bytes()).and_then(|()| f.sync_all()).map_err(|e| {
                    let _ = std::fs::remove_file(&tmp);
                    format!("{}: {e}", tmp.display())
                })?;
            }
            std::fs::rename(&tmp, &path).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                format!("{}: {e}", path.display())
            })?;
            Ok(path.display().to_string())
        }
        Request::Remove { path } => {
            let p = Path::new(&path);
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            if p.parent() != Some(dir) || !good_name(name) {
                return Err(format!("{path} isn’t a .repo file in {}.", dir.display()));
            }
            let meta = p.symlink_metadata().map_err(|e| format!("{path}: {e}"))?;
            if !meta.file_type().is_file() {
                return Err(format!("{path} isn’t a plain file."));
            }
            std::fs::remove_file(p).map_err(|e| format!("{path}: {e}"))?;
            Ok(path)
        }
    }
}

/// `olo-updates --helper`: one request on its input, as root, answered.
pub fn serve() -> std::process::ExitCode {
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        return std::process::ExitCode::FAILURE;
    }
    let reply = match serde_json::from_str::<Request>(line.trim()) {
        Ok(r) => match carry_out(Path::new(REPOS_DIR), r) {
            Ok(s) => Reply::Done(s),
            Err(e) => Reply::Failed(e),
        },
        Err(e) => Reply::Failed(format!("Not a request: {e}")),
    };
    println!("{}", serde_json::to_string(&reply).unwrap_or_default());
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_repo_files_in_their_folder() {
        // (Beside the test's program, on disk: never /tmp, which is RAM.)
        let dir = std::env::current_exe().unwrap().parent().unwrap().join(format!("olo-updates-helper-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = "[olo]\nname=Olo\nbaseurl=https://example.org/\n";
        let path = carry_out(&dir, Request::Write { name: "olo.repo".into(), text: text.into() }).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        assert!(carry_out(&dir, Request::Write { name: "olo.repo".into(), text: text.into() }).is_err(), "never written over");
        assert!(carry_out(&dir, Request::Write { name: "../passwd.repo".into(), text: text.into() }).is_err());
        assert!(carry_out(&dir, Request::Write { name: "x.conf".into(), text: text.into() }).is_err());
        assert!(carry_out(&dir, Request::Write { name: "bad.repo".into(), text: "<html>".into() }).is_err());
        assert!(carry_out(&dir, Request::Remove { path: "/etc/passwd".into() }).is_err());
        carry_out(&dir, Request::Remove { path }).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
