//! The Account window: a far host's account made ready for work with
//! agents. A head as the Host window's (whose account, how much of it
//! is ready), then what it is given in groups, each a heading and its
//! rows (`account::GROUPS`): a row is a light, a name, how it stands
//! and the button that does it, with a line under it that says what it
//! is, or, while it is being done, what its installer wrote last, or
//! what the host said when it couldn't be. Check Again and the one
//! button for all that is left are at the foot. (Both reviewers saw it
//! early, 2026-11-23: it had a paragraph for a head, the button for
//! all of it among the rows' own, an installer's line where a row's
//! standing is said, and no groups to grow by.)

use std::rc::Rc;

use olo_kit::{
    Element, IntoElement as _, ParentElement as _, Styled as _, WindowCx, canvas, div, icons, metrics as m, px, style,
    widgets::{Button, StringView, button_width, separator},
};
use olo_rex_client::ready::Step;

use crate::{
    account::{GROUPS, Item, Shown, tally},
    hub::{HostConnState, HostStands},
    panels::{self, Light},
};

/// The window's width: the Host window's, which opens it.
pub const ACCOUNT_WIDTH: f32 = crate::host_panel::HOST_WIDTH;

/// The head's sizes: the Host window's.
const ICON: f32 = 32.0;
const NAME_SIZE: f32 = style::PLAIN_SIZE + 3.0;
const NAME_ROW: f32 = 20.0;
const STATE_ROW: f32 = 14.0;
/// What a host wrote, under its row: a point smaller than a note.
const MONO_SIZE: f32 = style::SMALL_SIZE - 1.0;

/// What the window's buttons do.
#[derive(Clone, Debug, PartialEq)]
pub enum AccountDo {
    /// These done, one after another.
    Run(Vec<Item>),
    /// The skill taken off.
    RemoveSkill,
    /// What is being done stopped, and what waits let go.
    Stop,
    /// The host asked again how the account stands.
    Check,
    /// A host that isn't connected, connected to.
    Connect,
}

/// What stands where a row's light does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lamp {
    Light(Light),
    /// Being done: a ring in the waiting light's colour.
    Doing,
    /// Waits its turn: the same ring, grey.
    Queued,
}

/// What a row's button does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Press {
    Run,
    RemoveSkill,
}

/// How an item stands, on its row.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub lamp: Lamp,
    /// After its name: how it stands, in a word or two.
    pub said: String,
    /// Under it, in place of what it is: what its installer wrote
    /// last, or what the host said when it couldn't be done.
    pub host_said: Option<String>,
    /// Under it, in place of what it is, in the window's own words
    /// (git's name there and here, where they differ).
    pub note: Option<String>,
    /// Its button: its word, what it does, and whether it has
    /// something to do now. None: no button.
    pub button: Option<(&'static str, Press, bool)>,
}

/// The word on an item's button, for one that isn't there.
fn verb(item: Item) -> &'static str {
    match item {
        Item::Step(Step::Path) => "Add",
        Item::Step(Step::Git) => "Set",
        Item::Carry => "Copy",
        _ => "Install",
    }
}

/// How `item` stands on the window's host.
pub fn row(host: &HostStands, shown: &Shown, item: Item) -> Row {
    use olo_kit::words::tr;
    let reached = host.conn == HostConnState::Connected && host.ssh;
    let can = reached && !shown.busy();
    let plain = |lamp, said: &'static str, button| Row { lamp, said: if said.is_empty() { String::new() } else { tr(said).into() }, host_said: None, note: None, button };
    let set = matches!(item, Item::Step(Step::Path | Step::Git));
    if let Some(doing) = shown.doing(item) {
        let said = if doing.removing {
            "Removing…"
        } else if item == Item::Carry {
            "Copying…"
        } else if set {
            "Setting…"
        } else {
            "Installing…"
        };
        return Row { host_said: Some(doing.line.clone()).filter(|line| !line.is_empty()), ..plain(Lamp::Doing, said, None) };
    }
    if shown.queue.contains(&item) {
        return plain(Lamp::Queued, "Waiting", None);
    }
    if let Some(said) = shown.failed(item) {
        let what = match item {
            Item::Carry => "Couldn’t be copied",
            _ if set => "Couldn’t be set",
            _ => "Couldn’t be installed",
        };
        return Row { host_said: Some(said.to_owned()), ..plain(Lamp::Light(Light::Down), what, Some(("Try Again", Press::Run, can))) };
    }
    let off = |said: &'static str, can_be: bool| plain(Lamp::Light(Light::Off), said, Some((verb(item), Press::Run, can && can_be)));
    match (item, item.done(host, shown.who.as_ref())) {
        // (Asked, and not answered yet: a grey ring, as one that waits.
        // The amber ring is a thing being done to the account.)
        (_, None) if reached => plain(Lamp::Queued, "Checking…", None),
        // (Not known, of a host that isn't connected: a dash, and no
        // button. Six greyed Installs said nothing was there.)
        (_, None) => plain(Lamp::Light(Light::Off), "—", None),
        (Item::Skill, Some(true)) => plain(Lamp::Light(Light::On), "Installed", Some(("Remove", Press::RemoveSkill, can))),
        (Item::Carry, Some(true)) => plain(Lamp::Light(Light::On), "As on this computer", None),
        (Item::Carry, Some(false)) => {
            let to_copy = host.carried.map_or(0, |(to_copy, ..)| to_copy);
            Row { said: olo_kit::words::count(to_copy as u64, "1 file to copy", "{n} files to copy"), ..off("", true) }
        }
        (Item::Step(Step::Path), Some(true)) => plain(Lamp::Light(Light::On), "On the path", None),
        (Item::Step(Step::Git), Some(true)) => {
            let there = host.account.as_ref().and_then(|a| a.git.clone());
            match (there, &shown.who) {
                // (Another name there than here: set, and working, so
                // green and counted; said, with both names under it,
                // and this computer's offered, never put over it
                // unasked.)
                (Some(there), Some(here)) if there != *here => Row {
                    note: Some(olo_kit::tr!("There: {} <{}>  ·  Here: {} <{}>", there.0, there.1, here.0, here.1).into()),
                    ..plain(Lamp::Light(Light::On), "Differs from this computer", Some(("Use Mine", Press::Run, can)))
                },
                (Some((name, email)), _) => Row { said: format!("{name} <{email}>"), ..plain(Lamp::Light(Light::On), "", None) },
                (None, _) => plain(Lamp::Light(Light::On), "Set", None),
            }
        }
        (Item::Step(step), Some(true)) => Row { said: host.account.as_ref().and_then(|a| a.has(step)).unwrap_or_else(|| tr("Installed").into()), ..plain(Lamp::Light(Light::On), "", None) },
        (Item::Step(Step::Path), Some(false)) => off("Not on the path", true),
        (Item::Step(Step::Git), Some(false)) if host.account.as_ref().is_some_and(|a| !a.has_git) => off("Needs git there", false),
        (Item::Step(Step::Git), Some(false)) => off("Not set", shown.who.is_some()),
        (Item::Step(_), Some(false)) if host.account.as_ref().is_some_and(|a| !a.has_curl) => off("Needs curl there", false),
        (_, Some(false)) => off("Not installed", true),
    }
}

/// What the button for all of it would do now: those that aren't there
/// and can be put there, in their order.
pub fn left(host: &HostStands, shown: &Shown) -> Vec<Item> {
    Item::all().filter(|item| matches!(row(host, shown, *item), Row { lamp: Lamp::Light(Light::Off | Light::Down), button: Some((_, Press::Run, true)), .. })).collect()
}

/// The account's name at the window's head: who signs in there (the
/// address's account; with none in it, SSH signs in as who the person
/// is here).
fn account_name(host: &HostStands) -> String {
    let here = || if crate::server::demo_mode() { "you".to_owned() } else { std::env::var("USER").ok().filter(|user| !user.is_empty()).unwrap_or_else(|| host.name.clone()) };
    host.address.split_once('@').map(|(user, _)| user.to_owned()).filter(|user| !user.is_empty()).unwrap_or_else(here)
}

/// The head's line under the name: its light, and how the account
/// stands in a few words.
pub fn head_state(host: &HostStands, shown: &Shown) -> (Lamp, String) {
    use olo_kit::words::tr;
    let reached = host.conn == HostConnState::Connected && host.ssh;
    if !reached {
        return (Lamp::Light(panels::light_for(&host.conn)), panels::host_state(&host.conn, false));
    }
    if shown.busy() {
        // (Which of the run is being done: the one sign of how far it
        // is, where an installer says nothing for a while.)
        let at = shown.ran + usize::from(shown.doing.is_some());
        let of = shown.ran + usize::from(shown.doing.is_some()) + shown.queue.len();
        return (Lamp::Doing, olo_kit::tr!("Setting up {} of {}…", at.max(1), of.max(1)).into());
    }
    // (What couldn't be done is said here too: with it only on its
    // row, the head read as if nothing had gone wrong.)
    let failed = match shown.failed.len() {
        0 => String::new(),
        n => format!("  ·  {}", olo_kit::words::count(n as u64, "1 couldn’t be done", "{n} couldn’t be done")),
    };
    match tally(host) {
        None => (Lamp::Queued, tr("Checking…").into()),
        Some((0, _, _)) => (Lamp::Light(Light::Off), format!("{}{failed}", tr("Not set up"))),
        Some((done, all, _)) if done == all => (Lamp::Light(Light::On), tr("Ready").into()),
        Some((done, all, _)) => (Lamp::Light(Light::Waiting), format!("{}{failed}", olo_kit::tr!("{} of {} ready", done, all))),
    }
}

fn lamp(lamp: Lamp) -> Element {
    match lamp {
        Lamp::Light(light) => panels::light(light),
        Lamp::Doing => panels::busy_of(panels::LIGHT),
        Lamp::Queued => panels::ring_of(Light::Off, panels::LIGHT),
    }
}

pub fn render_account_panel(host: &HostStands, shown: &Shown, on_do: impl Fn(&AccountDo, &mut WindowCx) + 'static) -> Element {
    let on_do: Rc<dyn Fn(&AccountDo, &mut WindowCx)> = Rc::new(on_do);
    // (One width for the window's buttons: one column down its right,
    // and the foot's two at its two edges.)
    let wide = ["All Set", "Stop", "Install", "Remove", "Try Again", "Check Again", "Connect", "Add", "Set", "Copy", "Use Mine", "Set Up 9"]
        .iter()
        .map(|l| button_width(l, false))
        .fold(m::BUTTON_MIN_WIDTH, f32::max);
    let button = |label: String, id: String, what: AccountDo, enabled: bool| {
        let on_do = on_do.clone();
        Button::new(label).id(id).min_width(wide).enabled(enabled).on_click(move |_, cx| on_do(&what, cx))
    };
    let reached = host.conn == HostConnState::Connected && host.ssh;

    // The head: whose account, and how much of it is ready.
    let (head_lamp, state) = head_state(host, shown);
    let head = div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::SPACING))
        .child(
            canvas(|g, b| {
                icons::paint(g, b, "people");
            })
            .paint_key(0)
            .w(px(ICON))
            .h(px(ICON))
            .flex_shrink_0(),
        )
        .child(
            div().flex_1().min_w_0().h(px(NAME_ROW + STATE_ROW)).flex().flex_col().child(StringView::new(account_name(host)).bold(true).size(NAME_SIZE).cut(true).height(NAME_ROW)).child(
                div()
                    .w_full()
                    .h(px(STATE_ROW))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(m::TIGHT))
                    .child(lamp(head_lamp))
                    .child(StringView::new(state).size(style::SMALL_SIZE).height(STATE_ROW))
                    .child(div().flex_1().min_w_0().child(StringView::new(format!("·  {}", host.address)).size(style::SMALL_SIZE).dim(true).cut(true).height(STATE_ROW))),
            ),
        );
    // (What is promised of all of it, once, under the head. For a
    // host that isn't reached over SSH, why nothing here can be done.)
    let promise = if host.conn == HostConnState::Connected && !host.ssh {
        "This host isn’t reached over SSH: its account can’t be asked how it stands, or changed, from here."
    } else {
        "Installed in your home folder there, as you, by each maker’s own installer. Nothing runs as root."
    };
    let promise = StringView::new(olo_kit::words::tr(promise).to_string()).size(style::SMALL_SIZE).dim(true).wrap(true);

    let under = panels::LIGHT + m::TIGHT;
    let mut page = vec![div().w_full().flex().flex_col().gap(px(m::SPACING)).child(head).child(promise).into_element()];
    for (title, items) in GROUPS {
        let mut rows = Vec::new();
        for &item in items {
            let r = row(host, shown, item);
            let words = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(m::TIGHT))
                .child(lamp(r.lamp))
                .child(StringView::new(olo_kit::words::tr(item.title()).to_string()))
                .child(StringView::new("·").dim(true))
                .child(div().flex_1().min_w_0().child(StringView::new(r.said.clone()).dim(true).cut_words(true)));
            // (A row as tall with a button as without: what is under
            // it never moves when its button goes.)
            let line = div().w_full().h(px(m::CONTROL_HEIGHT + 2.0)).flex().flex_row().items_center().gap(px(m::SPACING)).child(words);
            let line = match r.button {
                Some((label, press, can)) => {
                    let what = match press {
                        Press::Run => AccountDo::Run(vec![item]),
                        Press::RemoveSkill => AccountDo::RemoveSkill,
                    };
                    line.child(button(label.to_owned(), format!("account-{}", item.id()), what, can))
                }
                None => line,
            };
            // (Under it, begun where its name begins and ended clear of
            // the buttons' column: what it is; or what the host wrote,
            // in the letters a host's words are always in here, one
            // line, its whole in its tip.)
            let note = match (&r.host_said, &r.note) {
                (Some(said), _) => StringView::new(said.clone()).mono(true).size(MONO_SIZE).dim(true).cut(true).height(style::line_row(style::SMALL_SIZE)),
                (None, Some(note)) => StringView::new(note.clone()).size(style::SMALL_SIZE).dim(true).cut(true).height(style::line_row(style::SMALL_SIZE)),
                (None, None) => StringView::new(olo_kit::words::tr(item.why()).to_string()).size(style::SMALL_SIZE).dim(true).cut_words(true).height(style::line_row(style::SMALL_SIZE)),
            };
            let note = div().w_full().pl(px(under)).pr(px(wide + m::SPACING)).child(note);
            rows.push(div().w_full().flex().flex_col().child(line).child(note).into_element());
        }
        page.push(
            div()
                .w_full()
                .flex()
                .flex_col()
                .child(StringView::new(olo_kit::words::tr(title).to_string()).bold(true).height(m::LABEL_ROW))
                .child(div().h(px(2.0)).flex_shrink_0())
                .child(separator())
                .child(div().h(px(m::TIGHT)).flex_shrink_0())
                .child(div().w_full().flex().flex_col().gap(px(m::TIGHT)).children(rows))
                .into_element(),
        );
    }

    // The foot: the host asked again, at its left; at its right the
    // one button for all that is left, which says how many they are,
    // is Stop while something is being done, and says so when nothing
    // is left.
    let all = left(host, shown);
    let lead = if shown.busy() {
        button(olo_kit::words::tr("Stop").into(), "account-all".into(), AccountDo::Stop, true)
    } else if all.is_empty() {
        let word = if tally(host).is_some_and(|(done, total, _)| done == total) { "All Set" } else { "Set Up" };
        button(olo_kit::words::tr(word).into(), "account-all".into(), AccountDo::Run(Vec::new()), false)
    } else {
        button(olo_kit::tr!("Set Up {}", all.len()).into(), "account-all".into(), AccountDo::Run(all.clone()), true).default(true)
    };
    let foot = div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::SPACING))
        // (For a host that isn't connected, the one thing to do here:
        // Connect, as the Host window's head has it.)
        .child(if reached || host.conn == HostConnState::Connecting {
            button(olo_kit::words::tr("Check Again").into(), "account-check".into(), AccountDo::Check, reached && !shown.checking && !shown.busy())
        } else {
            button(olo_kit::words::tr("Connect").into(), "account-check".into(), AccountDo::Connect, matches!(host.conn, HostConnState::Disconnected | HostConnState::Error(_)))
        })
        .child(div().flex_1())
        .child(lead);
    page.push(div().w_full().flex().flex_col().gap(px(m::SPACING)).child(separator()).child(foot).into_element());

    div().w_full().flex().flex_col().gap(px(m::SPACING + m::TIGHT)).p(px(m::WINDOW_INSET)).children(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::Doing;
    use olo_rex_client::ready::Account;

    fn host(account: Option<Account>, skill: Option<bool>, conn: HostConnState) -> HostStands {
        HostStands {
            name: "dev-box".into(),
            address: "you@dev-box.example".into(),
            conn,
            ssh: true,
            helper: crate::hub::HostHelper::None,
            helper_kept_off: false,
            helper_setting: true,
            bridge: None,
            granted: Vec::new(),
            skill,
            pool: None,
            version: None,
            account,
            // (The settings carried: as here, but where a test says.)
            carried: Some((0, 3, false)),
        }
    }

    const CLAUDE: Item = Item::Step(Step::Claude);
    const CODEX: Item = Item::Step(Step::Codex);
    const AGY: Item = Item::Step(Step::Antigravity);
    const PATH: Item = Item::Step(Step::Path);
    const GIT: Item = Item::Step(Step::Git);

    #[test]
    fn each_item_says_how_it_stands_and_what_can_be_done() {
        let who = Some(("You".to_owned(), "you@example.org".to_owned()));
        let some = Account { path: true, claude: Some("2.1.331".into()), git: who.clone(), has_curl: true, has_git: true, ..Account::default() };
        let h = host(Some(some.clone()), Some(false), HostConnState::Connected);
        let shown = Shown { key: "dev".into(), name: "dev-box".into(), who: who.clone(), ..Shown::default() };
        assert_eq!(row(&h, &shown, CLAUDE), Row { lamp: Lamp::Light(Light::On), said: "2.1.331".into(), host_said: None, note: None, button: None });
        assert_eq!(row(&h, &shown, PATH).said, "On the path");
        assert_eq!(row(&h, &shown, GIT).said, "You <you@example.org>");
        assert_eq!(row(&h, &shown, CODEX), Row { lamp: Lamp::Light(Light::Off), said: "Not installed".into(), host_said: None, note: None, button: Some(("Install", Press::Run, true)) });
        assert_eq!(row(&h, &shown, Item::Skill).button, Some(("Install", Press::Run, true)));
        assert_eq!(left(&h, &shown), [CODEX, AGY, Item::Skill]);
        assert_eq!(head_state(&h, &shown), (Lamp::Light(Light::Waiting), "4 of 7 ready".into()));
        // (The person's settings: how many would be copied; as here, nothing to press.)
        let lacking = HostStands { carried: Some((12, 2, false)), ..h.clone() };
        assert_eq!(row(&lacking, &shown, Item::Carry), Row { lamp: Lamp::Light(Light::Off), said: "12 files to copy".into(), host_said: None, note: None, button: Some(("Copy", Press::Run, true)) });
        assert_eq!(row(&h, &shown, Item::Carry), Row { lamp: Lamp::Light(Light::On), said: "As on this computer".into(), host_said: None, note: None, button: None });
        assert_eq!(left(&lacking, &shown), [CODEX, AGY, Item::Carry, Item::Skill]);
        assert_eq!(account_name(&h), "you");

        // One being done, others waiting: no buttons, nothing else begun meanwhile.
        let busy = Shown { doing: Some(Doing { item: CODEX, line: "Downloading…  40%".into(), removing: false }), queue: vec![AGY], ran: 1, ..shown.clone() };
        assert_eq!(row(&h, &busy, CODEX), Row { lamp: Lamp::Doing, said: "Installing…".into(), host_said: Some("Downloading…  40%".into()), note: None, button: None });
        assert_eq!(row(&h, &busy, AGY), Row { lamp: Lamp::Queued, said: "Waiting".into(), host_said: None, note: None, button: None });
        assert_eq!(row(&h, &busy, Item::Skill).button, Some(("Install", Press::Run, false)));
        assert!(left(&h, &busy).is_empty());
        assert_eq!(head_state(&h, &busy).1, "Setting up 2 of 3…");

        // One that couldn't be done: so said, the host's words under it, and to be tried again with the rest.
        let failed = Shown { failed: vec![(CODEX, "curl: (6) Could not resolve host".into())], ..shown.clone() };
        assert_eq!(
            row(&h, &failed, CODEX),
            Row {
                lamp: Lamp::Light(Light::Down),
                said: "Couldn’t be installed".into(),
                host_said: Some("curl: (6) Could not resolve host".into()),
                note: None,
                button: Some(("Try Again", Press::Run, true))
            }
        );
        assert_eq!(head_state(&h, &failed).1, "4 of 7 ready  ·  1 couldn’t be done");
        assert_eq!(left(&h, &failed), [CODEX, AGY, Item::Skill]);

        // What an item needs that isn't there: said, and its button has nothing to do.
        let bare = host(Some(Account::default()), Some(true), HostConnState::Connected);
        assert_eq!((row(&bare, &shown, CLAUDE).said, row(&bare, &shown, CLAUDE).button), ("Needs curl there".into(), Some(("Install", Press::Run, false))));
        assert_eq!(row(&bare, &shown, GIT).said, "Needs git there");
        assert_eq!(left(&bare, &shown), [PATH]);
        // (The skill that is there can be taken off: the one thing here that is.)
        assert_eq!(row(&bare, &shown, Item::Skill), Row { lamp: Lamp::Light(Light::On), said: "Installed".into(), host_said: None, note: None, button: Some(("Remove", Press::RemoveSkill, true)) });
        let fresh = HostStands { carried: Some((5, 0, false)), ..host(Some(Account { has_curl: true, has_git: true, ..Account::default() }), Some(false), HostConnState::Connected) };
        assert_eq!(row(&fresh, &Shown { who: None, ..shown.clone() }, GIT).button, Some(("Set", Press::Run, false)));
        assert_eq!(head_state(&fresh, &shown), (Lamp::Light(Light::Off), "Not set up".into()));

        // Git told another name there than here: said, and this computer's offered; not one of "all that is left".
        let other = host(Some(Account { git: Some(("Somebody Else".into(), "else@example.org".into())), ..some.clone() }), Some(true), HostConnState::Connected);
        assert_eq!(
            row(&other, &shown, GIT),
            Row {
                lamp: Lamp::Light(Light::On),
                said: "Differs from this computer".into(),
                host_said: None,
                note: Some("There: Somebody Else <else@example.org>  ·  Here: You <you@example.org>".into()),
                button: Some(("Use Mine", Press::Run, true))
            }
        );
        assert_eq!(left(&other, &shown), [CODEX, AGY]);

        // All of it there.
        let ready = host(Some(Account { codex: Some("0.172.0".into()), antigravity: Some("1.5.15".into()), ..some.clone() }), Some(true), HostConnState::Connected);
        assert!(left(&ready, &shown).is_empty());
        assert_eq!(head_state(&ready, &shown), (Lamp::Light(Light::On), "Ready".into()));

        // Not answered yet: asking. Not connected: nothing is known, nothing to press.
        let asking = HostStands { carried: None, ..host(None, None, HostConnState::Connected) };
        assert!(Item::all().all(|i| row(&asking, &shown, i) == Row { lamp: Lamp::Queued, said: "Checking…".into(), host_said: None, note: None, button: None }));
        assert_eq!(head_state(&asking, &shown), (Lamp::Queued, "Checking…".into()));
        let away = host(Some(some), Some(true), HostConnState::Disconnected);
        assert!(Item::all().all(|i| row(&away, &shown, i) == Row { lamp: Lamp::Light(Light::Off), said: "—".into(), host_said: None, note: None, button: None }));
        assert_eq!(head_state(&away, &shown), (Lamp::Light(Light::Off), "Disconnected".into()));
        assert!(left(&away, &shown).is_empty());
    }
}
