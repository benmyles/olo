//! Every process: a list (or a tree of who started whom), searched and
//! filtered with the kit's search (`user:`, `state:`, `kind:`, `pid:`),
//! sorted by any column; inspect one, quit it, send it any signal or
//! change its priority. Rows are sorted only when the list or the query
//! changes, not each frame, and only the rows on screen are drawn.

use olo_kit::{
    Cx, Menu,
    egui::{self, Align, Id, Rect, Shape, Stroke, Ui, pos2, vec2},
    files::format_size,
    metrics as m,
    search::{Facet, FilterBar, Query, SearchField},
    style::{self, c},
    widgets::{self, Button, Cell, CheckBox, Column, ColumnListView},
};

use crate::{
    Access, Activity,
    actions::SIGNALS,
    sample::{Proc, Unit},
};

/// What the sorted rows were made from: when it changes, they are made
/// again.
#[derive(Debug, Clone, PartialEq)]
pub struct RowsKey {
    sort: Option<(usize, bool)>,
    search: String,
    tree: bool,
    collapsed: usize,
}

/// A row: which process, how deep in the tree, whether it has children.
#[derive(Debug, Clone, Copy)]
pub struct RowRef {
    pub index: usize,
    pub depth: u16,
    pub children: bool,
}

const INDENT: f32 = 14.0;
const TRIANGLE: f32 = 12.0;

/// What a process is doing, from /proc's state letter.
pub fn state_name(state: char) -> &'static str {
    match state {
        'R' => "Running",
        'S' => "Sleeping",
        'D' => "Waiting",
        'T' | 't' => "Stopped",
        'Z' => "Zombie",
        'I' => "Idle",
        _ => "Other",
    }
}

fn kernel(p: &Proc) -> bool {
    p.pid == 2 || p.ppid == 2
}

const KEYS: &[&str] = &["user", "state", "kind", "pid"];

fn facets(app: &Activity) -> Vec<Facet> {
    let mut users: std::collections::BTreeMap<&str, usize> = Default::default();
    for p in &app.processes {
        *users.entry(p.user.as_str()).or_default() += 1;
    }
    let mut user = Facet::new("user", "User").any("Everyone").multiple();
    for (u, n) in users {
        user = user.value(u, u, Some(n));
    }
    let mut state = Facet::new("state", "State").any("Any State").multiple();
    for s in ["Running", "Sleeping", "Waiting", "Stopped", "Zombie", "Idle"] {
        let n = app.processes.iter().filter(|p| state_name(p.state) == s).count();
        state = state.value(s.to_lowercase(), s, Some(n));
    }
    let k = app.processes.iter().filter(|p| kernel(p)).count();
    let kind = Facet::new("kind", "Kind").any("All Kinds").value("program", "Programs", Some(app.processes.len() - k)).value("kernel", "Kernel Threads", Some(k));
    vec![user, state, kind, Facet::new("pid", "Process").typed_only()]
}

fn matches(q: &Query, p: &Proc) -> bool {
    q.matches(|key, n| match key {
        None => n.in_text(&p.name) || n.in_text(&p.command) || n.is(&p.pid.to_string()),
        Some("user") => n.is(&p.user),
        Some("state") => n.is(state_name(p.state)),
        Some("kind") => n.is(if kernel(p) { "kernel" } else { "program" }),
        Some("pid") => n.is(&p.pid.to_string()) || n.is(&p.ppid.to_string()),
        _ => false,
    })
}

fn compare(col: usize, a: &Proc, b: &Proc) -> std::cmp::Ordering {
    let o = match col {
        0 => olo_kit::files::natural_cmp(&a.name, &b.name),
        1 => a.pid.cmp(&b.pid),
        2 => a.user.cmp(&b.user),
        3 => a.cpu.total_cmp(&b.cpu),
        4 => a.memory.cmp(&b.memory),
        5 => a.threads.cmp(&b.threads),
        6 => a.read.unwrap_or(-1.0).total_cmp(&b.read.unwrap_or(-1.0)),
        7 => a.written.unwrap_or(-1.0).total_cmp(&b.written.unwrap_or(-1.0)),
        8 => a.nice.cmp(&b.nice),
        9 => state_name(a.state).cmp(state_name(b.state)),
        10 => a.cpu_time.total_cmp(&b.cpu_time),
        _ => a.started.total_cmp(&b.started),
    };
    o.then(a.pid.cmp(&b.pid))
}

/// The rows to show, sorted (and as a tree: each process under its
/// parent, siblings sorted; a search keeps the matches' ancestors).
fn make_rows(app: &Activity) -> Vec<RowRef> {
    let q = Query::parse(&app.settings.search, KEYS);
    let procs = &app.processes;
    let (col, asc) = app.settings.sort.unwrap_or((3, false));
    let cmp = |a: &usize, b: &usize| {
        let o = compare(col, &procs[*a], &procs[*b]);
        if asc { o } else { o.reverse() }
    };
    let shown: Vec<bool> = procs.iter().map(|p| q.is_empty() || matches(&q, p)).collect();
    if !app.settings.tree {
        let mut v: Vec<usize> = (0..procs.len()).filter(|&i| shown[i]).collect();
        v.sort_by(cmp);
        return v.into_iter().map(|index| RowRef { index, depth: 0, children: false }).collect();
    }
    let by_pid: std::collections::HashMap<i32, usize> = procs.iter().enumerate().map(|(i, p)| (p.pid, i)).collect();
    // A match keeps its ancestors, to show where it is.
    let mut keep = shown.clone();
    for (i, p) in procs.iter().enumerate() {
        if shown[i] {
            let mut pp = p.ppid;
            while let Some(&j) = by_pid.get(&pp) {
                if keep[j] {
                    break;
                }
                keep[j] = true;
                pp = procs[j].ppid;
            }
        }
    }
    let mut children: std::collections::HashMap<i32, Vec<usize>> = Default::default();
    let mut roots = Vec::new();
    for (i, p) in procs.iter().enumerate().filter(|(i, _)| keep[*i]) {
        if p.ppid != p.pid && by_pid.get(&p.ppid).is_some_and(|&j| keep[j]) {
            children.entry(p.ppid).or_default().push(i);
        } else {
            roots.push(i);
        }
    }
    roots.sort_by(cmp);
    for v in children.values_mut() {
        v.sort_by(cmp);
    }
    let mut out = Vec::with_capacity(procs.len());
    let mut stack: Vec<(usize, u16)> = roots.into_iter().rev().map(|i| (i, 0)).collect();
    while let Some((i, depth)) = stack.pop() {
        let kids = children.get(&procs[i].pid);
        out.push(RowRef { index: i, depth, children: kids.is_some_and(|k| !k.is_empty()) });
        if let Some(kids) = kids
            && !app.collapsed.contains(&procs[i].pid)
        {
            for &k in kids.iter().rev() {
                stack.push((k, depth + 1));
            }
        }
    }
    out
}

/// "1:02:03" of CPU time.
fn cpu_time(s: f64) -> String {
    let t = s.max(0.0);
    let (h, mi, sec) = ((t / 3600.0) as u64, (t % 3600.0 / 60.0) as u64, t % 60.0);
    if h > 0 { format!("{h}:{mi:02}:{:02.0}", sec.floor()) } else { format!("{mi}:{sec:04.1}") }
}

/// When a process started (since boot), as a time today or a date.
fn started(app: &Activity, since_boot: f64) -> String {
    let secs = app.system.boot as f64 + since_boot;
    let t = secs as libc::time_t;
    // SAFETY: localtime_r fills the zeroed tm we own (twice).
    let (tm, today) = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        let now = libc::time(std::ptr::null_mut());
        let mut tn: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tn);
        (tm, tn.tm_yday == tm.tm_yday && tn.tm_year == tm.tm_year)
    };
    let h = tm.tm_hour % 12;
    let time = format!("{}:{:02} {}", if h == 0 { 12 } else { h }, tm.tm_min, if tm.tm_hour < 12 { "AM" } else { "PM" });
    if today {
        time
    } else {
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        format!("{} {}, {time}", MONTHS[tm.tm_mon as usize % 12], tm.tm_mday)
    }
}

/// The Send Signal submenu for `pid`.
pub fn signal_menu(pid: i32) -> Menu {
    let mut menu = Menu::new("Send Signal");
    for (n, name, what) in SIGNALS {
        menu = menu.item(format!("{what} ({name})"), format!("signal:{pid}:{n}"), None);
    }
    menu
}

/// The priorities offered, as nice values.
pub const PRIORITIES: [(i32, &str); 7] = [(-20, "Highest"), (-10, "High"), (-5, "Above Normal"), (0, "Normal"), (5, "Below Normal"), (10, "Low"), (19, "Lowest")];

pub fn priority_menu(pid: i32, nice: i32) -> Menu {
    let mut menu = Menu::new("Priority");
    for (n, name) in PRIORITIES {
        menu = menu.item(format!("{name} ({n})"), format!("nice:{pid}:{n}"), None).checked(nice == n);
    }
    menu
}

fn row_menu(app: &Activity, p: &Proc) -> Menu {
    let pid = p.pid;
    let mut menu = Menu::new("")
        .item("Inspect", format!("inspect:{pid}"), None)
        .separator()
        .item("Quit", format!("quit:{pid}"), None)
        .item("Force Quit…", format!("force-quit:{pid}"), None)
        .separator()
        .submenu(signal_menu(pid))
        .submenu(priority_menu(pid, p.nice))
        .separator()
        .item("Show Program in Tracker", format!("show-program:{pid}"), None)
        .item("Copy Process ID", format!("copy-pid:{pid}"), None)
        .item("Copy Command Line", format!("copy-command:{pid}"), None);
    if app.access != Access::Full && p.uid != app.uid {
        menu = menu.separator().item("Get Full Access…", "full-access", None).enabled(app.access == Access::Mine);
    }
    menu
}

pub fn show(app: &mut Activity, cx: &mut Cx, ui: &mut Ui, area: Rect) {
    let p = style::palette(ui.ctx());
    ui.painter().rect_filled(area, 0.0, c(p.panel_background));
    let inner = area.shrink(m::WINDOW_INSET - 2.0);
    // The search, its filters and Tree, over the list.
    let facets = facets(app);
    let top = Rect::from_min_size(inner.min, vec2(inner.width(), m::CONTROL_HEIGHT));
    let tree_w = 64.0;
    let bar_w = FilterBar::width(ui, &app.settings.search, &facets) + m::SPACING + 40.0;
    let one_row = top.width() - bar_w - tree_w - 2.0 * m::SPACING >= 220.0;
    let field_w = if one_row { top.width() - bar_w - tree_w - 2.0 * m::SPACING } else { top.width() - tree_w - m::SPACING };
    let field = Rect::from_min_size(top.min, vec2(field_w, m::CONTROL_HEIGHT));
    if std::mem::take(&mut app.focus_search) {
        SearchField::focus(ui.ctx(), "activity-processes-search");
    }
    let mut changed = ui.put(field, SearchField::new("activity-processes-search", &mut app.settings.search).hint("Search: name, command, user:, state:, kind:").facets(&facets)).changed();
    let bar = if one_row {
        Rect::from_min_max(pos2(field.right() + m::SPACING, top.top()), pos2(top.right() - tree_w - m::SPACING, top.bottom()))
    } else {
        // Under the search, its first menu's text over the list's first
        // column's (inside the list's two lines of frame), as Logs has it.
        let frame = style::inset(ui.ctx(), top, 2.0).left() - top.left();
        let indent = frame + m::CELL_INSET - m::TEXT_INSET;
        Rect::from_min_size(pos2(top.left() + indent, top.bottom() + m::SPACING), vec2(top.width() - indent, m::CONTROL_HEIGHT))
    };
    // (The search field clears it all.)
    changed |= ui.put(bar, FilterBar::new("activity-processes", &mut app.settings.search, &facets).no_clear()).changed();
    let tree_r = Rect::from_min_size(pos2(top.right() - tree_w, top.top() + (m::CONTROL_HEIGHT - m::MARK_ROW) / 2.0), vec2(tree_w, m::MARK_ROW));
    let mut tree = app.settings.tree;
    if ui.put(tree_r, CheckBox::new(&mut tree, "Tree")).changed() {
        app.settings.tree = tree;
        changed = true;
    }
    if changed {
        app.rows_cache = None;
        app.store();
    }
    let controls_bottom = if one_row { top.bottom() } else { bar.bottom() };

    let bottom = Rect::from_min_max(pos2(inner.left(), inner.bottom() - m::CONTROL_HEIGHT), inner.max);
    let list_r = Rect::from_min_max(pos2(inner.left(), controls_bottom + m::SPACING), pos2(inner.right(), bottom.top() - m::SPACING));
    let key = RowsKey { sort: app.settings.sort, search: app.settings.search.clone(), tree: app.settings.tree, collapsed: app.collapsed.len() };
    let fresh = app.rows_cache.as_ref().is_some_and(|(g, k, _)| *g == app.generation && *k == key);
    if !fresh {
        let rows = make_rows(app);
        app.rows_cache = Some((app.generation, key, rows));
    }
    let rows = app.rows_cache.as_ref().map(|(_, _, r)| r.clone()).unwrap_or_default();
    let columns = [
        Column::new("Name", 200.0),
        Column::new("ID", 60.0).right(),
        Column::new("User", 72.0),
        Column::new("% CPU", 72.0).right(),
        Column::new("Memory", 76.0).right(),
        Column::new("Threads", 62.0).right(),
        Column::new("Read", 76.0).right(),
        Column::new("Written", 76.0).right(),
        Column::new("Nice", 44.0).right(),
        Column::new("State", 72.0),
        Column::new("CPU Time", 76.0).right(),
        Column::new("Started", 120.0),
    ];
    let pids: Vec<i32> = rows.iter().map(|r| app.processes[r.index].pid).collect();
    // The selection follows its process as rows move, and goes when it
    // leaves the list (filtered out or quit), so Quit never acts on a
    // process you can't see.
    if app.selected.is_some_and(|pid| !pids.contains(&pid)) && !app.reveal_selected {
        app.selected = None;
    }
    app.list.sort = app.settings.sort;
    let at = app.selected.and_then(|pid| pids.iter().position(|&p| p == pid));
    app.list.selected = at.into_iter().collect();
    if app.list.cursor.is_none_or(|c| Some(c) != at) {
        app.list.cursor = at;
        app.list.anchor = at;
    }
    let reveal = if std::mem::take(&mut app.reveal_selected) { at } else { None };
    // The list's state, out of the app while its cells read the app.
    let mut list = std::mem::take(&mut app.list);
    let procs = &app.processes;
    let tree_mode = app.settings.tree;
    let collapsed = &app.collapsed;
    let names: Vec<&str> = rows.iter().map(|r| procs[r.index].name.as_str()).collect();
    let name_of = |i: usize| names[i];
    let unknown = "—".to_owned();
    let app_ref: &Activity = app;
    let out = ui
        .scope_builder(egui::UiBuilder::new().max_rect(list_r), |ui| {
            ColumnListView::new("activity-processes", &columns, rows.len(), &mut list)
                .reveal(reveal)
                .empty_text("No processes match.")
                .type_select(&name_of)
                .show(ui, |cell: &Cell| {
                    let r = rows[cell.row];
                    let pr = &procs[r.index];
                    let text = match cell.column {
                        0 => {
                            if !tree_mode {
                                return cell.text(&pr.name);
                            }
                            let x = cell.rect.left() + m::CELL_INSET - 4.0 + r.depth as f32 * INDENT;
                            if r.children {
                                let open = !collapsed.contains(&pr.pid);
                                let mid = pos2(x + TRIANGLE / 2.0, cell.rect.center().y);
                                let pts = if open {
                                    vec![mid + vec2(-4.0, -2.0), mid + vec2(4.0, -2.0), mid + vec2(0.0, 3.0)]
                                } else {
                                    vec![mid + vec2(-2.0, -4.0), mid + vec2(3.0, 0.0), mid + vec2(-2.0, 4.0)]
                                };
                                cell.painter.add(Shape::convex_polygon(pts, cell.text_color, Stroke::NONE));
                            }
                            let rect = Rect::from_min_max(pos2(x + TRIANGLE + 2.0 - m::CELL_INSET, cell.rect.top()), cell.rect.max);
                            let shifted = Cell { painter: cell.painter, rect, row: cell.row, column: cell.column, selected: cell.selected, text_color: cell.text_color, align: cell.align };
                            return shifted.text(&pr.name);
                        }
                        1 => pr.pid.to_string(),
                        2 => pr.user.clone(),
                        3 => format!("{:.1}", pr.cpu),
                        // Kernel threads have no memory of their own.
                        4 if pr.memory == 0 => unknown.clone(),
                        4 => format_size(pr.memory),
                        5 => pr.threads.to_string(),
                        6 => pr.read.map_or_else(|| unknown.clone(), |v| Unit::Rate.format(v)),
                        7 => pr.written.map_or_else(|| unknown.clone(), |v| Unit::Rate.format(v)),
                        8 => pr.nice.to_string(),
                        9 => state_name(pr.state).to_owned(),
                        10 => cpu_time(pr.cpu_time),
                        _ => started(app_ref, pr.started),
                    };
                    cell.text(&text);
                })
        })
        .inner;
    app.list = list;
    handle(app, cx, ui, &out, &rows, &pids, list_r);

    // The totals and the buttons under the list.
    let threads: u64 = app.processes.iter().map(|p| p.threads as u64).sum();
    let summary = format!("{} of {} processes, {threads} threads · {} CPU", rows.len(), app.processes.len(), app.latest("cpu").map(|v| format!("{v:.0}%")).unwrap_or_default());
    let bw = 96.0;
    let force_r = Rect::from_min_size(pos2(bottom.right() - bw, bottom.top()), vec2(bw, m::CONTROL_HEIGHT));
    let quit_r = force_r.translate(vec2(-(bw + m::SPACING), 0.0));
    let inspect_r = quit_r.translate(vec2(-(bw + m::SPACING), 0.0));
    style::label_fit(ui.painter(), bottom.left(), bottom.center().y, Align::Min, &summary, style::plain_font(), c(p.panel_text), inspect_r.left() - bottom.left() - m::SPACING);
    let sel = app.selected_proc().map(|p| p.pid);
    let any = sel.is_some();
    let mut cmd = None;
    if ui.add_enabled_ui(any, |ui| ui.put(inspect_r, Button::new("Inspect").min_width(inspect_r.width()))).inner.clicked() {
        cmd = sel.map(|pid| format!("inspect:{pid}"));
    }
    if ui.add_enabled_ui(any, |ui| ui.put(quit_r, Button::new("Quit").min_width(quit_r.width()))).inner.clicked() {
        cmd = sel.map(|pid| format!("quit:{pid}"));
    }
    if ui.add_enabled_ui(any, |ui| ui.put(force_r, Button::new("Force Quit…").min_width(force_r.width()))).inner.clicked() {
        cmd = sel.map(|pid| format!("force-quit:{pid}"));
    }
    if let Some(cmd) = cmd {
        app.command(cx, None, &cmd);
    }
}

/// Clicks, keys and menus on the list.
fn handle(app: &mut Activity, cx: &mut Cx, ui: &mut Ui, out: &widgets::ListResponse, rows: &[RowRef], pids: &[i32], list_r: Rect) {
    if out.sort_changed {
        app.settings.sort = app.list.sort;
        app.rows_cache = None;
        app.store();
    }
    if out.selection_changed {
        app.selected = app.list.single().and_then(|r| pids.get(r).copied());
    }
    if let Some((row, _)) = out.context {
        app.selected = pids.get(row).copied();
    }
    // A click on a tree row's triangle opens or closes it.
    if app.settings.tree
        && ui.input(|i| i.pointer.primary_clicked())
        && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
        && list_r.contains(pos)
        && let Some(row) = out.row_at(pos)
        && let Some(r) = rows.get(row)
        && r.children
        && let Some((_, rr)) = out.visible.iter().find(|(n, _)| *n == row)
    {
        let x = rr.left() + m::CELL_INSET - 4.0 + r.depth as f32 * INDENT;
        if (x - 2.0..x + TRIANGLE + 2.0).contains(&pos.x) {
            let pid = app.processes[r.index].pid;
            if !app.collapsed.remove(&pid) {
                app.collapsed.insert(pid);
            }
            app.rows_cache = None;
        }
    }
    if let Some(row) = out.activated
        && let Some(&pid) = pids.get(row)
    {
        app.inspect(cx, pid);
    }
    if let Some((row, pos)) = out.context
        && let Some(r) = rows.get(row)
    {
        let menu = row_menu(app, &app.processes[r.index]);
        widgets::popup_menu_at(ui.ctx(), Id::unique("activity-row-menu"), pos, menu);
    }
    if let Some(cmd) = widgets::popup_result(ui.ctx(), Id::unique("activity-row-menu")) {
        app.command(cx, None, &cmd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(cpu_time(75.25), "1:15.2");
        assert_eq!(cpu_time(3725.0), "1:02:05");
        assert_eq!(state_name('D'), "Waiting");
    }
}
