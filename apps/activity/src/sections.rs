//! The sections that are cards of graphs and facts: Overview, CPU,
//! Memory, Disks, Network, GPU, Sensors, System and the Graphs of your
//! choosing. (Processes is processes.rs.)

use olo_kit::{
    Cx, Menu,
    dashboard::{self, Card, LegendItem, facts, facts_two, flow, flow_rows, legend, legend_height, meter},
    egui::{self, Align, Color32, Id, Rect, Sense, Ui, pos2, vec2},
    files::format_size,
    graph::{Graph, Series},
    metrics as m,
    style::{self, c},
    widgets::{self, Button, Cell, Column, ColumnListView, MenuField, ScrollView, TextControl},
};
use olo_theme::Palette;

use crate::{
    Access, Activity, Section,
    actions::Action,
    sample::{Group, Unit},
};

const GAP: f32 = dashboard::GAP;
/// A big graph's height in a card.
const GRAPH: f32 = 120.0;
/// A small one's (each core, each sensor).
const MINI_GRAPH: f32 = 44.0;
const MINI_CELL: f32 = 16.0 + MINI_GRAPH + 4.0;
const MINI_MIN_WIDTH: f32 = 150.0;

/// A source's series for a graph: its name, colour and samples.
fn series<'a>(app: &'a Activity, p: &Palette, key: &str, fill: bool) -> Option<Series<'a>> {
    let i = app.source(key)?;
    let s = &app.sources[i];
    Some(Series::new(&s.name, c(p.hue(s.color)), &app.history[i]).fill(fill))
}

fn value(app: &Activity, key: &str) -> String {
    let unit = app.source(key).map(|i| app.sources[i].unit);
    match (app.latest(key), unit) {
        (Some(v), Some(u)) => u.format(v),
        _ => "–".into(),
    }
}

fn legend_items<'a>(app: &'a Activity, p: &Palette, keys: &[&str], names: &[&'a str]) -> Vec<LegendItem<'a>> {
    keys.iter()
        .zip(names)
        .filter_map(|(k, n)| {
            let i = app.source(k)?;
            Some(LegendItem { color: c(p.hue(app.sources[i].color)), name: n, value: value(app, k) })
        })
        .collect()
}

/// A graph of `keys` in `r` (the first filled), scaled to `max` or what
/// it shows. Returns the time clicked, for looking back.
fn graph(app: &Activity, ui: &mut Ui, id: &str, r: Rect, keys: &[&str], max: Option<f64>, unit: Unit) -> Option<f64> {
    let p = style::palette(ui.ctx());
    let s: Vec<Series> = keys.iter().enumerate().filter_map(|(i, k)| series(app, &p, k, i == 0)).collect();
    let fmt = move |v: f64| unit.format(v);
    let mut g = Graph::new(id, &s, app.now()).span(app.settings.span as f64).grid(grid_for(app.settings.span)).unit(&fmt).smooth(app.settings.smooth).plain();
    if let Some(m) = max {
        g = g.max(m);
    }
    g.show(ui, r).clicked
}

/// Time lines: every 10 s over a minute, a minute over 5, 5 over 30.
fn grid_for(span: u32) -> f64 {
    match span {
        0..=60 => 10.0,
        61..=300 => 60.0,
        301..=900 => 180.0,
        _ => 300.0,
    }
}

/// A small graph with its name and value over it (a core, a sensor).
fn mini(app: &Activity, ui: &mut Ui, r: Rect, key: &str, label: &str, max: Option<f64>) {
    let p = style::palette(ui.ctx());
    let ink = style::card_ink(&p);
    let Some(i) = app.source(key) else { return };
    let unit = app.sources[i].unit;
    let mid = r.top() + 8.0;
    let v = value(app, key);
    let vw = style::label_width(ui.ctx(), &v, style::small_font());
    style::label_fit(ui.painter(), r.left(), mid, Align::Min, label, style::small_font(), ink.dim, r.width() - vw - m::SPACING);
    style::label(ui.painter(), r.right(), mid, Align::Max, &v, style::small_font(), ink.text);
    let g = Rect::from_min_size(pos2(r.left(), r.top() + 16.0), vec2(r.width(), MINI_GRAPH));
    let _ = graph(app, ui, key, g, &[key], max, unit);
}

/// Cells for `n` small graphs across `width`: their rects from `top`.
fn mini_grid(area_left: f32, width: f32, top: f32, n: usize) -> (Vec<Rect>, f32) {
    let area = Rect::from_min_size(pos2(area_left, top), vec2(width, 0.0));
    let rows = flow_rows(width, n, MINI_MIN_WIDTH, 8);
    (flow(area, n, MINI_MIN_WIDTH, 8, MINI_CELL), rows as f32 * (MINI_CELL + GAP) - GAP)
}

fn clock(secs: f64) -> String {
    let t = secs as libc::time_t;
    // SAFETY: localtime_r fills the zeroed tm we own.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    };
    let h = tm.tm_hour % 12;
    format!("{}:{:02}:{:02} {}", if h == 0 { 12 } else { h }, tm.tm_min, tm.tm_sec, if tm.tm_hour < 12 { "AM" } else { "PM" })
}

/// How long `secs` is, for people ("3 days, 4 hours").
pub fn duration(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (d, h, mi) = (s / 86400, s % 86400 / 3600, s % 3600 / 60);
    let unit = |n: u64, one: &str| format!("{n} {one}{}", if n == 1 { "" } else { "s" });
    match (d, h, mi) {
        (0, 0, 0) => unit(s, "second"),
        (0, 0, m) => unit(m, "minute"),
        (0, h, m) => format!("{}, {}", unit(h, "hour"), unit(m, "minute")),
        (d, h, _) => format!("{}, {}", unit(d, "day"), unit(h, "hour")),
    }
}

/// Wall-clock seconds for a sample time `t` (the app's clock).
fn wall(app: &Activity, t: f64) -> f64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    now - (app.t() - t)
}

fn ghz(mhz: f64) -> String {
    if mhz >= 1000.0 { format!("{:.2} GHz", mhz / 1000.0) } else { format!("{mhz:.0} MHz") }
}

/// The sidebar's figure for a section.
pub fn sidebar_value(app: &Activity, s: Section) -> String {
    let rate = |a: &str, b: &str| app.latest(a).unwrap_or(0.0) + app.latest(b).unwrap_or(0.0);
    match s {
        Section::Processes if !app.processes.is_empty() => app.processes.len().to_string(),
        Section::Cpu => app.latest("cpu").map(|v| format!("{v:.0}%")).unwrap_or_default(),
        Section::Memory => app.latest("memory").map(|v| format_size(v as u64)).unwrap_or_default(),
        Section::Disks => Unit::Rate.format(rate("disk-read", "disk-write")),
        Section::Network => Unit::Rate.format(rate("net-rx", "net-tx")),
        Section::Gpu => app.latest("gpu-busy").map(|v| format!("{v:.0}%")).unwrap_or_default(),
        Section::Sensors => match app.latest("battery") {
            Some(b) => format!("{b:.0}%"),
            None => hottest(app).map(|(_, v)| Unit::Celsius.format(v)).unwrap_or_default(),
        },
        _ => String::new(),
    }
}

/// The hottest temperature sensor now.
fn hottest(app: &Activity) -> Option<(String, f64)> {
    app.sources
        .iter()
        .enumerate()
        .filter(|(_, s)| s.unit == Unit::Celsius)
        .filter_map(|(i, s)| app.history[i].back().map(|(_, v)| (s.name.clone(), *v)).filter(|(_, v)| v.is_finite()))
        .max_by(|a, b| a.1.total_cmp(&b.1))
}

/// Draw section `s` in `rect`: cards on the gutter, scrolling.
pub fn show(app: &mut Activity, cx: &mut Cx, ui: &mut Ui, rect: Rect, s: Section) {
    dashboard::gutter(ui, rect);
    if s == Section::Graphs {
        return graphs(app, cx, ui, rect.shrink(GAP));
    }
    let p = style::palette(ui.ctx());
    let mut todo: Vec<Todo> = Vec::new();
    ScrollView::new(("activity-section", s)).border(false).background(style::card_paper(&p).0).show_in(ui, rect, |ui, _| {
        let area = Rect::from_min_max(ui.max_rect().min + vec2(GAP, GAP), pos2(ui.max_rect().right() - GAP, ui.max_rect().top() + 100_000.0));
        let used = match s {
            Section::Overview => overview(app, ui, area, &mut todo),
            Section::Cpu => cpu(app, ui, area, &mut todo),
            Section::Memory => memory(app, ui, area),
            Section::Disks => disks(app, ui, area, &mut todo),
            Section::Network => network(app, ui, area, &mut todo),
            Section::Gpu => gpu(app, ui, area, &mut todo),
            Section::Sensors => sensors(app, ui, area),
            Section::System => system(app, ui, area, &mut todo),
            Section::Processes | Section::Graphs => 0.0,
        };
        ui.allocate_rect(Rect::from_min_size(ui.max_rect().min, vec2(area.width(), used + 2.0 * GAP)), Sense::hover());
    });
    for t in todo {
        match t {
            Todo::Go(s) => app.set_section(s),
            Todo::Drill(t) => app.drill = t,
            Todo::Command(cmd) => app.command(cx, None, &cmd),
            Todo::Act(a) => app.act(cx, a),
            Todo::FullAccess => app.get_full_access(cx),
        }
    }
}

/// What a click asks for, done once drawing is over.
enum Todo {
    Go(Section),
    Drill(Option<f64>),
    Command(String),
    Act(Action),
    FullAccess,
}

// ---- Overview --------------------------------------------------------

const OVERVIEW_CARD: f32 = 196.0;

fn overview(app: &Activity, ui: &mut Ui, area: Rect, todo: &mut Vec<Todo>) -> f32 {
    let p = style::palette(ui.ctx());
    let sys = &app.system;
    let mut cards: Vec<Section> = vec![Section::Cpu, Section::Memory, Section::Network, Section::Disks];
    if app.source("gpu-busy").is_some() {
        cards.push(Section::Gpu);
    }
    if app.source("battery").is_some() || hottest(app).is_some() {
        cards.push(Section::Sensors);
    }
    let rects = flow(area, cards.len(), 250.0, 3, OVERVIEW_CARD);
    let mut bottom = area.top();
    for (s, r) in cards.iter().zip(&rects) {
        bottom = bottom.max(r.bottom());
        let mem_total = sys.memory.iter().find(|(k, _)| k == "MemTotal").map_or(0, |(_, v)| *v);
        let (title, figure, caption, keys, names, max, unit): (&str, String, String, Vec<&str>, Vec<&str>, Option<f64>, Unit) = match s {
            Section::Cpu => (
                "CPU",
                value(app, "cpu"),
                format!("{} cores{} · load {:.2}", sys.cores, app.latest("cpu-freq").map(|f| format!(" · {}", ghz(f))).unwrap_or_default(), sys.load[0]),
                vec!["cpu", "cpu-system", "cpu-iowait"],
                vec!["Busy", "System", "I/O wait"],
                Some(100.0),
                Unit::Percent,
            ),
            Section::Memory => (
                "Memory",
                value(app, "memory"),
                format!("of {} · {} swap used", format_size(mem_total), value(app, "swap")),
                vec!["memory", "cached"],
                vec!["Used", "Cached"],
                Some(mem_total as f64),
                Unit::Bytes,
            ),
            Section::Network => (
                "Network",
                Unit::Rate.format(app.latest("net-rx").unwrap_or(0.0) + app.latest("net-tx").unwrap_or(0.0)),
                sys.interfaces.iter().filter(|i| i.up).map(|i| i.name.clone()).collect::<Vec<_>>().join(", "),
                vec!["net-rx", "net-tx"],
                vec!["In", "Out"],
                None,
                Unit::Rate,
            ),
            Section::Disks => (
                "Disks",
                Unit::Rate.format(app.latest("disk-read").unwrap_or(0.0) + app.latest("disk-write").unwrap_or(0.0)),
                sys.volumes
                    .first()
                    .and_then(|v| Some(format!("{} free on {}", format_size(v.free?), v.mount.split(", ").next().unwrap_or(&v.mount))))
                    .unwrap_or_default(),
                vec!["disk-read", "disk-write"],
                vec!["Read", "Written"],
                None,
                Unit::Rate,
            ),
            Section::Gpu => (
                "GPU",
                value(app, "gpu-busy"),
                sys.gpu.as_ref().map(|g| format!("{} · {}", g.name, g.driver)).unwrap_or_default(),
                vec!["gpu-busy"],
                vec!["Busy"],
                Some(100.0),
                Unit::Percent,
            ),
            _ => match &sys.battery {
                Some(b) => (
                    "Battery",
                    value(app, "battery"),
                    format!("{} · {}", b.status, Unit::Watts.format(b.watts.abs())),
                    vec!["battery"],
                    vec!["Charge"],
                    Some(100.0),
                    Unit::Percent,
                ),
                None => {
                    let (name, v) = hottest(app).unwrap_or_default();
                    ("Temperature", Unit::Celsius.format(v), format!("Hottest: {name}"), vec![], vec![], None, Unit::Celsius)
                }
            },
        };
        let card = Card::new(title).figure(&figure).caption(&caption).clickable(true).show(ui, *r);
        if card.clicked {
            todo.push(Todo::Go(*s));
        }
        let mut items = legend_items(app, &p, &keys, &names);
        if *s == Section::Gpu {
            items.extend(legend_items(app, &p, &["gpu-power"], &["Power"]));
        }
        if *s == Section::Sensors && sys.battery.is_some() {
            items.extend(legend_items(app, &p, &["battery-power"], &["Power"]));
        }
        let lh = legend_height(ui, card.content.width(), &items);
        let g = Rect::from_min_max(card.content.min, pos2(card.content.right(), card.content.bottom() - lh - m::TIGHT));
        if !keys.is_empty() {
            if let Some(t) = graph(app, ui, &format!("overview-{title}"), g, &keys, max, unit) {
                todo.push(Todo::Drill(Some(t)));
            }
        } else if let Some(Section::Sensors) = Some(*s) {
            // No battery: the hottest few sensors' graph.
            let mut temps: Vec<(usize, f64)> =
                app.sources.iter().enumerate().filter(|(_, s)| s.unit == Unit::Celsius).filter_map(|(i, _)| Some((i, app.history[i].back()?.1))).collect();
            temps.sort_by(|a, b| b.1.total_cmp(&a.1));
            let keys: Vec<&str> = temps.iter().take(3).map(|(i, _)| app.sources[*i].key.as_str()).collect();
            let _ = graph(app, ui, "overview-temps", g, &keys, None, Unit::Celsius);
        }
        legend(ui, Rect::from_min_max(pos2(card.content.left(), card.content.bottom() - lh), card.content.max), &items);
    }
    let top = bottom + GAP;
    top - area.top() + busiest(app, ui, Rect::from_min_size(pos2(area.left(), top), vec2(area.width(), 0.0)), todo)
}

/// A process and its figure: (pid, name, figure).
type Busy = (i32, String, String);

/// The busiest processes now, or at the moment chosen on a graph.
fn busiest(app: &Activity, ui: &mut Ui, area: Rect, todo: &mut Vec<Todo>) -> f32 {
    let rows = 6;
    let h = Card::height(rows as f32 * dashboard::FACT_ROW + dashboard::FACT_ROW, true);
    let r = Rect::from_min_size(area.min, vec2(area.width(), h));
    let moment = app.drill.and_then(|t| app.moments.iter().rev().find(|m| m.at <= t + 0.01).or(app.moments.front()));
    let (title, caption) = match moment {
        Some(m) => {
            let ago = app.t() - m.at;
            (format!("At {}", clock(wall(app, m.at))), format!("{} ago · click a graph for another moment", duration(ago)))
        }
        None => ("Busiest Now".to_owned(), "Click a graph to see what was busy then".to_owned()),
    };
    let card = Card::new(&title).caption(&caption).show(ui, r);
    if moment.is_some() {
        let b = Rect::from_min_size(pos2(r.right() - m::CARD_INSET - m::BUTTON_MIN_WIDTH, r.top() + m::CARD_INSET - 2.0), vec2(m::BUTTON_MIN_WIDTH, m::CONTROL_HEIGHT));
        if ui.put(b, Button::new("Now").min_width(b.width())).clicked() {
            todo.push(Todo::Drill(None));
        }
    }
    let half = ((card.content.width() - 2.0 * m::SPACING) / 2.0).floor();
    let left = Rect::from_min_size(card.content.min, vec2(half, card.content.height()));
    let right = Rect::from_min_size(pos2(card.content.left() + half + 2.0 * m::SPACING, card.content.top()), vec2(card.content.width() - half - 2.0 * m::SPACING, card.content.height()));
    let (cpu, other, other_title): (Vec<Busy>, Vec<Busy>, &str) = match moment {
        Some(m) => (
            m.cpu.iter().take(rows).map(|(pid, n, v)| (*pid, n.clone(), format!("{v:.1}%"))).collect(),
            m.disk.iter().take(rows).map(|(pid, n, v)| (*pid, n.clone(), Unit::Rate.format(*v))).collect(),
            "Disk",
        ),
        None => {
            let mut by_cpu: Vec<&crate::sample::Proc> = app.processes.iter().collect();
            by_cpu.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
            let mut by_mem: Vec<&crate::sample::Proc> = app.processes.iter().collect();
            by_mem.sort_by_key(|p| std::cmp::Reverse(p.memory));
            (
                by_cpu.iter().take(rows).map(|p| (p.pid, p.name.clone(), format!("{:.1}%", p.cpu))).collect(),
                by_mem.iter().take(rows).map(|p| (p.pid, p.name.clone(), format_size(p.memory))).collect(),
                "Memory",
            )
        }
    };
    for (col, list, head) in [(left, &cpu, "CPU"), (right, &other, other_title)] {
        process_rows(ui, col, head, list, todo);
    }
    h
}

/// A small list of processes (name and a figure), each a way to it in
/// Processes.
fn process_rows(ui: &mut Ui, r: Rect, head: &str, rows: &[(i32, String, String)], todo: &mut Vec<Todo>) {
    let p = style::palette(ui.ctx());
    let ink = style::card_ink(&p);
    let row_h = dashboard::FACT_ROW;
    style::label(ui.painter(), r.left(), r.top() + row_h / 2.0, Align::Min, head, style::small_font(), ink.dim);
    for (i, (pid, name, fig)) in rows.iter().enumerate() {
        let rr = Rect::from_min_size(pos2(r.left(), r.top() + (i + 1) as f32 * row_h), vec2(r.width(), row_h));
        let resp = ui.interact(rr, Id::unique(("busiest", head, i)), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(rr.expand2(vec2(4.0, 0.0)), 3.0, ink.hover);
        }
        let fw = style::label_width(ui.ctx(), fig, style::plain_font());
        style::label_fit(ui.painter(), rr.left(), rr.center().y, Align::Min, name, style::plain_font(), ink.text, rr.width() - fw - m::SPACING);
        style::label(ui.painter(), rr.right(), rr.center().y, Align::Max, fig, style::plain_font(), ink.text);
        resp.widget_info(|| egui::WidgetInfo { current_text_value: Some(fig.clone()), ..egui::WidgetInfo::labeled(egui::Role::Button, true, name) });
        if resp.clicked() {
            todo.push(Todo::Command(format!("select:{pid}")));
        }
    }
}

// ---- CPU -------------------------------------------------------------

fn cpu(app: &Activity, ui: &mut Ui, area: Rect, todo: &mut Vec<Todo>) -> f32 {
    let p = style::palette(ui.ctx());
    let sys = &app.system;
    let mut y = area.top();
    // Everything.
    let keys = ["cpu", "cpu-user", "cpu-system", "cpu-iowait"];
    let items = legend_items(app, &p, &keys, &["Busy", "User", "System", "I/O wait"]);
    let lh = legend_height(ui, area.width() - 2.0 * m::CARD_INSET, &items);
    let caption = if sys.cpu_model.is_empty() { format!("{} cores", sys.cores) } else { sys.cpu_model.clone() };
    let h = Card::height(GRAPH + 40.0 + m::TIGHT + lh, true);
    let card = Card::new("CPU").figure(&value(app, "cpu")).caption(&caption).show(ui, Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h)));
    let g = Rect::from_min_size(card.content.min, vec2(card.content.width(), GRAPH + 40.0));
    if let Some(t) = graph(app, ui, "cpu-all", g, &keys, Some(100.0), Unit::Percent) {
        todo.push(Todo::Drill(Some(t)));
    }
    legend(ui, Rect::from_min_size(pos2(card.content.left(), g.bottom() + m::TIGHT), vec2(card.content.width(), lh)), &items);
    y += h + GAP;

    // Each core.
    let cores: Vec<usize> = app.sources.iter().enumerate().filter(|(_, s)| s.group == Group::Cpus).map(|(i, _)| i).collect();
    let inner_w = area.width() - 2.0 * m::CARD_INSET;
    let (_, grid_h) = mini_grid(0.0, inner_w, 0.0, cores.len());
    let h = Card::height(grid_h, false);
    let card = Card::new("Cores").show(ui, Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h)));
    let (cells, _) = mini_grid(card.content.left(), card.content.width(), card.content.top(), cores.len());
    for (k, (&i, r)) in cores.iter().zip(&cells).enumerate() {
        let freq = app.latest(&format!("freq{k}")).map(|f| format!(" · {}", ghz(f))).unwrap_or_default();
        let label = format!("{}{freq}", app.sources[i].name);
        mini(app, ui, *r, &app.sources[i].key.clone(), &label, Some(100.0));
    }
    y += h + GAP;

    // Clusters: frequencies and governors.
    if !sys.policies.is_empty() {
        let rows = sys.policies.len();
        let freq_keys: Vec<String> = sys.policies.iter().filter_map(|pol| pol.cores.first().map(|c| format!("freq{c}"))).collect();
        let fk: Vec<&str> = freq_keys.iter().map(String::as_str).collect();
        let names: Vec<String> = sys.policies.iter().map(cluster_name).collect();
        let nk: Vec<&str> = names.iter().map(String::as_str).collect();
        let items = legend_items(app, &p, &fk, &nk);
        let lh = legend_height(ui, inner_w, &items);
        let h = Card::height(rows as f32 * (m::CONTROL_HEIGHT + m::TIGHT) + m::TIGHT + GRAPH + m::TIGHT + lh, true);
        let card = Card::new("Frequencies").caption("Each cluster's cores change speed together; the governor decides how").show(ui, Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h)));
        let ink = style::card_ink(&p);
        for (k, pol) in sys.policies.iter().enumerate() {
            let row = Rect::from_min_size(pos2(card.content.left(), card.content.top() + k as f32 * (m::CONTROL_HEIGHT + m::TIGHT)), vec2(card.content.width(), m::CONTROL_HEIGHT));
            let label = format!("{}: {} – {}, now {}", names[k], ghz(pol.min), ghz(pol.max), ghz(pol.now));
            let field_w = 220.0f32.min(row.width() / 2.0);
            style::label_fit(ui.painter(), row.left(), row.center().y, Align::Min, &label, style::plain_font(), ink.text, row.width() - field_w - m::SPACING);
            let mut chosen = pol.governors.iter().position(|g| *g == pol.governor).unwrap_or(0);
            let options: Vec<&str> = pol.governors.iter().map(String::as_str).collect();
            let field = Rect::from_min_size(pos2(row.right() - field_w, row.top()), vec2(field_w, m::CONTROL_HEIGHT));
            if !options.is_empty() && ui.put(field, MenuField::new(("governor", pol.id), &mut chosen, &options).label("Governor:").width(field_w)).changed() {
                todo.push(Todo::Act(Action::Governor { policy: pol.id, governor: pol.governors[chosen].clone() }));
            }
        }
        let g = Rect::from_min_size(pos2(card.content.left(), card.content.top() + rows as f32 * (m::CONTROL_HEIGHT + m::TIGHT) + m::TIGHT), vec2(card.content.width(), GRAPH));
        let _ = graph(app, ui, "cpu-freqs", g, &fk, None, Unit::Megahertz);
        legend(ui, Rect::from_min_size(pos2(card.content.left(), g.bottom() + m::TIGHT), vec2(card.content.width(), lh)), &items);
        y += h + GAP;
    }

    // Idle states: how deeply the cores sleep when there's nothing to do.
    if !sys.idle_states.is_empty() {
        let busy = app.latest("cpu").unwrap_or(0.0);
        let mut rows: Vec<(String, String)> = vec![("Running".into(), format!("{busy:.1}% of the time"))];
        for st in &sys.idle_states {
            let what = if st.what.is_empty() { st.name.clone() } else { format!("{} ({})", st.name, st.what) };
            rows.push((what, format!("{:.1}% of the time, waking in {} µs", st.share, st.latency_us)));
        }
        let rr: Vec<(&str, &str)> = rows.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let fh = dashboard::facts_height(rr.len());
        let h = Card::height(10.0 + m::SPACING + fh, true);
        let card = Card::new("Idle States").caption("Where the cores' time goes: the deeper a state, the less power and the slower to wake").show(ui, Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h)));
        let hues = [olo_theme::Hue::Blue, olo_theme::Hue::Teal, olo_theme::Hue::Green, olo_theme::Hue::Purple, olo_theme::Hue::Gray];
        let mut parts = vec![(c(p.hue(hues[0])), busy)];
        parts.extend(sys.idle_states.iter().enumerate().map(|(i, st)| (c(p.hue(hues[(i + 1) % hues.len()])), st.share)));
        meter(ui, Rect::from_min_size(card.content.min, vec2(card.content.width(), 10.0)), &parts, 100.0);
        facts(ui, Rect::from_min_size(pos2(card.content.left(), card.content.top() + 10.0 + m::SPACING), vec2(card.content.width(), fh)), &rr);
        y += h + GAP;
    }

    // Load.
    let keys = ["load", "switches", "interrupts"];
    let rows: Vec<(&str, String)> = vec![
        ("Load average", format!("{:.2}, {:.2}, {:.2} (1, 5 and 15 minutes)", sys.load[0], sys.load[1], sys.load[2])),
        ("Processes", value(app, "processes")),
        ("Threads", value(app, "threads")),
        ("Context switches", value(app, "switches")),
        ("Interrupts", value(app, "interrupts")),
        ("Page faults", value(app, "faults")),
    ];
    let rr: Vec<(&str, &str)> = rows.iter().map(|(a, b)| (*a, b.as_str())).collect();
    let fh = dashboard::facts_height(rr.len().div_ceil(2));
    let h = Card::height(fh + m::SPACING + GRAPH, false);
    let card = Card::new("Load").show(ui, Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h)));
    facts_two(ui, card.content, &rr);
    let g = Rect::from_min_size(pos2(card.content.left(), card.content.top() + fh + m::SPACING), vec2(card.content.width(), GRAPH));
    let _ = graph(app, ui, "cpu-load", g, &keys[..1], None, Unit::Load);
    y += h;
    y - area.top()
}

fn cluster_name(pol: &crate::sample::Policy) -> String {
    match (pol.cores.first(), pol.cores.last()) {
        (Some(a), Some(b)) if a != b => format!("Cores {}–{}", a + 1, b + 1),
        (Some(a), _) => format!("Core {}", a + 1),
        _ => format!("Cluster {}", pol.id),
    }
}

// ---- Memory ----------------------------------------------------------

fn memory(app: &Activity, ui: &mut Ui, area: Rect) -> f32 {
    let p = style::palette(ui.ctx());
    let sys = &app.system;
    let field = |k: &str| sys.memory.iter().find(|(n, _)| n == k).map_or(0, |(_, v)| *v);
    let total = field("MemTotal");
    let mut y = area.top();
    let keys = ["memory", "cached", "swap"];
    let items = legend_items(app, &p, &keys, &["Used", "Cached", "Swap"]);
    let lh = legend_height(ui, area.width() - 2.0 * m::CARD_INSET, &items);
    let caption = format!("of {} · {} available", format_size(total), format_size(field("MemAvailable")));
    let h = Card::height(GRAPH + 40.0 + m::SPACING + 10.0 + m::TIGHT + lh, true);
    let card = Card::new("Memory").figure(&value(app, "memory")).caption(&caption).show(ui, Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h)));
    let g = Rect::from_min_size(card.content.min, vec2(card.content.width(), GRAPH + 40.0));
    let _ = graph(app, ui, "memory-all", g, &keys, Some(total as f64), Unit::Bytes);
    let used = app.latest("memory").unwrap_or(0.0);
    let cached = app.latest("cached").unwrap_or(0.0);
    let bar = Rect::from_min_size(pos2(card.content.left(), g.bottom() + m::SPACING), vec2(card.content.width(), 10.0));
    let hue = |k: &str| app.source(k).map_or(Color32::GRAY, |i| c(p.hue(app.sources[i].color)));
    meter(ui, bar, &[(hue("memory"), used), (hue("cached"), cached)], total as f64);
    legend(ui, Rect::from_min_size(pos2(card.content.left(), bar.bottom() + m::TIGHT), vec2(card.content.width(), lh)), &items);
    y += h + GAP;

    // zram and pressure, side by side when there's room.
    let pair = flow(Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), 0.0)), 2, 300.0, 2, 0.0);
    let zram_rows: Vec<(String, String)> = sys
        .zram
        .iter()
        .flat_map(|z| {
            let ratio = if z.compressed > 0 { z.original as f64 / z.compressed as f64 } else { 0.0 };
            [
                (format!("{} holds", z.name), format!("{} of {}", format_size(z.original), format_size(z.disk_size))),
                ("Compressed to".into(), format!("{} ({ratio:.1} : 1, {})", format_size(z.compressed), z.algorithm)),
                ("Using".into(), format!("{} of memory", format_size(z.used))),
            ]
        })
        .collect();
    let mut rows: Vec<(String, String)> = zram_rows;
    if rows.is_empty() {
        rows.push(("Swap".into(), format!("{} of {}", value(app, "swap"), format_size(field("SwapTotal")))));
    }
    let rr: Vec<(&str, &str)> = rows.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let pressure = sys.pressure.iter().find(|p| p.resource == "memory");
    let prow: Vec<(String, String)> = pressure
        .map(|pr| {
            let mut v = vec![("Some waiting".to_owned(), format!("{:.1}%, {:.1}%, {:.1}%", pr.some[0], pr.some[1], pr.some[2]))];
            if let Some(f) = pr.full {
                v.push(("All waiting".into(), format!("{:.1}%, {:.1}%, {:.1}%", f[0], f[1], f[2])));
            }
            v
        })
        .unwrap_or_default();
    let pr: Vec<(&str, &str)> = prow.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let side = pair[0].top() != pair[1].top();
    let zh = Card::height(dashboard::facts_height(rr.len()), true);
    let ph = Card::height(dashboard::facts_height(pr.len()) + m::SPACING + GRAPH * 0.6, true);
    let (zr, prr) = if side {
        (Rect::from_min_size(pair[0].min, vec2(area.width(), zh)), Rect::from_min_size(pos2(area.left(), y + zh + GAP), vec2(area.width(), ph)))
    } else {
        let h = zh.max(ph);
        (Rect::from_min_size(pair[0].min, vec2(pair[0].width(), h)), Rect::from_min_size(pair[1].min, vec2(pair[1].width(), h)))
    };
    let zcap = if sys.zram.is_empty() { "No compressed swap" } else { "Swap in memory, compressed" };
    let card = Card::new("Compressed Swap").caption(zcap).show(ui, zr);
    facts(ui, card.content, &rr);
    let card = Card::new("Memory Pressure").caption("Time tasks waited for memory: 10 s, 1 min, 5 min").show(ui, prr);
    facts(ui, card.content, &pr);
    let g = Rect::from_min_max(pos2(card.content.left(), card.content.top() + dashboard::facts_height(pr.len()) + m::SPACING), card.content.max);
    let _ = graph(app, ui, "memory-psi", g, &["psi-memory", "psi-memory-full"], None, Unit::Percent);
    y = zr.bottom().max(prr.bottom()) + GAP;

    // Every counter the kernel keeps.
    let all: Vec<(String, String)> = sys.memory.iter().map(|(k, v)| (k.clone(), if k.starts_with("HugePages_") { v.to_string() } else { format_size(*v) })).collect();
    let ar: Vec<(&str, &str)> = all.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let h = Card::height(dashboard::facts_height(ar.len().div_ceil(2)), true);
    let card = Card::new("Every Counter").caption("/proc/meminfo").show(ui, Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h)));
    facts_two(ui, card.content, &ar);
    y + h - area.top()
}

// ---- Disks -----------------------------------------------------------

fn disks(app: &Activity, ui: &mut Ui, area: Rect, todo: &mut Vec<Todo>) -> f32 {
    let p = style::palette(ui.ctx());
    let sys = &app.system;
    let mut y = area.top();
    for d in &sys.disks {
        let keys = [format!("disk:{}:read", d.name), format!("disk:{}:write", d.name), format!("disk:{}:busy", d.name)];
        let k: Vec<&str> = keys.iter().map(String::as_str).collect();
        let mut items = legend_items(app, &p, &k[..2], &["Read", "Written"]);
        items.push(LegendItem { color: Color32::TRANSPARENT, name: "Busy", value: value(app, &keys[2]) });
        items.push(LegendItem { color: Color32::TRANSPARENT, name: "Requests", value: format!("{:.0}/s", d.iops) });
        items.push(LegendItem { color: Color32::TRANSPARENT, name: "Each takes", value: format!("{:.2} ms", d.latency_ms) });
        let lh = legend_height(ui, area.width() - 2.0 * m::CARD_INSET, &items);
        // Its own account of its health (NVMe), with full access.
        let ctrl = crate::smart::controller_of(&d.name);
        let health = ctrl.as_ref().and_then(|c| sys.health.iter().find(|(n, _)| n == c)).map(|(_, h)| h.clone());
        let hrows: Vec<(String, String)> = match &health {
            Some(Some(h)) => vec![
                ("Health".into(), format!("{}, {}% of its life used", h.verdict(), h.used)),
                ("Spare".into(), format!("{}% left (warns under {}%)", h.spare, h.spare_threshold)),
                ("Written".into(), format_size(h.written_bytes.min(u64::MAX as u128) as u64)),
                ("Read".into(), format_size(h.read_bytes.min(u64::MAX as u128) as u64)),
                ("Powered on".into(), format!("{} hours, {} times", h.power_on_hours, h.power_cycles)),
                ("Unsafe shutdowns".into(), h.unsafe_shutdowns.to_string()),
                ("Media errors".into(), h.media_errors.to_string()),
                ("Temperature".into(), Unit::Celsius.format(h.temperature)),
            ],
            Some(None) => vec![("Health".into(), "Needs full access".into())],
            None => Vec::new(),
        };
        let hr: Vec<(&str, &str)> = hrows.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let hh = if hr.is_empty() { 0.0 } else { m::SPACING + dashboard::facts_height(hr.len().div_ceil(2)) };
        let kind = if d.rotational { "hard disk" } else if d.removable { "removable" } else { "solid-state" };
        let caption = format!("{} · {} {kind}", if d.model.is_empty() { "Disk" } else { &d.model }, format_size(d.size));
        let figure = Unit::Rate.format(app.latest(&keys[0]).unwrap_or(0.0) + app.latest(&keys[1]).unwrap_or(0.0));
        let h = Card::height(GRAPH + m::TIGHT + lh + hh, true);
        let card = Card::new(&d.name).figure(&figure).caption(&caption).show(ui, Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h)));
        let g = Rect::from_min_size(card.content.min, vec2(card.content.width(), GRAPH));
        if let Some(t) = graph(app, ui, &keys[0], g, &k[..2], None, Unit::Rate) {
            todo.push(Todo::Drill(Some(t)));
        }
        legend(ui, Rect::from_min_size(pos2(card.content.left(), g.bottom() + m::TIGHT), vec2(card.content.width(), lh)), &items);
        if !hr.is_empty() {
            facts_two(ui, Rect::from_min_max(pos2(card.content.left(), g.bottom() + m::TIGHT + lh + m::SPACING), card.content.max), &hr);
        }
        y += h + GAP;
    }
    // Volumes.
    let ink = style::card_ink(&p);
    let row = 40.0;
    let h = Card::height(sys.volumes.len() as f32 * row, false);
    let card = Card::new("Volumes").show(ui, Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h)));
    for (i, v) in sys.volumes.iter().enumerate() {
        let r = Rect::from_min_size(pos2(card.content.left(), card.content.top() + i as f32 * row), vec2(card.content.width(), row));
        let title = format!("{}{}", v.mount, if v.read_only { " (read-only)" } else { "" });
        let detail = match (v.size, v.free) {
            (Some(s), Some(f)) => format!("{} free of {}", format_size(f), format_size(s)),
            _ => "On the network".into(),
        };
        let dw = style::label_width(ui.ctx(), &detail, style::plain_font());
        style::label_fit(ui.painter(), r.left(), r.top() + 9.0, Align::Min, &title, style::bold_font(), ink.text, r.width() - dw - m::SPACING);
        style::label(ui.painter(), r.right(), r.top() + 9.0, Align::Max, &detail, style::plain_font(), ink.text);
        let sub = format!("{} · {}", v.kind, v.device);
        style::label_fit(ui.painter(), r.left(), r.top() + 24.0, Align::Min, &sub, style::small_font(), ink.dim, r.width() * 0.5);
        if let (Some(s), Some(f)) = (v.size, v.free) {
            let bar = Rect::from_min_max(pos2(r.left() + r.width() * 0.5, r.top() + 21.0), pos2(r.right(), r.top() + 27.0));
            let used = s.saturating_sub(f) as f64;
            let full = used / s.max(1) as f64;
            let hue = if full > 0.9 { p.hue(olo_theme::Hue::Red) } else { p.hue(olo_theme::Hue::Blue) };
            meter(ui, bar, &[(c(hue), used)], s as f64);
        }
    }
    y += h + GAP;
    let items = legend_items(app, &p, &["psi-io", "psi-io-full"], &["Some waiting", "All waiting"]);
    let lh = legend_height(ui, area.width() - 2.0 * m::CARD_INSET, &items);
    let h = Card::height(GRAPH * 0.7 + m::TIGHT + lh, true);
    let card = Card::new("I/O Pressure").caption("Time tasks waited for disks").show(ui, Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h)));
    let g = Rect::from_min_size(card.content.min, vec2(card.content.width(), GRAPH * 0.7));
    let _ = graph(app, ui, "io-psi", g, &["psi-io", "psi-io-full"], None, Unit::Percent);
    legend(ui, Rect::from_min_size(pos2(card.content.left(), g.bottom() + m::TIGHT), vec2(card.content.width(), lh)), &items);
    y + h - area.top()
}

// ---- Network ---------------------------------------------------------

const CONNECTIONS_HEIGHT: f32 = 320.0;

fn network(app: &mut Activity, ui: &mut Ui, area: Rect, todo: &mut Vec<Todo>) -> f32 {
    let p = style::palette(ui.ctx());
    let mut y = area.top();
    let interfaces = app.system.interfaces.clone();
    for n in &interfaces {
        let keys = [format!("net:{}:rx", n.name), format!("net:{}:tx", n.name)];
        let k: Vec<&str> = keys.iter().map(String::as_str).collect();
        let mut items = legend_items(app, &p, &k, &["In", "Out"]);
        items.push(LegendItem { color: Color32::TRANSPARENT, name: "Received", value: format_size(n.received) });
        items.push(LegendItem { color: Color32::TRANSPARENT, name: "Sent", value: format_size(n.sent) });
        if n.errors + n.drops > 0 {
            items.push(LegendItem { color: Color32::TRANSPARENT, name: "Errors", value: n.errors.to_string() });
            items.push(LegendItem { color: Color32::TRANSPARENT, name: "Dropped", value: n.drops.to_string() });
        }
        let lh = legend_height(ui, area.width() - 2.0 * m::CARD_INSET, &items);
        let mut cap = vec![if n.up { "Connected".to_owned() } else { "Not connected".to_owned() }];
        cap.extend(n.addresses.iter().filter(|a| !a.starts_with("fe80")).cloned());
        if !n.mac.is_empty() && n.mac != "00:00:00:00:00:00" {
            cap.push(n.mac.clone());
        }
        cap.push(format!("MTU {}", n.mtu));
        let caption = cap.join(" · ");
        let figure = Unit::Rate.format(app.latest(&keys[0]).unwrap_or(0.0) + app.latest(&keys[1]).unwrap_or(0.0));
        let h = Card::height(GRAPH * 0.8 + m::TIGHT + lh, true);
        let card = Card::new(&n.name).figure(&figure).caption(&caption).show(ui, Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h)));
        let g = Rect::from_min_size(card.content.min, vec2(card.content.width(), GRAPH * 0.8));
        if let Some(t) = graph(app, ui, &keys[0], g, &k, None, Unit::Rate) {
            todo.push(Todo::Drill(Some(t)));
        }
        legend(ui, Rect::from_min_size(pos2(card.content.left(), g.bottom() + m::TIGHT), vec2(card.content.width(), lh)), &items);
        y += h + GAP;
    }
    // Connections: who talks to whom.
    let h = Card::height(m::CONTROL_HEIGHT + m::SPACING + CONNECTIONS_HEIGHT, true);
    let full = app.access == Access::Full;
    let caption = if full { "Every program's" } else { "Programs of other users show no name without full access" };
    let card = Card::new("Connections").caption(caption).show(ui, Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h)));
    let field = Rect::from_min_size(card.content.min, vec2(card.content.width().min(320.0), m::CONTROL_HEIGHT));
    ui.put(field, TextControl::new(&mut app.connections_search).label("Search:").hint("Address, port, program or state"));
    let list = Rect::from_min_max(pos2(card.content.left(), field.bottom() + m::SPACING), card.content.max);
    let needle = olo_kit::search::Needle::new(app.connections_search.trim());
    let mut rows: Vec<&crate::sample::Connection> = app
        .connections
        .iter()
        .filter(|cn| needle.is_empty() || [cn.local.as_str(), &cn.remote, &cn.state, &cn.process, cn.proto.as_str()].iter().any(|f| needle.in_text(f)))
        .collect();
    let (col, asc) = app.settings.connections_sort.unwrap_or((4, true));
    rows.sort_by(|a, b| {
        let o = match col {
            0 => a.proto.cmp(&b.proto),
            1 => a.local.cmp(&b.local),
            2 => a.remote.cmp(&b.remote),
            3 => a.state.cmp(&b.state),
            _ => a.process.to_lowercase().cmp(&b.process.to_lowercase()),
        };
        let o = o.then(a.local.cmp(&b.local));
        if asc { o } else { o.reverse() }
    });
    let columns = [Column::new("Protocol", 64.0), Column::new("Local", 170.0), Column::new("Remote", 170.0), Column::new("State", 90.0), Column::new("Program", 150.0)];
    app.connections_list.sort = Some((col, asc));
    let cells: Vec<[String; 5]> = rows
        .iter()
        .map(|cn| {
            let prog = match cn.pid {
                Some(pid) => format!("{} ({pid})", cn.process),
                None => "—".into(),
            };
            // Not connected to anything (listening, or UDP unconnected).
            let remote = if cn.remote.ends_with(":0") && (cn.remote.starts_with("0.0.0.0") || cn.remote.starts_with("[::]")) { "—".into() } else { cn.remote.clone() };
            [cn.proto.clone(), cn.local.clone(), remote, cn.state.clone(), prog]
        })
        .collect();
    let pids: Vec<Option<i32>> = rows.iter().map(|cn| cn.pid).collect();
    let out = ui
        .scope_builder(egui::UiBuilder::new().max_rect(list), |ui| {
            ColumnListView::new("activity-connections", &columns, cells.len(), &mut app.connections_list)
                .empty_text("No connections match.")
                .show(ui, |cell: &Cell| cell.text(&cells[cell.row][cell.column]))
        })
        .inner;
    if out.sort_changed {
        app.settings.connections_sort = app.connections_list.sort;
    }
    if let Some(row) = out.activated
        && let Some(Some(pid)) = pids.get(row)
    {
        todo.push(Todo::Command(format!("inspect:{pid}")));
    }
    y + h - area.top()
}

// ---- GPU -------------------------------------------------------------

fn gpu(app: &Activity, ui: &mut Ui, area: Rect, todo: &mut Vec<Todo>) -> f32 {
    let p = style::palette(ui.ctx());
    let sys = &app.system;
    let mut y = area.top();
    let g_info = sys.gpu.clone().unwrap_or_default();
    let keys = ["gpu-busy"];
    let mut items = legend_items(app, &p, &keys, &["Busy"]);
    items.extend(legend_items(app, &p, &["gpu-jobs"], &["Jobs"]));
    let lh = legend_height(ui, area.width() - 2.0 * m::CARD_INSET, &items);
    let h = Card::height(GRAPH + 40.0 + m::TIGHT + lh, true);
    let caption = format!("{} · {} driver", g_info.name, g_info.driver);
    let card = Card::new("GPU").figure(&value(app, "gpu-busy")).caption(&caption).show(ui, Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h)));
    let g = Rect::from_min_size(card.content.min, vec2(card.content.width(), GRAPH + 40.0));
    if let Some(t) = graph(app, ui, "gpu-busy", g, &keys, Some(100.0), Unit::Percent) {
        todo.push(Todo::Drill(Some(t)));
    }
    legend(ui, Rect::from_min_size(pos2(card.content.left(), g.bottom() + m::TIGHT), vec2(card.content.width(), lh)), &items);
    y += h + GAP;
    let pair = flow(Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), 0.0)), 2, 300.0, 2, Card::height(GRAPH * 0.8 + 16.0, true));
    if app.source("gpu-power").is_some() {
        let card = Card::new("Power").figure(&value(app, "gpu-power")).caption("What the GPU draws").show(ui, pair[0]);
        let _ = graph(app, ui, "gpu-power", card.content, &["gpu-power"], None, Unit::Watts);
    }
    if app.source("gpu-memory").is_some() {
        let caption = if g_info.memory_limit > 0 { format!("of {} it may use", format_size(g_info.memory_limit)) } else { String::new() };
        let card = Card::new("Memory").figure(&value(app, "gpu-memory")).caption(&caption).show(ui, pair[1]);
        let bar = Rect::from_min_size(card.content.min, vec2(card.content.width(), 8.0));
        if g_info.memory_limit > 0 {
            let hue = c(p.hue(olo_theme::Hue::Red));
            meter(ui, bar, &[(hue, g_info.memory_used as f64)], g_info.memory_limit as f64);
        }
        let g = Rect::from_min_max(pos2(card.content.left(), bar.bottom() + m::SPACING), card.content.max);
        let _ = graph(app, ui, "gpu-memory", g, &["gpu-memory"], None, Unit::Bytes);
    }
    y = pair[1].bottom() + GAP;
    // Who uses it.
    let clients = sys.gpu_clients.clone().unwrap_or_default();
    let rows: Vec<(i32, String, String)> = clients
        .iter()
        .map(|cl| {
            let busy = cl.busy.map(|b| format!(" · {b:.0}%")).unwrap_or_default();
            (cl.pid, format!("{} ({})", cl.name, cl.pid), format!("{}{busy}", format_size(cl.memory)))
        })
        .collect();
    let h = Card::height((rows.len().max(1) + 1) as f32 * dashboard::FACT_ROW, true);
    let card = Card::new("Programs Using the GPU")
        .caption(if app.access == Access::Full { "Its memory each holds" } else { "Its memory each holds (other users' need full access)" })
        .show(ui, Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h)));
    process_rows(ui, card.content, "Memory on the GPU", &rows, todo);
    y + h - area.top()
}

// ---- Sensors ---------------------------------------------------------

fn sensors(app: &Activity, ui: &mut Ui, area: Rect) -> f32 {
    let p = style::palette(ui.ctx());
    let sys = &app.system;
    let mut y = area.top();
    if let Some(b) = &sys.battery {
        let mut rows: Vec<(&str, String)> = vec![("Status", b.status.clone())];
        if !b.health.is_empty() {
            rows.push(("Health", b.health.clone()));
        }
        if b.design > 0.0 {
            rows.push(("Holds", format!("{:.1} Wh of {:.1} Wh new ({:.0}%)", b.full, b.design, b.full / b.design * 100.0)));
        }
        rows.push(("Now", format!("{:.1} Wh", b.energy)));
        rows.push(("Cycles", b.cycles.to_string()));
        rows.push(("Power", Unit::Watts.format(b.watts.abs())));
        rows.push(("Voltage", format!("{:.2} V", b.volts)));
        if b.watts.abs() > 0.5 {
            let hours = if b.status == "Charging" { (b.full - b.energy) / b.watts.abs() } else { b.energy / b.watts.abs() };
            let label = if b.status == "Charging" { "Full in" } else { "Lasts" };
            rows.push((label, duration(hours * 3600.0)));
        }
        let rr: Vec<(&str, &str)> = rows.iter().map(|(a, b)| (*a, b.as_str())).collect();
        let fh = dashboard::facts_height(rr.len().div_ceil(2));
        let h = Card::height(10.0 + m::SPACING + fh + m::SPACING + GRAPH, true);
        let card = Card::new("Battery").figure(&value(app, "battery")).caption(&b.model).show(ui, Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h)));
        let bar = Rect::from_min_size(card.content.min, vec2(card.content.width(), 10.0));
        let hue = if b.capacity < 15.0 { olo_theme::Hue::Red } else { olo_theme::Hue::Green };
        meter(ui, bar, &[(c(p.hue(hue)), b.capacity)], 100.0);
        facts_two(ui, Rect::from_min_size(pos2(card.content.left(), bar.bottom() + m::SPACING), vec2(card.content.width(), fh)), &rr);
        let g = Rect::from_min_max(pos2(card.content.left(), bar.bottom() + m::SPACING + fh + m::SPACING), card.content.max);
        let _ = graph(app, ui, "battery", g, &["battery"], Some(100.0), Unit::Percent);
        y += h + GAP;
    }
    for (unit, title, caption) in [(Unit::Celsius, "Temperatures", "Every thermal sensor"), (Unit::Watts, "Power", "What each part draws"), (Unit::Rpm, "Fans", "")] {
        let keys: Vec<(String, String)> =
            app.sources.iter().filter(|s| s.group == Group::Sensors && s.unit == unit).map(|s| (s.key.clone(), s.name.clone())).collect();
        if keys.is_empty() {
            continue;
        }
        let inner_w = area.width() - 2.0 * m::CARD_INSET;
        let (_, gh) = mini_grid(0.0, inner_w, 0.0, keys.len());
        let h = Card::height(gh, !caption.is_empty());
        let card = Card::new(title).caption(caption).show(ui, Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h)));
        let (cells, _) = mini_grid(card.content.left(), card.content.width(), card.content.top(), keys.len());
        for ((key, name), r) in keys.iter().zip(&cells) {
            // The card says what kind; each says which.
            let short = ["temperature", "Temperature", "Temp", "temp", "power", "Power", "fan"].iter().fold(name.clone(), |n, w| n.replace(&format!(" {w}"), ""));
            mini(app, ui, *r, key, &short, None);
        }
        y += h + GAP;
    }
    y - GAP - area.top()
}

// ---- System ----------------------------------------------------------

fn system(app: &mut Activity, ui: &mut Ui, area: Rect, todo: &mut Vec<Todo>) -> f32 {
    let p = style::palette(ui.ctx());
    let mut y = area.top();
    let sys = app.system.clone();
    let mem = sys.memory.iter().find(|(k, _)| k == "MemTotal").map_or(0, |(_, v)| *v);
    let access = match app.access {
        Access::Full => "Full: every process, every setting",
        Access::Asking => "Asking for the password…",
        Access::Mine => "This account's processes",
    };
    let rows: Vec<(&str, String)> = vec![
        ("Model", sys.model.clone()),
        ("Name", sys.host.clone()),
        ("Kernel", sys.kernel.clone()),
        ("Up for", duration(sys.uptime)),
        ("Processor", if sys.cpu_model.is_empty() { format!("{} cores", sys.cores) } else { sys.cpu_model.clone() }),
        ("Memory", format_size(mem)),
        ("Open files", if sys.open_files.1 >= 1 << 62 { format!("{}, no limit", sys.open_files.0) } else { format!("{} of {}", sys.open_files.0, sys.open_files.1) }),
        ("Access", access.to_owned()),
    ];
    let rr: Vec<(&str, &str)> = rows.iter().map(|(a, b)| (*a, b.as_str())).collect();
    let fh = dashboard::facts_height(rr.len().div_ceil(2));
    let button = app.access == Access::Mine && crate::helper::way() != crate::helper::Way::Off;
    let h = Card::height(fh + if button { m::SPACING + m::CONTROL_HEIGHT } else { 0.0 }, false);
    let card = Card::new("This Computer").show(ui, Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h)));
    facts_two(ui, card.content, &rr);
    if button {
        let b = Rect::from_min_size(pos2(card.content.left(), card.content.top() + fh + m::SPACING), vec2(140.0, m::CONTROL_HEIGHT));
        if ui.put(b, Button::new("Get Full Access…").min_width(b.width())).clicked() {
            todo.push(Todo::FullAccess);
        }
    }
    y += h + GAP;

    // Pressure: how much time went waiting.
    let rows: Vec<(String, String)> = sys
        .pressure
        .iter()
        .map(|pr| {
            let name = match pr.resource.as_str() {
                "cpu" => "CPU",
                "memory" => "Memory",
                "io" => "I/O",
                "irq" => "Interrupts",
                other => other,
            };
            let full = pr.full.map(|f| format!(" · all: {:.1}%, {:.1}%, {:.1}%", f[0], f[1], f[2])).unwrap_or_default();
            (name.to_owned(), format!("some: {:.1}%, {:.1}%, {:.1}%{full}", pr.some[0], pr.some[1], pr.some[2]))
        })
        .collect();
    let rr: Vec<(&str, &str)> = rows.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let keys = ["psi-cpu", "psi-memory", "psi-io"];
    let items = legend_items(app, &p, &keys, &["CPU", "Memory", "I/O"]);
    let lh = legend_height(ui, area.width() - 2.0 * m::CARD_INSET, &items);
    let fh = dashboard::facts_height(rr.len());
    let h = Card::height(fh + m::SPACING + GRAPH + m::TIGHT + lh, true);
    let card = Card::new("Pressure").caption("Time tasks waited for each, over 10 s, 1 min and 5 min").show(ui, Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h)));
    facts(ui, card.content, &rr);
    let g = Rect::from_min_size(pos2(card.content.left(), card.content.top() + fh + m::SPACING), vec2(card.content.width(), GRAPH));
    let _ = graph(app, ui, "psi", g, &keys, None, Unit::Percent);
    legend(ui, Rect::from_min_size(pos2(card.content.left(), g.bottom() + m::TIGHT), vec2(card.content.width(), lh)), &items);
    y += h + GAP;

    // Tuning: the kernel's knobs.
    let list_h = (sys.tunables.len() as f32 * m::LIST_ROW + widgets::HEADER_HEIGHT + 6.0).min(360.0);
    let h = Card::height(list_h, true);
    let caption = if app.access == Access::Full { "Double-click one to change it" } else { "Double-click one to change it (with full access)" };
    let card = Card::new("Tuning").caption(caption).show(ui, Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h)));
    let columns = [Column::new("Setting", 230.0), Column::new("Value", 100.0), Column::new("What it does", 320.0)];
    let cells: Vec<[String; 3]> = sys
        .tunables
        .iter()
        .map(|(k, v)| [k.clone(), v.clone(), crate::sample::TUNABLES.iter().find(|(n, _)| n == k).map_or("", |(_, d)| *d).to_owned()])
        .collect();
    let out = ui
        .scope_builder(egui::UiBuilder::new().max_rect(card.content), |ui| {
            ColumnListView::new("activity-tunables", &columns, cells.len(), &mut app.tunables_list).show(ui, |cell: &Cell| cell.text(&cells[cell.row][cell.column]))
        })
        .inner;
    if let Some(row) = out.activated
        && let Some(cl) = cells.get(row)
    {
        todo.push(Todo::Command(format!("tune:{}", cl[0])));
    }
    if let Some((row, pos)) = out.context
        && let Some(cl) = cells.get(row)
    {
        app.tunables_list.select(row);
        widgets::popup_menu_at(ui.ctx(), Id::unique("activity-tune-menu"), pos, Menu::new("").item("Change…", format!("tune:{}", cl[0]), None));
    }
    if let Some(cmd) = widgets::popup_result(ui.ctx(), Id::unique("activity-tune-menu")) {
        todo.push(Todo::Command(cmd));
    }
    y + h - area.top()
}

// ---- Graphs ----------------------------------------------------------

/// Haiku's ActivityMonitor: graphs of whatever you choose (right-click a
/// graph to choose; add and remove graphs).
fn graphs(app: &mut Activity, cx: &mut Cx, ui: &mut Ui, area: Rect) {
    let p = style::palette(ui.ctx());
    let n = app.settings.views.len().max(1);
    let legends: Vec<Vec<LegendItem>> = app
        .settings
        .views
        .iter()
        .map(|v| {
            v.iter()
                .filter_map(|k| {
                    let i = app.source(k)?;
                    Some(LegendItem { color: c(p.hue(app.sources[i].color)), name: &app.sources[i].name, value: value(app, k) })
                })
                .collect()
        })
        .collect();
    let inner_w = area.width() - 2.0 * m::CARD_INSET;
    let lhs: Vec<f32> = legends.iter().map(|l| legend_height(ui, inner_w, l).max(dashboard::FACT_ROW)).collect();
    // Untitled cards: the graph and its legend, inset.
    let card_h = |graph: f32, lh: f32| 2.0 * m::CARD_INSET + graph + m::TIGHT + lh;
    let fixed: f32 = lhs.iter().map(|lh| card_h(0.0, *lh)).sum::<f32>() + (n - 1) as f32 * GAP;
    let graph_h = ((area.height() - fixed) / n as f32).max(48.0).floor();
    let mut y = area.top();
    let mut chosen = None;
    let views = app.settings.views.clone();
    for (v, keys) in views.iter().enumerate() {
        let h = card_h(graph_h, lhs[v]);
        let r = Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), h));
        y += h + GAP;
        style::card(ui.painter(), r, &p, false, false);
        let content = r.shrink(m::CARD_INSET);
        let g = Rect::from_min_size(content.min, vec2(content.width(), graph_h));
        // Each source scaled to its own range (its fixed top, or what it
        // shows), so CPU and memory share a graph.
        let span = app.settings.span as f64;
        let now = app.now();
        let picked: Vec<usize> = keys.iter().filter_map(|k| app.source(k)).collect();
        let formats: Vec<Box<dyn Fn(f64) -> String>> = picked.iter().map(|&i| {
            let unit = app.sources[i].unit;
            Box::new(move |x: f64| unit.format(x)) as Box<dyn Fn(f64) -> String>
        }).collect();
        let s: Vec<Series> = picked
            .iter()
            .zip(&formats)
            .map(|(&i, f)| {
                let src = &app.sources[i];
                let seen = app.history[i].iter().filter(|(t, _)| *t >= now - span).map(|(_, v)| *v).filter(|v| v.is_finite()).fold(0.0, f64::max);
                Series::new(&src.name, c(p.hue(src.color)), &app.history[i]).top(src.max.unwrap_or_else(|| olo_kit::graph::nice_ceiling(seen))).format(f.as_ref())
            })
            .collect();
        let resp = Graph::new(("view", v), &s, now).span(span).grid(grid_for(app.settings.span)).smooth(app.settings.smooth).plain().show(ui, g);
        legend(ui, Rect::from_min_size(pos2(content.left(), g.bottom() + m::TIGHT), vec2(content.width(), lhs[v])), &legends[v]);
        let whole = ui.interact(r, Id::unique(("activity-view", v)), Sense::click());
        let menu = view_menu(app, v);
        for resp in [resp.response, Some(whole)].into_iter().flatten() {
            if let Some(cmd) = widgets::context_menu(&resp, || menu.clone()) {
                chosen = Some(cmd);
            }
        }
    }
    if let Some(cmd) = chosen {
        app.command(cx, None, &cmd);
    }
}

/// The context menu of graph `v`: every source, by what it's about.
fn view_menu(app: &Activity, v: usize) -> Menu {
    let shown = &app.settings.views[v];
    let item = |menu: Menu, s: &crate::sample::Source| menu.item(s.name.clone(), format!("toggle:{v}:{}", s.key), None).checked(shown.contains(&s.key));
    let mut menu = Menu::new("");
    let groups = [
        (Group::Cpus, "Processors"),
        (Group::Frequencies, "Frequencies"),
        (Group::Disks, "Disks"),
        (Group::Network, "Network Interfaces"),
        (Group::Gpu, "GPU"),
        (Group::Pressure, "Pressure"),
        (Group::Sensors, "Sensors"),
    ];
    for s in app.sources.iter().filter(|s| s.group == Group::Main) {
        menu = item(menu, s);
    }
    menu = menu.separator();
    for (g, name) in groups {
        let mut sub = Menu::new(name);
        for s in app.sources.iter().filter(|s| s.group == g) {
            sub = item(sub, s);
        }
        if !sub.items.is_empty() {
            menu = menu.submenu(sub);
        }
    }
    menu.separator().item("Add Graph", "add-graph", None).item("Remove Graph", format!("remove-graph:{v}"), None).enabled(app.settings.views.len() > 1)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(duration(42.0), "42 seconds");
        assert_eq!(duration(61.0), "1 minute");
        assert_eq!(duration(3.0 * 3600.0 + 120.0), "3 hours, 2 minutes");
        assert_eq!(duration(2.0 * 86400.0 + 3600.0), "2 days, 1 hour");
        assert_eq!(ghz(2424.0), "2.42 GHz");
        assert_eq!(grid_for(300), 60.0);
    }
}
