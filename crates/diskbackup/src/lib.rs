//! DiskBackup's engine (Ben, 2026-11-07: "continuous, minimal impact …
//! full disk backups that are stored in the cloud … encrypted …
//! versioning in a way that minimizes disk usage so only changes and
//! deltas need to be kept and it should also compress everything … built
//! to work great even with slow and awkward storage backends like google
//! drive … files of any size").
//!
//! What's kept, in any [`Store`] (a folder, Google Drive, …):
//!
//! - **Chunks.** A file is cut where its content says (`chunker`: FastCDC's
//!   gear hash, about 1 MiB a chunk, 256 KiB to 4 MiB), so an edit changes
//!   only the chunks around it, and a chunk anywhere (another file, an older
//!   version) is kept once: its id is a keyed hash of what it holds
//!   (`crypto`: HMAC-SHA-256 under a key of the repository's, so ids say
//!   nothing about content). Each is compressed (zstd, unless that saves
//!   nothing) and sealed (XChaCha20-Poly1305).
//! - **Packs.** Chunks go up in packs of about 16 MiB (`data/…`): few large
//!   objects suit a slow store. A pack ends with its own sealed list of what
//!   it holds, so the index can always be made again from the packs.
//! - **The index** (`index/…`): where each chunk is, sealed, a file a flush.
//! - **Trees and snapshots.** A folder is a tree (its entries: name, kind,
//!   mode, times, size, and a file's chunks or a folder's tree), itself a
//!   blob, so an unchanged folder is the same blob again. A snapshot
//!   (`snapshots/…`) is a moment's tree of everything backed up: every
//!   version whole, sharing what didn't change. A file whose size, times
//!   and inode are as in the last snapshot isn't read again. A backup that
//!   doesn't finish keeps what it got through as an unfinished snapshot
//!   (`unfinished/…`), and the next goes on from it.
//! - **`config`**: the repository's version and chunking, sealed: opening it
//!   is how a key is known to be the right one.
//! - **Locks** (`locks/…`, `lock`): who's using it, so tidying (`prune`:
//!   what no snapshot kept, by `keep`, uses any more let go of) never pulls
//!   anything from under a backup or a restore, on any computer.
//!
//! The key (32 random bytes) is kept by Olo Passwords, not in the store.

pub mod backup;
pub mod chunker;
pub mod crypto;
pub mod drive;
pub mod exclude;
pub mod keep;
pub mod lock;
pub mod oauth;
pub mod prune;
pub mod repo;
pub mod restore;
pub mod store;
pub mod tree;
pub mod verify;

pub use backup::{Options, Progress, backup};
pub use crypto::{Id, Key};
pub use lock::Lock;
pub use prune::{Pruned, prune, repair, unused};
pub use repo::{Repo, Snapshot, Summary, TreeReader, Trees};
pub use restore::{Version, restore, versions};
pub use store::{Limit, LocalStore, Store, Throttled};
pub use tree::{Kind, Node, Tree};
pub use verify::{Checked, check};

/// What went wrong, said plainly.
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// The key doesn't open this repository.
    WrongKey,
    /// Something in the store isn't as it was written.
    Damaged(String),
    /// No repository there yet (or one there already, initialising).
    NoRepository,
    Exists,
    Cancelled,
    /// Someone else is using it in a way that can't be shared (who, and
    /// how): another computer tidying it, or backing up while this would.
    Busy(String),
    /// This one's lock went stale (the computer slept too long): what it
    /// was doing stops, and starts again.
    LockLost,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::WrongKey => write!(f, "this key doesn't open the backup"),
            Error::Damaged(what) => write!(f, "the backup is damaged: {what}"),
            Error::NoRepository => write!(f, "there's no backup there"),
            Error::Exists => write!(f, "there's a backup there already"),
            Error::Cancelled => write!(f, "stopped"),
            Error::Busy(why) => write!(f, "{why}: it tries again later"),
            Error::LockLost => write!(f, "paused too long (asleep?): it starts again"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
