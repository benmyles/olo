//! A process's inspector (Get Info for a process): what it is and how
//! it is doing, over tabs of its threads, open files, memory, environment
//! and resources (the cores it may use, how the kernel treats it when
//! memory runs out, its limits) and what the supervisor (Preferences ▸
//! Performance) found about it, with what can be done to it. Its details
//! are read on a worker (or by the helper, with full access) at each
//! sample.

use std::{
    collections::{HashMap, VecDeque},
    rc::Rc,
    time::Instant,
};

use olo_kit::{
    ClipboardItem, Corner, Cx, Element, FontWeight, Hsla, IntoElement as _, Menu, MenuBar, Painter, ParentElement as _, SharedString, Styled as _, View, ViewRef, WindowCx, agent, canvas, clipboard,
    div,
    files::format_size,
    menu, metrics as m,
    palette::Hue,
    px,
    style::{self, c},
    theme,
    widgets::{
        Button, CheckBox, Column, ColumnListView, EmptyState, Graph, ListModel, MenuField, Series, StringView, TabView,
        dashboard::{self, Facts, facts_label_width},
        nice_ceiling,
    },
};

use crate::{
    Access, Shared,
    actions::Action,
    details::Details,
    processes::{PRIORITIES, signal_menu, state_name},
    sample::{Proc, System},
    sections::{self, duration},
    tell,
};

const TABS: [&str; 7] = ["General", "Threads", "Open Files", "Memory", "Environment", "Resources", "Supervisor"];
/// The Supervisor tab: what the supervisor's alert and Deskbar's menu open.
pub const SUPERVISOR_TAB: usize = 6;
const HEAD: f32 = 64.0;

const OOM: [(i32, &str); 5] = [(1000, "First to Go"), (500, "Sooner"), (0, "Normally"), (-500, "Later"), (-1000, "Never")];

/// What a sample says of an inspected process: it (None: it has quit),
/// its parent's name, and the machine's figures its tabs go by.
#[derive(Clone, Default)]
pub struct Seen {
    pub proc: Option<Proc>,
    pub parent: String,
    pub uptime: f64,
    pub cores: usize,
    /// Whether the sample listed any processes.
    pub listed: bool,
}

impl Seen {
    pub fn of(list: &[Proc], system: &System, pid: i32) -> Seen {
        let proc = list.iter().find(|p| p.pid == pid).cloned();
        let parent = proc.as_ref().and_then(|p| list.iter().find(|q| q.pid == p.ppid)).map_or_else(String::new, |q| q.name.clone());
        Seen { proc, parent, uptime: system.uptime, cores: system.cores, listed: !list.is_empty() }
    }
}

/// What an inspector is told, by the app.
pub enum Msg {
    /// What every window knows changed (settings, access, the supervisor).
    Shared(Box<Shared>),
    /// A sample's account of its process.
    Seen(Box<Seen>),
    Details(Result<Box<Details>, String>),
    /// Show this tab.
    Tab(usize),
    /// Something that couldn't be done.
    Alert(String),
}

pub struct Inspector {
    pub pid: i32,
    pub name: String,
    tab: usize,
    details: Option<Result<Details, String>>,
    /// Each thread's ticks at the last reading, and when.
    before: Option<(Instant, HashMap<i32, u64>)>,
    thread_cpu: HashMap<i32, f64>,
    /// Threads, open files, memory maps, environment, limits.
    lists: [ListModel; 5],
    /// The cores chosen, while changing them.
    cores: Option<Vec<bool>>,
    shared: Shared,
    seen: Seen,
    /// Its CPU over time, since the inspector opened.
    history: Rc<VecDeque<(f64, f64)>>,
    /// When the latest sample came.
    last: Option<f64>,
    hz: f64,
}

type Me = ViewRef<Inspector>;

impl Inspector {
    pub fn new(pid: i32, name: String, tab: usize, shared: Shared, seen: Option<Seen>, _: &mut Cx<Self>) -> Self {
        let lists = std::array::from_fn(|_| ListModel::new(false));
        // SAFETY: sysconf has no preconditions.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
        Inspector {
            pid,
            name,
            tab: tab.min(TABS.len() - 1),
            details: None,
            before: None,
            thread_cpu: HashMap::new(),
            lists,
            cores: None,
            shared,
            seen: seen.unwrap_or_default(),
            history: Default::default(),
            last: None,
            hz,
        }
    }

    /// New details: each thread's CPU since the last.
    fn take(&mut self, d: Result<Details, String>) {
        if let Ok(d) = &d {
            let now = Instant::now();
            let ticks: HashMap<i32, u64> = d.threads.iter().map(|t| (t.tid, t.ticks)).collect();
            if let Some((at, was)) = &self.before {
                let secs = now.duration_since(*at).as_secs_f64().max(0.001);
                self.thread_cpu = ticks.iter().map(|(tid, t)| (*tid, was.get(tid).map_or(0.0, |w| t.saturating_sub(*w) as f64 / self.hz / secs * 100.0))).collect();
            }
            self.before = Some((now, ticks));
        }
        self.details = Some(d);
    }

    /// A sample's account of it: its CPU kept for the graph.
    fn saw(&mut self, seen: Seen) {
        let at = self.shared.clock.elapsed().as_secs_f64();
        if let Some(p) = &seen.proc {
            let h = Rc::make_mut(&mut self.history);
            h.push_back((at, p.cpu));
            while h.front().is_some_and(|(t, _)| *t < at - crate::KEEP_SECONDS) {
                h.pop_front();
            }
        }
        self.last = Some(at);
        self.seen = seen;
    }

    /// `command` from its menus and buttons: the Process menu's acting
    /// on this one; what isn't about it is the app's.
    fn run_command(&mut self, command: &str, cx: &mut Cx<Self>) {
        let (pid, access) = (self.pid, self.shared.access);
        if let Some(actions) = crate::process_actions(command) {
            return crate::act_all(access, actions, cx);
        }
        if let Some(path) = command.strip_prefix("open-path:") {
            let _ = olo_kit::tracker::open(std::path::Path::new(path));
            return;
        }
        if let Some(text) = command.strip_prefix("copy-text:") {
            return clipboard::write(ClipboardItem::new_string(text.to_owned()));
        }
        if let Some(name) = command.strip_prefix("supervisor-ignore:") {
            return sections::supervise(sections::Supervise::Ignore(name.to_owned()));
        }
        if let Some(pid) = command.strip_prefix("inspect:").and_then(|p| p.parse().ok()) {
            return tell(crate::Msg::Inspect(pid, 0, None));
        }
        match command {
            "close" => cx.close_window(),
            "quit-process" => crate::quit_processes(access, vec![pid], self.name.clone(), false, cx),
            "force-quit-process" => crate::quit_processes(access, vec![pid], self.name.clone(), true, cx),
            "full-access" => tell(crate::Msg::GetAccess(Vec::new())),
            "supervisor-preferences" => sections::supervise(sections::Supervise::Preferences),
            _ => tell(crate::Msg::Command(command.to_owned())),
        }
    }

    /// `command`, from a handler that has only the window.
    fn run(me: &Me, command: &str, cx: &mut WindowCx) {
        let command = command.to_owned();
        me.update(cx, move |v, cx| v.run_command(&command, cx));
    }

    fn act_on(me: &Me, action: Action, cx: &mut WindowCx) {
        me.update(cx, move |v, cx| v.act(action, cx));
    }

    fn act(&self, action: Action, cx: &mut WindowCx) {
        crate::act_all(self.shared.access, vec![action], cx);
    }

    fn general(&self, me: &Me, d: &Details, proc: Option<&Proc>, _: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let status = |k: &str| d.status.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str()).unwrap_or("");
        let parent = proc.map(|pr| pr.ppid).unwrap_or(0);
        let parent_name = if self.seen.parent.is_empty() { String::new() } else { format!("{} ", self.seen.parent) };
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
            let ago = self.seen.uptime - pr.started;
            rows.push(("Started", format!("{} ago", duration(ago))));
            rows.push(("CPU time", duration(pr.cpu_time)));
        }
        rows.push(("Scheduling", d.policy.clone()));
        let cores = match &d.affinity {
            Some(a) if a.len() >= self.seen.cores => format!("Any of the {}", self.seen.cores),
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
        // Priority, as one more fact (its field's text on the values'
        // line), and the parent.
        let label_w = facts_label_width(rows.iter().map(|(l, _)| *l).chain(["Priority"]));
        let facts: Vec<(SharedString, SharedString)> = rows.into_iter().map(|(a, b)| (SharedString::from(a), SharedString::from(b))).collect();
        let (dim, text) = (style::disabled_text(&p), c(p.panel_text));
        let nice = proc.map_or(0, |pr| pr.nice);
        let options: Vec<SharedString> = PRIORITIES.iter().map(|(n, name)| SharedString::from(format!("{name} ({n})"))).collect();
        let chosen = PRIORITIES.iter().position(|(n, _)| *n == nice).unwrap_or_else(|| PRIORITIES.iter().position(|(n, _)| *n >= nice).unwrap_or(3));
        let pid = self.pid;
        let nice_me = *me;
        // Its choice on the values' line.
        let divider = label_w - m::TEXT_INSET;
        let field =
            MenuField::new(SharedString::from(format!("inspect-nice-{pid}")), options, chosen).label("Priority").quiet_label().divider(divider).width(divider + 200.0).on_change(move |i, cx| {
                let nice = PRIORITIES[*i].0;
                Inspector::act_on(&nice_me, Action::Nice { pid, nice }, cx);
            });
        let mut row = div().flex().flex_row().items_center().h(px(m::CONTROL_HEIGHT)).mt(px(m::TIGHT)).flex_shrink_0().child(field).child(div().flex_1());
        if parent > 1 {
            let up = *me;
            row = row.child(Button::new("Inspect Parent").min_width(130.0).on_click(move |_, cx| Inspector::run(&up, &format!("inspect:{parent}"), cx)));
        }
        div().flex().flex_col().size_full().child(Facts::new(facts).colors(dim, text)).child(row).into_any_element()
    }

    fn threads(&self, d: &Details, cx: &mut Cx<Self>) -> Element {
        let columns = vec![
            Column::new("ID", 70.0).right(),
            Column::new("Name", 190.0),
            Column::new("State", 80.0),
            Column::new("% CPU", 60.0).right(),
            Column::new("Nice", 50.0).right(),
            Column::new("Core", 50.0).right(),
        ];
        let mut rows: Vec<&crate::details::Thread> = d.threads.iter().collect();
        let (col, asc) = self.lists[0].state().sort.unwrap_or((3, false));
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
        self.lists[0].state_mut().sort = Some((col, asc));
        let cells: Rc<Vec<[String; 6]>> =
            Rc::new(rows.iter().map(|t| [t.tid.to_string(), t.name.clone(), state_name(t.state).to_owned(), format!("{:.1}", cpu(t)), t.nice.to_string(), (t.core + 1).to_string()]).collect());
        list(ColumnListView::new(cx, format!("inspect-threads-{}", self.pid), &self.lists[0], columns, cells.len(), text_cells(cells)))
    }

    fn files(&self, me: &Me, d: &Details, cx: &mut Cx<Self>) -> Element {
        let Some(files) = &d.files else { return needs_access(me, self.shared.access, "Its open files can be seen with full access.") };
        let columns = vec![Column::new("FD", 50.0).right(), Column::new("Kind", 70.0), Column::new("Mode", 50.0), Column::new("What", 400.0)];
        let cells: Rc<Vec<[String; 4]>> = Rc::new(files.iter().map(|f| [f.fd.to_string(), f.kind.clone(), f.mode.clone(), f.target.clone()]).collect());
        let paths: Rc<Vec<Option<String>>> = Rc::new(files.iter().map(|f| (f.kind == "File" || f.kind == "Folder").then(|| f.target.clone())).collect());
        let (open_me, open_paths) = (*me, paths.clone());
        let (menu_me, menu_cells) = (*me, cells.clone());
        let n = cells.len();
        list(
            ColumnListView::new(cx, format!("inspect-files-{}", self.pid), &self.lists[1], columns, n, text_cells(cells))
                .empty_text("No open files.")
                .on_open(move |row, cx| {
                    if let Some(Some(path)) = open_paths.get(*row) {
                        Inspector::run(&open_me, &format!("open-path:{path}"), cx);
                    }
                })
                .on_context(move |&(row, at), cx| {
                    let Some(cl) = menu_cells.get(row) else { return };
                    let menu = match paths.get(row) {
                        Some(Some(path)) => Menu::new("").item("Show in Tracker", format!("open-path:{path}")).separator().item("Copy Path", format!("copy-text:{path}")),
                        _ => Menu::new("").item("Copy", format!("copy-text:{}", cl[3])),
                    };
                    let to = menu_me;
                    menu::context_menu(cx, menu, at, move |cmd, cx| Inspector::run(&to, cmd, cx));
                }),
        )
    }

    fn memory(&self, me: &Me, d: &Details, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
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
        let mut out = div().flex().flex_col().size_full();
        if let Some(mem) = &d.memory {
            let mut rows: Vec<(SharedString, SharedString)> =
                names.iter().filter_map(|(k, label)| mem.iter().find(|(n, _)| n == k).map(|(_, v)| (SharedString::from(*label), SharedString::from(format_size(*v))))).collect();
            let right = rows.split_off(rows.len().div_ceil(2));
            let (dim, text) = (style::disabled_text(&p), c(p.panel_text));
            out = out.child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(2.0 * m::SPACING))
                    .mb(px(m::SPACING))
                    .flex_shrink_0()
                    .child(div().flex_1().min_w_0().child(Facts::new(rows).colors(dim, text)))
                    .child(div().flex_1().min_w_0().child(Facts::new(right).colors(dim, text))),
            );
        }
        let Some(regions) = &d.regions else { return out.child(div().flex_1().min_h_0().child(needs_access(me, self.shared.access, "What it maps can be seen with full access."))).into_any_element() };
        let columns = vec![Column::new("Mapped", 340.0), Column::new("Mappings", 70.0).right(), Column::new("Size", 80.0).right(), Column::new("Access", 60.0)];
        let cells: Rc<Vec<[String; 4]>> = Rc::new(regions.iter().map(|r| [r.path.clone(), r.count.to_string(), format_size(r.size), r.perms.clone()]).collect());
        out.child(list(ColumnListView::new(cx, format!("inspect-maps-{}", self.pid), &self.lists[2], columns, cells.len(), text_cells(cells)))).into_any_element()
    }

    fn environment(&self, me: &Me, d: &Details, cx: &mut Cx<Self>) -> Element {
        let Some(env) = &d.environment else { return needs_access(me, self.shared.access, "Its environment can be seen with full access.") };
        let columns = vec![Column::new("Name", 180.0), Column::new("Value", 400.0)];
        let cells: Rc<Vec<[String; 2]>> = Rc::new(env.iter().map(|(k, v)| [k.clone(), v.clone()]).collect());
        let (menu_me, menu_cells) = (*me, cells.clone());
        list(ColumnListView::new(cx, format!("inspect-env-{}", self.pid), &self.lists[3], columns, cells.len(), text_cells(cells)).empty_text("No environment.").on_context(move |&(row, at), cx| {
            let Some([k, v]) = menu_cells.get(row) else { return };
            let menu = Menu::new("").item("Copy Value", format!("copy-text:{v}")).item("Copy Name and Value", format!("copy-text:{k}={v}"));
            let to = menu_me;
            menu::context_menu(cx, menu, at, move |cmd, cx| Inspector::run(&to, cmd, cx));
        }))
    }

    fn resources(&self, me: &Me, d: &Details, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let n = self.seen.cores.max(1);
        // The cores it may run on.
        let allowed: Vec<bool> = (0..n).map(|i| d.affinity.as_ref().is_none_or(|a| a.contains(&i))).collect();
        let cores = self.cores.clone().filter(|c| c.len() == n).unwrap_or_else(|| allowed.clone());
        // Both labels on one divider, BeOS style.
        let divider = facts_label_width(["May run on:", "Out of memory:"]);
        let text = c(p.panel_text);
        let may = canvas(move |g, b| {
            let r = style::label(g, b.right() - px(m::LABEL_GAP), b.top() + px(m::MARK_ROW / 2.0), style::Align::Right, "May run on:", style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), text);
            agent::record(g, agent::Node::label("May run on:", r));
        })
        .w(px(divider))
        .flex_shrink_0();
        let mut boxes = div().flex().flex_row().flex_wrap().flex_1().min_w_0();
        for (i, on) in cores.iter().enumerate() {
            let (to, all) = (*me, cores.clone());
            boxes = boxes.child(div().w(px(56.0)).h(px(m::MARK_ROW)).child(CheckBox::new((i + 1).to_string(), *on).id(format!("inspect-core-{i}")).on_toggle(move |on, cx| {
                let mut next = all.clone();
                next[i] = *on;
                to.update(cx, move |v, cx| {
                    v.cores = Some(next);
                    cx.invalidate();
                });
            })));
        }
        let changed = cores != allowed;
        let pid = self.pid;
        let (apply_me, revert_me) = (*me, *me);
        let chosen_cores: Vec<usize> = cores.iter().enumerate().filter(|(_, on)| **on).map(|(i, _)| i).collect();
        let buttons = div()
            .flex()
            .flex_row()
            .gap(px(m::SPACING))
            .pl(px(divider))
            .mt(px(m::TIGHT))
            .child(Button::new("Apply").min_width(90.0).enabled(changed && !chosen_cores.is_empty()).on_click(move |_, cx| {
                let cores = chosen_cores.clone();
                apply_me.update(cx, move |v, cx| {
                    v.cores = None;
                    v.act(Action::Affinity { pid, cores }, cx);
                    cx.invalidate();
                });
            }))
            .child(Button::new("Revert").min_width(90.0).enabled(changed).on_click(move |_, cx| {
                revert_me.update(cx, |v, cx| {
                    v.cores = None;
                    cx.invalidate();
                })
            }));
        // When memory runs out.
        let options: Vec<SharedString> = OOM.iter().map(|(v, n)| SharedString::from(format!("{n} ({v})"))).collect();
        let adj = d.oom_adjust.unwrap_or(0);
        let chosen = OOM.iter().position(|(v, _)| *v <= adj).unwrap_or(OOM.len() - 1);
        let oom_me = *me;
        let field = MenuField::new(SharedString::from(format!("inspect-oom-{pid}")), options, chosen).label("Out of memory:").divider(divider).width(divider + 180.0).on_change(move |i, cx| {
            Inspector::act_on(&oom_me, Action::OomAdjust { pid, value: OOM[*i].0 }, cx);
        });
        let score: SharedString = d.oom_score.map(|s| format!("Score {s} of 2000")).unwrap_or_default().into();
        let dim = style::disabled_text(&p);
        let score_el = canvas(move |g, b| {
            // On the field's text's baseline.
            let baseline = style::baseline_at(g, b.center().y, style::PLAIN_SIZE);
            let r = style::label_on_baseline(g, b.left(), baseline, style::Align::Left, score.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), dim);
            agent::record(g, agent::Node::label(score.clone(), r));
        })
        .flex_1()
        .min_w_0()
        .h_full();
        let oom = div().flex().flex_row().gap(px(m::SPACING)).h(px(m::CONTROL_HEIGHT)).mt(px(m::SPACING)).mb(px(m::SPACING)).flex_shrink_0().child(field).child(score_el);
        // Its limits.
        let columns = vec![Column::new("Limit", 180.0), Column::new("Soft", 110.0).right(), Column::new("Hard", 110.0).right(), Column::new("Units", 80.0)];
        let cells: Rc<Vec<[String; 4]>> = Rc::new(d.limits.iter().map(|l| [limit_name(&l.name), limit_value(&l.soft, &l.units), limit_value(&l.hard, &l.units), l.units.clone()]).collect());
        let limits = Rc::new(d.limits.clone());
        let menu_me = *me;
        let limits_list = ColumnListView::new(cx, format!("inspect-limits-{pid}"), &self.lists[4], columns, cells.len(), text_cells(cells)).on_context(move |&(row, at), cx| {
            let Some(l) = limits.get(row) else { return };
            let Some(res) = l.resource.clone() else { return };
            let hard: Option<u64> = l.hard.parse().ok();
            let menu = Menu::new("").item("Raise Soft Limit to Hard", "hard").enabled(l.soft != l.hard).item("Make Unlimited", "unlimited").enabled(hard.is_some());
            let (to, l) = (menu_me, l.clone());
            menu::context_menu(cx, menu, at, move |how, cx| {
                let num = |s: &str| s.parse::<u64>().ok();
                let (soft, hard) = match how {
                    "hard" => (num(&l.hard), num(&l.hard)),
                    _ => (None, None),
                };
                Inspector::act_on(&to, Action::Limit { pid, resource: res.clone(), soft, hard }, cx);
            });
        });
        div().flex().flex_col().size_full().child(div().flex().flex_row().flex_shrink_0().child(may).child(boxes)).child(buttons).child(oom).child(list(limits_list)).into_any_element()
    }
}

/// A list filling what's left of a tab.
fn list(view: ColumnListView<Inspector>) -> Element {
    div().flex_1().min_h_0().flex().flex_col().child(view).into_any_element()
}

/// Cells of plain text.
fn text_cells<const N: usize>(cells: Rc<Vec<[String; N]>>) -> impl Fn(&Inspector, &mut olo_kit::widgets::Cell, &mut Painter) + 'static {
    move |_, cell, g| {
        if let Some(row) = cells.get(cell.row) {
            cell.text(g, row[cell.column].clone());
        }
    }
}

fn kernel(p: &Proc) -> bool {
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
fn needs_access(me: &Me, access: Access, text: &str) -> Element {
    let text = match access {
        Access::Asking => "Asking for full access…",
        _ => text,
    };
    let mut empty = EmptyState::new(text.to_owned()).icon("authentication");
    if access == Access::Mine && crate::helper::way() != crate::helper::Way::Off {
        let me = *me;
        empty = empty.action("Get Full Access…", move |_, cx| Inspector::run(&me, "full-access", cx));
    }
    div().size_full().child(empty).into_any_element()
}

/// What the supervisor has on `pid`: each finding with how sure it is,
/// what it means and the evidence; whether it lowered it, and why; or
/// that it favours it (the active window's app).
fn supervised(me: &Me, st: &olo_protocol::performance::SupervisorState, pid: i32) -> Element {
    if !st.running {
        let me = *me;
        return div()
            .size_full()
            .child(
                EmptyState::new("The supervisor is off")
                    .detail("Turned on in Performance preferences (or Deskbar’s gauge), it lowers background work and watches for stuck processes, leaks and memory hogs.")
                    .action("Performance Preferences…", move |_, cx| Inspector::run(&me, "supervisor-preferences", cx)),
            )
            .into_any_element();
    }
    let findings: Vec<&olo_protocol::performance::Finding> = st.findings.iter().filter(|f| f.pid == pid).collect();
    let lowered = st.lowered.iter().find(|l| l.pid == pid);
    let favoured = st.favoured.as_ref().is_some_and(|(p, _)| *p == pid);
    if findings.is_empty() && lowered.is_none() && !favoured {
        return div().size_full().child(EmptyState::new("Nothing found").detail("The supervisor hasn’t found anything wrong with this process, and hasn’t changed its priority.")).into_any_element();
    }
    let p = theme::palette();
    let (text, dim) = (c(p.panel_text), style::disabled_text(&p));
    // Each paragraph wrapped to the tab, one under the other.
    let para = |s: String, color: Hsla| StringView::new(s).wrap(true).color(color);
    let heading = |s: String| StringView::new(s).wrap(true).bold(true).color(text);
    let gap = || div().h(px(m::SPACING * 1.5)).flex_shrink_0();
    let mut out = div().flex().flex_col().size_full();
    for f in &findings {
        let urgent = if f.urgent { ", urgent" } else { "" };
        out = out.child(heading(format!("{} ({:.0}% sure{urgent})", f.kind.title(), f.confidence * 100.0))).child(para(f.summary.clone(), text));
        for e in &f.evidence {
            out = out.child(para(format!("•  {e}"), dim));
        }
        out = out.child(gap());
    }
    if let Some(l) = lowered {
        out = out
            .child(heading("Lowered".into()))
            .child(para(format!("As {}: nice {} → {}, {} scheduling, {} I/O priority, so the active window’s app and Olo stay quick.", l.why, l.nice_before, l.nice, l.policy, l.io), text))
            .child(gap());
    }
    if favoured {
        out = out.child(heading("Favoured".into())).child(para("It has the active window: never lowered while it does.".into(), text)).child(gap());
    }
    let mut buttons = div().flex().flex_row().flex_wrap().gap(px(m::SPACING));
    if let Some(f) = findings.first() {
        let (to, name) = (*me, f.name.clone());
        buttons = buttons.child(Button::new("Ignore This Process").on_click(move |_, cx| Inspector::run(&to, &format!("supervisor-ignore:{name}"), cx)));
    }
    let to = *me;
    buttons = buttons.child(Button::new("Performance Preferences…").on_click(move |_, cx| Inspector::run(&to, "supervisor-preferences", cx)));
    out.child(buttons).into_any_element()
}

impl View for Inspector {
    type Message = Msg;

    fn message(&mut self, msg: Msg, cx: &mut Cx<Self>) {
        match msg {
            Msg::Shared(shared) => self.shared = *shared,
            Msg::Seen(seen) => self.saw(*seen),
            Msg::Details(d) => self.take(d.map(|d| *d)),
            Msg::Tab(tab) => self.tab = tab.min(TABS.len() - 1),
            Msg::Alert(message) => crate::alert(&message, cx),
        }
        cx.invalidate();
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        self.run_command(command, cx);
        true
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        tell(crate::Msg::InspectorClosed(self.pid));
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let me = cx.view_ref();
        let p = theme::palette();
        let pid = self.pid;
        let proc = self.seen.proc.clone();
        let nices: Vec<i32> = proc.iter().map(|p| p.nice).collect();
        let bar = MenuBar::new(cx, crate::menus(&self.shared.settings, &self.shared.sections, self.shared.access, &[pid], &nices, true));
        let gone = matches!(self.details, Some(Err(_))) || (proc.is_none() && self.seen.listed);

        // The head: its name and how it is doing.
        let name: SharedString = self.name.clone().into();
        let sub: SharedString = match &proc {
            Some(pr) => format!("Process {} · {} · {}", pr.pid, pr.user, state_name(pr.state)),
            None if gone => format!("Process {pid} has quit"),
            None => format!("Process {pid}"),
        }
        .into();
        let figs: Option<SharedString> = proc.as_ref().map(|pr| {
            let memory = if kernel(pr) { "a kernel thread".to_owned() } else { format_size(pr.memory) };
            format!("{:.1}% CPU · {memory}", pr.cpu).into()
        });
        let words = canvas(move |g, b| {
            let p = theme::palette();
            let (text, dim) = (c(p.panel_text), style::disabled_text(&p));
            let plain = style::ui_font(FontWeight::NORMAL);
            let n = style::label_fit(g, b.left(), b.top() + px(12.0), style::Align::Left, name.clone(), dashboard::FIGURE_SIZE, style::ui_font(FontWeight::BOLD), text, b.size.width);
            agent::record(g, agent::Node { role: "Heading", name: Some(name.clone()), bounds: n, enabled: true, ..Default::default() });
            let s = style::label_fit(g, b.left(), b.top() + px(34.0), style::Align::Left, sub.clone(), style::PLAIN_SIZE, plain.clone(), dim, b.size.width);
            agent::record(g, agent::Node::label(sub.clone(), s));
            if let Some(f) = &figs {
                let r = style::label_fit(g, b.left(), b.top() + px(52.0), style::Align::Left, f.clone(), style::SMALL_SIZE, plain, text, b.size.width);
                agent::record(g, agent::Node::label(f.clone(), r));
            }
        })
        .flex_1()
        .min_w_0()
        .h_full();
        let mut head = div().flex().flex_row().gap(px(m::SPACING)).h(px(HEAD)).flex_shrink_0().child(words);
        let graph = if self.history.is_empty() {
            div().into_any_element()
        } else {
            let top = self.history.iter().map(|(_, v)| *v).fold(100.0, f64::max);
            let series = vec![Series::new("CPU", c(p.hue(Hue::Blue)), self.history.clone()).fill(true)];
            Graph::live(SharedString::from(format!("inspect-{pid}")), series, self.shared.graph_clock(self.last))
                .span(self.shared.settings.span as f64)
                .max(nice_ceiling(top))
                .unit(|v| format!("{v:.1}%"))
                .smooth(self.shared.settings.smooth)
                .into_any_element()
        };
        head = head.child(div().flex_1().min_w_0().h_full().child(graph));

        // The chosen tab's.
        let content = if self.tab == SUPERVISOR_TAB {
            supervised(&me, &self.shared.supervisor, pid)
        } else {
            match &self.details {
                None => div().size_full().child(EmptyState::new("Reading…")).into_any_element(),
                Some(Err(e)) => div().size_full().child(EmptyState::new(e.clone())).into_any_element(),
                Some(Ok(d)) => {
                    let d = d.clone();
                    match self.tab {
                        0 => self.general(&me, &d, proc.as_ref(), cx),
                        2 => self.files(&me, &d, cx),
                        3 => self.memory(&me, &d, cx),
                        4 => self.environment(&me, &d, cx),
                        5 => self.resources(&me, &d, cx),
                        _ => self.threads(&d, cx),
                    }
                }
            }
        };
        let tabs = TabView::new(format!("inspect-tabs-{pid}"), TABS.iter().map(|t| SharedString::from(*t)).collect::<Vec<_>>(), self.tab)
            .on_select(cx.listener(|v: &mut Inspector, i: &usize, cx| {
                v.tab = (*i).min(TABS.len() - 1);
                cx.invalidate();
            }))
            .child(content);

        // What can be done to it.
        let alive = proc.is_some();
        let bw = 96.0;
        let (signal_me, quit_me, force_me) = (me, me, me);
        let foot = div()
            .flex()
            .flex_row()
            .gap(px(m::SPACING))
            .h(px(m::CONTROL_HEIGHT))
            .flex_shrink_0()
            .child(Button::new("Send Signal…").min_width(120.0).enabled(alive).on_click(move |_, cx| {
                let (to, b) = (signal_me, cx.listener_bounds());
                menu::popup(cx, signal_menu(&[pid]), b, Corner::BottomLeft, move |cmd, cx| Inspector::run(&to, cmd, cx));
            }))
            .child(div().flex_1())
            .child(Button::new("Quit").min_width(bw).enabled(alive).on_click(move |_, cx| Inspector::run(&quit_me, &format!("quit:{pid}"), cx)))
            .child(Button::new("Force Quit…").min_width(bw).enabled(alive).on_click(move |_, cx| Inspector::run(&force_me, &format!("force-quit:{pid}"), cx)));
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.panel_background))
            .child(bar)
            .child(div().flex_1().min_h_0().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).child(head).child(div().flex_1().min_h_0().child(tabs)).child(foot))
    }
}
