//! Passwords' windows, on olo-kit. The main window's own view
//! ([`MainView`]) shows the account setup, the lock and the connection's
//! panels, or the three panes: the sources, the items under their
//! filters, and the selected item or its editor, which is a view of its
//! own ([`Detail`]: it alone is drawn again as a one-time code counts
//! down, or as the pointer moves over its fields). The Review Sync and
//! Sync Problems windows are here too.
//!
//! Each locks the app (`crate::app`) while it says what it shows, and
//! again to act; what's drawn later (a list's cells) is drawn from what
//! the view kept. What the app wants of a view's own things (`Ui`) the
//! view takes as it says what it shows.

use std::{
    collections::HashSet,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Local, Utc};
use olo_kit::{
    Bounds, Child, ClickEvent, Corner, Cx, Element, FocusHandle, Font, FontStyle, FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, Menu, MenuBar, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, OutgoingDrag, ParentElement as _, Pixels, Point, ScrollHandle, SharedString, Styled as _, TextRun, View, WindowCx, agent, canvas, div,
    due::Due,
    fill, icons, menu, metrics as m, point, px, relative,
    search::{self, Facet, FilterBar, Needle, Query, SearchEvent, SearchField, search_field},
    size,
    style::{self, c},
    theme,
    widgets::{
        Button, CheckBox, Column, ColumnListView, ListModel, ScrollView, Sidebar, StringView, TextControl, TextField, TextFieldEvent, button_row,
        sidebar::{self, Row},
        text_field,
    },
};
use onepassword::{ItemCategory, ItemField, ItemFieldType, ItemOverview, ItemState};

use crate::{
    Accounts, Conn, Passwords, Source, app,
    edit::{self, EditForm, Slot},
    lock::Finger,
    store::SyncState,
};

/// What File ▸ New Item offers, most used first.
pub const NEW_CATEGORIES: [ItemCategory; 13] = [
    ItemCategory::Login,
    ItemCategory::Password,
    ItemCategory::SecureNote,
    ItemCategory::CreditCard,
    ItemCategory::Identity,
    ItemCategory::BankAccount,
    ItemCategory::ApiCredentials,
    ItemCategory::Database,
    ItemCategory::Email,
    ItemCategory::Server,
    ItemCategory::Router,
    ItemCategory::SoftwareLicense,
    ItemCategory::Membership,
];

pub fn icon(c: ItemCategory) -> &'static str {
    match c {
        ItemCategory::Login => "authentication",
        ItemCategory::Password | ItemCategory::ApiCredentials | ItemCategory::SshKey | ItemCategory::CryptoWallet => "key",
        ItemCategory::CreditCard => "credit-card",
        ItemCategory::SecureNote => "secure-note",
        ItemCategory::Identity
        | ItemCategory::DriverLicense
        | ItemCategory::Passport
        | ItemCategory::OutdoorLicense
        | ItemCategory::Membership
        | ItemCategory::Rewards
        | ItemCategory::SocialSecurityNumber
        | ItemCategory::MedicalRecord => "id-card",
        ItemCategory::Person => "people",
        ItemCategory::BankAccount => "bank",
        ItemCategory::Database => "database",
        ItemCategory::Email => "mail",
        ItemCategory::Server => "system",
        ItemCategory::Router => "network-prefs",
        ItemCategory::SoftwareLicense => "installer",
        ItemCategory::Document => "document",
        ItemCategory::Unsupported => "document",
    }
}

const SOURCES_WIDTH: f32 = 184.0;
const LIST_WIDTH: f32 = 290.0;
const ITEM_ROW: f32 = 42.0;
const STATUS_HEIGHT: f32 = 22.0;
const FIELD_ROW: f32 = 44.0;
/// An item's title.
const TITLE_SIZE: f32 = 18.0;
/// Where the middle of an item's title is, down from the top of its
/// header's room.
const TITLE_MIDDLE: f32 = 16.0;
/// The Copy and Show buttons on a field the pointer is over.
const FIELD_BUTTON: f32 = 56.0;
/// Where a field's label and its value are, down its row.
const FIELD_LABEL_Y: f32 = 13.0;
const FIELD_VALUE_Y: f32 = 31.0;

fn dim(p: &olo_theme::Palette) -> Hsla {
    style::disabled_text(p)
}

/// A sunken well's two lines, in logical pixels: whole device pixels.
fn edge(cx: &WindowCx) -> f32 {
    let s = cx.scale_factor();
    style::line_pixels(s) * 2.0 / s
}

/// A sunken well of `background` with `content` inside its lines (`e`
/// wide: [`edge`]).
fn well(e: f32, background: olo_theme::Rgba, content: impl IntoElement) -> Element {
    let ground = canvas(move |g, b| {
        let p = theme::palette();
        let b = style::snap(g, b);
        style::fill_rect(g, b, c(background));
        style::sunken(g, b, &p, false);
    })
    .paint_key(style::key(background))
    .absolute()
    .inset_0();
    div().relative().size_full().child(ground).child(div().absolute().left(px(e)).top(px(e)).right(px(e)).bottom(px(e)).overflow_hidden().child(content))
}

/// A line across the window a device pixel tall (under the toolbar,
/// over the status line).
fn rule(cx: &WindowCx) -> Element {
    let s = cx.scale_factor();
    canvas(|g, b| {
        let p = theme::palette();
        let b = style::snap(g, b);
        style::fill_rect(g, b, style::shade(&p, p.panel_background, 1.3));
    })
    .paint_key(0)
    .w_full()
    .h(px(style::line_pixels(s) / s))
    .flex_shrink_0()
}

/// A centred column: icon, heading, message, then `extra`.
fn panel(icon_name: &'static str, heading: &str, message: String, extra: impl IntoElement) -> Element {
    let heading: SharedString = heading.to_owned().into();
    let key = style::key((icon_name, &heading));
    let face = canvas(move |g, b| {
        let p = theme::palette();
        icons::paint(g, style::snap_centred(g, point(b.center().x, b.top() + px(32.0)), 64.0, 64.0), icon_name);
        let t = style::label_fit(
            g,
            b.center().x,
            b.top() + px(64.0 + 2.0 * m::SPACING + 11.0),
            style::Align::Center,
            heading.clone(),
            16.0,
            style::ui_font(FontWeight::BOLD),
            c(p.panel_text),
            b.size.width,
        );
        agent::record(g, agent::Node { role: "Heading", name: Some(heading.clone()), bounds: t, enabled: true, ..Default::default() });
    })
    .paint_key(key)
    .w_full()
    .h(px(64.0 + 2.0 * m::SPACING + 22.0));
    let column = div()
        .w(px(380.0))
        .max_w_full()
        .flex()
        .flex_col()
        .gap(px(m::SPACING))
        .child(face)
        .child(div().w_full().text_center().child(StringView::new(message).wrap(true)))
        .child(div().h(px(m::SPACING)))
        .child(extra);
    div().size_full().flex().flex_col().items_center().px(px(m::WINDOW_INSET)).child(div().w_full().h(relative(0.22)).min_h(px(m::WINDOW_INSET)).flex_shrink_0()).child(column).into_any_element()
}

/// Small text in the disabled text's colour, wrapped (a note under a
/// choice).
fn note(text: impl Into<SharedString>) -> Element {
    div().w_full().text_size(px(style::SMALL_SIZE)).child(StringView::new(text).wrap(true).dim(true))
}

/// The app's accounts, then any synced now that it doesn't list.
pub fn listed_accounts(app: &Passwords) -> Vec<onepassword::accounts::Account> {
    let mut listed: Vec<onepassword::accounts::Account> = match &app.accounts {
        Accounts::Found(list) => list.clone(),
        _ => Vec::new(),
    };
    for l in &app.settings.accounts {
        if !listed.iter().any(|a| same(a, &l.id)) {
            listed.push(onepassword::accounts::Account { uuid: l.id.clone(), name: l.label.clone(), email: String::new(), address: String::new() });
        }
    }
    listed
}

/// Whether `id` (a UUID, or a name typed by hand) is the account `a`.
pub fn same(a: &onepassword::accounts::Account, id: &str) -> bool {
    a.uuid == id || a.name == id
}

/// The id to sync `a` by: the one already in use for it (an account once
/// named by hand keeps its name, which its synced vaults are kept under),
/// else its UUID.
pub fn linked_id(app: &Passwords, a: &onepassword::accounts::Account) -> String {
    app.settings.accounts.iter().find(|l| same(a, &l.id)).map_or_else(|| a.uuid.clone(), |l| l.id.clone())
}

/// "just now", "5 minutes ago", "at 14:05".
fn sync_ago(at: chrono::DateTime<chrono::Utc>) -> String {
    let secs = (chrono::Utc::now() - at).num_seconds().max(0);
    match secs {
        0..60 => "just now".into(),
        60..120 => "a minute ago".into(),
        120..3600 => format!("{} minutes ago", secs / 60),
        _ => format!("at {}", at.with_timezone(&chrono::Local).format("%H:%M")),
    }
}

/// What a click on the sync status shows.
enum More {
    Text(String),
    Problems(Vec<crate::sync::Problem>),
}

fn plural(c: ItemCategory) -> String {
    match c {
        ItemCategory::Identity => "Identities".into(),
        ItemCategory::Email => "Email Accounts".into(),
        ItemCategory::Person => "People".into(),
        ItemCategory::Unsupported => "Other".into(),
        c => format!("{}s", c.label()),
    }
}

/// What the main window is told by the app.
pub enum MainMsg {
    /// Something it shows changed.
    Refresh,
}

/// The main window: the setup, the lock, the connection's panels, or the
/// three panes under the toolbar, over the status line.
pub struct MainView {
    /// The setup's account, named by hand.
    account: Child<TextField>,
    /// The lock panel's password.
    lock: Child<TextField>,
    search: Child<SearchField>,
    list: ListModel,
    detail: Child<Detail>,
    /// The filters, as the search field and the bar have them, and the
    /// items they were made for (the app's `rev`).
    facets: Rc<Vec<Facet>>,
    facets_rev: Option<u64>,
    /// What the list's cells are drawn from: the items listed, what's
    /// under each title, and what the search marks.
    rows: Arc<Vec<ItemOverview>>,
    subtitles: Rc<Vec<String>>,
    query: Rc<Query>,
    searched: String,
    /// What those were worked out for: the items (the app's `rev`), the
    /// source and the search. (Not again for a selection that moved.)
    listed_for: Option<(u64, Source, String)>,
    /// When "Synced 5 minutes ago" next reads differently.
    wake: Due,
}

impl MainView {
    pub fn new(cx: &mut Cx<MainView>) -> MainView {
        let account = text_field(cx, |f| f.name("Account").hint("as in 1Password's sidebar, or its UUID"));
        let lock = text_field(cx, |f| f.name("Password").password(true));
        let search = search_field(cx, "Search");
        cx.subscribe(&lock, |v: &mut MainView, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Changed => {
                let text = v.lock.read(cx).text().to_owned();
                app().lock.password = text;
                cx.invalidate();
            }
            TextFieldEvent::Entered => app().submit_password(),
            TextFieldEvent::Typed(_) => {}
        });
        cx.subscribe(&account, |v: &mut MainView, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Entered => {
                let typed = v.account.read(cx).text().to_owned();
                app().sync_chosen(&typed);
            }
            _ => cx.invalidate(),
        });
        cx.subscribe(&search, |v: &mut MainView, e: &SearchEvent, cx| {
            if let SearchEvent::Changed = e {
                let text = v.search.read(cx).text(cx).to_owned();
                app().set_search(text, false);
            }
        });
        // (As it was before an upgrade, or before the window was closed.)
        let text = app().search.clone();
        if !text.is_empty() {
            search.update(cx, |f, cx| f.set_text(text, cx));
        }
        let detail = cx.new_view(Detail::new);
        let active = cx.is_window_active();
        app().lock.focus(active);
        MainView {
            account,
            lock,
            search,
            list: ListModel::new(false),
            detail,
            facets: Rc::new(Vec::new()),
            facets_rev: None,
            rows: Arc::new(Vec::new()),
            subtitles: Rc::new(Vec::new()),
            query: Rc::new(Query::parse("", &[])),
            searched: String::new(),
            listed_for: None,
            wake: Due::default(),
        }
    }

    /// What the app wants of the window's own things, done.
    fn take_asks(&mut self, a: &mut Passwords, cx: &mut Cx<Self>) {
        if std::mem::take(&mut a.ui.clear_lock) {
            self.lock.update(cx, |f, cx| f.clear_secret(cx));
        }
        if std::mem::take(&mut a.ui.clear_account) {
            self.account.update(cx, |f, cx| f.set_text("", cx));
        }
        if std::mem::take(&mut a.ui.push_search) {
            let text = a.search.clone();
            self.search.update(cx, |f, cx| f.set_text(text, cx));
        }
        if std::mem::take(&mut a.ui.focus_search) {
            self.search.update(cx, |f, cx| f.focus(cx));
        }
        // (The filters' values and counts follow the items.)
        if self.facets_rev != Some(a.rev) {
            self.facets_rev = Some(a.rev);
            self.facets = Rc::new((*a.facets_rc()).clone());
            let facets = self.facets.clone();
            self.search.update(cx, |f, cx| f.set_facets(facets, cx));
        }
    }

    /// A key, before whatever has the keyboard takes it: Up and Down in
    /// the search field move the list's selection.
    fn key(&mut self, e: &KeyDownEvent, cx: &mut Cx<Self>) {
        let mut a = app();
        // Anything the person does puts off the auto-lock.
        a.used();
        let up = match e.keystroke.key.as_str() {
            "up" => true,
            "down" => false,
            _ => return,
        };
        if e.keystroke.modifiers.modified() || a.editor.is_some() || a.lock.locked || a.conn == Conn::Setup {
            return;
        }
        let field = self.search.read(cx);
        if !field.is_focused(cx) || field.suggesting(cx) {
            return;
        }
        a.step_selection(up);
        drop(a);
        cx.stop_propagation();
    }

    fn setup(&mut self, a: &mut Passwords, cx: &mut Cx<Self>) -> Element {
        if matches!(a.accounts, Accounts::Unknown) {
            a.find_accounts();
        }
        let cancel_to = a.set_up();
        let listed = listed_accounts(a);
        let message = format!(
            "Passwords keeps your items on this computer, encrypted, and can keep them in step with your 1Password accounts both ways. To sync, turn on Settings ▸ Developer ▸ Integrate with other apps in 1Password, then {} the accounts.",
            if listed.is_empty() { "name" } else { "choose" }
        );
        let mut extra = div().w_full().flex().flex_col().gap(px(m::SPACING));
        for (i, acc) in listed.iter().enumerate() {
            let on = a.chosen.iter().any(|c| same(acc, c));
            let account = acc.clone();
            extra = extra.child(CheckBox::new(acc.name.clone(), on).id(format!("account-{i}")).on_toggle(move |on: &bool, _| {
                let mut a = app();
                if *on {
                    let id = linked_id(&a, &account);
                    a.chosen.push(id);
                } else {
                    a.chosen.retain(|c| !same(&account, c));
                }
                a.changed();
            }));
            let detail = [acc.email.as_str(), acc.address.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" · ");
            if !detail.is_empty() {
                extra = extra.child(note(detail).pl(px(21.0)).mt(px(-m::SPACING * 0.75)));
            }
        }
        if !listed.is_empty() {
            extra = extra.child(CheckBox::new("Another account", a.other_account).id("account-other").on_toggle(|on: &bool, _| {
                let mut a = app();
                a.other_account = *on;
                a.changed();
            }));
        }
        let typed = !self.account.read(cx).text().trim().is_empty();
        if listed.is_empty() || a.other_account {
            extra = extra.child(TextControl::new(cx, &self.account).label("Account:").divider(70.0));
            if !self.account.is_focused(cx) && !typed && a.chosen.is_empty() {
                self.account.focus(cx);
            }
        }
        let any = !a.chosen.is_empty() || ((listed.is_empty() || a.other_account) && typed);
        let mut row = button_row();
        if cancel_to {
            row = row.child(Button::new("Cancel").on_click(|_: &ClickEvent, _| {
                let mut a = app();
                a.conn = if a.vaults.is_empty() { Conn::Connecting } else { Conn::Ready };
                if a.vaults.is_empty() {
                    a.connect();
                }
                a.changed();
            }));
        } else {
            row = row.child(Button::new("Keep Only on This Computer").on_click(|_: &ClickEvent, _| app().keep_local()));
        }
        row = row.child(Button::new("Sync").default(true).enabled(any).on_click(cx.listener(|v: &mut MainView, _: &ClickEvent, cx| {
            let typed = v.account.read(cx).text().to_owned();
            app().sync_chosen(&typed);
        })));
        extra = extra.child(row);
        if !any && cancel_to {
            extra = extra.child(note("With no account chosen, Passwords keeps its items only on this computer."));
        }
        panel("passwords", "Sync with 1Password?", message, extra)
    }

    fn locked(&mut self, a: &mut Passwords, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let mut message = match (&a.lock.finger, &a.lock.key_note) {
            (Finger::Listening(s), Some(_)) if s == "Touch ID or enter your password" => "Touch ID, touch your security key, or enter your password".into(),
            (Finger::Listening(s), _) => s.clone(),
            (_, Some(k)) => k.clone(),
            _ => format!("Enter the password for “{}” to unlock it.", a.lock.user()),
        };
        // Who is waiting: the app by name, or apps, if more than one.
        if let Some(first) = a.waiting.first() {
            let who = if a.waiting.iter().all(|w| w.from == first.from) {
                let name = olo_xdg::apps::our_app(first.from.trim_start_matches("olo-")).map_or("An app", |(name, _)| name);
                let passkey = a.waiting.iter().all(|w| matches!(w.request, olo_keychain::Request::PasskeySign { .. } | olo_keychain::Request::PasskeyCreate { .. }));
                format!("{name} is waiting for a {}.", if passkey { "passkey" } else { "login" })
            } else {
                "Apps are waiting for logins.".to_owned()
            };
            message = format!("{who} {message}");
        }
        let icon_name = if matches!(a.lock.finger, Finger::Listening(_)) { "fingerprint" } else { "passwords" };
        if !self.lock.is_focused(cx) && cx.is_window_active() {
            self.lock.focus(cx);
        }
        let mut extra = div().w_full().flex().flex_col().gap(px(m::SPACING)).child(TextControl::new(cx, &self.lock).label("Password:").divider(78.0));
        if let Some(e) = &a.lock.error {
            extra = extra.child(StringView::new(e.clone()).wrap(true).color(c(p.failure)));
        }
        let label = if a.lock.checking { "Checking…" } else { "Unlock" };
        let ready = !a.lock.checking && !a.lock.password.is_empty();
        extra = extra.child(button_row().child(Button::new(label).default(true).enabled(ready).on_click(|_: &ClickEvent, _| app().submit_password())));
        panel(icon_name, "Passwords is locked", message, extra)
    }

    /// The toolbar, the three panes and the status line.
    fn browser(&mut self, a: &mut Passwords, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        if std::mem::take(&mut a.unlocked) {
            // The list takes the keyboard from the password field.
            self.list.focus(cx);
        }
        // Toolbar: New Item, Generator, and the search field on the right.
        let toolbar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            .h(px(m::TOOLBAR_HEIGHT))
            .px(px(m::SPACING))
            .flex_shrink_0()
            // (Nothing is made on an agent screen: what's there is shared.)
            .children((!crate::on_screen()).then(|| {
                Button::new("New Item").on_click(|_: &ClickEvent, cx| {
                    let b = cx.listener_bounds();
                    menu::popup(cx, crate::category_menu(Menu::new("New Item"), "new:"), b, Corner::BottomLeft, |cmd, _| app().command(cmd));
                })
            }))
            .child(Button::new("Generator").on_click(|_: &ClickEvent, _| app().open_generator(None)))
            .child(div().flex_1())
            .child(div().w(px(280.0)).child(&self.search));

        // The status line: what's going on, and the sync's state on the
        // right (a click shows a problem in full, or the changes waiting).
        let left: SharedString = match a.status.text() {
            Some(s) => s.clone(),
            None if a.loading => "Opening…".into(),
            None => {
                let n = a.visible().len();
                let said = if a.search.trim().is_empty() { format!("{n} item{}", if n == 1 { "" } else { "s" }) } else { format!("{n} found") };
                // (What a screen's Passwords shows is the person's to change.)
                if crate::on_screen() { format!("{said} shared with this screen") } else { said }.into()
            }
        };
        let review_click = matches!(a.sync, Some(SyncState::Review { .. } | SyncState::Declined { .. })) && !matches!(a.conn, Conn::Failed(_));
        let right = match (&a.conn, &a.sync) {
            (Conn::Failed(m), _) => Some((m.split('.').next().unwrap_or(m).to_string(), true, None)),
            (_, Some(SyncState::Running)) => Some(("Syncing with 1Password…".to_string(), false, None)),
            (_, Some(SyncState::Review { changes, .. })) => Some((format!("{} to review", if *changes == 1 { "1 sync change".to_string() } else { format!("{changes} sync changes") }), false, None)),
            (_, Some(SyncState::Declined { pending })) => {
                let to_send = if *pending > 0 { format!(" · {pending} to send") } else { String::new() };
                Some((format!("Not synced: changes not applied{to_send}"), false, None))
            }
            (_, Some(SyncState::Done { at, problems, pending, .. })) => {
                let failed = problems.iter().filter(|p| !p.note).count();
                let mut text = format!("Synced {}", sync_ago(*at));
                if *pending > 0 {
                    text += &format!(" · {pending} to send");
                }
                if failed > 0 {
                    text += &if failed == 1 { " · 1 problem".to_string() } else { format!(" · {failed} problems") };
                } else if !problems.is_empty() {
                    text += " · some items left out";
                }
                let more = (!problems.is_empty()).then(|| More::Problems(problems.clone()));
                Some((text, failed > 0, more))
            }
            (_, Some(SyncState::Failed { message, pending })) => {
                // The first clause; a click shows the rest.
                let first = message.split(['.', ',']).next().unwrap_or(message);
                let to_send = if *pending > 0 { format!(" · {pending} to send") } else { String::new() };
                Some((format!("Not synced: {first}{to_send}"), true, Some(More::Text(message.clone()))))
            }
            _ => None,
        };
        // ("Synced a minute ago" is said again as the minutes pass, while
        // it counts them: `sync_ago`.)
        let said_again = match (&a.conn, &a.sync) {
            (Conn::Failed(_), _) => None,
            (_, Some(SyncState::Done { at, .. })) => {
                let secs = (chrono::Utc::now() - *at).num_seconds().max(0) as u64;
                (secs < 3600).then(|| Instant::now() + Duration::from_secs(60 - secs % 60))
            }
            _ => None,
        };
        self.wake.at(said_again, cx, |_, cx| cx.invalidate());
        let status_left = canvas({
            let left = left.clone();
            move |g, b| {
                let p = theme::palette();
                let r = style::label_fit(g, b.left(), b.center().y, style::Align::Left, left.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), c(p.panel_text), b.size.width);
                agent::record(g, agent::Node::label(left.clone(), r));
            }
        })
        .paint_key(style::key(&left))
        .flex_1()
        .min_w_0()
        .h_full();
        // A bar of its own along the bottom (painted, as Tracker's count bar
        // is: its text sits on the bar, not on the window's edge).
        let bar = canvas(|g, b| style::fill_rect(g, b, c(theme::palette().panel_background))).paint_key(0).absolute().inset_0();
        let mut status = div().relative().flex().flex_row().items_center().gap(px(m::SPACING)).h(px(STATUS_HEIGHT)).px(px(m::SPACING)).flex_shrink_0().child(bar).child(status_left);
        if let Some((text, bad, more)) = right {
            let clickable = more.is_some() || review_click;
            let text: SharedString = text.into();
            let width = f32::from(style::text_width(text.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL))).ceil() + 1.0;
            let label = text.clone();
            let el = canvas(move |g, b| {
                let p = theme::palette();
                let color = if bad { c(p.failure) } else { dim(&p) };
                let r = style::label(g, b.right(), b.center().y, style::Align::Right, label.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), color);
                agent::record(g, agent::Node { clickable, ..agent::Node::label(label.clone(), r) });
            })
            .paint_key(style::key((&text, bad, clickable)))
            .size_full();
            let mut holder = div().id("passwords-sync-status").w(px(width)).h_full().flex_shrink_0().child(el);
            if clickable {
                let more = Rc::new(more);
                holder = holder.cursor_pointer().on_click(move |_: &ClickEvent, _| {
                    let mut a = app();
                    match &*more {
                        Some(More::Text(t)) => a.show_problem(t.clone()),
                        Some(More::Problems(list)) => a.show_problems(list.clone()),
                        None if a.review.is_some() => a.open_review(),
                        None => a.sync_and_review(),
                    }
                });
            }
            status = status.child(holder);
        }
        let _ = p;

        // The three panes: the sources and the items here, the item a
        // view of its own.
        let sources = self.sources(a, cx);
        let items = self.items(a, cx);
        let panes = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_row()
            .gap(px(m::SPACING))
            .p(px(m::SPACING))
            .child(div().w(px(SOURCES_WIDTH)).h_full().flex_shrink_0().child(sources))
            .child(div().w(px(LIST_WIDTH)).h_full().flex_shrink_0().child(items))
            .child(div().flex_1().min_w_0().h_full().child(&self.detail));
        div().size_full().flex().flex_col().child(toolbar).child(rule(cx)).child(panes).child(rule(cx)).child(status).into_any_element()
    }

    /// The sources: All Items, the vaults (under their accounts' names
    /// with several), the categories there are, and the Archive.
    fn sources(&mut self, a: &mut Passwords, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let active = |i: &&ItemOverview| i.state == ItemState::Active;
        let mut rows: Vec<Row> = Vec::new();
        let mut of: Vec<Option<Source>> = Vec::new();
        let mut push = |row: Row, source: Option<Source>| {
            rows.push(row);
            of.push(source);
        };
        push(Row::new("passwords", "All Items").value(a.items.iter().filter(active).count().to_string()), Some(Source::All));
        // With several accounts, each account's vaults under its name (as
        // Tracker lists volumes), then any kept only here.
        let mut groups: Vec<(String, Vec<&onepassword::VaultOverview>)> = Vec::new();
        if a.settings.accounts.len() > 1 {
            for l in &a.settings.accounts {
                groups.push((l.label.clone(), a.vaults.iter().filter(|v| a.vault_accounts.get(&v.id) == Some(&l.id)).collect()));
            }
            let here: Vec<_> = a.vaults.iter().filter(|v| !a.vault_accounts.contains_key(&v.id)).collect();
            if !here.is_empty() {
                groups.push(("On This Computer".into(), here));
            }
        } else if !a.vaults.is_empty() {
            groups.push(("Vaults".into(), a.vaults.iter().collect()));
        }
        for (heading, vaults) in groups.into_iter().filter(|(_, v)| !v.is_empty()) {
            push(Row::heading(heading), None);
            for v in vaults {
                let n = a.items.iter().filter(active).filter(|i| i.vault_id == v.id).count();
                push(Row::new("vault", v.title.clone()).value(n.to_string()), Some(Source::Vault(v.id.clone())));
            }
        }
        let mut categories: Vec<ItemCategory> = a.items.iter().filter(active).map(|i| i.category).collect();
        categories.sort_by_key(|c| c.label());
        categories.dedup();
        if !categories.is_empty() {
            push(Row::heading("Categories"), None);
            for cat in categories {
                let n = a.items.iter().filter(active).filter(|i| i.category == cat).count();
                push(Row::new(icon(cat), plural(cat)).value(n.to_string()), Some(Source::Category(cat)));
            }
        }
        let archived = a.items.iter().filter(|i| i.state == ItemState::Archived).count();
        if archived > 0 {
            push(Row::heading(""), None);
            push(Row::new("archive", "Archive").value(archived.to_string()), Some(Source::Archive));
        }
        let chosen = of.iter().position(|s| s.as_ref() == Some(&a.source)).unwrap_or(0);
        let list = Sidebar::new("passwords-sources", rows, chosen)
            .bare()
            .on_choose(move |i: &usize, _| {
                if let Some(Some(source)) = of.get(*i) {
                    app().set_source(source.clone());
                }
            })
            .build(cx);
        well(edge(cx), p.list_background, list)
    }

    /// The filters over the list and the list: each item's icon, title
    /// and what tells it apart (the search's finds marked).
    fn items(&mut self, a: &mut Passwords, cx: &mut Cx<Self>) -> Element {
        let visible = a.visible();
        // The filters over the list (tags, what items have); the rest are
        // typed (`type:`, `vault:`, `site:`, `user:`), suggested as they are.
        let line = edge(cx);
        // Their text over the list's icons, level with the sources' first
        // row beside them (inside its well's lines).
        let indent = line + m::SPACING - m::TEXT_INSET;
        let top = line + m::TIGHT + (sidebar::ROW - m::CONTROL_HEIGHT) / 2.0;
        // (What the menus choose goes into the search field, which the app
        // follows: `FilterBar::drives`.)
        let filters = FilterBar::new("passwords", a.search.clone(), self.facets.clone()).no_clear().max_width(LIST_WIDTH - indent).drives(&self.search);

        // The list, its selection the app's.
        let selected = visible.iter().position(|i| Some(&i.id) == a.selected.as_ref());
        let reveal = std::mem::take(&mut a.reveal_selection);
        {
            let mut s = self.list.state_mut();
            s.selected = selected.into_iter().collect();
            if s.cursor != selected {
                s.cursor = selected;
                s.anchor = selected;
            }
        }
        if reveal && let Some(r) = selected {
            self.list.reveal(cx, r);
        }
        if self.listed_for.as_ref().is_none_or(|(rev, source, search)| *rev != a.rev || *source != a.source || *search != a.search) {
            self.listed_for = Some((a.rev, a.source.clone(), a.search.clone()));
            self.query = Rc::new(Query::parse(&a.search, &search::keys(&self.facets)));
            self.searched = a.search.clone();
            // Under each title, where the search found it when that's
            // inside it (a section, a field), else its username or site.
            let found = a.found();
            self.subtitles = Rc::new(visible.iter().zip(found.iter()).map(|(i, f)| f.clone().unwrap_or_else(|| a.subtitle(i))).collect());
            self.rows = visible.clone();
        }
        let empty = if a.loading {
            "Loading…"
        } else if !a.search.is_empty() {
            "Nothing matches."
        } else {
            "No items."
        };
        let list = ColumnListView::new(cx, "passwords-items", &self.list, vec![Column::new("", 100.0)], visible.len(), |v: &MainView, cell, g| {
            let Some(item) = v.rows.get(cell.row) else { return };
            let b = cell.bounds;
            icons::paint(g, style::snap_centred(g, point(b.left() + px(m::SPACING + 16.0), b.center().y), 32.0, 32.0), icon(item.category));
            let x = b.left() + px(m::SPACING * 2.0 + 32.0);
            let width = (b.right() - x - px(m::SPACING)).max(px(0.0));
            let p = theme::palette();
            let mark = search::mark_color(&p);
            let sub = v.subtitles.get(cell.row).cloned().unwrap_or_default();
            let font = style::ui_font(FontWeight::NORMAL);
            for (text, size, color, dy) in [(item.title.clone(), style::PLAIN_SIZE, cell.text_color, -8.0), (sub, style::SMALL_SIZE, if cell.selected { cell.text_color } else { dim(&p) }, 8.0)] {
                // What the search found, marked (not on the selection's own
                // colour, which it would fight).
                let runs = if cell.selected || v.query.is_empty() { plain_run(&text, font.clone(), color) } else { search::highlight(&text, &v.query, &font, color, mark) };
                style::label_runs_fit(g, x, b.center().y + px(dy), style::Align::Left, text.clone().into(), size, &runs, width);
                cell.say(text);
            }
        })
        .row_key(|v: &MainView, row| match v.rows.get(row) {
            Some(i) => style::key((&i.id, &i.title, i.category.as_str(), v.subtitles.get(row), &v.searched)),
            None => 0,
        })
        .no_header()
        .row_height(ITEM_ROW)
        .empty_text(empty)
        .on_select(cx.listener(|v: &mut MainView, _: &(), _| {
            let row = v.list.state().single();
            if let Some(item) = row.and_then(|r| v.rows.get(r)) {
                let id = item.id.clone();
                app().select(Some(id));
            }
        }))
        .on_open(|_: &usize, _| app().open_website())
        .on_context(cx.listener(|v: &mut MainView, &(row, at): &(usize, Point<Pixels>), cx| {
            let Some(item) = v.rows.get(row) else { return };
            let id = item.id.clone();
            let shown = {
                let mut a = app();
                a.select(Some(id.clone()));
                a.item_menu(&id)
            };
            if let Some(m) = shown {
                menu::context_menu(cx, m, at, |cmd, _| app().command(cmd));
            }
        }));
        // Rows of their own height, scrolled: level with the sources and the
        // item's fields beside them only by chance.
        let grid = canvas(olo_kit::review::own_grid).paint_key(0).absolute().inset_0();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().pt(px(top)).pl(px(indent)).h(px(top + m::CONTROL_HEIGHT)).flex_shrink_0().child(filters))
            .child(div().relative().flex_1().min_h_0().mt(px(m::SPACING)).child(list).child(grid))
            .into_any_element()
    }
}

impl View for MainView {
    type Message = MainMsg;

    fn message(&mut self, _: MainMsg, cx: &mut Cx<Self>) {
        self.detail.update(cx, |_, cx| cx.invalidate());
        cx.invalidate();
    }

    fn command(&mut self, command: &str, _: &mut Cx<Self>) -> bool {
        app().command(command);
        true
    }

    fn close_requested(&mut self, _: &mut Cx<Self>) -> bool {
        app().close_requested()
    }

    fn window_activated(&mut self, active: bool, cx: &mut Cx<Self>) {
        // (Touch ID is listened for only while the window is in front.)
        app().lock.focus(active);
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let mut a = app();
        self.take_asks(&mut a, cx);
        let bar = MenuBar::new(cx, a.menus());
        let body = if a.conn == Conn::Setup {
            self.setup(&mut a, cx)
        } else if a.lock.locked {
            self.locked(&mut a, cx)
        } else {
            match a.conn.clone() {
                Conn::Connecting if a.items.is_empty() => panel("passwords", "Opening your passwords…", "The key is in your login keyring.".into(), div()),
                Conn::Failed(message) if a.items.is_empty() => {
                    panel("warning", "Can’t open your passwords", message, button_row().child(Button::new("Try Again").default(true).on_click(|_: &ClickEvent, _| app().connect())))
                }
                Conn::Ready if a.items.is_empty() && a.sync == Some(SyncState::Running) => {
                    let account = a.account_label();
                    let message = format!("Copying the items in {account} from 1Password. If 1Password asks whether to allow Passwords, choose Allow (and unlock it if it is locked).");
                    panel("passwords", "Bringing in your items…", message, div())
                }
                _ => self.browser(&mut a, cx),
            }
        };
        drop(a);
        div()
            .size_full()
            .flex()
            .flex_col()
            // Anything the person does puts off the auto-lock.
            .on_window_mouse_down(cx.listener(|_: &mut MainView, _: &MouseDownEvent, cx| {
                let mut a = app();
                a.used();
                // (Locked, the password's field keeps the keyboard: a
                // press beside it would leave nothing to type in.)
                if a.lock.locked {
                    cx.invalidate();
                }
            }))
            .capture_key_down(cx.listener(|v: &mut MainView, e: &KeyDownEvent, cx| v.key(e, cx)))
            .child(bar)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}

/// `text`, all of it in `font` and `color`.
fn plain_run(text: &str, font: Font, color: Hsla) -> Vec<TextRun> {
    vec![TextRun { len: text.len(), font, color, background_color: None, underline: None, strikethrough: None, size: None }]
}

/// `text` in `font` and `color`, what `mark` looks for marked.
fn marked(text: &str, mark: Option<&Query>, font: Font, color: Hsla, p: &olo_theme::Palette) -> Vec<TextRun> {
    match mark {
        Some(q) => search::highlight(text, q, &font, color, search::mark_color(p)),
        None => plain_run(text, font, color),
    }
}

/// The sections and fields a search asks for by name (`section:`,
/// `field:`, and those it leaves out with `-section:`, `-field:`).
struct Asked<'a> {
    sections: Vec<&'a Needle>,
    fields: Vec<&'a Needle>,
    not_sections: Vec<&'a Needle>,
    not_fields: Vec<&'a Needle>,
}

impl<'a> Asked<'a> {
    fn of(q: &'a Query) -> Asked<'a> {
        Asked { sections: q.values("section"), fields: q.values("field"), not_sections: q.left_out("section"), not_fields: q.left_out("field") }
    }

    /// Whether it leaves anything of an item out.
    fn narrows(&self) -> bool {
        !(self.sections.is_empty() && self.fields.is_empty() && self.not_sections.is_empty() && self.not_fields.is_empty())
    }

    /// Whether a field labelled `label`, in section `section` (titled; or
    /// none: ""), shows.
    fn shows(&self, section: &str, label: &str) -> bool {
        let any = |list: &[&Needle], name: &str| list.iter().any(|n| crate::names(n, name));
        (self.sections.is_empty() || any(&self.sections, section)) && (self.fields.is_empty() || any(&self.fields, label)) && !any(&self.not_sections, section) && !any(&self.not_fields, label)
    }
}

fn date(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%-d %b %Y, %H:%M").to_string()
}

/// A line of dim italics across the item pane, under its header.
fn italic_note(text: &'static str) -> Element {
    canvas(move |g, b| {
        let p = theme::palette();
        let mut font = style::ui_font(FontWeight::NORMAL);
        font.style = FontStyle::Italic;
        let room = b.size.width - px(2.0 * m::WINDOW_INSET);
        let r = style::label_fit(g, b.left() + px(m::WINDOW_INSET), b.center().y, style::Align::Left, text, style::PLAIN_SIZE, font, dim(&p), room);
        agent::record(g, agent::Node::label(text, r));
    })
    .paint_key(style::key(text))
    .w_full()
    .h(px(28.0))
    .flex_shrink_0()
}

/// An item's header: its icon, title, kind and vault, and its Edit
/// button, whose label the title shares a baseline with.
fn header(title: &str, category: ItemCategory, vault: &str, edit: Option<Button>) -> Element {
    let title: SharedString = title.to_owned().into();
    let sub: SharedString = if vault.is_empty() { category.label().to_string() } else { format!("{}  ·  {vault}", category.label()) }.into();
    let has_button = edit.is_some();
    let inset = m::WINDOW_INSET;
    let x = inset * 2.0 + 48.0;
    let (face_title, face_sub) = (title.clone(), sub.clone());
    let face = canvas(move |g, b| {
        let p = theme::palette();
        icons::paint(g, Bounds { origin: b.origin + point(px(inset), px(inset)), size: size(px(48.0), px(48.0)) }, icon(category));
        if !has_button {
            // The title's capitals centred on TITLE_MIDDLE.
            let room = (b.right() - b.left() - px(x + inset)).max(px(0.0));
            let font = style::ui_font(FontWeight::BOLD);
            let baseline = style::baseline_at(g, b.top() + px(inset + TITLE_MIDDLE), TITLE_SIZE);
            let t = style::label_on_baseline_fit(g, b.left() + px(x), baseline, style::Align::Left, face_title.clone(), TITLE_SIZE, font, c(p.document_text), room);
            agent::record(g, agent::Node { role: "Heading", name: Some(face_title.clone()), bounds: t, enabled: true, ..Default::default() });
        }
        let s = style::label(g, b.left() + px(x), b.top() + px(inset + 38.0), style::Align::Left, face_sub.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim(&p));
        agent::record(g, agent::Node::label(face_sub.clone(), s));
        style::hline_above(g, b.left() + px(inset), b.right() - px(inset), b.bottom(), style::shade(&p, p.document_background, 1.15));
    })
    .paint_key(style::key((&title, &sub, category.as_str(), has_button)))
    .absolute()
    .inset_0();
    let mut out = div().relative().w_full().h(px(48.0 + 2.0 * inset)).flex_shrink_0().child(face);
    if let Some(button) = edit {
        // The button lower than the title's middle, so its label shares
        // the title's baseline; the title drawn in the button's row, its
        // middle where the button's is.
        let lift = (TITLE_SIZE - style::PLAIN_SIZE) * style::CAP_HEIGHT / 2.0;
        let top = inset + TITLE_MIDDLE + lift - m::CONTROL_HEIGHT / 2.0;
        let key = style::key(&title);
        let named = canvas(move |g, b| {
            let p = theme::palette();
            let font = style::ui_font(FontWeight::BOLD);
            let baseline = style::baseline_at(g, b.center().y, style::PLAIN_SIZE);
            let t = style::label_on_baseline_fit(g, b.left(), baseline, style::Align::Left, title.clone(), TITLE_SIZE, font, c(p.document_text), (b.size.width - px(m::SPACING)).max(px(0.0)));
            agent::record(g, agent::Node { role: "Heading", name: Some(title.clone()), bounds: t, enabled: true, ..Default::default() });
        })
        .paint_key(key)
        .flex_1()
        .min_w_0()
        .h(px(m::CONTROL_HEIGHT));
        out = out.child(div().absolute().left(px(x)).right(px(inset)).top(px(top)).flex().flex_row().items_center().child(named).child(button));
    }
    out
}

/// A section's title over its fields, what `mark` looks for marked.
fn section_heading(title: String, mark: Option<Rc<Query>>, searched: &str) -> Element {
    let title: SharedString = title.into();
    let key = style::key((&title, mark.is_some().then_some(searched)));
    canvas(move |g, b| {
        let p = theme::palette();
        let runs = marked(&title, mark.as_deref(), style::ui_font(FontWeight::BOLD), c(p.document_text), &p);
        let r = style::label_runs(g, b.left() + px(m::WINDOW_INSET), b.bottom() - px(9.0), style::Align::Left, title.clone(), style::PLAIN_SIZE, &runs);
        agent::record(g, agent::Node { role: "Heading", name: Some(title.clone()), bounds: r, enabled: true, ..Default::default() });
    })
    .paint_key(key)
    .w_full()
    .h(px(30.0))
    .flex_shrink_0()
}

/// How a field's value reads.
fn shown(f: &ItemField, revealed: bool) -> (String, bool) {
    let masked = || "•".repeat(10);
    match f.field_type {
        ItemFieldType::Concealed | ItemFieldType::SshKey if !revealed => (masked(), true),
        ItemFieldType::Totp => {
            let code = crate::otp_code(&f.value).or_else(|| f.otp().and_then(|o| o.code.clone()));
            match code {
                Some(c) if c.len() == 6 => (format!("{} {}", &c[..3], &c[3..]), true),
                Some(c) => (c, true),
                None => ("Invalid one-time password secret".into(), false),
            }
        }
        ItemFieldType::CreditCardNumber if !revealed => {
            let digits: String = f.value.chars().filter(|c| c.is_ascii_digit()).collect();
            let last = &digits[digits.len().saturating_sub(4)..];
            (format!("•••• {last}"), true)
        }
        ItemFieldType::CreditCardNumber => {
            let digits: Vec<char> = f.value.chars().filter(|c| c.is_ascii_digit()).collect();
            (digits.chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join(" "), true)
        }
        // 08/2029, as 1Password shows it (and one kept by an older
        // Passwords as 202908).
        ItemFieldType::MonthYear => (onepassword::fields::month_year(&f.value).unwrap_or_else(|| f.value.clone()), false),
        ItemFieldType::Date => match chrono::NaiveDate::parse_from_str(&f.value, "%Y-%m-%d") {
            Ok(d) => (d.format("%-d %B %Y").to_string(), false),
            Err(_) => (f.value.clone(), false),
        },
        ItemFieldType::Address => (onepassword::fields::address_text(&onepassword::fields::address(f)), false),
        _ => (f.value.clone(), matches!(f.field_type, ItemFieldType::Concealed | ItemFieldType::SshKey)),
    }
}

/// What the item pane is told.
pub enum DetailMsg {
    /// A field dragged out of it was dropped (or the drag given up).
    Dragged,
}

/// The selected item (or the editor): its header, fields, websites,
/// notes, tags and dates.
pub struct Detail {
    /// The editor's text fields, made for the app's editor as its fields
    /// were when the app's count of them (`Ui::form`) was `form_rev`.
    form: Option<EditForm>,
    form_rev: u64,
    /// Which of the form's secrets show, as the fields have it.
    reveals: Vec<bool>,
    /// The field the pointer is over: its Copy (and Show) show.
    hover: Option<String>,
    /// The field pressed, and where: released there it's a click, moved
    /// it's dragged out.
    press: Option<(String, Point<Pixels>)>,
    scroll: ScrollHandle,
    editor_scroll: ScrollHandle,
    /// The next second, while a one-time code counts down.
    wake: Due,
}

impl Detail {
    pub fn new(_: &mut Cx<Detail>) -> Detail {
        Detail { form: None, form_rev: 0, reveals: Vec::new(), hover: None, press: None, scroll: ScrollHandle::new(), editor_scroll: ScrollHandle::new(), wake: Due::default() }
    }

    /// Something was typed in the editor's field `slot`: into the editor.
    pub fn typed(&mut self, slot: Slot, cx: &mut Cx<Self>) {
        let Some(text) = self.form.as_ref().and_then(|f| f.field(slot)).map(|f| f.read(cx).text().to_owned()) else { return };
        let mut a = app();
        // (Not what was typed in a form that has gone since.)
        if self.form_rev != a.ui.form {
            return;
        }
        if let Some(ed) = a.editor.as_mut() {
            slot.put(ed, text);
        }
    }

    /// What the app wants of the pane's own things, done: the editor's
    /// form made for its fields as they are now, its secrets shown or
    /// hidden, a value put in, and where it's scrolled.
    fn take_asks(&mut self, a: &mut Passwords, cx: &mut Cx<Self>) {
        if self.form_rev != a.ui.form {
            self.form_rev = a.ui.form;
            self.form = a.editor.as_ref().map(|e| EditForm::new(e, cx));
            self.reveals = a.editor.as_ref().map(|e| e.fields.iter().map(|f| f.reveal).collect()).unwrap_or_default();
            self.hover = None;
            self.press = None;
        }
        if let Some((i, text)) = a.ui.form_value.take()
            && let Some(form) = &self.form
        {
            form.set_value(i, &text, true, cx);
            if let Some(r) = self.reveals.get_mut(i) {
                *r = true;
            }
        }
        if let (Some(e), Some(form)) = (&a.editor, &self.form) {
            for (i, f) in e.fields.iter().enumerate() {
                if self.reveals.get(i).is_some_and(|r| *r != f.reveal) {
                    form.reveal(i, f.reveal, cx);
                    self.reveals[i] = f.reveal;
                }
            }
        }
        if std::mem::take(&mut a.ui.editor_top) {
            self.editor_scroll.set_offset(cx, point(px(0.0), px(0.0)));
        }
        if std::mem::take(&mut a.ui.editor_bottom) {
            self.editor_scroll.scroll_to_bottom(cx);
        }
        if std::mem::take(&mut a.ui.detail_top) {
            self.scroll.set_offset(cx, point(px(0.0), px(0.0)));
        }
    }

    /// Drags a field's value out, onto another app's field: the value (a
    /// one-time password's current code), drawn as the field's name,
    /// never the secret.
    fn drag(&mut self, item: &str, field: &str, cx: &mut Cx<Self>) {
        let Some((value, label)) = app().dragged_field(item, field) else { return };
        let icon = olo_kit::dnd::picture(cx, "passwords", 32.0, Some(&label), point(px(16.0), px(16.0)));
        let bytes: Arc<[u8]> = Arc::from(value.into_bytes());
        let drag = OutgoingDrag {
            types: vec![("text/plain;charset=utf-8".into(), bytes.clone()), ("text/plain".into(), bytes.clone()), ("UTF8_STRING".into(), bytes)],
            copy: true,
            move_: false,
            ask: false,
            icon,
        };
        olo_kit::dnd::start(cx, drag, |_| DetailMsg::Dragged);
    }

    /// One field: its label, its value (a link's to open, a secret's
    /// hidden until shown), and, while the pointer is over it, Copy (and
    /// Show). A click copies it (a link's value opens it); dragged, it
    /// goes out as its value (onto another app's field). `key` is the
    /// field's id (`url:…` for a website).
    #[allow(clippy::too_many_arguments)]
    fn field_row(&self, cx: &mut Cx<Self>, item: &str, f: &ItemField, key: String, revealed: bool, mark: Option<Rc<Query>>, searched: &str) -> Element {
        let hovered = self.hover.as_deref() == Some(key.as_str());
        let label: SharedString = if f.title.is_empty() { edit::kind_label(f.field_type).to_lowercase() } else { f.title.clone() }.into();
        let (value, mono) = shown(f, revealed);
        let value: SharedString = value.replace('\n', " ⏎ ").into();
        let link = f.field_type == ItemFieldType::Url || (f.field_type == ItemFieldType::Text && f.value.starts_with("http"));
        let url = link.then(|| if f.value.contains("://") { f.value.clone() } else { format!("https://{}", f.value) });
        let secret = matches!(f.field_type, ItemFieldType::Concealed | ItemFieldType::SshKey | ItemFieldType::CreditCardNumber);
        let buttons_w = if hovered { (if secret { 2.0 } else { 1.0 }) * FIELD_BUTTON + if secret { m::TIGHT } else { 0.0 } } else { 0.0 };
        let totp = (f.field_type == ItemFieldType::Totp).then(|| onepassword::totp::Totp::parse(&f.value).ok()).flatten();
        let value_font = move || if mono { style::mono_font(FontWeight::NORMAL) } else { style::ui_font(FontWeight::NORMAL) };
        // (A code's pie turns each second: its row is drawn each time.)
        let face_key = totp.is_none().then(|| style::key((&label, &value, hovered, link, secret, mono, buttons_w.to_bits(), mark.is_some().then_some(searched))));
        let (face_label, face_value) = (label.clone(), value.clone());
        let mut face = canvas(move |g, b| {
            let p = theme::palette();
            let (label, value) = (&face_label, &face_value);
            if hovered {
                let lit = style::snap(g, Bounds::from_corners(b.origin + point(px(m::TIGHT), px(1.0)), b.bottom_right() - point(px(m::TIGHT), px(1.0))));
                g.paint_quad(fill(lit, style::shade(&p, p.document_background, 1.04)).corner_radii(px(2.0)));
            }
            let x = b.left() + px(m::WINDOW_INSET);
            // What the search looks for, marked (not in a secret, which
            // it doesn't look in).
            let runs = marked(label, mark.as_deref(), style::ui_font(FontWeight::NORMAL), dim(&p), &p);
            let l = style::label_runs(g, x, b.top() + px(FIELD_LABEL_Y), style::Align::Left, label.clone(), style::SMALL_SIZE, &runs);
            let color = if link { c(p.keyboard_navigation) } else { c(p.document_text) };
            let room = (b.right() - x - px(m::WINDOW_INSET + buttons_w)).max(px(0.0));
            let runs = marked(value, mark.as_deref().filter(|_| !secret), value_font(), color, &p);
            let v = style::label_runs_fit(g, x, b.top() + px(FIELD_VALUE_Y), style::Align::Left, value.clone(), style::PLAIN_SIZE, &runs, room);
            agent::record(g, agent::Node { role: "Label", name: Some(label.clone()), value: Some(value.clone()), bounds: l.union(&v), enabled: true, clickable: true, ..Default::default() });
            // A little pie of the code's time left, a ring round it.
            if let Some(t) = &totp {
                let left = t.now().1 as f32 / t.period.max(1) as f32;
                let centre = point(v.right() + px(m::SPACING + 7.0), v.center().y);
                let ink = if left < 0.2 { c(p.failure) } else { dim(&p) };
                g.paint_sector(centre, px(5.5), px(6.5), 0.0, std::f32::consts::TAU, dim(&p));
                g.paint_sector(centre, px(0.0), px(5.0), 0.0, std::f32::consts::TAU * left, ink);
            }
        });
        if let Some(key) = face_key {
            face = face.paint_key(key);
        }
        let face = face.absolute().inset_0();
        // Pressed: a click copies (or opens a link's value); moved, a drag
        // out. (Where it was pressed is the pane's: the row is drawn again
        // as the pointer comes onto it.)
        let (hover_key, down_key, move_key, up_key) = (key.clone(), key.clone(), key.clone(), key.clone());
        let (move_item, up_item) = (item.to_owned(), item.to_owned());
        let click_value = value.clone();
        let mut row = div()
            .id(SharedString::from(format!("field-{key}")))
            .relative()
            .w_full()
            .h(px(FIELD_ROW))
            .flex_shrink_0()
            .child(face)
            // The pointer onto it, or off it: its buttons come and go.
            .on_hover(cx.listener(move |d: &mut Detail, over: &bool, cx| {
                let here = d.hover.as_deref() == Some(hover_key.as_str());
                if *over && !here {
                    d.hover = Some(hover_key.clone());
                    cx.invalidate();
                } else if !*over && here {
                    d.hover = None;
                    cx.invalidate();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |d: &mut Detail, e: &MouseDownEvent, _| {
                    d.press = Some((down_key.clone(), e.position));
                }),
            )
            .moves_matter_while_pressed()
            .on_mouse_move(cx.listener(move |d: &mut Detail, e: &MouseMoveEvent, cx| {
                let from = d.press.as_ref().filter(|(k, _)| *k == move_key).map(|(_, at)| *at);
                if let Some(at) = from
                    && e.pressed_button == Some(MouseButton::Left)
                    && ((e.position.x - at.x).abs() > px(3.0) || (e.position.y - at.y).abs() > px(3.0))
                {
                    d.press = None;
                    d.drag(&move_item, &move_key, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |d: &mut Detail, e: &MouseUpEvent, cx| {
                    let pressed = d.press.take().is_some_and(|(k, _)| k == up_key);
                    if !pressed {
                        return;
                    }
                    // (Not on its buttons, which do their own.)
                    let b = cx.listener_bounds();
                    if buttons_w > 0.0 && e.position.x >= b.right() - px(m::WINDOW_INSET + buttons_w + m::TIGHT) {
                        return;
                    }
                    // On a link's value (where it was drawn: its text, on
                    // its line): opened.
                    let on_value = || {
                        let x = b.left() + px(m::WINDOW_INSET);
                        let room = (b.right() - x - px(m::WINDOW_INSET + buttons_w)).max(px(0.0));
                        let wide = style::text_width(click_value.clone(), style::PLAIN_SIZE, value_font()).min(room);
                        let half = px(style::PLAIN_SIZE * style::LINE_HEIGHT / 2.0);
                        let line = b.top() + px(FIELD_VALUE_Y);
                        e.position.x >= x && e.position.x <= x + wide && e.position.y >= line - half && e.position.y <= line + half
                    };
                    match &url {
                        Some(url) if on_value() => crate::open_url(url),
                        _ => app().copy_field(&up_item, &up_key),
                    }
                }),
            );
        if hovered {
            let mut buttons = div().absolute().right(px(m::WINDOW_INSET)).top(px((FIELD_ROW - m::CONTROL_HEIGHT) / 2.0)).flex().flex_row().gap(px(m::TIGHT));
            if secret {
                let k = key.clone();
                buttons = buttons
                    .child(Button::new(if revealed { "Hide" } else { "Show" }).id(format!("show-{key}")).min_width(FIELD_BUTTON).on_click(move |_: &ClickEvent, _| app().toggle_revealed(k.clone())));
            }
            let (k, it) = (key.clone(), item.to_owned());
            buttons = buttons.child(Button::new("Copy").id(format!("copy-{key}")).min_width(FIELD_BUTTON).on_click(move |_: &ClickEvent, _| app().copy_field(&it, &k)));
            row = row.child(buttons);
        }
        // Right-click: the value copied, or its `op://` reference (for the
        // `op` command, `op read`, `op inject`).
        if !key.starts_with("url:") {
            let (menu_key, menu_item) = (key.clone(), item.to_owned());
            row = row.on_mouse_down(MouseButton::Right, move |e: &MouseDownEvent, cx| {
                let m = Menu::new("Field").item("Copy", "copy").item("Copy Reference", "reference");
                let (k, it) = (menu_key.clone(), menu_item.clone());
                menu::context_menu(cx, m, e.position, move |cmd, _| {
                    let mut a = app();
                    match cmd {
                        "reference" => a.copy_reference(&it, Some(&k)),
                        _ => a.copy_field(&it, &k),
                    }
                });
            });
        }
        if link {
            row = row.cursor_pointer();
        }
        row
    }
}

impl View for Detail {
    type Message = DetailMsg;

    fn message(&mut self, _: DetailMsg, _: &mut Cx<Self>) {}

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let e = edge(cx);
        let mut a = app();
        self.take_asks(&mut a, cx);
        // How wide the pane's content is.
        let pane = f32::from(cx.viewport_size().width) - SOURCES_WIDTH - LIST_WIDTH - 4.0 * m::SPACING - 2.0 * e;
        if a.editor.is_some() && self.form.is_some() {
            let focus_title = std::mem::take(&mut a.focus_title);
            let focus_field = a.focus_field.take();
            self.wake.at(None, cx, |_, _| {});
            let (Some(editor), Some(form)) = (&a.editor, &self.form) else { return div() };
            if focus_title {
                form.title.focus(cx);
            }
            if let Some(i) = focus_field {
                form.focus_field(editor, i, cx);
            }
            let vaults: Vec<_> = a.vaults.iter().map(|v| onepassword::VaultOverview { title: a.vault_name(v), ..v.clone() }).collect();
            let content = edit::view(editor, form, &vaults, pane - 2.0 * m::WINDOW_INSET, cx);
            drop(a);
            let view = ScrollView::new(&self.editor_scroll).border(false).auto_hide(true).child(div().p(px(m::WINDOW_INSET)).child(content));
            return well(e, p.document_background, view);
        }
        let Some(id) = a.selected.clone() else {
            let text = if a.loading { "" } else { "No item selected" };
            self.wake.at(None, cx, |_, _| {});
            let empty = canvas(move |g, b| {
                let p = theme::palette();
                let r = style::label(g, b.center().x, b.center().y, style::Align::Center, text, style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim(&p));
                agent::record(g, agent::Node::label(text, r));
            })
            .paint_key(style::key(text))
            .size_full();
            return well(e, p.document_background, empty);
        };
        let Some(overview) = a.overview(&id).cloned() else { return well(e, p.document_background, div()) };
        let vault = a.vault_title(&overview.vault_id);
        let Some(item) = a.details.get(&id).cloned() else {
            let (text, color) = match a.missing.get(&id) {
                Some(e) => (format!("Couldn’t open this item: {e}"), c(p.failure)),
                None => ("Loading…".to_string(), dim(&p)),
            };
            let content = div()
                .flex()
                .flex_col()
                .child(header(&overview.title, overview.category, &vault, None))
                .child(div().px(px(m::WINDOW_INSET)).pt(px(m::SPACING)).child(StringView::new(text).wrap(true).color(color)));
            return well(e, p.document_background, content);
        };
        // What the search looks for, marked; and with `section:` or
        // `field:`, only the sections and fields it asks for.
        let searched = a.search.clone();
        let query = Rc::new(Query::parse(&searched, &search::keys(&a.facets_rc())));
        let revealed: HashSet<String> = item.fields.iter().filter(|f| a.revealed(&f.id)).map(|f| f.id.clone()).collect();
        let locked = a.lock.locked;
        // (The rest is made from what was read: the app isn't held.)
        drop(a);

        let archived = overview.state == ItemState::Archived;
        let edit_button = (!archived && !crate::on_screen()).then(|| Button::new("Edit").min_width(64.0).on_click(|_: &ClickEvent, _| app().edit_selected()));
        let mut col = div().w_full().flex().flex_col().child(header(&item.title, item.category, &vault, edit_button));
        if archived {
            col = col.child(italic_note("This item is in the Archive."));
        }
        let ask = Asked::of(&query);
        if ask.narrows() {
            col = col.child(italic_note("Only the sections and fields the search asks for."));
        }
        let mark = (!query.is_empty()).then(|| query.clone());
        let mut counting = false;
        // Built-in fields first, then each section's.
        let mut groups: Vec<(Option<String>, Vec<&ItemField>)> = vec![(None, Vec::new())];
        for f in &item.fields {
            let key = f.section_id.clone().filter(|s| item.sections.iter().any(|x| &x.id == s && !x.title.is_empty()));
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, list)) => list.push(f),
                None => groups.push((key, vec![f])),
            }
        }
        for (section, fields) in &groups {
            let title = section.as_ref().and_then(|s| item.sections.iter().find(|x| &x.id == s)).map(|s| s.title.clone());
            let fields: Vec<&ItemField> = fields.iter().copied().filter(|f| ask.shows(title.as_deref().unwrap_or_default(), &f.title)).collect();
            if fields.is_empty() {
                continue;
            }
            if let Some(title) = title {
                col = col.child(section_heading(title, mark.clone(), &searched));
            }
            let passkey = (section.as_deref() == Some(crate::passkey::SECTION)).then(|| crate::passkey::shown(&fields));
            let fields: Vec<&ItemField> = match &passkey {
                Some(shown) => shown.iter().collect(),
                None => fields,
            };
            for f in fields {
                if f.value.is_empty() && f.field_type != ItemFieldType::Totp {
                    continue;
                }
                counting |= f.field_type == ItemFieldType::Totp;
                col = col.child(self.field_row(cx, &id, f, f.id.clone(), revealed.contains(&f.id), mark.clone(), &searched));
            }
        }
        if !item.websites.is_empty() && !ask.narrows() {
            col = col.child(section_heading("Websites".into(), None, &searched));
            for w in &item.websites {
                let f = ItemField::new("", &w.label, ItemFieldType::Url, &w.url);
                col = col.child(self.field_row(cx, &id, &f, format!("url:{}", w.url), true, mark.clone(), &searched));
            }
        }
        if !item.notes.is_empty() && !ask.narrows() {
            let ink = c(p.document_text);
            col = col
                .child(section_heading("Notes".into(), None, &searched))
                .child(div().px(px(m::WINDOW_INSET)).pt(px(m::TIGHT)).pb(px(m::SPACING)).child(StringView::new(item.notes.clone()).wrap(true).color(ink)));
        }
        if !item.tags.is_empty() && !ask.narrows() {
            let tags: SharedString = item.tags.join(", ").into();
            let key = style::key(&tags);
            col = col.child(section_heading("Tags".into(), None, &searched)).child(
                canvas(move |g, b| {
                    let p = theme::palette();
                    let r = style::label_fit(
                        g,
                        b.left() + px(m::WINDOW_INSET),
                        b.center().y,
                        style::Align::Left,
                        tags.clone(),
                        style::PLAIN_SIZE,
                        style::ui_font(FontWeight::NORMAL),
                        c(p.document_text),
                        b.size.width - px(2.0 * m::WINDOW_INSET),
                    );
                    agent::record(g, agent::Node::label(tags.clone(), r));
                })
                .paint_key(key)
                .w_full()
                .h(px(22.0))
                .flex_shrink_0(),
            );
        }
        // One line when it fits, else Modified and Created a line each.
        let (modified, created) = (format!("Modified {}", date(item.updated_at)), format!("Created {}", date(item.created_at)));
        let both = format!("{modified}   ·   {created}");
        let font = style::ui_font(FontWeight::NORMAL);
        let room = pane - 2.0 * m::WINDOW_INSET;
        let lines: Vec<SharedString> = if f32::from(style::text_width(both.clone(), style::SMALL_SIZE, font.clone())) <= room { vec![both.into()] } else { vec![modified.into(), created.into()] };
        let step = style::SMALL_SIZE + m::TIGHT * 2.0;
        let n = lines.len();
        let key = style::key(&lines);
        let dates = canvas(move |g, b| {
            let p = theme::palette();
            let (l, r) = (b.left() + px(m::WINDOW_INSET), b.right() - px(m::WINDOW_INSET));
            style::hline(g, l, r, b.top(), style::shade(&p, p.document_background, 1.15));
            for (i, text) in lines.iter().enumerate() {
                let t = style::label(g, l, b.top() + px(17.0 + step * i as f32), style::Align::Left, text.clone(), style::SMALL_SIZE, font.clone(), dim(&p));
                agent::record(g, agent::Node::label(text.clone(), t));
            }
        })
        .paint_key(key)
        .w_full()
        .h(px(34.0 + step * (n - 1) as f32))
        .flex_shrink_0();
        col = col.child(div().h(px(m::SPACING)).flex_shrink_0()).child(dates);
        // One-time codes count down: the pane says what it shows again on
        // the second, as their periods turn (it alone, and only while one
        // shows).
        if counting && !locked {
            let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
            let to_next = Duration::from_millis(1000 - (since.as_millis() % 1000) as u64);
            self.wake.at(Some(Instant::now() + to_next), cx, |_, cx| cx.invalidate());
        } else {
            self.wake.at(None, cx, |_, _| {});
        }
        let view = ScrollView::new(&self.scroll).border(false).auto_hide(true).child(col);
        well(e, p.document_background, view)
    }
}

const REVIEW_ROW: f32 = 42.0;

/// A window's message, list and buttons, as an alert's are laid out.
fn sheet(message: String, list: impl IntoElement, buttons: impl IntoElement) -> Element {
    div().size_full().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).child(StringView::new(message).wrap(true)).child(div().flex_1().min_h_0().child(list)).child(buttons)
}

/// The Review Sync window: every change a sync would make, both ways,
/// and nothing happens until Apply.
pub struct ReviewView {
    list: ScrollHandle,
    focus: FocusHandle,
    /// The pointer is over the line that says what couldn't be read.
    over_note: bool,
}

impl ReviewView {
    pub fn new(cx: &mut Cx<Self>) -> ReviewView {
        let focus = FocusHandle::new();
        focus.focus(cx);
        ReviewView { list: ScrollHandle::new(), focus, over_note: false }
    }
}

impl View for ReviewView {
    type Message = ();

    fn message(&mut self, _: (), cx: &mut Cx<Self>) {
        cx.invalidate();
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        // Closed from its tab: as Cancel.
        app().review_closed();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let a = app();
        let Some(review) = &a.review else { return div() };
        let here = review.rows.iter().filter(|r| r.here).count();
        let there = review.rows.len() - here;
        let count = |n: usize| if n == 1 { "1 change".to_string() } else { format!("{n} changes") };
        let mut sides = Vec::new();
        if here > 0 {
            sides.push(format!("{} on this computer", count(here)));
        }
        if there > 0 {
            sides.push(format!("{} in 1Password", count(there)));
        }
        let message = format!("Syncing with {} makes {}. Nothing changes on either side until you apply them.", a.account_label(), sides.join(" and "));
        let mut rows = div().w_full().flex().flex_col();
        for row in &review.rows {
            let title = if row.vault.is_empty() { row.title.clone() } else { format!("{}  ·  {}", row.title, row.vault) };
            let mut action = row.action().to_string();
            if !row.detail.is_empty() {
                action = format!("{action}: {}", row.detail);
            }
            let warn = matches!(row.what, crate::sync::What::Delete | crate::sync::What::Conflict);
            let category = row.category;
            let (title, action): (SharedString, SharedString) = (title.into(), action.into());
            let key = style::key((&title, &action, warn, category.as_str()));
            rows = rows.child(
                canvas(move |g, b| {
                    let p = theme::palette();
                    icons::paint(g, style::snap_centred(g, point(b.left() + px(m::SPACING + 16.0), b.center().y), 32.0, 32.0), icon(category));
                    let x = b.left() + px(m::SPACING * 2.0 + 32.0);
                    let width = (b.right() - x - px(m::SPACING)).max(px(0.0));
                    let font = style::ui_font(FontWeight::NORMAL);
                    let t = style::label_fit(g, x, b.center().y - px(8.0), style::Align::Left, title.clone(), style::PLAIN_SIZE, font.clone(), c(p.list_item_text), width);
                    let d = style::label_fit(g, x, b.center().y + px(8.0), style::Align::Left, action.clone(), style::SMALL_SIZE, font, if warn { c(p.failure) } else { dim(&p) }, width);
                    agent::record(g, agent::Node { role: "ListItem", name: Some(title.clone()), value: Some(action.clone()), bounds: t.union(&d), enabled: true, ..Default::default() });
                })
                .paint_key(key)
                .w_full()
                .h(px(REVIEW_ROW))
                .flex_shrink_0(),
            );
        }
        let p = theme::palette();
        let list = ScrollView::new(&self.list).background(p.list_background).child(rows);
        let n = review.rows.len();
        let apply = if n == 1 { "Apply Change".to_string() } else { format!("Apply {n} Changes") };
        let mut buttons = button_row();
        // What couldn't be read: a line on the left, the list a click away.
        if !review.problems.is_empty() {
            let list = review.problems.clone();
            buttons = buttons.child(
                problems_note(&review.problems, self.over_note)
                    .on_hover(cx.listener(|v: &mut ReviewView, over: &bool, cx| {
                        v.over_note = *over;
                        cx.invalidate();
                    }))
                    .on_click(move |_: &ClickEvent, _| app().show_problems(list.clone())),
            );
            buttons = buttons.child(div().flex_1());
        }
        drop(a);
        buttons = buttons
            .child(Button::new("Cancel").on_click(|_: &ClickEvent, _| app().answer_review(false)))
            .child(Button::new(apply).default(true).on_click(|_: &ClickEvent, _| app().answer_review(true)));
        sheet(message, list, buttons).track_focus(&self.focus).on_key_down(|e: &KeyDownEvent, cx| {
            if e.keystroke.key == "escape" {
                cx.stop_propagation();
                app().answer_review(false);
            }
        })
    }
}

/// "⚠ 3 problems reading 1Password…", in the window's own text, as a
/// link to the Sync Problems window (underlined under the pointer).
fn problems_note(list: &[crate::sync::Problem], over: bool) -> Element {
    let failed = list.iter().filter(|p| !p.note).count();
    let (icon_name, text): (&'static str, SharedString) = match failed {
        0 => ("info", "Some items are left out…".into()),
        1 => ("warning", "1 problem reading 1Password…".into()),
        n => ("warning", format!("{n} problems reading 1Password…").into()),
    };
    let key = style::key((icon_name, &text, over));
    let face = canvas(move |g, b| {
        let p = theme::palette();
        icons::paint(g, style::snap_centred(g, point(b.left() + px(8.0), b.center().y), 16.0, 16.0), icon_name);
        let x = b.left() + px(16.0 + m::SPACING / 2.0);
        let t = style::label(g, x, b.center().y, style::Align::Left, text.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), c(p.panel_text));
        if over {
            style::hline_above(g, t.left(), t.right(), t.bottom() + px(1.0), c(p.panel_text));
        }
        agent::record(g, agent::Node { clickable: true, ..agent::Node::label(text.clone(), t) });
    })
    .paint_key(key)
    .size_full();
    div().id("passwords-problems-note").h(px(m::CONTROL_HEIGHT)).w(px(260.0)).cursor_pointer().child(face)
}

/// The Sync Problems window: what a sync couldn't do, grouped by vault,
/// each with its reason in plain words.
pub struct ProblemsView {
    list: ScrollHandle,
    focus: FocusHandle,
}

impl ProblemsView {
    pub fn new(cx: &mut Cx<Self>) -> ProblemsView {
        let focus = FocusHandle::new();
        focus.focus(cx);
        ProblemsView { list: ScrollHandle::new(), focus }
    }
}

impl View for ProblemsView {
    type Message = ();

    fn message(&mut self, _: (), cx: &mut Cx<Self>) {
        cx.invalidate();
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        app().problems_gone();
    }

    fn body(&mut self, _: &mut Cx<Self>) -> Element {
        let a = app();
        let Some(problems) = &a.problems else { return div() };
        let list = &problems.list;
        let failed = list.iter().filter(|p| !p.note).count();
        // While a review waits, nothing has synced yet.
        let reviewing = a.review.is_some();
        let message = match (reviewing, failed) {
            (true, 0) => "These are left out of the sync under review:",
            (true, _) => "These parts of 1Password couldn’t be read, so they aren’t in the sync under review:",
            (false, 0) => "Everything else synced. These were left out:",
            (false, 1) => "Everything else synced, but this couldn’t be:",
            (false, _) => "Everything else synced, but these couldn’t be:",
        };
        let p = theme::palette();
        let mut rows = div().w_full().flex().flex_col();
        let mut vault: Option<&str> = None;
        for problem in list {
            if vault != Some(problem.vault.as_str()) {
                vault = Some(problem.vault.as_str());
                let name: SharedString = if problem.vault.is_empty() { "Not in a vault".into() } else { problem.vault.clone().into() };
                let key = style::key(&name);
                rows = rows.child(
                    canvas(move |g, b| {
                        let p = theme::palette();
                        let t = style::label_fit(
                            g,
                            b.left() + px(m::SPACING),
                            b.center().y + px(2.0),
                            style::Align::Left,
                            name.clone(),
                            style::PLAIN_SIZE,
                            style::ui_font(FontWeight::BOLD),
                            c(p.list_item_text),
                            b.size.width - px(2.0 * m::SPACING),
                        );
                        agent::record(g, agent::Node { role: "Heading", name: Some(name.clone()), bounds: t, enabled: true, ..Default::default() });
                        style::hline_above(g, b.left(), b.right(), b.bottom(), dim(&p).opacity(0.4));
                    })
                    .paint_key(key)
                    .w_full()
                    .h(px(26.0))
                    .flex_shrink_0(),
                );
            }
            rows = rows.child(problem_row(problem, &p));
        }
        let shown = ScrollView::new(&self.list).background(p.list_background).child(rows);
        let text = crate::sync::as_text(list);
        let mut buttons = button_row().child(Button::new("Copy").on_click(move |_: &ClickEvent, _| olo_kit::clipboard::write(olo_kit::ClipboardItem::new_string(text.clone()))));
        if failed > 0 && !reviewing {
            buttons = buttons.child(Button::new("Sync Again").on_click(|_: &ClickEvent, _| {
                let mut a = app();
                a.close_problems();
                a.sync_and_review();
            }));
        }
        drop(a);
        buttons = buttons.child(Button::new("Close").default(true).on_click(|_: &ClickEvent, _| app().close_problems()));
        sheet(message.to_owned(), shown, buttons).track_focus(&self.focus).on_key_down(|e: &KeyDownEvent, cx| {
            if e.keystroke.key == "escape" {
                cx.stop_propagation();
                app().close_problems();
            }
        })
    }
}

/// One problem: its icon, the item (or the reason, for the whole
/// vault) and the reason under it, wrapped. The icon and the line beside
/// it share a row of a whole, even height, so they are level at every
/// scale (a line of text's own height isn't whole: the two were a pixel
/// apart at 1×).
fn problem_row(problem: &crate::sync::Problem, p: &olo_theme::Palette) -> Element {
    const LINE: f32 = 18.0;
    let x = m::SPACING * 2.0 + 16.0;
    let icon_name = if problem.note { "info" } else { "warning" };
    let titled = problem.item.is_some();
    let reason_color = if titled { dim(p) } else { c(p.list_item_text) };
    let mut text = div().flex_1().min_w_0().flex().flex_col().gap(px(1.0));
    if let Some(item) = &problem.item {
        let item: SharedString = item.clone().into();
        let key = style::key(&item);
        text = text.child(
            canvas(move |g, b| {
                let p = theme::palette();
                let t = style::label_fit(g, b.left(), b.center().y, style::Align::Left, item.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), c(p.list_item_text), b.size.width);
                agent::record(g, agent::Node::label(item.clone(), t));
            })
            .paint_key(key)
            .w_full()
            .h(px(LINE))
            .flex_shrink_0(),
        );
    }
    let reason = StringView::new(problem.reason.clone()).wrap(true).color(reason_color);
    // (Alone, its first line is the one beside the icon: in the middle
    // of the icon's row.)
    text = text.child(if titled { div().text_size(px(style::SMALL_SIZE)).child(reason) } else { div().pt(px((LINE - style::PLAIN_SIZE * style::LINE_HEIGHT) / 2.0)).child(reason) });
    let mark = canvas(move |g, b| {
        icons::paint(g, style::snap_centred(g, b.center(), 16.0, 16.0), icon_name);
    })
    .paint_key(style::key(icon_name))
    .w(px(16.0))
    .h(px(LINE))
    .flex_shrink_0();
    div().w_full().flex().flex_row().items_start().py(px(5.0)).pl(px(m::SPACING)).pr(px(m::SPACING)).gap(px(x - m::SPACING - 16.0)).child(mark).child(text)
}
