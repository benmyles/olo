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
    Element, IntoElement as _, ParentElement as _, SharedString, Styled as _, WindowCx, div, metrics as m, px, style, theme,
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
    let p = theme::palette();
    let on_do: Rc<dyn Fn(&ServerDo, &mut WindowCx)> = Rc::new(on_do);
    let button = |label: &str, what: ServerDo| {
        let on_do = on_do.clone();
        Button::new(label.to_owned()).min_width(m::BUTTON_MIN_WIDTH).on_click(move |_, cx| on_do(&what, cx))
    };
    let body = |text: String| StringView::new(text).wrap(true).into_element();
    let small = |text: String| StringView::new(text).size(style::SMALL_SIZE).dim(true).wrap(true);
    let amber = |text: String| StringView::new(text).color(style::c(p.warning)).wrap(true).into_element();
    let gap = |h: f32| div().h(px(h)).flex_shrink_0().into_element();
    // (The alert's column: from past its icon to the window's inset.)
    let column = SERVER_WIDTH - olo_kit::widgets::ALERT_TEXT_X - m::WINDOW_INSET;
    // Waiting, and working: a heading, the bar with its words, a line.
    let waiting = |heading: String, bar: StatusBar, under: StringView, note: Option<String>, foot: Element| {
        let mut body =
            vec![alert_title(heading).into_element(), gap(m::TIGHT), bar.into_element(), gap(m::TIGHT), under.size(style::SMALL_SIZE).dim(true).cut(true).height(m::LABEL_ROW).into_element()];
        if let Some(note) = note {
            body.extend([gap(m::SPACING), small(note).into_element()]);
        }
        sheet("rex", body, foot)
    };
    let Some(server) = managed.server.as_ref().filter(|_| !matches!(managed.stage, Stage::Looking | Stage::RootKey { .. })) else {
        // No server read yet (or root's key to be chosen first).
        return match &managed.stage {
            Stage::RootKey { keys, .. } if keys.is_empty() => sheet(
                "key",
                vec![
                    alert_title("Olo Passwords has no SSH keys to offer").into_element(),
                    gap(m::TIGHT),
                    body(olo_kit::tr!("Looking after a server needs root. Unlock Olo Passwords, or add a key to it that root on {} accepts.", managed.name).into()),
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
            Stage::RootKey { keys, chosen, .. } => sheet(
                "key",
                vec![
                    alert_title(olo_kit::tr!("Root on {} didn’t accept your keys", managed.name)).into_element(),
                    gap(m::TIGHT),
                    body(
                        olo_kit::words::tr(
                            "Looking after a server needs root. Choose a key that root accepts there. If none does, add your public key to /root/.ssh/authorized_keys on that machine, then try again.",
                        )
                        .into(),
                    ),
                    gap(m::SPACING),
                    MenuField::new("server-root-key", keys.iter().map(panels::key_called).collect(), *chosen).label("Key:").on_change(on_key).into_element(),
                ],
                button_row().child(button("Cancel", ServerDo::Close)).child(button("Try Again", ServerDo::Again).default(true)),
            ),
            Stage::Failed { what, why, said, icon, .. } => {
                sheet(icon, failed(managed, what, why, said), button_row().child(button("Close", ServerDo::Close)).child(button("Try Again", ServerDo::Again).default(true)))
            }
            _ => waiting(
                olo_kit::tr!("Connecting to {}…", managed.name).into(),
                StatusBar::busy(phase).label(olo_kit::tr!("Signing in as {} with a key from Olo Passwords", managed.root())),
                StringView::new(olo_kit::words::tr("Nothing is changed until you choose something to do.").to_string()),
                None,
                button_row().child(button("Cancel", ServerDo::Close)),
            ),
        };
    };
    match &managed.stage {
        Stage::Confirm { action, choose } => {
            let warning = action.warning(server);
            let what = action.what(server);
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
                said.extend([gap(m::TIGHT), amber(olo_kit::words::of(warning).to_string())]);
            }
            if !what.is_empty() {
                said.extend([gap(m::TIGHT), body(olo_kit::words::of(what).to_string())]);
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
            let shown = action.shown(server);
            let command = action.command(server).is_some();
            let lines = if command { panels::command_lines(&shown, panels::command_room(column)) } else { shown.lines().map(str::to_owned).collect() };
            let over: String = match (command, action) {
                (true, _) => olo_kit::tr!("Rex runs this as {}:", managed.root()).into(),
                (false, Action::Allow(_)) => olo_kit::tr!("Rex adds these entries to {} as {}:", olo_rex_client::provision::ACCOUNTS_FILE, managed.root()).into(),
                (false, _) => olo_kit::tr!("Rex changes {} as {}:", olo_rex_client::provision::ACCOUNTS_FILE, managed.root()).into(),
            };
            said.extend([gap(m::SPACING), small(over).into_element(), gap(m::TIGHT - 2.0), panels::well(panels::mono_lines(lines))]);
            // (The safe button is the one Return presses where
            // something ends by it.)
            sheet(if costly { "warning" } else { "info" }, said, button_row().child(button("Cancel", ServerDo::Back).default(costly)).child(button(action.verb(), ServerDo::Go).default(!costly)))
        }
        Stage::Working { action, said } => waiting(
            olo_kit::tr!("{} on {}…", olo_kit::words::of(action.doing()), managed.name).into(),
            StatusBar::busy(phase).label(match action {
                Action::Update => olo_kit::tr!("Running Rex’s installer as {}", managed.root()),
                Action::Start(_) | Action::Stop(_) | Action::Restart(_) => olo_kit::tr!("Running systemctl as {}", managed.root()),
                Action::Allow(_) | Action::Remove(_) => olo_kit::tr!("Changing {} as {}", olo_rex_client::provision::ACCOUNTS_FILE, managed.root()),
                Action::Join(_) | Action::Leave(_) => olo_kit::tr!("Changing the rex group as {}", managed.root()),
            }),
            StringView::new(if said.is_empty() { " ".to_owned() } else { said.clone() }).mono(true),
            Some(olo_kit::words::tr("It can’t be stopped once it has started.").into()),
            button_row().child(button("Cancel", ServerDo::Back).enabled(false)),
        ),
        Stage::Failed { what, why, said, icon, log } => {
            let mut foot = button_row();
            if log.is_some() {
                foot = foot.child(button("Show Log", ServerDo::ShowLog));
            }
            sheet(icon, failed(managed, what, why, said), foot.child(button("OK", ServerDo::Back).default(true)))
        }
        _ => {
            let people = if managed.shared() { "Members" } else { "Accounts" };
            let tabs: Vec<SharedString> = ["Server", people, "Log"].into_iter().map(SharedString::from).collect();
            let at = Page::ALL.iter().position(|p| *p == managed.page).unwrap_or(0);
            let page: Vec<Element> = match managed.page {
                Page::Server => server_page(managed, server, &button, &small),
                Page::People => people_page(cx, managed, server, list, rows, chosen, &button, &small, on_chosen),
                Page::Log => log_page(cx.scale_factor(), managed, log, &button, on_range),
            };
            div().w_full().flex().flex_col().p(px(m::WINDOW_INSET)).child(
                TabView::new("rex-server-tabs", tabs, at)
                    .grows()
                    .on_select(on_page)
                    .child(div().flex_1().min_h(px(page_height())).flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING)).children(page)),
            )
        }
    }
}

/// What didn't go: in a few words, how things stand, and what the host
/// said, as it said it, in a well.
fn failed(managed: &Managed, what: &str, why: &str, said: &str) -> Vec<Element> {
    let gap = |h: f32| div().h(px(h)).flex_shrink_0().into_element();
    // (How many letters a line of the alert's well holds.)
    let room = panels::command_room(SERVER_WIDTH - olo_kit::widgets::ALERT_TEXT_X - m::WINDOW_INSET);
    let mut body = vec![alert_title(what.to_owned()).into_element()];
    if !why.is_empty() {
        body.extend([gap(m::TIGHT), StringView::new(why.to_owned()).wrap(true).into_element()]);
    }
    if !said.is_empty() {
        body.extend([
            gap(m::SPACING),
            StringView::new(olo_kit::tr!("{} said:", managed.name)).size(style::SMALL_SIZE).dim(true).wrap(true).into_element(),
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
fn section(title: &str) -> Element {
    let gap = |h: f32| div().h(px(h)).flex_shrink_0();
    div().w_full().flex().flex_col().child(StringView::new(title.to_owned()).bold(true).height(m::LABEL_ROW)).child(gap(2.0)).child(separator()).into_element()
}

/// The Server page: each service in use (both, where neither is), then
/// Rex's version.
fn server_page(managed: &Managed, server: &Server, button: &dyn Fn(&str, ServerDo) -> Button, small: &dyn Fn(String) -> StringView) -> Vec<Element> {
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
    let block = |title: &str, line: Element, note: String, cost: Option<String>| {
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
    button: &dyn Fn(&str, ServerDo) -> Button,
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
fn log_page(scale: f32, managed: &Managed, log: Option<Element>, button: &dyn Fn(&str, ServerDo) -> Button, on_range: impl Fn(&usize, &mut WindowCx) + 'static) -> Vec<Element> {
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
}
