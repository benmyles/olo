//! Passwords for agent screens (Ben, 2026-10-31): what's shared from the
//! person's Passwords with every screen (Screen ▸ Passwords for Every
//! Screen…) and with each one (its Passwords tab: more, or less, than
//! every screen has). The list is every item Passwords has, searched and
//! filtered with the kit's language (`vault:`, `kind:`, `shared:`),
//! several chosen at once (⇧, ⌘, ⌘A) and shared or not in one go.
//!
//! Passwords keeps the choice, sealed in its store, and writes what each
//! screen has as a copy beside the screen that the screen's own
//! Passwords opens by itself (`olo_keychain::shared`). Asked from here
//! (the person's display, over the keychain socket), never from a
//! screen; asking waits for the person's Passwords to be unlocked.

use std::{
    collections::{BTreeMap, HashSet},
    rc::Rc,
};

use olo_keychain::{Request, Response, ScreenShares, Shareable};
use olo_ui::{
    agent,
    gpui::{AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, canvas, div, px},
    metrics as m,
    search::{Facet, FilterBar, Query, SearchEvent, SearchField, search_field},
    style, theme,
    widgets::{Button, Column, ColumnListView, EmptyState, ListModel},
};

/// The filters the search knows.
const KEYS: &[&str] = &["vault", "kind", "shared"];

/// What Passwords said, or why it couldn't.
enum State {
    Asking,
    Ready(Data),
    /// Passwords isn't running: it can be opened.
    NotRunning,
    Problem(String),
}

struct Data {
    items: Vec<Shareable>,
    everywhere: HashSet<String>,
    screens: BTreeMap<String, ScreenShares>,
}

/// How an item stands with the screen (or every screen) being looked at.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// Every screen has it (and this one does).
    Everywhere,
    /// This screen has it besides.
    Here,
    /// Every screen has it, but not this one.
    NotHere,
    No,
}

impl Standing {
    fn shared(self) -> bool {
        matches!(self, Standing::Everywhere | Standing::Here)
    }

    /// As the list's first column says it.
    fn said(self, screen: bool) -> &'static str {
        match (self, screen) {
            (Standing::Everywhere, true) => "Every screen",
            (Standing::Everywhere, false) => "Shared",
            (Standing::Here, _) => "This screen",
            (Standing::NotHere, _) => "Not here",
            (Standing::No, _) => "",
        }
    }
}

impl Data {
    fn standing(&self, id: &str, screen: Option<&str>) -> Standing {
        let all = self.everywhere.contains(id);
        let s = screen.and_then(|n| self.screens.get(n));
        match (all, s) {
            (true, Some(s)) if s.remove.iter().any(|r| r == id) => Standing::NotHere,
            (true, _) => Standing::Everywhere,
            (false, Some(s)) if s.add.iter().any(|a| a == id) => Standing::Here,
            _ => Standing::No,
        }
    }
}

/// What's shared with `screen` (None: every screen): the editor.
pub struct Shares {
    screen: Option<String>,
    state: State,
    search: Entity<SearchField>,
    query: String,
    list: Entity<ListModel>,
    /// The items shown (indexes into `Data::items`), in order.
    shown: Vec<usize>,
    /// A change on its way to Passwords.
    saving: bool,
    /// How wide it's laid out (its window or tab says).
    pub width: f32,
}

impl Shares {
    pub fn new(screen: Option<String>, cx: &mut Context<Shares>) -> Shares {
        let search = search_field(cx, "Search: words, vault:, kind:, shared:");
        cx.subscribe(&search, |s: &mut Shares, f, e: &SearchEvent, cx| {
            if let SearchEvent::Changed = e {
                s.query = f.read(cx).text(cx).to_owned();
                s.pick(cx);
            }
        })
        .detach();
        let list = ListModel::new(cx, true);
        let mut s = Shares { screen, state: State::Asking, search, query: String::new(), list, shown: Vec::new(), saving: false, width: 600.0 };
        s.ask(cx);
        s
    }

    /// Ask Passwords what's shared (on a worker: it waits for an unlock).
    fn ask(&mut self, cx: &mut Context<Shares>) {
        self.state = State::Asking;
        cx.notify();
        let task = cx.background_executor().spawn(async move { olo_keychain::call(&Request::Shares) });
        cx.spawn(async move |me, cx| {
            let answer = task.await;
            let _ = me.update(cx, |s, cx| s.answered(answer, cx));
        })
        .detach();
    }

    fn answered(&mut self, answer: Result<Response, olo_keychain::CallError>, cx: &mut Context<Shares>) {
        self.saving = false;
        // (Response is zeroed when dropped, so it's read, not taken apart.)
        self.state = match &answer {
            Ok(Response::Shares { items, everywhere, screens }) => State::Ready(Data { items: items.clone(), everywhere: everywhere.iter().cloned().collect(), screens: screens.clone() }),
            Ok(Response::Shared) => return self.ask(cx),
            Ok(Response::NotSetUp) => State::Problem("Passwords isn’t set up yet: open it to set it up, then share from it here.".into()),
            Ok(Response::Error { message }) => State::Problem(message.clone()),
            Ok(_) => State::Problem("Passwords gave an answer Agent Screens doesn’t know.".into()),
            Err(olo_keychain::CallError::NotRunning) => State::NotRunning,
            Err(e) => State::Problem(e.to_string()),
        };
        self.pick(cx);
    }

    /// The items the search leaves, in order.
    fn pick(&mut self, cx: &mut Context<Shares>) {
        let State::Ready(d) = &self.state else {
            self.shown.clear();
            return cx.notify();
        };
        let q = Query::parse(&self.query, KEYS);
        let screen = self.screen.as_deref();
        self.shown = (0..d.items.len())
            .filter(|&i| {
                let it = &d.items[i];
                q.is_empty()
                    || q.matches(|key, n| match key {
                        None => n.in_text(&it.title) || n.in_text(&it.subtitle) || n.in_text(&it.vault_title) || n.in_text(&it.kind),
                        Some("vault") => n.is(&it.vault_title) || n.in_text(&it.vault_title),
                        Some("kind") => n.is(&it.kind) || n.in_text(&it.kind),
                        Some("shared") => n.is(if d.standing(&it.id, screen).shared() { "yes" } else { "no" }),
                        _ => false,
                    })
            })
            .collect();
        let rows = self.shown.len();
        self.list.update(cx, |l, _| l.state.selected.retain(|&r| r < rows));
        cx.notify();
    }

    fn facets(&self) -> Rc<Vec<Facet>> {
        let State::Ready(d) = &self.state else { return Rc::new(Vec::new()) };
        let mut vaults: Vec<&str> = d.items.iter().map(|i| i.vault_title.as_str()).collect();
        vaults.sort_unstable();
        vaults.dedup();
        let mut kinds: Vec<&str> = d.items.iter().map(|i| i.kind.as_str()).collect();
        kinds.sort_unstable();
        kinds.dedup();
        let count = |f: &dyn Fn(&Shareable) -> bool| Some(d.items.iter().filter(|i| f(i)).count());
        let vault = vaults.iter().fold(Facet::new("vault", "Vault").any("All Vaults"), |f, v| f.value(v.to_lowercase(), *v, count(&|i| i.vault_title == *v)));
        let kind = kinds.iter().fold(Facet::new("kind", "Kind").any("All Kinds"), |f, k| f.value(k.to_lowercase(), *k, count(&|i| i.kind == *k)));
        let screen = self.screen.as_deref();
        let on = count(&|i| d.standing(&i.id, screen).shared());
        let shared = Facet::new("shared", "Shared").any("Shared or Not").value("yes", "Shared", on).value("no", "Not Shared", Some(d.items.len() - on.unwrap_or(0)));
        Rc::new(vec![shared, vault, kind])
    }

    /// The chosen items' ids.
    fn chosen(&self, cx: &Context<Shares>) -> Vec<String> {
        let State::Ready(d) = &self.state else { return Vec::new() };
        self.list.read(cx).state.selected.iter().filter_map(|&r| self.shown.get(r)).map(|&i| d.items[i].id.clone()).collect()
    }

    /// Share the chosen items (`on`), or stop.
    fn share(&mut self, on: bool, cx: &mut Context<Shares>) {
        let ids = self.chosen(cx);
        let State::Ready(d) = &self.state else { return };
        if ids.is_empty() || self.saving {
            return;
        }
        let request = match &self.screen {
            None => {
                let mut all: Vec<String> = d.items.iter().map(|i| i.id.clone()).filter(|id| d.everywhere.contains(id)).collect();
                if on {
                    all.extend(ids.iter().filter(|id| !d.everywhere.contains(*id)).cloned());
                } else {
                    all.retain(|id| !ids.contains(id));
                }
                Request::SetShares { everywhere: Some(all), screen: None }
            }
            Some(name) => {
                let mut s = d.screens.get(name).cloned().unwrap_or_default();
                for id in &ids {
                    let all = d.everywhere.contains(id);
                    s.add.retain(|a| a != id);
                    s.remove.retain(|r| r != id);
                    match (on, all) {
                        (true, false) => s.add.push(id.clone()),
                        (false, true) => s.remove.push(id.clone()),
                        _ => {}
                    }
                }
                Request::SetShares { everywhere: None, screen: Some((name.clone(), s)) }
            }
        };
        self.saving = true;
        cx.notify();
        let task = cx.background_executor().spawn(async move { olo_keychain::call(&request) });
        cx.spawn(async move |me, cx| {
            let answer = task.await;
            let _ = me.update(cx, |s, cx| s.answered(answer, cx));
        })
        .detach();
    }

    fn open_passwords(&mut self, cx: &mut Context<Shares>) {
        let started =
            olo_xdg::find_program("olo-passwords").ok_or_else(|| "Passwords isn’t installed.".to_owned()).and_then(|p| olo_xdg::open::spawn(&[p.into_os_string()]).map_err(|e| e.to_string()));
        match started {
            Ok(()) => self.ask(cx),
            Err(e) => {
                self.state = State::Problem(format!("Passwords couldn’t be started: {e}"));
                cx.notify();
            }
        }
    }
}

impl Render for Shares {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let empty = |text: &str| div().size_full().child(EmptyState::new(text.to_owned()).icon("passwords"));
        let d = match &self.state {
            State::Asking => return empty("Asking Passwords… (unlock it if it asks).").into_any_element(),
            State::NotRunning => {
                let to = me.clone();
                let e = EmptyState::new("Passwords isn’t open: what’s shared comes from it.").icon("passwords").action("Open Passwords", move |_, cx| to.update(cx, |s, cx| s.open_passwords(cx)));
                return div().size_full().child(e).into_any_element();
            }
            State::Problem(message) => {
                let to = me.clone();
                let e = EmptyState::new(message.clone()).icon("passwords").action("Try Again", move |_, cx| to.update(cx, |s, cx| s.ask(cx)));
                return div().size_full().child(e).into_any_element();
            }
            State::Ready(d) => d,
        };
        let screen = self.screen.clone();
        let cells: Rc<Vec<[String; 5]>> = Rc::new(
            self.shown
                .iter()
                .map(|&i| {
                    let it = &d.items[i];
                    [d.standing(&it.id, screen.as_deref()).said(screen.is_some()).to_owned(), it.title.clone(), it.subtitle.clone(), it.vault_title.clone(), it.kind.clone()]
                })
                .collect(),
        );
        let columns = vec![Column::new("Shared", 92.0), Column::new("Item", 200.0), Column::new("Username or Site", 180.0), Column::new("Vault", 110.0), Column::new("Kind", 100.0)];
        let names = cells.clone();
        let empty_text = if d.items.is_empty() { "Passwords has no items." } else { "No items match." };
        let list = ColumnListView::new(format!("shares-{}", screen.as_deref().unwrap_or("every-screen")), &self.list, columns, cells.len(), {
            let cells = cells.clone();
            move |cell, window, cx| {
                if let Some(row) = cells.get(cell.row) {
                    cell.text(window, cx, row[cell.column].clone());
                }
            }
        })
        .type_select(move |r| names.get(r).map(|c| SharedString::from(c[1].clone())).unwrap_or_default())
        .empty_text(empty_text);
        // How many are shared, said beside the search.
        let on = d.items.iter().filter(|i| d.standing(&i.id, screen.as_deref()).shared()).count();
        let summary = match &screen {
            None => format!("{on} of {} shared with every screen", d.items.len()),
            Some(_) => format!("{on} of {} shared with this screen", d.items.len()),
        };
        let facets = self.facets();
        let bar_w = FilterBar::width(cx, &self.query, &facets);
        let filters_me = me.clone();
        let filters = FilterBar::new(format!("shares-filter-{}", screen.as_deref().unwrap_or("all")), self.query.clone(), facets).no_clear().on_change(move |text, _, cx| {
            filters_me.update(cx, |s, cx| {
                s.search.update(cx, |f, cx| f.set_text(text.clone(), cx));
                s.query = text;
                s.pick(cx);
            })
        });
        let label = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let r = style::label_fit(
                    window,
                    cx,
                    b.left(),
                    b.center().y,
                    style::Align::Left,
                    summary.clone(),
                    style::PLAIN_SIZE,
                    style::ui_font(FontWeight::NORMAL),
                    style::disabled_text(&p),
                    b.size.width,
                );
                agent::record(window, cx, agent::Node::label(summary.clone(), r));
            },
        )
        .flex_1()
        .min_w_0()
        .h(px(m::CONTROL_HEIGHT));
        let chosen = !self.list.read(cx).state.selected.is_empty() && !self.saving;
        let (share_me, stop_me) = (me.clone(), me.clone());
        let buttons = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            .child(label)
            .child(Button::new("Stop Sharing").enabled(chosen).on_click(move |_, cx| stop_me.update(cx, |s, cx| s.share(false, cx))))
            .child(Button::new("Share").enabled(chosen).on_click(move |_, cx| share_me.update(cx, |s, cx| s.share(true, cx))));
        // The search and its filters on one row if they fit, else the
        // filters under it.
        let width = self.width;
        let field_w = 260.0f32.min(width);
        let top = if field_w + m::SPACING + bar_w <= width {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(m::SPACING))
                .child(div().w(px(field_w)).flex_shrink_0().child(self.search.clone()))
                .child(div().flex_1().min_w_0().child(filters.max_width(width - field_w - m::SPACING)))
        } else {
            // (Its first menu's text over the list's first column's, as
            // History has it.)
            let frame = f32::from(style::inset(window, olo_ui::gpui::Bounds::default(), 2.0).left());
            let indent = frame + m::CELL_INSET - m::TEXT_INSET;
            div().flex().flex_col().gap(px(m::SPACING)).child(div().w(px(field_w)).child(self.search.clone())).child(div().pl(px(indent)).child(filters.max_width(width - indent)))
        };
        div().size_full().flex().flex_col().gap(px(m::SPACING)).child(top).child(div().flex_1().min_h_0().child(list)).child(buttons).into_any_element()
    }
}

/// The window of what every screen has.
pub fn open_everywhere(
    kept: Option<olo_ui::gpui::WindowHandle<olo_ui::Frame>>,
    cx: &mut Context<crate::viewer::Viewer>,
    keep: impl FnOnce(&mut crate::viewer::Viewer, Option<olo_ui::gpui::WindowHandle<olo_ui::Frame>>) + 'static,
) {
    let options = olo_ui::WindowOptions::new("Passwords for Every Screen", 820.0, 520.0).min_size(560.0, 320.0).keep_size("shares");
    olo_ui::open_or_raise(
        kept,
        cx,
        options,
        |_, cx| {
            let view = cx.new(|cx| Shares::new(None, cx));
            cx.new(|_| Padded(view))
        },
        keep,
    );
}

/// The editor in a window's inset.
struct Padded(Entity<Shares>);

impl Render for Padded {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = f32::from(window.viewport_size().width) - 2.0 * m::WINDOW_INSET;
        self.0.update(cx, |s, _| s.width = width);
        div().size_full().p(px(m::WINDOW_INSET)).child(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn how_each_stands() {
        let d = Data {
            items: Vec::new(),
            everywhere: ["a", "b"].map(String::from).into_iter().collect(),
            screens: [("agent".to_owned(), ScreenShares { add: vec!["c".into()], remove: vec!["b".into()] })].into_iter().collect(),
        };
        let on = |id: &str, s: Option<&str>| d.standing(id, s);
        assert!(on("a", Some("agent")) == Standing::Everywhere && on("b", Some("agent")) == Standing::NotHere && on("c", Some("agent")) == Standing::Here && on("d", Some("agent")) == Standing::No);
        assert!(on("b", None) == Standing::Everywhere && on("c", None) == Standing::No && on("b", Some("other")) == Standing::Everywhere);
        assert!(!Standing::NotHere.shared() && Standing::Here.shared());
    }
}
