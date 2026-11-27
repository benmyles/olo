//! The Server window's drawing: a host's Rex server looked after from
//! here (`server` is where it stands, `olo_rex_client::manage` what a
//! server is and what is done to one).
//!
//! Three pages in a tab view. **Server**: each of Rex's services in use
//! there (its light, for how long it has run and how many sessions are
//! open, what it is for; Restart and Stop, or Start), and Rex's version
//! with Update. **Accounts** (a login server's) or **Members** (a shared
//! server's `rex` group): who may use it, as a list, with Allow… (or
//! Add…) and Remove… for the one chosen. **Log**: what the service has
//! written, to read and copy.
//!
//! Nothing is done at a click: each button puts its action to the
//! person as an alert in the window (what it costs in the warning's
//! colour, what it does, and what is run as root, exactly, in a well),
//! and the page comes back with the host's own answer once it is done.
//! Every stage fills the frame the pages have ([`sheet`]), its icon on
//! the alert's stripe, its words in one column and its buttons at the
//! foot: the window never changes size under the pointer.

use std::rc::Rc;
use std::time::Duration;

use olo_kit::{
    Element, IntoElement as _, ParentElement as _, SharedString, Styled as _, WindowCx, canvas, div, metrics as m, px, style, theme,
    widgets::{AlertFrame, Button, Column, ColumnListView, ListModel, MenuField, StatusBar, StringView, TAB_HEIGHT, TabView, TextView, alert_title, button_row, divider_for, separator},
};
use olo_rex_client::manage::{Action, LogRange, Server, Service};

use crate::panels::{self, Light};
use crate::server::{Managed, Page, Stage};

/// The window's width: Hosts and Ports', which it is opened from.
pub const SERVER_WIDTH: f32 = 580.0;

/// How many lines of the log show at once.
const LOG_LINES: f32 = 13.0;

/// The log's text: the small monospace.
const LOG_SIZE: f32 = style::SMALL_SIZE;

/// How tall the log's view is inside its frame: a whole number of
/// lines and its text's two margins, over its bar along the bottom. A
/// log of as many lines fits it with nothing to scroll (with one margin
/// counted it showed a thumb the bar's whole length); and, its top
/// margin kept clear (`TextView::clear_top`), scrolled to its end or to
/// its top its first and last lines show whole (it cut its first line
/// in half: the design reviews).
fn log_room() -> f32 {
    LOG_LINES * TextView::line_height_at(LOG_SIZE) + 2.0 * TextView::MARGINS.1 + olo_kit::widgets::SCROLL_BAR
}

/// How tall a page is at least: what the Log page needs (its row of
/// controls, a step, its view in its frame of two lines), the tallest
/// of the three as they usually are, so that going from one to another
/// moves nothing.
fn page_height() -> f32 {
    2.0 * m::SPACING + m::CONTROL_HEIGHT + m::SPACING + log_room() + 4.0
}

/// How tall the window's content is with its pages showing (the tab
/// view's tabs and its panel's own margins round a page, in the
/// window's): every other stage is as tall.
fn frame_height() -> f32 {
    2.0 * m::WINDOW_INSET + TAB_HEIGHT + 2.0 * m::WINDOW_INSET + page_height()
}

/// How wide the menu of people to let in is: a name's room, not the
/// alert's whole column (which read as a field to type in).
const CHOOSER: f32 = 160.0;

/// What a list keeps of its width for its rim and its scroll bar (and
/// the tab view's panel of the window's for its own edge): its columns
/// are as wide as the rest, so none runs under the bar.
const LIST_EDGE: f32 = 24.0;

/// What a button of the window asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum ServerDo {
    /// Back to the page, from what was asked or what didn't go.
    Back,
    /// The person agreed: do it.
    Go,
    /// Look at the host again (with the key chosen for root).
    Again,
    /// Close the window.
    Close,
    /// Put this to the person.
    Ask(Action),
    /// Put letting somebody in to the person (who, is chosen there).
    LetIn,
    /// From what didn't go to the log that may say why.
    ShowLog,
    /// The Log page, on this service's log.
    LogOf(Service),
    /// Read the log again.
    Refresh,
    /// The log, whole, to the clipboard.
    Copy,
    /// Olo Passwords, opened: it has the keys.
    OpenPasswords,
}

/// A row of the Accounts (or Members) list.
#[derive(Clone, Debug, PartialEq)]
pub struct PersonRow {
    /// The account's name in the file (or the member's).
    pub name: String,
    /// As it is shown.
    pub shown: String,
    /// Who may use it, in words: only where that isn't the account
    /// itself (else empty).
    pub who: String,
    /// Its open sessions, in words (a member: nothing).
    pub sessions: String,
}

/// The list's rows, as the host last said things stand.
pub fn person_rows(managed: &Managed) -> Vec<PersonRow> {
    let Some(server) = &managed.server else { return Vec::new() };
    if managed.shared() {
        return server.members.iter().map(|name| PersonRow { name: name.clone(), shown: name.clone(), who: String::new(), sessions: String::new() }).collect();
    }
    let Some(file) = &server.accounts else { return Vec::new() };
    file.accounts()
        .into_iter()
        .map(|a| {
            let own = server.people.iter().find(|p| p.name == a.name).map(|p| format!("peer:{}", p.uid));
            let others: Vec<String> = a.principals.iter().filter(|p| Some(*p) != own.as_ref()).map(|p| principal_said(p, server)).collect();
            PersonRow {
                shown: if a.name == "*" { olo_kit::words::tr("Every regular account").into() } else { a.name.clone() },
                who: others.join("; "),
                sessions: match (a.name.as_str(), server.sessions_of(&a.name)) {
                    ("*", _) => String::new(),
                    (_, Some(0)) | (_, None) if server.asked_sessions => olo_kit::words::tr("None").into(),
                    (_, Some(open)) => olo_kit::tr!("{} open", open).into(),
                    _ => String::new(),
                },
                name: a.name,
            }
        })
        .collect()
}

/// A principal of the accounts file (how a caller is known to the
/// login server) in the person's words: `peer:1000` is whoever is
/// signed in to the machine as that user, which from here is over SSH.
pub fn principal_said(principal: &str, server: &Server) -> String {
    if let Some(uid) = principal.strip_prefix("peer:") {
        return match uid.parse::<u32>().ok().and_then(|uid| server.people.iter().find(|p| p.uid == uid)) {
            Some(person) => olo_kit::tr!("{}, over SSH", person.name).into(),
            None => olo_kit::tr!("User {}, over SSH", uid).into(),
        };
    }
    match principal {
        "tailscale:*" => olo_kit::words::tr("Anyone on the tailnet").into(),
        "tailscale:tag:*" => olo_kit::words::tr("Any tagged device on the tailnet").into(),
        "identified" => olo_kit::words::tr("Anyone who is identified").into(),
        "anonymous" => olo_kit::words::tr("Anyone at all").into(),
        other => {
            if let Some(tag) = other.strip_prefix("tailscale:tag:") {
                olo_kit::tr!("Devices tagged {}, over Tailscale", tag).into()
            } else if let Some(login) = other.strip_prefix("tailscale:") {
                olo_kit::tr!("{}, over Tailscale", login).into()
            } else {
                other.to_owned()
            }
        }
    }
}

/// For how long a service has run, in whole units: "less than a
/// minute", "9 minutes", "2 hours", "3 days".
fn running_for(age: Duration) -> String {
    let s = age.as_secs();
    match s {
        0..60 => olo_kit::words::tr("less than a minute").into(),
        60..3600 => olo_kit::words::count(s / 60, "1 minute", "{n} minutes"),
        3600..86_400 => olo_kit::words::count(s / 3600, "1 hour", "{n} hours"),
        _ => olo_kit::words::count(s / 86_400, "1 day", "{n} days"),
    }
}

/// How long ago a service failed, in whole units as its running is
/// said: "Failed just now", "Failed 4 minutes ago".
fn failed_ago(age: Duration) -> String {
    let s = age.as_secs();
    match s {
        0..60 => olo_kit::words::tr("Failed just now").into(),
        60..3600 => olo_kit::words::count(s / 60, "Failed 1 minute ago", "Failed {n} minutes ago"),
        3600..86_400 => olo_kit::words::count(s / 3600, "Failed 1 hour ago", "Failed {n} hours ago"),
        _ => olo_kit::words::count(s / 86_400, "Failed 1 day ago", "Failed {n} days ago"),
    }
}

/// `line` on lines of `per_line` letters at most, as a long line is
/// written out in a fixed-width face: broken at a space, each line
/// after the first set in by two letters. (Left to wrap, the rest of a
/// host's sentence began at the margin, and one message read as two:
/// the fifth design review. A command's lines are set so too:
/// `panels::command_lines`.)
pub(crate) fn hung(line: &str, per_line: usize) -> Vec<String> {
    const IN: &str = "  ";
    let mut lines: Vec<String> = Vec::new();
    let mut now = String::new();
    for word in line.split(' ') {
        let lead = if lines.is_empty() { 0 } else { IN.len() };
        if !now.is_empty() && lead + now.chars().count() + 1 + word.chars().count() > per_line {
            lines.push(format!("{}{now}", if lines.is_empty() { "" } else { IN }));
            now.clear();
        }
        if !now.is_empty() {
            now.push(' ');
        }
        now.push_str(word);
    }
    lines.push(format!("{}{now}", if lines.is_empty() { "" } else { IN }));
    lines
}

/// A failed service's last words on two lines of `per_line` letters at
/// most: whole, or cut at a word with an ellipsis (cut on the status
/// line, they ended "is not one: lin…", the line's number gone: the
/// third design review).
fn last_words(words: &str, per_line: usize) -> Vec<String> {
    let mut lines = hung(words, per_line.max(8));
    if lines.len() > 2 {
        lines.truncate(2);
        // (Room for the ellipsis, at a word.)
        let mut second = lines[1].clone();
        while second.chars().count() + 1 > per_line.max(8) {
            second.truncate(second.rfind(' ').unwrap_or(0));
        }
        lines[1] = format!("{}…", second.trim_end_matches([',', ':', ';', ' ']));
    }
    lines
}

/// How a service stands: its light, and the same in words (with the
/// sessions open through it, where it runs and the host could say).
fn service_state(managed: &Managed, server: &Server, service: Service) -> (Light, String) {
    let unit = match service {
        Service::Login => server.facts.login,
        Service::Shared => server.facts.shared,
    };
    let state = server.state(service);
    if unit.active {
        let running: String = match (server.up(service), managed.read) {
            // (Counted on from when the host said it, by the kit's
            // own clock for ages: no timer of the window's.)
            (Some(up), Some(read)) => olo_kit::tr!("Running for {}", olo_kit::ago::age(Duration::from_secs(up) + read.elapsed(), running_for)).into(),
            _ => olo_kit::words::tr("Running").into(),
        };
        let open = match server.sessions_on(service) {
            Some(0) => Some(olo_kit::words::tr("no sessions open").to_string()),
            Some(open) => Some(olo_kit::words::count(open as u64, "1 session open", "{n} sessions open")),
            None => None,
        };
        return (Light::On, open.map_or(running.clone(), |open| format!("{running}  ·  {open}")));
    }
    if state.active {
        return (Light::On, olo_kit::words::tr("Ready: it starts when somebody signs in").into());
    }
    if server.failed(service) {
        // (Its last words go under this line, whole: `last_words`.)
        return (
            Light::Down,
            match (server.ended(service), managed.read) {
                (Some(ended), Some(read)) => olo_kit::ago::age(Duration::from_secs(ended) + read.elapsed(), failed_ago),
                _ => olo_kit::words::tr("Failed").into(),
            },
        );
    }
    (Light::Off, olo_kit::words::tr(if state.enabled { "Stopped" } else { "Not in use" }).into())
}

/// What a service is for, whether it starts with the machine, and what
/// stopping it leaves alone; and, apart, what stopping it costs.
fn service_notes(managed: &Managed, server: &Server, service: Service) -> (String, Option<String>) {
    let state = server.state(service);
    let what: String = match service {
        Service::Login => olo_kit::tr!("Each person on {} gets a Rex server of their own; they sign in through this one.", managed.name).into(),
        Service::Shared => olo_kit::words::tr("Everyone in the rex group shares this server, and every terminal on it runs as the rex account.").into(),
    };
    let boot: Option<String> = match (state.active, state.enabled) {
        (_, true) => Some(olo_kit::tr!("It starts when {} starts.", managed.name).into()),
        (true, false) => Some(olo_kit::tr!("It won’t start again when {} restarts.", managed.name).into()),
        (false, false) => None,
    };
    let mut note = what;
    if let Some(boot) = boot {
        note = format!("{note} {boot}");
    }
    match (service, state.active) {
        (Service::Login, true) => (format!("{note} {}", olo_kit::words::tr("Restarting or stopping it leaves open sessions running.")), None),
        (Service::Shared, true) => (note, Some(olo_kit::words::tr("Restarting or stopping it ends every session.").into())),
        _ => (note, None),
    }
}

/// A stage that isn't the pages, as an alert filling the frame the
/// pages have: `icon` on the stripe, `body` from the top, `foot` (the
/// buttons) at the bottom. The window keeps its size from one stage to
/// the next.
fn sheet(icon: &str, body: Vec<Element>, foot: Element) -> Element {
    let frame = AlertFrame::new(icon.to_owned()).children(body).child(div().flex_1().min_h(px(m::SPACING))).child(foot);
    div().w_full().min_h(px(frame_height())).flex().flex_col().child(div().flex_1().w_full().flex().flex_row().child(frame)).into_element()
}

/// The Server window. (`phase`: where a busy bar's block is. `rows`
/// and `chosen`: the list's, as the window keeps them. `log`: the Log
/// page's text view, kept and filled by the window.)
#[allow(clippy::too_many_arguments)]
pub fn render_server_panel(
    cx: &olo_kit::Cx<crate::ui::RexWindow>,
    managed: &Managed,
    phase: f32,
    list: &ListModel,
    rows: &Rc<Vec<PersonRow>>,
    chosen: Option<usize>,
    log: Option<Element>,
    on_do: impl Fn(&ServerDo, &mut WindowCx) + 'static,
    on_key: impl Fn(&usize, &mut WindowCx) + 'static,
    on_page: impl Fn(&usize, &mut WindowCx) + 'static,
    on_person: impl Fn(&usize, &mut WindowCx) + 'static,
    on_range: impl Fn(&usize, &mut WindowCx) + 'static,
    on_chosen: impl Fn(&(), &mut WindowCx) + 'static,
) -> Element {
    let on_do: Rc<dyn Fn(&ServerDo, &mut WindowCx)> = Rc::new(on_do);
    let button = |label: &'static str, what: ServerDo| {
        let on_do = on_do.clone();
        // (Its words looked up in the person's language: a `String` made of
        // the literal was shown as it is, in English.)
        Button::new(olo_kit::words::tr(label).to_owned()).min_width(m::BUTTON_MIN_WIDTH).on_click(move |_, cx| on_do(&what, cx))
    };
    if let Some(stage) = render_stage(managed, phase, SERVER_WIDTH, false, None, &sheet, &button, on_key, on_person) {
        return stage;
    }
    let Some(server) = managed.server.as_ref() else { return div() };
    let small = |text: String| StringView::new(text).size(style::SMALL_SIZE).dim(true).wrap(true);
    let people = if managed.shared() { "Members" } else { "Accounts" };
    let tabs: Vec<SharedString> = ["Server", people, "Log"].into_iter().map(SharedString::from).collect();
    let at = Page::ALL.iter().position(|p| *p == managed.page).unwrap_or(0);
    let page: Vec<Element> = match managed.page {
        Page::Server => server_page(managed, server, &button, &small),
        Page::People => people_page(cx, managed, server, list, rows, chosen, &button, &small, on_chosen),
        Page::Log => log_page(cx.scale_factor(), managed, log, &button, on_range),
    };
    div().w_full().flex().flex_col().p(px(m::WINDOW_INSET)).child(
        TabView::new("rex-server-tabs", tabs, at).grows().on_select(on_page).child(div().flex_1().min_h(px(page_height())).flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING)).children(page)),
    )
}

/// What the window shows at a stage that isn't its pages: a host being
/// looked at, a key to choose for root, a question before a change, the
/// change under way, and what didn't go; each an alert's icon, what it
/// says and its buttons, which `wrap` makes an element of. None where
/// the pages show. Shared with the Host window's Tweaks tab, whose
/// record of the host goes through the same stages
/// (`server::for_tweaks`): `width`, the alert's; `in_tab`, it is a
/// page of a window that stays (what leaves a stage there is Cancel,
/// not Close); `extra`, something of the asker's own in a question.
#[allow(clippy::too_many_arguments)]
pub fn render_stage(
    managed: &Managed,
    phase: f32,
    width: f32,
    in_tab: bool,
    extra: Option<Element>,
    wrap: &dyn Fn(&str, Vec<Element>, Element) -> Element,
    button: &dyn Fn(&'static str, ServerDo) -> Button,
    on_key: impl Fn(&usize, &mut WindowCx) + 'static,
    on_person: impl Fn(&usize, &mut WindowCx) + 'static,
) -> Option<Element> {
    let mut extra = extra;
    let close = if in_tab { "Cancel" } else { "Close" };
    let body = |text: String| StringView::new(text).wrap(true).into_element();
    let small = |text: String| StringView::new(text).size(style::SMALL_SIZE).dim(true).wrap(true);
    // (What a change costs: the first thing said under the title, in
    // the alert's own text, beside the warning's icon. It was in the
    // warning's colour: brown on the panel in the light look, at less
    // contrast than text of its size wants, and in the dark look the
    // loudest line of the alert, over its title: Opus's look,
    // 2026-11-27.)
    let cost = |text: String| StringView::new(text).wrap(true).into_element();
    let gap = |h: f32| div().h(px(h)).flex_shrink_0().into_element();
    // (The alert's column: from past its icon to the window's inset.)
    let column = width - olo_kit::widgets::ALERT_TEXT_X - m::WINDOW_INSET;
    // Waiting, and working: a heading, the bar with its words, a line.
    // Waiting, and working: a heading, the bar with its words, a line.
    // (`label`: what is being done, over the bar. `under`: the line
    // the host wrote last, where there is one. `note`: the window's
    // own word of it. In the Server window the note is under the bar,
    // in its grey. On a tab it is the second sentence over the bar,
    // and only the host's line is under it, in the alert's own ink:
    // the host's line and the note in one grey a few points apart read
    // as two lines of the host's, both looks, 2026-11-27.)
    let waiting = |heading: String, label: String, under: Option<String>, note: String, foot: Element| {
        let hosts_line = under.is_some();
        let mut body = vec![alert_title(heading).into_element(), gap(m::TIGHT)];
        if in_tab {
            body.extend([StringView::new(format!("{label}. {note}")).wrap(true).into_element(), gap(m::TIGHT), StatusBar::busy(phase).into_element()]);
            if let Some(under) = under {
                body.extend([gap(m::TIGHT), StringView::new(if under.is_empty() { " ".to_owned() } else { under }).mono(true).size(style::SMALL_SIZE).cut(true).height(m::LABEL_ROW).into_element()]);
            }
        } else {
            let line = match under {
                Some(under) => StringView::new(if under.is_empty() { " ".to_owned() } else { under }).mono(true),
                None => StringView::new(note.clone()),
            };
            body.extend([StatusBar::busy(phase).label(label).into_element(), gap(m::TIGHT), line.size(style::SMALL_SIZE).dim(true).cut(true).height(m::LABEL_ROW).into_element()]);
            // (Under the host's line, the note of it; where there is no
            // such line the note is the line.)
            if hosts_line {
                body.extend([gap(m::SPACING), small(note).into_element()]);
            }
        }
        wrap("rex", body, foot)
    };
    let Some(server) = managed.server.as_ref().filter(|_| !matches!(managed.stage, Stage::Looking | Stage::RootKey { .. })) else {
        // No server read yet (or root's key to be chosen first).
        return Some(match &managed.stage {
            Stage::RootKey { keys, .. } if keys.is_empty() => wrap(
                "key",
                vec![
                    alert_title("Olo Passwords has no SSH keys to offer").into_element(),
                    gap(m::TIGHT),
                    body(if in_tab {
                        olo_kit::tr!(
                            "Changing {}’s settings needs root, and {} can’t use sudo there without a password. Unlock Olo Passwords, or add a key to it that root on {} accepts.",
                            managed.name,
                            managed.target.user.as_deref().unwrap_or_default(),
                            managed.name
                        )
                        .into()
                    } else {
                        olo_kit::tr!("{} can’t use sudo on {} without a password, so looking after its server needs root’s own login, or an account that can. Unlock Olo Passwords, or add a key to it that root there accepts.", managed.target.user.as_deref().unwrap_or_default(), managed.name).into()
                    }),
                ],
                // (A side trip, not a third answer: at the left, under the words.)
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(button("Open Passwords", ServerDo::OpenPasswords))
                    .child(div().flex_1())
                    .child(button_row().child(button("Cancel", ServerDo::Close)).child(button("Try Again", ServerDo::Again).default(true)))
                    .into_element(),
            ),
            Stage::RootKey { keys, chosen, .. } => wrap(
                "key",
                vec![
                    alert_title(olo_kit::tr!("Root on {} didn’t accept your keys", managed.name)).into_element(),
                    gap(m::TIGHT),
                    // (What would do instead, said: Ben, 2026-11-24.)
                    body(if in_tab {
                        olo_kit::tr!(
                            "Changing {}’s settings needs root, and {} can’t use sudo there without a password. Choose a key that root on {} accepts. If none does, add your public key to /root/.ssh/authorized_keys on that machine, then try again.",
                            managed.name,
                            managed.target.user.as_deref().unwrap_or_default(),
                            managed.name
                        )
                        .into()
                    } else {
                        olo_kit::tr!(
                            "{} can’t use sudo on {} without a password, so looking after its server needs root’s own login, or an account that can. Choose a key that root accepts there. If none does, add your public key to /root/.ssh/authorized_keys on that machine, then try again.",
                            managed.target.user.as_deref().unwrap_or_default(),
                            managed.name
                        )
                        .into()
                    }),
                    gap(m::SPACING),
                    MenuField::new("server-root-key", keys.iter().map(panels::key_called).collect(), *chosen).label("Key:").on_change(on_key).into_element(),
                ],
                button_row().child(button("Cancel", ServerDo::Close)).child(button("Try Again", ServerDo::Again).default(true)),
            ),
            Stage::Failed { what, why, said, icon, .. } => {
                wrap(icon, failed(managed, what, why, said, width), button_row().child(button(close, ServerDo::Close)).child(button("Try Again", ServerDo::Again).default(true)))
            }
            // (The stage in the heading, what is done in the bar: asking
            // whether the account can use sudo first, as Add Host does.)
            _ => waiting(
                match (managed.root_by.is_none(), in_tab) {
                    (true, _) => olo_kit::tr!("Checking sudo on {}…", managed.name).into(),
                    (false, true) => olo_kit::tr!("Reading {}’s settings…", managed.name).into(),
                    (false, false) => olo_kit::tr!("Connecting to {}…", managed.name).into(),
                },
                // (Asking whether the account may use sudo, then root's
                // look, with sudo or signed in as root.)
                match managed.root_by {
                    Some(olo_rex_client::provision::RootBy::Sudo) => olo_kit::tr!("Running sudo as {}", managed.target.user.as_deref().unwrap_or_default()),
                    Some(_) => olo_kit::tr!("Signing in as {} with a key from Olo Passwords", managed.root()),
                    None => olo_kit::tr!("Asking whether {} can use sudo", managed.target.user.as_deref().unwrap_or_default()),
                }
                .into(),
                None,
                olo_kit::words::tr("Nothing is changed until you choose something to do.").to_string(),
                button_row().child(button("Cancel", ServerDo::Close)),
            ),
        });
    };
    Some(match &managed.stage {
        Stage::Confirm { action, choose } => {
            let warning = action.warning(server);
            let what = action.what_on(server, Some(&managed.name));
            let costly = warning.is_some();
            // (An update says to what, where the repository said.)
            let title: SharedString = match (action, &server.newer) {
                (Action::Update, Some(newer)) => olo_kit::tr!("Update Rex on {} to {}?", managed.name, newer).into(),
                _ => olo_kit::tr!("{} on {}?", olo_kit::words::of(action.title()), managed.name).into(),
            };
            let mut said = vec![alert_title(title).into_element()];
            // (What it costs first, in the warning's colour and no other
            // words in it; then what it does.)
            if let Some(warning) = warning {
                said.extend([gap(m::TIGHT), cost(olo_kit::words::of(warning).to_string())]);
            }
            if !what.is_empty() {
                if let Some((prose, cmd)) = what.split_once("\nTo restart: ") {
                    let sp = f32::from(style::text_width(" ", style::PLAIN_SIZE, style::ui_font(olo_kit::FontWeight::NORMAL)));
                    let restart_line = div()
                        .w_full()
                        .h(px(m::LABEL_ROW))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(sp))
                        .child(StringView::new(olo_kit::words::tr("To restart:").to_owned()).height(m::LABEL_ROW))
                        .child(StringView::new(cmd.to_owned()).mono(true).size(panels::COMMAND_SIZE).height(m::LABEL_ROW).beside(style::PLAIN_SIZE))
                        .into_element();
                    said.extend([gap(m::TIGHT), body(olo_kit::words::of(prose.to_owned()).to_string()), gap(m::TIGHT), restart_line]);
                } else {
                    said.extend([gap(m::TIGHT), body(olo_kit::words::of(what).to_string())]);
                }
            }
            // Who, where somebody is to be chosen: the alert's title,
            // its words and what is run follow the choice.
            if *choose {
                let could = managed.could_join();
                let at = match action {
                    Action::Allow(person) => could.iter().position(|p| p.name == person.name),
                    Action::Join(name) => could.iter().position(|p| &p.name == name),
                    _ => None,
                };
                let label = if managed.shared() { "Person:" } else { "Account:" };
                said.extend([
                    gap(m::SPACING),
                    MenuField::new("server-person", could.iter().map(|p| SharedString::from(p.name.clone())).collect(), at.unwrap_or(0))
                        .label(label)
                        .width(divider_for(&[label]) + CHOOSER)
                        .on_change(on_person)
                        .into_element(),
                ]);
            }
            // (Something of the asker's own in the question, over what
            // is run: a choice that changes it.)
            if let Some(extra) = extra.take() {
                said.extend([gap(m::SPACING), extra]);
            }
            let shown = action.shown(server);
            // (A change of limits is shown as what changes, not as the
            // script that does it, which is a page long.)
            let limits = matches!(action, Action::Tune(_) | Action::Untune(_));
            let command = action.command(server).is_some() && !limits;
            // (What is run, as it is run: through sudo, shown with it.)
            let shown = if command && managed.root_by == Some(olo_rex_client::provision::RootBy::Sudo) { olo_rex_client::provision::by_sudo(&shown) } else { shown };
            let lines = if command { panels::command_lines(&shown, panels::command_room(column)) } else { shown.lines().map(str::to_owned).collect() };
            // (A change of the machine's settings: a table of each from
            // what it is to what it will be, where the action has one.)
            let changes = action.changes(server);
            let actor = confirm_actor(action, server, managed);
            let over = confirm_over(command, action, &actor, changes.as_deref(), lines.len());
            let well = match &changes {
                Some(changes) => changes_well(changes, in_tab, managed.confirm_id),
                None => panels::well(panels::mono_lines(lines)),
            };
            said.extend([gap(m::SPACING), small(over).into_element(), gap(m::TIGHT - 2.0), well]);
            if matches!(action, Action::Untune(olo_rex_client::tune::Tune::RexLimits)) && changes.is_some() {
                let path = action.shown(server);
                let paths = div().w_full().flex().flex_col().children(path.lines().map(|p| StringView::new(p.to_owned()).mono(true).size(panels::COMMAND_SIZE).dim(true).height(CHANGE_ROW)));
                said.extend([gap(m::TIGHT), paths.into_element()]);
            }
            // (The safe button is the one Return presses where
            // something ends by it: a server stopped under its
            // sessions. A change of settings can be put back, its
            // warning is said over it, and its own button, at the
            // right, is the one: a ring that changed sides with the
            // question made Cancel look like what was asked for,
            // Opus's look, 2026-11-27.)
            let safe = costly && !limits;
            wrap(if costly { "warning" } else { "info" }, said, button_row().child(button("Cancel", ServerDo::Back).default(safe)).child(button(action.verb(), ServerDo::Go).default(!safe)))
        }
        Stage::Working { action, said } => waiting(
            olo_kit::tr!("{} on {}…", olo_kit::words::of(action.doing()), managed.name).into(),
            match action {
                Action::Update => olo_kit::tr!("Running Rex’s installer as {}", managed.root()),
                Action::Start(_) | Action::Stop(_) | Action::Restart(_) => olo_kit::tr!("Running systemctl as {}", managed.root()),
                Action::Allow(_) | Action::Remove(_) => olo_kit::tr!("Changing {} as {}", olo_rex_client::provision::ACCOUNTS_FILE, managed.root()),
                Action::Join(_) | Action::Leave(_) => olo_kit::tr!("Changing the rex group as {}", managed.root()),
                Action::Tune(_) | Action::Untune(_) => olo_kit::tr!("Changing the machine’s settings as {}", confirm_actor(action, server, managed)),
            }
            .into(),
            Some(said.clone()),
            olo_kit::words::tr("It can’t be stopped once it has started.").to_string(),
            // (A Cancel that can't be pressed beside words that say it
            // can't be stopped said one thing twice, once with a button
            // that does nothing: Opus's look. The window keeps it, for
            // its foot's place.)
            if in_tab { div() } else { button_row().child(button("Cancel", ServerDo::Back).enabled(false)) },
        ),
        Stage::Failed { what, why, said, icon, log } => {
            let mut foot = button_row();
            if log.is_some() {
                foot = foot.child(button("Show Log", ServerDo::ShowLog));
            }
            wrap(icon, failed(managed, what, why, said, width), foot.child(button("OK", ServerDo::Back).default(true)))
        }
        _ => return None,
    })
}

/// Who writes the settings on the host for `action`: the connected account
/// for a personal-only Rex server limits change (written in that account's
/// own config directory as that account), and root otherwise.
fn confirm_actor(action: &Action, server: &Server, managed: &Managed) -> String {
    use olo_rex_client::tune::{RexScope, Tune};
    let personal_rex = matches!(action, Action::Untune(Tune::RexLimits)) || matches!(action, Action::Tune(kinds) if kinds.as_slice() == [Tune::RexLimits]);
    if personal_rex
        && server.tuning.as_ref().is_some_and(|t| t.rex_limits.scope == RexScope::User)
        && let Some(acct) = server.tuning.as_ref().and_then(|t| t.rex_limits.user.as_deref()).or(managed.target.user.as_deref()).filter(|u| !u.is_empty() && *u != "root")
    {
        return format!("{acct}@{}", managed.target.host);
    }
    managed.root()
}

/// Caption over a confirmation's well, counting the settings in its
/// table whenever more than one setting changes.
fn confirm_over(command: bool, action: &Action, root: &str, changes: Option<&[olo_rex_client::manage::Change]>, lines_len: usize) -> String {
    use olo_rex_client::tune::Tune;
    let changed = changes.map_or(0, |lines| lines.iter().filter(|line| matches!(line, olo_rex_client::manage::Change::Value { .. })).count());
    let shell_only = matches!(action, Action::Untune(Tune::Shell)) || matches!(action, Action::Tune(kinds) if kinds.as_slice() == [Tune::Shell]);
    match (command, action) {
        (true, _) => olo_kit::tr!("Rex runs this as {}:", root).into(),
        // (One of them is "this".)
        (false, Action::Tune(_)) if changed == 1 => olo_kit::tr!("Rex sets this as {}:", root).into(),
        (false, Action::Tune(_)) if changed > 1 && !shell_only => olo_kit::tr!("Rex sets these {} settings as {}:", changed, root).into(),
        (false, Action::Tune(_)) => olo_kit::tr!("Rex sets these as {}:", root).into(),
        (false, Action::Untune(_)) if changed == 1 => olo_kit::tr!("Rex sets this back as {}:", root).into(),
        (false, Action::Untune(_)) if changed > 1 && !shell_only => olo_kit::tr!("Rex sets these {} settings back as {}:", changed, root).into(),
        (false, Action::Untune(_)) if changes.is_some() => olo_kit::tr!("Rex sets these back as {}:", root).into(),
        (false, Action::Untune(_)) if lines_len == 1 => olo_kit::tr!("Rex removes this as {}:", root).into(),
        (false, Action::Untune(_)) => olo_kit::tr!("Rex removes these as {}:", root).into(),
        (false, Action::Allow(_)) => olo_kit::tr!("Rex adds these entries to {} as {}:", olo_rex_client::provision::ACCOUNTS_FILE, root).into(),
        (false, _) => olo_kit::tr!("Rex changes {} as {}:", olo_rex_client::provision::ACCOUNTS_FILE, root).into(),
    }
}

/// From the column of what a setting is now to the column of what it
/// will be; how tall a line of the table is; and the tallest a
/// confirmation's table of changes may grow before scrolling inside its
/// well (chosen so the bottom edge bisects a row in half, never looking
/// complete when more rows lie below).
const NOW_TO_NEW: f32 = 24.0;
const CHANGE_ROW: f32 = 14.0;
const CHANGES_SCROLL_AT: usize = 15;
const CHANGES_MAX_H: f32 = 237.0;

/// Returns the [`ScrollHandle`] for the current confirmation in the Server
/// window (`in_tab = false`) or Tweaks tab (`in_tab = true`), retained
/// across redraws of the same question and reset to a fresh handle at the
/// top whenever a new confirmation (`confirm_id`) opens.
fn changes_scroll(in_tab: bool, confirm_id: u64) -> olo_kit::ScrollHandle {
    thread_local! {
        static SCROLLS: std::cell::RefCell<[(u64, olo_kit::ScrollHandle); 2]> =
            std::cell::RefCell::new([(u64::MAX, olo_kit::ScrollHandle::new()), (u64::MAX, olo_kit::ScrollHandle::new())]);
    }
    SCROLLS.with(|cell| {
        let mut slots = cell.borrow_mut();
        let slot = &mut slots[usize::from(in_tab)];
        if slot.0 != confirm_id {
            *slot = (confirm_id, olo_kit::ScrollHandle::new());
        }
        slot.1.clone()
    })
}

/// The sunken well holding what a change of settings changes: a plain
/// `panels::well` when every row fits, and a flush-scrollbar scroll well
/// with an opaque pinned Now / New header and hairline divider across
/// the well's whole inner width when it scrolls.
fn changes_well(changes: &[olo_rex_client::manage::Change], in_tab: bool, confirm_id: u64) -> Element {
    use olo_rex_client::manage::Change;
    let mono = style::mono_font(olo_kit::FontWeight::NORMAL);
    // (What the host has, as it has it: a number, a path. Anything else
    // in a value's place is Olo's own word of it, "not set", "no
    // limit", "in memory", "removed", and is set as words are: in the
    // fixed-width face it read as what the host literally holds,
    // Opus's look.)
    let literal = |text: &str| text.starts_with('/') || text.chars().any(|c| c.is_ascii_digit());
    let ui = style::ui_font(olo_kit::FontWeight::NORMAL);
    let wide = |text: &str| {
        let (size, font) = if literal(text) { (panels::COMMAND_SIZE, mono.clone()) } else { (style::DETAIL_SIZE, ui.clone()) };
        f32::from(style::text_width(olo_kit::words::of(text.to_owned()), size, font)).ceil() + 1.0
    };
    let head_wide = |text: &'static str| f32::from(style::text_width(olo_kit::words::tr(text).to_owned(), style::DETAIL_SIZE, style::ui_font(olo_kit::FontWeight::NORMAL))).ceil() + 1.0;
    let values = || changes.iter().filter_map(|line| if let Change::Value { now, new, number, .. } = line { Some((now.as_str(), new.as_str(), *number)) } else { None });
    let right = values().any(|(_, _, number)| number);
    let has_rex_limits = changes.iter().any(|line| matches!(line, Change::Value { name, plain: false, .. } if olo_rex_client::tune::rex_limit_note(name).is_some()));
    let mut now_wide = values().map(|(now, _, _)| wide(now)).fold(head_wide("Now"), f32::max);
    let mut new_wide = values().map(|(_, new, _)| wide(new)).fold(head_wide("New"), f32::max);
    if has_rex_limits && changes.len() <= CHANGES_SCROLL_AT {
        // Keep Now and New at one fixed width across all four Rex confirmation
        // sheets so the columns never shift between Apply and Put Back.
        now_wide = now_wide.max(56.0);
        new_wide = new_wide.max(56.0);
    }
    let note_flag_wide = if has_rex_limits {
        let min_flag = f32::from(style::text_width("--max-sessions", panels::COMMAND_SIZE, mono.clone())).ceil() + 1.0;
        changes
            .iter()
            .filter_map(|line| match line {
                Change::Value { name, plain: false, .. } if olo_rex_client::tune::rex_limit_note(name).is_some() => {
                    Some(f32::from(style::text_width(name.clone(), panels::COMMAND_SIZE, mono.clone())).ceil() + 1.0)
                }
                _ => None,
            })
            .fold(min_flag, f32::max)
    } else {
        0.0
    };
    let cell = move |text: StringView, wide: f32| {
        let cell = div().w(px(wide)).flex_shrink_0().flex().flex_row();
        if right { cell.justify_end().child(text) } else { cell.child(text) }
    };
    // (A table's lines stand nearer than a paragraph's: all that are
    // short at once are nineteen of them, in a page that keeps its
    // height.)
    let row = CHANGE_ROW;
    let line = |name: Element, now: StringView, new: StringView| {
        div()
            .w_full()
            .h(px(row))
            .flex()
            .flex_row()
            .items_center()
            .child(div().flex_1().min_w_0().child(name))
            .child(cell(now, now_wide))
            .child(div().w(px(NOW_TO_NEW)).flex_shrink_0())
            .child(cell(new, new_wide))
            .into_element()
    };
    let words = |text: &str| StringView::new(olo_kit::words::of(text.to_owned())).size(style::DETAIL_SIZE);
    let value = |text: &str| {
        if literal(text) {
            StringView::new(text.to_owned()).mono(true).size(panels::COMMAND_SIZE)
        } else {
            // ("not set": nothing is there, said quieter.)
            words(text).dim(text == "not set")
        }
    };
    let key = |text: &str| StringView::new(text.to_owned()).mono(true).size(panels::COMMAND_SIZE);
    let apart = || div().h(px(4.0)).flex_shrink_0().into_element();
    let header = line(div(), words("Now").dim(true), words("New").dim(true));
    let mut rows = Vec::new();
    for (i, change) in changes.iter().enumerate() {
        match change {
            Change::Group(name) => {
                if i > 0 {
                    rows.push(apart());
                }
                rows.push(div().w_full().h(px(row)).flex().flex_row().items_center().child(words(name)).into_element());
            }
            Change::Value { name, now, new, plain, inside, .. } => {
                // (A tweak that is one line is a group of one: apart
                // from the lines over it as a group's name is.)
                if *plain && !*inside && i > 0 {
                    rows.push(apart());
                }
                let name_el = if !*plain && let Some(note) = olo_rex_client::tune::rex_limit_note(name) {
                    div()
                        .w_full()
                        .h(px(row))
                        .pl(px(if *inside { 12.0 } else { 0.0 }))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(20.0))
                        .child(div().w(px(note_flag_wide)).flex_shrink_0().child(key(name).height(row).beside(style::DETAIL_SIZE)))
                        .child(div().flex_1().min_w_0().child(words(note).dim(true).height(row).cut(true)))
                        .into_element()
                } else {
                    // (A setting's name the host has no one word for, "open
                    // files", is words too.)
                    let name = if *plain || name.contains(' ') { words(name) } else { key(name) }.cut(true);
                    div().pl(px(if *inside { 12.0 } else { 0.0 })).child(name).into_element()
                };
                rows.push(line(name_el, value(now), value(new)));
            }
            Change::Note(said) => rows.push(div().w_full().h(px(row)).pl(px(12.0)).flex().flex_row().items_center().child(words(said).dim(true)).into_element()),
        }
    }
    let scrolls = changes.len() > CHANGES_SCROLL_AT;
    if !scrolls && !has_rex_limits {
        // (A well's own foot is nearer its last line's descenders than its
        // head is to its first line: two points made up here. Other non-scrolling
        // tables stay as they were.)
        return panels::well(div().w_full().pb(px(2.0)).flex().flex_col().child(header).child(apart()).children(rows).into_element());
    }
    // Header band over a 1 px rule across the well's whole inner width, with
    // matching paddings (and a flush scrollbar when the table scrolls).
    let s = olo_kit::scale_factor();
    let bevel = style::line_pixels(s) * 2.0 / s;
    let rule_h = style::line_pixels(s) / s;
    let pad_x = (panels::WELL_INSET - bevel).max(0.0);
    let head_top = (5.0 - bevel).max(0.0);
    let well_fill = |p: &olo_theme::Palette| {
        if p.mode == olo_theme::ThemeMode::Dark { style::c(olo_theme::mix(p.panel_background, olo_theme::BLACK, 0.28)) } else { style::tint(p.panel_background, 1.06) }
    };
    let back = canvas(move |g, b| {
        let p = theme::palette();
        style::fill_rect(g, b, well_fill(&p));
    })
    .paint_key(0)
    .absolute()
    .inset_0();
    let head_bg = canvas(move |g, b| {
        let p = theme::palette();
        style::fill_rect(g, b, well_fill(&p));
    })
    .paint_key(0)
    .absolute()
    .inset_0();
    let head_pr = if scrolls { olo_kit::widgets::SCROLL_BAR + panels::WELL_INSET } else { pad_x };
    let head_strip = div().relative().w_full().child(head_bg).child(div().relative().w_full().pt(px(head_top)).pb(px(3.0)).pl(px(pad_x)).pr(px(head_pr)).child(header));
    let rule = canvas(|g, b| {
        let p = theme::palette();
        style::hline(g, b.left(), b.right(), b.top(), style::shade(&p, p.panel_background, 1.35));
    })
    .paint_key(0)
    .w_full()
    .h(px(rule_h))
    .flex_shrink_0();
    let body: Element = if scrolls {
        let scroll = changes_scroll(in_tab, confirm_id);
        let sv = olo_kit::widgets::ScrollView::new(&scroll).border(false).child(div().w_full().pt(px(4.0)).pb(px(6.0)).pl(px(pad_x)).pr(px(panels::WELL_INSET)).flex().flex_col().children(rows));
        div().w_full().h(px(CHANGES_MAX_H)).flex().flex_col().child(sv).into_element()
    } else {
        div().w_full().pt(px(4.0)).pb(px(6.0)).pl(px(pad_x)).pr(px(pad_x)).flex().flex_col().children(rows).into_element()
    };
    let frame = canvas(|g, b| {
        style::sunken(g, b, &theme::palette(), false);
    })
    .paint_key(0)
    .absolute()
    .inset_0();
    div().relative().w_full().child(back).child(div().relative().w_full().p(px(bevel)).flex().flex_col().child(head_strip).child(rule).child(body)).child(frame).into_element()
}

/// What didn't go: in a few words, how things stand, and what the host
/// said, as it said it, in a well.
fn failed(managed: &Managed, what: &str, why: &str, said: &str, width: f32) -> Vec<Element> {
    let gap = |h: f32| div().h(px(h)).flex_shrink_0().into_element();
    // (How many letters a line of the alert's well holds.)
    let room = panels::command_room(width - olo_kit::widgets::ALERT_TEXT_X - m::WINDOW_INSET);
    let mut body = vec![alert_title(what.to_owned()).into_element()];
    if !why.is_empty() {
        body.extend([gap(m::TIGHT), StringView::new(why.to_owned()).wrap(true).into_element()]);
    }
    if !said.is_empty() {
        body.extend([
            gap(m::SPACING),
            // (A host that couldn't be looked at said nothing: what
            // is in the well is this computer's word of why it
            // wasn't reached. "dev-box said: ssh: connect to host
            // dev-box … timed out", Gemini's look.)
            StringView::new(if managed.server.is_some() { SharedString::from(olo_kit::tr!("{} said:", managed.name)) } else { olo_kit::words::tr("What went wrong:").into() })
                .size(style::SMALL_SIZE)
                .dim(true)
                .wrap(true)
                .into_element(),
            gap(m::TIGHT - 2.0),
            // (A line a line, as a command's well, the rest of a long
            // one set in: one paragraph of them sat nearer the well's
            // foot than its top, and its second line read as another
            // message.)
            panels::well(panels::mono_lines(said.lines().flat_map(|line| hung(line, room)).collect())),
        ]);
    }
    body
}

/// A section's heading over its rule, as Add Host's groups of steps
/// have theirs.
fn section(title: &'static str) -> Element {
    let gap = |h: f32| div().h(px(h)).flex_shrink_0();
    // (Looked up in the person's language: a `String` made of the literal
    // was shown as it is.)
    div().w_full().flex().flex_col().child(StringView::new(olo_kit::words::tr(title).to_owned()).bold(true).height(m::LABEL_ROW)).child(gap(2.0)).child(separator()).into_element()
}

/// The Server page: each service in use (both, where neither is), then
/// Rex's version.
fn server_page(managed: &Managed, server: &Server, button: &dyn Fn(&'static str, ServerDo) -> Button, small: &dyn Fn(String) -> StringView) -> Vec<Element> {
    let p = theme::palette();
    let row = |lamp: Option<Light>, words: String, actions: Vec<Button>| {
        let mut said = div().flex_1().min_w_0().flex().flex_row().items_center().gap(px(m::TIGHT));
        if let Some(lamp) = lamp {
            said = said.child(panels::light(lamp));
        }
        div().w_full().flex().flex_row().items_center().gap(px(m::SPACING)).child(said.child(div().flex_1().min_w_0().child(StringView::new(words).cut(true)))).children(actions)
    };
    // (Its heading and rule; its line and buttons a step under; its
    // notes under those, clear of the buttons: what it costs last, in
    // the warning's colour.)
    let block = |title: &'static str, line: Element, note: String, cost: Option<String>| {
        let mut notes = div().w_full().flex().flex_col().gap(px(2.0)).child(small(note));
        if let Some(cost) = cost {
            notes = notes.child(StringView::new(cost).size(style::SMALL_SIZE).color(style::c(p.warning)).wrap(true));
        }
        div().w_full().flex().flex_col().child(section(title)).child(div().h(px(m::TIGHT)).flex_shrink_0()).child(line).child(div().h(px(m::TIGHT + 2.0)).flex_shrink_0()).child(notes).into_element()
    };
    // (The page's width inside the tab view's panel and its own margins.)
    let wide = SERVER_WIDTH - 4.0 * m::WINDOW_INSET - 2.0 * m::SPACING;
    let mut page = Vec::new();
    if server.mode().is_none() && !server.failed(Service::Login) && !server.failed(Service::Shared) {
        page.push(StringView::new(olo_kit::tr!("Neither kind of Rex server is in use on {}. Start one of them:", managed.name)).wrap(true).into_element());
    }
    for service in [Service::Login, Service::Shared] {
        if !server.state(service).used() && !server.failed(service) && server.mode().is_some() {
            continue;
        }
        // Neither kind in use (the line over them says so): each as
        // what it is, with Start… beside it, and no line of its own
        // that says "Not in use" a second and a third time. (With
        // those, this page alone was taller than the frame: the
        // fourth design review.)
        if server.mode().is_none() && !server.failed(service) {
            let what: String = match service {
                Service::Login => olo_kit::words::tr("Each person gets their own Rex server, and signs in through this one.").into(),
                Service::Shared => olo_kit::words::tr("Everyone in the rex group shares one server, run as the rex account.").into(),
            };
            let line =
                div().w_full().flex().flex_row().items_center().gap(px(m::SPACING)).child(div().flex_1().min_w_0().child(small(what))).child(button("Start…", ServerDo::Ask(Action::Start(service))));
            page.push(div().w_full().flex().flex_col().child(section(service.name())).child(div().h(px(m::TIGHT)).flex_shrink_0()).child(line).into_element());
            continue;
        }
        let (lamp, words) = service_state(managed, server, service);
        let actions = if server.state(service).active {
            vec![button("Restart…", ServerDo::Ask(Action::Restart(service))), button("Stop…", ServerDo::Ask(Action::Stop(service)))]
        } else if server.failed(service) {
            vec![button("Show Log", ServerDo::LogOf(service)), button("Start…", ServerDo::Ask(Action::Start(service)))]
        } else {
            vec![button("Start…", ServerDo::Ask(Action::Start(service)))]
        };
        let (note, cost) = service_notes(managed, server, service);
        // (A failed one's last words under its line, from under its
        // words: its own, in the log's face, on two lines at most.)
        let line = row(Some(lamp), words, actions);
        let line = match server.last_words(service).filter(|_| server.failed(service)) {
            Some(last) => {
                let room = wide - panels::LIGHT - m::TIGHT;
                let per_line = (room / (f32::from(style::text_width("0000000000", style::SMALL_SIZE, style::mono_font(olo_kit::FontWeight::NORMAL))) / 10.0)).floor() as usize;
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(m::TIGHT))
                    .child(line)
                    .child(
                        div()
                            .w_full()
                            .pl(px(panels::LIGHT + m::TIGHT))
                            .flex()
                            .flex_col()
                            .children(last_words(last, per_line).into_iter().map(|line| StringView::new(line).mono(true).size(style::SMALL_SIZE).dim(true).wrap(true))),
                    )
                    .into_element()
            }
            None => line.into_element(),
        };
        page.push(block(service.name(), line, note, cost));
    }
    // Rex itself: the version there, and whether its repository has a
    // newer one (asked as the window opened, where the host's package
    // manager could be).
    let version = server.version();
    let asked = |checked: bool| -> String {
        if checked {
            olo_kit::tr!("{}’s package manager checked Rex’s repository when this window opened.", managed.name).into()
        } else {
            olo_kit::tr!("As {}’s package manager last heard from Rex’s repository.", managed.name).into()
        }
    };
    let (line, note, can): (String, String, bool) = match (&version, &server.newer, server.checked) {
        (None, _, _) => (olo_kit::words::tr("Rex isn’t installed").into(), olo_kit::words::tr("Update installs it, with Rex’s installer.").into(), true),
        (Some(version), Some(newer), checked) => (olo_kit::tr!("{} is installed  ·  {} is available", version, newer).into(), asked(checked), true),
        (Some(version), None, true) => (olo_kit::tr!("{} is installed  ·  up to date", version).into(), asked(true), false),
        (Some(version), None, false) => {
            (olo_kit::tr!("{} is installed", version).into(), olo_kit::words::tr("Update asks Rex’s repository for a newer version, and installs it if there is one.").into(), true)
        }
    };
    page.push(block("Version", row(None, line, vec![button("Update…", ServerDo::Ask(Action::Update)).enabled(can)]), note, None));
    // (A section's step between them, as Add Host's groups of steps
    // have: each heading plainly over its own.)
    vec![div().w_full().flex().flex_col().gap(px(m::SECTION - m::TIGHT)).children(page).into_element()]
}

/// The Accounts page (a login server's) or the Members page (a shared
/// server's): who may use it; Allow… (or Add…) for somebody else of the
/// machine's, chosen in the alert that asks; Remove… for the one chosen.
#[allow(clippy::too_many_arguments)]
fn people_page(
    cx: &olo_kit::Cx<crate::ui::RexWindow>,
    managed: &Managed,
    server: &Server,
    list: &ListModel,
    rows: &Rc<Vec<PersonRow>>,
    chosen: Option<usize>,
    button: &dyn Fn(&'static str, ServerDo) -> Button,
    small: &dyn Fn(String) -> StringView,
    on_chosen: impl Fn(&(), &mut WindowCx) + 'static,
) -> Vec<Element> {
    let members = managed.shared();
    // (The list takes what the page's other rows leave: its columns to
    // its width, less its rim and its bar. Who else may use an account
    // has a column only where some account is used by another than
    // itself: a tailnet's people, say.)
    let wide = SERVER_WIDTH - 4.0 * m::WINDOW_INSET - 2.0 * m::SPACING - LIST_EDGE;
    let others = rows.iter().any(|r| !r.who.is_empty());
    let columns = match (members, others) {
        (true, _) => vec![Column::new("Account", wide)],
        (false, false) => vec![Column::new("Account", 200.0), Column::new("Sessions", wide - 200.0)],
        (false, true) => vec![Column::new("Account", 130.0), Column::new("Also used by", wide - 130.0 - 90.0), Column::new("Sessions", 90.0)],
    };
    let names = rows.clone();
    let people = ColumnListView::new(cx, "rex-server-people", list, columns, rows.len(), move |v: &crate::ui::RexWindow, cell, g| {
        let Some(r) = v.server_rows.get(cell.row) else { return };
        match (cell.column, others) {
            (0, _) => cell.text(g, r.shown.clone()),
            (1, true) => cell.text(g, r.who.clone()),
            _ => cell.text(g, r.sessions.clone()),
        }
    })
    .row_key(|v: &crate::ui::RexWindow, row| v.server_rows.get(row).map_or(0, |r| style::key((&r.name, &r.shown, &r.who, &r.sessions))))
    .type_select(move |_, i| names.get(i).map(|r| r.shown.clone().into()).unwrap_or_default())
    .empty_text(match (&server.accounts_trouble, members) {
        (Some(_), false) => olo_kit::words::tr("The accounts file couldn’t be read.").to_string(),
        (_, true) => olo_kit::words::tr("Nobody is in the rex group.").to_string(),
        _ => olo_kit::words::tr("No account is listed: nobody can sign in through the login server.").to_string(),
    })
    .on_select(on_chosen);
    let could = !managed.could_join().is_empty();
    let picked = chosen.and_then(|i| rows.get(i));
    let remove = match picked {
        Some(row) if members => button("Remove…", ServerDo::Ask(Action::Leave(row.name.clone()))),
        Some(row) => button("Remove…", ServerDo::Ask(Action::Remove(row.name.clone()))),
        None => button("Remove…", ServerDo::Back).enabled(false),
    };
    let actions = div().w_full().flex().flex_row().items_center().gap(px(m::SPACING)).child(button(if members { "Add…" } else { "Allow…" }, ServerDo::LetIn).enabled(could)).child(remove);
    let note: String = match (&server.accounts_trouble, members, could) {
        (Some(why), false, _) => why.clone(),
        (_, true, true) => olo_kit::words::tr("Members of the rex group use the shared server, where every terminal runs as the rex account. Changes take effect the next time they sign in.").into(),
        (_, true, false) => olo_kit::tr!("Everyone with an account on {} is in the rex group, and uses the shared server.", managed.name).into(),
        (_, false, true) => olo_kit::tr!("Accounts that may sign in through the login server, kept in {}.", olo_rex_client::provision::ACCOUNTS_FILE).into(),
        (_, false, false) => olo_kit::tr!("Everyone with an account on {} may sign in. The list is kept in {}.", managed.name, olo_rex_client::provision::ACCOUNTS_FILE).into(),
    };
    vec![div().w_full().flex_1().min_h(px(olo_kit::widgets::HEADER_HEIGHT + 4.0 * m::LIST_ROW)).child(people).into_element(), actions.into_element(), small(note).into_element()]
}

/// A day of the log as its heading says it: "Sunday 22 November"
/// (the day as the host has it; its ISO form where it isn't one).
fn day_said(day: &str) -> String {
    match day.parse::<jiff::civil::Date>() {
        Ok(date) => date.strftime("%A %-d %B").to_string(),
        Err(_) => day.to_owned(),
    }
}

/// The log as its view shows it: its text, and the runs that set it.
/// Each day under a heading (the plain face, bold, quieter, a line's
/// room over every one but the first: set like the lines, a day's line
/// was lost among them), each line in the monospace, the word that
/// says a line warns or reports an error in the warning's and the
/// failure's colours (a refused sign-in is found by its colour).
pub fn log_text(lines: &[String]) -> (String, Vec<olo_kit::TextRun>) {
    let p = theme::palette();
    let run = |len: usize, font: olo_kit::Font, color: olo_kit::Hsla| olo_kit::TextRun { len, font, color, background_color: None, underline: None, strikethrough: None, size: None };
    let (mono, plain) = (style::mono_font(olo_kit::FontWeight::NORMAL), style::c(p.document_text));
    let mut text = String::new();
    let mut runs: Vec<olo_kit::TextRun> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let end = if i + 1 < lines.len() { "\n" } else { "" };
        if olo_rex_client::manage::is_day(line) {
            let said = format!("{}{}{end}", if text.is_empty() { "" } else { "\n" }, day_said(line));
            runs.push(run(said.len(), style::ui_font(olo_kit::FontWeight::BOLD), style::c(olo_theme::mix(p.document_text, p.document_background, 0.35))));
            text.push_str(&said);
            continue;
        }
        // (After its time: the level, where it begins with one.)
        let from = line.find(' ').map_or(0, |at| at + 1);
        let level = [("WARN", p.warning), ("WARNING", p.warning), ("ERROR", p.failure), ("FATAL", p.failure), ("error:", p.failure)]
            .into_iter()
            .filter(|(word, _)| line[from..].starts_with(word) && line[from + word.len()..].chars().next().is_none_or(|c| c == ' '))
            .max_by_key(|(word, _)| word.len());
        match level {
            Some((word, color)) => {
                runs.push(run(from, mono.clone(), plain));
                runs.push(run(word.len(), style::mono_font(olo_kit::FontWeight::BOLD), style::c(color)));
                runs.push(run(line.len() - from - word.len() + end.len(), mono.clone(), plain));
            }
            None => runs.push(run(line.len() + end.len(), mono.clone(), plain)),
        }
        text.push_str(line);
        text.push_str(end);
    }
    (text, runs)
}

/// For how long the log shown has been the one read: "just now", then
/// by the minute (it isn't followed as it is written: Refresh reads it
/// again).
fn read_ago(age: Duration) -> String {
    match age.as_secs() / 60 {
        0 => olo_kit::words::tr("Read just now").into(),
        minutes if minutes < 60 => olo_kit::tr!("Read {} min ago", minutes).into(),
        minutes => olo_kit::tr!("Read {} h ago", minutes / 60).into(),
    }
}

/// The Log page: which of the log, when it was read, Copy and Refresh,
/// over what the service wrote (the newest last, and shown first).
fn log_page(scale: f32, managed: &Managed, log: Option<Element>, button: &dyn Fn(&'static str, ServerDo) -> Button, on_range: impl Fn(&usize, &mut WindowCx) + 'static) -> Vec<Element> {
    let ranges: Vec<SharedString> = LogRange::ALL.iter().map(|r| SharedString::from(r.name())).collect();
    let at = LogRange::ALL.iter().position(|r| *r == managed.log_range).unwrap_or(0);
    let has = managed.log.as_ref().is_some_and(|lines| !lines.is_empty());
    let read: String = match (managed.log_reading, managed.log_read) {
        // (While it is read, the view says so: once.)
        (true, _) => String::new(),
        (false, Some(at)) => olo_kit::ago::age(at.elapsed(), read_ago),
        (false, None) => String::new(),
    };
    let top = div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::SPACING))
        .child(MenuField::new("server-log-range", ranges, at).label("Show:").on_change(on_range))
        .child(div().flex_1())
        // (Against the buttons it speaks of, in the controls' own size: on their line.)
        .child(StringView::new(read).dim(true))
        .child(button("Copy", ServerDo::Copy).enabled(has))
        .child(button("Refresh", ServerDo::Refresh).enabled(!managed.log_reading));
    let mut page = vec![top.into_element()];
    if let Some(log) = log {
        // (Its frame's two lines, on device pixels, as a list's.)
        let rim = 2.0 * style::line_pixels(scale) / scale;
        page.push(div().w_full().h(px(log_room() + 2.0 * rim)).flex_shrink_0().flex().flex_col().child(log).into_element());
    }
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_is_set_by_its_days_and_its_levels() {
        let lines: Vec<String> = ["2026-11-22", "14:03:11 INFO  login server starting", "16:12:55 WARN  sign-in refused", "2026-11-23", "00:00:14 ERROR it ended", "00:00:15 ERRORS aren't a level"]
            .iter()
            .map(|l| (*l).to_owned())
            .collect();
        let (text, runs) = log_text(&lines);
        assert_eq!(text, "Sunday 22 November\n14:03:11 INFO  login server starting\n16:12:55 WARN  sign-in refused\n\nMonday 23 November\n00:00:14 ERROR it ended\n00:00:15 ERRORS aren't a level");
        // The runs cover the text, to the byte (the view takes no others).
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), text.len());
        // A day's heading, a plain line, a line with its level apart, the next day's heading with its room over it.
        let lens: Vec<usize> = runs.iter().map(|r| r.len).collect();
        assert_eq!(lens, [19, 37, 9, 4, 18, 20, 9, 5, 10, 30]);
        assert_ne!(runs[3].color, runs[2].color);
        assert_ne!(runs[7].color, runs[3].color);
        assert_eq!(day_said("not a day"), "not a day");
        assert!(log_text(&[]).0.is_empty());
    }

    #[test]
    fn how_long_ago_the_log_was_read_changes_by_the_minute() {
        assert_eq!(read_ago(Duration::from_secs(59)), "Read just now");
        assert_eq!(read_ago(Duration::from_secs(60)), "Read 1 min ago");
        assert_eq!(read_ago(Duration::from_secs(3 * 3600 + 5)), "Read 3 h ago");
        // A failure, in the whole words a running time is said in.
        assert_eq!(failed_ago(Duration::from_secs(30)), "Failed just now");
        assert_eq!(failed_ago(Duration::from_secs(250)), "Failed 4 minutes ago");
        assert_eq!(failed_ago(Duration::from_secs(3600)), "Failed 1 hour ago");
        // Its last words: whole where two lines hold them, else cut at a word.
        assert_eq!(last_words("the rex account has no shell", 40), ["the rex account has no shell"]);
        let long = "the accounts file is not one: line 12, a comma is missing after the account alice, before the account bo";
        assert_eq!(last_words(long, 40), ["the accounts file is not one: line 12, a", "  comma is missing after the account…"]);
        assert!(last_words(long, 40).iter().all(|line| line.chars().count() <= 40));
        // A long line written out: broken at a space, the rest set in.
        assert_eq!(hung("Job for rex.service failed because the control process exited with error code.", 44), ["Job for rex.service failed because the", "  control process exited with error code."]);
        assert_eq!(hung("short", 44), ["short"]);
    }

    /// The pages' least height is the Log page's own: its controls, a
    /// step, and a view of whole lines.
    #[test]
    fn the_log_shows_whole_lines() {
        let line = TextView::line_height_at(LOG_SIZE);
        assert_eq!(line.fract(), 0.0);
        assert_eq!((log_room() - 2.0 * TextView::MARGINS.1 - olo_kit::widgets::SCROLL_BAR) / line, LOG_LINES);
        assert_eq!(frame_height() - page_height(), 4.0 * m::WINDOW_INSET + TAB_HEIGHT);
    }

    #[test]
    fn confirm_caption_counts_settings_from_the_table() {
        use olo_rex_client::manage::Change;
        use olo_rex_client::tune::Tune;
        let val = |name: &str| Change::Value { name: name.into(), now: "1".into(), new: "2".into(), plain: false, inside: false, number: true };
        let one = [val("MaxSessions")];
        assert_eq!(confirm_over(false, &Action::Tune(vec![Tune::Sessions]), "root@dev-box.example", Some(&one), 1), "Rex sets this as root@dev-box.example:");
        let four = [val("--max-windows"), val("--max-panes"), val("--max-sessions"), val("--max-blocks")];
        assert_eq!(confirm_over(false, &Action::Tune(vec![Tune::RexLimits]), "root@dev-box.example", Some(&four), 4), "Rex sets these 4 settings as root@dev-box.example:");
        assert_eq!(confirm_over(false, &Action::Untune(Tune::RexLimits), "root@dev-box.example", Some(&four), 2), "Rex sets these 4 settings back as root@dev-box.example:");
        // Groups are not counted as settings.
        let grouped = [Change::Group("A".into()), val("a"), val("b"), Change::Group("B".into()), val("c")];
        assert_eq!(confirm_over(false, &Action::Tune(vec![Tune::Kernel, Tune::Memory]), "root@dev-box.example", Some(&grouped), 5), "Rex sets these 3 settings as root@dev-box.example:");
        // Shell lists accounts rather than numeric settings.
        assert_eq!(confirm_over(false, &Action::Untune(Tune::Shell), "root@dev-box.example", Some(&four), 2), "Rex sets these back as root@dev-box.example:");

        // Finding 22: personal-only RexLimits Apply and Put Back name the account, not root
        // (even under sudo), while shared RexLimits and multi-tweak batches keep root.
        let mut user_managed = crate::server::demo("confirm-rex-user").unwrap();
        let user_server = user_managed.server.as_ref().unwrap();
        assert_eq!(confirm_actor(&Action::Tune(vec![Tune::RexLimits]), user_server, &user_managed), "alice@build.example");
        assert_eq!(confirm_actor(&Action::Untune(Tune::RexLimits), user_server, &user_managed), "alice@build.example");
        assert_eq!(confirm_actor(&Action::Tune(vec![Tune::Sessions, Tune::RexLimits]), user_server, &user_managed), "root@build.example");
        user_managed.root_by = Some(olo_rex_client::provision::RootBy::Sudo);
        assert_eq!(confirm_actor(&Action::Tune(vec![Tune::RexLimits]), user_server, &user_managed), "alice@build.example");
        assert_eq!(confirm_actor(&Action::Tune(vec![Tune::Sessions]), user_server, &user_managed), "root, with sudo as alice");

        let shared_managed = crate::server::demo("confirm-rex").unwrap();
        let shared_server = shared_managed.server.as_ref().unwrap();
        assert_eq!(confirm_actor(&Action::Tune(vec![Tune::RexLimits]), shared_server, &shared_managed), "root@build.example");
        assert_eq!(confirm_actor(&Action::Untune(Tune::RexLimits), shared_server, &shared_managed), "root@build.example");

        // Personal Put Back demo lists & counts only the 2 differing settings as alice@build.example.
        let pb_user = crate::server::demo("confirm-put-back-rex-user").unwrap();
        let pb_server = pb_user.server.as_ref().unwrap();
        let pb_action = Action::Untune(Tune::RexLimits);
        let pb_changes = pb_action.changes(pb_server).unwrap();
        assert_eq!(pb_changes.len(), 2);
        let pb_actor = confirm_actor(&pb_action, pb_server, &pb_user);
        assert_eq!(confirm_over(false, &pb_action, &pb_actor, Some(&pb_changes), 1), "Rex sets these 2 settings back as alice@build.example:");
    }

    #[test]
    fn each_confirmation_gets_its_own_scroll_handle_reset_at_top() {
        let a1 = changes_scroll(true, 101);
        let a2 = changes_scroll(true, 101);
        assert_eq!(a1.id(), a2.id(), "redraws of the same confirmation keep one handle");
        let b = changes_scroll(true, 102);
        assert_ne!(a1.id(), b.id(), "a new confirmation gets a fresh handle");
        assert_eq!(b.offset(), olo_kit::Point::default(), "a new confirmation starts at top");
        let srv = changes_scroll(false, 102);
        assert_ne!(b.id(), srv.id(), "Server window and Tweaks tab keep independent handles");
    }
}
