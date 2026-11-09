//! DiskBackup's main window (Ben, 2026-11-08: "improve the ux in
//! diskbackup … it's complicated and not intuitive"; "where are the
//! settings for limiting up/down speed and resource usage and knowing
//! exactly what's being backed up now what the plan is what's still
//! remaining"; "i should also be able to choose if i even want auto
//! backups … i should be able to clear out / remove old backups to start
//! fresh"). One question first, where backups go, until that's answered;
//! then what matters most at the top: whether your files are safe (when
//! they were last backed up, where to, when next), what's wrong with the
//! fix beside it, and while a backup runs, the whole of it (how far, of
//! how much, what's left, how fast, the file now, each folder's turn).
//! Under that, the settings by what they're about, in tabs: Schedule,
//! Files, Backups (each one kept: delete some, or erase them all),
//! Speed & Resources, Destination.
//!
//! A backup that didn't finish is said (what it got through, that the next
//! goes on from it) and listed first in Backups, to delete if it isn't
//! wanted; each backup's check is listed too (after it was made, or
//! Validate's, which reads one back whole), and damage to the latest is
//! said at the top. Destination says what DiskBackup is connected to (whose
//! Google Drive, its space; a folder's disk), with Disconnect (Ben,
//! 2026-11-08: "when i'm connected it should show the details of that
//! connection and a button to disconnect it").

use olo_diskbackup::exclude::GROUPS;
use olo_protocol::diskbackup::{Check, Doing, Priority, Progress, Settings, SnapshotInfo, Stage, State, Target};
use olo_ui::{
    Menu, MenuBar,
    gpui::{
        self, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, canvas, div, px,
    },
    metrics as m, style, theme,
    widgets::{self, BBox, Button, CheckBox, Column, ColumnListView, ListModel, MenuField, RadioButton, StatusBar, StringView, TabView, TextControl, TextField, TextFieldEvent, Tone},
};

use crate::{
    app::{Disk, clock, count, plural, size, speed_label},
    target::{self, Where},
};

/// The tabs, in order.
const TABS: [&str; 5] = ["Schedule", "Files", "Backups", "Speed & Resources", "Destination"];
const DESTINATION: usize = 4;

/// Busy with something that can be stopped (a backup, a tidy, erasing,
/// validating).
fn busy_with(st: &State) -> bool {
    matches!(st.doing, Doing::BackingUp | Doing::Pruning | Doing::Erasing | Doing::Validating | Doing::Deleting { .. })
}

/// What a backup takes "every": minutes, and what it's called.
const EVERY: &[(u32, &str)] = &[(15, "15 minutes"), (30, "30 minutes"), (60, "hour"), (120, "2 hours"), (360, "6 hours"), (720, "12 hours"), (1440, "day")];
/// Speed limits offered, KB/s (0: none).
const SPEEDS: &[u64] = &[0, 256, 512, 1000, 2000, 5000, 10_000, 25_000];
/// How long each kind of backup is kept: the choices.
const HOURLY: &[(u32, &str)] = &[(0, "None"), (12, "12 hours"), (24, "24 hours"), (48, "48 hours"), (168, "A week")];
const DAILY: &[(u32, &str)] = &[(0, "None"), (7, "7 days"), (14, "14 days"), (30, "30 days"), (90, "90 days")];
const WEEKLY: &[(u32, &str)] = &[(0, "None"), (4, "4 weeks"), (8, "8 weeks"), (12, "12 weeks"), (26, "26 weeks"), (52, "52 weeks")];
const MONTHLY: &[(u32, &str)] = &[(0, "None"), (6, "6 months"), (12, "12 months"), (24, "24 months"), (60, "5 years"), (u32::MAX, "Forever")];

/// The header's picture, points.
const ICON: f32 = 48.0;
/// A label column, points.
const DIVIDER: f32 = 140.0;

pub struct MainWindow {
    app: Entity<Disk>,
    tab: usize,
    patterns: Entity<TextField>,
    /// The Backups tab's list (several may be chosen, to delete).
    list: Entity<ListModel>,
    _sub: gpui::Subscription,
    _observe: gpui::Subscription,
}

impl MainWindow {
    pub fn new(app: Entity<Disk>, _window: &mut Window, cx: &mut Context<Self>) -> MainWindow {
        let now = app.read(cx).settings.excludes.join(", ");
        let patterns = cx.new(|cx| {
            let mut f = TextField::new(cx).hint("~/Videos/raw, **/*.iso");
            f.set_text(now, cx);
            f
        });
        let to = app.clone();
        let sub = cx.subscribe(&patterns, move |_, field, e: &TextFieldEvent, cx| {
            if !matches!(e, TextFieldEvent::Changed) {
                return;
            }
            let text = field.read(cx).text().to_owned();
            to.update(cx, |d, cx| d.change(|s| s.excludes = text.split([',', '\n']).map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()).collect(), cx));
        });
        let observe = cx.observe(&app, |w: &mut MainWindow, app, cx| {
            // (On Destination: where backups go, looked at once it changes.)
            if w.tab == DESTINATION {
                app.update(cx, |d, _| d.look_at_target(false));
            }
            // (Put back by Undo: the field says so. What's being typed
            // stays as typed while it means the same.)
            let now = app.read(cx).settings.excludes.clone();
            let typed: Vec<String> = w.patterns.read(cx).text().split([',', '\n']).map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()).collect();
            if typed != now {
                w.patterns.update(cx, |f, cx| f.set_text(now.join(", "), cx));
            }
            cx.notify();
        });
        MainWindow { app, tab: 0, patterns, list: ListModel::new(cx, true), _sub: sub, _observe: observe }
    }
}

// ---- words for times, sizes and amounts ---------------------------------

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// "just now", "12 minutes ago", "3 hours ago", "yesterday at 14:05",
/// "on Tue at 14:05", "on 3 Nov".
pub fn ago(t: i64) -> String {
    use chrono::{Local, TimeZone};
    let d = (now() - t).max(0);
    match d {
        0..60 => "just now".into(),
        60..3600 => format!("{} ago", minutes(d / 60)),
        3600..43_200 => format!("{} ago", if d / 3600 == 1 { "an hour".into() } else { format!("{} hours", d / 3600) }),
        _ => {
            let Some(at) = Local.timestamp_opt(t, 0).single() else { return String::new() };
            let days = (Local::now().date_naive() - at.date_naive()).num_days();
            match days {
                0 => format!("today at {}", at.format("%H:%M")),
                1 => format!("yesterday at {}", at.format("%H:%M")),
                2..7 => format!("on {} at {}", at.format("%a"), at.format("%H:%M")),
                _ => format!("on {}", at.format("%-d %b %Y")),
            }
        }
    }
}

/// "in a minute", "in 48 minutes", "at 15:00", "tomorrow at 03:00".
pub fn until(t: i64) -> String {
    use chrono::{Local, TimeZone};
    let d = t - now();
    match d {
        ..=60 => "in a minute".into(),
        61..3600 => format!("in {}", minutes((d + 59) / 60)),
        _ => {
            let Some(at) = Local.timestamp_opt(t, 0).single() else { return String::new() };
            if at.date_naive() == Local::now().date_naive() { format!("at {}", at.format("%H:%M")) } else { format!("tomorrow at {}", at.format("%H:%M")) }
        }
    }
}

fn minutes(n: i64) -> String {
    if n == 1 { "a minute".into() } else { format!("{n} minutes") }
}

/// "12 seconds", "4 minutes", "1 hour 5 minutes".
pub fn took(secs: f64) -> String {
    let s = secs.round() as i64;
    match s {
        ..1 => "under a second".into(),
        1 => "1 second".into(),
        2..60 => format!("{s} seconds"),
        60..3600 => minutes((s + 30) / 60),
        _ => {
            let (h, m) = (s / 3600, (s % 3600 + 30) / 60);
            let h = if h == 1 { "1 hour".to_owned() } else { format!("{h} hours") };
            if m == 0 { h } else { format!("{h} {}", minutes(m)) }
        }
    }
}

/// What's kept, said plainly: "Every backup from the last hour, then one
/// an hour for 24 hours, one a day for 14 days, one a week for 8 weeks and
/// one a month for 24 months."
pub fn keep_words(k: &olo_protocol::diskbackup::Keep) -> String {
    let mut parts = Vec::new();
    let mut add = |n: u32, one: &str, unit: &str| {
        if n == u32::MAX {
            parts.push(format!("one {one} for ever"));
        } else if n > 0 {
            parts.push(format!("one {one} for {n} {unit}{}", if n == 1 { "" } else { "s" }));
        }
    };
    add(k.hourly, "an hour", "hour");
    add(k.daily, "a day", "day");
    add(k.weekly, "a week", "week");
    add(k.monthly, "a month", "month");
    match parts.len() {
        0 => "Every backup from the last hour; older ones go.".into(),
        1 => format!("Every backup from the last hour, then {}.", parts[0]),
        n => format!("Every backup from the last hour, then {} and {}.", parts[..n - 1].join(", "), parts[n - 1]),
    }
}

/// Folder `p` (as the settings keep it) as the window says it.
/// `button`, with a tip saying more of what it does.
fn tipped(id: &'static str, tip: &'static str, button: Button) -> gpui::AnyElement {
    div().id(id).tooltip(widgets::tooltip(tip)).child(button).into_any_element()
}

/// `t` with its first letter a capital.
fn capital(t: &str) -> String {
    let mut c = t.chars();
    c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
}

fn folder_name(p: &str) -> String {
    if p == "~" { "Your home folder".into() } else { p.to_owned() }
}

/// What a backup that didn't finish got through, in a line.
pub fn unfinished_words(u: &olo_protocol::diskbackup::Unfinished) -> String {
    if u.files == 0 {
        format!("Unfinished: {} sent before it stopped; the next backup goes on from there.", size(u.sent))
    } else {
        format!("Unfinished: {} ({}) done and {} sent; the next backup goes on from there.", plural(u.files, "file"), size(u.bytes), size(u.sent))
    }
}

/// A backup's check, in a list's cell.
pub fn check_cell(c: Option<&Check>) -> &'static str {
    match c {
        None => "—",
        Some(c) if c.error.is_some() => "Not checked",
        Some(c) if !c.problems.is_empty() => "✗ Damaged",
        Some(c) if c.full => "✓ Full",
        Some(_) => "✓ Fast",
    }
}

/// A backup's check, said in a line.
pub fn check_words(c: Option<&Check>) -> String {
    let Some(c) = c else { return "Not checked yet: Validate reads it back and checks it’s whole.".into() };
    if let Some(e) = &c.error {
        return format!("Couldn’t be checked {}: {e}.", ago(c.time));
    }
    if !c.problems.is_empty() {
        let mut t = format!("Damaged (found {}): {}.", ago(c.time), c.problems.join("; "));
        if !c.files.is_empty() {
            let more = c.files_affected.saturating_sub(c.files.len() as u64);
            let home = target::home().display().to_string();
            let named: Vec<String> = c.files.iter().map(|f| f.strip_prefix(&home).map_or(f.clone(), |r| format!("~{r}"))).collect();
            t.push_str(&format!(" Can’t come back as it was: {}{}.", named.join(", "), if more > 0 { format!(" and {more} more") } else { String::new() }));
        }
        t.push_str(" The next backup sends again what it can.");
        return t;
    }
    if c.full {
        return format!("Validated fully {}: all it needs ({}) read back, and whole.", ago(c.time), size(c.bytes_read));
    }
    let mut how = Vec::new();
    if c.by_checksum > 0 {
        how.push(format!("{} as sent, by {} checksums", plural(c.by_checksum, "pack"), if c.by_checksum == 1 { "its" } else { "their" }));
    }
    if c.sampled > 0 {
        how.push(format!("{} read back", plural(c.sampled, "sample")));
    }
    let how = if how.is_empty() { String::new() } else { format!(" ({})", how.join("; ")) };
    format!("Validated {}: everything it needs is there{how}. Validate Fully reads it all back.", ago(c.time))
}

// ---- the header: whether your files are safe -----------------------------

/// The headline, what's under it, and whether it's bad news.
pub fn headline(st: &State, s: &Settings) -> (String, String, bool) {
    let to = format!("To {}", s.target.describe());
    if st.pid == 0 {
        return ("DiskBackup isn’t running".into(), "Its service starts with your session: log out and in again, or ask Olo Doctor to start it.".into(), true);
    }
    let progress = st.progress.clone().unwrap_or_default();
    // (Under way, with the network gone: it waits, and goes on.)
    let what = match &st.doing {
        Doing::BackingUp => Some("The backup"),
        Doing::Validating => Some("Validating"),
        Doing::Pruning | Doing::Deleting { .. } => Some("Tidying"),
        Doing::Erasing => Some("Erasing"),
        _ => None,
    };
    if let Some(what) = what.filter(|_| progress.offline) {
        // (Where it goes is in Destination: the line has room for this.)
        return ("Waiting for the network".into(), format!("{what} goes on by itself once it’s back, from where it got to."), false);
    }
    match &st.doing {
        Doing::BackingUp => {
            let title = match progress.stage {
                Stage::Counting => "Getting ready: counting what to back up",
                Stage::Reading if progress.resuming => "Resuming the backup",
                // ("Backing up your home folder": what, said.)
                Stage::Reading => return reading_headline(&progress, s),
                Stage::Saving => "Finishing the backup",
                Stage::Checking => return ("Checking the backup".into(), format!("Everything it needs looked for, in {}.", s.target.describe()), false),
                Stage::Stopping => return ("Stopping the backup".into(), "Finishing what’s being sent, then keeping what’s done.".into(), false),
            };
            // (Reading: the file it's at, under the title; where it goes
            // is in Destination.)
            if progress.stage == Stage::Reading && !progress.current.is_empty() {
                let home = target::home().display().to_string();
                let current = match progress.current.strip_prefix(&home) {
                    Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
                    _ => progress.current.clone(),
                };
                return (title.into(), format!("Now: {current}"), false);
            }
            return (title.into(), format!("{to}."), false);
        }
        Doing::Pruning => return ("Tidying old backups".into(), format!("Freeing the space only backups past keeping took, in {}.", s.target.describe()), false),
        Doing::Deleting { what } => {
            let one = what == "the unfinished backup" || what == "a backup";
            let title = if what == "the unfinished backup" {
                "Deleting the unfinished backup"
            } else if one {
                "Deleting a backup"
            } else {
                "Deleting backups"
            };
            return (title.into(), format!("Then the space only {} took is freed, in {}.", if one { "it" } else { "they" }, s.target.describe()), false);
        }
        Doing::Restoring => return ("Restoring".into(), to, false),
        Doing::Erasing => return ("Erasing every backup".into(), format!("{to}. The next backup starts afresh."), false),
        Doing::Validating => return ("Validating backups".into(), format!("Every byte read back and checked, from {}.", s.target.describe()), false),
        Doing::Waiting { why } => {
            let mut w = why.clone();
            if let Some(c) = w.get(..1) {
                w.replace_range(..1, &c.to_uppercase());
            }
            return ("Waiting".into(), format!("{w}. {to}."), false);
        }
        Doing::Idle => {}
    }
    let next = match st.next {
        Some(n) if s.automatic => format!("Next {}.", until(n)),
        _ => "Next when you click Back Up Now.".into(),
    };
    match (&st.last, st.last_ok) {
        (Some(l), _) if l.error.is_none() => (format!("Backed up {}", ago(l.time)), format!("{to}. {next}"), now() - l.time > 3 * 24 * 3600),
        // (Stopped: the person's doing, not trouble, unless nothing whole
        // is recent.)
        (Some(l), ok) if l.stopped() => {
            let whole = ok.map_or("No backup has finished yet".to_owned(), |t| format!("The last whole one was {}", ago(t)));
            ("Backup stopped".into(), format!("You stopped it {}. {whole}. {next}", ago(l.time)), ok.is_some_and(|t| now() - t > 3 * 24 * 3600))
        }
        (Some(l), ok) => {
            let whole = ok.map_or("No backup has finished yet".to_owned(), |t| format!("The last whole one was {}", ago(t)));
            (format!("The backup {} didn’t finish", ago(l.time)), format!("{}. {whole}. {next}", l.error.clone().unwrap_or_default()), true)
        }
        (None, _) => ("Not backed up yet".into(), format!("{to}. {}", if s.automatic { "The first backup starts in a moment." } else { "Click Back Up Now to make the first." }), false),
    }
}

/// Backing up, reading: what (your home folder), and the file it's at.
fn reading_headline(p: &Progress, s: &Settings) -> (String, String, bool) {
    let title = format!("Backing up {}", olo_protocol::diskbackup::words::what(s));
    if p.current.is_empty() {
        return (title, format!("To {}.", s.target.describe()), false);
    }
    let home = target::home().display().to_string();
    let current = match p.current.strip_prefix(&home) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
        _ => p.current.clone(),
    };
    (title, format!("Now: {current}"), false)
}

/// Something to see to, in a line: a small mark, what's wrong, and the
/// button that sees to it, on the banners' tint (a `Banner` is for news;
/// this is a reminder).
fn notice(text: impl Into<SharedString>, button: Button) -> impl IntoElement {
    const MARK: f32 = 16.0;
    let back = canvas(|_, _, _| {}, |b, _, window, cx| widgets::banner_back(window, b, Some(Tone::Failure), &theme::palette(cx))).absolute().inset_0();
    let mark = canvas(|_, _, _| {}, |b, _, window, cx| widgets::verdict_mark(window, b, Tone::Failure, MARK, &theme::palette(cx))).w(px(MARK)).h(px(MARK)).flex_shrink_0();
    div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::SPACING))
        .px(px(m::SPACING))
        .py(px(m::TIGHT + 2.0))
        .child(back)
        .child(mark)
        .child(div().flex_1().min_w_0().child(StringView::new(text.into()).wrap(true)))
        .child(button)
}

/// A picture `name`, `size` points square, crisp.
fn picture(name: &'static str, size: f32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let at = style::snap_centred(window, b.center(), size, size);
            olo_ui::icons::paint(window, at, name);
        },
    )
    .w(px(size))
    .h(px(size))
    .flex_shrink_0()
}

impl Render for MainWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let d = self.app.read(cx);
        let (s, st) = (d.settings.clone(), d.state.clone());
        let set_up = s.target != Target::None;
        let running = busy_with(&st);
        let (note, busy, edit) = (d.note.clone(), d.busy, d.edit_menu());
        let app = self.app.clone();
        let bar_app = self.app.clone();
        let bar = MenuBar::new(
            vec![
                Menu::new("File")
                    .item("Back Up Now", "back-up-now")
                    .key("b")
                    .enabled(set_up && !running)
                    .item("Stop", "stop")
                    .enabled(running)
                    .item("Restore…", "restore")
                    .key("r")
                    .enabled(set_up)
                    .separator()
                    .item("Erase All Backups…", "erase-all")
                    .enabled(set_up && !running)
                    .separator()
                    .item("Close", "close")
                    .key("w")
                    .item("Quit", "quit")
                    .key("q"),
                edit,
            ],
            move |c, window, cx| match c {
                "close" => window.remove_window(),
                _ => bar_app.update(cx, |d, cx| d.command(c, cx)),
            },
        );
        let p = theme::palette(cx);
        let mut body = div().flex_1().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET));
        if !set_up {
            body = body.child(self.welcome(cx));
        } else {
            body = body.child(self.header(&s, &st, cx));
            if !s.key_in_passwords {
                let keep = app.clone();
                body = body.child(notice(
                    "The key is only on this computer: keep it in Passwords, to restore anywhere else.",
                    Button::new("Keep Key in Passwords").enabled(!busy).on_click(move |_, cx| keep.update(cx, |d, cx| d.command("keep-key", cx))),
                ));
            }
            // The latest backup found damaged when checked (not while one
            // runs: that's what sends again what's lost).
            if !running && let Some(c) = st.last.as_ref().and_then(|l| l.check.as_ref()).filter(|c| !c.problems.is_empty()) {
                let again = app.clone();
                body = body.child(notice(
                    format!("The last backup isn’t whole: {}. The next backup sends again what it can.", c.problems.join("; ")),
                    Button::new("Back Up Now").id("damage-back-up").enabled(!running).on_click(move |_, cx| again.update(cx, |d, cx| d.command("back-up-now", cx))),
                ));
            }
            let items: Vec<gpui::AnyElement> = match self.tab {
                0 => self.schedule(&s, cx),
                1 => self.files(&s, &st, cx),
                2 => self.backups(&s, &st, cx),
                3 => self.speed(&s, cx),
                _ => self.destination(&s, &st, cx),
            };
            let me = cx.entity();
            let tabs: Vec<SharedString> = TABS.into_iter().map(SharedString::from).collect();
            // (In a column of its own that takes the rest: a growing tab
            // view is at least as tall as what holds it.)
            body = body.child(
                div().flex_1().flex().flex_col().child(
                    TabView::new("diskbackup-tabs", tabs, self.tab)
                        .grows()
                        .on_select(move |i, _, cx| {
                            me.update(cx, |w, cx| {
                                w.tab = i;
                                // (Looked at again each time it's chosen.)
                                if i == DESTINATION {
                                    w.app.update(cx, |d, _| d.look_at_target(true));
                                }
                                cx.notify();
                            })
                        })
                        .child(div().flex_1().flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING)).children(items)),
                ),
            );
        }
        if let Some((text, error)) = note {
            body = body.child(StringView::new(text).wrap(true).color(if error { style::c(p.failure) } else { style::disabled_text(&p) }));
        }
        div().size_full().flex().flex_col().child(bar).child(body)
    }
}

impl MainWindow {
    fn command(&self, c: &'static str) -> impl Fn(&mut Window, &mut gpui::App) + 'static {
        let app = self.app.clone();
        move |_, cx| app.update(cx, |d, cx| d.command(c, cx))
    }

    /// Not set up: the one question, where backups go.
    fn welcome(&self, cx: &mut Context<Self>) -> gpui::Div {
        let busy = self.app.read(cx).busy;
        let dim = |t: &'static str| StringView::new(t).wrap(true).dim(true);
        // (Side by side; their words as long as each other's, so their
        // buttons line up.)
        let choice = |icon: &'static str, title: &'static str, about: &'static str, button: Button| {
            BBox::new(title)
                .child(div().flex().flex_row().items_start().gap(px(m::SPACING)).child(picture(icon, 32.0)).child(div().flex_1().min_w_0().child(dim(about))))
                .child(div().flex().flex_row().justify_end().child(button))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(
                div().flex().flex_row().items_center().gap(px(m::SPACING * 2.0)).child(picture("diskbackup", ICON)).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(m::TIGHT))
                        .child(StringView::new("Back up this computer").bold(true).size(style::TITLE_SIZE).height(style::line_row(style::TITLE_SIZE)))
                        .child(
                            StringView::new("DiskBackup copies your files, encrypted, every hour, and keeps older copies too, so any file can be brought back as it was. Where should the backups go?")
                                .wrap(true),
                        ),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(m::SPACING))
                    .child(div().flex_1().min_w_0().child(choice(
                        "disk",
                        "A Disk or Folder",
                        "A disk of its own, or a folder on one: your backups kept apart from this computer.",
                        Button::new("Choose a Folder…").enabled(!busy).on_click(self.command("choose-folder")),
                    )))
                    .child(div().flex_1().min_w_0().child(choice(
                        "diskbackup",
                        "Google Drive",
                        "Your Google Drive keeps them anywhere: sign in once, or let an agent set it up.",
                        Button::new("Connect Google Drive…").enabled(!busy).on_click(self.command("google-drive")),
                    ))),
            )
            .child(dim("Your home folder is backed up, every hour, leaving out what can be downloaded or built again. Change any of it once it’s set up."))
    }

    /// Set up: how it stands, and Back Up Now (or Stop).
    fn header(&self, s: &Settings, st: &State, cx: &mut Context<Self>) -> gpui::Div {
        let p = theme::palette(cx);
        let (title, detail, bad) = headline(st, s);
        let running = busy_with(st);
        let mut buttons = div().flex().flex_row().gap(px(m::SPACING)).flex_shrink_0();
        buttons = buttons.child(Button::new("Restore…").on_click(self.command("restore")));
        let stopping = st.progress.as_ref().is_some_and(|p| p.stage == Stage::Stopping);
        buttons = buttons.child(if running {
            Button::new("Stop").enabled(!stopping).on_click(self.command("stop"))
        } else {
            Button::new("Back Up Now").default(true).enabled(st.pid != 0).on_click(self.command("back-up-now"))
        });
        let mut words = div().flex_1().min_w_0().flex().flex_col().gap(px(m::TIGHT));
        let mut t = StringView::new(title).bold(true).size(style::TITLE_SIZE).height(style::line_row(style::TITLE_SIZE));
        if bad {
            t = t.color(style::c(p.failure));
        }
        // (While it runs, the file it's at: one line, whatever its length.)
        let detail = if running { StringView::new(detail).cut(true).height(style::line_row(style::PLAIN_SIZE)) } else { StringView::new(detail).wrap(true) };
        words = words.child(t).child(detail);
        // While it runs, the whole of it (under, the window's width); else
        // the last one, in a line.
        let progress = if running && let Some(pr) = &st.progress { Some(self.progress(s, st, pr)) } else { None };
        if st.doing == Doing::Idle
            && let Some(l) = st.last.as_ref().filter(|l| l.error.is_none())
        {
            let mut said = format!("Last: {}, {} new or changed, {} new, took {}.", plural(l.files, "file"), count(l.new_files + l.changed_files), size(l.new_bytes), took(l.seconds));
            if l.unreadable > 0 {
                said.push_str(&format!(" {} couldn’t be read.", plural(l.unreadable, "file")));
            }
            match &l.check {
                Some(c) if c.whole() => said.push_str(" Validated: all there."),
                Some(Check { error: Some(e), .. }) => said.push_str(&format!(" Not checked ({e}).")),
                _ => {}
            }
            words = words.child(StringView::new(said).wrap(true).dim(true));
        }
        // What a backup that didn't finish got through.
        if st.doing == Doing::Idle
            && let Some(u) = &st.unfinished
        {
            words = words.child(StringView::new(unfinished_words(u)).wrap(true).dim(true));
        }
        let row = div().flex().flex_row().items_start().gap(px(m::SPACING * 2.0)).child(picture("diskbackup", ICON)).child(words).child(buttons);
        div().flex().flex_col().gap(px(m::TIGHT)).child(row).children(progress)
    }

    /// A backup (or a tidy, or erasing) under way: the whole of it.
    fn progress(&self, s: &Settings, st: &State, pr: &Progress) -> gpui::Div {
        let dim = |t: String| StringView::new(t).wrap(true).dim(true);
        let line = |t: String| StringView::new(t).wrap(true);
        let mut col = div().flex().flex_col().gap(px(m::TIGHT));
        match (&st.doing, pr.stage) {
            (Doing::BackingUp, Stage::Counting) => {
                col = col.child(line(format!("Counting what there is to back up: {}, {} so far.", plural(pr.total_files, "file"), size(pr.total_bytes))));
                col = col.child(dim("The first backup reads everything; later ones read only what changed, in a fraction of the time.".into()));
            }
            (Doing::BackingUp | Doing::Validating, Stage::Checking) | (Doing::Validating, _) => {
                // (The bar throughout, so nothing moves: empty till it knows
                // how much there is to read back.)
                let done = if pr.total_bytes == 0 { 0.0 } else { (pr.bytes as f32 / pr.total_bytes as f32).min(1.0) };
                let label = if pr.total_bytes == 0 { "Looking at what’s there".to_owned() } else { format!("{} of {} read back", size(pr.bytes), size(pr.total_bytes)) };
                col = col.child(StatusBar::new(done).label(label).trailing(format!("{:.0}%", done * 100.0)));
                col = col.child(line(if pr.current.is_empty() { "Looking at what’s there…".into() } else { format!("{}…", pr.current) }));
            }
            // Stopping: the pack being sent, finished first (then what's
            // done is kept).
            (Doing::BackingUp, Stage::Stopping) => {
                let (bar, label, then) = match pr.sending_left() {
                    Some((done, left)) => (
                        done as f32 / pr.sending as f32,
                        format!("Finishing sending a pack: {} of {}", size(done), size(pr.sending)),
                        format!("Then what’s done is kept, so the next backup goes on from there{}.", left.map(|l| format!(" ({})", olo_protocol::diskbackup::words::left(l))).unwrap_or_default()),
                    ),
                    None => (1.0, "Keeping what’s done…".to_owned(), "So the next backup goes on from there.".to_owned()),
                };
                col = col.child(StatusBar::new(bar.min(1.0)).label(label).trailing(format!("{:.0}%", bar.min(1.0) * 100.0)));
                col = col.child(line(format!("{}, {} new to the backup, {} sent so far.", pr.of_files(), size(pr.new_bytes), size(pr.uploaded))));
                col = col.child(dim(then));
            }
            (Doing::BackingUp, stage) => {
                let (done, left) = pr.how_far(now());
                let saving = stage == Stage::Saving;
                // How far through all of it (what's unchanged since the
                // last backup is gone through without being read again).
                let label = if saving { "Writing the backup’s lists…".to_owned() } else { pr.gone_through() };
                col = col.child(StatusBar::new(if saving { 1.0 } else { done }).label(label).trailing(format!("{:.0}%", if saving { 100.0 } else { done * 100.0 })));
                let mut words = pr.of_files();
                if let Some(l) = left.filter(|_| !saving) {
                    words.push_str(&format!(" · {}", olo_protocol::diskbackup::words::left(l)));
                }
                col = col.child(line(words));
                // What the backup didn't have yet, and that sent.
                let spent = (now() - pr.started).max(1) as f64;
                let rate = pr.uploaded as f64 / spent;
                let limit = if s.upload_kbs > 0 { format!(", at most {}", speed_label(s.upload_kbs)) } else { String::new() };
                col = col.child(dim(format!("{} new to the backup · {} sent, compressed and encrypted ({}/s{limit})", size(pr.new_bytes), size(pr.uploaded), size(rate as u64))));
                // Each folder's turn (they're gone through in order).
                if s.paths.len() > 1 {
                    let mut paths: Vec<(String, std::path::PathBuf)> = s.paths.iter().map(|p| (folder_name(p), target::expand(p))).collect();
                    paths.sort_by(|a, b| a.1.cmp(&b.1));
                    let now_at = paths.iter().rposition(|(_, p)| std::path::Path::new(&pr.current).starts_with(p));
                    let marks: Vec<String> = paths
                        .iter()
                        .enumerate()
                        .map(|(i, (name, _))| match now_at {
                            Some(n) if i < n => format!("✓ {name}"),
                            Some(n) if i == n => format!("▸ {name}"),
                            _ => format!("○ {name}"),
                        })
                        .collect();
                    col = col.child(dim(format!("Folders: {}", marks.join("   "))));
                }
            }
            (Doing::Erasing, _) => {
                let done = if pr.total_files == 0 { 0.0 } else { pr.files as f32 / pr.total_files as f32 };
                // (Google Drive: its folder deleted whole, one step.)
                let label = if pr.total_files == 0 {
                    format!("{}…", if pr.current.is_empty() { "Getting ready" } else { &pr.current })
                } else {
                    format!("Deleting {} of {}", count(pr.files), count(pr.total_files))
                };
                col = col.child(StatusBar::new(done).label(label).trailing(format!("{:.0}%", done * 100.0)));
            }
            // Tidying, or deleting (a tidy after): the step it's on, how
            // many of how many, and (deleting what isn't used) what's freed.
            (Doing::Pruning | Doing::Deleting { .. }, _) => {
                let what = if pr.current.is_empty() { "Getting ready".to_owned() } else { pr.current.clone() };
                let done = if pr.total_files == 0 { 0.0 } else { (pr.files as f32 / pr.total_files as f32).min(1.0) };
                let label = match (pr.total_files, pr.files) {
                    (0, 0) => format!("{what}…"),
                    (0, n) => format!("{what}: {}", count(n)),
                    (t, n) => format!("{what}: {} of {}", count(n), count(t)),
                };
                col = col.child(StatusBar::new(done).label(label).trailing(if pr.total_files == 0 { String::new() } else { format!("{:.0}%", done * 100.0) }));
                let step = if pr.steps > 0 { format!("Step {} of {}", pr.step.max(1), pr.steps) } else { "Starting".into() };
                let freed = if pr.total_bytes > 0 { format!(" · {} of {} freed", size(pr.bytes), size(pr.total_bytes)) } else { String::new() };
                let spent = took((now() - pr.started).max(0) as f64);
                col = col.child(dim(format!("{step}{freed} · {spent} so far")));
            }
            _ => {}
        }
        col.pt(px(m::TIGHT))
    }

    // ---- the tabs -------------------------------------------------------

    fn schedule(&self, s: &Settings, _cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let app = self.app.clone();
        let set = move |f: fn(&mut Settings)| {
            let app = app.clone();
            move |_: &mut Window, cx: &mut gpui::App| app.update(cx, |d, cx| d.change(f, cx))
        };
        let every_at = EVERY.iter().position(|(m, _)| *m == s.every_minutes);
        let mut names: Vec<SharedString> = EVERY.iter().map(|(_, n)| SharedString::from(*n)).collect();
        if every_at.is_none() {
            names.push(format!("{} minutes", s.every_minutes).into());
        }
        let every_app = self.app.clone();
        let every = MenuField::new("every", names, every_at.unwrap_or(EVERY.len())).enabled(s.automatic).on_change(move |i, _, cx| {
            if let Some((m, _)) = EVERY.get(i) {
                let m = *m;
                every_app.update(cx, |d, cx| d.change(|s| s.every_minutes = m, cx));
            }
        });
        let battery_app = self.app.clone();
        let when = BBox::new("When to Back Up")
            .child(div().flex().flex_row().items_center().gap(px(m::SPACING)).child(RadioButton::new("Automatically, every", s.automatic).on_select(set(|s| s.automatic = true))).child(every))
            .child(RadioButton::new("Only when I click Back Up Now", !s.automatic).on_select(set(|s| s.automatic = false)))
            .child(
                div()
                    .pt(px(m::TIGHT))
                    .child(CheckBox::new("Also when running on battery", s.on_battery).on_toggle(move |on, _, cx| battery_app.update(cx, |d, cx| d.change(|s| s.on_battery = on, cx)))),
            );
        let keep_menu = |id: &'static str, label: &'static str, choices: &'static [(u32, &'static str)], now: u32, set: fn(&mut Settings, u32)| {
            let at = choices.iter().position(|(n, _)| *n == now);
            let mut names: Vec<SharedString> = choices.iter().map(|(_, name)| SharedString::from(*name)).collect();
            if at.is_none() {
                names.push(now.to_string().into());
            }
            let app = self.app.clone();
            MenuField::new(id, names, at.unwrap_or(choices.len())).label(label).divider(DIVIDER).on_change(move |i, _, cx| {
                if let Some((n, _)) = choices.get(i) {
                    let n = *n;
                    app.update(cx, |d, cx| d.change(|s| set(s, n), cx));
                }
            })
        };
        let k = s.keep;
        let keep = BBox::new("Backups Kept")
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING * 2.0))
                    .child(div().flex_1().flex().flex_col().gap(px(m::SPACING)).child(keep_menu("hourly", "One an hour for:", HOURLY, k.hourly, |s, n| s.keep.hourly = n)).child(keep_menu(
                        "daily",
                        "One a day for:",
                        DAILY,
                        k.daily,
                        |s, n| s.keep.daily = n,
                    )))
                    .child(div().flex_1().flex().flex_col().gap(px(m::SPACING)).child(keep_menu("weekly", "One a week for:", WEEKLY, k.weekly, |s, n| s.keep.weekly = n)).child(keep_menu(
                        "monthly",
                        "One a month for:",
                        MONTHLY,
                        k.monthly,
                        |s, n| s.keep.monthly = n,
                    ))),
            )
            .child(StringView::new(keep_words(&k)).wrap(true).dim(true))
            .child(StringView::new("Older ones go, and the space only they took is freed, once a day. Delete any yourself under Backups.").wrap(true).dim(true));
        vec![when.into_any_element(), keep.into_any_element()]
    }

    fn files(&self, s: &Settings, st: &State, _cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let mut what = BBox::new("Back Up These Folders");
        let n = s.paths.len();
        for (i, path) in s.paths.iter().enumerate() {
            let app = self.app.clone();
            let shown = folder_name(path);
            let sub = if path == "~" { target::home().display().to_string() } else { String::new() };
            what = what.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(m::SPACING))
                    .child(picture("folder", 16.0))
                    .child(div().flex_1().min_w_0().flex().flex_row().gap(px(m::SPACING)).child(StringView::new(shown)).child(StringView::new(sub).dim(true)))
                    .child(Button::new("Remove").id(format!("remove-{i}")).enabled(n > 1).on_click(move |_, cx| {
                        app.update(cx, |d, cx| {
                            d.change(
                                |s| {
                                    if i < s.paths.len() {
                                        s.paths.remove(i);
                                    }
                                },
                                cx,
                            )
                        })
                    })),
            );
        }
        what = what.child(
            div().flex().flex_row().items_center().gap(px(m::SPACING)).pt(px(m::TIGHT)).child(Button::new("Add a Folder…").on_click(self.command("add-folder"))).child(
                StringView::new(match &st.last {
                    Some(l) if l.error.is_none() => format!("{} in {}, by the last backup.", size(l.bytes), plural(l.files, "file")),
                    _ => String::new(),
                })
                .dim(true),
            ),
        );
        // What can be made again: two columns, what each means in its tip.
        let chosen = s.exclude_groups.clone().unwrap_or_else(olo_diskbackup::exclude::default_groups);
        let check = |g: &'static olo_diskbackup::exclude::Group| {
            let app = self.app.clone();
            let on = chosen.iter().any(|c| c == g.id);
            let id = g.id;
            div().id(SharedString::from(format!("leave-{id}"))).tooltip(widgets::tooltip(g.about)).child(CheckBox::new(g.name, on).on_toggle(move |on, _, cx| {
                app.update(cx, |d, cx| {
                    d.change(
                        |s| {
                            let mut now = s.exclude_groups.clone().unwrap_or_else(olo_diskbackup::exclude::default_groups);
                            now.retain(|g| g != id);
                            if on {
                                now.push(id.to_owned());
                            }
                            s.exclude_groups = Some(now);
                        },
                        cx,
                    )
                })
            }))
        };
        let half = GROUPS.len().div_ceil(2);
        let left = div().flex_1().min_w_0().flex().flex_col().gap(px(m::TIGHT)).children(GROUPS[..half].iter().map(check));
        let right = div().flex_1().min_w_0().flex().flex_col().gap(px(m::TIGHT)).children(GROUPS[half..].iter().map(check));
        // (What each covers: in its tip; how patterns go, in the field's
        // (and by its example, the placeholder).)
        let leave = BBox::new("Leave Out What Can Be Downloaded or Built Again").child(div().flex().flex_row().gap(px(m::SPACING * 2.0)).child(left).child(right)).child(
            div()
                .id("leave-patterns")
                .pt(px(m::SPACING))
                .tooltip(widgets::tooltip("Folders and files, separated by commas: ~/ is your home folder, **/ anywhere, * any name."))
                .child(TextControl::new(self.patterns.clone()).label("Also leave out:").divider(DIVIDER)),
        );
        vec![what.into_any_element(), leave.into_any_element()]
    }

    fn backups(&self, s: &Settings, st: &State, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let p = theme::palette(cx);
        let d = self.app.read(cx);
        let snaps: Vec<SnapshotInfo> = match &d.snapshots {
            Some(Ok(l)) => l.clone(),
            _ => Vec::new(),
        };
        // The unfinished one first (the next goes on from it), then each
        // kept, newest first.
        let unfinished = st.unfinished.clone();
        let first = usize::from(unfinished.is_some());
        let said = match &d.snapshots {
            None => "Asking DiskBackup’s service…".to_owned(),
            Some(Err(e)) => format!("The backups can’t be listed: {e}."),
            Some(Ok(l)) => {
                let mut t = if l.is_empty() {
                    "No backup has finished yet.".to_owned()
                } else {
                    let taken = st.size.map(|b| format!(", {} in all", size(b))).unwrap_or_default();
                    format!("{} kept in {}{taken}.", plural(l.len() as u64, "backup"), s.target.describe())
                };
                if unfinished.is_some() {
                    t.push_str(" The unfinished one is where the next goes on from: delete it to start over.");
                }
                t.push_str(" Choose any to validate or delete them.");
                t
            }
        };
        let running = busy_with(st);
        let (rows, u) = (snaps.clone(), unfinished.clone());
        let list = ColumnListView::new(
            "backups",
            &self.list,
            vec![Column::new("When", 160.0), Column::new("Files", 70.0), Column::new("Size", 75.0), Column::new("New", 75.0), Column::new("Took", 90.0), Column::new("Validated", 100.0)],
            snaps.len() + first,
            move |cell, window, cx| {
                let text = match (&u, cell.row.checked_sub(first)) {
                    (Some(u), None) => match cell.column {
                        0 => format!("Unfinished ({})", clock(u.time)),
                        1 if u.files == 0 => "—".into(),
                        1 => count(u.files),
                        2 if u.bytes == 0 => "—".into(),
                        2 => size(u.bytes),
                        3 => size(u.new_bytes),
                        4 if u.seconds == 0.0 => "—".into(),
                        4 => took(u.seconds),
                        _ => "—".into(),
                    },
                    (_, Some(i)) => {
                        let Some(b) = rows.get(i) else { return };
                        match cell.column {
                            // (How long ago, while that's the shorter.)
                            0 if now() - b.time < 12 * 3600 => format!("{} ({})", clock(b.time), ago(b.time)),
                            0 => clock(b.time),
                            1 => count(b.files),
                            2 => size(b.bytes),
                            3 => size(b.new_bytes),
                            4 => took(b.seconds),
                            _ => check_cell(b.check.as_ref()).into(),
                        }
                    }
                    _ => return,
                };
                cell.text(window, cx, text);
            },
        )
        .on_select(|_, cx| cx.refresh_windows());
        let selected: Vec<usize> = self.list.read(cx).state.selected.iter().copied().collect();
        let unfinished_chosen = first == 1 && selected.contains(&0);
        let chosen: Vec<String> = selected.iter().filter_map(|&i| i.checked_sub(first).and_then(|i| snaps.get(i)).map(|b| b.id.clone())).collect();
        let n = chosen.len() + usize::from(unfinished_chosen);
        // What the one chosen is, in a line (all of it in its tip).
        let (detail, bad) = match (selected.as_slice(), &unfinished) {
            ([0], Some(u)) => (unfinished_words(u), false),
            ([i], _) => match i.checked_sub(first).and_then(|i| snaps.get(i)) {
                Some(b) => (check_words(b.check.as_ref()), b.check.as_ref().is_some_and(|c| !c.problems.is_empty())),
                None => (String::new(), false),
            },
            ([], _) => ("Choose a backup to see how it was last checked.".into(), false),
            _ => (format!("{} chosen.", plural(n as u64, "backup")), false),
        };
        let mut detail = StringView::new(detail).cut(true).height(style::line_row(style::PLAIN_SIZE));
        detail = if bad { detail.color(style::c(p.failure)) } else { detail.dim(true) };
        let (del, check, check_fully) = (self.app.clone(), self.app.clone(), self.app.clone());
        let (to_check, to_check_fully) = (chosen.clone(), chosen.clone());
        let delete = Button::new(if n > 1 { format!("Delete {n} Backups…") } else { "Delete…".into() }).enabled(n > 0 && !running).on_click(move |_, cx| {
            let ids = chosen.clone();
            del.update(cx, |d, cx| d.delete(ids, unfinished_chosen, cx));
        });
        let can_check = !to_check.is_empty() && !running;
        let validate = tipped(
            "backups-validate",
            "Check the backups chosen are whole, quickly: everything they need there, the store’s checksums compared with what was sent, and a few samples read back (as after each backup). What’s damaged is repaired as far as it can be.",
            Button::new(if to_check.len() > 1 { format!("Validate {}", to_check.len()) } else { "Validate".into() }).enabled(can_check).on_click(move |_, cx| {
                let ids = to_check.clone();
                check.update(cx, |d, cx| d.validate(ids, false, cx));
            }),
        );
        let validate_fully = tipped(
            "backups-validate-fully",
            "Read every byte the backups chosen need back from where they’re kept, and check each piece, as a restore would",
            Button::new("Validate Fully…").enabled(can_check).on_click(move |_, cx| {
                let ids = to_check_fully.clone();
                check_fully.update(cx, |d, cx| d.validate(ids, true, cx));
            }),
        );
        let row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            .child(Button::new("Restore…").id("backups-restore").on_click(self.command("restore")))
            .child(validate)
            .child(validate_fully)
            .child(delete)
            .child(div().flex_1())
            .child(Button::new("Erase All Backups…").enabled(!running && !(snaps.is_empty() && unfinished.is_none())).on_click(self.command("erase-all")));
        let kept = BBox::new("Backups Kept").child(StringView::new(said).wrap(true).dim(true)).child(div().h(px(180.0)).child(list)).child(detail).child(row);
        vec![kept.into_any_element()]
    }

    fn speed(&self, s: &Settings, _cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let speed_menu = |id: &'static str, label: &'static str, now: u64, set: fn(&mut Settings, u64)| {
            let at = SPEEDS.iter().position(|k| *k == now);
            let mut names: Vec<SharedString> = SPEEDS.iter().map(|k| SharedString::from(speed_label(*k))).collect();
            if at.is_none() {
                names.push(speed_label(now).into());
            }
            let app = self.app.clone();
            MenuField::new(id, names, at.unwrap_or(SPEEDS.len())).label(label).divider(DIVIDER).on_change(move |i, _, cx| {
                if let Some(k) = SPEEDS.get(i) {
                    let k = *k;
                    app.update(cx, |d, cx| d.change(|s| set(s, k), cx));
                }
            })
        };
        let network = BBox::new("Network")
            .child(speed_menu("upload", "Upload at most:", s.upload_kbs, |s, k| s.upload_kbs = k))
            .child(speed_menu("download", "Download at most:", s.download_kbs, |s, k| s.download_kbs = k))
            .child(StringView::new("Uploading is backing up; downloading is restoring, and tidying. A limit holds at once, for a backup under way too.").wrap(true).dim(true));
        let names: Vec<SharedString> = Priority::ALL.iter().map(|p| SharedString::from(p.label())).collect();
        let at = Priority::ALL.iter().position(|p| *p == s.priority).unwrap_or(0);
        let app = self.app.clone();
        let about = match s.priority {
            Priority::Idle => "It takes only what nothing else wants, the processor’s time and the disk’s: never in your way, though slower while you work.",
            Priority::Low => "Below anything of yours, but steadily: you won’t notice it, and it finishes sooner than only when idle.",
            Priority::Normal => "As much as any program: done soonest, and you may notice it while it runs.",
        };
        let computer = BBox::new("This Computer")
            .child(MenuField::new("priority", names, at).label("Back up:").divider(DIVIDER).on_change(move |i, _, cx| {
                let p = Priority::ALL[i];
                app.update(cx, |d, cx| d.change(|s| s.priority = p, cx));
            }))
            .child(StringView::new(about).wrap(true).dim(true))
            .child(StringView::new("A backup reads one file at a time, and leaves what it read out of memory: it never takes much of either.").wrap(true).dim(true));
        vec![network.into_any_element(), computer.into_any_element()]
    }

    fn destination(&self, s: &Settings, st: &State, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let p = theme::palette(cx);
        let d = self.app.read(cx);
        let busy = d.busy;
        // As last looked at (None: looking).
        let looked = d.where_.as_ref().filter(|(t, _)| *t == s.target).and_then(|(_, r)| r.clone());
        let taken = st.size.map(|b| format!("The backups take {} there", size(b)));
        let space = |free: u64, total: Option<u64>| match total {
            Some(t) if t > 0 => format!("{} of {} free", size(free), size(t)),
            _ => "no limit to its space".to_owned(),
        };
        let and = |first: Option<String>, then: String| match first {
            Some(f) => format!("{f}; {then}."),
            None => format!("{}.", capital(&then)),
        };
        let (icon, title, detail, status, buttons): (&'static str, String, String, (String, bool), Vec<gpui::AnyElement>) = match &s.target {
            Target::GoogleDrive { account, .. } => {
                let status = match &looked {
                    None => ("Checking the connection…".to_owned(), false),
                    Some(Ok(Where::Drive(a))) => (format!("Connected. {}", and(taken.clone(), format!("your Drive has {}", space(a.limit.map_or(0, |l| l.saturating_sub(a.used)), a.limit)))), false),
                    Some(Ok(_)) => (String::new(), false),
                    Some(Err(e)) => (format!("Not connected: {e}."), true),
                };
                let mut buttons = Vec::new();
                // (Signed out by Google: sign in again, with the client from before.)
                if matches!(looked, Some(Err(_))) {
                    buttons.push(Button::new("Sign In Again…").enabled(!busy).on_click(self.command("google-drive")).into_any_element());
                }
                buttons.push(Button::new("Disconnect…").enabled(!busy).on_click(self.command("disconnect")).into_any_element());
                buttons.push(tipped(
                    "use-folder",
                    "Back up to a disk or folder from now on; the backups in Google Drive stay as they are",
                    Button::new("Use a Folder Instead…").enabled(!busy).on_click(self.command("choose-folder")),
                ));
                ("diskbackup", "Google Drive".to_owned(), format!("{account}, in the folder “{}”", olo_diskbackup::drive::FOLDER), status, buttons)
            }
            Target::Folder { path } => {
                let status = match &looked {
                    None => ("Looking at its disk…".to_owned(), false),
                    Some(Ok(Where::Disk { free, total })) => (and(taken.clone(), format!("its disk has {}", space(*free, Some(*total)))), false),
                    Some(Ok(_)) => (String::new(), false),
                    Some(Err(e)) => (format!("{}.", capital(e)), true),
                };
                let name = std::path::Path::new(path).file_name().map_or(path.clone(), |n| n.to_string_lossy().into_owned());
                let buttons = vec![
                    Button::new("Choose Another Folder…").enabled(!busy).on_click(self.command("choose-folder")).into_any_element(),
                    tipped(
                        "use-drive",
                        "Back up to your Google Drive from now on; the backups in this folder stay as they are",
                        Button::new("Use Google Drive Instead…").enabled(!busy).on_click(self.command("google-drive")),
                    ),
                ];
                ("disk", name, crate::app::tilde(std::path::Path::new(path)), status, buttons)
            }
            Target::None => ("disk", "Not set up".into(), String::new(), (String::new(), false), Vec::new()),
        };
        let open_label = if matches!(s.target, Target::GoogleDrive { .. }) { "Open in Google Drive" } else { "Show in Tracker" };
        let mut status_view = StringView::new(status.0).cut(true).height(style::line_row(style::PLAIN_SIZE));
        status_view = if status.1 { status_view.color(style::c(p.failure)) } else { status_view.dim(true) };
        let where_ = BBox::new("Where Backups Go")
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(m::SPACING))
                    .child(picture(icon, 32.0))
                    // (Three lines as tall as each other: the picture centred
                    // on the middle one.)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(StringView::new(title).bold(true).height(style::line_row(style::PLAIN_SIZE)))
                            .child(StringView::new(detail).cut(true).height(style::line_row(style::PLAIN_SIZE)))
                            .child(status_view),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING))
                    .pt(px(m::TIGHT))
                    .children(buttons)
                    .child(div().flex_1())
                    .child(Button::new(open_label).id("show-target").on_click(self.command("show-target"))),
            );
        let mut key = BBox::new("Key");
        if s.key_in_passwords {
            key = key.child(
                StringView::new("Your backups are encrypted before they leave this computer. Their key is kept in Passwords (“DiskBackup Key”): with it, they can be restored on any computer.")
                    .wrap(true)
                    .dim(true),
            );
        } else {
            key = key
                .child(
                    StringView::new("Your backups are encrypted before they leave this computer, with a key only this computer has. Keep it in Passwords too, or they can’t be read anywhere else.")
                        .wrap(true),
                )
                .child(div().pt(px(m::TIGHT)).child(Button::new("Keep Key in Passwords").id("key-keep").enabled(!busy).on_click(self.command("keep-key"))));
        }
        let app = self.app.clone();
        let deskbar = BBox::new("Deskbar").child(CheckBox::new("Show DiskBackup in Deskbar", s.in_deskbar).on_toggle(move |on, _, cx| app.update(cx, |d, cx| d.change(|s| s.in_deskbar = on, cx))));
        vec![where_.into_any_element(), key.into_any_element(), deskbar.into_any_element()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use olo_protocol::diskbackup::{Keep, Last};

    #[test]
    fn words_for_times() {
        let t = now();
        assert_eq!(ago(t - 20), "just now");
        assert_eq!(ago(t - 60), "a minute ago");
        assert_eq!(ago(t - 12 * 60), "12 minutes ago");
        assert_eq!(ago(t - 3600), "an hour ago");
        assert_eq!(until(t + 30), "in a minute");
        assert_eq!(until(t + 48 * 60), "in 48 minutes");
        assert_eq!(took(0.3), "under a second");
        assert_eq!(took(12.0), "12 seconds");
        assert_eq!(took(240.0), "4 minutes");
        assert_eq!(took(3900.0), "1 hour 5 minutes");
    }

    #[test]
    fn what_is_kept_said_plainly() {
        assert_eq!(keep_words(&Keep::default()), "Every backup from the last hour, then one an hour for 24 hours, one a day for 14 days, one a week for 8 weeks and one a month for 24 months.");
        assert_eq!(keep_words(&Keep { hourly: 0, daily: 7, weekly: 0, monthly: u32::MAX }), "Every backup from the last hour, then one a day for 7 days and one a month for ever.");
        assert_eq!(keep_words(&Keep { hourly: 0, daily: 0, weekly: 0, monthly: 0 }), "Every backup from the last hour; older ones go.");
    }

    #[test]
    fn how_far_and_how_long() {
        let t = now();
        let p = Progress { started: t - 100, bytes: 25, total_bytes: 100, ..Default::default() };
        let (done, left) = p.how_far(t);
        assert!((done - 0.25).abs() < 1e-6);
        assert_eq!(left, Some(300), "a quarter in 100 s: 300 s to go");
        // Past the plan (it grew): never done while it runs.
        let p = Progress { started: t - 100, bytes: 150, total_bytes: 100, ..Default::default() };
        assert!(p.how_far(t).0 <= 0.99);
        // Too soon to say.
        assert_eq!(Progress { started: t - 3, bytes: 1, total_bytes: 100, ..Default::default() }.how_far(t).1, None);
    }

    #[test]
    fn the_headline_says_how_it_stands() {
        let s = Settings { target: Target::Folder { path: "/backups".into() }, ..Settings::default() };
        let mut st = State { pid: 1, ..State::default() };
        assert_eq!(headline(&st, &s).0, "Not backed up yet");
        st.last = Some(Last { time: now() - 12 * 60, ..Default::default() });
        st.next = Some(now() + 48 * 60);
        let (title, detail, bad) = headline(&st, &s);
        assert_eq!(title, "Backed up 12 minutes ago");
        assert!(detail.contains("To /backups") && detail.contains("Next in 48 minutes"), "{detail}");
        assert!(!bad);
        let manual = Settings { automatic: false, ..s.clone() };
        assert!(headline(&st, &manual).1.contains("Next when you click Back Up Now"));
        st.last = Some(Last { time: now() - 60, error: Some("the disk is full".into()), ..Default::default() });
        let (title, detail, bad) = headline(&st, &s);
        assert!(bad && title.contains("didn’t finish") && detail.contains("the disk is full"), "{title} {detail}");
        // Stopped: calmly, unless nothing whole is recent.
        st.last = Some(Last { time: now() - 60, error: Some(olo_protocol::diskbackup::STOPPED.into()), ..Default::default() });
        st.last_ok = Some(now() - 3600);
        let (title, detail, bad) = headline(&st, &s);
        assert!(!bad && title == "Backup stopped" && detail.starts_with("You stopped it a minute ago. The last whole one was an hour ago."), "{title} {detail}");
        st.last_ok = Some(now() - 5 * 86_400);
        assert!(headline(&st, &s).2);
        // Under way: the file it's at, the home folder as ~.
        st.last_ok = None;
        st.doing = Doing::BackingUp;
        st.progress = Some(Progress { stage: Stage::Reading, current: format!("{}/Music/a.flac", target::home().display()), ..Default::default() });
        assert_eq!(headline(&st, &s), ("Backing up your home folder".into(), "Now: ~/Music/a.flac".into(), false));
        st.progress = Some(Progress { stage: Stage::Reading, resuming: true, ..Default::default() });
        assert_eq!(headline(&st, &s).0, "Resuming the backup");
        st.doing = Doing::Validating;
        assert_eq!(headline(&st, &s).0, "Validating backups");
        st.doing = Doing::Deleting { what: "the unfinished backup".into() };
        assert_eq!(headline(&st, &s), ("Deleting the unfinished backup".into(), "Then the space only it took is freed, in /backups.".into(), false));
        st.doing = Doing::Deleting { what: "2 backups and the unfinished one".into() };
        assert_eq!(headline(&st, &s).0, "Deleting backups");
        st.pid = 0;
        assert!(headline(&st, &s).2);
    }

    /// An unfinished backup, and each backup's check, said plainly.
    #[test]
    fn unfinished_and_checks_said() {
        use olo_protocol::diskbackup::Unfinished;
        let u = Unfinished { time: now(), files: 1234, bytes: 2_100_000_000, sent: 1_200_000_000, ..Default::default() };
        assert_eq!(unfinished_words(&u), "Unfinished: 1,234 files (2.10 GB) done and 1.20 GB sent; the next backup goes on from there.");
        let left = Unfinished { files: 0, sent: 5_000_000, ..u };
        assert!(unfinished_words(&left).starts_with("Unfinished: 5.0 MB sent before it stopped"));
        assert_eq!(check_cell(None), "—");
        let whole = Check { time: now(), full: true, bytes_read: 3_000_000, ..Default::default() };
        assert_eq!(check_cell(Some(&whole)), "✓ Full");
        assert!(check_words(Some(&whole)).starts_with("Validated fully just now: all it needs (3.0 MB) read back"));
        let fast = Check { full: false, by_checksum: 12, sampled: 1, ..whole.clone() };
        assert_eq!(check_cell(Some(&fast)), "✓ Fast");
        assert_eq!(check_words(Some(&fast)), "Validated just now: everything it needs is there (12 packs as sent, by their checksums; 1 sample read back). Validate Fully reads it all back.");
        let home = target::home().display().to_string();
        let bad = Check { problems: vec!["2 of its packs are missing".into()], files: vec![format!("{home}/a.bin")], files_affected: 3, ..whole.clone() };
        assert_eq!(check_cell(Some(&bad)), "✗ Damaged");
        let said = check_words(Some(&bad));
        assert!(said.contains("2 of its packs are missing") && said.contains("~/a.bin and 2 more") && said.ends_with("sends again what it can."), "{said}");
        let failed = Check { error: Some("Google Drive didn't answer".into()), ..whole };
        assert_eq!(check_cell(Some(&failed)), "Not checked");
    }
}
