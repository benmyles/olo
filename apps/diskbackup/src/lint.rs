//! The layout lint of DiskBackup's window (`cargo ui-lint`; HACKING, "The
//! layout lint"): drawn headlessly in every state it shows (never backed
//! up, backed up, failed, stopped, waiting, each stage of a backup, tidying,
//! deleting, erasing, validating, an unfinished backup left), each of its
//! tabs, light and dark at 1×, 1.5× and 2×, with the longest things it
//! says (a folder named by its path, a file deep in one, a long error), and
//! nothing may run past its room, sit over a control or be found by the
//! layout review. (Ben, 2026-11-09: "in DiskBackup when the backup is
//! starting the heading can get too long and overlaps controls not sure
//! why the linter didn't pick that up": it wasn't linted at all.)

use olo_kit::lint::{Found, Lint, assert_clean};
use olo_protocol::diskbackup::{Check, Doing, Last, Progress, STOPPED, Settings, Stage, State, Target, Unfinished};

use crate::window::MainWindow;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Every state, named, with the settings it's shown with.
fn states() -> Vec<(&'static str, Settings, State)> {
    let t = now();
    let home = Settings { target: Target::Folder { path: "/run/media/me/Backup Disk/Olo DiskBackup".into() }, ..Settings::default() };
    // (One folder that isn't home is named by its path: the longest title.)
    let folder = Settings { paths: vec!["~/Documents/Client Projects 2026/Deliverables and Drafts".into()], ..home.clone() };
    let drive = Settings { target: Target::GoogleDrive { account: "someone.with.a.long.name@example-company.com".into(), folder: "id".into() }, ..home.clone() };
    let base = State { pid: std::process::id(), snapshots: 412, size: Some(1_234_000_000_000), target: "a folder".into(), ..State::default() };
    let check = Check { time: t - 3500, by_checksum: 1204, sampled: 8, bytes_read: 33_000_000, seconds: 4.2, ..Check::default() };
    let last = Last {
        time: t - 3600,
        files: 312_004,
        bytes: 412_000_000_000,
        new_files: 1_651,
        changed_files: 23_456,
        new_bytes: 23_000_000_000,
        uploaded: 21_000_000_000,
        seconds: 7_380.0,
        unreadable: 12,
        check: Some(check),
        ..Last::default()
    };
    let backed_up = State { last: Some(last.clone()), last_ok: Some(t - 3600), next: Some(t + 3600), ..base.clone() };
    let progress = |stage| Progress {
        started: t - 600,
        files: 165_100,
        bytes: 230_000_000_000,
        new_bytes: 16_000_000_000,
        uploaded: 12_000_000_000,
        current: format!("{}/Music/A Very Long Album Name (Deluxe Edition, Remastered)/01 - A Very Long Track Name Indeed.flac", crate::target::home().display()),
        total_files: 312_004,
        total_bytes: 412_000_000_000,
        estimated: true,
        stage,
        sending: 134_217_728,
        sending_done: 67_000_000,
        upload_limit: 3_000_000,
        ..Progress::default()
    };
    let doing = |doing: Doing, p: Progress| State { doing, progress: Some(p), ..backed_up.clone() };
    let tidying = Progress { step: 3, steps: olo_diskbackup::prune::STEPS as u32, ..Progress::default() };
    vec![
        ("not running", home.clone(), State { pid: 0, ..State::default() }),
        ("never backed up", home.clone(), base.clone()),
        ("backed up", home.clone(), backed_up.clone()),
        ("backed up to Google Drive", drive.clone(), backed_up.clone()),
        (
            "failed",
            home.clone(),
            State {
                last: Some(Last { error: Some("the backup disk “Backup Disk” was unplugged while a pack was written to /run/media/me/Backup Disk/Olo DiskBackup/data/ab".into()), ..last.clone() }),
                last_ok: Some(t - 9 * 86_400),
                ..base.clone()
            },
        ),
        ("stopped", home.clone(), State { last: Some(Last { error: Some(STOPPED.into()), ..last.clone() }), last_ok: Some(t - 86_400), ..base.clone() }),
        ("waiting on battery", home.clone(), State { doing: Doing::Waiting { why: "on battery: it waits for power".into() }, ..backed_up.clone() }),
        ("waiting for the network", drive.clone(), State { doing: Doing::Waiting { why: "no network: it goes on once there’s one".into() }, ..backed_up.clone() }),
        ("counting", home.clone(), doing(Doing::BackingUp, progress(Stage::Counting))),
        ("counting a folder", folder.clone(), doing(Doing::BackingUp, progress(Stage::Counting))),
        ("backing up", home.clone(), doing(Doing::BackingUp, progress(Stage::Reading))),
        ("backing up a folder", folder.clone(), doing(Doing::BackingUp, progress(Stage::Reading))),
        ("backing up nothing yet", folder.clone(), doing(Doing::BackingUp, Progress { current: String::new(), ..progress(Stage::Reading) })),
        ("resuming", folder.clone(), doing(Doing::BackingUp, Progress { resuming: true, ..progress(Stage::Reading) })),
        ("saving", drive.clone(), doing(Doing::BackingUp, progress(Stage::Saving))),
        ("checking", drive.clone(), doing(Doing::BackingUp, progress(Stage::Checking))),
        ("stopping", drive.clone(), doing(Doing::BackingUp, progress(Stage::Stopping))),
        ("waiting for the network mid-backup", drive.clone(), doing(Doing::BackingUp, Progress { offline: true, ..progress(Stage::Reading) })),
        ("tidying", home.clone(), doing(Doing::Pruning, tidying.clone())),
        ("deleting the unfinished backup", drive.clone(), doing(Doing::Deleting { what: "the unfinished backup".into() }, tidying.clone())),
        ("deleting backups", drive.clone(), doing(Doing::Deleting { what: "12 backups".into() }, tidying)),
        ("erasing", drive.clone(), doing(Doing::Erasing, Progress::default())),
        ("validating", drive.clone(), doing(Doing::Validating, Progress { files: 120_000, total_files: 312_004, bytes: 1_000_000_000, total_bytes: 412_000_000_000, ..Progress::default() })),
        (
            "an unfinished backup left",
            drive,
            State { unfinished: Some(Unfinished { time: t - 7200, files: 165_100, bytes: 230_000_000_000, new_bytes: 16_000_000_000, sent: 12_000_000_000, seconds: 3_600.0 }), ..backed_up },
        ),
    ]
}

#[test]
fn ui_lint_main_window() {
    let mut lint = Lint::new();
    let mut found = Vec::new();
    for (name, settings, state) in states() {
        for f in lint.window(&crate::app::main_options(), move |cx| MainWindow::for_lint(settings.clone(), state.clone(), cx)) {
            found.push(Found { text: format!("{name}: {}", f.text), ..f });
        }
    }
    assert_clean("DiskBackup's window", &found);
}
