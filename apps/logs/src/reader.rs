//! Reading this user's journal on a worker thread: every file, oldest
//! first, sent on in batches as lines, then the file being written,
//! followed as it grows (woken by its changes, `olo_kit::watch`: journald
//! touches the file for readers after each write) and across journald
//! rotating it. Or a log file an app writes beside the journal, the
//! same way (`run_file`).

use std::{sync::Arc, time::Duration};

use crate::{
    journal::{self, Journal},
    model::{self, Built, Builder, Line},
};

pub enum Update {
    /// New lines, and more text for lines already sent (by index).
    Lines { new: Vec<Line>, more: Vec<(usize, String)> },
    /// Everything there was has been read; new lines follow as written.
    Loaded,
    Failed(String),
}

const BATCH: usize = 4000;
/// How often to look anyway: without inotify, or for a write that
/// slipped by.
const POLL: Duration = Duration::from_millis(500);
const LOOK: Duration = Duration::from_secs(30);

/// Read the journal, giving `on` each update; with `follow`, keep going
/// as it is written. Stops when `on` returns false.
pub fn run(follow: bool, on: &mut dyn FnMut(Update) -> bool) {
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let files = journal::user_files(uid);
    if files.is_empty() {
        on(Update::Failed("There is no journal for this user to read (in /var/log/journal or /run/log/journal).".into()));
        return;
    }
    let mut builder = Builder::default();
    let mut entries = Vec::new();
    let mut active: Option<Journal> = None;
    let last = files.len() - 1;
    for (i, path) in files.iter().enumerate() {
        let mut j = match Journal::open(path) {
            Ok(j) => j,
            Err(e) if i == last => {
                on(Update::Failed(format!("The journal couldn't be read: {e}")));
                return;
            }
            Err(_) => continue,
        };
        loop {
            entries.clear();
            if j.read_new(BATCH, &mut entries).is_err() {
                break;
            }
            if entries.is_empty() {
                break;
            }
            if !on(build(&mut builder, &entries)) {
                return;
            }
        }
        if i == last {
            active = Some(j);
        }
    }
    if !on(Update::Loaded) || !follow {
        return;
    }
    let Some(mut j) = active else { return };
    let (tx, woken) = std::sync::mpsc::channel();
    let watch = olo_kit::watch::watch([j.path.clone()], move |_| {
        let _ = tx.send(());
    });
    let wait = if watch.is_some() { LOOK } else { POLL };
    // (What was written before the watch began is read at once.)
    let mut first = true;
    loop {
        if !std::mem::take(&mut first) {
            let _ = woken.recv_timeout(wait);
        }
        while woken.try_recv().is_ok() {}
        loop {
            entries.clear();
            if j.read_new(BATCH, &mut entries).is_err() || entries.is_empty() {
                break;
            }
            if !on(build(&mut builder, &entries)) {
                return;
            }
        }
        // Rotated: what was written to the old file is read above (we
        // still have it open); go on with the new one.
        if j.replaced()
            && let Ok(new) = Journal::open(&j.path)
        {
            j = new;
        }
        // Nothing new: check the receiver is still there.
        if !on(Update::Lines { new: Vec::new(), more: Vec::new() }) {
            return;
        }
    }
}

fn build(builder: &mut Builder, entries: &[journal::Entry]) -> Update {
    let mut new: Vec<Line> = Vec::with_capacity(entries.len());
    let mut more = Vec::new();
    let first = builder.count;
    for e in entries {
        match builder.add(e) {
            Some(Built::New(l)) => new.push(l),
            // A continuation of a line in this batch goes straight onto it.
            Some(Built::Continues { index, text }) if index >= first => {
                let l = &mut new[index - first];
                l.message.push('\n');
                l.message.push_str(&text);
            }
            Some(Built::Continues { index, text }) => more.push((index, text)),
            None => {}
        }
    }
    Update::Lines { new, more }
}

/// Read the log file `path` (one an app writes beside the journal: an
/// Agent Screen's), giving `on` each update; with `follow`, keep going as
/// it grows (from the start again if it is cut short, as a screen started
/// again under the same name does). Its lines are its name's; those
/// without a time of their own take the file's (as first read) or the
/// time they came.
pub fn run_file(path: &std::path::Path, follow: bool, on: &mut dyn FnMut(Update) -> bool) {
    use std::io::{Read, Seek, SeekFrom};
    let program: Arc<str> = Arc::from(path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    let boot = model::current_boot().unwrap_or_default();
    let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros() as u64);
    let written = std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or_else(now, |d| d.as_micros() as u64);
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            on(Update::Failed(format!("“{}” couldn't be read: {e}", path.display())));
            return;
        }
    };
    let (mut at, mut count) = (0u64, 0usize);
    // A line not yet ended.
    let mut partial: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 1 << 20];
    // Read what has been written since `at`, as lines.
    let mut read = |f: &mut std::fs::File, at: &mut u64, count: &mut usize, partial: &mut Vec<u8>, fallback: u64, on: &mut dyn FnMut(Update) -> bool| -> bool {
        let len = f.metadata().map_or(0, |m| m.len());
        if len < *at {
            // Cut short: from the start again.
            *at = 0;
            partial.clear();
        }
        if f.seek(SeekFrom::Start(*at)).is_err() {
            return true;
        }
        loop {
            let n = match f.read(&mut buf) {
                Ok(0) | Err(_) => return true,
                Ok(n) => n,
            };
            *at += n as u64;
            partial.extend_from_slice(&buf[..n]);
            let Some(end) = partial.iter().rposition(|b| *b == b'\n') else { continue };
            let whole: Vec<u8> = partial.drain(..=end).collect();
            let (mut new, mut more): (Vec<Line>, Vec<(usize, String)>) = (Vec::new(), Vec::new());
            let first = *count;
            for raw in String::from_utf8_lossy(&whole).lines() {
                if raw.trim().is_empty() {
                    continue;
                }
                if *count > 0 && model::continues_line(raw) {
                    let text = model::strip_ansi(raw).trim_end().to_owned();
                    match new.last_mut() {
                        Some(l) => {
                            l.message.push('\n');
                            l.message.push_str(&text);
                        }
                        None => more.push((first - 1, text)),
                    }
                    continue;
                }
                new.push(model::file_line(raw, &program, fallback, boot));
                *count += 1;
            }
            if !on(Update::Lines { new, more }) {
                return false;
            }
        }
    };
    if !read(&mut f, &mut at, &mut count, &mut partial, written, on) || !on(Update::Loaded) || !follow {
        return;
    }
    let (tx, woken) = std::sync::mpsc::channel();
    let watch = olo_kit::watch::watch([path.to_path_buf()], move |_| {
        let _ = tx.send(());
    });
    let wait = if watch.is_some() { LOOK } else { POLL };
    // (What was written before the watch began is read at once.)
    let mut first = true;
    loop {
        if !std::mem::take(&mut first) {
            let _ = woken.recv_timeout(wait);
        }
        while woken.try_recv().is_ok() {}
        if !read(&mut f, &mut at, &mut count, &mut partial, now(), on) {
            return;
        }
        // Nothing new: check the receiver is still there.
        if !on(Update::Lines { new: Vec::new(), more: Vec::new() }) {
            return;
        }
    }
}
