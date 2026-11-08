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

use olo_diskbackup::exclude::GROUPS;
use olo_protocol::diskbackup::{Doing, Priority, Progress, Settings, SnapshotInfo, Stage, State, Target};
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
    target,
};

/// The tabs, in order.
const TABS: [&str; 5] = ["Schedule", "Files", "Backups", "Speed & Resources", "Destination"];

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

/// How a backup under way stands: how much of it is done (0 to 1), and,
/// once there's enough to go by, how long is left (seconds).
pub fn how_far(p: &Progress, at: i64) -> (f32, Option<i64>) {
    if p.total_bytes == 0 {
        return (0.0, None);
    }
    // (The plan was the last backup's: never more than all of it, while
    // it's still going.)
    let done = (p.bytes as f64 / p.total_bytes as f64).min(0.99);
    let spent = (at - p.started).max(0) as f64;
    let left = (done > 0.02 && spent >= 10.0).then(|| (spent * (1.0 - done) / done).round() as i64);
    (done as f32, left)
}

/// "about 6 minutes left", "about 2 hours left", "less than a minute left".
fn left_words(secs: i64) -> String {
    match secs {
        ..60 => "less than a minute left".into(),
        60..5400 => format!("about {} left", minutes((secs + 30) / 60)),
        _ => format!("about {} hours left", (secs + 1800) / 3600),
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
fn folder_name(p: &str) -> String {
    if p == "~" { "Your home folder".into() } else { p.to_owned() }
}

// ---- the header: whether your files are safe -----------------------------

/// The headline, what's under it, and whether it's bad news.
pub fn headline(st: &State, s: &Settings) -> (String, String, bool) {
    let to = format!("To {}", s.target.describe());
    if st.pid == 0 {
        return ("DiskBackup isn’t running".into(), "Its service starts with your session: log out and in again, or ask Olo Doctor to start it.".into(), true);
    }
    let progress = st.progress.clone().unwrap_or_default();
    match &st.doing {
        Doing::BackingUp => {
            let title = match progress.stage {
                Stage::Counting => "Getting ready: counting what to back up",
                Stage::Reading => "Backing up",
                Stage::Saving => "Finishing the backup",
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
        Doing::Pruning => return ("Tidying old backups".into(), format!("Letting go of backups past keeping, and freeing the space only they took. {to}."), false),
        Doing::Restoring => return ("Restoring".into(), to, false),
        Doing::Erasing => return ("Erasing every backup".into(), format!("{to}. The next backup starts afresh."), false),
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

/// Something to see to, in a line: a small mark, what's wrong, and the
/// button that sees to it, on the banners' tint (a `Banner` is for news;
/// this is a reminder).
fn notice(text: &'static str, button: Button) -> impl IntoElement {
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
        .child(div().flex_1().min_w_0().child(StringView::new(text).wrap(true)))
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
        let running = matches!(st.doing, Doing::BackingUp | Doing::Pruning | Doing::Erasing);
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
            let items: Vec<gpui::AnyElement> = match self.tab {
                0 => self.schedule(&s, cx),
                1 => self.files(&s, &st, cx),
                2 => self.backups(&s, &st, cx),
                3 => self.speed(&s, cx),
                _ => self.destination(&s, cx),
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
                        Button::new("Set Up Google Drive…").enabled(!busy).on_click(self.command("google-drive")),
                    ))),
            )
            .child(dim("Your home folder is backed up, every hour, leaving out what can be downloaded or built again. Change any of it once it’s set up."))
    }

    /// Set up: how it stands, and Back Up Now (or Stop).
    fn header(&self, s: &Settings, st: &State, cx: &mut Context<Self>) -> gpui::Div {
        let p = theme::palette(cx);
        let (title, detail, bad) = headline(st, s);
        let running = matches!(st.doing, Doing::BackingUp | Doing::Pruning | Doing::Erasing);
        let mut buttons = div().flex().flex_row().gap(px(m::SPACING)).flex_shrink_0();
        buttons = buttons.child(Button::new("Restore…").on_click(self.command("restore")));
        buttons = buttons.child(if running {
            Button::new("Stop").on_click(self.command("stop"))
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
            words = words.child(StringView::new(said).wrap(true).dim(true));
        }
        let row = div().flex().flex_row().items_start().gap(px(m::SPACING * 2.0)).child(picture("diskbackup", ICON)).child(words).child(buttons);
        div().flex().flex_col().gap(px(m::TIGHT)).child(row).children(progress)
    }

    /// A backup (or a tidy, or erasing) under way: the whole of it.
    fn progress(&self, s: &Settings, st: &State, pr: &Progress) -> gpui::Div {
        let dim = |t: String| StringView::new(t).wrap(true).dim(true);
        let line = |t: String| StringView::new(t).wrap(true);
        let mut col = div().flex().flex_col().gap(px(m::TIGHT));
        let about = if pr.estimated { "about " } else { "" };
        match (&st.doing, pr.stage) {
            (Doing::BackingUp, Stage::Counting) => {
                col = col.child(line(format!("Counting what there is to back up: {}, {} so far.", plural(pr.total_files, "file"), size(pr.total_bytes))));
                col = col.child(dim("The first backup reads everything; later ones read only what changed, in a fraction of the time.".into()));
            }
            (Doing::BackingUp, stage) => {
                let (done, left) = how_far(pr, now());
                let saving = stage == Stage::Saving;
                let label = if saving { "Writing the backup’s lists…".to_owned() } else { format!("{} of {about}{}", size(pr.bytes), size(pr.total_bytes)) };
                col = col.child(StatusBar::new(if saving { 1.0 } else { done }).label(label).trailing(format!("{:.0}%", if saving { 100.0 } else { done * 100.0 })));
                let mut words = format!("{} of {about}{}", count(pr.files), plural(pr.total_files, "file"));
                if let Some(l) = left.filter(|_| !saving) {
                    words.push_str(&format!(" · {}", left_words(l)));
                }
                let spent = (now() - pr.started).max(1) as f64;
                let rate = pr.uploaded as f64 / spent;
                let limit = if s.upload_kbs > 0 { format!(", at most {}", speed_label(s.upload_kbs)) } else { String::new() };
                words.push_str(&format!(" · {} new · {} sent ({}/s{limit})", size(pr.new_bytes), size(pr.uploaded), size(rate as u64)));
                col = col.child(line(words));
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
                col = col.child(StatusBar::new(done).label(format!("Deleting {} of {}", count(pr.files), count(pr.total_files))).trailing(format!("{:.0}%", done * 100.0)));
            }
            (Doing::Pruning, _) => {
                col = col.child(line(if pr.current.is_empty() { "Tidying…".into() } else { format!("Tidying: {}…", pr.current) }));
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
        let d = self.app.read(cx);
        let snaps: Vec<SnapshotInfo> = match &d.snapshots {
            Some(Ok(l)) => l.clone(),
            _ => Vec::new(),
        };
        let said = match &d.snapshots {
            None => "Asking DiskBackup’s service…".to_owned(),
            Some(Err(e)) => format!("The backups can’t be listed: {e}."),
            Some(Ok(l)) if l.is_empty() => "No backups kept yet.".into(),
            Some(Ok(l)) => {
                let taken = st.size.map(|b| format!(", {} in all", size(b))).unwrap_or_default();
                format!("{} kept in {}{taken}. Choose some to delete them, or restore from any.", plural(l.len() as u64, "backup"), s.target.describe())
            }
        };
        let running = matches!(st.doing, Doing::BackingUp | Doing::Pruning | Doing::Erasing);
        let rows = snaps.clone();
        let list = ColumnListView::new(
            "backups",
            &self.list,
            vec![Column::new("When", 190.0), Column::new("Files", 90.0), Column::new("New", 90.0), Column::new("Size", 90.0), Column::new("Took", 100.0)],
            snaps.len(),
            move |cell, window, cx| {
                let Some(b) = rows.get(cell.row) else { return };
                let text = match cell.column {
                    0 => format!("{} ({})", clock(b.time), ago(b.time)),
                    1 => count(b.files),
                    2 => size(b.new_bytes),
                    3 => size(b.bytes),
                    _ => took(b.seconds),
                };
                cell.text(window, cx, text);
            },
        )
        .on_select(|_, cx| cx.refresh_windows());
        let chosen: Vec<String> = self.list.read(cx).state.selected.iter().filter_map(|&i| snaps.get(i).map(|b| b.id.clone())).collect();
        let n = chosen.len();
        let del = self.app.clone();
        let delete = Button::new(if n > 1 { format!("Delete {n} Backups…") } else { "Delete…".into() }).enabled(n > 0 && !running).on_click(move |_, cx| {
            let ids = chosen.clone();
            del.update(cx, |d, cx| d.delete(ids, cx));
        });
        let row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            .child(Button::new("Restore…").id("backups-restore").on_click(self.command("restore")))
            .child(delete)
            .child(div().flex_1())
            .child(Button::new("Erase All Backups…").enabled(!running && !snaps.is_empty()).on_click(self.command("erase-all")));
        let kept = BBox::new("Backups Kept").child(StringView::new(said).wrap(true).dim(true)).child(div().h(px(180.0)).child(list)).child(row);
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

    fn destination(&self, s: &Settings, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let d = self.app.read(cx);
        let busy = d.busy;
        let (icon, kind) = match &s.target {
            Target::GoogleDrive { .. } => ("diskbackup", "Google Drive"),
            _ => ("disk", "A disk or folder"),
        };
        let where_ = BBox::new("Where Backups Go")
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(m::SPACING))
                    .child(picture(icon, 32.0))
                    .child(div().flex_1().min_w_0().flex().flex_col().child(StringView::new(s.target.describe()).bold(true)).child(StringView::new(kind).dim(true))),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING))
                    .pt(px(m::TIGHT))
                    .child(Button::new("Choose Another Folder…").enabled(!busy).on_click(self.command("choose-folder")))
                    .child(Button::new("Google Drive…").enabled(!busy).on_click(self.command("google-drive"))),
            )
            .child(StringView::new("Moving elsewhere starts there afresh; the backups here stay until you erase them.").wrap(true).dim(true));
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
        let (done, left) = how_far(&p, t);
        assert!((done - 0.25).abs() < 1e-6);
        assert_eq!(left, Some(300), "a quarter in 100 s: 300 s to go");
        // Past the plan (it grew): never done while it runs.
        let p = Progress { started: t - 100, bytes: 150, total_bytes: 100, ..Default::default() };
        assert!(how_far(&p, t).0 <= 0.99);
        // Too soon to say.
        assert_eq!(how_far(&Progress { started: t - 3, bytes: 1, total_bytes: 100, ..Default::default() }, t).1, None);
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
        assert_eq!(headline(&st, &s), ("Backing up".into(), "Now: ~/Music/a.flac".into(), false));
        st.pid = 0;
        assert!(headline(&st, &s).2);
    }
}
