//! Reminders: `olo-todo --service`, which the session keeps running (with
//! no window). It shows each to-do's reminder at its time as a
//! notification: the to-do's title, when it's due and its list, in its
//! list's colour, with Done (it's checked off, there and then), In an
//! Hour (the reminder comes again then) and Show (To-Do opens on it).
//!
//! It sleeps until the next reminder is due, on the wall's clock
//! (`olo_kit::clock::Alarm`: through a sleep of the computer, and a clock
//! set), and is woken when a list or a to-do changes, reading again only
//! what changed. With no to-do to remind of it waits for one, doing
//! nothing.

use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use jiff::{Zoned, tz::TimeZone};
use olo_kit::{
    clock::Alarm,
    notify::{Note, NoteEvent, NoteId, NoteKind, Notifier},
};
use olo_todo::{Book, item::remind_at, store};
use serde::{Deserialize, Serialize};

use crate::words;

/// A reminder comes again this long after In an Hour.
const SNOOZE: i64 = 3600;
/// One whose time passed longer ago than this (the computer was off) is
/// let go: its to-do says it's overdue.
const STALE: i64 = 86_400;

/// What's kept between runs (`$XDG_STATE_HOME/olo/todo-reminders.json`):
/// the reminders shown already (by key, with when), not shown again
/// after a restart; and those put off (by UID, until when).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Kept {
    pub fired: BTreeMap<String, i64>,
    pub snoozed: BTreeMap<String, i64>,
}

fn kept_path() -> PathBuf {
    olo_kit::state_dir().join("todo-reminders.json")
}

/// A reminder to show: of which to-do, when.
#[derive(Debug, Clone, PartialEq)]
pub struct Reminder {
    /// What tells it from another (its to-do, its time).
    pub key: String,
    pub uid: String,
    pub at: i64,
}

/// Every reminder of what's still to do: each to-do's own, and one for
/// each put off, at the time it was put off to.
pub fn reminders(book: &Book, kept: &Kept, zone: &TimeZone) -> Vec<Reminder> {
    let mut out = Vec::new();
    for item in book.items.iter().filter(|i| i.todo.open()) {
        let uid = &item.todo.uid;
        if let Some(at) = remind_at(&item.todo, zone).map(|t| t.as_second()) {
            out.push(Reminder { key: format!("{uid}@{at}"), uid: uid.clone(), at });
        }
        if let Some(at) = kept.snoozed.get(uid) {
            out.push(Reminder { key: format!("{uid}@later@{at}"), uid: uid.clone(), at: *at });
        }
    }
    out.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.key.cmp(&b.key)));
    out
}

/// What a reminder says under its to-do's title: when it's due, and its
/// list.
pub fn said(item: &olo_todo::Item, book: &Book, now: &Zoned, h24: bool) -> String {
    let list = book.list(&item.list).map(|l| l.name.clone()).unwrap_or_default();
    match &item.todo.due {
        Some(due) if olo_todo::view::overdue(due, now) => olo_kit::tr!("Was due {} · {}", words::due_in_words(due, now, h24), list),
        Some(due) => olo_kit::tr!("Due {} · {}", words::due_in_words(due, now, h24), list),
        None => list,
    }
}

pub fn run() -> std::process::ExitCode {
    let alarm = match Alarm::new() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("olo-todo: no timer to remind with: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let root = store::root();
    let _ = std::fs::create_dir_all(&root);
    // What changed since the last waking (the watch's), read then.
    let changed: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
    let h24 = Arc::new(AtomicBool::new(true));
    {
        let (h24, waker) = (h24.clone(), alarm.waker());
        olo_kit::settings::watch(move |s| {
            if h24.swap(s.clock.twenty_four_hour, Ordering::Relaxed) != s.clock.twenty_four_hour {
                waker.wake();
            }
        });
    }
    // Which to-do each notification is of, and what's asked of it there
    // (done on this loop, which owns the files' reading).
    let showing: Arc<Mutex<HashMap<NoteId, String>>> = Arc::default();
    let asked: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let notes = {
        let (showing, asked, waker) = (showing.clone(), asked.clone(), alarm.waker());
        Notifier::with_events("To-Do", "olo-todo", move |e| match e {
            NoteEvent::Action { id, key, token } => {
                let Some(uid) = showing.lock().unwrap().get(&id).cloned() else { return };
                if key == "done" || key == "later" {
                    asked.lock().unwrap().push((key, uid));
                    waker.wake();
                    return;
                }
                let Ok(exe) = std::env::current_exe() else { return };
                let exe = PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
                let mut cmd = std::process::Command::new(exe);
                cmd.args(["--show", &uid]).stdin(std::process::Stdio::null());
                if let Some(token) = token {
                    cmd.env("XDG_ACTIVATION_TOKEN", token);
                }
                let _ = cmd.spawn().map(olo_xdg::reap_child);
            }
            NoteEvent::Closed { id } => {
                showing.lock().unwrap().remove(&id);
            }
            _ => {}
        })
    };
    let mut kept: Kept = olo_kit::store::load(&kept_path()).unwrap_or_default();
    let mut book = Book::load(&root);
    let mut watched: Vec<PathBuf> = Vec::new();
    let mut _watch = None;
    loop {
        // What changed, read again; the folders watched, as the lists are now.
        let paths = std::mem::take(&mut *changed.lock().unwrap());
        if !paths.is_empty() {
            book.refresh(&root, &paths);
        }
        let mut folders = vec![root.clone()];
        folders.extend(book.lists.iter().map(|l| root.join(&l.id)));
        if folders != watched {
            watched = folders.clone();
            let (changed, waker) = (changed.clone(), alarm.waker());
            _watch = olo_kit::watch::watch(folders, move |p| {
                changed.lock().unwrap().extend(p.iter().cloned());
                waker.wake();
            });
            // (Whatever changed while they weren't watched.)
            book = Book::load(&root);
        }
        let zone = TimeZone::system();
        let now = crate::now(&zone);
        let unix = now.timestamp().as_second();
        let before = kept.clone();
        // What was asked of a notification: its to-do done, or put off.
        for (key, uid) in std::mem::take(&mut *asked.lock().unwrap()) {
            match key.as_str() {
                "done" => {
                    if let Some(item) = book.item(&uid).cloned() {
                        let mut todo = item.todo.clone();
                        todo.status = olo_todo::Status::Done;
                        todo.done_at = Some(crate::now(&jiff::tz::TimeZone::system()).timestamp());
                        match store::change(&item, &todo) {
                            Ok(_) => {
                                book.refresh(&root, std::slice::from_ref(&item.file));
                            }
                            Err(e) => tracing::warn!("to-do: {uid}: {e}"),
                        }
                    }
                }
                _ => {
                    kept.snoozed.insert(uid, unix + SNOOZE);
                }
            }
        }
        // (The reminders in the person's language, as it is now.)
        olo_kit::words::choose();
        let list = reminders(&book, &kept, &zone);
        for r in list.iter().filter(|r| r.at <= unix) {
            if kept.fired.contains_key(&r.key) {
                continue;
            }
            kept.fired.insert(r.key.clone(), r.at);
            kept.snoozed.retain(|uid, until| !(uid == &r.uid && *until <= unix));
            let Some(item) = book.item(&r.uid) else { continue };
            if unix - r.at > STALE {
                continue;
            }
            let fill = olo_kit::theme::palette().hue(book.list(&item.list).map_or(olo_theme::Hue::Blue, |l| l.hue));
            let note = Note::new(item.todo.title.clone())
                .body(said(item, &book, &now, h24.load(Ordering::Relaxed)))
                .icon("todo")
                .kind(NoteKind::Important)
                .accent([fill[0], fill[1], fill[2]])
                .action("done", olo_kit::words::tr("Done"))
                .action("later", olo_kit::words::tr("In an Hour"))
                .action("show", olo_kit::words::tr("Show"))
                .shows(["--show".to_owned(), r.uid.clone()])
                .sticky(true);
            let id = notes.show(None, note);
            showing.lock().unwrap().insert(id, r.uid.clone());
        }
        // A to-do done or gone meanwhile: its notification goes.
        let stale: Vec<NoteId> = showing.lock().unwrap().iter().filter(|(_, uid)| !book.item(uid).is_some_and(|i| i.todo.open())).map(|(id, _)| *id).collect();
        for id in stale {
            notes.close(id);
            showing.lock().unwrap().remove(&id);
        }
        // (Those long past are forgotten; one put off for a to-do that's
        // done or gone, too.)
        kept.fired.retain(|_, at| *at > unix - 7 * 86_400);
        kept.snoozed.retain(|uid, _| book.item(uid).is_some_and(|i| i.todo.open()));
        if kept != before {
            olo_kit::store::save(kept_path(), &kept);
        }
        let next = list.iter().filter(|r| r.at > unix).map(|r| r.at).min();
        drop(list);
        alarm.wait(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use olo_calendar::event::When;
    use olo_todo::{Item, List, Remind, ToDo};

    #[test]
    fn reminders_of_what_is_still_to_do() {
        let zone = TimeZone::UTC;
        let at = |h: i8| When::Zoned(jiff::civil::date(2026, 11, 20).at(h, 0, 0, 0).to_zoned(TimeZone::UTC).unwrap());
        let item = |uid: &str, due: Option<When>, remind: Option<Remind>, done: bool| {
            let mut t = ToDo::new(uid, uid);
            t.due = due;
            t.remind = remind;
            if done {
                t.status = olo_todo::Status::Done;
            }
            Item { list: "home".into(), file: format!("/x/{uid}.ics").into(), changed: None, todo: t }
        };
        let book = Book {
            lists: vec![List { id: "home".into(), name: "Home".into(), hue: olo_theme::Hue::Green, sections: vec![] }],
            items: vec![
                item("call", Some(at(15)), Some(Remind::Before(900)), false),
                item("rent", Some(When::Date(jiff::civil::date(2026, 11, 21))), Some(Remind::Before(-9 * 3600)), false),
                item("done", Some(at(10)), Some(Remind::Before(0)), true),
                item("none", Some(at(11)), None, false),
            ],
        };
        let mut kept = Kept::default();
        let unix = |text: &str| olo_todo::item::moment(text).unwrap().as_second();
        let list = reminders(&book, &kept, &zone);
        assert_eq!(list.iter().map(|r| (r.uid.as_str(), r.at)).collect::<Vec<_>>(), [("call", unix("20261120T144500Z")), ("rent", unix("20261121T090000Z"))]);
        // Put off: once more, then.
        kept.snoozed.insert("call".into(), unix("20261120T160000Z"));
        let list = reminders(&book, &kept, &zone);
        assert_eq!(list.len(), 3);
        assert_eq!(list[1].key, format!("call@later@{}", unix("20261120T160000Z")));
        // What it says: when it's due, its list.
        let now = jiff::civil::date(2026, 11, 20).at(14, 45, 0, 0).to_zoned(TimeZone::UTC).unwrap();
        assert_eq!(said(&book.items[0], &book, &now, false), "Due today, 3 PM · Home");
        let late = jiff::civil::date(2026, 11, 20).at(16, 0, 0, 0).to_zoned(TimeZone::UTC).unwrap();
        assert_eq!(said(&book.items[0], &book, &late, false), "Was due today, 3 PM · Home");
    }
}
