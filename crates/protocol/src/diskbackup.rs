//! DiskBackup: what the backup service (`olo-diskbackup --service`) backs
//! up and where (its settings), what it's doing now and has done (its
//! state), and what can be asked of it (its socket). The app, Deskbar,
//! Tracker, Doctor and agents read and drive it through these.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The runtime directory, or `OLO_DISKBACKUP_DIR` (tests: a service kept
/// apart from the person's).
fn runtime_dir() -> PathBuf {
    std::env::var_os("OLO_DISKBACKUP_DIR").or_else(|| std::env::var_os("XDG_RUNTIME_DIR")).map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// Where backups go.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Target {
    /// Not set up yet.
    #[default]
    None,
    /// A folder: a disk of its own, a mounted share.
    Folder { path: String },
    /// A folder of Google Drive's (by id), in `account`.
    GoogleDrive { account: String, folder: String },
}

impl Target {
    pub fn describe(&self) -> String {
        match self {
            Target::None => "nowhere yet".into(),
            Target::Folder { path } => path.clone(),
            Target::GoogleDrive { account, .. } => format!("Google Drive ({account})"),
        }
    }
}

/// How many snapshots are kept: the latest of each of the last so many
/// hours, days, weeks and months (the rest go, and what only they used).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keep {
    pub hourly: u32,
    pub daily: u32,
    pub weekly: u32,
    pub monthly: u32,
}

impl Default for Keep {
    fn default() -> Keep {
        Keep { hourly: 24, daily: 14, weekly: 8, monthly: 24 }
    }
}

/// What DiskBackup does (its app's settings; `olo-diskbackup --set`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Backing up at all (on once set up).
    pub enabled: bool,
    pub target: Target,
    /// The folders backed up (`~` the home folder).
    pub paths: Vec<String>,
    /// The groups of what can be made again left out
    /// (`olo_diskbackup::exclude::GROUPS`, by id); None: the defaults.
    pub exclude_groups: Option<Vec<String>>,
    /// The person's own patterns left out (`~/Videos/raw`, `**/*.iso`).
    pub excludes: Vec<String>,
    /// Back up on a schedule (every `every_minutes`); off, only when asked
    /// (Back Up Now). Ben, 2026-11-08: "i should also be able to choose if
    /// i even want auto backups or only when i click backup".
    pub automatic: bool,
    /// How often, minutes.
    pub every_minutes: u32,
    pub keep: Keep,
    /// Speed limits, kilobytes a second (0: none) (Ben, 2026-11-07: "make
    /// sure that i can also set a max upload/download speed").
    pub upload_kbs: u64,
    pub download_kbs: u64,
    /// Back up on battery too (else it waits for power).
    pub on_battery: bool,
    /// How much a backup gives way to the person's own work (the CPU's and
    /// the disks' priority).
    pub priority: Priority,
    /// Stay on each folder's own disk (not into what's mounted inside).
    pub one_file_system: bool,
    /// The key is kept in Passwords too (the app keeps it there: so the
    /// backups can be restored on another computer).
    pub key_in_passwords: bool,
    /// Its item in Deskbar's tray (once set up).
    pub in_deskbar: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            enabled: false,
            target: Target::None,
            paths: vec!["~".into()],
            exclude_groups: None,
            excludes: Vec::new(),
            automatic: true,
            every_minutes: 60,
            keep: Keep::default(),
            upload_kbs: 0,
            download_kbs: 0,
            on_battery: true,
            priority: Priority::Idle,
            one_file_system: true,
            key_in_passwords: false,
            in_deskbar: true,
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        match std::env::var_os("OLO_DISKBACKUP_DIR") {
            Some(d) => PathBuf::from(d).join("diskbackup-settings.json"),
            None => olo_xdg::config_dir().join("diskbackup.json"),
        }
    }

    pub fn load() -> Settings {
        std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.new");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, path)
    }
}

/// How much a backup gives way to whatever else the computer does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Priority {
    /// Only what nothing else wants (the CPU's and the disks' idle class):
    /// never in the way, slowest while you work.
    #[default]
    Idle,
    /// Below your work, but steadily (nice 19, the disks' lowest
    /// best-effort level).
    Low,
    /// As any program (a backup done soonest).
    Normal,
}

impl Priority {
    pub const ALL: [Priority; 3] = [Priority::Idle, Priority::Low, Priority::Normal];

    /// As the window says it.
    pub fn label(self) -> &'static str {
        match self {
            Priority::Idle => "Only when the computer is idle",
            Priority::Low => "Gently, below my own work",
            Priority::Normal => "At full speed",
        }
    }
}

/// What the service is doing.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "doing", rename_all = "kebab-case")]
pub enum Doing {
    /// Waiting for the next backup.
    #[default]
    Idle,
    /// Not backing up, and why (not set up, on battery, the key missing).
    Waiting {
        why: String,
    },
    BackingUp,
    Restoring,
    /// Letting go of snapshots past keeping, and what only they used.
    Pruning,
    /// Every backup there deleted (Erase All Backups), to start fresh.
    Erasing,
    /// Backups read back and checked (Validate).
    Validating,
    /// Backups deleted as asked (Delete…): `what` (“the unfinished
    /// backup”, “2 backups”), then the space only they took freed, said as
    /// it goes (a tidy's steps).
    Deleting {
        what: String,
    },
}

/// A backup or restore under way.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Progress {
    /// Seconds since 1970, when it started.
    pub started: i64,
    pub files: u64,
    pub bytes: u64,
    /// New content read so far, and what went up.
    pub new_bytes: u64,
    pub uploaded: u64,
    /// The file being read (or written).
    pub current: String,
    /// The whole of it: the files and bytes there are to go through (the
    /// last backup's, so `estimated`; or counted first, for the first).
    pub total_files: u64,
    pub total_bytes: u64,
    pub estimated: bool,
    pub stage: Stage,
    /// Going on from a backup that didn't finish (`State::unfinished`).
    pub resuming: bool,
    /// Tidying (or deleting): its step, of how many (0: not tidying).
    pub step: u32,
    pub steps: u32,
    /// The pack being sent (its size; 0: none), and how much of it has
    /// gone: what a stop waits for.
    pub sending: u64,
    pub sending_done: u64,
    /// Bytes a second it may send (0: no limit), to say how long that
    /// takes.
    pub upload_limit: u64,
}

/// Where a backup is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    /// Counting what there is (the first backup: nothing to go by).
    Counting,
    /// Going through the files, reading what's new.
    #[default]
    Reading,
    /// The folders' lists and the snapshot written.
    Saving,
    /// The backup just made checked: everything it needs in the store.
    Checking,
    /// Stopped: what's done so far being kept (what's waiting to go up
    /// sent first), for the next to go on from.
    Stopping,
}

/// The last backup that ended.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Last {
    /// Seconds since 1970, when it ended.
    pub time: i64,
    /// Why it didn't finish, if it didn't.
    pub error: Option<String>,
    pub files: u64,
    pub bytes: u64,
    pub new_files: u64,
    pub changed_files: u64,
    pub new_bytes: u64,
    pub uploaded: u64,
    pub seconds: f64,
    /// Files that couldn't be read.
    pub unreadable: u64,
    /// It checked after (None: not, stopped first).
    pub check: Option<Check>,
}

/// A backup checked (Ben, 2026-11-08: "backups also need to be
/// validatable and should be validated at the end of each backup but i
/// should be able to manually trigger a re-validate of any backup at any
/// time"; "add a fast validate option which compares checksums and just
/// spot-checks some samples … fast validate should be the default and the
/// one that is done automatically after a backup. full validate is only
/// when explicitly requested"). Fast: everything it needs in the store as
/// long as it was written, its lists read back whole, the store's
/// checksums compared with what was sent, a few packs read back as
/// samples. Full (`full`): every byte of it read back and checked, as a
/// restore would.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Check {
    /// When, seconds since 1970.
    pub time: i64,
    /// Every byte read back (Validate), not only its lists.
    pub full: bool,
    /// What's wrong, said plainly (none: it restores whole).
    pub problems: Vec<String>,
    /// Files it can't bring back as they were: the first few, and how many.
    pub files: Vec<String>,
    pub files_affected: u64,
    /// What was read back.
    pub bytes_read: u64,
    /// Packs the store's checksums showed as sent, and packs read back as
    /// samples (fast).
    pub by_checksum: u64,
    pub sampled: u64,
    pub seconds: f64,
    /// Why it couldn't be checked (no finding either way).
    pub error: Option<String>,
}

impl Check {
    /// Checked, and nothing wrong.
    pub fn whole(&self) -> bool {
        self.error.is_none() && self.problems.is_empty()
    }
}

/// What [`Last::error`] says when the person stopped it (Stop, `--stop`):
/// not a failure.
pub const STOPPED: &str = "stopped";

impl Last {
    /// Stopped by the person (not failed: said calmly, not as trouble).
    pub fn stopped(&self) -> bool {
        self.error.as_deref() == Some(STOPPED)
    }
}

/// A backup that didn't finish (stopped, failed, or the computer turned
/// off part way): what it got through is kept, and the next goes on from
/// there (Ben, 2026-11-08: "it needs to track in-progress / unfinished
/// backups too and they need to resume if i start again and i need the
/// ability to clean it up").
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Unfinished {
    /// When it got that far, seconds since 1970.
    pub time: i64,
    /// Files backed up so far, and their bytes (0: it stopped before
    /// keeping a list of them; what it sent is still there).
    pub files: u64,
    pub bytes: u64,
    /// New content read so far, before compression.
    pub new_bytes: u64,
    /// What it sent (compressed and sealed): about what deleting it frees.
    pub sent: u64,
    pub seconds: f64,
}

/// The last tidy: snapshots past keeping let go of, and the space only
/// they used freed (once a day, after a backup).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Tidied {
    /// Seconds since 1970, when it ended.
    pub time: i64,
    /// Why it didn't finish, if it didn't.
    pub error: Option<String>,
    /// Snapshots let go of since the tidy before.
    pub forgotten: u32,
    /// Bytes the store holds less.
    pub freed: u64,
}

/// What the service is doing and has done (its state file, followed by
/// inotify: the app, Deskbar, Tracker, Doctor).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// The service's process (a state file whose process is gone is old).
    pub pid: u32,
    #[serde(flatten)]
    pub doing: Doing,
    pub progress: Option<Progress>,
    pub last: Option<Last>,
    /// The last that finished whole, seconds since 1970.
    pub last_ok: Option<i64>,
    /// When the next is due, seconds since 1970.
    pub next: Option<i64>,
    pub snapshots: u32,
    /// Where it backs up to.
    pub target: String,
    /// What the backups take there, bytes (known since the last tidy, and
    /// grown by each backup since).
    pub size: Option<u64>,
    pub tidied: Option<Tidied>,
    /// This computer's backup that didn't finish, if one is kept.
    pub unfinished: Option<Unfinished>,
}

impl State {
    pub fn path() -> PathBuf {
        runtime_dir().join("olo-diskbackup.json")
    }

    pub fn load() -> State {
        let state: State = std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        if state.pid != 0 && !std::path::Path::new(&format!("/proc/{}", state.pid)).exists() {
            return State::default();
        }
        state
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        let tmp = path.with_extension("json.part");
        std::fs::write(&tmp, serde_json::to_vec(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, &path)
    }
}

/// The service's socket: a JSON request a line, a JSON reply.
pub fn socket() -> PathBuf {
    runtime_dir().join("olo-diskbackup.sock")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Request {
    /// Back up now (not waiting for the schedule).
    BackUpNow,
    /// Stop what's under way (what went up stays: the next one goes on).
    Stop,
    /// Tidy now (not waiting for the daily one): snapshots past keeping
    /// let go of, and the space only they took freed.
    Tidy,
    /// Which of folder `dir`'s files (absolute) are backed up as they are
    /// now, in this computer's newest snapshot (Tracker's ticks).
    Marks { dir: String },
    /// The versions of `path` (absolute) this computer's snapshots kept
    /// (Tracker's Older Versions…).
    Versions { path: String },
    /// This computer's backups (snapshots), newest first.
    Snapshots,
    /// Delete these backups (snapshot ids, hex), and the unfinished one
    /// (`State::unfinished`) if `unfinished`, and free the space only they
    /// took.
    Delete {
        snapshots: Vec<String>,
        #[serde(default)]
        unfinished: bool,
    },
    /// Delete every backup there (this computer's and any other's) and
    /// start again, empty, with the same key.
    EraseAll,
    /// Validate these backups (snapshot ids, hex): fast (checksums, and
    /// samples read back), or `full`, every byte read back; what's damaged
    /// is repaired as far as it can be (the next backup sends again what it
    /// still can).
    Validate {
        snapshots: Vec<String>,
        #[serde(default)]
        full: bool,
    },
}

/// One backup (`Reply::Snapshots`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SnapshotInfo {
    /// Its id, in hex.
    pub id: String,
    /// When it was taken, seconds since 1970.
    pub time: i64,
    pub files: u64,
    pub bytes: u64,
    pub new_files: u64,
    pub changed_files: u64,
    pub new_bytes: u64,
    pub stored_bytes: u64,
    pub seconds: f64,
    /// Files it couldn't read.
    pub unreadable: u64,
    /// Its last check (after it was made, or Validate's).
    pub check: Option<Check>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Reply {
    Ok,
    Error {
        message: String,
    },
    Marks {
        /// The folder is in what's backed up (and the newest snapshot has
        /// it).
        covered: bool,
        /// Its files whose current version that snapshot holds, by name.
        backed_up: Vec<String>,
        /// When that snapshot was taken, seconds since 1970.
        time: i64,
    },
    Versions {
        versions: Vec<Version>,
    },
    Snapshots {
        snapshots: Vec<SnapshotInfo>,
    },
}

/// One version of a file the backups kept (`Reply::Versions`), newest
/// first.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Version {
    /// The newest snapshot holding it (its id, in hex), and its time.
    pub snapshot: String,
    pub time: i64,
    /// The oldest snapshot's time holding it.
    pub first: i64,
    pub size: u64,
    /// Its modified time then, ns since 1970.
    pub mtime: i64,
    /// What's there now is this version.
    pub current: bool,
}

/// `request` to the service, its reply (the service not running: an
/// error saying so).
pub fn call(request: &Request) -> Result<Reply, String> {
    use std::io::{BufRead, Write};
    let mut s = std::os::unix::net::UnixStream::connect(socket()).map_err(|_| "the DiskBackup service isn't running".to_string())?;
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    s.write_all(format!("{}\n", serde_json::to_string(request).unwrap_or_default()).as_bytes()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    std::io::BufReader::new(&s).read_line(&mut line).map_err(|e| e.to_string())?;
    match serde_json::from_str::<Reply>(&line) {
        Ok(Reply::Error { message }) => Err(message),
        Ok(r) => Ok(r),
        Err(e) => Err(e.to_string()),
    }
}

/// How DiskBackup says what it's doing, the same in its window and in
/// Deskbar (Ben, 2026-11-08: "it's also not entirely clear to me what it
/// means and what it's doing like what is read vs new and i'm backing up
/// my whole home dir which is hundreds of gigs so why doesn't it say
/// anything about that"). A backup goes *through* everything it backs up
/// (a file unchanged since the last backup is taken as it was, not read
/// again); what the backup didn't have yet is *new to the backup*; that
/// is *sent*, compressed and encrypted.
pub mod words {
    use super::{Progress, Settings, Stage};

    /// "820 KB", "12.4 MB", "2.31 GB", "1.20 TB".
    pub fn size(b: u64) -> String {
        match b {
            0..1_000_000 => format!("{} KB", b.div_ceil(1000)),
            1_000_000..1_000_000_000 => format!("{:.1} MB", b as f64 / 1e6),
            1_000_000_000..1_000_000_000_000 => format!("{:.2} GB", b as f64 / 1e9),
            _ => format!("{:.2} TB", b as f64 / 1e12),
        }
    }

    /// "312,004".
    pub fn count(n: u64) -> String {
        let s = n.to_string();
        let mut out = String::new();
        for (i, ch) in s.chars().enumerate() {
            if i > 0 && (s.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(ch);
        }
        out
    }

    /// "1 file", "12 files".
    pub fn plural(n: u64, what: &str) -> String {
        if n == 1 { format!("1 {what}") } else { format!("{} {what}s", count(n)) }
    }

    /// What's backed up: "your home folder", "~/Documents", "3 folders".
    pub fn what(s: &Settings) -> String {
        match s.paths.as_slice() {
            [p] if p == "~" => "your home folder".into(),
            [p] => p.clone(),
            ps => format!("{} folders", ps.len()),
        }
    }

    /// "about 6 minutes left", "about 2 hours left", "less than a minute
    /// left".
    pub fn left(secs: i64) -> String {
        let minutes = |n: i64| if n == 1 { "a minute".to_owned() } else { format!("{n} minutes") };
        match secs {
            ..60 => "less than a minute left".into(),
            60..5400 => format!("about {} left", minutes((secs + 30) / 60)),
            _ => format!("about {} hours left", (secs + 1800) / 3600),
        }
    }

    impl Progress {
        /// How much is gone through, of how much: "2.30 GB of 412.00 GB
        /// gone through", "of about" by the last backup's, and past that,
        /// "660.6 MB gone through, more than the last backup's 524.3 MB".
        pub fn gone_through(&self) -> String {
            match (self.estimated, self.bytes > self.total_bytes) {
                (true, true) => format!("{} gone through, more than the last backup’s {}", size(self.bytes), size(self.total_bytes)),
                (true, false) => format!("{} of about {} gone through", size(self.bytes), size(self.total_bytes)),
                _ => format!("{} of {} gone through", size(self.bytes), size(self.total_bytes)),
            }
        }

        /// Files, the same way: "1,651 of 312,004 files", "126 files, more
        /// than the last backup’s 100".
        pub fn of_files(&self) -> String {
            match (self.estimated, self.files > self.total_files) {
                (true, true) => format!("{}, more than the last backup’s {}", plural(self.files, "file"), count(self.total_files)),
                (true, false) => format!("{} of about {}", count(self.files), plural(self.total_files, "file")),
                _ => format!("{} of {}", count(self.files), plural(self.total_files, "file")),
            }
        }

        /// A pack being sent: how much of it has gone, and (with an upload
        /// limit) about how long the rest takes, seconds.
        pub fn sending_left(&self) -> Option<(u64, Option<i64>)> {
            if self.sending == 0 {
                return None;
            }
            let done = self.sending_done.min(self.sending);
            let left = (self.upload_limit > 0).then(|| ((self.sending - done) / self.upload_limit) as i64);
            Some((done, left))
        }

        /// How a backup under way stands at `at` (seconds since 1970): how
        /// much of it is gone through (0 to 1), and, once there's enough
        /// to go by, how long is left (seconds).
        pub fn how_far(&self, at: i64) -> (f32, Option<i64>) {
            if self.total_bytes == 0 {
                return (0.0, None);
            }
            // (The plan was the last backup's: never more than all of it,
            // while it's still going.)
            let done = (self.bytes as f64 / self.total_bytes as f64).min(0.99);
            let spent = (at - self.started).max(0) as f64;
            let left = (done > 0.02 && spent >= 10.0).then(|| (spent * (1.0 - done) / done).round() as i64);
            (done as f32, left)
        }

        /// The backup under way in lines, plainly (`s`: what it backs up),
        /// at `at`: what it's doing; how far through how much, and how long
        /// is left; files of files; what's new to the backup, and sent.
        pub fn lines(&self, s: &Settings, at: i64) -> Vec<String> {
            match self.stage {
                Stage::Counting => {
                    vec![format!("Getting ready to back up {}: counting what’s there", what(s)), format!("{} and {} found so far", plural(self.total_files, "file"), size(self.total_bytes))]
                }
                Stage::Saving => vec!["Finishing the backup: writing its lists".into()],
                Stage::Checking => vec!["Validating the backup just made".into()],
                Stage::Stopping => match self.sending_left() {
                    Some((done, left)) => vec![
                        format!("Stopping: finishing sending a pack, {} of {}{}", size(done), size(self.sending), left.map(|l| format!(" ({})", self::left(l))).unwrap_or_default()),
                        "Then what’s done is kept, so the next backup goes on from there".into(),
                    ],
                    None => vec!["Stopping: keeping what’s done, so the next goes on from there".into()],
                },
                Stage::Reading => {
                    let (done, left) = self.how_far(at);
                    let mut through = format!("{} ({:.0}%)", self.gone_through(), done * 100.0);
                    if let Some(l) = left {
                        through.push_str(&format!(" · {}", self::left(l)));
                    }
                    vec![
                        format!("{} {}", if self.resuming { "Resuming the backup of" } else { "Backing up" }, what(s)),
                        through,
                        self.of_files(),
                        format!("{} new to the backup · {} sent", size(self.new_bytes), size(self.uploaded)),
                    ]
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Plain words: what, how far through how much, how long, files,
        /// what's new and sent.
        #[test]
        fn a_backup_said_plainly() {
            let s = Settings::default();
            let p = Progress {
                started: 1000,
                files: 1651,
                bytes: 23_000_000_000,
                new_bytes: 1_600_000_000,
                uploaded: 1_200_000_000,
                total_files: 312_004,
                total_bytes: 412_000_000_000,
                stage: Stage::Reading,
                ..Default::default()
            };
            assert_eq!(
                p.lines(&s, 1600),
                ["Backing up your home folder", "23.00 GB of 412.00 GB gone through (6%) · about 3 hours left", "1,651 of 312,004 files", "1.60 GB new to the backup · 1.20 GB sent",]
            );
            let more = Progress { estimated: true, files: 126, total_files: 100, bytes: 660_600_000, total_bytes: 524_300_000, ..p.clone() };
            assert_eq!(more.gone_through(), "660.6 MB gone through, more than the last backup’s 524.3 MB");
            assert_eq!(more.of_files(), "126 files, more than the last backup’s 100");
            let counting = Progress { stage: Stage::Counting, total_files: 120_345, total_bytes: 210_400_000_000, ..Default::default() };
            assert_eq!(counting.lines(&s, 0)[1], "120,345 files and 210.40 GB found so far");
            assert_eq!(left(50), "less than a minute left");
            assert_eq!(left(6 * 60), "about 6 minutes left");
        }
    }
}
