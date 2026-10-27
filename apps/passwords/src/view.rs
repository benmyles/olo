//! Drawing the main window: the account setup, lock and connection
//! panels, and the three panes (sources, items, the selected item).

use chrono::{DateTime, Local, Utc};
use onepassword::{ItemCategory, ItemField, ItemFieldType, ItemOverview, ItemState};
use olo_kit::{
    App, Cx, Menu, egui::{self, Align, Color32, FontId, Id, Pos2, Rect, Sense, Ui, UiBuilder, pos2, vec2},
    icons, metrics as m, style,
    widgets::{self, Button, ScrollView, TextControl},
};

use crate::{
    Accounts, Conn, EditAction, Passwords, Source, edit,
    store::SyncState,
    lock::Finger,
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
const SOURCE_ROW: f32 = 22.0;
const STATUS_HEIGHT: f32 = 22.0;
const FIELD_ROW: f32 = 44.0;

fn title_font() -> FontId {
    FontId::new(18.0, style::bold())
}

fn dim(p: &olo_theme::Palette) -> Color32 {
    style::disabled_text(p)
}

fn child(ui: &mut Ui, rect: Rect) -> Ui {
    ui.new_child(UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(Align::Min)))
}

/// A sunken list well.
fn well(ui: &Ui, rect: Rect, background: olo_theme::Rgba, focused: bool) -> Rect {
    let p = style::palette(ui.ctx());
    let rect = style::snap(ui.ctx(), rect);
    ui.painter().rect_filled(rect, 0.0, style::c(background));
    style::sunken(ui.painter(), rect, &p, focused);
    rect.shrink(1.0)
}

pub fn main(app: &mut Passwords, cx: &mut Cx, ui: &mut Ui) {
    let p = style::palette(ui.ctx());
    let full = ui.available_rect_before_wrap();
    ui.painter().rect_filled(full, 0.0, style::c(p.panel_background));
    if app.conn == Conn::Setup {
        return setup(app, cx, ui, full);
    }
    if app.lock.locked {
        return locked(app, ui, full);
    }
    match app.conn.clone() {
        Conn::Connecting if app.items.is_empty() => {
            panel(ui, full, "passwords", "Opening your passwords…", "The key is in your login keyring.", |_| {});
        }
        Conn::Failed(message) if app.items.is_empty() => {
            panel(ui, full, "warning", "Can’t open your passwords", &message, |ui| {
                buttons(ui, |ui| {
                    if ui.add(Button::new("Try Again").default(true)).clicked() {
                        app.connect(cx);
                    }
                });
            });
        }
        Conn::Ready if app.items.is_empty() && app.sync == Some(SyncState::Running) => {
            let account = app.account_label();
            panel(ui, full, "passwords", "Bringing in your items…", &format!(
                "Copying the items in {account} from 1Password. If 1Password asks whether to allow Passwords, choose Allow (and unlock it if it is locked)."
            ), |_| {});
        }
        _ => browser(app, cx, ui, full),
    }
}

/// A centred column: icon, heading, message, then `extra`.
fn panel(ui: &mut Ui, full: Rect, icon_name: &str, heading: &str, message: &str, extra: impl FnOnce(&mut Ui)) {
    let p = style::palette(ui.ctx());
    let width = 380.0f32.min(full.width() - 2.0 * m::WINDOW_INSET);
    let top = full.top() + (full.height() * 0.22).max(m::WINDOW_INSET);
    let x = full.center().x - width / 2.0;
    icons::paint(ui.painter(), Rect::from_center_size(pos2(full.center().x, top + 32.0), vec2(64.0, 64.0)), icon_name);
    let mut y = top + 64.0 + m::SPACING * 2.0;
    let g = ui.painter().layout(heading.into(), FontId::new(16.0, style::bold()), style::c(p.panel_text), width);
    ui.painter().galley(pos2(full.center().x - g.size().x / 2.0, y), g.clone(), style::c(p.panel_text));
    y += g.size().y + m::SPACING;
    let g = ui.painter().layout(message.into(), style::plain_font(), style::c(p.panel_text), width);
    let mut job = (*g.job).clone();
    job.halign = Align::Center;
    let g = ui.painter().layout_job(job);
    ui.painter().galley(pos2(full.center().x, y), g.clone(), style::c(p.panel_text));
    y += g.size().y + m::SPACING * 2.0;
    let mut child = child(ui, Rect::from_min_max(pos2(x, y), pos2(x + width, full.bottom() - m::WINDOW_INSET)));
    child.spacing_mut().item_spacing.y = m::SPACING;
    extra(&mut child);
}

fn setup(app: &mut Passwords, cx: &mut Cx, ui: &mut Ui, full: Rect) {
    if matches!(app.accounts, Accounts::Unknown) {
        app.find_accounts(cx);
    }
    let mut connect = false;
    let cancel_to = app.set_up();
    // The app's accounts, then any synced now that it doesn't list.
    let mut listed: Vec<onepassword::accounts::Account> = match &app.accounts {
        Accounts::Found(list) => list.clone(),
        _ => Vec::new(),
    };
    for l in &app.settings.accounts {
        if !listed.iter().any(|a| same(a, &l.id)) {
            listed.push(onepassword::accounts::Account { uuid: l.id.clone(), name: l.label.clone(), email: String::new(), address: String::new() });
        }
    }
    let mut keep_local = false;
    let message = format!(
        "Passwords keeps your items on this computer, encrypted, and can keep them in step with your 1Password accounts both ways. To sync, turn on Settings ▸ Developer ▸ Integrate with other apps in 1Password, then {} the accounts.",
        if listed.is_empty() { "name" } else { "choose" }
    );
    panel(ui, full, "passwords", "Sync with 1Password?", &message, |ui| {
        let p = style::palette(ui.ctx());
        for a in &listed {
            let mut on = app.chosen.iter().any(|c| same(a, c));
            let r = ui.add(widgets::CheckBox::new(&mut on, &a.name));
            if r.clicked() {
                if on {
                    app.chosen.push(linked_id(app, a));
                } else {
                    app.chosen.retain(|c| !same(a, c));
                }
            }
            let detail = [a.email.as_str(), a.address.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" · ");
            if !detail.is_empty() {
                let g = ui.painter().layout(detail, style::small_font(), style::disabled_text(&p), ui.available_width() - 21.0);
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y), Sense::hover());
                ui.painter().galley(pos2(rect.left() + 21.0, rect.top() - m::SPACING * 0.75), g, style::disabled_text(&p));
            }
        }
        if !listed.is_empty() {
            let mut on = app.other_account;
            if ui.add(widgets::CheckBox::new(&mut on, "Another account")).clicked() {
                app.other_account = on;
            }
        }
        if listed.is_empty() || app.other_account {
            let field = ui.add(TextControl::new(&mut app.account_field).label("Account:").divider(70.0).hint("as in 1Password's sidebar, or its UUID"));
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                connect = true;
            }
            if !field.has_focus() && app.account_field.is_empty() && app.chosen.is_empty() {
                field.request_focus();
            }
        } else if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            connect = true;
        }
        let any = !app.chosen.is_empty() || ((listed.is_empty() || app.other_account) && !app.account_field.trim().is_empty());
        buttons(ui, |ui| {
            let ok = ui.add_enabled(any, Button::new("Sync").default(true));
            if ok.clicked() {
                connect = true;
            }
            if !cancel_to && ui.add(Button::new("Keep Only on This Computer")).clicked() {
                keep_local = true;
            }
            if cancel_to && ui.add(Button::new("Cancel")).clicked() {
                app.conn = if app.vaults.is_empty() { Conn::Connecting } else { Conn::Ready };
                if app.vaults.is_empty() {
                    app.connect(cx);
                }
            }
        });
        if !any && cancel_to {
            let g = ui.painter().layout(
                "With no account chosen, Passwords keeps its items only on this computer.".into(),
                style::small_font(),
                style::disabled_text(&p),
                ui.available_width(),
            );
            let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
            ui.painter().galley(r.min, g, style::disabled_text(&p));
        }
    });
    if keep_local {
        app.keep_local(cx);
        return;
    }
    if connect {
        let mut chosen: Vec<crate::Linked> = listed
            .iter()
            .filter(|a| app.chosen.iter().any(|c| same(a, c)))
            .map(|a| crate::Linked { id: linked_id(app, a), label: a.name.clone() })
            .collect();
        let typed = app.account_field.trim().to_string();
        if (listed.is_empty() || app.other_account) && !typed.is_empty() && !chosen.iter().any(|c| c.id == typed) {
            chosen.push(crate::Linked { id: typed.clone(), label: typed });
        }
        if chosen.is_empty() && !cancel_to {
            return;
        }
        app.choose_accounts(cx, chosen);
    }
}

/// Whether `id` (a UUID, or a name typed by hand) is the account `a`.
fn same(a: &onepassword::accounts::Account, id: &str) -> bool {
    a.uuid == id || a.name == id
}

/// The id to sync `a` by: the one already in use for it (an account once
/// named by hand keeps its name, which its synced vaults are kept under),
/// else its UUID.
fn linked_id(app: &Passwords, a: &onepassword::accounts::Account) -> String {
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

fn locked(app: &mut Passwords, ui: &mut Ui, full: Rect) {
    let p = style::palette(ui.ctx());
    let mut message = match &app.lock.finger {
        Finger::Listening(s) => s.clone(),
        _ => format!("Enter the password for “{}” to unlock it.", app.lock.user()),
    };
    // Who is waiting: the app by name, or apps, if more than one.
    if let Some(first) = app.waiting.first() {
        let who = if app.waiting.iter().all(|w| w.from == first.from) {
            let name = olo_xdg::apps::our_app(first.from.trim_start_matches("olo-")).map_or("An app", |(name, _)| name);
            format!("{name} is waiting for a login.")
        } else {
            "Apps are waiting for logins.".to_owned()
        };
        message = format!("{who} {message}");
    }
    let icon_name = if matches!(app.lock.finger, Finger::Listening(_)) { "fingerprint" } else { "passwords" };
    panel(ui, full, icon_name, "Passwords is locked", &message, |ui| {
        let field = ui.add(TextControl::new(&mut app.lock.password).label("Password:").divider(78.0).password(true));
        // Check Enter before taking focus back: lost_focus() reads it live.
        if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            app.lock.submit();
        }
        if !field.has_focus() {
            field.request_focus();
            ui.ctx().request_repaint();
        }
        if let Some(e) = &app.lock.error {
            ui.label(egui::RichText::new(e).color(style::c(p.failure)));
        }
        buttons(ui, |ui| {
            let label = if app.lock.checking { "Checking…" } else { "Unlock" };
            if ui.add_enabled(!app.lock.checking && !app.lock.password.is_empty(), Button::new(label).default(true)).clicked() {
                app.lock.submit();
            }
        });
    });
}

fn browser(app: &mut Passwords, cx: &mut Cx, ui: &mut Ui, full: Rect) {
    let p = style::palette(ui.ctx());
    if std::mem::take(&mut app.unlocked) {
        ui.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
    }
    let toolbar = Rect::from_min_size(full.min, vec2(full.width(), m::TOOLBAR_HEIGHT));
    let status = Rect::from_min_max(pos2(full.left(), full.bottom() - STATUS_HEIGHT), full.max);
    let body = Rect::from_min_max(pos2(full.left(), toolbar.bottom()), pos2(full.right(), status.top()));

    // Toolbar: New Item, Generator, and the search field on the right.
    style::hline(ui.painter(), full.left(), full.right(), toolbar.bottom() - 1.0, style::shade(&p, p.panel_background, 1.3));
    let mut bar = ui.new_child(
        UiBuilder::new()
            .max_rect(toolbar.shrink2(vec2(m::SPACING, m::TIGHT)))
            .layout(egui::Layout::left_to_right(Align::Center)),
    );
    bar.spacing_mut().item_spacing.x = m::SPACING;
    let new_button = bar.add(Button::new("New Item"));
    let new_id = Id::unique("passwords-new");
    if new_button.clicked() {
        widgets::popup_menu_at(bar.ctx(), new_id, new_button.rect.left_bottom(), crate::category_menu(Menu::new("New Item"), "new:"));
    }
    if let Some(cmd) = widgets::popup_result(bar.ctx(), new_id)
        && let Some(w) = app.main {
            app.command(cx, w, &cmd);
        }
    if bar.add(Button::new("Generator")).clicked() {
        app.open_generator(cx, None);
    }
    bar.with_layout(egui::Layout::right_to_left(Align::Center), |bar| {
        let search = bar.add(TextControl::new(&mut app.search).width(240.0).hint("Search"));
        if std::mem::take(&mut app.focus_search) {
            search.request_focus();
        }
        if (search.has_focus() || search.lost_focus()) && bar.input(|i| i.key_pressed(egui::Key::Escape)) {
            app.search.clear();
        }
        if search.changed() {
            // Keep the selection on something listed.
            if !app.visible().iter().any(|i| Some(&i.id) == app.selected.as_ref()) {
                let first = app.visible().first().map(|i| i.id.clone());
                app.select(cx, first);
            }
        }
    });

    // Status line.
    style::hline(ui.painter(), full.left(), full.right(), status.top(), style::shade(&p, p.panel_background, 1.3));
    let text = match &app.status {
        Some((s, _)) => s.clone(),
        None if app.loading => "Opening…".into(),
        None => {
            let n = app.visible().len();
            format!("{n} item{}", if n == 1 { "" } else { "s" })
        }
    };
    style::label(ui.painter(), status.left() + m::SPACING, status.center().y, Align::Min, text, style::small_font(), style::c(p.panel_text));
    // Waiting changes: a click shows them (or looks again, if declined).
    let review_click = matches!(app.sync, Some(SyncState::Review { .. } | SyncState::Declined { .. })) && !matches!(app.conn, Conn::Failed(_));
    let right = match (&app.conn, &app.sync) {
        (Conn::Failed(m), _) => Some((m.split('.').next().unwrap_or(m).to_string(), true, None)),
        (_, Some(SyncState::Running)) => Some(("Syncing with 1Password…".to_string(), false, None)),
        (_, Some(SyncState::Review { changes, .. })) => {
            Some((format!("{} to review", if *changes == 1 { "1 sync change".to_string() } else { format!("{changes} sync changes") }), false, None))
        }
        (_, Some(SyncState::Declined { pending })) => {
            let to_send = if *pending > 0 { format!(" · {pending} to send") } else { String::new() };
            Some((format!("Not synced: changes not applied{to_send}"), false, None))
        }
        (_, Some(SyncState::Done { at, problems, pending, .. })) => {
            let ago = sync_ago(*at);
            let failed = problems.iter().filter(|p| !p.note).count();
            let mut text = format!("Synced {ago}");
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
    if let Some((text, bad, detail)) = right {
        let color = if bad { style::c(p.failure) } else { style::disabled_text(&p) };
        let r = style::label(ui.painter(), status.right() - m::SPACING, status.center().y, Align::Max, text, style::small_font(), color);
        // Click a problem to read all of it.
        if let Some(detail) = detail {
            let response = ui.interact(r, Id::unique("passwords-sync-status"), Sense::click());
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.clicked() {
                match detail {
                    More::Text(text) => app.show_problem(cx, text),
                    More::Problems(list) => app.show_problems(cx, list),
                }
            }
        }
        if review_click && ui.interact(r, Id::unique("passwords-sync-review"), Sense::click()).clicked() {
            if app.review.is_some() {
                app.open_review(cx);
            } else {
                app.sync_and_review(cx);
            }
        }
    }

    // The three panes.
    let inner = body.shrink(m::SPACING);
    let sources = Rect::from_min_size(inner.min, vec2(SOURCES_WIDTH, inner.height()));
    let list = Rect::from_min_size(pos2(sources.right() + m::SPACING, inner.top()), vec2(LIST_WIDTH, inner.height()));
    let detail = Rect::from_min_max(pos2(list.right() + m::SPACING, inner.top()), inner.max);
    sources_pane(app, cx, ui, sources);
    items_pane(app, cx, ui, list);
    detail_pane(app, cx, ui, detail);
}

fn sources_pane(app: &mut Passwords, cx: &mut Cx, ui: &mut Ui, rect: Rect) {
    let p = style::palette(ui.ctx());
    let inner = well(ui, rect, p.list_background, false);
    let active = |i: &&ItemOverview| i.state == ItemState::Active;
    let mut rows: Vec<(Option<Source>, String, &str, usize)> = Vec::new();
    rows.push((Some(Source::All), "All Items".into(), "passwords", app.items.iter().filter(active).count()));
    // With several accounts, each account's vaults under its name (as
    // Tracker lists volumes), then any kept only here.
    let mut groups: Vec<(String, Vec<&onepassword::VaultOverview>)> = Vec::new();
    if app.settings.accounts.len() > 1 {
        for l in &app.settings.accounts {
            let vaults: Vec<_> = app.vaults.iter().filter(|v| app.vault_accounts.get(&v.id) == Some(&l.id)).collect();
            groups.push((l.label.clone(), vaults));
        }
        let here: Vec<_> = app.vaults.iter().filter(|v| !app.vault_accounts.contains_key(&v.id)).collect();
        if !here.is_empty() {
            groups.push(("On This Computer".into(), here));
        }
    } else if !app.vaults.is_empty() {
        groups.push(("Vaults".into(), app.vaults.iter().collect()));
    }
    for (heading, vaults) in groups.into_iter().filter(|(_, v)| !v.is_empty()) {
        rows.push((None, heading, "", 0));
        for v in vaults {
            let n = app.items.iter().filter(active).filter(|i| i.vault_id == v.id).count();
            rows.push((Some(Source::Vault(v.id.clone())), v.title.clone(), "vault", n));
        }
    }
    let mut categories: Vec<ItemCategory> = app.items.iter().filter(active).map(|i| i.category).collect();
    categories.sort_by_key(|c| c.label());
    categories.dedup();
    if !categories.is_empty() {
        rows.push((None, "Categories".into(), "", 0));
        for c in categories {
            let n = app.items.iter().filter(active).filter(|i| i.category == c).count();
            rows.push((Some(Source::Category(c)), plural(c), icon(c), n));
        }
    }
    let archived = app.items.iter().filter(|i| i.state == ItemState::Archived).count();
    if archived > 0 {
        rows.push((None, String::new(), "", 0));
        rows.push((Some(Source::Archive), "Archive".into(), "archive", archived));
    }

    let mut clicked = None;
    let mut child = child(ui, inner);
    ScrollView::new("passwords-sources").border(false).auto_hide(true).show(&mut child, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for (source, label, icon_name, count) in &rows {
            let height = if source.is_some() { SOURCE_ROW } else if label.is_empty() { m::TIGHT * 2.0 } else { SOURCE_ROW + 2.0 };
            let (r, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
            let Some(source) = source else {
                if !label.is_empty() {
                    style::label(ui.painter(), r.left() + m::SPACING, r.center().y + 2.0, Align::Min, label, style::bold_font(), dim(&p));
                }
                continue;
            };
            let selected = &app.source == source;
            let text = if selected { style::c(p.list_selected_item_text) } else { style::c(p.list_item_text) };
            if selected {
                ui.painter().rect_filled(r, 0.0, style::c(p.list_selected_background));
            }
            icons::paint(ui.painter(), Rect::from_center_size(pos2(r.left() + m::SPACING + 8.0, r.center().y), vec2(16.0, 16.0)), icon_name);
            let count_text = count.to_string();
            let count_w = style::label_width(ui.ctx(), &count_text, style::small_font());
            let mut job = egui::text::LayoutJob::simple_singleline(label.clone(), style::plain_font(), text);
            job.wrap = egui::text::TextWrapping::truncate_at_width(r.width() - 2.0 * m::SPACING - 24.0 - count_w - m::SPACING);
            let g = ui.painter().layout_job(job);
            let pos = style::text_origin(ui.ctx(), &g, r.left() + m::SPACING + 22.0, r.center().y, style::PLAIN_SIZE);
            ui.painter().galley(pos, g, text);
            style::label(ui.painter(), r.right() - m::SPACING, r.center().y, Align::Max, count_text, style::small_font(), if selected { text } else { dim(&p) });
            if response.clicked() {
                clicked = Some(source.clone());
            }
        }
    });
    if let Some(source) = clicked {
        app.set_source(cx, source);
    }
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

fn items_pane(app: &mut Passwords, cx: &mut Cx, ui: &mut Ui, rect: Rect) {
    let p = style::palette(ui.ctx());
    let list_id = Id::unique("passwords-items");
    let focused = ui.memory(|m| m.has_focus(list_id));
    let inner = well(ui, rect, p.list_background, focused);
    let visible: Vec<ItemOverview> = app.visible().into_iter().cloned().collect();
    let selected_row = visible.iter().position(|i| Some(&i.id) == app.selected.as_ref());

    // Arrow keys move the selection from the list or the search field.
    let typing = ui.memory(|m| m.focused()).is_some_and(|f| f != list_id);
    let search_focused = typing && ui.ctx().memory(|m| m.focused()).is_some() && app.editor.is_none();
    if (!typing || search_focused) && !visible.is_empty() && !olo_kit::menu::is_open(ui.ctx(), Id::unique("passwords-item-menu"))
        && !olo_kit::menu::is_open(ui.ctx(), Id::unique("passwords-new")) {
        let (up, down) = ui.input(|i| (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown)));
        let next = match (up, down, selected_row) {
            (true, _, Some(r)) => Some(r.saturating_sub(1)),
            (_, true, Some(r)) => Some((r + 1).min(visible.len() - 1)),
            (true, _, None) | (_, true, None) => Some(0),
            _ => None,
        };
        if let Some(row) = next
            && Some(row) != selected_row
        {
            app.select(cx, Some(visible[row].id.clone()));
            app.reveal_selection = true;
        }
    }

    let mut child = child(ui, inner);
    if visible.is_empty() {
        let text = if app.loading {
            "Loading…"
        } else if !app.search.is_empty() {
            "Nothing matches."
        } else {
            "No items."
        };
        style::label(child.painter(), inner.center().x, inner.top() + 40.0, Align::Center, text, style::plain_font(), dim(&p));
        return;
    }
    if std::mem::take(&mut app.reveal_selection)
        && let Some(row) = visible.iter().position(|i| Some(&i.id) == app.selected.as_ref())
    {
        ScrollView::reveal(child.ctx(), "passwords-list", row as f32 * ITEM_ROW, (row + 1) as f32 * ITEM_ROW);
    }
    let mut clicked: Option<(String, bool)> = None;
    let mut context: Option<(String, Pos2)> = None;
    ScrollView::new("passwords-list").border(false).focused(focused).show_rows(&mut child, ITEM_ROW, visible.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for row in range {
            let item = &visible[row];
            let (r, response) = ui.allocate_exact_size(vec2(ui.available_width(), ITEM_ROW), Sense::click());
            let selected = Some(&item.id) == app.selected.as_ref();
            let text = if selected { style::c(p.list_selected_item_text) } else { style::c(p.list_item_text) };
            if selected {
                ui.painter().rect_filled(r, 0.0, style::c(p.list_selected_background));
            }
            icons::paint(ui.painter(), Rect::from_center_size(pos2(r.left() + m::SPACING + 16.0, r.center().y), vec2(32.0, 32.0)), icon(item.category));
            let x = r.left() + m::SPACING * 2.0 + 32.0;
            let width = r.right() - x - m::SPACING;
            for (line, font, color, dy) in [
                (item.title.clone(), style::plain_font(), text, -8.0),
                (app.subtitle(item), style::small_font(), if selected { text } else { dim(&p) }, 8.0),
            ] {
                let mut job = egui::text::LayoutJob::simple_singleline(line, font.clone(), color);
                job.wrap = egui::text::TextWrapping::truncate_at_width(width);
                let g = ui.painter().layout_job(job);
                let pos = style::text_origin(ui.ctx(), &g, x, r.center().y + dy, font.size);
                ui.painter().galley(pos, g, color);
            }
            if response.clicked() || response.secondary_clicked() {
                clicked = Some((item.id.clone(), response.double_clicked()));
            }
            if response.double_clicked() {
                clicked = Some((item.id.clone(), true));
            }
            if response.secondary_clicked()
                && let Some(pos) = response.interact_pointer_pos()
            {
                context = Some((item.id.clone(), pos));
            }
        }
    });
    if let Some((id, double)) = clicked {
        ui.memory_mut(|m| m.request_focus(list_id));
        app.select(cx, Some(id));
        if double {
            app.open_website(cx);
        }
    }
    if ui.memory(|m| m.has_focus(list_id)) && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        app.open_website(cx);
    }
    let menu_id = Id::unique("passwords-item-menu");
    if let Some((id, pos)) = context
        && let Some(o) = app.overview(&id)
    {
        let archived = o.state == ItemState::Archived;
        // Right-clicking selected it, so its details are on the way;
        // until they arrive the copies stay enabled.
        let details = app.details.get(&id);
        let has = |t: ItemFieldType| details.is_none_or(|d| d.fields.iter().any(|f| f.field_type == t && !f.value.is_empty()));
        let menu = Menu::new("Item")
            .item("Copy Username", "copy:username", None)
            .enabled(details.is_none_or(|d| d.username().is_some()))
            .item("Copy Password", "copy:password", None)
            .enabled(has(ItemFieldType::Concealed))
            .item("Copy One-Time Password", "copy:otp", None)
            .enabled(has(ItemFieldType::Totp))
            .separator()
            .item("Open Website", "open", None)
            .enabled(!o.websites.is_empty())
            .item("Edit", "edit", None)
            .enabled(!archived)
            .separator()
            .item("Move to Archive", "archive", None)
            .enabled(!archived)
            .item("Delete…", "delete", None);
        widgets::popup_menu_at(ui.ctx(), menu_id, pos, menu);
    }
    if let Some(cmd) = widgets::popup_result(ui.ctx(), menu_id)
        && let Some(w) = app.main {
            app.command(cx, w, &cmd);
        }
}

fn detail_pane(app: &mut Passwords, cx: &mut Cx, ui: &mut Ui, rect: Rect) {
    let p = style::palette(ui.ctx());
    let inner = well(ui, rect, p.document_background, false);
    let mut child = child(ui, inner);

    if app.editor.is_some() {
        let vaults: Vec<_> = app.vaults.iter().map(|v| onepassword::VaultOverview { title: app.vault_name(v), ..v.clone() }).collect();
        let mut action = None;
        ScrollView::new("passwords-editor").border(false).auto_hide(true).show(&mut child, |ui| {
            let mut content = ui.new_child(
                UiBuilder::new()
                    .max_rect(Rect::from_min_size(ui.min_rect().min + vec2(m::WINDOW_INSET, m::WINDOW_INSET), vec2(ui.available_width() - 2.0 * m::WINDOW_INSET, f32::INFINITY)))
                    .layout(egui::Layout::top_down(Align::Min)),
            );
            if let Some(editor) = &mut app.editor {
                action = editor.ui(&mut content, &vaults);
            }
            ui.allocate_space(vec2(ui.available_width(), content.min_rect().height() + 2.0 * m::WINDOW_INSET));
        });
        match action {
            Some(EditAction::Save) => app.save_editor(cx),
            Some(EditAction::Cancel) => app.editor = None,
            Some(EditAction::Generate(field)) => {
                let item = app.editor.as_ref().and_then(|e| e.item_id()).unwrap_or("").to_string();
                app.open_generator(cx, Some((item, field)));
            }
            None => {}
        }
        return;
    }

    let Some(id) = app.selected.clone() else {
        if !app.loading {
            style::label(child.painter(), inner.center().x, inner.center().y, Align::Center, "No item selected", style::plain_font(), dim(&p));
        }
        return;
    };
    let Some(overview) = app.overview(&id).cloned() else { return };
    let Some(item) = app.details.get(&id).cloned() else {
        let (text, color) = match app.missing.get(&id) {
            Some(e) => (format!("Couldn’t open this item: {e}"), style::c(p.failure)),
            None => ("Loading…".to_string(), dim(&p)),
        };
        header(&mut child, &overview.title, overview.category, &app.vault_title(&overview.vault_id));
        let g = child.painter().layout(text, style::plain_font(), color, inner.width() - 2.0 * m::WINDOW_INSET);
        child.painter().galley(pos2(inner.left() + m::WINDOW_INSET, inner.top() + 96.0), g, color);
        return;
    };

    let vault = app.vault_title(&item.vault_id).to_string();
    let archived = overview.state == ItemState::Archived;
    let mut copy: Option<String> = None;
    let mut drag: Option<String> = None;
    let mut toggle: Option<String> = None;
    let mut open: Option<String> = None;
    let mut edit = false;
    ScrollView::new("passwords-detail").border(false).auto_hide(true).show(&mut child, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let width = ui.available_width();
        let header_rect = header(ui, &item.title, item.category, &vault);
        if !archived {
            let button = Rect::from_min_size(pos2(header_rect.right() - m::WINDOW_INSET - 64.0, header_rect.top() + m::WINDOW_INSET + 2.0), vec2(64.0, m::CONTROL_HEIGHT));
            if place(ui, button, Button::new("Edit").min_width(64.0)).clicked() {
                edit = true;
            }
        } else {
            let (r, _) = ui.allocate_exact_size(vec2(width, 28.0), Sense::hover());
            style::label(ui.painter(), r.left() + m::WINDOW_INSET, r.center().y, Align::Min, "This item is in the Archive.", style::italic_font(), dim(&p));
        }

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
            if fields.is_empty() {
                continue;
            }
            if let Some(title) = section.as_ref().and_then(|s| item.sections.iter().find(|x| &x.id == s)).map(|s| s.title.clone()) {
                section_heading(ui, &title);
            }
            for f in fields {
                match field_row(ui, f, app.revealed.contains(&f.id)) {
                    Some(FieldAction::Copy) => copy = Some(f.id.clone()),
                    Some(FieldAction::Drag) => drag = Some(f.id.clone()),
                    Some(FieldAction::Toggle) => toggle = Some(f.id.clone()),
                    Some(FieldAction::Open(url)) => open = Some(url),
                    None => {}
                }
            }
        }
        if !item.websites.is_empty() {
            section_heading(ui, "Websites");
            for w in &item.websites {
                let f = ItemField::new("", &w.label, ItemFieldType::Url, &w.url);
                match field_row(ui, &f, true) {
                    Some(FieldAction::Copy) => {
                        open = None;
                        copy = Some(format!("url:{}", w.url));
                    }
                    Some(FieldAction::Open(url)) => open = Some(url),
                    Some(FieldAction::Drag) => drag = Some(format!("url:{}", w.url)),
                    _ => {}
                }
            }
        }
        if !item.notes.is_empty() {
            section_heading(ui, "Notes");
            let g = ui.painter().layout(item.notes.clone(), style::plain_font(), style::c(p.document_text), width - 2.0 * m::WINDOW_INSET);
            let (r, _) = ui.allocate_exact_size(vec2(width, g.size().y + m::SPACING * 2.0), Sense::hover());
            ui.painter().galley(pos2(r.left() + m::WINDOW_INSET, r.top() + m::TIGHT), g, style::c(p.document_text));
        }
        if !item.tags.is_empty() {
            section_heading(ui, "Tags");
            let (r, _) = ui.allocate_exact_size(vec2(width, SOURCE_ROW), Sense::hover());
            style::label(ui.painter(), r.left() + m::WINDOW_INSET, r.center().y, Align::Min, item.tags.join(", "), style::plain_font(), style::c(p.document_text));
        }
        ui.add_space(m::SPACING);
        let (r, _) = ui.allocate_exact_size(vec2(width, 34.0), Sense::hover());
        style::hline(ui.painter(), r.left() + m::WINDOW_INSET, r.right() - m::WINDOW_INSET, r.top(), style::shade(&p, p.document_background, 1.15));
        let when = format!("Modified {}   ·   Created {}", date(item.updated_at), date(item.created_at));
        style::label(ui.painter(), r.left() + m::WINDOW_INSET, r.center().y, Align::Min, when, style::small_font(), dim(&p));
    });

    if edit {
        app.edit_selected(cx);
    }
    if let Some(field) = toggle
        && !app.revealed.remove(&field)
    {
        app.revealed.insert(field);
    }
    if let Some(url) = open {
        crate::open_url(&url);
    }
    if let Some(field) = drag {
        app.drag_field(cx, ui.ctx(), &id, &field);
    }
    match copy {
        Some(url) if url.starts_with("url:") => {
            cx.copy(url[4..].to_string());
            app.flash("Copied the website.");
        }
        Some(field) => app.copy_field(cx, &id, &field),
        None => {}
    }
    // One-time codes count down.
    if item.fields.iter().any(|f| f.field_type == ItemFieldType::Totp) {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }
}

fn date(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%-d %b %Y, %H:%M").to_string()
}

/// Icon, title, and category · vault; returns the header's rect.
fn header(ui: &mut Ui, title: &str, category: ItemCategory, vault: &str) -> Rect {
    let p = style::palette(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0 + 2.0 * m::WINDOW_INSET), Sense::hover());
    icons::paint(ui.painter(), Rect::from_min_size(r.min + vec2(m::WINDOW_INSET, m::WINDOW_INSET), vec2(48.0, 48.0)), icon(category));
    let x = r.left() + m::WINDOW_INSET * 2.0 + 48.0;
    let width = r.right() - x - m::WINDOW_INSET - 72.0;
    let mut job = egui::text::LayoutJob::simple_singleline(title.into(), title_font(), style::c(p.document_text));
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    let g = ui.painter().layout_job(job);
    let pos = style::text_origin(ui.ctx(), &g, x, r.top() + m::WINDOW_INSET + 16.0, 18.0);
    ui.painter().galley(pos, g, style::c(p.document_text));
    let sub = if vault.is_empty() { category.label().to_string() } else { format!("{}  ·  {vault}", category.label()) };
    style::label(ui.painter(), x, r.top() + m::WINDOW_INSET + 38.0, Align::Min, sub, style::plain_font(), dim(&p));
    style::hline(ui.painter(), r.left() + m::WINDOW_INSET, r.right() - m::WINDOW_INSET, r.bottom() - 1.0, style::shade(&p, p.document_background, 1.15));
    r
}

fn section_heading(ui: &mut Ui, title: &str) {
    let p = style::palette(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    style::label(ui.painter(), r.left() + m::WINDOW_INSET, r.bottom() - 9.0, Align::Min, title, style::bold_font(), style::c(p.document_text));
}

enum FieldAction {
    Copy,
    /// Dragged out (onto another app's field).
    Drag,
    Toggle,
    Open(String),
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
        ItemFieldType::MonthYear if f.value.len() == 6 && f.value.chars().all(|c| c.is_ascii_digit()) => {
            (format!("{}/{}", &f.value[4..], &f.value[..4]), false)
        }
        ItemFieldType::Address => match &f.details {
            Some(onepassword::ItemFieldDetails::Address(Some(a))) => (
                [a.street.as_str(), a.city.as_str(), a.state.as_str(), a.zip.as_str(), a.country.as_str()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", "),
                false,
            ),
            _ => (f.value.clone(), false),
        },
        _ => (f.value.clone(), matches!(f.field_type, ItemFieldType::Concealed | ItemFieldType::SshKey)),
    }
}

fn field_row(ui: &mut Ui, f: &ItemField, revealed: bool) -> Option<FieldAction> {
    let p = style::palette(ui.ctx());
    if f.value.is_empty() && f.field_type != ItemFieldType::Totp {
        return None;
    }
    let mut action = None;
    let (r, response) = ui.allocate_exact_size(vec2(ui.available_width(), FIELD_ROW), Sense::click_and_drag());
    let hovered = ui.rect_contains_pointer(r);
    if hovered {
        ui.painter().rect_filled(r.shrink2(vec2(m::TIGHT, 1.0)), 2.0, style::shade(&p, p.document_background, 1.04));
    }
    let label = if f.title.is_empty() { edit::kind_label(f.field_type).to_lowercase() } else { f.title.clone() };
    let x = r.left() + m::WINDOW_INSET;
    style::label(ui.painter(), x, r.top() + 13.0, Align::Min, label, style::small_font(), dim(&p));
    let (value, mono) = shown(f, revealed);
    let link = f.field_type == ItemFieldType::Url || (f.field_type == ItemFieldType::Text && f.value.starts_with("http"));
    let color = if link { style::c(p.keyboard_navigation) } else { style::c(p.document_text) };
    let font = if mono { style::mono_font(style::PLAIN_SIZE) } else { style::plain_font() };
    let buttons_w = if hovered { 2.0 * 56.0 + m::TIGHT } else { 0.0 };
    let mut job = egui::text::LayoutJob::simple_singleline(value.replace('\n', " ⏎ "), font.clone(), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(r.right() - x - m::WINDOW_INSET - buttons_w);
    let g = ui.painter().layout_job(job);
    let pos = style::text_origin(ui.ctx(), &g, x, r.top() + 31.0, font.size);
    let value_rect = Rect::from_min_size(pos, g.size());
    ui.painter().galley(pos, g, color);
    if f.field_type == ItemFieldType::Totp && let Ok(t) = onepassword::totp::Totp::parse(&f.value) {
        // A little pie of the code's time left.
        let left = t.now().1 as f32 / t.period as f32;
        let c = pos2(value_rect.right() + m::SPACING + 7.0, value_rect.center().y);
        ui.painter().circle_stroke(c, 6.0, egui::Stroke::new(1.0, dim(&p)));
        let mut points = vec![c];
        let steps = 24;
        for i in 0..=steps {
            let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * left * i as f32 / steps as f32;
            points.push(c + vec2(a.cos(), a.sin()) * 5.0);
        }
        ui.painter().add(egui::Shape::convex_polygon(points, if left < 0.2 { style::c(p.failure) } else { dim(&p) }, egui::Stroke::NONE));
    }
    if hovered {
        let mut x = r.right() - m::WINDOW_INSET;
        let mut button = |ui: &mut Ui, text: &str| {
            let b = Rect::from_min_size(pos2(x - 56.0, r.center().y - m::CONTROL_HEIGHT / 2.0), vec2(56.0, m::CONTROL_HEIGHT));
            x -= 56.0 + m::TIGHT;
            place(ui, b, Button::new(text).min_width(56.0)).clicked()
        };
        if button(ui, "Copy") {
            action = Some(FieldAction::Copy);
        }
        if matches!(f.field_type, ItemFieldType::Concealed | ItemFieldType::SshKey | ItemFieldType::CreditCardNumber)
            && button(ui, if revealed { "Hide" } else { "Show" })
        {
            action = Some(FieldAction::Toggle);
        }
    }
    if action.is_none() && response.drag_started() {
        action = Some(FieldAction::Drag);
    }
    if action.is_none() && response.clicked() {
        let over_value = response.interact_pointer_pos().is_some_and(|p| value_rect.contains(p));
        action = Some(if link && over_value {
            let url = if f.value.contains("://") { f.value.clone() } else { format!("https://{}", f.value) };
            FieldAction::Open(url)
        } else {
            FieldAction::Copy
        });
    }
    if link && response.hovered() && response.hover_pos().is_some_and(|p| value_rect.contains(p)) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    action
}

/// A right-aligned row of buttons straight under what's above it.
fn buttons(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    ui.allocate_ui(vec2(width, m::CONTROL_HEIGHT + 4.0), |ui| widgets::button_row(ui, add));
}

/// Adds `widget` in `rect` without moving the layout cursor (`Ui::put` does).
fn place(ui: &mut Ui, rect: Rect, widget: impl egui::Widget) -> egui::Response {
    ui.new_child(UiBuilder::new().max_rect(rect)).add(widget)
}

/// What a click on the sync status shows.
enum More {
    Text(String),
    Problems(Vec<crate::sync::Problem>),
}

/// What the review window asks for.
pub enum ReviewAction {
    Apply,
    Cancel,
    Problems,
}

const REVIEW_ROW: f32 = 42.0;

/// The Review Sync window: every change a sync would make, both ways,
/// and nothing happens until Apply.
pub fn review(app: &Passwords, ui: &mut Ui) -> Option<ReviewAction> {
    let Some(review) = &app.review else { return None };
    let p = style::palette(ui.ctx());
    let mut action = None;
    widgets::window_content(ui, |ui| {
        ui.spacing_mut().item_spacing.y = m::SPACING;
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
        let text = format!(
            "Syncing with {} makes {}. Nothing changes on either side until you apply them.",
            app.account_label(),
            sides.join(" and ")
        );
        let g = ui.painter().layout(text, style::plain_font(), style::c(p.panel_text), ui.available_width());
        let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
        ui.painter().galley(r.min, g, style::c(p.panel_text));
        let list_height = ui.available_height() - m::CONTROL_HEIGHT - 4.0 - m::SPACING;
        ui.allocate_ui(vec2(ui.available_width(), list_height), |ui| {
            ScrollView::new("passwords-review").background(p.list_background).show_rows(ui, REVIEW_ROW, review.rows.len(), |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for row in &review.rows[range] {
                    let (r, response) = ui.allocate_exact_size(vec2(ui.available_width(), REVIEW_ROW), Sense::hover());
                    icons::paint(ui.painter(), Rect::from_center_size(pos2(r.left() + m::SPACING + 16.0, r.center().y), vec2(32.0, 32.0)), icon(row.category));
                    let x = r.left() + m::SPACING * 2.0 + 32.0;
                    let width = r.right() - x - m::SPACING;
                    let title = if row.vault.is_empty() { row.title.clone() } else { format!("{}  ·  {}", row.title, row.vault) };
                    let mut action = row.action().to_string();
                    if !row.detail.is_empty() {
                        action = format!("{action}: {}", row.detail);
                    }
                    let warn = matches!(row.what, crate::sync::What::Delete | crate::sync::What::Conflict);
                    for (line, font, color, dy) in [
                        (title, style::plain_font(), style::c(p.list_item_text), -8.0),
                        (action.clone(), style::small_font(), if warn { style::c(p.failure) } else { dim(&p) }, 8.0),
                    ] {
                        let mut job = egui::text::LayoutJob::simple_singleline(line, font.clone(), color);
                        job.wrap = egui::text::TextWrapping::truncate_at_width(width);
                        let g = ui.painter().layout_job(job);
                        let pos = style::text_origin(ui.ctx(), &g, x, r.center().y + dy, font.size);
                        ui.painter().galley(pos, g, color);
                    }
                    response.on_hover_text(action);
                }
            });
        });
        buttons(ui, |ui| {
            let apply = if review.rows.len() == 1 { "Apply Change".to_string() } else { format!("Apply {} Changes", review.rows.len()) };
            if ui.add(Button::new(&apply).default(true)).clicked() {
                action = Some(ReviewAction::Apply);
            }
            if ui.add(Button::new("Cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                action = Some(ReviewAction::Cancel);
            }
            // What couldn't be read: a line on the left, the list a click away.
            if !review.problems.is_empty() {
                ui.with_layout(egui::Layout::left_to_right(Align::Center), |ui| {
                    if problems_note(ui, &review.problems).clicked() {
                        action = Some(ReviewAction::Problems);
                    }
                });
            }
        });
    });
    action
}

/// "⚠ 3 items couldn’t be read", in the window's own text, as a link
/// to the Sync Problems window.
fn problems_note(ui: &mut Ui, list: &[crate::sync::Problem]) -> egui::Response {
    let p = style::palette(ui.ctx());
    let failed = list.iter().filter(|p| !p.note).count();
    let (icon, text) = match failed {
        0 => ("info", "Some items are left out…".to_string()),
        1 => ("warning", "1 problem reading 1Password…".to_string()),
        n => ("warning", format!("{n} problems reading 1Password…")),
    };
    let g = ui.painter().layout_no_wrap(text, style::small_font(), style::c(p.panel_text));
    let (r, response) = ui.allocate_exact_size(vec2(16.0 + m::SPACING / 2.0 + g.size().x, m::CONTROL_HEIGHT), Sense::click());
    icons::paint(ui.painter(), Rect::from_center_size(pos2(r.left() + 8.0, r.center().y), vec2(16.0, 16.0)), icon);
    let x = r.left() + 16.0 + m::SPACING / 2.0;
    let color = style::c(p.panel_text);
    let pos = style::text_origin(ui.ctx(), &g, x, r.center().y, style::small_font().size);
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        let y = pos.y + g.size().y - 1.0;
        ui.painter().hline(x..=x + g.size().x, y, egui::Stroke::new(1.0, color));
    }
    ui.painter().galley(pos, g, color);
    response
}

/// What the Sync Problems window asks for.
pub enum ProblemsAction {
    Close,
    SyncAgain,
    Copy(String),
}

/// The Sync Problems window: what a sync couldn't do, grouped by vault,
/// each with its reason in plain words.
pub fn problems(app: &Passwords, ui: &mut Ui) -> Option<ProblemsAction> {
    let Some((_, list)) = &app.problems else { return None };
    let p = style::palette(ui.ctx());
    let mut action = None;
    widgets::window_content(ui, |ui| {
        ui.spacing_mut().item_spacing.y = m::SPACING;
        let failed = list.iter().filter(|p| !p.note).count();
        // While a review waits, nothing has synced yet.
        let reviewing = app.review.is_some();
        let text = match (reviewing, failed) {
            (true, 0) => "These are left out of the sync under review:",
            (true, _) => "These parts of 1Password couldn’t be read, so they aren’t in the sync under review:",
            (false, 0) => "Everything else synced. These were left out:",
            (false, 1) => "Everything else synced, but this couldn’t be:",
            (false, _) => "Everything else synced, but these couldn’t be:",
        };
        let g = ui.painter().layout(text.into(), style::plain_font(), style::c(p.panel_text), ui.available_width());
        let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
        ui.painter().galley(r.min, g, style::c(p.panel_text));
        let list_height = ui.available_height() - m::CONTROL_HEIGHT - 4.0 - m::SPACING;
        ui.allocate_ui(vec2(ui.available_width(), list_height), |ui| {
            ScrollView::new("passwords-problems").background(p.list_background).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let width = ui.available_width();
                let mut vault: Option<&str> = None;
                for problem in list {
                    if vault != Some(problem.vault.as_str()) {
                        vault = Some(problem.vault.as_str());
                        let name = if problem.vault.is_empty() { "Not in a vault" } else { problem.vault.as_str() };
                        let (r, _) = ui.allocate_exact_size(vec2(width, 26.0), Sense::hover());
                        let font = style::bold_font();
                        let mut job = egui::text::LayoutJob::simple_singleline(name.to_string(), font.clone(), style::c(p.list_item_text));
                        job.wrap = egui::text::TextWrapping::truncate_at_width(width - m::SPACING * 2.0);
                        let g = ui.painter().layout_job(job);
                        let pos = style::text_origin(ui.ctx(), &g, r.left() + m::SPACING, r.center().y + 2.0, font.size);
                        ui.painter().galley(pos, g, style::c(p.list_item_text));
                        ui.painter().hline(r.left()..=r.right(), r.bottom() - 0.5, egui::Stroke::new(1.0, dim(&p).gamma_multiply(0.4)));
                    }
                    problem_row(ui, &p, problem, width);
                }
            });
        });
        buttons(ui, |ui| {
            if ui.add(Button::new("Close").default(true)).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                action = Some(ProblemsAction::Close);
            }
            if failed > 0 && !reviewing && ui.add(Button::new("Sync Again")).clicked() {
                action = Some(ProblemsAction::SyncAgain);
            }
            if ui.add(Button::new("Copy")).clicked() {
                action = Some(ProblemsAction::Copy(crate::sync::as_text(list)));
            }
        });
    });
    action
}

/// One problem: its icon, the item (or the reason, for the whole
/// vault) and the reason under it, wrapped.
fn problem_row(ui: &mut Ui, p: &olo_theme::Palette, problem: &crate::sync::Problem, width: f32) {
    let x = m::SPACING * 2.0 + 16.0;
    let text_width = width - x - m::SPACING;
    let title = problem.item.as_ref().map(|item| {
        let mut job = egui::text::LayoutJob::simple_singleline(item.clone(), style::plain_font(), style::c(p.list_item_text));
        job.wrap = egui::text::TextWrapping::truncate_at_width(text_width);
        ui.painter().layout_job(job)
    });
    let reason_font = if title.is_some() { style::small_font() } else { style::plain_font() };
    let reason_color = if title.is_some() { dim(p) } else { style::c(p.list_item_text) };
    let reason = ui.painter().layout(problem.reason.clone(), reason_font, reason_color, text_width);
    let pad = 6.0;
    let height = pad * 2.0 + title.as_ref().map_or(0.0, |t| t.size().y + 1.0) + reason.size().y;
    let (r, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let icon = if problem.note { "info" } else { "warning" };
    let first_line = title.as_ref().map_or(reason.rows.first().map_or(16.0, |row| row.height()), |t| t.size().y);
    icons::paint(ui.painter(), Rect::from_min_size(pos2(r.left() + m::SPACING, r.top() + pad + (first_line - 16.0) / 2.0), vec2(16.0, 16.0)), icon);
    let mut y = r.top() + pad;
    if let Some(t) = title {
        let h = t.size().y;
        ui.painter().galley(pos2(r.left() + x, y), t, style::c(p.list_item_text));
        y += h + 1.0;
    }
    ui.painter().galley(pos2(r.left() + x, y), reason, reason_color);
}
