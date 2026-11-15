//! Upgrade Fedora: the release this computer runs, whether a newer one
//! is out (Fedora's own list of releases), and the upgrade to it: every
//! package of the new release downloaded now, then installed as the
//! computer restarts (as `dnf system-upgrade` does, through the daemon's
//! system upgrade and offline transaction). What waits for the next start
//! (an upgrade, or updates put off till then) shows here and over the
//! updates, to restart for or cancel.

use olo_kit::{
    ClickEvent, Cx, Element, IntoElement as _, ParentElement as _, Styled as _, div, metrics as m, px, style, theme,
    widgets::{Button, StringView, button_row},
};

use crate::{
    Msg as AppMsg, OsUpdates,
    review::{self, Op, Plan},
};

#[derive(Clone)]
pub enum Msg {
    /// The newest release out (None: couldn't tell), or why not.
    Checked(Result<Option<u32>, String>),
    Prepare(u32),
    CancelWaiting,
    Cancelled(Result<(), String>),
}

#[derive(Default)]
pub struct State {
    pub current: Option<u32>,
    /// The release to upgrade to, when there's a newer one.
    pub next: Option<u32>,
    pub newest: Option<u32>,
    pub checking: bool,
    pub error: Option<String>,
}

/// The Fedora release this computer runs (`/etc/os-release`'s
/// VERSION_ID; `OLO_UPDATES_OS_RELEASE` names another file for tests).
pub fn current() -> Option<u32> {
    let path = std::env::var("OLO_UPDATES_OS_RELEASE").unwrap_or_else(|_| "/etc/os-release".into());
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().find_map(|l| l.strip_prefix("VERSION_ID=")).map(|v| v.trim_matches('"')).and_then(|v| v.parse().ok())
}

/// The newest Fedora out: from fedoraproject.org's releases.json (a
/// file's when `OLO_UPDATES_RELEASES` names one; a test against the fake
/// daemon without it says one newer than this).
fn newest() -> Result<Option<u32>, String> {
    let text = match std::env::var("OLO_UPDATES_RELEASES") {
        Ok(path) => std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?,
        Err(_) if !olo_system::dnf::real() => return Ok(current().map(|c| c + 1)),
        Err(_) => crate::repos::fetch("https://fedoraproject.org/releases.json")?,
    };
    let list: Vec<serde_json::Value> = serde_json::from_str(&text).map_err(|e| format!("Fedora’s list of releases couldn’t be read: {e}"))?;
    Ok(list.iter().filter_map(|r| r.get("version").and_then(|v| v.as_str())).filter(|v| !v.contains(' ')).filter_map(|v| v.parse::<u32>().ok()).max())
}

pub fn check(app: &mut OsUpdates, cx: &mut Cx<OsUpdates>) {
    app.rel.current = current();
    app.rel.checking = true;
    cx.work(newest, |r| AppMsg::Release(Msg::Checked(r)));
}

pub fn message(app: &mut OsUpdates, msg: Msg, cx: &mut Cx<OsUpdates>) {
    match msg {
        Msg::Checked(r) => {
            app.rel.checking = false;
            match r {
                Ok(newest) => {
                    app.rel.error = None;
                    app.rel.newest = newest;
                    // One release at a time (Fedora supports two, but one
                    // is what's tested most).
                    app.rel.next = match (app.rel.current, newest) {
                        (Some(c), Some(n)) if n > c => Some(c + 1),
                        _ => None,
                    };
                }
                Err(e) => app.rel.error = Some(e),
            }
        }
        Msg::Prepare(to) => {
            let title = format!("Upgrade to Fedora {to}");
            review::open(Plan { title: title.clone(), ops: vec![(Op::SystemUpgrade, Vec::new())], release: Some(to.to_string()), offline_only: true, comment: title }, cx.messenger());
        }
        Msg::CancelWaiting => {
            let Some(s) = app.session.clone() else { return };
            cx.work(move || s.cancel_offline().map_err(|e| olo_system::dnf::error_text(&e)), |r| AppMsg::Release(Msg::Cancelled(r)));
        }
        Msg::Cancelled(r) => match r {
            Ok(()) => {
                app.say("What waited for the next start was cancelled.", false);
                app.load(false, cx);
            }
            Err(e) => app.say(e, true),
        },
    }
}

pub fn command(app: &mut OsUpdates, command: &str, cx: &mut Cx<OsUpdates>) -> bool {
    match command {
        "prepare-upgrade" => match app.rel.next {
            Some(n) => message(app, Msg::Prepare(n), cx),
            None => return false,
        },
        "cancel-waiting" => message(app, Msg::CancelWaiting, cx),
        _ => return false,
    }
    true
}

/// What waits for the next start, if anything: a box to restart for it
/// or cancel it.
pub fn waiting_banner(app: &mut OsUpdates, cx: &mut Cx<OsUpdates>) -> Option<Element> {
    if !app.offline.pending {
        return None;
    }
    let p = theme::palette();
    let st = &app.offline.status;
    let target = st.get("target_releasever").filter(|t| app.rel.current.is_none_or(|c| t.parse::<u32>().ok() != Some(c)));
    let what = match target {
        Some(t) => format!("The upgrade to Fedora {t} is downloaded and waits for the computer to restart."),
        None => "Updates are downloaded and wait for the computer to restart: they’re installed then.".to_owned(),
    };
    Some(
        div()
            .flex()
            .items_center()
            .gap(px(m::SPACING))
            .p(px(m::SPACING))
            .bg(style::c(p.list_background))
            .child(div().flex_1().min_w_0().child(StringView::new(what).wrap(true)))
            .child(Button::new("Cancel It").id("cancel-waiting").on_click(cx.listener(|v: &mut OsUpdates, _: &ClickEvent, cx| message(v, Msg::CancelWaiting, cx))))
            .child(Button::new("Restart Now").id("restart").default(true).on_click(cx.listener(|v: &mut OsUpdates, _: &ClickEvent, cx| v.restart(cx))))
            .into_element(),
    )
}

pub fn body(app: &mut OsUpdates, cx: &mut Cx<OsUpdates>) -> Element {
    let title = |s: String| StringView::new(s).bold(true).size(style::TITLE_SIZE).height(style::line_row(style::TITLE_SIZE));
    let mut col = div().size_full().flex().flex_col().gap(px(m::SPACING));
    if let Some(b) = waiting_banner(app, cx) {
        col = col.child(b);
    }
    let runs = match app.rel.current {
        Some(c) => format!("This computer runs Fedora {c}."),
        None => "Which Fedora this computer runs can’t be told (/etc/os-release says nothing).".to_owned(),
    };
    col = col.child(title(runs));
    match (app.rel.next, &app.rel.error, app.rel.checking) {
        (_, _, true) => col = col.child(StringView::new("Looking for a newer release…").dim(true)),
        (Some(n), _, _) => {
            col = col
                .child(StringView::new(format!("Fedora {n} is out.")).bold(true))
                .child(
                    StringView::new(format!(
                        "Upgrading downloads every package of Fedora {n} now, while you work, then installs them as the computer restarts: that takes a while, and the computer can’t be used meanwhile. Your files, settings and apps stay. Update to the newest of this release first, and have a backup."
                    ))
                    .wrap(true),
                )
                .children((!app.updates.updates.is_empty()).then(|| {
                    StringView::new(format!("{} of Fedora {} {} waiting: install {} first, in Updates.", app.updates.updates.len(), app.rel.current.unwrap_or(0), if app.updates.updates.len() == 1 { "update is" } else { "updates are" }, if app.updates.updates.len() == 1 { "it" } else { "them" }))
                        .wrap(true)
                        .color(style::c(theme::palette().failure))
                }))
                .child(
                    button_row().justify_start().child(
                        Button::new(format!("Prepare the Upgrade to Fedora {n}…")).id("prepare-upgrade").default(true).enabled(app.loading.is_none() && !app.offline.pending).on_click(cx.listener(move |v: &mut OsUpdates, _: &ClickEvent, cx| message(v, Msg::Prepare(n), cx))),
                    ),
                );
            if app.rel.newest.is_some_and(|newest| newest > n) {
                col = col.child(StringView::new(format!("Fedora {} is out too: after this upgrade, the next.", app.rel.newest.unwrap_or(n))).dim(true));
            }
        }
        (None, Some(e), _) => col = col.child(StringView::new(format!("Whether a newer release is out can’t be told: {e}")).wrap(true).color(style::c(theme::palette().failure))),
        (None, None, _) => col = col.child(StringView::new("It’s the newest release: there’s nothing to upgrade to.").dim(true)),
    }
    col.child(div().flex_1()).child(app.status(cx)).into_element()
}
