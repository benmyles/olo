//! Drawing the main window: the account setup, lock and connection
//! panels, and the three panes (sources, items, the selected item), each
//! of the panes a cached part of the window (`olo_ui::part`); and the
//! Review Sync and Sync Problems windows.

use std::{cell::Cell, rc::Rc};

use chrono::{DateTime, Local, Utc};
use olo_ui::{
    Menu, agent,
    gpui::{
        AnyElement, App, Bounds, Context, Corner, CursorStyle, Div, Entity, Font, FontStyle, FontWeight, HitboxBehavior, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
        MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled as _, Subscription,
        TextRun, Window, canvas, div, point, px, relative, size,
    },
    icons, menu, metrics as m, part,
    search::{self, FilterBar, Needle, Query},
    style::{self, c},
    theme,
    widgets::{
        Button, CheckBox, Column, ColumnListView, ScrollView, Sidebar, StringView, TextControl, button_row,
        sidebar::{self, Row},
    },
};
use onepassword::{ItemCategory, ItemField, ItemFieldType, ItemOverview, ItemState};

use crate::{Accounts, Conn, Passwords, Source, edit, lock::Finger, store::SyncState};

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

type Me = Entity<Passwords>;

fn dim(p: &olo_theme::Palette) -> Hsla {
    style::disabled_text(p)
}

/// A sunken well's two lines, in logical pixels: whole device pixels.
fn edge(window: &Window) -> f32 {
    let s = window.scale_factor();
    style::line_pixels(s) * 2.0 / s
}

/// A sunken well of `background` with `content` inside its lines.
fn well(window: &Window, background: olo_theme::Rgba, content: impl IntoElement) -> Div {
    let ground = canvas(
        |_, _, _| {},
        move |b, _, window, cx| {
            let p = theme::palette(cx);
            let b = style::snap(window, b);
            style::fill_rect(window, b, c(background));
            style::sunken(window, b, &p, false);
        },
    )
    .absolute()
    .inset_0();
    let e = edge(window);
    div().relative().size_full().child(ground).child(div().absolute().left(px(e)).top(px(e)).right(px(e)).bottom(px(e)).overflow_hidden().child(content))
}

/// A line across the window a device pixel tall (under the toolbar,
/// over the status line).
fn rule(window: &Window, cx: &App) -> AnyElement {
    let p = theme::palette(cx);
    let line = style::shade(&p, p.panel_background, 1.3);
    canvas(|_, _, _| {}, move |b, _, window, _| style::fill_rect(window, style::snap(window, b), line)).w_full().h(style::hairline(window)).flex_shrink_0().into_any_element()
}

/// The main window: the setup, the lock, the connection's panels, or the
/// three panes.
pub fn main(app: &mut Passwords, window: &mut Window, cx: &mut Context<Passwords>) -> AnyElement {
    if app.conn == Conn::Setup {
        return setup(app, window, cx);
    }
    if app.lock.locked {
        return locked(app, window, cx);
    }
    match app.conn.clone() {
        Conn::Connecting if app.items.is_empty() => panel("passwords", "Opening your passwords…", "The key is in your login keyring.".into(), div()),
        Conn::Failed(message) if app.items.is_empty() => {
            let me = cx.entity();
            panel("warning", "Can’t open your passwords", message, button_row().child(Button::new("Try Again").default(true).on_click(move |_, cx| me.update(cx, |a, cx| a.connect(cx)))))
        }
        Conn::Ready if app.items.is_empty() && app.sync == Some(SyncState::Running) => {
            let account = app.account_label();
            let message = format!("Copying the items in {account} from 1Password. If 1Password asks whether to allow Passwords, choose Allow (and unlock it if it is locked).");
            panel("passwords", "Bringing in your items…", message, div())
        }
        _ => browser(app, window, cx),
    }
}

/// A centred column: icon, heading, message, then `extra`.
fn panel(icon_name: &'static str, heading: &str, message: String, extra: impl IntoElement) -> AnyElement {
    let heading: SharedString = heading.to_owned().into();
    let face = canvas(
        |_, _, _| {},
        move |b, _, window, cx| {
            let p = theme::palette(cx);
            icons::paint(window, style::snap_centred(window, point(b.center().x, b.top() + px(32.0)), 64.0, 64.0), icon_name);
            let t = style::label_fit(
                window,
                cx,
                b.center().x,
                b.top() + px(64.0 + 2.0 * m::SPACING + 11.0),
                style::Align::Center,
                heading.clone(),
                16.0,
                style::ui_font(FontWeight::BOLD),
                c(p.panel_text),
                b.size.width,
            );
            agent::record(window, cx, agent::Node { role: "Heading", name: Some(heading.clone()), bounds: t, enabled: true, ..Default::default() });
        },
    )
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
fn note(text: impl Into<SharedString>) -> Div {
    div().w_full().text_size(px(style::SMALL_SIZE)).child(StringView::new(text).wrap(true).dim(true))
}

fn setup(app: &mut Passwords, window: &mut Window, cx: &mut Context<Passwords>) -> AnyElement {
    if matches!(app.accounts, Accounts::Unknown) {
        app.find_accounts();
    }
    let me = cx.entity();
    let cancel_to = app.set_up();
    let listed = listed_accounts(app);
    let message = format!(
        "Passwords keeps your items on this computer, encrypted, and can keep them in step with your 1Password accounts both ways. To sync, turn on Settings ▸ Developer ▸ Integrate with other apps in 1Password, then {} the accounts.",
        if listed.is_empty() { "name" } else { "choose" }
    );
    let mut extra = div().w_full().flex().flex_col().gap(px(m::SPACING));
    for (i, a) in listed.iter().enumerate() {
        let on = app.chosen.iter().any(|c| same(a, c));
        let (to, account) = (me.clone(), a.clone());
        extra = extra.child(CheckBox::new(a.name.clone(), on).id(format!("account-{i}")).on_toggle(move |on, _, cx| {
            let account = account.clone();
            to.update(cx, |app, cx| {
                if on {
                    let id = linked_id(app, &account);
                    app.chosen.push(id);
                } else {
                    app.chosen.retain(|c| !same(&account, c));
                }
                cx.notify();
            })
        }));
        let detail = [a.email.as_str(), a.address.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" · ");
        if !detail.is_empty() {
            extra = extra.child(note(detail).pl(px(21.0)).mt(px(-m::SPACING * 0.75)));
        }
    }
    if !listed.is_empty() {
        let to = me.clone();
        extra = extra.child(CheckBox::new("Another account", app.other_account).id("account-other").on_toggle(move |on, _, cx| {
            to.update(cx, |a, cx| {
                a.other_account = on;
                cx.notify();
            })
        }));
    }
    let typed = !app.account_field.read(cx).text().trim().is_empty();
    if listed.is_empty() || app.other_account {
        extra = extra.child(TextControl::new(app.account_field.clone()).label("Account:").divider(70.0));
        if !app.account_field.read(cx).is_focused(window) && !typed && app.chosen.is_empty() {
            app.account_field.read(cx).focus(window);
        }
    }
    let any = !app.chosen.is_empty() || ((listed.is_empty() || app.other_account) && typed);
    let mut row = button_row();
    if cancel_to {
        let to = me.clone();
        row = row.child(Button::new("Cancel").on_click(move |_, cx| {
            to.update(cx, |a, cx| {
                a.conn = if a.vaults.is_empty() { Conn::Connecting } else { Conn::Ready };
                if a.vaults.is_empty() {
                    a.connect(cx);
                }
                cx.notify();
            })
        }));
    } else {
        let to = me.clone();
        row = row.child(Button::new("Keep Only on This Computer").on_click(move |_, cx| to.update(cx, |a, cx| a.keep_local(cx))));
    }
    let to = me.clone();
    row = row.child(Button::new("Sync").default(true).enabled(any).on_click(move |_, cx| to.update(cx, |a, cx| a.sync_chosen(cx))));
    extra = extra.child(row);
    if !any && cancel_to {
        extra = extra.child(note("With no account chosen, Passwords keeps its items only on this computer."));
    }
    panel("passwords", "Sync with 1Password?", message, extra)
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

fn locked(app: &mut Passwords, window: &mut Window, cx: &mut Context<Passwords>) -> AnyElement {
    let me = cx.entity();
    let p = theme::palette(cx);
    let mut message = match (&app.lock.finger, &app.lock.key_note) {
        (Finger::Listening(s), Some(_)) if s == "Touch ID or enter your password" => "Touch ID, touch your security key, or enter your password".into(),
        (Finger::Listening(s), _) => s.clone(),
        (_, Some(k)) => k.clone(),
        _ => format!("Enter the password for “{}” to unlock it.", app.lock.user()),
    };
    // Who is waiting: the app by name, or apps, if more than one.
    if let Some(first) = app.waiting.first() {
        let who = if app.waiting.iter().all(|w| w.from == first.from) {
            let name = olo_xdg::apps::our_app(first.from.trim_start_matches("olo-")).map_or("An app", |(name, _)| name);
            let passkey = app.waiting.iter().all(|w| matches!(w.request, olo_keychain::Request::PasskeySign { .. } | olo_keychain::Request::PasskeyCreate { .. }));
            format!("{name} is waiting for a {}.", if passkey { "passkey" } else { "login" })
        } else {
            "Apps are waiting for logins.".to_owned()
        };
        message = format!("{who} {message}");
    }
    let icon_name = if matches!(app.lock.finger, Finger::Listening(_)) { "fingerprint" } else { "passwords" };
    if !app.lock_field.read(cx).is_focused(window) && window.is_window_active() {
        app.lock_field.read(cx).focus(window);
    }
    let mut extra = div().w_full().flex().flex_col().gap(px(m::SPACING)).child(TextControl::new(app.lock_field.clone()).label("Password:").divider(78.0));
    if let Some(e) = &app.lock.error {
        extra = extra.child(StringView::new(e.clone()).wrap(true).color(c(p.failure)));
    }
    let label = if app.lock.checking { "Checking…" } else { "Unlock" };
    let ready = !app.lock.checking && !app.lock.password.is_empty();
    extra = extra.child(button_row().child(Button::new(label).default(true).enabled(ready).on_click(move |_, cx| me.update(cx, |a, cx| a.submit_password(cx)))));
    panel(icon_name, "Passwords is locked", message, extra)
}

/// What a click on the sync status shows.
enum More {
    Text(String),
    Problems(Vec<crate::sync::Problem>),
}

fn browser(app: &mut Passwords, window: &mut Window, cx: &mut Context<Passwords>) -> AnyElement {
    let me = cx.entity();
    let p = theme::palette(cx);
    if std::mem::take(&mut app.unlocked) {
        // The list takes the keyboard from the password field.
        app.items_list.read(cx).focus(window);
    }
    let Some(parts) = &app.parts else { return div().into_any_element() };
    let (sources, items, detail) = (parts.sources.clone(), parts.items.clone(), parts.detail.clone());

    // Toolbar: New Item, Generator, and the search field on the right.
    let (new_me, gen_me) = (me.clone(), me.clone());
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
            Button::new("New Item").on_click_at(move |b, window, cx| {
                let to = new_me.clone();
                menu::popup(window, cx, crate::category_menu(Menu::new("New Item"), "new:"), b, Corner::BottomLeft, move |cmd, window, cx| to.update(cx, |a, cx| a.command(cmd, window, cx)));
            })
        }))
        .child(Button::new("Generator").on_click(move |_, cx| gen_me.update(cx, |a, cx| a.open_generator(None, cx))))
        .child(div().flex_1())
        .child(div().w(px(280.0)).child(app.search_field.clone()));

    // The status line: what's going on, and the sync's state on the
    // right (a click shows a problem in full, or the changes waiting).
    let left: SharedString = match app.status.text() {
        Some(s) => s.clone(),
        None if app.loading => "Opening…".into(),
        None => {
            let n = app.visible().len();
            let said = if app.search.trim().is_empty() { format!("{n} item{}", if n == 1 { "" } else { "s" }) } else { format!("{n} found") };
            // (What a screen's Passwords shows is the person's to change.)
            if crate::on_screen() { format!("{said} shared with this screen") } else { said }.into()
        }
    };
    let review_click = matches!(app.sync, Some(SyncState::Review { .. } | SyncState::Declined { .. })) && !matches!(app.conn, Conn::Failed(_));
    let right = match (&app.conn, &app.sync) {
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
    let ink = c(p.panel_text);
    let status_left = canvas(
        |_, _, _| {},
        move |b, _, window, cx| {
            let r = style::label_fit(window, cx, b.left(), b.center().y, style::Align::Left, left.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), ink, b.size.width);
            agent::record(window, cx, agent::Node::label(left.clone(), r));
        },
    )
    .flex_1()
    .min_w_0()
    .h_full();
    // A bar of its own along the bottom (painted, as Tracker's count bar
    // is: its text sits on the bar, not on the window's edge).
    let bar = canvas(|_, _, _| {}, |b, _, window, cx| style::fill_rect(window, b, c(theme::palette(cx).panel_background))).absolute().inset_0();
    let mut status = div().relative().flex().flex_row().items_center().gap(px(m::SPACING)).h(px(STATUS_HEIGHT)).px(px(m::SPACING)).flex_shrink_0().child(bar).child(status_left);
    if let Some((text, bad, more)) = right {
        let color = if bad { c(p.failure) } else { dim(&p) };
        let clickable = more.is_some() || review_click;
        let text: SharedString = text.into();
        let width = f32::from(style::text_width(window, text.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL))).ceil() + 1.0;
        let label = text.clone();
        let el = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let r = style::label(window, cx, b.right(), b.center().y, style::Align::Right, label.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), color);
                agent::record(window, cx, agent::Node { clickable, ..agent::Node::label(label.clone(), r) });
            },
        )
        .size_full();
        let mut holder = div().id("passwords-sync-status").w(px(width)).h_full().flex_shrink_0().child(el);
        if clickable {
            let (to, more) = (me.clone(), Rc::new(more));
            holder = holder.cursor(CursorStyle::PointingHand).on_click(move |_, _, cx| {
                let more = more.clone();
                to.update(cx, |a, cx| match &*more {
                    Some(More::Text(t)) => a.show_problem(t.clone(), cx),
                    Some(More::Problems(list)) => a.show_problems(list.clone(), cx),
                    None if a.review.is_some() => a.open_review(cx),
                    None => a.sync_and_review(cx),
                })
            });
        }
        status = status.child(holder);
    }

    // The three panes, each a part of its own.
    let panes = div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_row()
        .gap(px(m::SPACING))
        .p(px(m::SPACING))
        .child(part::cached(&sources, StyleRefinement::default().w(px(SOURCES_WIDTH)).h_full().flex_shrink_0()))
        .child(part::cached(&items, StyleRefinement::default().w(px(LIST_WIDTH)).h_full().flex_shrink_0()))
        .child(part::cached(&detail, StyleRefinement::default().flex_1().min_w_0().h_full()));
    div().size_full().flex().flex_col().child(toolbar).child(rule(window, cx)).child(panes).child(rule(window, cx)).child(status).into_any_element()
}

/// The sources: All Items, the vaults (under their accounts' names with
/// several), the categories there are, and the Archive.
pub fn sources(app: &mut Passwords, window: &mut Window, cx: &mut Context<Passwords>) -> AnyElement {
    let me = cx.entity();
    let p = theme::palette(cx);
    let active = |i: &&ItemOverview| i.state == ItemState::Active;
    let mut rows: Vec<Row> = Vec::new();
    let mut of: Vec<Option<Source>> = Vec::new();
    let mut push = |row: Row, source: Option<Source>| {
        rows.push(row);
        of.push(source);
    };
    push(Row::new("passwords", "All Items").value(app.items.iter().filter(active).count().to_string()), Some(Source::All));
    // With several accounts, each account's vaults under its name (as
    // Tracker lists volumes), then any kept only here.
    let mut groups: Vec<(String, Vec<&onepassword::VaultOverview>)> = Vec::new();
    if app.settings.accounts.len() > 1 {
        for l in &app.settings.accounts {
            groups.push((l.label.clone(), app.vaults.iter().filter(|v| app.vault_accounts.get(&v.id) == Some(&l.id)).collect()));
        }
        let here: Vec<_> = app.vaults.iter().filter(|v| !app.vault_accounts.contains_key(&v.id)).collect();
        if !here.is_empty() {
            groups.push(("On This Computer".into(), here));
        }
    } else if !app.vaults.is_empty() {
        groups.push(("Vaults".into(), app.vaults.iter().collect()));
    }
    for (heading, vaults) in groups.into_iter().filter(|(_, v)| !v.is_empty()) {
        push(Row::heading(heading), None);
        for v in vaults {
            let n = app.items.iter().filter(active).filter(|i| i.vault_id == v.id).count();
            push(Row::new("vault", v.title.clone()).value(n.to_string()), Some(Source::Vault(v.id.clone())));
        }
    }
    let mut categories: Vec<ItemCategory> = app.items.iter().filter(active).map(|i| i.category).collect();
    categories.sort_by_key(|c| c.label());
    categories.dedup();
    if !categories.is_empty() {
        push(Row::heading("Categories"), None);
        for cat in categories {
            let n = app.items.iter().filter(active).filter(|i| i.category == cat).count();
            push(Row::new(icon(cat), plural(cat)).value(n.to_string()), Some(Source::Category(cat)));
        }
    }
    let archived = app.items.iter().filter(|i| i.state == ItemState::Archived).count();
    if archived > 0 {
        push(Row::heading(""), None);
        push(Row::new("archive", "Archive").value(archived.to_string()), Some(Source::Archive));
    }
    let chosen = of.iter().position(|s| s.as_ref() == Some(&app.source)).unwrap_or(0);
    let list = Sidebar::new("passwords-sources", rows, chosen).bare().on_choose(move |i, _, cx| {
        if let Some(Some(source)) = of.get(i) {
            let source = source.clone();
            me.update(cx, |a, cx| a.set_source(source, cx));
        }
    });
    well(window, p.list_background, list).into_any_element()
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

/// The filters over the list and the list: each item's icon, title and
/// what tells it apart (the search's finds marked).
pub fn items(app: &mut Passwords, window: &mut Window, cx: &mut Context<Passwords>) -> AnyElement {
    let me = cx.entity();
    let p = theme::palette(cx);
    let visible = app.visible();
    // The filters over the list (tags, what items have); the rest are
    // typed (`type:`, `vault:`, `site:`, `user:`), suggested as they are.
    let facets = app.facets_rc();
    let line = edge(window);
    // Their text over the list's icons, level with the sources' first
    // row beside them (inside its well's lines).
    let indent = line + m::SPACING - m::TEXT_INSET;
    let top = line + m::TIGHT + (sidebar::ROW - m::CONTROL_HEIGHT) / 2.0;
    // (What the menus choose goes into the search field, which the app
    // follows: `FilterBar::drives`.)
    let filters = FilterBar::new("passwords", app.search.clone(), facets.clone()).no_clear().max_width(LIST_WIDTH - indent).drives(&app.search_field);

    // The list, its selection the app's.
    let selected = visible.iter().position(|i| Some(&i.id) == app.selected.as_ref());
    let reveal = std::mem::take(&mut app.reveal_selection);
    app.items_list.update(cx, |l, _| {
        l.state.selected = selected.into_iter().collect();
        if l.state.cursor != selected {
            l.state.cursor = selected;
            l.state.anchor = selected;
        }
        if reveal && let Some(r) = selected {
            l.reveal(r);
        }
    });
    let query = Rc::new(Query::parse(&app.search, &search::keys(&facets)));
    // Under each title, where the search found it when that's inside
    // it (a section, a field), else its username or site.
    let found = app.found();
    let subtitles: Rc<Vec<String>> = Rc::new(visible.iter().zip(found.iter()).map(|(i, f)| f.clone().unwrap_or_else(|| app.subtitle(i))).collect());
    let mark = search::mark_color(&p);
    let empty = if app.loading {
        "Loading…"
    } else if !app.search.is_empty() {
        "Nothing matches."
    } else {
        "No items."
    };
    let (cells, picked, menu_rows) = (visible.clone(), visible.clone(), visible.clone());
    let (select_me, open_me, menu_me) = (me.clone(), me.clone(), me);
    let list_model = app.items_list.clone();
    let list = ColumnListView::new("passwords-items", &app.items_list, vec![Column::new("", 100.0)], visible.len(), move |cell, window, cx| {
        let Some(item) = cells.get(cell.row) else { return };
        let b = cell.bounds;
        icons::paint(window, style::snap_centred(window, point(b.left() + px(m::SPACING + 16.0), b.center().y), 32.0, 32.0), icon(item.category));
        let x = b.left() + px(m::SPACING * 2.0 + 32.0);
        let width = (b.right() - x - px(m::SPACING)).max(px(0.0));
        let p = theme::palette(cx);
        let sub = subtitles.get(cell.row).cloned().unwrap_or_default();
        let font = style::ui_font(FontWeight::NORMAL);
        for (text, size, color, dy) in [(item.title.clone(), style::PLAIN_SIZE, cell.text_color, -8.0), (sub, style::SMALL_SIZE, if cell.selected { cell.text_color } else { dim(&p) }, 8.0)] {
            // What the search found, marked (not on the selection's own
            // colour, which it would fight).
            let runs = if cell.selected || query.is_empty() {
                vec![TextRun { len: text.len(), font: font.clone(), color, background_color: None, underline: None, strikethrough: None, size: None }]
            } else {
                search::highlight(&text, &query, &font, color, mark)
            };
            style::label_runs_fit(window, cx, x, b.center().y + px(dy), style::Align::Left, text.clone().into(), size, &runs, width);
            cell.say(text);
        }
    })
    .no_header()
    .row_height(ITEM_ROW)
    .empty_text(empty)
    .on_select(move |_, cx| {
        let row = list_model.read(cx).state.single();
        if let Some(item) = row.and_then(|r| picked.get(r)) {
            let id = item.id.clone();
            select_me.update(cx, |a, cx| a.select(Some(id), cx));
        }
    })
    .on_open(move |_, _, cx| open_me.update(cx, |a, cx| a.open_website(cx)))
    .on_context(move |row, at, window, cx| {
        let Some(item) = menu_rows.get(row) else { return };
        let id = item.id.clone();
        let shown = menu_me.update(cx, |a, cx| {
            a.select(Some(id.clone()), cx);
            a.item_menu(&id)
        });
        if let Some(m) = shown {
            let to = menu_me.clone();
            menu::context_menu(window, cx, m, at, move |cmd, window, cx| to.update(cx, |a, cx| a.command(cmd, window, cx)));
        }
    });
    // Rows of their own height, scrolled: level with the sources and the
    // item's fields beside them only by chance.
    let grid = canvas(|_, _, _| {}, |b, _, window, _| olo_ui::review::own_grid(window, b)).absolute().inset_0();
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(div().pt(px(top)).pl(px(indent)).h(px(top + m::CONTROL_HEIGHT)).flex_shrink_0().child(filters))
        .child(div().relative().flex_1().min_h_0().mt(px(m::SPACING)).child(list).child(grid))
        .into_any_element()
}

/// The selected item (or the editor): its header, fields, websites,
/// notes, tags and dates.
pub fn detail(app: &mut Passwords, window: &mut Window, cx: &mut Context<Passwords>) -> AnyElement {
    let me = cx.entity();
    let p = theme::palette(cx);
    if let (Some(editor), Some(form)) = (&app.editor, &app.form) {
        if std::mem::take(&mut app.focus_title) {
            form.title.read(cx).focus(window);
        }
        if let Some(i) = app.focus_field.take() {
            form.focus_field(editor, i, window, cx);
        }
        let vaults: Vec<_> = app.vaults.iter().map(|v| onepassword::VaultOverview { title: app.vault_name(v), ..v.clone() }).collect();
        let width = f32::from(window.viewport_size().width) - SOURCES_WIDTH - LIST_WIDTH - 4.0 * m::SPACING - 2.0 * edge(window) - 2.0 * m::WINDOW_INSET;
        let content = edit::view(&me, editor, form, &vaults, width, cx);
        let view = ScrollView::new("passwords-editor", &app.editor_scroll).border(false).auto_hide(true).child(div().p(px(m::WINDOW_INSET)).child(content));
        return well(window, p.document_background, view).into_any_element();
    }
    let Some(id) = app.selected.clone() else {
        let text = if app.loading { "" } else { "No item selected" };
        let empty = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let r = style::label(window, cx, b.center().x, b.center().y, style::Align::Center, text, style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim(&p));
                agent::record(window, cx, agent::Node::label(text, r));
            },
        )
        .size_full();
        return well(window, p.document_background, empty).into_any_element();
    };
    let Some(overview) = app.overview(&id).cloned() else { return well(window, p.document_background, div()).into_any_element() };
    let vault = app.vault_title(&overview.vault_id);
    let Some(item) = app.details.get(&id).cloned() else {
        let (text, color) = match app.missing.get(&id) {
            Some(e) => (format!("Couldn’t open this item: {e}"), c(p.failure)),
            None => ("Loading…".to_string(), dim(&p)),
        };
        let content = div()
            .flex()
            .flex_col()
            .child(header(&overview.title, overview.category, &vault, None))
            .child(div().px(px(m::WINDOW_INSET)).pt(px(m::SPACING)).child(StringView::new(text).wrap(true).color(color)));
        return well(window, p.document_background, content).into_any_element();
    };
    let archived = overview.state == ItemState::Archived;
    let edit_me = me.clone();
    let edit_button = (!archived && !crate::on_screen()).then(|| Button::new("Edit").min_width(64.0).on_click(move |_, cx| edit_me.update(cx, |a, cx| a.edit_selected(cx))));
    let mut col = div().w_full().flex().flex_col().child(header(&item.title, item.category, &vault, edit_button));
    if archived {
        col = col.child(italic_note("This item is in the Archive."));
    }
    // What the search looks for, marked; and with `section:` or
    // `field:`, only the sections and fields it asks for.
    let facets = app.facets_rc();
    let query = Rc::new(Query::parse(&app.search, &search::keys(&facets)));
    let ask = Asked::of(&query);
    if ask.narrows() {
        col = col.child(italic_note("Only the sections and fields the search asks for."));
    }
    let mark = (!query.is_empty()).then(|| query.clone());
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
            col = col.child(section_heading(title, mark.clone()));
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
            col = col.child(field_row(app, &me, &id, f, f.id.clone(), app.revealed(&f.id), mark.clone(), cx));
        }
    }
    if !item.websites.is_empty() && !ask.narrows() {
        col = col.child(section_heading("Websites".into(), None));
        for w in &item.websites {
            let f = ItemField::new("", &w.label, ItemFieldType::Url, &w.url);
            col = col.child(field_row(app, &me, &id, &f, format!("url:{}", w.url), true, mark.clone(), cx));
        }
    }
    if !item.notes.is_empty() && !ask.narrows() {
        let ink = c(p.document_text);
        col =
            col.child(section_heading("Notes".into(), None)).child(div().px(px(m::WINDOW_INSET)).pt(px(m::TIGHT)).pb(px(m::SPACING)).child(StringView::new(item.notes.clone()).wrap(true).color(ink)));
    }
    if !item.tags.is_empty() && !ask.narrows() {
        let tags: SharedString = item.tags.join(", ").into();
        col = col.child(section_heading("Tags".into(), None)).child(
            canvas(
                |_, _, _| {},
                move |b, _, window, cx| {
                    let p = theme::palette(cx);
                    let r = style::label_fit(
                        window,
                        cx,
                        b.left() + px(m::WINDOW_INSET),
                        b.center().y,
                        style::Align::Left,
                        tags.clone(),
                        style::PLAIN_SIZE,
                        style::ui_font(FontWeight::NORMAL),
                        c(p.document_text),
                        b.size.width - px(2.0 * m::WINDOW_INSET),
                    );
                    agent::record(window, cx, agent::Node::label(tags.clone(), r));
                },
            )
            .w_full()
            .h(px(22.0))
            .flex_shrink_0(),
        );
    }
    // One line when it fits, else Modified and Created a line each.
    let (modified, created) = (format!("Modified {}", date(item.updated_at)), format!("Created {}", date(item.created_at)));
    let both = format!("{modified}   ·   {created}");
    let font = style::ui_font(FontWeight::NORMAL);
    let room = width_of_detail(window) - 2.0 * m::WINDOW_INSET;
    let lines: Vec<SharedString> = if f32::from(style::text_width(window, both.clone(), style::SMALL_SIZE, font.clone())) <= room { vec![both.into()] } else { vec![modified.into(), created.into()] };
    let step = style::SMALL_SIZE + m::TIGHT * 2.0;
    let rule_color = style::shade(&p, p.document_background, 1.15);
    let n = lines.len();
    let dates = canvas(
        |_, _, _| {},
        move |b, _, window, cx| {
            let p = theme::palette(cx);
            let (l, r) = (b.left() + px(m::WINDOW_INSET), b.right() - px(m::WINDOW_INSET));
            style::hline(window, l, r, b.top(), rule_color);
            for (i, text) in lines.iter().enumerate() {
                let t = style::label(window, cx, l, b.top() + px(17.0 + step * i as f32), style::Align::Left, text.clone(), style::SMALL_SIZE, font.clone(), dim(&p));
                agent::record(window, cx, agent::Node::label(text.clone(), t));
            }
        },
    )
    .w_full()
    .h(px(34.0 + step * (n - 1) as f32))
    .flex_shrink_0();
    col = col.child(div().h(px(m::SPACING)).flex_shrink_0()).child(dates);
    let view = ScrollView::new("passwords-detail", &app.detail_scroll).border(false).auto_hide(true).child(col);
    well(window, p.document_background, view).into_any_element()
}

/// A line of dim italics across the item pane, under its header.
fn italic_note(text: &'static str) -> AnyElement {
    let text: SharedString = text.into();
    canvas(
        |_, _, _| {},
        move |b, _, window, cx| {
            let p = theme::palette(cx);
            let mut font = style::ui_font(FontWeight::NORMAL);
            font.style = FontStyle::Italic;
            let room = b.size.width - px(2.0 * m::WINDOW_INSET);
            let r = style::label_fit(window, cx, b.left() + px(m::WINDOW_INSET), b.center().y, style::Align::Left, text.clone(), style::PLAIN_SIZE, font, dim(&p), room);
            agent::record(window, cx, agent::Node::label(text.clone(), r));
        },
    )
    .w_full()
    .h(px(28.0))
    .flex_shrink_0()
    .into_any_element()
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

/// How wide the item pane's content is.
fn width_of_detail(window: &Window) -> f32 {
    f32::from(window.viewport_size().width) - SOURCES_WIDTH - LIST_WIDTH - 4.0 * m::SPACING - 2.0 * edge(window)
}

fn date(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%-d %b %Y, %H:%M").to_string()
}

/// An item's header: its icon, title, kind and vault, and its Edit
/// button, whose label the title shares a baseline with.
fn header(title: &str, category: ItemCategory, vault: &str, edit: Option<Button>) -> AnyElement {
    let title: SharedString = title.to_owned().into();
    let sub: SharedString = if vault.is_empty() { category.label().to_string() } else { format!("{}  ·  {vault}", category.label()) }.into();
    let has_button = edit.is_some();
    let inset = m::WINDOW_INSET;
    let x = inset * 2.0 + 48.0;
    let (face_title, face_sub) = (title.clone(), sub);
    let face = canvas(
        |_, _, _| {},
        move |b, _, window, cx| {
            let p = theme::palette(cx);
            icons::paint(window, Bounds { origin: b.origin + point(px(inset), px(inset)), size: size(px(48.0), px(48.0)) }, icon(category));
            if !has_button {
                // The title's capitals centred on TITLE_MIDDLE.
                let room = (b.right() - b.left() - px(x + inset)).max(px(0.0));
                let font = style::ui_font(FontWeight::BOLD);
                let baseline = style::baseline_at(window, b.top() + px(inset + TITLE_MIDDLE), TITLE_SIZE);
                let t = style::label_on_baseline_fit(window, cx, b.left() + px(x), baseline, style::Align::Left, face_title.clone(), TITLE_SIZE, font, c(p.document_text), room);
                agent::record(window, cx, agent::Node { role: "Heading", name: Some(face_title.clone()), bounds: t, enabled: true, ..Default::default() });
            }
            let s = style::label(window, cx, b.left() + px(x), b.top() + px(inset + 38.0), style::Align::Left, face_sub.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim(&p));
            agent::record(window, cx, agent::Node::label(face_sub.clone(), s));
            style::hline_above(window, b.left() + px(inset), b.right() - px(inset), b.bottom(), style::shade(&p, p.document_background, 1.15));
        },
    )
    .absolute()
    .inset_0();
    let mut out = div().relative().w_full().h(px(48.0 + 2.0 * inset)).flex_shrink_0().child(face);
    if let Some(button) = edit {
        // The button lower than the title's middle, so its label shares
        // the title's baseline; the title drawn in the button's row, its
        // middle where the button's is.
        let lift = (TITLE_SIZE - style::PLAIN_SIZE) * style::CAP_HEIGHT / 2.0;
        let top = inset + TITLE_MIDDLE + lift - m::CONTROL_HEIGHT / 2.0;
        let named = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let font = style::ui_font(FontWeight::BOLD);
                let baseline = style::baseline_at(window, b.center().y, style::PLAIN_SIZE);
                let t =
                    style::label_on_baseline_fit(window, cx, b.left(), baseline, style::Align::Left, title.clone(), TITLE_SIZE, font, c(p.document_text), (b.size.width - px(m::SPACING)).max(px(0.0)));
                agent::record(window, cx, agent::Node { role: "Heading", name: Some(title.clone()), bounds: t, enabled: true, ..Default::default() });
            },
        )
        .flex_1()
        .min_w_0()
        .h(px(m::CONTROL_HEIGHT));
        out = out.child(div().absolute().left(px(x)).right(px(inset)).top(px(top)).flex().flex_row().items_center().child(named).child(button));
    }
    out.into_any_element()
}

/// A section's title over its fields, what `mark` looks for marked.
fn section_heading(title: String, mark: Option<Rc<Query>>) -> AnyElement {
    let title: SharedString = title.into();
    canvas(
        |_, _, _| {},
        move |b, _, window, cx| {
            let p = theme::palette(cx);
            let runs = marked(&title, mark.as_deref(), style::ui_font(FontWeight::BOLD), c(p.document_text), &p);
            let r = style::label_runs(window, cx, b.left() + px(m::WINDOW_INSET), b.bottom() - px(9.0), style::Align::Left, title.clone(), style::PLAIN_SIZE, &runs);
            agent::record(window, cx, agent::Node { role: "Heading", name: Some(title.clone()), bounds: r, enabled: true, ..Default::default() });
        },
    )
    .w_full()
    .h(px(30.0))
    .flex_shrink_0()
    .into_any_element()
}

/// `text` in `font` and `color`, what `mark` looks for marked.
fn marked(text: &str, mark: Option<&Query>, font: Font, color: Hsla, p: &olo_theme::Palette) -> Vec<TextRun> {
    match mark {
        Some(q) => search::highlight(text, q, &font, color, search::mark_color(p)),
        None => vec![TextRun { len: text.len(), font, color, background_color: None, underline: None, strikethrough: None, size: None }],
    }
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

/// One field: its label, its value (a link's to open, a secret's hidden
/// until shown), and, while the pointer is over it, Copy (and Show). A
/// click copies it (a link's value opens it); dragged, it goes out as its
/// value (onto another app's field). `key` is the field's id (`url:…` for
/// a website).
fn field_row(app: &Passwords, me: &Me, item: &str, f: &ItemField, key: String, revealed: bool, mark: Option<Rc<Query>>, cx: &App) -> AnyElement {
    let p = theme::palette(cx);
    let hovered = app.hover_field.as_deref() == Some(key.as_str());
    let label: SharedString = if f.title.is_empty() { edit::kind_label(f.field_type).to_lowercase() } else { f.title.clone() }.into();
    let (value, mono) = shown(f, revealed);
    let value: SharedString = value.replace('\n', " ⏎ ").into();
    let link = f.field_type == ItemFieldType::Url || (f.field_type == ItemFieldType::Text && f.value.starts_with("http"));
    let url = link.then(|| if f.value.contains("://") { f.value.clone() } else { format!("https://{}", f.value) });
    let color = if link { c(p.keyboard_navigation) } else { c(p.document_text) };
    let secret = matches!(f.field_type, ItemFieldType::Concealed | ItemFieldType::SshKey | ItemFieldType::CreditCardNumber);
    let buttons_w = if hovered { (if secret { 2.0 } else { 1.0 }) * FIELD_BUTTON + if secret { m::TIGHT } else { 0.0 } } else { 0.0 };
    let totp = (f.field_type == ItemFieldType::Totp).then(|| onepassword::totp::Totp::parse(&f.value).ok()).flatten();
    // Where its value and the row were drawn, for the clicks.
    let value_at: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::default();
    let row_at: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::default();
    let (paint_value_at, paint_row_at, hover_key, hover_me) = (value_at.clone(), row_at.clone(), key.clone(), me.clone());
    let face = canvas(
        |b, window, _| window.insert_hitbox(b, HitboxBehavior::Normal),
        move |b, hitbox, window, cx| {
            let p = theme::palette(cx);
            paint_row_at.set(Some(b));
            if hovered {
                let lit = style::snap(window, Bounds::from_corners(b.origin + point(px(m::TIGHT), px(1.0)), b.bottom_right() - point(px(m::TIGHT), px(1.0))));
                window.paint_quad(olo_ui::gpui::fill(lit, style::shade(&p, p.document_background, 1.04)).corner_radii(px(2.0)));
            }
            let x = b.left() + px(m::WINDOW_INSET);
            // What the search looks for, marked (not in a secret, which
            // it doesn't look in).
            let runs = marked(&label, mark.as_deref(), style::ui_font(FontWeight::NORMAL), dim(&p), &p);
            let l = style::label_runs(window, cx, x, b.top() + px(13.0), style::Align::Left, label.clone(), style::SMALL_SIZE, &runs);
            let font = if mono { style::mono_font(FontWeight::NORMAL) } else { style::ui_font(FontWeight::NORMAL) };
            let room = (b.right() - x - px(m::WINDOW_INSET + buttons_w)).max(px(0.0));
            let runs = marked(&value, mark.as_deref().filter(|_| !secret), font, color, &p);
            let v = style::label_runs_fit(window, cx, x, b.top() + px(31.0), style::Align::Left, value.clone(), style::PLAIN_SIZE, &runs, room);
            paint_value_at.set(Some(v));
            agent::record(window, cx, agent::Node { role: "Label", name: Some(label.clone()), value: Some(value.clone()), bounds: l.union(&v), enabled: true, clickable: true, ..Default::default() });
            // A little pie of the code's time left, a ring round it.
            if let Some(t) = &totp {
                let left = t.now().1 as f32 / t.period.max(1) as f32;
                let centre = point(v.right() + px(m::SPACING + 7.0), v.center().y);
                let ink = if left < 0.2 { c(p.failure) } else { dim(&p) };
                window.paint_sector(centre, px(5.5), px(6.5), 0.0, std::f32::consts::TAU, dim(&p));
                window.paint_sector(centre, px(0.0), px(5.0), 0.0, std::f32::consts::TAU * left, ink);
            }
            // The pointer onto it, or off it: its buttons come and go.
            let (key, me) = (hover_key.clone(), hover_me.clone());
            window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
                if !phase.bubble() {
                    return;
                }
                let over = hitbox.is_hovered(window);
                me.update(cx, |a, cx| {
                    let here = a.hover_field.as_deref() == Some(key.as_str());
                    if over && !here {
                        a.hover_field = Some(key.clone());
                        cx.notify();
                    } else if !over && here {
                        a.hover_field = None;
                        cx.notify();
                    }
                });
            });
        },
    )
    .absolute()
    .inset_0();
    // Pressed: a click copies (or opens a link's value); moved, a drag
    // out. (Where it was pressed is the app's: the row is drawn again as
    // the pointer comes onto it.)
    let (down_me, click_me, drag_me) = (me.clone(), me.clone(), me.clone());
    let (down_key, click_key, drag_key, drag_item, click_item) = (key.clone(), key.clone(), key.clone(), item.to_owned(), item.to_owned());
    let mut row = div()
        .id(SharedString::from(format!("field-{key}")))
        .relative()
        .w_full()
        .h(px(FIELD_ROW))
        .flex_shrink_0()
        .child(face)
        .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, _, cx| {
            let at = (down_key.clone(), e.position);
            down_me.update(cx, |a, _| a.field_press = Some(at));
        })
        .on_mouse_move(move |e: &MouseMoveEvent, window, cx| {
            let from = drag_me.read(cx).field_press.as_ref().filter(|(k, _)| *k == drag_key).map(|(_, at)| *at);
            if let Some(at) = from
                && e.pressed_button == Some(MouseButton::Left)
                && ((e.position.x - at.x).abs() > px(3.0) || (e.position.y - at.y).abs() > px(3.0))
            {
                let (key, item) = (drag_key.clone(), drag_item.clone());
                drag_me.update(cx, |a, cx| {
                    a.field_press = None;
                    a.drag_field(&item, &key, window, cx);
                });
            }
        })
        .on_mouse_up(MouseButton::Left, move |e: &MouseUpEvent, _, cx| {
            let pressed = click_me.update(cx, |a, _| a.field_press.take()).is_some_and(|(k, _)| k == click_key);
            if !pressed {
                return;
            }
            // (Not on its buttons, which do their own.)
            let on_buttons = buttons_w > 0.0 && row_at.get().is_some_and(|r| e.position.x >= r.right() - px(m::WINDOW_INSET + buttons_w + m::TIGHT));
            if on_buttons {
                return;
            }
            let on_value = value_at.get().is_some_and(|v| v.contains(&e.position));
            match &url {
                Some(url) if on_value => crate::open_url(url),
                _ => {
                    let (key, item) = (click_key.clone(), click_item.clone());
                    click_me.update(cx, |a, cx| a.copy_field(&item, &key, cx));
                }
            }
        });
    if hovered {
        let mut buttons = div().absolute().right(px(m::WINDOW_INSET)).top(px((FIELD_ROW - m::CONTROL_HEIGHT) / 2.0)).flex().flex_row().gap(px(m::TIGHT));
        if secret {
            let (to, k) = (me.clone(), key.clone());
            buttons = buttons.child(Button::new(if revealed { "Hide" } else { "Show" }).id(format!("show-{key}")).min_width(FIELD_BUTTON).on_click(move |_, cx| {
                let k = k.clone();
                to.update(cx, |a, cx| a.toggle_revealed(k, cx))
            }));
        }
        let (to, k, it) = (me.clone(), key.clone(), item.to_owned());
        buttons = buttons.child(Button::new("Copy").id(format!("copy-{key}")).min_width(FIELD_BUTTON).on_click(move |_, cx| {
            let (k, it) = (k.clone(), it.clone());
            to.update(cx, |a, cx| a.copy_field(&it, &k, cx))
        }));
        row = row.child(buttons);
    }
    // Right-click: the value copied, or its `op://` reference (for the
    // `op` command, `op read`, `op inject`).
    if !key.starts_with("url:") {
        let (menu_me, menu_key, menu_item) = (me.clone(), key.clone(), item.to_owned());
        row = row.on_mouse_down(MouseButton::Right, move |e: &MouseDownEvent, window, cx| {
            let m = olo_ui::menu::Menu::new("Field").item("Copy", "copy").item("Copy Reference", "reference");
            let (to, k, it) = (menu_me.clone(), menu_key.clone(), menu_item.clone());
            menu::context_menu(window, cx, m, e.position, move |cmd, _, cx| {
                to.update(cx, |a, cx| match cmd {
                    "reference" => a.copy_reference(&it, Some(&k), cx),
                    _ => a.copy_field(&it, &k, cx),
                })
            });
        });
    }
    if link {
        row = row.cursor(CursorStyle::PointingHand);
    }
    row.into_any_element()
}

const REVIEW_ROW: f32 = 42.0;

/// The Review Sync window: every change a sync would make, both ways,
/// and nothing happens until Apply.
pub struct ReviewView {
    app: Me,
    list: ScrollHandle,
    _observe: Subscription,
}

impl ReviewView {
    pub fn new(app: Me, cx: &mut Context<Self>) -> ReviewView {
        let observe = cx.observe(&app, |_, _, cx| cx.notify());
        ReviewView { app, list: ScrollHandle::new(), _observe: observe }
    }
}

/// A window's message, list and buttons, as an alert's are laid out.
fn sheet(message: String, list: impl IntoElement, buttons: impl IntoElement) -> Div {
    div().size_full().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).child(StringView::new(message).wrap(true)).child(div().flex_1().min_h_0().child(list)).child(buttons)
}

impl Render for ReviewView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let a = self.app.read(cx);
        let Some(review) = &a.review else { return div().into_any_element() };
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
            rows = rows.child(
                canvas(
                    |_, _, _| {},
                    move |b, _, window, cx| {
                        let p = theme::palette(cx);
                        icons::paint(window, style::snap_centred(window, point(b.left() + px(m::SPACING + 16.0), b.center().y), 32.0, 32.0), icon(category));
                        let x = b.left() + px(m::SPACING * 2.0 + 32.0);
                        let width = (b.right() - x - px(m::SPACING)).max(px(0.0));
                        let font = style::ui_font(FontWeight::NORMAL);
                        let t = style::label_fit(window, cx, x, b.center().y - px(8.0), style::Align::Left, title.clone(), style::PLAIN_SIZE, font.clone(), c(p.list_item_text), width);
                        let d = style::label_fit(window, cx, x, b.center().y + px(8.0), style::Align::Left, action.clone(), style::SMALL_SIZE, font, if warn { c(p.failure) } else { dim(&p) }, width);
                        agent::record(window, cx, agent::Node { role: "ListItem", name: Some(title.clone()), value: Some(action.clone()), bounds: t.union(&d), enabled: true, ..Default::default() });
                    },
                )
                .w_full()
                .h(px(REVIEW_ROW))
                .flex_shrink_0(),
            );
        }
        let p = theme::palette(cx);
        let list = ScrollView::new("passwords-review", &self.list).background(p.list_background).child(rows);
        let n = review.rows.len();
        let apply = if n == 1 { "Apply Change".to_string() } else { format!("Apply {n} Changes") };
        let mut buttons = button_row();
        // What couldn't be read: a line on the left, the list a click away.
        if !review.problems.is_empty() {
            let (to, list) = (self.app.clone(), review.problems.clone());
            buttons = buttons.child(problems_note(&review.problems).on_click(move |_, _, cx| {
                let list = list.clone();
                to.update(cx, |a, cx| a.show_problems(list, cx))
            }));
            buttons = buttons.child(div().flex_1());
        }
        let (cancel, ok, esc) = (self.app.clone(), self.app.clone(), self.app.clone());
        buttons = buttons
            .child(Button::new("Cancel").on_click(move |_, cx| cancel.update(cx, |a, cx| a.answer_review(false, cx))))
            .child(Button::new(apply).default(true).on_click(move |_, cx| ok.update(cx, |a, cx| a.answer_review(true, cx))));
        sheet(message, list, buttons)
            .on_key_down(move |e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    cx.stop_propagation();
                    esc.update(cx, |a, cx| a.answer_review(false, cx));
                }
            })
            .into_any_element()
    }
}

/// "⚠ 3 problems reading 1Password…", in the window's own text, as a
/// link to the Sync Problems window.
fn problems_note(list: &[crate::sync::Problem]) -> olo_ui::gpui::Stateful<Div> {
    let failed = list.iter().filter(|p| !p.note).count();
    let (icon_name, text): (&'static str, SharedString) = match failed {
        0 => ("info", "Some items are left out…".into()),
        1 => ("warning", "1 problem reading 1Password…".into()),
        n => ("warning", format!("{n} problems reading 1Password…").into()),
    };
    let face = canvas(
        |b, window, _| window.insert_hitbox(b, HitboxBehavior::Normal),
        move |b, hitbox, window, cx| {
            let p = theme::palette(cx);
            icons::paint(window, style::snap_centred(window, point(b.left() + px(8.0), b.center().y), 16.0, 16.0), icon_name);
            let x = b.left() + px(16.0 + m::SPACING / 2.0);
            let t = style::label(window, cx, x, b.center().y, style::Align::Left, text.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), c(p.panel_text));
            if hitbox.is_hovered(window) {
                style::hline_above(window, t.left(), t.right(), t.bottom() + px(1.0), c(p.panel_text));
            }
            agent::record(window, cx, agent::Node { clickable: true, ..agent::Node::label(text.clone(), t) });
            window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, _| {
                if phase.bubble() {
                    window.refresh();
                }
            });
        },
    )
    .size_full();
    div().id("passwords-problems-note").h(px(m::CONTROL_HEIGHT)).w(px(260.0)).cursor(CursorStyle::PointingHand).child(face)
}

/// The Sync Problems window: what a sync couldn't do, grouped by vault,
/// each with its reason in plain words.
pub struct ProblemsView {
    app: Me,
    list: ScrollHandle,
    _observe: Subscription,
}

impl ProblemsView {
    pub fn new(app: Me, cx: &mut Context<Self>) -> ProblemsView {
        let observe = cx.observe(&app, |_, _, cx| cx.notify());
        ProblemsView { app, list: ScrollHandle::new(), _observe: observe }
    }
}

impl Render for ProblemsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let a = self.app.read(cx);
        let Some(problems) = &a.problems else { return div().into_any_element() };
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
        let p = theme::palette(cx);
        let mut rows = div().w_full().flex().flex_col();
        let mut vault: Option<&str> = None;
        for problem in list {
            if vault != Some(problem.vault.as_str()) {
                vault = Some(problem.vault.as_str());
                let name: SharedString = if problem.vault.is_empty() { "Not in a vault".into() } else { problem.vault.clone().into() };
                rows = rows.child(
                    canvas(
                        |_, _, _| {},
                        move |b, _, window, cx| {
                            let p = theme::palette(cx);
                            let t = style::label_fit(
                                window,
                                cx,
                                b.left() + px(m::SPACING),
                                b.center().y + px(2.0),
                                style::Align::Left,
                                name.clone(),
                                style::PLAIN_SIZE,
                                style::ui_font(FontWeight::BOLD),
                                c(p.list_item_text),
                                b.size.width - px(2.0 * m::SPACING),
                            );
                            agent::record(window, cx, agent::Node { role: "Heading", name: Some(name.clone()), bounds: t, enabled: true, ..Default::default() });
                            style::hline_above(window, b.left(), b.right(), b.bottom(), dim(&p).opacity(0.4));
                        },
                    )
                    .w_full()
                    .h(px(26.0))
                    .flex_shrink_0(),
                );
            }
            rows = rows.child(problem_row(problem, &p));
        }
        let shown = ScrollView::new("passwords-problems", &self.list).background(p.list_background).child(rows);
        let (close, again, copy, esc) = (self.app.clone(), self.app.clone(), self.app.clone(), self.app.clone());
        let text = crate::sync::as_text(list);
        let mut buttons = button_row().child(Button::new("Copy").on_click(move |_, cx| {
            cx.write_to_clipboard(olo_ui::gpui::ClipboardItem::new_string(text.clone()));
            copy.update(cx, |_, cx| cx.notify());
        }));
        if failed > 0 && !reviewing {
            buttons = buttons.child(Button::new("Sync Again").on_click(move |_, cx| {
                again.update(cx, |a, cx| {
                    a.close_problems(cx);
                    a.sync_and_review(cx);
                })
            }));
        }
        buttons = buttons.child(Button::new("Close").default(true).on_click(move |_, cx| close.update(cx, |a, cx| a.close_problems(cx))));
        sheet(message.to_owned(), shown, buttons)
            .on_key_down(move |e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    cx.stop_propagation();
                    esc.update(cx, |a, cx| a.close_problems(cx));
                }
            })
            .into_any_element()
    }
}

/// One problem: its icon, the item (or the reason, for the whole
/// vault) and the reason under it, wrapped.
fn problem_row(problem: &crate::sync::Problem, p: &olo_theme::Palette) -> AnyElement {
    let x = m::SPACING * 2.0 + 16.0;
    let icon_name = if problem.note { "info" } else { "warning" };
    let titled = problem.item.is_some();
    let reason_color = if titled { dim(p) } else { c(p.list_item_text) };
    let mut text = div().flex_1().min_w_0().flex().flex_col().gap(px(1.0));
    if let Some(item) = &problem.item {
        let item: SharedString = item.clone().into();
        text = text.child(
            canvas(
                |_, _, _| {},
                move |b, _, window, cx| {
                    let p = theme::palette(cx);
                    let t = style::label_fit(
                        window,
                        cx,
                        b.left(),
                        b.center().y,
                        style::Align::Left,
                        item.clone(),
                        style::PLAIN_SIZE,
                        style::ui_font(FontWeight::NORMAL),
                        c(p.list_item_text),
                        b.size.width,
                    );
                    agent::record(window, cx, agent::Node::label(item.clone(), t));
                },
            )
            .w_full()
            .h(px(style::PLAIN_SIZE * style::LINE_HEIGHT))
            .flex_shrink_0(),
        );
    }
    let reason = StringView::new(problem.reason.clone()).wrap(true).color(reason_color);
    text = text.child(if titled { div().text_size(px(style::SMALL_SIZE)).child(reason) } else { div().child(reason) });
    let mark = canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            icons::paint(window, style::snap(window, Bounds { origin: b.origin, size: size(px(16.0), px(16.0)) }), icon_name);
        },
    )
    .w(px(16.0))
    .h(px(16.0))
    .flex_shrink_0();
    div()
        .w_full()
        .flex()
        .flex_row()
        .items_start()
        .py(px(6.0))
        .pl(px(m::SPACING))
        .pr(px(m::SPACING))
        .gap(px(x - m::SPACING - 16.0))
        .child(div().pt(px((style::PLAIN_SIZE * style::LINE_HEIGHT - 16.0) / 2.0)).child(mark))
        .child(text)
        .into_any_element()
}
