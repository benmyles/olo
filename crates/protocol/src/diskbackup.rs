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
    /// How often, minutes.
    pub every_minutes: u32,
    pub keep: Keep,
    /// Speed limits, kilobytes a second (0: none) (Ben, 2026-11-07: "make
    /// sure that i can also set a max upload/download speed").
    pub upload_kbs: u64,
    pub download_kbs: u64,
    /// Back up on battery too (else it waits for power).
    pub on_battery: bool,
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
            every_minutes: 60,
            keep: Keep::default(),
            upload_kbs: 0,
            download_kbs: 0,
            on_battery: true,
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Reply {
    Ok,
    Error { message: String },
}
