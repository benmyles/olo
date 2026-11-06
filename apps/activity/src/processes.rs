//! Every process: a list (or a tree of who started whom), searched and
//! filtered with the kit's search (`user:`, `state:`, `kind:`, `pid:`),
//! sorted by any column; inspect one, quit it, send it any signal or
//! change its priority. Rows are sorted only when the list or the query
//! changes, not each frame, and only the rows on screen are drawn.

use std::rc::Rc;

use olo_ui::{
    Menu,
    files::format_size,
    gpui::{AnyElement, AnyView, Bounds, Context, FontWeight, IntoElement, ParentElement as _, PathBuilder, Pixels, SharedString, StyleRefinement, Styled as _, Window, canvas, div, point, px},
    menu, metrics as m,
    search::{Facet, FilterBar, Query},
    style::{self, c},
    theme,
    widgets::{Button, CheckBox, Column, ColumnListView},
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

/// The rows shown, made when the list or the query changes: each one's
/// place, process and name (for typing to select).
pub struct Rows {
    pub refs: Vec<RowRef>,
    pub pids: Vec<i32>,
    pub names: Vec<SharedString>,
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

fn facets(app: &Activity) -> Rc<Vec<Facet>> {
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
    Rc::new(vec![user, state, kind, Facet::new("pid", "Process").typed_only()])
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
        0 => olo_ui::files::natural_cmp(&a.name, &b.name),
        1 => a.pid.cmp(&b.pid),
        2 => a.user.cmp(&b.user),
        3 => a.cpu.total_cmp(&b.cpu),
        4 => a.memory.cmp(&b.memory),
        5 => a.threads.cmp(&b.threads),
        6 => a.read.unwrap_or(-1.0).total_cmp(&b.read.unwrap_or(-1.0)),
        7 => a.written.unwrap_or(-1.0).total_cmp(&b.written.unwrap_or(-1.0)),
        8 => a.sent.unwrap_or(-1.0).total_cmp(&b.sent.unwrap_or(-1.0)),
        9 => a.received.unwrap_or(-1.0).total_cmp(&b.received.unwrap_or(-1.0)),
        10 => a.nice.cmp(&b.nice),
        11 => state_name(a.state).cmp(state_name(b.state)),
        12 => a.cpu_time.total_cmp(&b.cpu_time),
        13 => a.started.total_cmp(&b.started),
        // Each GPU's column: those using it above those not.
        n => {
            let g = |pr: &Proc| pr.gpus.get(n - GPU_COLUMNS).copied().flatten().unwrap_or(-1.0);
            g(a).total_cmp(&g(b))
        }
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

/// "12" or "12,40,41", for a command.
pub fn pids_text(pids: &[i32]) -> String {
    pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(",")
}

/// The Send Signal submenu for `pids`.
pub fn signal_menu(pids: &[i32]) -> Menu {
    let mut menu = Menu::new("Send Signal");
    let pids = pids_text(pids);
    for (n, name, what) in SIGNALS {
        menu = menu.item(format!("{what} ({name})"), format!("signal:{pids}:{n}"));
    }
    menu
}

/// The priorities offered, as nice values.
pub const PRIORITIES: [(i32, &str); 7] = [(-20, "Highest"), (-10, "High"), (-5, "Above Normal"), (0, "Normal"), (5, "Below Normal"), (10, "Low"), (19, "Lowest")];

/// The Priority submenu for `pids`, whose nice values are `nices`: the
/// one they all have checked (none if they differ).
pub fn priority_menu(pids: &[i32], nices: &[i32]) -> Menu {
    let mut menu = Menu::new("Priority");
    let all = nices.first().copied().filter(|n| nices.iter().all(|m| m == n));
    let pids = pids_text(pids);
    for (n, name) in PRIORITIES {
        menu = menu.item(format!("{name} ({n})"), format!("nice:{pids}:{n}")).checked(all == Some(n));
    }
    menu
}

/// The menu of a row: for the processes chosen (`pids`, the row's among
/// them), every item acting on them all; Inspect on the first.
fn row_menu(app: &Activity, pids: &[i32]) -> Menu {
    let several = pids.len() > 1;
    let first = pids.first().copied().unwrap_or_default();
    let list = pids_text(pids);
    let programs = if several { "Show Programs in Tracker" } else { "Show Program in Tracker" };
    let mut menu = Menu::new("")
        .item("Inspect", format!("inspect:{first}"))
        .separator()
        .item(if several { format!("Quit {} Processes", pids.len()) } else { "Quit".into() }, format!("quit:{list}"))
        .item(if several { format!("Force Quit {} Processes…", pids.len()) } else { "Force Quit…".into() }, format!("force-quit:{list}"))
        .separator()
        .submenu(signal_menu(pids))
        .submenu(priority_menu(pids, &app.nices(pids)))
        .separator()
        .item(programs, format!("show-program:{list}"))
        .item(if several { "Copy Process IDs" } else { "Copy Process ID" }, format!("copy-pid:{list}"))
        .item(if several { "Copy Command Lines" } else { "Copy Command Line" }, format!("copy-command:{list}"));
    if app.access != Access::Full && pids.iter().any(|&pid| app.proc(pid).is_some_and(|p| p.uid != app.uid)) {
        menu = menu.separator().item("Get Full Access…", "full-access").enabled(app.access == Access::Mine);
    }
    menu
}

/// The filters' menus for the processes now (made again when they change).
fn facets_now(app: &mut Activity, cx: &mut Context<Activity>) -> Rc<Vec<Facet>> {
    if let Some((g, f)) = &app.facets
        && *g == app.generation
    {
        return f.clone();
    }
    let f = facets(app);
    app.facets = Some((app.generation, f.clone()));
    app.search.update(cx, |s, cx| s.set_facets(f.clone(), cx));
    f
}

/// The rows, made again only when the processes, sort, query, tree or
/// what's folded changed.
fn rows_now(app: &mut Activity) -> Rc<Rows> {
    let key = RowsKey { sort: app.settings.sort, search: app.settings.search.clone(), tree: app.settings.tree, collapsed: app.collapsed.len() };
    if let Some((g, k, rows)) = &app.rows_cache
        && *g == app.generation
        && *k == key
    {
        return rows.clone();
    }
    let refs = make_rows(app);
    let pids = refs.iter().map(|r| app.processes[r.index].pid).collect();
    let names = refs.iter().map(|r| SharedString::from(app.processes[r.index].name.clone())).collect();
    let rows = Rc::new(Rows { refs, pids, names });
    app.rows_cache = Some((app.generation, key, rows.clone()));
    rows
}

/// A cell's text, and for a tree's first column how deep it is and
/// whether it has a triangle (open or not).
fn cell_text(app: &Activity, r: RowRef, column: usize) -> (String, Option<(u16, Option<bool>)>) {
    let Some(pr) = app.processes.get(r.index) else { return (String::new(), None) };
    let unknown = || "—".to_owned();
    let text = match column {
        0 if app.settings.tree => return (pr.name.clone(), Some((r.depth, r.children.then(|| !app.collapsed.contains(&pr.pid))))),
        0 => pr.name.clone(),
        1 => pr.pid.to_string(),
        2 => pr.user.clone(),
        3 => format!("{:.1}", pr.cpu),
        // Kernel threads have no memory of their own.
        4 if pr.memory == 0 => unknown(),
        4 => format_size(pr.memory),
        5 => pr.threads.to_string(),
        6 => pr.read.map_or_else(unknown, |v| Unit::Rate.format(v)),
        7 => pr.written.map_or_else(unknown, |v| Unit::Rate.format(v)),
        8 => pr.sent.map_or_else(unknown, |v| Unit::Rate.format(v)),
        9 => pr.received.map_or_else(unknown, |v| Unit::Rate.format(v)),
        10 => pr.nice.to_string(),
        11 => state_name(pr.state).to_owned(),
        12 => cpu_time(pr.cpu_time),
        13 => started(app, pr.started),
        // A GPU's: how much of it this process keeps busy; blank if it
        // doesn't use it.
        n => pr.gpus.get(n - GPU_COLUMNS).copied().flatten().map_or_else(String::new, |v| format!("{v:.1}")),
    };
    (text, None)
}

/// Where the GPUs' columns begin.
const GPU_COLUMNS: usize = 14;

/// A GPU's name for its column: its maker and "GPU" ("Intel GPU").
fn short_gpu_name(name: &str) -> String {
    let maker = name.split_whitespace().next().unwrap_or("GPU");
    if maker.eq_ignore_ascii_case("gpu") { "GPU".into() } else { format!("{maker} GPU") }
}

/// An open triangle points down, a closed one right.
fn triangle(window: &mut Window, mid: olo_ui::gpui::Point<Pixels>, open: bool, color: olo_ui::gpui::Hsla) {
    let pts = if open { [(-4.0, -2.0), (4.0, -2.0), (0.0, 3.0)] } else { [(-2.0, -4.0), (3.0, 0.0), (-2.0, 4.0)] };
    let mut path = PathBuilder::fill();
    path.move_to(mid + point(px(pts[0].0), px(pts[0].1)));
    path.line_to(mid + point(px(pts[1].0), px(pts[1].1)));
    path.line_to(mid + point(px(pts[2].0), px(pts[2].1)));
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// The list of processes, for a cached `Part` of its own.
pub fn list(app: &mut Activity, _: &mut Window, cx: &mut Context<Activity>) -> AnyElement {
    let me = cx.entity();
    let rows = rows_now(app);
    // The selection follows its processes as rows move, and each goes
    // when it leaves the list (filtered out or quit), so an action never
    // reaches a process you can't see chosen.
    if !app.reveal_selected {
        app.selected.retain(|pid| rows.pids.contains(pid));
    }
    let at: std::collections::BTreeSet<usize> = app.selected.iter().filter_map(|pid| rows.pids.iter().position(|p| p == pid)).collect();
    let first = at.iter().next().copied();
    let reveal = std::mem::take(&mut app.reveal_selected);
    let sort = app.settings.sort;
    app.list.update(cx, |l, _| {
        l.state.sort = sort;
        l.state.selected = at.clone();
        // (The keyboard's place and a range's start stay where they were
        // while still chosen: they move with their rows.)
        if l.state.cursor.is_none_or(|c| !at.contains(&c)) {
            l.state.cursor = first;
        }
        if l.state.anchor.is_none_or(|c| !at.contains(&c)) {
            l.state.anchor = first;
        }
        if reveal && let Some(r) = first {
            l.reveal(r);
        }
    });

    let columns = vec![
        Column::new("Name", 200.0),
        Column::new("ID", 64.0).right(),
        Column::new("User", 72.0),
        Column::new("% CPU", 72.0).right(),
        Column::new("Memory", 76.0).right(),
        Column::new("Threads", 62.0).right(),
        Column::new("Read", 76.0).right(),
        Column::new("Written", 76.0).right(),
        Column::new("Sent", 76.0).right(),
        Column::new("Received", 76.0).right(),
        Column::new("Nice", 44.0).right(),
        Column::new("State", 72.0),
        Column::new("CPU Time", 76.0).right(),
        Column::new("Started", 120.0),
    ];
    // A column for each GPU (% of it this process keeps busy), after the
    // rest (so a sort kept from before is the same column), named by the
    // GPU: "Intel GPU", "NVIDIA GPU".
    let mut columns = columns;
    let gpu_names: Vec<String> = app.system.gpus.iter().map(|g| short_gpu_name(&g.name)).collect();
    for (i, name) in gpu_names.iter().enumerate() {
        let taken = gpu_names[..i].contains(name);
        let title = if gpu_names.len() == 1 {
            "% GPU".to_owned()
        } else if taken {
            format!("% {name} {i}")
        } else {
            format!("% {name}")
        };
        columns.push(Column::new(SharedString::from(title), 96.0).right());
    }
    let triangles = app.triangles.clone();
    let (cell_me, cell_rows, cell_triangles) = (me.clone(), rows.clone(), triangles.clone());
    let (key_me, key_rows, columns_n) = (me.clone(), rows.clone(), columns.len());
    let names = rows.clone();
    let (press_me, press_rows) = (me.clone(), rows.clone());
    let (select_me, select_rows) = (me.clone(), rows.clone());
    let (open_me, open_rows) = (me.clone(), rows.clone());
    let (menu_me, menu_rows) = (me.clone(), rows.clone());
    let sort_me = me.clone();
    let list = ColumnListView::new("activity-processes", &app.list, columns, rows.refs.len(), move |cell, window, cx| {
        let Some(&r) = cell_rows.refs.get(cell.row) else { return };
        let (text, tree) = cell_text(cell_me.read(cx), r, cell.column);
        if let Some((depth, open)) = tree {
            let x = cell.bounds.left() + px(m::CELL_INSET - 4.0 + depth as f32 * INDENT);
            if let Some(open) = open {
                let mid = point(x + px(TRIANGLE / 2.0), cell.bounds.center().y);
                triangle(window, mid, open, cell.text_color);
                let hit = Bounds::from_corners(point(x - px(2.0), cell.bounds.top()), point(x + px(TRIANGLE + 2.0), cell.bounds.bottom()));
                cell_triangles.borrow_mut().insert(cell.row, hit);
            } else {
                cell_triangles.borrow_mut().remove(&cell.row);
            }
            cell.bounds = Bounds::from_corners(point(x + px(TRIANGLE + 2.0 - m::CELL_INSET), cell.bounds.top()), cell.bounds.bottom_right());
        }
        cell.text(window, cx, text);
    })
    .empty_text("No processes match.")
    // A row is drawn again only when what it says does (most of a second's
    // samples change a few rows' figures).
    .row_key(move |row, cx| {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        if let Some(&r) = key_rows.refs.get(row) {
            let app = key_me.read(cx);
            for column in 0..columns_n {
                cell_text(app, r, column).hash(&mut h);
            }
        }
        h.finish()
    })
    .type_select(move |i| names.names.get(i).cloned().unwrap_or_default())
    .on_sort(move |_, cx| {
        sort_me.update(cx, |a, cx| {
            a.settings.sort = a.list.read(cx).state.sort;
            a.rows_cache = None;
            a.store();
            cx.notify();
        })
    })
    .on_select(move |_, cx| {
        let pids: Vec<i32> = select_me.read(cx).list.read(cx).state.selected.iter().filter_map(|&r| select_rows.pids.get(r).copied()).collect();
        select_me.update(cx, |a, cx| {
            a.selected = pids;
            cx.notify();
        });
    })
    // A press on a tree row's triangle opens or closes it.
    .on_press(move |row, at, _, cx| {
        let hit = triangles.borrow().get(&row).is_some_and(|b| b.contains(&at));
        if !hit {
            return false;
        }
        let Some(&pid) = press_rows.pids.get(row) else { return false };
        press_me.update(cx, |a, cx| {
            if !a.collapsed.remove(&pid) {
                a.collapsed.insert(pid);
            }
            a.rows_cache = None;
            cx.notify();
        });
        true
    })
    .on_open(move |row, _, cx| {
        if let Some(&pid) = open_rows.pids.get(row) {
            open_me.update(cx, |a, cx| a.inspect(pid, cx));
        }
    })
    .on_context(move |row, at, window, cx| {
        let Some(&pid) = menu_rows.pids.get(row) else { return };
        // On a chosen row: for every one chosen; else that row, chosen.
        let menu = menu_me.update(cx, |a, cx| {
            if !a.selected.contains(&pid) {
                a.selected = vec![pid];
            }
            cx.notify();
            let pids = a.chosen_pids();
            (!pids.is_empty()).then(|| row_menu(a, &pids))
        });
        let Some(menu) = menu else { return };
        let to = menu_me.clone();
        menu::context_menu(window, cx, menu, at, move |cmd, window, cx| to.update(cx, |a, cx| a.command(cmd, Some(window), cx)));
    });

    div().size_full().flex().flex_col().child(list).into_any_element()
}

pub fn view(app: &mut Activity, width: f32, list: AnyView, window: &mut Window, cx: &mut Context<Activity>) -> AnyElement {
    let me = cx.entity();
    let p = theme::palette(cx);
    let inset = m::WINDOW_INSET - 2.0;
    let inner_w = width - 2.0 * inset;

    // The search, its filters and Tree, over the list.
    let facets = facets_now(app, cx);
    let tree_w = 64.0;
    let bar_w = FilterBar::width(cx, &app.settings.search, &facets) + m::SPACING + 40.0;
    let one_row = inner_w - bar_w - tree_w - 2.0 * m::SPACING >= 220.0;
    let filters_me = me.clone();
    // (The search field clears it all.)
    let filters = FilterBar::new("activity-processes", app.settings.search.clone(), facets).no_clear().on_change(move |text, _, cx| {
        filters_me.update(cx, |a, cx| {
            a.search.update(cx, |s, cx| s.set_text(text.clone(), cx));
            a.settings.search = text;
            a.rows_cache = None;
            a.store();
            cx.notify();
        })
    });
    let tree_me = me.clone();
    let tree = div().w(px(tree_w)).flex_shrink_0().child(CheckBox::new("Tree", app.settings.tree).id("activity-tree").height(m::CONTROL_HEIGHT).on_toggle(move |on, _, cx| {
        tree_me.update(cx, |a, cx| {
            a.settings.tree = on;
            a.rows_cache = None;
            a.store();
            cx.notify();
        })
    }));
    let search = div().flex_1().min_w_0().child(app.search.clone());
    let controls = if one_row {
        div().flex().flex_row().items_center().gap(px(m::SPACING)).child(search).child(filters.max_width(bar_w)).child(tree)
    } else {
        // Under the search, its first menu's text over the list's first
        // column's (inside the list's two lines of frame), as Logs has it.
        let frame = f32::from(style::inset(window, Bounds::default(), 2.0).left());
        let indent = frame + m::CELL_INSET - m::TEXT_INSET;
        div()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(div().flex().flex_row().items_center().gap(px(m::SPACING)).child(search).child(tree))
            .child(div().pl(px(indent)).child(filters.max_width(inner_w - indent)))
    };

    let rows = rows_now(app);
    // The totals and the buttons under the list.
    let threads: u64 = app.processes.iter().map(|p| p.threads as u64).sum();
    let summary: SharedString =
        format!("{} of {} processes, {threads} threads · {} CPU", rows.refs.len(), app.processes.len(), app.latest("cpu").map(|v| format!("{v:.0}%")).unwrap_or_default()).into();
    // The buttons act on every process chosen (Inspect on the first).
    let sel = app.chosen_pids();
    let bw = 96.0;
    let button = |label: &'static str, cmd: &'static str| {
        let me = me.clone();
        let pids = pids_text(&sel);
        Button::new(label).min_width(bw).enabled(!sel.is_empty()).on_click(move |window, cx| {
            me.update(cx, |a, cx| a.command(&format!("{cmd}:{pids}"), Some(window), cx));
        })
    };
    let text = style::c(p.panel_text);
    let summary_el = canvas(
        |_, _, _| {},
        move |b, _, window, cx| {
            let r = style::label_fit(window, cx, b.left(), b.center().y, style::Align::Left, summary.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), text, b.size.width);
            olo_ui::agent::record(window, cx, olo_ui::agent::Node::label(summary.clone(), r));
        },
    )
    .flex_1()
    .min_w_0()
    .h_full();
    let foot = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::SPACING))
        .h(px(m::CONTROL_HEIGHT))
        .flex_shrink_0()
        .child(summary_el)
        .child(button("Inspect", "inspect"))
        .child(button("Quit", "quit"))
        .child(button("Force Quit…", "force-quit"));
    div()
        .size_full()
        .flex()
        .flex_col()
        .gap(px(m::SPACING))
        .p(px(inset))
        .bg(c(p.panel_background))
        .child(controls)
        // (Its own part, cached: a caret blinking in the search above
        // doesn't draw the rows again.)
        .child(div().flex_1().min_h_0().child(list.cached(StyleRefinement::default().size_full())))
        .child(foot)
        .into_any_element()
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
