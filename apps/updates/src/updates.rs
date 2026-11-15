//! The Updates section: every update in a list (its check box, name, the
//! installed and new versions, what kind it is and how severe, its
//! repository and size), searched and filtered as every Olo list is
//! (`kind:`, `severity:`, `repo:`, `chosen:`), the selected one's details
//! beside it: versions, sizes, its advisories (with their CVEs and bugs,
//! linked), its changelog since what's installed, what it needs. Under the
//! list, Olo Doctor's pattern for acting on all, some or one (Choose: All
//! · Security Updates · Bug Fixes · Enhancements · Other Updates · None,
//! and "Update 12 Packages…"); a row's own menu updates just it.

use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

use olo_kit::{
    Bounds, Child, ClickEvent, Cx, Element, FontWeight, IntoElement as _, ParentElement as _, Pixels, Point, ScrollHandle, Styled as _, agent, chosen, div, menu, metrics as m, px,
    search::{self, Facet, Query, SearchEvent, SearchField, search_field},
    style, theme,
    widgets::{Button, Column, ColumnListView, LinkButton, ListModel, ScrollView, StringView, Tag, check_box_look},
};
use olo_system::dnf::{self, Advisory, Details, Update};

use crate::{
    Msg as AppMsg, OsUpdates, Section,
    review::{self, Op, Plan},
    words,
};

pub const SECURITY: chosen::Kind = chosen::Kind { verb: "Update", one: "Security Update", many: "Security Updates", by_default: true };
pub const BUGFIX: chosen::Kind = chosen::Kind { verb: "Update", one: "Bug Fix", many: "Bug Fixes", by_default: true };
pub const ENHANCEMENT: chosen::Kind = chosen::Kind { verb: "Update", one: "Enhancement", many: "Enhancements", by_default: true };
pub const OTHER: chosen::Kind = chosen::Kind { verb: "Update", one: "Other Update", many: "Other Updates", by_default: true };
const KINDS: [&chosen::Kind; 4] = [&SECURITY, &BUGFIX, &ENHANCEMENT, &OTHER];

/// The details' column.
const DETAILS: f32 = 300.0;
/// The labels' column in the details.
const LABELS: f32 = 84.0;
const KEYS: &[&str] = &["kind", "severity", "repo", "chosen"];

#[derive(Clone)]
pub enum Msg {
    Searched,
    Selected,
    Sorted,
    /// A row's check box (by row), clicked.
    Toggle(usize),
    Details(String, Result<Arc<Details>, String>),
    Act,
    Choose(chosen::Choice),
    /// Update (or reinstall) just these keys.
    Only(Op, Vec<String>),
}

pub struct State {
    pub search: Child<SearchField>,
    pub query: Query,
    facets: Option<Rc<Vec<Facet>>>,
    pub chosen: chosen::Chosen,
    pub list: ListModel,
    /// The updates shown, as indices into `updates`, in the order shown.
    rows: Rc<Vec<usize>>,
    /// Each shown row's check box, where it was drawn.
    checks: Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>,
    /// The update shown in the details ("name.arch").
    pub selected: Option<String>,
    details: HashMap<String, Result<Arc<Details>, String>>,
    scroll: ScrollHandle,
}

/// Which of the kinds an update is (its index in `KINDS`).
fn kind_of(u: &Update, advisories: &HashMap<String, Advisory>) -> usize {
    match u.kind(advisories) {
        "security" => 0,
        "bugfix" => 1,
        "enhancement" => 2,
        _ => 3,
    }
}

/// The worst severity among an update's advisories ("" with none).
fn severity_of<'a>(u: &Update, advisories: &'a HashMap<String, Advisory>) -> &'a str {
    u.advisories.iter().filter_map(|id| advisories.get(id)).map(|a| words::severity(&a.severity)).filter(|s| !s.is_empty()).min_by_key(|s| words::severity_rank(s)).unwrap_or("")
}

impl State {
    pub fn new(cx: &mut Cx<OsUpdates>, text: &str, chosen: chosen::Chosen, selected: Option<String>) -> State {
        let search = search_field(cx, "Search: names, CVEs, kind:, severity:, repo:");
        let t = text.to_owned();
        search.update(cx, |s, cx| s.set_text(t, cx));
        cx.subscribe(&search, |_: &mut OsUpdates, e: &SearchEvent, cx| {
            if let SearchEvent::Changed = e {
                cx.send(AppMsg::Updates(Msg::Searched));
            }
        });
        State {
            search,
            query: Query::parse(text, KEYS),
            facets: None,
            chosen,
            list: ListModel::new(true),
            rows: Rc::default(),
            checks: Rc::default(),
            selected,
            details: HashMap::new(),
            scroll: ScrollHandle::new(),
        }
    }

    /// New updates read: what's chosen of what's gone is forgotten, the
    /// rows made again, the selection kept on the same update.
    pub fn took(&mut self, updates: &dnf::Updates, cx: &mut Cx<OsUpdates>) {
        let keys: Vec<String> = updates.updates.iter().map(|u| u.new.key()).collect();
        self.chosen.keep(keys.iter().map(String::as_str));
        self.facets = None;
        self.details.clear();
        self.refilter(updates);
        self.select_again(updates, cx);
    }

    fn kind(&self, u: &Update, updates: &dnf::Updates) -> &'static chosen::Kind {
        KINDS[kind_of(u, &updates.advisories)]
    }

    pub fn is_chosen(&self, u: &Update, updates: &dnf::Updates) -> bool {
        self.chosen.is(&u.new.key(), self.kind(u, updates))
    }

    pub fn counts<'a>(&self, updates: &dnf::Updates) -> Vec<chosen::Count<'a>> {
        let mut out: Vec<chosen::Count<'a>> = KINDS.iter().map(|k| (*k, 0, 0)).collect();
        for u in &updates.updates {
            let i = kind_of(u, &updates.advisories);
            out[i].2 += 1;
            if self.chosen.is(&u.new.key(), KINDS[i]) {
                out[i].1 += 1;
            }
        }
        out
    }

    pub fn choose(&mut self, updates: &dnf::Updates, choice: chosen::Choice) {
        let items: Vec<(String, &chosen::Kind)> = updates.updates.iter().map(|u| (u.new.key(), KINDS[kind_of(u, &updates.advisories)])).collect();
        self.chosen.choose(items.iter().map(|(k, kind)| (k.as_str(), *kind)), |kind| KINDS.iter().position(|k| *k == kind).is_some_and(|i| choice.picks(i)));
        if !self.query.values("chosen").is_empty() {
            self.refilter(updates);
        }
    }

    /// The keys ("name.arch") of the rows selected.
    pub fn selected_keys(&self, updates: &dnf::Updates) -> Vec<String> {
        self.list.state().selected.iter().filter_map(|r| self.rows.get(*r)).filter_map(|i| updates.updates.get(*i)).map(|u| u.new.key()).collect()
    }

    /// Whether `u` matches the search.
    fn matches(&self, u: &Update, updates: &dnf::Updates) -> bool {
        let adv: Vec<&Advisory> = u.advisories.iter().filter_map(|id| updates.advisories.get(id)).collect();
        let chosen = self.is_chosen(u, updates);
        let kind = ["security", "bugfix", "enhancement", "other"][kind_of(u, &updates.advisories)];
        self.query.matches(|key, n| match key {
            Some("kind") => n.is(kind) || n.starts(kind),
            Some("severity") => adv.iter().any(|a| n.is(&a.severity) || n.starts(&a.severity)),
            Some("repo") => n.is(&u.new.repo_id) || n.starts(&u.new.repo_id),
            Some("chosen") => n.is(if chosen { "yes" } else { "no" }),
            Some(_) => false,
            None => {
                n.in_text(&u.new.name)
                    || n.in_text(&u.new.summary)
                    || n.in_text(&u.new.evr)
                    || adv.iter().any(|a| n.in_text(&a.id) || n.in_text(&a.title) || a.references.iter().any(|r| n.in_text(&r.id)))
            }
        })
    }

    /// The rows again: those that match, in the order asked for (by
    /// default the most pressing first, then by name).
    pub fn refilter(&mut self, updates: &dnf::Updates) {
        let mut rows: Vec<usize> = (0..updates.updates.len()).filter(|i| self.matches(&updates.updates[*i], updates)).collect();
        let sort = self.list.state().sort;
        let a = &updates.advisories;
        let key = |i: &usize| {
            let u = &updates.updates[*i];
            (kind_of(u, a), words::severity_rank(severity_of(u, a)), u.new.name.to_lowercase())
        };
        match sort {
            None => rows.sort_by_key(key),
            Some((column, ascending)) => {
                rows.sort_by(|x, y| {
                    let (u, v) = (&updates.updates[*x], &updates.updates[*y]);
                    let o = match column {
                        0 => self.is_chosen(v, updates).cmp(&self.is_chosen(u, updates)),
                        2 => u.installed.evr.cmp(&v.installed.evr),
                        3 => u.new.evr.cmp(&v.new.evr),
                        4 => key(x).cmp(&key(y)),
                        5 => u.new.repo_id.cmp(&v.new.repo_id),
                        6 => u.new.download_size.cmp(&v.new.download_size),
                        _ => u.new.name.to_lowercase().cmp(&v.new.name.to_lowercase()),
                    };
                    let o = o.then_with(|| u.new.name.to_lowercase().cmp(&v.new.name.to_lowercase()));
                    if ascending { o } else { o.reverse() }
                });
            }
        }
        self.rows = Rc::new(rows);
    }

    /// Keep the selection on the update it was on.
    fn select_again(&mut self, updates: &dnf::Updates, cx: &mut Cx<OsUpdates>) {
        let row = self.selected.as_ref().and_then(|k| self.rows.iter().position(|i| updates.updates.get(*i).is_some_and(|u| u.new.key() == *k)));
        let mut st = self.list.state_mut();
        st.selected.clear();
        if let Some(r) = row {
            st.select(r);
        }
        drop(st);
        let _ = cx;
    }

    /// The filters, from the updates (made again when they change).
    fn facets(&mut self, updates: &dnf::Updates, cx: &mut Cx<OsUpdates>) -> Rc<Vec<Facet>> {
        if let Some(f) = &self.facets {
            return f.clone();
        }
        let a = &updates.advisories;
        let mut kinds = [0usize; 4];
        let mut severities: HashMap<String, usize> = HashMap::new();
        let mut repos: HashMap<String, usize> = HashMap::new();
        for u in &updates.updates {
            kinds[kind_of(u, a)] += 1;
            let s = severity_of(u, a);
            if !s.is_empty() {
                *severities.entry(s.to_owned()).or_default() += 1;
            }
            *repos.entry(u.new.repo_id.clone()).or_default() += 1;
        }
        let mut kind = Facet::new("kind", "Kind").any("All Kinds");
        for (i, (code, label)) in [("security", "Security"), ("bugfix", "Bug Fix"), ("enhancement", "Enhancement"), ("other", "Other")].into_iter().enumerate() {
            kind = kind.value(code, label, Some(kinds[i]));
        }
        let mut severity = Facet::new("severity", "Severity").any("Any Severity");
        let mut sev: Vec<(String, usize)> = severities.into_iter().collect();
        sev.sort_by_key(|(s, _)| words::severity_rank(s));
        for (s, n) in sev {
            severity = severity.value(s.to_lowercase(), s, Some(n));
        }
        let mut repo = Facet::new("repo", "Repository").any("All Repositories").multiple();
        let mut rs: Vec<(String, usize)> = repos.into_iter().collect();
        rs.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
        for (r, n) in rs {
            repo = repo.value(r.clone(), r, Some(n));
        }
        let chosen = Facet::new("chosen", "Chosen").any("Chosen or Not").value("yes", "Chosen", None).value("no", "Not Chosen", None);
        let facets = Rc::new(vec![kind, severity, repo, chosen]);
        self.facets = Some(facets.clone());
        let shown = facets.clone();
        self.search.update(cx, |s, cx| s.set_facets(shown, cx));
        facets
    }

    /// Open the review of updating `keys` (or reinstalling them).
    fn review(&self, op: Op, keys: Vec<String>, updates: &dnf::Updates, cx: &mut Cx<OsUpdates>) {
        if keys.is_empty() {
            return;
        }
        let names: Vec<String> = keys.iter().filter_map(|k| updates.updates.iter().find(|u| u.new.key() == *k)).map(|u| u.new.name.clone()).collect();
        let what = match (op, names.as_slice()) {
            (Op::Reinstall, [one]) => format!("Reinstall {one}"),
            (Op::Reinstall, _) => format!("Reinstall {}", words::count(keys.len(), "package", "packages")),
            (_, [one]) => format!("Update {one}"),
            _ => format!("Update {}", words::count(keys.len(), "package", "packages")),
        };
        review::open(Plan { title: what.clone(), ops: vec![(op, keys)], release: None, offline_only: false, comment: what }, cx.messenger());
    }
}

/// A row's text in column `c`.
fn cell_text(u: &Update, a: &HashMap<String, Advisory>, c: usize) -> String {
    match c {
        1 => u.new.name.clone(),
        2 => u.installed.evr.clone(),
        3 => u.new.evr.clone(),
        4 => {
            let s = severity_of(u, a);
            let k = words::kind(u.kind(a));
            if s.is_empty() { k.to_owned() } else { format!("{k} · {s}") }
        }
        5 => u.new.repo_id.clone(),
        6 => dnf::size_words(u.new.download_size),
        _ => String::new(),
    }
}

pub fn message(app: &mut OsUpdates, msg: Msg, cx: &mut Cx<OsUpdates>) {
    match msg {
        Msg::Searched => {
            let text = app.u.search.read(cx).text(cx).to_owned();
            app.u.query = Query::parse(&text, KEYS);
            app.u.refilter(&app.updates);
            app.u.select_again(&app.updates, cx);
        }
        Msg::Sorted => {
            app.u.refilter(&app.updates);
            app.u.select_again(&app.updates, cx);
        }
        Msg::Selected => {
            let keys = app.u.selected_keys(&app.updates);
            if keys.len() == 1 {
                app.u.selected = keys.into_iter().next();
                details_for_selected(app, cx);
            } else if keys.is_empty() {
                app.u.selected = None;
            }
        }
        Msg::Toggle(row) => {
            let Some(u) = app.u.rows.get(row).and_then(|i| app.updates.updates.get(*i)).cloned() else { return };
            let kind = app.u.kind(&u, &app.updates);
            let on = !app.u.chosen.is(&u.new.key(), kind);
            // A check box clicked on a selected row checks every selected row.
            let rows: Vec<usize> = if app.u.list.state().selected.contains(&row) { app.u.list.state().selected.iter().copied().collect() } else { vec![row] };
            for r in rows {
                if let Some(u) = app.u.rows.get(r).and_then(|i| app.updates.updates.get(*i)) {
                    let kind = KINDS[kind_of(u, &app.updates.advisories)];
                    app.u.chosen.set(&u.new.key(), kind, on);
                }
            }
            if !app.u.query.values("chosen").is_empty() {
                app.u.refilter(&app.updates);
            }
        }
        Msg::Details(nevra, r) => {
            app.u.details.insert(nevra, r);
        }
        Msg::Act => {
            let keys: Vec<String> = app.updates.updates.iter().filter(|u| app.u.is_chosen(u, &app.updates)).map(|u| u.new.key()).collect();
            app.u.review(Op::Upgrade, keys, &app.updates, cx);
        }
        Msg::Choose(c) => app.u.choose(&app.updates, c),
        Msg::Only(op, keys) => app.u.review(op, keys, &app.updates, cx),
    }
}

/// Fetch the selected update's details, if they aren't here yet.
pub fn details_for_selected(app: &mut OsUpdates, cx: &mut Cx<OsUpdates>) {
    let Some(key) = &app.u.selected else { return };
    let Some(u) = app.updates.updates.iter().find(|u| u.new.key() == *key) else { return };
    let nevra = u.new.nevra.clone();
    if app.u.details.contains_key(&nevra) {
        return;
    }
    let Some(session) = app.session.clone() else { return };
    app.u.details.insert(nevra.clone(), Err(String::new()));
    let n = nevra.clone();
    cx.work(move || session.details(&n, "upgrades").map(Arc::new).map_err(|e| dnf::error_text(&e)), move |r| AppMsg::Updates(Msg::Details(nevra, r)));
}

pub fn command(app: &mut OsUpdates, command: &str, cx: &mut Cx<OsUpdates>) -> bool {
    match command {
        "update-chosen" => message(app, Msg::Act, cx),
        "update-selected" => {
            let keys = app.u.selected_keys(&app.updates);
            message(app, Msg::Only(Op::Upgrade, keys), cx);
        }
        "reinstall-selected" => {
            let keys = app.u.selected_keys(&app.updates);
            message(app, Msg::Only(Op::Reinstall, keys), cx);
        }
        "find" => {
            app.section = Section::Updates;
            app.u.search.update(cx, |s, cx| s.focus(cx));
        }
        "copy" | "copy-names" => {
            let keys = app.u.selected_keys(&app.updates);
            let names: Vec<String> =
                keys.iter().filter_map(|k| app.updates.updates.iter().find(|u| u.new.key() == *k)).map(|u| format!("{} {} → {}", u.new.name, u.installed.evr, u.new.evr)).collect();
            if names.is_empty() {
                return false;
            }
            olo_kit::clipboard::write(olo_kit::ClipboardItem::new_string(names.join("\n")));
        }
        // An agent's: check or uncheck one update by its name.
        c if c.starts_with("check:") || c.starts_with("uncheck:") => {
            let (on, name) = c.split_once(':').map(|(a, b)| (a == "check", b)).unwrap_or((true, ""));
            let Some(u) = app.updates.updates.iter().find(|u| u.new.name == name || u.new.key() == name) else { return false };
            let kind = KINDS[kind_of(u, &app.updates.advisories)];
            app.u.chosen.set(&u.new.key(), kind, on);
        }
        _ => return false,
    }
    cx.invalidate();
    true
}

/// The section.
pub fn body(app: &mut OsUpdates, cx: &mut Cx<OsUpdates>) -> Element {
    let p = theme::palette();
    let updates = app.updates.clone();
    let facets = app.u.facets(&updates, cx);
    let width = search::header_width(cx, "updates").unwrap_or(800.0);
    let shown = app.u.rows.len();
    let total = updates.updates.len();
    let header = search::header(cx, &app.u.search, "updates", facets, width).min_field(240.0).indent(search::list_indent(cx)).count(if app.session.is_some() {
        format!("{shown} of {total}")
    } else {
        String::new()
    });

    let list: Element = if app.session.is_none() {
        let (title, detail) = match (&app.error, &app.loading) {
            (Some(e), _) => ("The packages can’t be read".to_owned(), e.clone()),
            (None, Some(l)) => (l.what.clone(), "The package service reads what’s installed and what each repository has.".to_owned()),
            _ => ("Nothing read yet".to_owned(), String::new()),
        };
        crate::well(cx, p.document_background, olo_kit::widgets::EmptyState::new(title).icon("software-update").detail(detail))
    } else if total == 0 {
        crate::well(
            cx,
            p.document_background,
            olo_kit::widgets::EmptyState::new("Everything is up to date")
                .icon("software-update")
                .detail(format!("Checked {}. Check again with File ▸ Check for Updates.", app.fresh_at.or(app.read_at).map(words::ago).unwrap_or_default())),
        )
    } else {
        list_view(app, cx)
    };

    let details = details_view(app, cx);
    let counts = app.u.counts(&updates);
    let busy = app.loading.is_some();
    let foot = div().flex().items_center().gap(px(m::SPACING)).child(div().flex_1().min_w_0().child(app.status(cx))).child(chosen::bar(
        "updates",
        &counts,
        !busy && app.session.is_some(),
        cx.listener(|v: &mut OsUpdates, c: &chosen::Choice, cx| {
            v.u.choose(&v.updates, *c);
            cx.invalidate();
        }),
        cx.listener(|v: &mut OsUpdates, _: &ClickEvent, cx| message(v, Msg::Act, cx)),
    ));
    let mut col = div().size_full().flex().flex_col().gap(px(m::SPACING)).child(header);
    if let Some(b) = crate::release::waiting_banner(app, cx) {
        col = col.child(b);
    }
    col.child(div().flex_1().min_h_0().flex().flex_row().gap(px(m::SPACING)).child(div().flex_1().min_w_0().h_full().child(list)).child(div().w(px(DETAILS)).flex_shrink_0().h_full().child(details)))
        .child(foot)
        .into_element()
}

fn list_view(app: &mut OsUpdates, cx: &mut Cx<OsUpdates>) -> Element {
    let columns: Rc<[Column]> = Rc::from(vec![
        Column::new("", 24.0),
        Column::new("Package", 136.0),
        Column::new("Installed", 104.0),
        Column::new("Update", 104.0),
        Column::new("Kind", 136.0),
        Column::new("Repository", 84.0),
        Column::new("Size", 58.0).right(),
    ]);
    let rows = app.u.rows.clone();
    let checks = app.u.checks.clone();
    let (cell_rows, key_rows, name_rows, menu_rows) = (rows.clone(), rows.clone(), rows.clone(), rows.clone());
    let press_checks = checks.clone();
    ColumnListView::new(cx, "updates-list", &app.u.list, columns, rows.len(), move |v: &OsUpdates, cell, g| {
        let Some(u) = cell_rows.get(cell.row).and_then(|i| v.updates.updates.get(*i)) else { return };
        let a = &v.updates.advisories;
        if cell.column == 0 {
            let p = theme::palette();
            let s = px(m::MARK_SIZE);
            let c = cell.bounds.center();
            let well = style::snap(g, Bounds::new(Point::new(c.x - s / 2.0, c.y - s / 2.0), olo_kit::size(s, s)));
            let on = v.u.is_chosen(u, &v.updates);
            check_box_look(g, well, &p, on);
            checks.borrow_mut().insert(cell.row, cell.bounds);
            agent::record(g, agent::Node { role: "CheckBox", name: Some(u.new.name.clone().into()), bounds: well, enabled: true, clickable: true, checked: Some(on), ..Default::default() });
            return;
        }
        let text = cell_text(u, a, cell.column);
        if cell.column == 4 && u.kind(a) == "security" && !cell.selected {
            let p = theme::palette();
            let sev = words::severity_rank(severity_of(u, a));
            let color = if sev <= 1 { style::c(p.failure) } else { cell.text_color };
            cell.text_with(g, text, style::ui_font(FontWeight::NORMAL), color);
        } else {
            cell.text(g, text);
        }
    })
    .row_key(move |v: &OsUpdates, row| {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        if let Some(u) = key_rows.get(row).and_then(|i| v.updates.updates.get(*i)) {
            u.new.nevra.hash(&mut h);
            u.installed.evr.hash(&mut h);
            v.u.is_chosen(u, &v.updates).hash(&mut h);
        }
        h.finish()
    })
    .type_select(move |v: &OsUpdates, row| name_rows.get(row).and_then(|i| v.updates.updates.get(*i)).map(|u| u.new.name.clone().into()).unwrap_or_default())
    .empty_text("No update matches the search.")
    .on_select(cx.listener(|v: &mut OsUpdates, _: &(), cx| {
        message(v, Msg::Selected, cx);
        cx.invalidate();
    }))
    .on_sort(cx.listener(|v: &mut OsUpdates, _: &(), cx| {
        message(v, Msg::Sorted, cx);
        cx.invalidate();
    }))
    .on_press(cx.asks(move |v: &mut OsUpdates, &(row, at): &(usize, Point<Pixels>), cx| {
        if !press_checks.borrow().get(&row).is_some_and(|b| b.contains(&at)) {
            return false;
        }
        message(v, Msg::Toggle(row), cx);
        cx.invalidate();
        true
    }))
    .on_open(cx.listener(|v: &mut OsUpdates, row: &usize, cx| {
        message(v, Msg::Toggle(*row), cx);
        cx.invalidate();
    }))
    .on_context(cx.listener(move |v: &mut OsUpdates, &(row, at): &(usize, Point<Pixels>), cx| {
        if !v.u.list.state().selected.contains(&row) {
            v.u.list.state_mut().selected.clear();
            v.u.list.state_mut().select(row);
            message(v, Msg::Selected, cx);
        }
        let keys = v.u.selected_keys(&v.updates);
        let Some(first) = menu_rows.get(row).and_then(|i| v.updates.updates.get(*i)).map(|u| u.new.name.clone()) else { return };
        let n = keys.len();
        let label = |verb: &str| if n == 1 { format!("{verb} {first}…") } else { format!("{verb} {} Packages…", n) };
        let m = olo_kit::Menu::new("Update")
            .item(label("Update Only"), "update-selected")
            .item(label("Reinstall"), "reinstall-selected")
            .separator()
            .item(if n == 1 { "Choose".to_owned() } else { format!("Choose These {n}") }, "choose-selected")
            .item(if n == 1 { "Don’t Choose".to_owned() } else { format!("Choose None of These {n}") }, "unchoose-selected")
            .separator()
            .item("Copy Name", "copy-names");
        let to = cx.view_ref();
        cx.invalidate();
        menu::context_menu(cx, m, at, move |command, cx| {
            let command = command.to_owned();
            to.update(cx, move |v: &mut OsUpdates, cx| {
                let keys = v.u.selected_keys(&v.updates);
                match command.as_str() {
                    "choose-selected" | "unchoose-selected" => {
                        let on = command == "choose-selected";
                        for k in &keys {
                            if let Some(u) = v.updates.updates.iter().find(|u| u.new.key() == *k) {
                                let kind = KINDS[kind_of(u, &v.updates.advisories)];
                                v.u.chosen.set(k, kind, on);
                            }
                        }
                    }
                    "copy-names" => {
                        let names: Vec<String> = keys.iter().filter_map(|k| v.updates.updates.iter().find(|u| u.new.key() == *k)).map(|u| u.new.name.clone()).collect();
                        olo_kit::clipboard::write(olo_kit::ClipboardItem::new_string(names.join("\n")));
                    }
                    other => {
                        v.run(other, cx);
                    }
                }
                cx.invalidate();
            });
        });
    }))
    .into_element()
}

/// One labelled line of the details: the label, then what it says.
pub fn field(label: &str, value: impl olo_kit::IntoElement) -> Element {
    div()
        .flex()
        .flex_row()
        .gap(px(m::TIGHT))
        .child(div().w(px(LABELS)).flex_shrink_0().child(StringView::new(label.to_owned()).dim(true).wrap(true)))
        .child(div().flex_1().min_w_0().child(value))
        .into_element()
}

/// Text that wraps, as a value.
pub fn text(s: impl Into<String>) -> Element {
    StringView::new(s.into()).wrap(true).into_element()
}

/// A link that opens in the browser.
pub fn link(label: impl Into<String>, url: String) -> Element {
    LinkButton::new(label.into())
        .on_click(move |_, _| {
            let url = url.clone();
            let _ = std::thread::Builder::new().name("open link".into()).spawn(move || {
                if let Err(e) = olo_xdg::open::open(&olo_xdg::open::Target::Url(url.clone())) {
                    tracing::warn!("open {url}: {e}");
                }
            });
        })
        .into_element()
}

/// A heading in the details.
pub fn heading(s: &str) -> Element {
    div().pt(px(m::SPACING)).child(StringView::new(s.to_owned()).bold(true).wrap(true)).into_element()
}

/// The selected update's details.
fn details_view(app: &mut OsUpdates, cx: &mut Cx<OsUpdates>) -> Element {
    let p = theme::palette();
    let n = app.u.list.state().selected.len();
    let u = app.u.selected.as_ref().and_then(|k| app.updates.updates.iter().find(|u| u.new.key() == *k)).cloned();
    let content: Element = match (u, n) {
        (_, 2..) => {
            let keys = app.u.selected_keys(&app.updates);
            let (dl, inst): (u64, i64) = keys
                .iter()
                .filter_map(|k| app.updates.updates.iter().find(|u| u.new.key() == *k))
                .fold((0, 0), |(d, i), u| (d + u.new.download_size, i + u.new.install_size as i64 - u.installed.install_size as i64));
            div()
                .flex()
                .flex_col()
                .gap(px(m::TIGHT))
                .child(StringView::new(format!("{n} updates selected")).bold(true))
                .child(field("Download", text(dnf::size_words(dl))))
                .child(field("Disk space", text(signed_size(inst))))
                .child(div().pt(px(m::SPACING)).flex().gap(px(m::SPACING)).child(Button::new(format!("Update These {n}…")).on_click(cx.listener(|v: &mut OsUpdates, _: &ClickEvent, cx| {
                    let keys = v.u.selected_keys(&v.updates);
                    message(v, Msg::Only(Op::Upgrade, keys), cx);
                }))))
                .into_element()
        }
        (Some(u), _) => update_details(app, &u, cx),
        (None, _) => StringView::new(if app.updates.updates.is_empty() { String::new() } else { "Select an update to read about it: its versions, why it came out, what changed.".to_owned() })
            .dim(true)
            .wrap(true)
            .into_element(),
    };
    let inner = div().w_full().p(px(m::SPACING)).child(content);
    crate::well(cx, p.document_background, ScrollView::new(&app.u.scroll).border(false).auto_hide(true).background(p.document_background).child(inner))
}

fn signed_size(bytes: i64) -> String {
    if bytes >= 0 { format!("{} more", dnf::size_words(bytes as u64)) } else { format!("{} less", dnf::size_words((-bytes) as u64)) }
}

fn update_details(app: &mut OsUpdates, u: &Update, cx: &mut Cx<OsUpdates>) -> Element {
    let a = &app.updates.advisories;
    let mut col = div().flex().flex_col().gap(px(m::TIGHT));
    col = col.child(StringView::new(u.new.name.clone()).bold(true).size(style::TITLE_SIZE).height(style::line_row(style::TITLE_SIZE)));
    if !u.new.summary.is_empty() {
        col = col.child(text(u.new.summary.clone()));
    }
    let mut tags = div().flex().flex_row().gap(px(m::TIGHT)).pt(px(m::TIGHT));
    tags = tags.child(Tag::new(words::kind(u.kind(a))));
    let sev = severity_of(u, a);
    if !sev.is_empty() {
        tags = tags.child(Tag::new(sev.to_owned()));
    }
    col = col.child(tags);
    col = col.child(heading("Versions"));
    col = col.child(field("Installed", text(if u.installed.evr.is_empty() { "—".to_owned() } else { format!("{} ({})", u.installed.evr, words::date(u.installed.install_time)) })));
    col = col.child(field("Update", text(format!("{} (built {})", u.new.evr, words::date(u.new.buildtime)))));
    col = col.child(field("Architecture", text(u.new.arch.clone())));
    col = col.child(field("Repository", text(u.new.repo_id.clone())));
    col = col.child(field("Download", text(dnf::size_words(u.new.download_size))));
    col = col.child(field("Disk space", text(signed_size(u.new.install_size as i64 - u.installed.install_size as i64))));
    if !u.installed.reason.is_empty() {
        col = col.child(field(
            "Installed as",
            text(match u.installed.reason.as_str() {
                "User" => "Chosen by someone".to_owned(),
                "Dependency" => "Needed by another package".to_owned(),
                "Weak Dependency" => "Recommended by another package".to_owned(),
                other => other.to_owned(),
            }),
        ));
    }
    if !u.new.license.is_empty() {
        col = col.child(field("Licence", text(u.new.license.clone())));
    }
    if !u.new.url.is_empty() {
        col = col.child(field("Home page", link(u.new.url.clone(), u.new.url.clone())));
    }
    col = col.child(div().pt(px(m::SPACING)).flex().gap(px(m::SPACING)).child(Button::new("Update Only This…").id("update-only").on_click(cx.listener(|v: &mut OsUpdates, _: &ClickEvent, cx| {
        let keys = v.u.selected.clone().into_iter().collect();
        message(v, Msg::Only(Op::Upgrade, keys), cx);
    }))));
    // Its advisories.
    for id in &u.advisories {
        let Some(adv) = a.get(id) else { continue };
        col = col.child(heading(&format!("Advisory: {}", adv.title)));
        let url = format!("https://bodhi.fedoraproject.org/updates/{}", adv.id);
        col = col.child(field("Advisory", if adv.id.starts_with("FEDORA") { link(adv.id.clone(), url) } else { text(adv.id.clone()) }));
        let kind = match words::severity(&adv.severity) {
            "" => words::kind(&adv.kind).to_owned(),
            s => format!("{}, {s}", words::kind(&adv.kind)),
        };
        col = col.child(field("Kind", text(kind)));
        if adv.issued > 0 {
            col = col.child(field("Issued", text(words::date(adv.issued))));
        }
        let mut refs = div().flex().flex_col();
        let mut any = false;
        for r in &adv.references {
            let label = match r.kind.as_str() {
                "bugzilla" => format!("Bug {}", r.id),
                _ => r.id.clone(),
            };
            refs = refs.child(if r.url.is_empty() { text(label) } else { link(label, r.url.clone()) });
            any = true;
        }
        if any {
            col = col.child(field("Fixes", refs));
        }
        if !adv.description.is_empty() {
            col = col.child(div().pt(px(m::TIGHT)).child(text(adv.description.trim().to_owned())));
        }
    }
    // Its changelog, newer than what's installed first.
    col = col.child(heading("What Changed"));
    match app.u.details.get(&u.new.nevra) {
        None | Some(Err(_)) if app.u.details.get(&u.new.nevra).is_none_or(|r| r.as_ref().err().is_some_and(|e| e.is_empty())) => {
            col = col.child(StringView::new("Reading the changelog…").dim(true));
        }
        Some(Err(e)) => col = col.child(StringView::new(e.clone()).dim(true).wrap(true)),
        Some(Ok(d)) => {
            let since = u.installed.buildtime;
            let new: Vec<&dnf::Change> = d.changelog.iter().filter(|c| since == 0 || c.when > since - 86_400).collect();
            let shown: Vec<&dnf::Change> = if new.is_empty() { d.changelog.iter().take(3).collect() } else { new.into_iter().take(20).collect() };
            if shown.is_empty() {
                col = col.child(StringView::new("Its packager wrote nothing.").dim(true));
            }
            for c in shown {
                col = col.child(div().pt(px(m::TIGHT)).child(StringView::new(format!("{} · {}", words::date(c.when), c.author)).dim(true).small(true).wrap(true)));
                col = col.child(text(c.text.trim().to_owned()));
            }
            if !d.requires.is_empty() {
                col = col.child(heading("Needs"));
                let n = d.requires.len();
                let mut list: Vec<String> = d.requires.iter().filter(|r| !r.starts_with("rpmlib(")).take(24).cloned().collect();
                if n > 24 {
                    list.push(format!("and {} more", n - 24));
                }
                col = col.child(text(list.join("\n")));
            }
        }
        _ => {}
    }
    col.into_element()
}
