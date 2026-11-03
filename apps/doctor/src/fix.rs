//! What a fix is made of: small steps the app runs as this user, or the
//! privileged helper (helper.rs) runs as root, each doing one thing in
//! process (a file written, a sysfs knob set, a systemd unit stopped
//! over D-Bus): nothing is shelled out to.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UnitAction {
    Start,
    Stop,
    Enable,
    Disable,
    /// Have systemd re-read its unit files (after one is removed).
    Reload,
    /// Start it, or start it again (an updated program).
    Restart,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Step {
    /// Write a whole file (through a sibling, then renamed into place).
    WriteFile { path: PathBuf, content: String, mode: u32 },
    /// Copy a file (a program) whole, owned by whoever runs the step.
    CopyFile { from: PathBuf, to: PathBuf, mode: u32 },
    /// Have NetworkManager read its configuration again.
    NetworkManagerReload,
    /// Write a value to a sysfs attribute.
    WriteSysfs { path: PathBuf, value: String },
    /// Remove a file (already gone is fine).
    RemoveFile { path: PathBuf },
    /// Act on a system (not user) systemd unit.
    Unit { name: String, action: UnitAction },
    Signal { pid: i32, signal: i32 },
    /// List a program in /etc/shells (a login shell must be there).
    AddShell { path: PathBuf },
    /// Run a shell once (`-c 'exit 0'`): it starts, before it's anyone's
    /// login shell.
    TryShell { path: PathBuf },
    /// Make `shell` `user`'s login shell (AccountsService's SetShell).
    SetLoginShell { user: String, shell: PathBuf },
}

impl Step {
    /// What it does, for a report ("Writing /etc/udev/rules.d/…").
    pub fn describe(&self) -> String {
        match self {
            Step::WriteFile { path, .. } => format!("Writing {}", path.display()),
            Step::CopyFile { to, .. } => format!("Installing {}", to.display()),
            Step::NetworkManagerReload => "Reloading NetworkManager's settings".into(),
            Step::WriteSysfs { path, value } => format!("Setting {} to {value}", path.display()),
            Step::RemoveFile { path } => format!("Removing {}", path.display()),
            Step::Unit { name, action } => {
                let verb = match action {
                    UnitAction::Start => "Starting",
                    UnitAction::Stop => "Stopping",
                    UnitAction::Enable => "Enabling",
                    UnitAction::Disable => "Disabling",
                    UnitAction::Reload => "Reloading systemd for",
                    UnitAction::Restart => "Restarting",
                };
                format!("{verb} {name}")
            }
            Step::Signal { pid, .. } => format!("Stopping process {pid}"),
            Step::AddShell { path } => format!("Listing {} in /etc/shells", path.display()),
            Step::TryShell { path } => format!("Trying {}", path.display()),
            Step::SetLoginShell { user, shell } => format!("Making {} {user}'s login shell", shell.display()),
        }
    }

    /// Do it (as whoever runs this).
    pub fn run(&self) -> Result<(), String> {
        let failed = |e: std::io::Error| format!("{}: {e}", self.describe());
        match self {
            Step::WriteFile { path, content, mode } => write_file(path, content, *mode).map_err(failed),
            Step::CopyFile { from, to, mode } => std::fs::read(from).and_then(|bytes| write_bytes(to, &bytes, *mode)).map_err(failed),
            Step::NetworkManagerReload => olo_system::network::reload().map_err(|e| format!("{}: {e}", self.describe())),
            Step::WriteSysfs { path, value } => std::fs::write(path, value).map_err(failed),
            Step::RemoveFile { path } => match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(failed(e)),
                _ => Ok(()),
            },
            Step::Unit { name, action } => unit(name, action).map_err(|e| format!("{}: {e}", self.describe())),
            Step::Signal { pid, signal } => {
                // SAFETY: kill with a pid and signal number has no memory effects.
                if unsafe { libc::kill(*pid, *signal) } == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                    Ok(())
                } else {
                    Err(failed(std::io::Error::last_os_error()))
                }
            }
            Step::AddShell { path } => add_shell(path).map_err(failed),
            Step::TryShell { path } => try_shell(path),
            Step::SetLoginShell { user, shell } => set_login_shell(user, shell).map_err(|e| format!("{}: {e}", self.describe())),
        }
    }
}

/// `path` listed in /etc/shells (it is already: nothing written).
fn add_shell(path: &Path) -> std::io::Result<()> {
    add_shell_to(Path::new("/etc/shells"), path)
}

/// `path` listed in the shells file `shells`.
fn add_shell_to(shells: &Path, path: &Path) -> std::io::Result<()> {
    let text = std::fs::read_to_string(shells).unwrap_or_default();
    let line = path.to_string_lossy();
    if text.lines().any(|l| l.trim() == line) {
        return Ok(());
    }
    let mut text = text;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&line);
    text.push('\n');
    write_file(shells, &text, 0o644)
}

/// The shell at `path` runs a command and exits cleanly.
fn try_shell(path: &Path) -> Result<(), String> {
    // (Olo's own build of it: the one program a step runs.)
    match std::process::Command::new(path).args(["-c", "exit 0"]).stdin(std::process::Stdio::null()).status() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("{} didn't run a command ({s})", path.display())),
        Err(e) => Err(format!("{} didn't start: {e}", path.display())),
    }
}

/// `user`'s login shell made `shell`, through AccountsService (which
/// asks polkit; root, as the helper is, may).
fn set_login_shell(user: &str, shell: &Path) -> Result<(), String> {
    olo_system::accounts::set_shell(user, &shell.to_string_lossy()).map_err(|e| e.to_string())
}

fn write_file(path: &Path, content: &str, mode: u32) -> std::io::Result<()> {
    write_bytes(path, content.as_bytes(), mode)
}

fn write_bytes(path: &Path, content: &[u8], mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = path.with_file_name(format!(".{name}.olo-new"));
    std::fs::write(&tmp, content)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    std::fs::rename(&tmp, path)
}

pub use olo_system::systemd::UnitState;

/// What systemd says about a system unit.
pub fn unit_state(name: &str) -> Result<UnitState, String> {
    olo_system::systemd::unit_state(name).map_err(|e| e.to_string())
}

fn unit(name: &str, action: &UnitAction) -> Result<(), String> {
    use olo_system::systemd;
    match action {
        UnitAction::Start => systemd::start_stop(name, true),
        UnitAction::Stop => systemd::start_stop(name, false),
        UnitAction::Enable => systemd::enable(name, true),
        UnitAction::Disable => systemd::enable(name, false),
        UnitAction::Reload => systemd::reload(),
        UnitAction::Restart => systemd::restart(name),
    }
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_round_trip() {
        let steps = vec![
            Step::WriteFile { path: "/etc/x".into(), content: "a\n".into(), mode: 0o644 },
            Step::Unit { name: "keyd.service".into(), action: UnitAction::Disable },
            Step::Signal { pid: 4, signal: 15 },
        ];
        let back: Vec<Step> = serde_json::from_str(&serde_json::to_string(&steps).unwrap()).unwrap();
        assert_eq!(back, steps);
    }

    #[test]
    fn files_are_written_whole_and_removed() {
        let dir = PathBuf::from(format!("/var/tmp/olo-doctor-fix-test-{}", std::process::id()));
        let path = dir.join("rules.d/71.rules");
        Step::WriteFile { path: path.clone(), content: "x\n".into(), mode: 0o644 }.run().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "x\n");
        assert!(std::fs::read_dir(path.parent().unwrap()).unwrap().count() == 1, "no temporary left behind");
        Step::RemoveFile { path: path.clone() }.run().unwrap();
        assert!(!path.exists());
        Step::RemoveFile { path }.run().unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn signalling_a_gone_process_is_fine() {
        // No such pid (the largest allowed is far below), and a signal
        // 0 never changes anything.
        assert!(Step::Signal { pid: i32::MAX, signal: 0 }.run().is_ok());
    }
}
