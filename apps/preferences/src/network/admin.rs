//! `olo-net-admin`, the network's root helper (see its file), as the
//! panel's worker uses it: started through polkit the first time a change
//! needs root (pkexec asks for the password with Olo's agent), then kept
//! and asked again while the panel is open, so the password is asked
//! once. `OLO_NET_ADMIN=direct` (tests) runs it as this user, with
//! `OLO_NET_ROOT` and `OLO_NM_BUS` where it acts.

use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

#[derive(Default)]
pub struct Admin {
    running: Option<(Child, ChildStdin, BufReader<ChildStdout>)>,
}

impl Admin {
    /// The helper's answer to `request`: Ok, or what went wrong, said
    /// for the person.
    pub fn ask(&mut self, request: &serde_json::Value) -> Result<(), String> {
        if self.running.is_none() {
            self.running = Some(start()?);
        }
        let (child, input, output) = self.running.as_mut().expect("started");
        let mut line = String::new();
        let said = writeln!(input, "{request}").and_then(|_| input.flush()).and_then(|_| output.read_line(&mut line));
        match said {
            Ok(n) if n > 0 => {
                let answer: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                match answer["error"].as_str() {
                    Some(e) => Err(e.to_owned()),
                    None => Ok(()),
                }
            }
            // It ended: the password wasn't given (pkexec says 126 or 127),
            // or it failed.
            _ => {
                let code = child.wait().ok().and_then(|s| s.code());
                self.running = None;
                Err(match code {
                    Some(126) | Some(127) => "Not changed: an administrator's password is needed.".to_owned(),
                    _ => "Not changed: the network's helper stopped.".to_owned(),
                })
            }
        }
    }

    /// Let it go (the panel closed): it ends as its input does.
    pub fn end(&mut self) {
        if let Some((mut child, input, _)) = self.running.take() {
            drop(input);
            let _ = child.wait();
        }
    }
}

fn start() -> Result<(Child, ChildStdin, BufReader<ChildStdout>), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = std::path::PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
    let helper = exe.with_file_name("olo-net-admin");
    let direct = std::env::var("OLO_NET_ADMIN").is_ok_and(|v| v == "direct");
    let mut cmd = if direct {
        Command::new(&helper)
    } else if std::env::var_os("OLO_REAL_SESSION").is_some() && std::env::var_os("OLO_SCREEN").is_none() {
        // (Polkit asks with Olo's agent; there's no way across to root in
        // process.)
        let mut c = Command::new("pkexec");
        c.arg("--disable-internal-agent").arg(&helper);
        c
    } else {
        // Elsewhere polkit's dialog would come up on the person's screen.
        return Err("Not changed: changes as an administrator are made in the person's own session.".into());
    };
    let mut child = cmd.arg("serve").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| format!("{}: {e}", helper.display()))?;
    let input = child.stdin.take().ok_or("no input")?;
    let output = BufReader::new(child.stdout.take().ok_or("no output")?);
    Ok((child, input, output))
}
