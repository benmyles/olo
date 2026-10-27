//! Find, powered by ripgrep's crates: `ignore` walks the tree in parallel
//! (skipping hidden and .gitignored files unless asked not to) and
//! `grep-searcher` reads file contents, skipping binary files. Matching
//! follows ripgrep's defaults: a literal string, smart case (case-blind
//! unless the query has a capital letter).

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::{WalkBuilder, WalkState};

use crate::fs::{self, Entry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Names,
    Contents,
    Either,
}

impl Mode {
    pub const LABELS: [&'static str; 3] = ["Name", "Contents", "Name or contents"];

    pub fn from_index(i: usize) -> Mode {
        match i {
            1 => Mode::Contents,
            2 => Mode::Either,
            _ => Mode::Names,
        }
    }
}

/// A result: the item, and for a contents match the first matching line.
#[derive(Debug, Clone)]
pub struct Hit {
    pub entry: Entry,
    pub line: Option<(u64, String)>,
}

/// Live progress, read by the window while the search runs.
#[derive(Debug, Default)]
pub struct Progress {
    pub scanned: AtomicU64,
    /// The folder most recently entered.
    pub folder: Mutex<PathBuf>,
}

pub struct Query {
    pub root: PathBuf,
    pub text: String,
    pub mode: Mode,
    /// Also look in hidden files and ones .gitignore (etc.) excludes.
    pub everything: bool,
}

/// Files larger than this are found by name but their contents aren't read.
const MAX_CONTENT_SIZE: u64 = 64 * 1024 * 1024;
/// Longest snippet of a matching line shown in the results.
const SNIPPET: usize = 160;

fn matcher(text: &str) -> Option<RegexMatcher> {
    RegexMatcherBuilder::new().fixed_strings(true).case_smart(true).build(text).ok()
}

/// Run `q` on worker threads until done or `cancel` is set. `send` gets
/// batches of hits (at most every 150 ms) and is called a last time with
/// `done = true`.
pub fn spawn(q: Query, cancel: Arc<AtomicBool>, progress: Arc<Progress>, send: impl Fn(Vec<Hit>, bool) + Send + 'static) {
    std::thread::spawn(move || {
        let Some(m) = matcher(q.text.trim()) else {
            send(Vec::new(), true);
            return;
        };
        let (tx, rx) = mpsc::channel::<Hit>();
        let walker = {
            let cancel = cancel.clone();
            let root = q.root.clone();
            let mode = q.mode;
            let everything = q.everything;
            std::thread::spawn(move || walk(&root, mode, everything, &m, &cancel, &progress, tx))
        };
        // Batch results so the window redraws a few times a second, not
        // once per hit.
        let mut batch = Vec::new();
        let mut last = Instant::now();
        loop {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(hit) => batch.push(hit),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            if !batch.is_empty() && last.elapsed() > Duration::from_millis(150) {
                send(std::mem::take(&mut batch), false);
                last = Instant::now();
            }
        }
        let _ = walker.join();
        if !cancel.load(Ordering::Relaxed) {
            send(batch, true);
        }
    });
}

fn walk(
    root: &Path,
    mode: Mode,
    everything: bool,
    m: &RegexMatcher,
    cancel: &AtomicBool,
    progress: &Progress,
    tx: mpsc::Sender<Hit>,
) {
    let skip = skipped_mounts();
    WalkBuilder::new(root)
        .standard_filters(!everything)
        .filter_entry(move |e| !skip.iter().any(|m| m == e.path()))
        .follow_links(false)
        .build_parallel()
        .run(|| {
            let tx = tx.clone();
            let mut searcher = SearcherBuilder::new()
                .binary_detection(BinaryDetection::quit(0))
                .line_number(true)
                .build();
            Box::new(move |result| {
                if cancel.load(Ordering::Relaxed) {
                    return WalkState::Quit;
                }
                let Ok(dent) = result else { return WalkState::Continue };
                if dent.depth() == 0 {
                    return WalkState::Continue;
                }
                let path = dent.path();
                progress.scanned.fetch_add(1, Ordering::Relaxed);
                if dent.file_type().is_some_and(|t| t.is_dir())
                    && let Ok(mut f) = progress.folder.try_lock()
                {
                    *f = path.to_owned();
                }
                let name = dent.file_name().to_string_lossy();
                let by_name = mode != Mode::Contents && m.is_match(name.as_bytes()).unwrap_or(false);
                let is_file = dent.file_type().is_some_and(|t| t.is_file());
                let line = if !by_name && mode != Mode::Names && is_file {
                    first_match(&mut searcher, m, path)
                } else {
                    None
                };
                if (by_name || line.is_some())
                    && let Some(entry) = fs::entry(path)
                    && tx.send(Hit { entry, line }).is_err()
                {
                    return WalkState::Quit;
                }
                WalkState::Continue
            })
        });
}

/// Mount points not worth searching: kernel pseudo-filesystems (/proc,
/// /sys, /dev…), runtime state and network mounts, which can be slow or
/// hang. Real disks mounted anywhere (/home, /var, /run/media/…) are kept.
fn skipped_mounts() -> Vec<PathBuf> {
    const VIRTUAL: &[&str] = &[
        "proc", "sysfs", "devtmpfs", "devpts", "cgroup", "cgroup2", "securityfs", "debugfs", "tracefs",
        "fusectl", "configfs", "bpf", "pstore", "efivarfs", "mqueue", "hugetlbfs", "autofs", "binfmt_misc",
        "rpc_pipefs", "nfs", "nfs4", "cifs", "smb3", "9p", "fuse.sshfs", "fuse.gvfsd-fuse", "fuse.portal",
    ];
    let Ok(mounts) = std::fs::read_to_string("/proc/self/mounts") else { return Vec::new() };
    mounts
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (_, dir, kind) = (f.next()?, f.next()?, f.next()?);
            // /proc/mounts escapes spaces as \040.
            let dir = PathBuf::from(dir.replace("\\040", " "));
            let runtime = kind == "tmpfs" && (dir.starts_with("/run") && !dir.starts_with("/run/media") || dir.starts_with("/dev"));
            (VIRTUAL.contains(&kind) || runtime).then_some(dir)
        })
        .collect()
}

/// The first line of `path` that matches, with its number.
fn first_match(searcher: &mut Searcher, m: &RegexMatcher, path: &Path) -> Option<(u64, String)> {
    if std::fs::metadata(path).map(|md| md.len() > MAX_CONTENT_SIZE).unwrap_or(true) {
        return None;
    }
    let mut found = None;
    let _ = searcher.search_path(m, path, First(&mut found));
    found
}

/// A sink that keeps the first match and stops.
struct First<'a>(&'a mut Option<(u64, String)>);

impl Sink for First<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        let text = String::from_utf8_lossy(mat.bytes());
        let text = text.trim();
        let snippet: String = if text.chars().count() > SNIPPET {
            text.chars().take(SNIPPET).chain(['…']).collect()
        } else {
            text.to_owned()
        };
        *self.0 = Some((mat.line_number().unwrap_or(0), snippet));
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(root: &Path, text: &str, mode: Mode, everything: bool) -> Vec<Hit> {
        let (tx, rx) = mpsc::channel();
        spawn(
            Query { root: root.into(), text: text.into(), mode, everything },
            Arc::new(AtomicBool::new(false)),
            Arc::default(),
            move |hits, done| {
                let _ = tx.send((hits, done));
            },
        );
        let mut all = Vec::new();
        for (hits, done) in rx {
            all.extend(hits);
            if done {
                break;
            }
        }
        all.sort_by(|a, b| a.entry.path.cmp(&b.entry.path));
        all
    }

    #[test]
    fn names_contents_and_filters() {
        let dir = std::env::temp_dir().join(format!("olo-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::write(dir.join("Report.txt"), "nothing here\n").unwrap();
        std::fs::write(dir.join("sub/notes.md"), "line one\nthe quarterly REPORT is due\n").unwrap();
        std::fs::write(dir.join("sub/blob.bin"), b"report\0\x01\x02").unwrap();
        std::fs::write(dir.join(".hidden/report-old.txt"), "x").unwrap();
        // .gitignore applies only inside a git repo unless require_git is off.
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".gitignore"), "ignored/\n").unwrap();
        std::fs::create_dir_all(dir.join("ignored")).unwrap();
        std::fs::write(dir.join("ignored/report.log"), "report").unwrap();

        let names = |hits: &[Hit]| hits.iter().map(|h| h.entry.name.clone()).collect::<Vec<_>>();

        // Smart case: lower-case query is case-blind.
        assert_eq!(names(&run(&dir, "report", Mode::Names, false)), ["Report.txt"]);
        // A capital makes it case-sensitive.
        assert!(run(&dir, "REPORT", Mode::Names, false).is_empty());
        // Contents: first matching line with its number; binaries skipped.
        let c = run(&dir, "report", Mode::Contents, false);
        assert_eq!(names(&c), ["notes.md"]);
        assert_eq!(c[0].line, Some((2, "the quarterly REPORT is due".into())));
        // Hidden and ignored files only when asked.
        let all = names(&run(&dir, "report", Mode::Names, true));
        assert!(all.contains(&"report-old.txt".to_owned()) && all.contains(&"report.log".to_owned()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
