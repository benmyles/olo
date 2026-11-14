//! `OLO_DOCTOR_DEMO=1`: a made-up report, one of every kind of finding
//! (a critical one, problems with a fix and without, what's recommended,
//! suggestions), what's fine, an undo and something ignored, so the
//! window can be looked at and tried headless without looking at the
//! computer it runs on, nor telling its Deskbar. Its fixes change
//! nothing: each takes a moment and is then gone from the next look
//! (Intel's video driver's fails, to show a failure; the worktrunk
//! suggestion and its undo turn into each other). Ignoring is real (the
//! session's config folder's list): with no list yet, GNOME's services
//! are ignored, to show the section.

use std::{
    collections::BTreeSet,
    sync::{Mutex, PoisonError},
    time::Duration,
};

use crate::checks::{Finding, Fine, Fix, Report, Severity, Undo};

pub fn on() -> bool {
    std::env::var_os("OLO_DOCTOR_DEMO").is_some_and(|v| !v.is_empty() && v != "0")
}

/// What its fixes have fixed ("undone:worktrunk": its undo pressed).
static FIXED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

const WORKTRUNK: &str = "worktrunk-place";
const UNDONE_WORKTRUNK: &str = "undone:worktrunk";

fn finding(id: &str, severity: Severity, title: &str, detail: &str, fix: Option<(&str, bool)>) -> Finding {
    Finding { id: id.into(), severity, title: title.into(), detail: detail.into(), fix: fix.map(|(label, root)| Fix { label: label.into(), root, steps: Vec::new() }) }
}

pub fn report() -> Report {
    let fixed = FIXED.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let mut r = Report::default();
    r.findings = vec![
        finding(
            "diskbackup:damaged",
            Severity::Critical,
            "The last backup isn't whole",
            "The backup DiskBackup made at 09:12 on the backup disk “Archive” stopped part way through (the disk was unplugged), and the one before it is from 6 days ago. Back up again now, with the disk plugged in, so what changed since is kept.",
            Some(("Back Up Now", false)),
        ),
        finding(
            "remapper:keyd",
            Severity::Problem,
            "keyd grabs the keyboards and mice",
            "keyd, a key remapper, takes every keyboard and mouse for itself and passes on what they do through a device of its own: Olo sees one keyboard, its pointer lags under load, and the touchpad's gestures don't reach it. Olo remaps keys itself (Preferences ▸ Keyboard).",
            Some(("Stop and Uninstall keyd", true)),
        ),
        finding(
            "login:handed-over",
            Severity::Problem,
            "Olo's login screen couldn't show this time",
            "At this start Olo's login screen didn't come up within 20 seconds, so GDM's was shown in its place. Its log has why: open Logs on olo-greeter. Restarting the computer tries again.",
            None,
        ),
        finding(
            "power-modes:follower-old",
            Severity::Advice,
            "Olo's power mode follower is out of date",
            "The service that sets the processors and the disk as each power mode plans is an older version than this Olo's: Balanced doesn't hold the processors to 75% yet.",
            Some(("Update It", true)),
        ),
        finding(
            "video:intel",
            Severity::Advice,
            "Intel graphics can't decode H.264 and HEVC",
            "Fedora's own Intel video driver leaves out the patented formats, so videos in them are decoded by the processors: hotter, and slower to start. RPM Fusion's full driver decodes them on the graphics chip.",
            Some(("Install the Full Driver", true)),
        ),
        finding(
            "tool:just",
            Severity::Suggestion,
            "Install just",
            "just runs a project's commands from its justfile, as make does without its quirks. From Fedora's repositories.",
            Some(("Install just", true)),
        ),
        finding(
            "brush-shell",
            Severity::Suggestion,
            "Use brush as your shell",
            "brush (github.com/reubeno/brush) is a shell compatible with bash, written in Rust as Olo is. New shells, Terminal's too, use it; bash scripts run as before, and it's undone with a button.",
            Some(("Use brush", true)),
        ),
        finding(
            crate::checks::START_UP_SCREEN,
            Severity::Suggestion,
            "Show Olo's start-up screen as the computer starts",
            "From the moment the computer starts until you log in, it shows the system's own logos. Olo's start-up screen (its wordmark and the row of icons lit as it starts) can show instead, at your screen's scale (200%), and asks a disk's password in Olo's field. It's made into the start-up image, which takes a minute or two; Olo Doctor can go back to the one before.",
            Some(("Use Olo's Start-Up Screen", true)),
        ),
        finding(
            "start-up-chime",
            Severity::Suggestion,
            "Play Olo's chime as the computer starts",
            "Olo's chime plays as you log in. It can play as the computer starts instead, as a Mac's does: from its own speakers while the start-up screen shows, at the volume they were last set to (none while they're muted). It plays the chime chosen in Preferences ▸ Sound, and none if anyone on this computer turned the start-up sound off.",
            Some(("Play It as the Computer Starts", true)),
        ),
        finding(
            "gnome:services",
            Severity::Advice,
            "2 of GNOME's services run",
            "GNOME's online accounts and its iPhone monitor run in this session, though nothing of Olo's uses them.",
            Some(("Stop Them", false)),
        ),
    ];
    if fixed.contains(UNDONE_WORKTRUNK) {
        r.findings.push(finding(
            WORKTRUNK,
            Severity::Suggestion,
            "Make worktrunk's worktrees where Terminal does",
            "worktrunk (wt) makes a repository's worktrees beside it; Terminal makes them in ~/src/worktrees/<repository>/<branch>. One place for both.",
            Some(("Use Terminal's Place", false)),
        ));
    } else {
        let title = "worktrunk's worktrees are where Terminal makes them".to_owned();
        r.fine.push(Fine { title: title.clone(), detail: "In ~/src/worktrees/<repository>/<branch>, as Preferences ▸ Terminal has it.".into() });
        r.undo.push(Undo { id: "undo:worktrunk-place".into(), title, fix: Fix { label: "Put It Back".into(), root: false, steps: Vec::new() } });
    }
    r.findings.retain(|f| !fixed.contains(&f.id));
    for (title, detail) in [
        ("No input remapper grabs the keyboards and mice", ""),
        ("Olo's udev rules are installed and current", "Input devices, the presence sensor and security keys."),
        ("rtkit is running", "Olo's frames and voice processing can ask for realtime."),
        ("The Wi-Fi heals itself", "Olo's Wi-Fi healer is running."),
        ("Bluetooth's hardware answers", ""),
        ("The power modes are set as Olo plans them", "Balanced: the processors at most 75%; the disk's deepest sleep allowed."),
        ("Temporary files are on disk", "TMPDIR is /var/tmp/ben."),
        ("Token Pool's gateway is running", "On port 17891."),
        ("Backed up 40 minutes ago", "To “Archive”, encrypted, its key kept in Passwords."),
        ("just is installed", "just runs a project's commands. It's in /usr/bin."),
    ] {
        r.fine.push(Fine { title: title.into(), detail: detail.into() });
    }
    if !crate::ignored::path().exists() {
        let _ = crate::ignored::set("gnome:services", true);
    }
    r.set_aside(&crate::ignored::all());
    r.sort();
    r
}

/// What a fix does here: nothing, after a moment.
pub fn fix(id: &str) -> Vec<Result<(), String>> {
    std::thread::sleep(Duration::from_millis(900));
    if id == "video:intel" {
        return vec![Err("RPM Fusion's repository couldn't be reached: no network.".into())];
    }
    let mut fixed = FIXED.lock().unwrap_or_else(PoisonError::into_inner);
    match id {
        "undo:worktrunk-place" => _ = fixed.insert(UNDONE_WORKTRUNK.into()),
        WORKTRUNK => _ = fixed.remove(UNDONE_WORKTRUNK),
        _ => _ = fixed.insert(id.into()),
    }
    vec![Ok(())]
}
