//! Reading this user's journal on a worker thread: every file, oldest
//! first, sent on in batches as lines, then the file being written,
//! followed as it grows and across journald rotating it.

use std::time::Duration;

use crate::{
    journal::{self, Journal},
    model::{Built, Builder, Line},
};

pub enum Update {
    /// New lines, and more text for lines already sent (by index).
    Lines { new: Vec<Line>, more: Vec<(usize, String)> },
    /// Everything there was has been read; new lines follow as written.
    Loaded,
    Failed(String),
}

const BATCH: usize = 4000;
const POLL: Duration = Duration::from_millis(500);

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
    loop {
        std::thread::sleep(POLL);
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
