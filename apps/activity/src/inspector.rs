//! A process's inspector (Get Info for a process): what it is and how
//! it is doing, over tabs of its threads, open files, memory, environment
//! and resources (the cores it may use, how the kernel treats it when
//! memory runs out, its limits), with what can be done to it. Its details
//! are read on a worker (or by the helper, with full access) at each
//! sample.

use std::{collections::HashMap, time::Instant};

use olo_kit::{
    Menu,
    dashboard::{self, facts_in},
    egui::{self, Align, Id, Rect, Ui, pos2, vec2},
    files::format_size,
    graph::{Graph, Series},
    metrics as m,
    style::{self, c},
    widgets::{self, Button, Cell, CheckBox, Column, ColumnListView, EmptyState, ListState, MenuField, TabView},
};
use olo_theme::Hue;

use crate::{
    Access, Activity,
    actions::Action,
    details::Details,
    processes::{PRIORITIES, signal_menu, state_name},
    sections::duration,
};

const TABS: [&str; 6] = ["General", "Threads", "Open Files", "Memory", "Environment", "Resources"];
const HEAD: f32 = 64.0;

/// What the inspector asks of the app.
pub enum Ask {
    Command(String),
    Act(Action),
}

pub struct Inspector {
    pub pid: i32,
    pub name: String,
    tab: usize,
    details: Option<Result<Details, String>>,
    /// Each thread's ticks at the last reading, and when.
    before: Option<(Instant, HashMap<i32, u64>)>,
    thread_cpu: HashMap<i32, f64>,
    lists: [ListState; 5],
    /// The cores chosen, while changing them.
    cores: Option<Vec<bool>>,
}

const OOM: [(i32, &str); 5] = [(1000, "First to Go"), (500, "Sooner"), (0, "Normally"), (-500, "Later"), (-1000, "Never")];

impl Inspector {
    pub fn new(pid: i32, name: String) -> Self {
        Inspector { pid, name, tab: 0, details: None, before: None, thread_cpu: HashMap::new(), lists: Default::default(), cores: None }
    }

    /// New details: each thread's CPU since the last.
    pub fn take(&mut self, d: Result<Details, String>, hz: f64) {
        if let Ok(d) = &d {
            let now = Instant::now();
            let ticks: HashMap<i32, u64> = d.threads.iter().map(|t| (t.tid, t.ticks)).collect();
            if let Some((at, was)) = &self.before {
                let secs = now.duration_since(*at).as_secs_f64().max(0.001);
                self.thread_cpu = ticks.iter().map(|(tid, t)| (*tid, was.get(tid).map_or(0.0, |w| t.saturating_sub(*w) as f64 / hz / secs * 100.0))).collect();
            }
            self.before = Some((now, ticks));
        }
        self.details = Some(d);
    }

    pub fn ui(&mut self, app: &Activity, ui: &mut Ui) -> Option<Ask> {
        let p = style::palette(ui.ctx());
        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0.0, c(p.panel_background));
        let inner = full.shrink(m::WINDOW_INSET);
        let mut ask = None;
        let proc = app.proc(self.pid).cloned();
        let gone = matches!(self.details, Some(Err(_))) || (proc.is_none() && !app.processes.is_empty());

        // The head: its name and how it is doing.
        let head = Rect::from_min_size(inner.min, vec2(inner.width(), HEAD));
        let figure_font = dashboard::figure_font();
        style::label_fit(ui.painter(), head.left(), head.top() + 12.0, Align::Min, &self.name, figure_font.clone(), c(p.panel_text), head.width() * 0.5);
        let sub = match &proc {
            Some(pr) => format!("Process {} · {} · {}", pr.pid, pr.user, state_name(pr.state)),
            None if gone => format!("Process {} has quit", self.pid),
            None => format!("Process {}", self.pid),
        };
        style::label_fit(ui.painter(), head.left(), head.top() + 34.0, Align::Min, &sub, style::plain_font(), style::disabled_text(&p), head.width() * 0.5);
        if let Some(pr) = &proc {
            let memory = if kernel(pr) { "a kernel thread".to_owned() } else { format_size(pr.memory) };
            let figs = format!("{:.1}% CPU · {memory}", pr.cpu);
            style::label_fit(ui.painter(), head.left(), head.top() + 52.0, Align::Min, &figs, style::small_font(), c(p.panel_text), head.width() * 0.5);
        }
        let graph_r = Rect::from_min_max(pos2(head.left() + head.width() * 0.5 + m::SPACING, head.top()), head.max);
        if let Some(h) = app.proc_history.get(&self.pid) {
            let series = [Series::new("CPU", c(p.hue(Hue::Blue)), h).fill(true)];
            let fmt = |v: f64| format!("{v:.1}%");
            let top = h.iter().map(|(_, v)| *v).fold(100.0, f64::max);
            Graph::new(("inspect", self.pid), &series, app.now()).span(app.settings.span as f64).max(olo_kit::graph::nice_ceiling(top)).unit(&fmt).smooth(app.settings.smooth).show(ui, graph_r);
        }

        // What can be done to it.
        let bottom = Rect::from_min_max(pos2(inner.left(), inner.bottom() - m::CONTROL_HEIGHT), inner.max);
        let bw = 96.0;
        let force_r = Rect::from_min_size(pos2(bottom.right() - bw, bottom.top()), vec2(bw, m::CONTROL_HEIGHT));
        let quit_r = force_r.translate(vec2(-(bw + m::SPACING), 0.0));
        let signal_r = Rect::from_min_size(bottom.min, vec2(120.0, m::CONTROL_HEIGHT));
        let alive = proc.is_some();
        let pid = self.pid;
        let signal_id = Id::unique(("inspect-signal", pid));
        let resp = ui.add_enabled_ui(alive, |ui| ui.put(signal_r, Button::new("Send Signal…").min_width(signal_r.width()))).inner;
        if resp.clicked() {
            widgets::popup_menu_at(ui.ctx(), signal_id, resp.rect.left_bottom(), signal_menu(pid));
        }
        if let Some(cmd) = widgets::popup_result(ui.ctx(), signal_id) {
            ask = Some(Ask::Command(cmd));
        }
        if ui.add_enabled_ui(alive, |ui| ui.put(quit_r, Button::new("Quit").min_width(quit_r.width()))).inner.clicked() {
            ask = Some(Ask::Command(format!("quit:{pid}")));
        }
        if ui.add_enabled_ui(alive, |ui| ui.put(force_r, Button::new("Force Quit…").min_width(force_r.width()))).inner.clicked() {
            ask = Some(Ask::Command(format!("force-quit:{pid}")));
        }

        let tabs = Rect::from_min_max(pos2(inner.left(), head.bottom() + m::SPACING), pos2(inner.right(), bottom.top() - m::SPACING));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(tabs));
        let mut tab = self.tab;
        let got = TabView::new(&mut tab, &TABS).show(&mut child, |ui, t| {
            // Its left edge on the chosen tab's label's (the first tab's
            // text, as the facts start under it).
            let area = ui.max_rect().shrink(m::WINDOW_INSET - 3.0);
            match &self.details {
                None => {
                    ui.put(area, EmptyState::new("Reading…"));
                    None
                }
                Some(Err(e)) => {
                    ui.put(area, EmptyState::new(e));
                    None
                }
                Some(Ok(d)) => {
                    let d = d.clone();
                    match t {
                        0 => self.general(app, ui, area, &d, proc.as_ref()),
                        1 => self.threads(ui, area, &d),
                        2 => self.files(app, ui, area, &d),
                        3 => self.memory(app, ui, area, &d),
                        4 => self.environment(app, ui, area, &d),
                        _ => self.resources(app, ui, area, &d),
                    }
                }
            }
        });
        self.tab = tab;
        got.or(ask)
    }

    fn general(&mut self, app: &Activity, ui: &mut Ui, area: Rect, d: &Details, proc: Option<&crate::sample::Proc>) -> Option<Ask> {
        let p = style::palette(ui.ctx());
        let status = |k: &str| d.status.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str()).unwrap_or("");
        let parent = proc.map(|pr| pr.ppid).unwrap_or(0);
        let parent_name = app.proc(parent).map_or_else(String::new, |pp| format!("{} ", pp.name));
        let io = |k: &str| d.io.as_ref().and_then(|io| io.iter().find(|(n, _)| n == k).map(|(_, v)| *v));
        // A kernel thread has no program or folder, even to root.
        let unreadable = if proc.is_some_and(kernel) { "None: a kernel thread" } else { "Needs full access" };
        let mut rows: Vec<(&str, String)> = vec![
            ("Program", d.exe.clone().unwrap_or_else(|| unreadable.into())),
            ("Command", if d.command.is_empty() { "—".into() } else { d.command.join(" ") }),
            ("Folder", d.cwd.clone().unwrap_or_else(|| unreadable.into())),
            ("Parent", if parent > 0 { format!("{parent_name}({parent})") } else { "—".into() }),
            ("User", proc.map_or_else(String::new, |pr| format!("{} ({})", pr.user, pr.uid))),
        ];
        if let Some(pr) = proc {
            let ago = app.system.uptime - pr.started;
            rows.push(("Started", format!("{} ago", duration(ago))));
            rows.push(("CPU time", duration(pr.cpu_time)));
        }
        rows.push(("Scheduling", d.policy.clone()));
        let cores = match &d.affinity {
            Some(a) if a.len() >= app.system.cores => format!("Any of the {}", app.system.cores),
            Some(a) => a.iter().map(|c| (c + 1).to_string()).collect::<Vec<_>>().join(", "),
            None => "—".into(),
        };
        rows.push(("Cores", cores));
        rows.push(("Switches", format!("{} voluntary, {} forced", status("voluntary_ctxt_switches"), status("nonvoluntary_ctxt_switches"))));
        if let (Some(r), Some(w)) = (io("read_bytes"), io("write_bytes")) {
            rows.push(("Disk", format!("{} read, {} written", format_size(r), format_size(w))));
        }
        if let (Some(r), Some(w)) = (io("rchar"), io("wchar")) {
            rows.push(("All I/O", format!("{} read, {} written", format_size(r), format_size(w))));
        }
        rows.push(("Control group", if d.cgroup.is_empty() { "—".into() } else { d.cgroup.clone() }));
        if !d.namespaces.is_empty() {
            rows.push(("Isolated", d.namespaces.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(", ")));
        }
        let rr: Vec<(&str, &str)> = rows.iter().map(|(a, b)| (*a, b.as_str())).collect();
        let h = facts_in(ui, Rect::from_min_size(area.min, vec2(area.width(), area.height() - m::CONTROL_HEIGHT - m::SPACING)), &rr, style::disabled_text(&p), c(p.panel_text));
        // Priority, as one more fact (its field's text on the values'
        // line), and the parent.
        let label_w = dashboard::facts_label_width(ui, rr.iter().map(|(l, _)| *l).chain(["Priority"]));
        let y = (area.top() + h + m::TIGHT).min(area.bottom() - m::CONTROL_HEIGHT);
        style::label(ui.painter(), area.left(), y + m::CONTROL_HEIGHT / 2.0, Align::Min, "Priority", style::plain_font(), style::disabled_text(&p));
        let fw = 200.0f32.min(area.width() - label_w);
        let field = Rect::from_min_size(pos2(area.left() + label_w - m::TEXT_INSET, y), vec2(fw, m::CONTROL_HEIGHT));
        let nice = proc.map_or(0, |pr| pr.nice);
        let names: Vec<String> = PRIORITIES.iter().map(|(n, name)| format!("{name} ({n})")).collect();
        let options: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut chosen = PRIORITIES.iter().position(|(n, _)| *n == nice).unwrap_or_else(|| PRIORITIES.iter().position(|(n, _)| *n >= nice).unwrap_or(3));
        let mut ask = None;
        if ui.put(field, MenuField::new(("inspect-nice", self.pid), &mut chosen, &options).width(fw)).changed() {
            ask = Some(Ask::Act(Action::Nice { pid: self.pid, nice: PRIORITIES[chosen].0 }));
        }
        if parent > 1 {
            let b = Rect::from_min_size(pos2(area.right() - 130.0, y), vec2(130.0, m::CONTROL_HEIGHT));
            if ui.put(b, Button::new("Inspect Parent").min_width(b.width())).clicked() {
                ask = Some(Ask::Command(format!("inspect:{parent}")));
            }
        }
        ask
    }

    fn threads(&mut self, ui: &mut Ui, area: Rect, d: &Details) -> Option<Ask> {
        let columns = [Column::new("ID", 70.0).right(), Column::new("Name", 190.0), Column::new("State", 80.0), Column::new("% CPU", 60.0).right(), Column::new("Nice", 50.0).right(), Column::new("Core", 50.0).right()];
        let mut rows: Vec<&crate::details::Thread> = d.threads.iter().collect();
        let (col, asc) = self.lists[0].sort.unwrap_or((3, false));
        let cpu = |t: &crate::details::Thread| self.thread_cpu.get(&t.tid).copied().unwrap_or(0.0);
        rows.sort_by(|a, b| {
            let o = match col {
                0 => a.tid.cmp(&b.tid),
                1 => a.name.cmp(&b.name),
                2 => state_name(a.state).cmp(state_name(b.state)),
                3 => cpu(a).total_cmp(&cpu(b)),
                4 => a.nice.cmp(&b.nice),
                _ => a.core.cmp(&b.core),
            }
            .then(a.tid.cmp(&b.tid));
            if asc { o } else { o.reverse() }
        });
        self.lists[0].sort = Some((col, asc));
        let cells: Vec<[String; 6]> = rows
            .iter()
            .map(|t| [t.tid.to_string(), t.name.clone(), state_name(t.state).to_owned(), format!("{:.1}", cpu(t)), t.nice.to_string(), (t.core + 1).to_string()])
            .collect();
        ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
            ColumnListView::new(("inspect-threads", self.pid), &columns, cells.len(), &mut self.lists[0]).show(ui, |cell: &Cell| cell.text(&cells[cell.row][cell.column]))
        });
        None
    }

    fn files(&mut self, app: &Activity, ui: &mut Ui, area: Rect, d: &Details) -> Option<Ask> {
        let Some(files) = &d.files else { return needs_access(app, ui, area, "Its open files can be seen with full access.") };
        let columns = [Column::new("FD", 50.0).right(), Column::new("Kind", 70.0), Column::new("Mode", 50.0), Column::new("What", 400.0)];
        let cells: Vec<[String; 4]> = files.iter().map(|f| [f.fd.to_string(), f.kind.clone(), f.mode.clone(), f.target.clone()]).collect();
        let out = ui
            .scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                ColumnListView::new(("inspect-files", self.pid), &columns, cells.len(), &mut self.lists[1])
                    .empty_text("No open files.")
                    .show(ui, |cell: &Cell| cell.text(&cells[cell.row][cell.column]))
            })
            .inner;
        let path_of = |row: usize| files.get(row).filter(|f| f.kind == "File" || f.kind == "Folder").map(|f| f.target.clone());
        if let Some(path) = out.activated.and_then(path_of) {
            return Some(Ask::Command(format!("open-path:{path}")));
        }
        if let Some((row, pos)) = out.context {
            self.lists[1].select(row);
            let mut menu = Menu::new("").item("Copy", format!("copy-text:{}", cells[row][3]), None);
            if let Some(path) = path_of(row) {
                menu = Menu::new("").item("Show in Tracker", format!("open-path:{path}"), None).separator().item("Copy Path", format!("copy-text:{path}"), None);
            }
            widgets::popup_menu_at(ui.ctx(), Id::unique(("inspect-files-menu", self.pid)), pos, menu);
        }
        widgets::popup_result(ui.ctx(), Id::unique(("inspect-files-menu", self.pid))).map(Ask::Command)
    }

    fn memory(&mut self, app: &Activity, ui: &mut Ui, area: Rect, d: &Details) -> Option<Ask> {
        let p = style::palette(ui.ctx());
        let names = [
            ("Rss", "Resident"),
            ("Pss", "Its share"),
            ("Pss_Anon", "Its share, private"),
            ("Pss_File", "Its share, of files"),
            ("Shared_Clean", "Shared, unchanged"),
            ("Shared_Dirty", "Shared, changed"),
            ("Private_Clean", "Private, unchanged"),
            ("Private_Dirty", "Private, changed"),
            ("Swap", "Swapped out"),
            ("Locked", "Locked in memory"),
        ];
        let mut top = area.top();
        if let Some(mem) = &d.memory {
            let rows: Vec<(&str, String)> = names.iter().filter_map(|(k, label)| mem.iter().find(|(n, _)| n == k).map(|(_, v)| (*label, format_size(*v)))).collect();
            let rr: Vec<(&str, &str)> = rows.iter().map(|(a, b)| (*a, b.as_str())).collect();
            let half = rr.len().div_ceil(2);
            let w = ((area.width() - 2.0 * m::SPACING) / 2.0).floor();
            let a = facts_in(ui, Rect::from_min_size(area.min, vec2(w, area.height())), &rr[..half], style::disabled_text(&p), c(p.panel_text));
            facts_in(ui, Rect::from_min_size(pos2(area.left() + w + 2.0 * m::SPACING, area.top()), vec2(w, area.height())), &rr[half..], style::disabled_text(&p), c(p.panel_text));
            top += a + m::SPACING;
        }
        let Some(regions) = &d.regions else { return needs_access(app, ui, Rect::from_min_max(pos2(area.left(), top), area.max), "What it maps can be seen with full access.") };
        let columns = [Column::new("Mapped", 340.0), Column::new("Mappings", 70.0).right(), Column::new("Size", 80.0).right(), Column::new("Access", 60.0)];
        let cells: Vec<[String; 4]> = regions.iter().map(|r| [r.path.clone(), r.count.to_string(), format_size(r.size), r.perms.clone()]).collect();
        let list = Rect::from_min_max(pos2(area.left(), top), area.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(list), |ui| {
            ColumnListView::new(("inspect-maps", self.pid), &columns, cells.len(), &mut self.lists[2]).show(ui, |cell: &Cell| cell.text(&cells[cell.row][cell.column]))
        });
        None
    }

    fn environment(&mut self, app: &Activity, ui: &mut Ui, area: Rect, d: &Details) -> Option<Ask> {
        let Some(env) = &d.environment else { return needs_access(app, ui, area, "Its environment can be seen with full access.") };
        let columns = [Column::new("Name", 180.0), Column::new("Value", 400.0)];
        let out = ui
            .scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                ColumnListView::new(("inspect-env", self.pid), &columns, env.len(), &mut self.lists[3])
                    .empty_text("No environment.")
                    .show(ui, |cell: &Cell| cell.text(if cell.column == 0 { &env[cell.row].0 } else { &env[cell.row].1 }))
            })
            .inner;
        if let Some((row, pos)) = out.context {
            self.lists[3].select(row);
            let (k, v) = &env[row];
            let menu = Menu::new("").item("Copy Value", format!("copy-text:{v}"), None).item("Copy Name and Value", format!("copy-text:{k}={v}"), None);
            widgets::popup_menu_at(ui.ctx(), Id::unique(("inspect-env-menu", self.pid)), pos, menu);
        }
        widgets::popup_result(ui.ctx(), Id::unique(("inspect-env-menu", self.pid))).map(Ask::Command)
    }

    fn resources(&mut self, app: &Activity, ui: &mut Ui, area: Rect, d: &Details) -> Option<Ask> {
        let p = style::palette(ui.ctx());
        let mut ask = None;
        let n = app.system.cores.max(1);
        // The cores it may run on.
        let allowed: Vec<bool> = (0..n).map(|i| d.affinity.as_ref().is_none_or(|a| a.contains(&i))).collect();
        let cores = self.cores.get_or_insert_with(|| allowed.clone());
        if cores.len() != n {
            *cores = allowed.clone();
        }
        // Both labels on one divider, BeOS style.
        let divider = widgets::divider_for(ui, &["May run on:", "Out of memory:"]);
        style::label(ui.painter(), area.left() + divider - m::LABEL_GAP, area.top() + m::MARK_ROW / 2.0, Align::Max, "May run on:", style::plain_font(), c(p.panel_text));
        let x0 = area.left() + divider;
        let per_row = (((area.right() - x0) / 56.0).floor() as usize).max(1);
        for (i, on) in cores.iter_mut().enumerate() {
            let (row, col) = (i / per_row, i % per_row);
            let r = Rect::from_min_size(pos2(x0 + col as f32 * 56.0, area.top() + row as f32 * m::MARK_ROW), vec2(54.0, m::MARK_ROW));
            ui.put(r, CheckBox::new(on, &(i + 1).to_string()));
        }
        let rows = n.div_ceil(per_row);
        let mut y = area.top() + rows as f32 * m::MARK_ROW + m::TIGHT;
        let changed = *cores != allowed;
        let apply = Rect::from_min_size(pos2(x0, y), vec2(90.0, m::CONTROL_HEIGHT));
        let reset = apply.translate(vec2(90.0 + m::SPACING, 0.0));
        if ui.add_enabled_ui(changed && cores.iter().any(|c| *c), |ui| ui.put(apply, Button::new("Apply").min_width(apply.width()))).inner.clicked() {
            ask = Some(Ask::Act(Action::Affinity { pid: self.pid, cores: cores.iter().enumerate().filter(|(_, on)| **on).map(|(i, _)| i).collect() }));
            self.cores = None;
        }
        if ui.add_enabled_ui(changed, |ui| ui.put(reset, Button::new("Revert").min_width(reset.width()))).inner.clicked() {
            self.cores = None;
        }
        y += m::CONTROL_HEIGHT + m::SPACING;
        // When memory runs out.
        let names: Vec<String> = OOM.iter().map(|(v, n)| format!("{n} ({v})")).collect();
        let options: Vec<&str> = names.iter().map(String::as_str).collect();
        let adj = d.oom_adjust.unwrap_or(0);
        let mut chosen = OOM.iter().position(|(v, _)| *v <= adj).unwrap_or(OOM.len() - 1);
        let fw = (divider + 180.0).min(area.width());
        let field = Rect::from_min_size(pos2(area.left(), y), vec2(fw, m::CONTROL_HEIGHT));
        if ui.put(field, MenuField::new(("inspect-oom", self.pid), &mut chosen, &options).label("Out of memory:").divider(divider).width(fw)).changed() {
            ask = Some(Ask::Act(Action::OomAdjust { pid: self.pid, value: OOM[chosen].0 }));
        }
        let score = d.oom_score.map(|s| format!("Score {s} of 2000")).unwrap_or_default();
        // On the field's text's baseline.
        let baseline = style::plain_baseline(ui.painter(), options[chosen], field.center().y);
        style::label_on_baseline(ui.painter(), field.right() + m::SPACING, baseline, Align::Min, &score, style::small_font(), style::disabled_text(&p));
        y += m::CONTROL_HEIGHT + m::SPACING;
        // Its limits.
        let columns = [Column::new("Limit", 180.0), Column::new("Soft", 110.0).right(), Column::new("Hard", 110.0).right(), Column::new("Units", 80.0)];
        let cells: Vec<[String; 4]> = d.limits.iter().map(|l| [limit_name(&l.name), limit_value(&l.soft, &l.units), limit_value(&l.hard, &l.units), l.units.clone()]).collect();
        let list = Rect::from_min_max(pos2(area.left(), y), area.max);
        let out = ui
            .scope_builder(egui::UiBuilder::new().max_rect(list), |ui| {
                ColumnListView::new(("inspect-limits", self.pid), &columns, cells.len(), &mut self.lists[4]).show(ui, |cell: &Cell| cell.text(&cells[cell.row][cell.column]))
            })
            .inner;
        let menu_id = Id::unique(("inspect-limit-menu", self.pid));
        if let Some((row, pos)) = out.context
            && let Some(l) = d.limits.get(row)
            && let Some(res) = &l.resource
        {
            self.lists[4].select(row);
            let hard: Option<u64> = l.hard.parse().ok();
            let menu = Menu::new("")
                .item("Raise Soft Limit to Hard", format!("limit:{res}:hard"), None)
                .enabled(l.soft != l.hard)
                .item("Make Unlimited", format!("limit:{res}:unlimited"), None)
                .enabled(hard.is_some());
            widgets::popup_menu_at(ui.ctx(), menu_id, pos, menu);
        }
        if let Some(cmd) = widgets::popup_result(ui.ctx(), menu_id)
            && let Some(rest) = cmd.strip_prefix("limit:")
            && let Some((res, how)) = rest.split_once(':')
            && let Some(l) = d.limits.iter().find(|l| l.resource.as_deref() == Some(res))
        {
            let num = |s: &str| s.parse::<u64>().ok();
            let (soft, hard) = match how {
                "hard" => (num(&l.hard), num(&l.hard)),
                _ => (None, None),
            };
            ask = Some(Ask::Act(Action::Limit { pid: self.pid, resource: res.into(), soft, hard }));
        }
        ask
    }
}

fn kernel(p: &crate::sample::Proc) -> bool {
    p.pid == 2 || p.ppid == 2
}

/// A limit's name for people ("Max cpu time" is "CPU time").
fn limit_name(name: &str) -> String {
    let n = name.trim_start_matches("Max ");
    match n {
        "cpu time" => "CPU time".into(),
        "resident set" => "Resident memory".into(),
        "msgqueue size" => "Message queues".into(),
        "realtime priority" => "Real-time priority".into(),
        "realtime timeout" => "Real-time timeout".into(),
        _ => {
            let mut s = n.to_owned();
            if let Some(f) = s.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            s
        }
    }
}

/// A limit in bytes as a size ("8.0 MB"); others as they are.
fn limit_value(v: &str, units: &str) -> String {
    match (units, v.parse::<u64>()) {
        ("bytes", Ok(n)) => format_size(n),
        _ => v.to_owned(),
    }
}

/// An empty tab whose contents need full access, with the way to get it.
fn needs_access(app: &Activity, ui: &mut Ui, area: Rect, text: &str) -> Option<Ask> {
    let text = match app.access {
        Access::Asking => "Asking for full access…",
        _ => text,
    };
    let top = Rect::from_min_size(area.min, vec2(area.width(), (area.height() - m::CONTROL_HEIGHT - m::SPACING).max(0.0)));
    ui.put(top, EmptyState::new(text).icon("authentication"));
    if app.access == Access::Mine && crate::helper::way() != crate::helper::Way::Off {
        let b = Rect::from_center_size(pos2(area.center().x, top.bottom() - m::CONTROL_HEIGHT), vec2(140.0, m::CONTROL_HEIGHT));
        if ui.put(b, Button::new("Get Full Access…").min_width(b.width())).clicked() {
            return Some(Ask::Command("full-access".into()));
        }
    }
    None
}
