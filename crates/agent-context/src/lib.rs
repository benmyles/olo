//! What every coding agent on a Rex host is told before it begins:
//! where it is running and what Olo gives it there, as a section Olo
//! keeps in each harness's own standing instructions (Claude Code,
//! Codex, Antigravity). (Ben, 2026-11-27, of an agent that spent a
//! long while finding out where it ran and how to open a link in his
//! browser: "can we have it so that all agents on a rex host get a
//! section added to their system instructions or the closest to that
//! each harness allows that explains where they're running and an
//! overview of the relevant skills and capabilities of each".)
//!
//! # Where each harness reads user-level standing instructions
//!
//! Verified live against the CLIs installed on a Rex host:
//!
//! - **Claude Code** ([`Harness::Claude`]): `$CLAUDE_CONFIG_DIR/CLAUDE.md`, or
//!   `~/.claude/CLAUDE.md` when `CLAUDE_CONFIG_DIR` is unset or empty. Managed
//!   as a versioned block inside that file.
//! - **Codex** ([`Harness::Codex`]): `$CODEX_HOME/AGENTS.md`, or
//!   `~/.codex/AGENTS.md` when `CODEX_HOME` is unset or empty. Managed as a
//!   versioned block inside that file. (Note: if a user separately creates
//!   `$CODEX_HOME/AGENTS.override.md`, Codex reads that override file instead
//!   of `AGENTS.md`.)
//! - **Antigravity (`agy`)** ([`Harness::Antigravity`]):
//!   `~/.gemini/config/rules/olo-rex.md` with `trigger: always_on` YAML
//!   frontmatter. Antigravity loads `always_on` rule files from its global
//!   configuration folder `~/.gemini/config/rules/` in every workspace (git or
//!   non-git), with no config-directory environment override; Olo owns this
//!   rule file whole (`install` writes it, `remove` deletes it) without
//!   touching any `GEMINI.md` or `AGENTS.md` of the person's.

mod block;
mod text;

use std::{
    ffi::OsString,
    fmt,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub use text::{AGY_FRONTMATTER, BEGIN_MARKER, END_MARKER, MAX_BYTES, MAX_LINES, VERSION};

/// The coding agent harnesses Olo manages standing instructions for on a Rex host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Harness {
    /// Claude Code (`~/.claude/CLAUDE.md`, overridden by `CLAUDE_CONFIG_DIR`).
    Claude,
    /// OpenAI Codex CLI (`~/.codex/AGENTS.md`, overridden by `CODEX_HOME`).
    Codex,
    /// Antigravity CLI (`~/.gemini/config/rules/olo-rex.md`, owned whole by Olo).
    Antigravity,
}

/// How Olo stores its section for a given [`Harness`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Storage {
    /// A versioned begin/end block inside a user-editable Markdown file.
    ManagedBlock,
    /// A dedicated rule file owned whole by Olo.
    OwnedRuleFile,
}

/// Environment overrides that redirect harness configuration folders.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env {
    /// `CLAUDE_CONFIG_DIR`, if set and non-empty.
    pub claude_config_dir: Option<PathBuf>,
    /// `CODEX_HOME`, if set and non-empty.
    pub codex_home: Option<PathBuf>,
}

impl Env {
    /// Read harness overrides from the current process environment.
    pub fn current() -> Self {
        Self::from_lookup(|name| std::env::var_os(name))
    }

    /// Build from an arbitrary environment lookup function (empty values are
    /// treated as unset).
    pub fn from_lookup<F>(mut get: F) -> Self
    where
        F: FnMut(&str) -> Option<OsString>,
    {
        let non_empty = |v: Option<OsString>| v.filter(|s| !s.is_empty()).map(PathBuf::from);
        Self { claude_config_dir: non_empty(get("CLAUDE_CONFIG_DIR")), codex_home: non_empty(get("CODEX_HOME")) }
    }
}

/// State of Olo's standing instructions for one harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The harness's configuration directory does not exist on this machine.
    NotInstalled,
    /// The harness is installed, and Olo's section is absent.
    Absent,
    /// Olo's current section is installed cleanly.
    Current,
    /// An older version, modified copy, or damaged block/markers are present.
    Stale,
}

/// Result of [`install`] or [`remove`] for one harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The harness's configuration directory does not exist; nothing was touched.
    NotInstalled,
    /// Already in the requested state; nothing was written (mtime untouched).
    Unchanged,
    /// Created, updated, repaired, or removed.
    Changed,
}

/// Per-harness outcome for batch calls ([`status_all`], [`install_all`], [`remove_all`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessReport<T> {
    pub harness: Harness,
    pub path: PathBuf,
    pub result: T,
}

impl Harness {
    /// Every supported harness, in canonical order.
    pub const ALL: [Harness; 3] = [Harness::Claude, Harness::Codex, Harness::Antigravity];

    /// Short CLI name (`claude`, `codex`, `agy`).
    pub fn name(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Antigravity => "agy",
        }
    }

    /// Whether Olo manages a block inside a shared file or owns the whole file.
    pub fn storage(self) -> Storage {
        match self {
            Harness::Claude | Harness::Codex => Storage::ManagedBlock,
            Harness::Antigravity => Storage::OwnedRuleFile,
        }
    }

    /// Configuration directory for this harness, honouring its environment
    /// override when set and non-empty.
    pub fn config_dir(self, home: &Path, env: &Env) -> PathBuf {
        match self {
            Harness::Claude => env.claude_config_dir.clone().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| home.join(".claude")),
            Harness::Codex => env.codex_home.clone().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| home.join(".codex")),
            Harness::Antigravity => home.join(".gemini"),
        }
    }

    /// Path where this harness reads its user-level standing instructions.
    pub fn instructions_path(self, home: &Path, env: &Env) -> PathBuf {
        let dir = self.config_dir(home, env);
        match self {
            Harness::Claude => dir.join("CLAUDE.md"),
            Harness::Codex => dir.join("AGENTS.md"),
            Harness::Antigravity => dir.join("config").join("rules").join("olo-rex.md"),
        }
    }

    /// Whether this harness's own configuration folder is present on the machine.
    pub fn is_present(self, home: &Path, env: &Env) -> bool {
        let overridden = match self {
            Harness::Claude => env.claude_config_dir.as_ref().is_some_and(|p| !p.as_os_str().is_empty()),
            Harness::Codex => env.codex_home.as_ref().is_some_and(|p| !p.as_os_str().is_empty()),
            Harness::Antigravity => false,
        };
        if !overridden && home.as_os_str().is_empty() {
            return false;
        }
        self.config_dir(home, env).is_dir()
    }
}

impl fmt::Display for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The standing-instructions text installed for every harness (LF line endings,
/// ending with a single newline).
pub fn section() -> &'static str {
    text::SECTION
}

/// Inspect Olo's standing instructions for `harness`.
pub fn status(harness: Harness, home: &Path, env: &Env) -> io::Result<Status> {
    if !harness.is_present(home, env) {
        return Ok(Status::NotInstalled);
    }
    let path = harness.instructions_path(home, env);
    let existing = read_optional(&path)?;
    match harness.storage() {
        Storage::ManagedBlock => Ok(block::inspect(existing.as_deref())),
        Storage::OwnedRuleFile => match existing {
            None => Ok(Status::Absent),
            Some(bytes) if bytes == text::render_rule_file().as_bytes() => Ok(Status::Current),
            Some(_) => Ok(Status::Stale),
        },
    }
}

/// Install or update Olo's standing instructions for `harness`.
///
/// Does nothing when the harness's configuration folder is absent
/// ([`Outcome::NotInstalled`]), and leaves the file untouched (mtime preserved)
/// when already current ([`Outcome::Unchanged`]).
pub fn install(harness: Harness, home: &Path, env: &Env) -> io::Result<Outcome> {
    if !harness.is_present(home, env) {
        return Ok(Outcome::NotInstalled);
    }
    let path = harness.instructions_path(home, env);
    let existing = read_optional(&path)?;
    match harness.storage() {
        Storage::ManagedBlock => {
            let Some(updated) = block::apply_install(existing.as_deref()) else {
                return Ok(Outcome::Unchanged);
            };
            write_preserving(&path, &updated)?;
            Ok(Outcome::Changed)
        }
        Storage::OwnedRuleFile => {
            let wanted = text::render_rule_file();
            if existing.as_deref() == Some(wanted.as_bytes()) {
                return Ok(Outcome::Unchanged);
            }
            write_preserving(&path, wanted.as_bytes())?;
            Ok(Outcome::Changed)
        }
    }
}

/// Remove Olo's standing instructions for `harness`, keeping every byte of the
/// person's own content intact.
pub fn remove(harness: Harness, home: &Path, env: &Env) -> io::Result<Outcome> {
    if !harness.is_present(home, env) {
        return Ok(Outcome::NotInstalled);
    }
    let path = harness.instructions_path(home, env);
    match harness.storage() {
        Storage::ManagedBlock => {
            let Some(existing) = read_optional(&path)? else {
                return Ok(Outcome::Unchanged);
            };
            let Some(cleaned) = block::apply_remove(&existing) else {
                return Ok(Outcome::Unchanged);
            };
            let is_symlink = is_symlink(&path)?;
            if cleaned.is_empty() && !is_symlink {
                fs::remove_file(&path)?;
            } else {
                write_preserving(&path, &cleaned)?;
            }
            Ok(Outcome::Changed)
        }
        Storage::OwnedRuleFile => match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Outcome::Unchanged),
            Err(e) => Err(e),
            Ok(_) => {
                fs::remove_file(&path)?;
                Ok(Outcome::Changed)
            }
        },
    }
}

/// Inspect every harness in [`Harness::ALL`].
pub fn status_all(home: &Path, env: &Env) -> Vec<HarnessReport<io::Result<Status>>> {
    Harness::ALL.into_iter().map(|harness| HarnessReport { harness, path: harness.instructions_path(home, env), result: status(harness, home, env) }).collect()
}

/// Install or update every present harness in [`Harness::ALL`].
pub fn install_all(home: &Path, env: &Env) -> Vec<HarnessReport<io::Result<Outcome>>> {
    Harness::ALL.into_iter().map(|harness| HarnessReport { harness, path: harness.instructions_path(home, env), result: install(harness, home, env) }).collect()
}

/// Remove Olo's section from every present harness in [`Harness::ALL`].
pub fn remove_all(home: &Path, env: &Env) -> Vec<HarnessReport<io::Result<Outcome>>> {
    Harness::ALL.into_iter().map(|harness| HarnessReport { harness, path: harness.instructions_path(home, env), result: remove(harness, home, env) }).collect()
}

fn read_optional(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn is_symlink(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(meta.file_type().is_symlink()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Write `bytes` to `path`:
/// - If `path` is a symbolic link, write through the link so the symlink and
///   target's permissions/inode are preserved.
/// - Otherwise write atomically via a temporary file beside `path` and rename,
///   preserving the existing file's permissions if one existed.
fn write_preserving(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if is_symlink(path)? {
        let mut f = OpenOptions::new().write(true).create(true).truncate(true).open(path)?;
        f.write_all(bytes)?;
        f.flush()?;
        return Ok(());
    }

    let parent = path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "instructions path has no parent directory"))?;
    fs::create_dir_all(parent)?;

    let existing_perms = match fs::metadata(path) {
        Ok(meta) => Some(meta.permissions()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };

    static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);
    let leaf = path.file_name().and_then(|s| s.to_str()).unwrap_or("instructions");
    let pid = std::process::id();

    let (tmp_path, mut tmp_file) = loop {
        let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{leaf}.olo-tmp.{pid}.{n}"));
        match OpenOptions::new().write(true).create_new(true).open(&candidate) {
            Ok(f) => break (candidate, f),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };

    let write_res = (|| -> io::Result<()> {
        tmp_file.write_all(bytes)?;
        tmp_file.flush()?;
        drop(tmp_file);
        if let Some(perms) = existing_perms {
            fs::set_permissions(&tmp_path, perms)?;
        }
        fs::rename(&tmp_path, path)?;
        Ok(())
    })();

    if write_res.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    write_res
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::{MetadataExt, PermissionsExt, symlink},
        time::{Duration, UNIX_EPOCH},
    };

    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = PathBuf::from(format!("/var/tmp/olo-agent-context-ut-{tag}-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }

        fn path(&self) -> &Path {
            &self.dir
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn section_size_facts_and_privacy() {
        let s = section();
        assert!(s.ends_with('\n'), "section must end with a single newline");
        assert!(!s.ends_with("\n\n"), "section must not end with trailing blank line");
        let lines = s.lines().count();
        assert!(lines <= MAX_LINES, "section too long: {lines} lines > {MAX_LINES}");
        assert!(s.len() <= MAX_BYTES, "section too large: {} bytes > {MAX_BYTES}", s.len());
        assert!(text::render_block("\n").len() <= MAX_BYTES, "rendered block too large: {} bytes > {MAX_BYTES}", text::render_block("\n").len());
        assert!(text::render_rule_file().len() <= MAX_BYTES, "rendered agy rule file too large: {} bytes > {MAX_BYTES}", text::render_rule_file().len());

        // Essential facts present.
        for needle in ["REX_BLOCK", "olo open", "olo screen", "olo op read", "olo agent", "allowed this host", "olo-rex-bridge", "`rex`"] {
            assert!(s.contains(needle), "section missing required fact: {needle}");
        }

        // Never names this host, current user, or real network addresses.
        let lower = s.to_lowercase();
        for forbidden in ["@", "127.0.0.1", "192.168.", "10.0.", "/home/"] {
            assert!(!lower.contains(forbidden), "section must not contain {forbidden:?}");
        }
        if let Ok(hostname) = fs::read_to_string("/etc/hostname") {
            let h = hostname.trim().to_lowercase();
            if h.len() >= 3 {
                assert!(!lower.contains(&h), "section must not contain machine hostname {h:?}");
            }
        }
        if let Ok(user) = std::env::var("USER") {
            let u = user.trim().to_lowercase();
            if u.len() >= 3 {
                // Ensure the user's account name does not appear as a standalone word.
                let words: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).collect();
                assert!(!words.contains(&u.as_str()), "section must not name account {u:?}");
            }
        }
    }

    #[test]
    fn paths_with_and_without_env_overrides() {
        let home = Path::new("/var/tmp/fake-home");
        let default_env = Env::default();
        assert_eq!(Harness::Claude.config_dir(home, &default_env), PathBuf::from("/var/tmp/fake-home/.claude"));
        assert_eq!(Harness::Claude.instructions_path(home, &default_env), PathBuf::from("/var/tmp/fake-home/.claude/CLAUDE.md"));
        assert_eq!(Harness::Codex.config_dir(home, &default_env), PathBuf::from("/var/tmp/fake-home/.codex"));
        assert_eq!(Harness::Codex.instructions_path(home, &default_env), PathBuf::from("/var/tmp/fake-home/.codex/AGENTS.md"));
        assert_eq!(Harness::Antigravity.config_dir(home, &default_env), PathBuf::from("/var/tmp/fake-home/.gemini"));
        assert_eq!(Harness::Antigravity.instructions_path(home, &default_env), PathBuf::from("/var/tmp/fake-home/.gemini/config/rules/olo-rex.md"));

        let custom_env = Env::from_lookup(|k| match k {
            "CLAUDE_CONFIG_DIR" => Some(OsString::from("/var/tmp/alt-claude")),
            "CODEX_HOME" => Some(OsString::from("/var/tmp/alt-codex")),
            _ => None,
        });
        assert_eq!(Harness::Claude.config_dir(home, &custom_env), PathBuf::from("/var/tmp/alt-claude"));
        assert_eq!(Harness::Claude.instructions_path(home, &custom_env), PathBuf::from("/var/tmp/alt-claude/CLAUDE.md"));
        assert_eq!(Harness::Codex.config_dir(home, &custom_env), PathBuf::from("/var/tmp/alt-codex"));
        assert_eq!(Harness::Codex.instructions_path(home, &custom_env), PathBuf::from("/var/tmp/alt-codex/AGENTS.md"));
        assert_eq!(Harness::Antigravity.instructions_path(home, &custom_env), PathBuf::from("/var/tmp/fake-home/.gemini/config/rules/olo-rex.md"));

        // Empty env overrides fall back to default home locations.
        let empty_env = Env::from_lookup(|k| match k {
            "CLAUDE_CONFIG_DIR" | "CODEX_HOME" => Some(OsString::from("")),
            _ => None,
        });
        assert_eq!(Harness::Claude.config_dir(home, &empty_env), PathBuf::from("/var/tmp/fake-home/.claude"));
        assert_eq!(Harness::Codex.config_dir(home, &empty_env), PathBuf::from("/var/tmp/fake-home/.codex"));

        // Empty home without override never probes relative `.claude`/`.codex`/`.gemini`.
        for h in Harness::ALL {
            assert!(!h.is_present(Path::new(""), &default_env));
        }
    }

    #[test]
    fn absent_harness_creates_nothing() {
        let sb = Scratch::new("absent");
        let home = sb.path();
        let env = Env::default();

        for h in Harness::ALL {
            assert_eq!(status(h, home, &env).unwrap(), Status::NotInstalled);
            assert_eq!(install(h, home, &env).unwrap(), Outcome::NotInstalled);
            assert_eq!(remove(h, home, &env).unwrap(), Outcome::NotInstalled);
            assert!(!h.config_dir(home, &env).exists());
            assert!(!h.instructions_path(home, &env).exists());
        }

        // Only creating ~/.claude makes Claude present while others stay untouched.
        fs::create_dir_all(home.join(".claude")).unwrap();
        let installed = install_all(home, &env);
        assert_eq!(installed[0].result.as_ref().copied().unwrap(), Outcome::Changed);
        assert_eq!(installed[1].result.as_ref().copied().unwrap(), Outcome::NotInstalled);
        assert_eq!(installed[2].result.as_ref().copied().unwrap(), Outcome::NotInstalled);
        assert!(!home.join(".codex").exists());
        assert!(!home.join(".gemini").exists());
    }

    #[test]
    fn fresh_install_idempotent_mtime_and_clean_remove_for_all_harnesses() {
        let sb = Scratch::new("fresh");
        let home = sb.path();
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::create_dir_all(home.join(".codex")).unwrap();
        fs::create_dir_all(home.join(".gemini")).unwrap();
        let env = Env::default();

        let old_time = UNIX_EPOCH + Duration::from_secs(1_700_000_000);

        for h in Harness::ALL {
            let p = h.instructions_path(home, &env);
            assert_eq!(status(h, home, &env).unwrap(), Status::Absent);
            assert_eq!(remove(h, home, &env).unwrap(), Outcome::Unchanged);
            assert_eq!(install(h, home, &env).unwrap(), Outcome::Changed);
            assert_eq!(status(h, home, &env).unwrap(), Status::Current);

            // Set an old mtime and verify a second install does not touch the file at all.
            let f = OpenOptions::new().write(true).open(&p).unwrap();
            f.set_modified(old_time).unwrap();
            let meta_before = fs::metadata(&p).unwrap();
            assert_eq!(meta_before.modified().unwrap(), old_time);

            assert_eq!(install(h, home, &env).unwrap(), Outcome::Unchanged);
            let meta_after = fs::metadata(&p).unwrap();
            assert_eq!(meta_after.modified().unwrap(), old_time, "{h}: second install must not modify mtime");
            assert_eq!(meta_after.ino(), meta_before.ino(), "{h}: second install must not replace inode");

            // Removing takes Olo's block/file away completely.
            assert_eq!(remove(h, home, &env).unwrap(), Outcome::Changed);
            assert!(!p.exists(), "{h}: file created solely for Olo should be deleted on remove");
            assert_eq!(status(h, home, &env).unwrap(), Status::Absent);
            assert_eq!(remove(h, home, &env).unwrap(), Outcome::Unchanged);
        }
    }

    #[test]
    fn preserves_user_text_before_and_after_no_trailing_nl_and_crlf() {
        let sb = Scratch::new("preserve");
        let home = sb.path();
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::create_dir_all(home.join(".codex")).unwrap();
        let env = Env::default();

        // 1. Standard LF file with trailing newline.
        let cases: &[(&str, &[u8])] = &[
            ("trailing_lf", b"# My Rules\nKeep it simple.\n"),
            ("no_trailing_nl", b"# My Rules\nNo final newline"),
            ("two_trailing_lf", b"# My Rules\n\n"),
            ("crlf_trailing", b"# My Rules\r\nWindows endings.\r\n"),
            ("crlf_no_trailing", b"# My Rules\r\nNo final crlf"),
            ("non_utf8_user_bytes", b"# Notes \xff\xfe\nKeep raw bytes.\n"),
        ];

        for (label, original) in cases {
            let p = Harness::Claude.instructions_path(home, &env);
            fs::write(&p, original).unwrap();
            assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Absent, "{label}");

            assert_eq!(install(Harness::Claude, home, &env).unwrap(), Outcome::Changed, "{label}");
            assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Current, "{label}");
            assert_eq!(install(Harness::Claude, home, &env).unwrap(), Outcome::Unchanged, "{label}");

            let installed = fs::read(&p).unwrap();
            assert!(installed.starts_with(original), "{label}: user prefix must be byte-for-byte intact");
            if label.starts_with("crlf") {
                assert!(installed.windows(2).any(|w| w == b"\r\n"), "{label}: CRLF file must keep CRLF");
                // Every '\n' in a CRLF file must be preceded by '\r'.
                for (idx, &b) in installed.iter().enumerate() {
                    if b == b'\n' {
                        assert!(idx > 0 && installed[idx - 1] == b'\r', "{label}: bare LF at byte {idx}");
                    }
                }
            }

            assert_eq!(remove(Harness::Claude, home, &env).unwrap(), Outcome::Changed, "{label}");
            assert_eq!(fs::read(&p).unwrap(), *original, "{label}: round-trip remove must restore exact bytes");
        }

        // 2. Block in the middle of a file: user text before AND after (including no trailing newline).
        let codex_path = Harness::Codex.instructions_path(home, &env);
        let middle = format!("# Before\n\n{BEGIN_MARKER}\nold body\n{END_MARKER}\n\n# After without newline");
        fs::write(&codex_path, &middle).unwrap();
        assert_eq!(status(Harness::Codex, home, &env).unwrap(), Status::Stale);
        assert_eq!(install(Harness::Codex, home, &env).unwrap(), Outcome::Changed);
        let updated = fs::read_to_string(&codex_path).unwrap();
        assert!(updated.starts_with("# Before\n\n<!-- olo:rex-host:begin v1 -->\n"));
        assert!(updated.ends_with(&format!("{END_MARKER}\n\n# After without newline")));
        assert_eq!(status(Harness::Codex, home, &env).unwrap(), Status::Current);

        assert_eq!(remove(Harness::Codex, home, &env).unwrap(), Outcome::Changed);
        assert_eq!(fs::read_to_string(&codex_path).unwrap(), "# Before\n\n# After without newline");
    }

    #[test]
    fn symlink_and_permissions_preserved() {
        let sb = Scratch::new("symlink-perms");
        let home = sb.path();
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::create_dir_all(home.join(".codex")).unwrap();
        let env = Env::default();

        // 1. Regular file permissions (e.g. 0o600) survive atomic rename.
        let codex_path = Harness::Codex.instructions_path(home, &env);
        fs::write(&codex_path, "# Private\n").unwrap();
        fs::set_permissions(&codex_path, fs::Permissions::from_mode(0o600)).unwrap();

        let ino_before = fs::metadata(&codex_path).unwrap().ino();
        assert_eq!(install(Harness::Codex, home, &env).unwrap(), Outcome::Changed);
        let meta_after = fs::metadata(&codex_path).unwrap();
        assert_eq!(meta_after.permissions().mode() & 0o777, 0o600, "permissions must survive atomic rename");
        assert_ne!(meta_after.ino(), ino_before, "regular file write must be atomic via temp+rename");
        // No temp files left behind.
        let extras: Vec<_> = fs::read_dir(home.join(".codex")).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().contains(".olo-tmp.")).collect();
        assert!(extras.is_empty(), "temp files left behind: {extras:?}");

        // 2. Symlink: written through the link, symlink kept, target permissions kept.
        let dotfiles = sb.path().join("dotfiles");
        fs::create_dir_all(&dotfiles).unwrap();
        let target = dotfiles.join("CLAUDE.md");
        fs::write(&target, "# Dotfiles rules\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();

        let link = Harness::Claude.instructions_path(home, &env);
        symlink(&target, &link).unwrap();

        assert_eq!(install(Harness::Claude, home, &env).unwrap(), Outcome::Changed);
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "symlink must remain a symlink");
        assert_eq!(fs::read_link(&link).unwrap(), target);
        assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o640);
        let target_text = fs::read_to_string(&target).unwrap();
        assert!(target_text.starts_with("# Dotfiles rules\n\n"));
        assert!(target_text.contains(BEGIN_MARKER));

        // Second install on symlink is a no-op (mtime untouched).
        let old_time = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        OpenOptions::new().write(true).open(&target).unwrap().set_modified(old_time).unwrap();
        assert_eq!(install(Harness::Claude, home, &env).unwrap(), Outcome::Unchanged);
        assert_eq!(fs::metadata(&target).unwrap().modified().unwrap(), old_time);

        // Remove through symlink restores target and keeps the symlink even when empty.
        assert_eq!(remove(Harness::Claude, home, &env).unwrap(), Outcome::Changed);
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&target).unwrap(), "# Dotfiles rules\n");

        // Symlink whose target holds ONLY Olo's block: remove empties target, keeps symlink.
        fs::write(&target, "").unwrap();
        assert_eq!(install(Harness::Claude, home, &env).unwrap(), Outcome::Changed);
        assert_eq!(remove(Harness::Claude, home, &env).unwrap(), Outcome::Changed);
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "symlink must be kept even when emptied");
        assert_eq!(fs::read_to_string(&target).unwrap(), "");

        // 3. Dangling symlink: install creates target through link, remove empties target, link kept.
        let dangling_target = dotfiles.join("new-target.md");
        let _ = fs::remove_file(&link);
        symlink(&dangling_target, &link).unwrap();
        assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Absent);
        assert_eq!(install(Harness::Claude, home, &env).unwrap(), Outcome::Changed);
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert!(dangling_target.is_file());
        assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Current);
        assert_eq!(remove(Harness::Claude, home, &env).unwrap(), Outcome::Changed);
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&dangling_target).unwrap(), "");

        // 4. Symlinked config directory is treated as present.
        let real_dir = sb.path().join("real-claude-dir");
        fs::create_dir_all(&real_dir).unwrap();
        let link_home = sb.path().join("link-home");
        fs::create_dir_all(&link_home).unwrap();
        symlink(&real_dir, link_home.join(".claude")).unwrap();
        assert!(Harness::Claude.is_present(&link_home, &env));
        assert_eq!(install(Harness::Claude, &link_home, &env).unwrap(), Outcome::Changed);
        assert!(real_dir.join("CLAUDE.md").is_file());
    }

    #[test]
    fn block_positions_adjacent_versions_and_crlf_repairs() {
        let sb = Scratch::new("edge-blocks");
        let home = sb.path();
        fs::create_dir_all(home.join(".claude")).unwrap();
        let env = Env::default();
        let p = Harness::Claude.instructions_path(home, &env);

        // Block at top of file (with and without blank separator, no final newline).
        for (label, before_remove, expected_after) in [
            ("top_with_blank", format!("{BEGIN_MARKER}\nold\n{END_MARKER}\n\n# After"), "# After"),
            ("top_no_blank", format!("{BEGIN_MARKER}\nold\n{END_MARKER}\n# After"), "# After"),
            ("middle_no_blanks", format!("# Before\n{BEGIN_MARKER}\nold\n{END_MARKER}\n# After\n"), "# Before\n# After\n"),
            ("middle_crlf_no_blanks", format!("# Before\r\n{BEGIN_MARKER}\r\nold\r\n{END_MARKER}\r\n# After"), "# Before\r\n# After"),
            ("eof_no_trailing_nl", format!("# Before\n\n{BEGIN_MARKER}\nold\n{END_MARKER}"), "# Before\n"),
        ] {
            fs::write(&p, &before_remove).unwrap();
            assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Stale, "{label}");
            assert_eq!(install(Harness::Claude, home, &env).unwrap(), Outcome::Changed, "{label}");
            assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Current, "{label}");
            assert_eq!(remove(Harness::Claude, home, &env).unwrap(), Outcome::Changed, "{label}");
            assert_eq!(fs::read_to_string(&p).unwrap(), expected_after, "{label}");
        }

        // Two adjacent blocks with no blank line between them.
        let adjacent = format!("# Top\n\n{BEGIN_MARKER}\na\n{END_MARKER}\n{BEGIN_MARKER}\nb\n{END_MARKER}\n");
        fs::write(&p, &adjacent).unwrap();
        assert_eq!(install(Harness::Claude, home, &env).unwrap(), Outcome::Changed);
        assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Current);
        assert_eq!(remove(Harness::Claude, home, &env).unwrap(), Outcome::Changed);
        assert_eq!(fs::read_to_string(&p).unwrap(), "# Top\n");

        // Older/future/bare version markers are all recognized, upgraded, and removable.
        for marker in ["<!-- olo:rex-host:begin -->", "<!-- olo:rex-host:begin v0 -->", "<!-- olo:rex-host:begin v2 -->", "<!-- olo:rex-host:begin v1.5 -->"] {
            let content = format!("# Mine\n\n{marker}\nother version\n{END_MARKER}\n");
            fs::write(&p, &content).unwrap();
            assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Stale, "{marker}");
            assert_eq!(install(Harness::Claude, home, &env).unwrap(), Outcome::Changed, "{marker}");
            assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Current, "{marker}");
            assert!(fs::read_to_string(&p).unwrap().contains(BEGIN_MARKER));
            assert_eq!(remove(Harness::Claude, home, &env).unwrap(), Outcome::Changed, "{marker}");
            assert_eq!(fs::read_to_string(&p).unwrap(), "# Mine\n");
        }

        // Damaged CRLF file: unclosed begin with user text after stays CRLF and keeps user text.
        let crlf_damaged = "# Top\r\n\r\n<!-- olo:rex-host:begin v1 -->\r\n# Keep me\r\n";
        fs::write(&p, crlf_damaged).unwrap();
        assert_eq!(install(Harness::Claude, home, &env).unwrap(), Outcome::Changed);
        let bytes = fs::read(&p).unwrap();
        assert!(bytes.windows(9).any(|w| w == b"# Keep me"));
        for (idx, &b) in bytes.iter().enumerate() {
            if b == b'\n' {
                assert!(idx > 0 && bytes[idx - 1] == b'\r', "bare LF at {idx} after CRLF repair");
            }
        }
        assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Current);
    }

    #[test]
    fn repairs_damaged_blocks_without_losing_user_content() {
        let sb = Scratch::new("repair");
        let home = sb.path();
        fs::create_dir_all(home.join(".claude")).unwrap();
        let env = Env::default();
        let p = Harness::Claude.instructions_path(home, &env);

        // 1. Two blocks with user content before, between, and after.
        let two = format!("# Top\n\n<!-- olo:rex-host:begin v0 -->\nold 1\n{END_MARKER}\n\n# Middle\n\n{BEGIN_MARKER}\nold 2\n{END_MARKER}\n\n# Bottom\n");
        fs::write(&p, &two).unwrap();
        assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Stale);
        assert_eq!(install(Harness::Claude, home, &env).unwrap(), Outcome::Changed);
        let repaired = fs::read_to_string(&p).unwrap();
        assert_eq!(repaired.matches(BEGIN_MARKER).count(), 1);
        assert_eq!(repaired.matches(END_MARKER).count(), 1);
        assert!(repaired.contains("# Top\n"));
        assert!(repaired.contains("# Middle\n"));
        assert!(repaired.contains("# Bottom\n"));
        assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Current);
        assert_eq!(install(Harness::Claude, home, &env).unwrap(), Outcome::Unchanged);
        assert_eq!(remove(Harness::Claude, home, &env).unwrap(), Outcome::Changed);
        assert_eq!(fs::read_to_string(&p).unwrap(), "# Top\n\n# Middle\n\n# Bottom\n");

        // 2. Unclosed begin marker followed by user content: user content must survive.
        let unclosed_user = "# Before\n\n<!-- olo:rex-host:begin v1 -->\n# Must Not Be Lost\nKeep this rule.\n";
        fs::write(&p, unclosed_user).unwrap();
        assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Stale);
        assert_eq!(install(Harness::Claude, home, &env).unwrap(), Outcome::Changed);
        let repaired = fs::read_to_string(&p).unwrap();
        assert_eq!(repaired.matches(BEGIN_MARKER).count(), 1);
        assert_eq!(repaired.matches(END_MARKER).count(), 1);
        assert!(repaired.contains("# Before\n"));
        assert!(repaired.contains("# Must Not Be Lost\nKeep this rule.\n"));
        assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Current);

        // 3. Unclosed begin marker where only the end marker line was deleted (exact body + user text after).
        let missing_end = format!("# Top\n\n{BEGIN_MARKER}\n{}# User Tail\n", section());
        fs::write(&p, &missing_end).unwrap();
        assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Stale);
        assert_eq!(install(Harness::Claude, home, &env).unwrap(), Outcome::Changed);
        let repaired = fs::read_to_string(&p).unwrap();
        assert_eq!(repaired.matches(section().lines().next().unwrap()).count(), 1, "section must not be duplicated");
        assert!(repaired.contains("# Top\n"));
        assert!(repaired.contains("# User Tail\n"));
        assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Current);

        // 4. Two begins before one end with user text in between.
        let double_begin = format!("<!-- olo:rex-host:begin v1 -->\n# Keep Between Begins\n\n{BEGIN_MARKER}\nstale\n{END_MARKER}\n");
        fs::write(&p, &double_begin).unwrap();
        assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Stale);
        assert_eq!(install(Harness::Claude, home, &env).unwrap(), Outcome::Changed);
        let repaired = fs::read_to_string(&p).unwrap();
        assert_eq!(repaired.matches(BEGIN_MARKER).count(), 1);
        assert_eq!(repaired.matches(END_MARKER).count(), 1);
        assert!(repaired.contains("# Keep Between Begins\n"));
        assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Current);

        // 5. Stray end marker with no begin.
        let stray_end = format!("# A\n{END_MARKER}\n# B\n");
        fs::write(&p, &stray_end).unwrap();
        assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Stale);
        assert_eq!(install(Harness::Claude, home, &env).unwrap(), Outcome::Changed);
        let repaired = fs::read_to_string(&p).unwrap();
        assert_eq!(repaired.matches(BEGIN_MARKER).count(), 1);
        assert_eq!(repaired.matches(END_MARKER).count(), 1);
        assert!(repaired.contains("# A\n"));
        assert!(repaired.contains("# B\n"));
        assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Current);

        // 6. Inline mention of the marker inside prose is ignored.
        let inline = format!("Do not touch `{BEGIN_MARKER}` inside prose.\n");
        fs::write(&p, &inline).unwrap();
        assert_eq!(status(Harness::Claude, home, &env).unwrap(), Status::Absent);
        assert_eq!(install(Harness::Claude, home, &env).unwrap(), Outcome::Changed);
        assert_eq!(remove(Harness::Claude, home, &env).unwrap(), Outcome::Changed);
        assert_eq!(fs::read_to_string(&p).unwrap(), inline);
    }

    #[test]
    fn antigravity_owned_rule_file_lifecycle() {
        let sb = Scratch::new("agy");
        let home = sb.path();
        fs::create_dir_all(home.join(".gemini")).unwrap();
        let env = Env::default();
        let p = Harness::Antigravity.instructions_path(home, &env);

        // Also place a user rule beside ours to prove remove never touches it.
        let user_rule = home.join(".gemini/config/rules/my-own-rule.md");
        fs::create_dir_all(user_rule.parent().unwrap()).unwrap();
        fs::write(&user_rule, "---\ntrigger: always_on\n---\n# Mine\n").unwrap();

        assert_eq!(status(Harness::Antigravity, home, &env).unwrap(), Status::Absent);
        assert_eq!(install(Harness::Antigravity, home, &env).unwrap(), Outcome::Changed);
        assert_eq!(status(Harness::Antigravity, home, &env).unwrap(), Status::Current);
        let content = fs::read_to_string(&p).unwrap();
        assert!(content.starts_with(AGY_FRONTMATTER));
        assert!(content.contains(BEGIN_MARKER));
        assert!(content.contains(section()));
        assert!(content.contains(END_MARKER));

        // Stale rule file is detected and overwritten.
        fs::write(&p, "---\ntrigger: always_on\n---\nold content\n").unwrap();
        assert_eq!(status(Harness::Antigravity, home, &env).unwrap(), Status::Stale);
        assert_eq!(install(Harness::Antigravity, home, &env).unwrap(), Outcome::Changed);
        assert_eq!(status(Harness::Antigravity, home, &env).unwrap(), Status::Current);

        // Remove deletes only Olo's rule file, leaving user's rule intact.
        assert_eq!(remove(Harness::Antigravity, home, &env).unwrap(), Outcome::Changed);
        assert!(!p.exists());
        assert_eq!(fs::read_to_string(&user_rule).unwrap(), "---\ntrigger: always_on\n---\n# Mine\n");
    }

    #[test]
    fn env_overrides_drive_install_and_status() {
        let sb = Scratch::new("env-override");
        let home = sb.path().join("home");
        let alt_claude = sb.path().join("custom-claude");
        let alt_codex = sb.path().join("custom-codex");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&alt_claude).unwrap();
        fs::create_dir_all(&alt_codex).unwrap();

        let env = Env { claude_config_dir: Some(alt_claude.clone()), codex_home: Some(alt_codex.clone()) };
        assert_eq!(install(Harness::Claude, &home, &env).unwrap(), Outcome::Changed);
        assert_eq!(install(Harness::Codex, &home, &env).unwrap(), Outcome::Changed);
        assert_eq!(install(Harness::Antigravity, &home, &env).unwrap(), Outcome::NotInstalled);

        assert!(alt_claude.join("CLAUDE.md").is_file());
        assert!(alt_codex.join("AGENTS.md").is_file());
        assert!(!home.join(".claude").exists());
        assert!(!home.join(".codex").exists());
        assert_eq!(status(Harness::Claude, &home, &env).unwrap(), Status::Current);
        assert_eq!(status(Harness::Codex, &home, &env).unwrap(), Status::Current);
    }

    #[test]
    fn batch_helpers_report_every_harness() {
        let sb = Scratch::new("batch");
        let home = sb.path();
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::create_dir_all(home.join(".gemini")).unwrap();
        let env = Env::default();

        let before = status_all(home, &env);
        assert_eq!(before.len(), 3);
        assert_eq!(before[0].result.as_ref().copied().unwrap(), Status::Absent);
        assert_eq!(before[1].result.as_ref().copied().unwrap(), Status::NotInstalled);
        assert_eq!(before[2].result.as_ref().copied().unwrap(), Status::Absent);

        let ins = install_all(home, &env);
        assert_eq!(ins[0].result.as_ref().copied().unwrap(), Outcome::Changed);
        assert_eq!(ins[1].result.as_ref().copied().unwrap(), Outcome::NotInstalled);
        assert_eq!(ins[2].result.as_ref().copied().unwrap(), Outcome::Changed);

        let rem = remove_all(home, &env);
        assert_eq!(rem[0].result.as_ref().copied().unwrap(), Outcome::Changed);
        assert_eq!(rem[1].result.as_ref().copied().unwrap(), Outcome::NotInstalled);
        assert_eq!(rem[2].result.as_ref().copied().unwrap(), Outcome::Changed);
    }
}
