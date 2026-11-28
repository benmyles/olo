//! Where a shell says it is.
//!
//! A new tab or window opens in the folder its tab is in (Cmd+T, Cmd+N),
//! which Terminal also shows and keeps for Undo Close Tab. Most shells
//! move their process there on `cd`, and `/proc/PID/cwd` says. Some
//! don't: brush keeps its folder for what it runs, its process staying
//! where it started (Olo's own brush moves it at each prompt; one
//! installed apart, a login shell, doesn't, and a new tab opened in the
//! home folder while its tab's title showed where it was: QA,
//! 2026-11-28). Those are followed by what the shell itself says at its
//! prompt: OSC 7 (`file://HOST/PATH`, as fish, VTE's shell integration
//! and others send it), or a title of the usual `USER@HOST:DIR` (bash's
//! on Fedora and most systems, at every prompt), each only of this
//! computer and this user, the newer of the two its word.
//!
//! Only what the shell itself says: a report is kept only while the
//! shell has the terminal (a program in front, a nested shell or ssh's
//! far one, says where *it* is), so the shell's word stands while that
//! program runs, and after. Only while its process stays where it was
//! when it first said where it is: one seen to have moved follows its
//! `cd`, and from then on `/proc` is what's read, as before (a title set
//! once and never again says nothing then); where `/proc` can't be read,
//! nothing is said either. And only a path a new shell can be given:
//! UTF-8, no NUL (one that isn't can't go to olo-termd, nor be kept with
//! the window's tabs).
//!
//! Nothing here looks at the folder itself (a `stat` on a mount that
//! hangs would hang Terminal): a new shell is started there by
//! olo-termd, whose child stays home where it can't go (`Pty::spawn`).

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

/// Where a shell's word came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Osc7,
    Title,
}

/// What a shell has said of where it is.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Said {
    /// The folder it last said it's in, and how (OSC 7, or its title).
    word: Option<(PathBuf, Source)>,
    /// Where its process was when it first said where it is.
    first_cwd: Option<PathBuf>,
    /// Its process has been seen somewhere else since: it follows its
    /// `cd`, and what it says isn't needed (nor trusted over `/proc`).
    moved: bool,
}

/// This computer and user, as a shell names them.
#[derive(Debug, Clone)]
pub struct Here {
    pub user: String,
    /// Its host name, as the kernel has it (maybe with its domain).
    pub host: String,
    pub home: PathBuf,
}

/// This computer and user (read once: Terminal's own environment, the
/// shells' too).
pub fn here() -> &'static Here {
    static HERE: OnceLock<Here> = OnceLock::new();
    HERE.get_or_init(|| Here {
        user: std::env::var("USER").or_else(|_| std::env::var("LOGNAME")).unwrap_or_default(),
        host: std::fs::read_to_string("/proc/sys/kernel/hostname").map(|h| h.trim().to_owned()).unwrap_or_default(),
        home: std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default(),
    })
}

impl Said {
    /// It sent OSC 7 (`uri`, `file://HOST/PATH`), the shell having the
    /// terminal (`its`: else it's a program in front's, not kept): its
    /// folder, if of this computer. `cwd` is where its process is now.
    pub fn osc7(&mut self, uri: &[u8], its: bool, here: &Here, cwd: Option<&Path>) {
        if !its {
            return;
        }
        match osc7_local(uri, here) {
            Some(Some(dir)) => {
                self.word = Some((dir, Source::Osc7));
                self.saw(cwd);
            }
            // (Of this computer, but a folder no new shell can be given:
            // what it said before is old news, set aside, its process's
            // folder read instead; kept, the next tab and the layout went
            // to the folder it had left: Astra's review, 2026-11-28.)
            Some(None) => self.word = None,
            // (Another computer's, or not a report: nothing of ours.)
            None => {}
        }
    }

    /// Its title changed (None: reset), the shell having the terminal
    /// (`its`): the folder it says, while it's of the usual
    /// `USER@HOST:DIR` (the newer word); one that says no folder ("make
    /// -j16") takes back only what a title said. `cwd` as for [`Self::osc7`].
    pub fn title(&mut self, title: Option<&str>, its: bool, here: &Here, cwd: Option<&Path>) {
        if !its {
            return;
        }
        match title.and_then(|t| title_dir(t, here)) {
            Some(dir) => {
                self.word = Some((dir, Source::Title));
                self.saw(cwd);
            }
            None if self.word.as_ref().is_some_and(|(_, s)| *s == Source::Title) => self.word = None,
            None => {}
        }
    }

    /// Its process seen at `cwd` (unread: None): kept the first time, and
    /// latched as moved once it's anywhere else.
    fn saw(&mut self, cwd: Option<&Path>) {
        match (&self.first_cwd, cwd) {
            (None, Some(now)) => self.first_cwd = Some(now.to_path_buf()),
            (Some(first), Some(now)) if first != now => self.moved = true,
            _ => {}
        }
    }

    /// The folder the shell is in by its own word, where that's to be
    /// followed: its process read (`cwd_now`) where it was when it first
    /// said where it is, never seen elsewhere. (Whether the shell has the
    /// terminal is the caller's to say: its word stands while a program
    /// of its runs, for what's kept of the tab; a new tab opens where the
    /// program in front is.)
    pub fn followed(&mut self, cwd_now: Option<&Path>) -> Option<PathBuf> {
        self.word.as_ref()?;
        let now = cwd_now?;
        self.saw(Some(now));
        let (word, _) = self.word.as_ref()?;
        (!self.moved && self.first_cwd.as_deref() == Some(now)).then(|| word.clone())
    }

    /// Whether there's a word to follow at all (else nothing's read).
    pub fn has_word(&self) -> bool {
        self.word.is_some() && !self.moved
    }

    /// Where it says it is, as [`Self::followed`] has it (its process
    /// read now, `cwd_now`, still where it was when it first said, never
    /// seen elsewhere), said again as OSC 7 (its path %-encoded, no host:
    /// this computer's), whether it said so by OSC 7 or by its title: for
    /// olo-termd's replay to a Terminal attaching or over an upgrade, the
    /// shell's folder kept whatever program is in front then. (Its word
    /// by OSC 7 alone, unchecked, replayed after a newer title, or after
    /// the process had moved: Astra's and Neon's reviews, 2026-11-28.)
    #[allow(dead_code)] // (Only olo-termd replays.)
    pub fn report(&mut self, cwd_now: Option<&Path>) -> Option<Vec<u8>> {
        let dir = self.followed(cwd_now)?;
        let mut out = b"\x1b]7;file://".to_vec();
        for b in dir.to_str()?.bytes() {
            if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
                out.push(b);
            } else {
                out.extend(format!("%{b:02X}").bytes());
            }
        }
        out.push(0x07);
        Some(out)
    }
}

/// Whether `host` (a shell's) is this computer: by its whole name where
/// it has a domain, by the name before its domain where it hasn't
/// (`build.east.example` is not `build.west.example`).
fn is_here(host: &str, here: &Here) -> bool {
    if here.host.is_empty() {
        return false;
    }
    if host.contains('.') {
        return host.eq_ignore_ascii_case(&here.host);
    }
    here.host.split('.').next().is_some_and(|short| host.eq_ignore_ascii_case(short))
}

/// A path a new shell can be given (and the window's tabs kept with):
/// UTF-8, no NUL.
fn usable(path: String) -> Option<PathBuf> {
    (!path.contains('\0')).then(|| PathBuf::from(path))
}

/// Whether `path` can be handed on: to olo-termd for a new shell (as
/// JSON), and kept with the window's tabs. One read from `/proc` may not
/// be (a folder named with a byte that isn't UTF-8): the spawn's request
/// was then empty, olo-termd never answered and the new tab waited, and
/// the layout wasn't kept (Astra's review, 2026-11-28).
pub fn representable(path: &Path) -> bool {
    path.to_str().is_some_and(|s| !s.contains('\0'))
}

/// The folder of an OSC 7 report, `file://HOST/PATH` (its path
/// %-encoded), where HOST is this computer, `localhost` or none.
#[cfg(test)]
fn osc7_dir(uri: &[u8], here: &Here) -> Option<PathBuf> {
    osc7_local(uri, here).flatten()
}

/// [`osc7_dir`], telling a report of this computer whose folder can't be
/// handed on (`Some(None)`: not UTF-8 when %-decoded, a NUL, a broken
/// %-escape) from one that isn't this computer's, or isn't a report
/// (None).
fn osc7_local(uri: &[u8], here: &Here) -> Option<Option<PathBuf>> {
    let rest = uri.strip_prefix(b"file://")?;
    let slash = rest.iter().position(|&b| b == b'/')?;
    let host = std::str::from_utf8(&rest[..slash]).ok()?;
    if !(host.is_empty() || host.eq_ignore_ascii_case("localhost") || is_here(host, here)) {
        return None;
    }
    Some(decoded(&rest[slash..]))
}

/// A report's %-encoded path, decoded, where a new shell can be given it.
fn decoded(encoded: &[u8]) -> Option<PathBuf> {
    let mut path = Vec::with_capacity(encoded.len());
    let mut bytes = encoded.iter();
    while let Some(&b) = bytes.next() {
        if b == b'%' {
            let hex = [*bytes.next()?, *bytes.next()?];
            path.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            path.push(b);
        }
    }
    usable(String::from_utf8(path).ok()?)
}

/// The folder a title of the usual `USER@HOST:DIR` (`USER@HOST: DIR` on
/// Debian: the one space after the colon isn't the folder's) says, `~`
/// for the home folder, where USER is this user and HOST this computer,
/// and DIR a whole path, every byte of it its own.
fn title_dir(title: &str, here: &Here) -> Option<PathBuf> {
    let (who, dir) = title.split_once(':')?;
    let (user, host) = who.split_once('@')?;
    if here.user.is_empty() || user != here.user || !is_here(host, here) {
        return None;
    }
    let dir = dir.strip_prefix(' ').unwrap_or(dir);
    if dir == "~" {
        return Some(here.home.clone()).filter(|h| h.is_absolute());
    }
    if let Some(within) = dir.strip_prefix("~/") {
        return here.home.is_absolute().then(|| here.home.join(within)).and_then(|p| usable(p.into_os_string().into_string().ok()?));
    }
    dir.starts_with('/').then(|| usable(dir.to_owned())).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt as _;

    fn here() -> Here {
        Here { user: "ada".into(), host: "olo-box.example".into(), home: PathBuf::from("/home/ada") }
    }

    #[test]
    fn an_osc_7_of_this_computer() {
        let h = here();
        assert_eq!(osc7_dir(b"file://olo-box/home/ada/My%20Work", &h), Some(PathBuf::from("/home/ada/My Work")));
        assert_eq!(osc7_dir(b"file://OLO-BOX.example/srv", &h), Some(PathBuf::from("/srv")));
        assert_eq!(osc7_dir(b"file:///tmp", &h), Some(PathBuf::from("/tmp")));
        assert_eq!(osc7_dir(b"file://localhost/tmp", &h), Some(PathBuf::from("/tmp")));
        // Another computer's (ssh's far shell), or not a folder's.
        assert_eq!(osc7_dir(b"file://far-away/home/ada", &h), None);
        assert_eq!(osc7_dir(b"https://olo-box/x", &h), None);
        assert_eq!(osc7_dir(b"file://olo-box", &h), None);
        assert_eq!(osc7_dir(b"file://olo-box/bad%2", &h), None);
        // Another computer of the same name in another domain.
        assert_eq!(osc7_dir(b"file://olo-box.elsewhere/srv", &h), None);
        // Not a path a new shell can be given: not UTF-8, or a NUL in it.
        assert_eq!(osc7_dir(b"file:///tmp/%FF", &h), None);
        assert_eq!(osc7_dir(b"file:///tmp/a%00b", &h), None);
    }

    #[test]
    fn a_title_of_user_at_host() {
        let h = here();
        assert_eq!(title_dir("ada@olo-box:~", &h), Some(PathBuf::from("/home/ada")));
        assert_eq!(title_dir("ada@olo-box:~/src/olo", &h), Some(PathBuf::from("/home/ada/src/olo")));
        assert_eq!(title_dir("ada@olo-box: /var/tmp/a:b", &h), Some(PathBuf::from("/var/tmp/a:b")));
        // Its folder's own spaces kept: "draft " isn't "draft".
        assert_eq!(title_dir("ada@olo-box:/home/ada/draft ", &h), Some(PathBuf::from("/home/ada/draft ")));
        // Not this user, not this computer, or not a folder.
        assert_eq!(title_dir("root@olo-box:/etc", &h), None);
        assert_eq!(title_dir("ada@far-away:~", &h), None);
        assert_eq!(title_dir("ada@olo-box.elsewhere:~", &h), None);
        assert_eq!(title_dir("vim notes.txt", &h), None);
        assert_eq!(title_dir("ada@olo-box:relative", &h), None);
        assert_eq!(title_dir("✳ Fix the login: ada@olo-box", &h), None);
    }

    /// Brush in a tab: its process stays where it started, and what it
    /// says is followed, the newer word of the two; bash moves its
    /// process, which is followed as before, for good; what a program in
    /// front says (a nested shell, ssh's far one) is never kept.
    #[test]
    fn what_is_followed() {
        let h = here();
        let home = Path::new("/home/ada");
        let mut brush = Said::default();
        brush.title(Some("ada@olo-box:~"), true, &h, Some(home));
        brush.title(Some("ada@olo-box:~/fixtures"), true, &h, Some(home));
        assert_eq!(brush.followed(Some(home)), Some(PathBuf::from("/home/ada/fixtures")));
        assert_eq!(brush.followed(None), None, "its process unread: nothing said for it");
        // A nested shell's OSC 7 while it has the terminal: not kept.
        brush.osc7(b"file:///opt/app", false, &h, Some(home));
        assert_eq!(brush.followed(Some(home)), Some(PathBuf::from("/home/ada/fixtures")));
        // The shell's own OSC 7, then a newer title: the newer word.
        brush.osc7(b"file://olo-box/srv/data", true, &h, Some(home));
        assert_eq!(brush.followed(Some(home)), Some(PathBuf::from("/srv/data")));
        brush.title(Some("ada@olo-box:~/work"), true, &h, Some(home));
        assert_eq!(brush.followed(Some(home)), Some(PathBuf::from("/home/ada/work")));
        // A title of no folder takes back only a title's word.
        brush.title(Some("make -j16"), true, &h, Some(home));
        assert_eq!(brush.followed(Some(home)), None);
        brush.osc7(b"file:///srv/data", true, &h, Some(home));
        brush.title(Some("make -j16"), true, &h, Some(home));
        assert_eq!(brush.followed(Some(home)), Some(PathBuf::from("/srv/data")), "an OSC 7's word stands");
        // Bash: its process moved with it, so /proc is read, and stays
        // read once it's back where it started (cd ~).
        let mut bash = Said::default();
        bash.title(Some("ada@olo-box:~"), true, &h, Some(home));
        bash.title(Some("ada@olo-box:/tmp"), true, &h, Some(Path::new("/tmp")));
        bash.title(Some("ada@olo-box:~"), true, &h, Some(home));
        assert_eq!(bash.followed(Some(home)), None, "moved once: /proc from then on");
        assert!(!bash.has_word());
        // A lookup that sees it moved retires it too.
        let mut seen = Said::default();
        seen.title(Some("ada@olo-box:~/project"), true, &h, Some(home));
        assert_eq!(seen.followed(Some(Path::new("/elsewhere"))), None);
        assert_eq!(seen.followed(Some(home)), None, "back where it was: still moved");
        assert_eq!(Said::default().followed(Some(home)), None);
    }

    /// An OSC 7 word said again (olo-termd's replay) says the same
    /// folder; a title's isn't (the title is replayed itself).
    #[test]
    fn an_osc_7_word_is_said_again_the_same() {
        let h = here();
        let home = Some(Path::new("/home/ada"));
        let mut said = Said::default();
        said.osc7(b"file://olo-box/home/ada/My%20Work%20%C3%A9", true, &h, home);
        let report = said.report(home).unwrap();
        let uri = report.strip_prefix(b"\x1b]7;").and_then(|r| r.strip_suffix(b"\x07")).unwrap();
        assert_eq!(osc7_dir(uri, &h), Some(PathBuf::from("/home/ada/My Work é")));
        // A newer title says where it is now: that, said again.
        said.title(Some("ada@olo-box:~/work"), true, &h, home);
        let report = said.report(home).unwrap();
        let uri = report.strip_prefix(b"\x1b]7;").and_then(|r| r.strip_suffix(b"\x07")).unwrap();
        assert_eq!(osc7_dir(uri, &h), Some(PathBuf::from("/home/ada/work")));
        // Its process seen elsewhere (bash moves it): nothing said again.
        assert_eq!(said.report(Some(Path::new("/var"))), None);
        assert_eq!(said.report(home), None, "latched");
    }

    /// A report of this computer whose folder can't be handed on sets
    /// what it said before aside; another computer's leaves it be.
    #[test]
    fn an_unusable_report_of_here_sets_the_old_one_aside() {
        let h = here();
        let home = Some(Path::new("/home/ada"));
        let mut said = Said::default();
        said.osc7(b"file:///home/ada/a", true, &h, home);
        assert_eq!(said.followed(home), Some(PathBuf::from("/home/ada/a")));
        said.osc7(b"file://far.example/srv/b", true, &h, home);
        assert_eq!(said.followed(home), Some(PathBuf::from("/home/ada/a")), "another computer's: kept");
        said.osc7(b"file:///home/ada/caf%FF", true, &h, home);
        assert_eq!(said.followed(home), None, "set aside: /proc read instead");
        assert!(!said.has_word());
        assert!(representable(Path::new("/home/ada/a")));
        assert!(!representable(Path::new(std::ffi::OsStr::from_bytes(b"/home/ada/caf\xff"))));
    }
}
