//! The sections that are cards of graphs and facts: Overview, the
//! Supervisor's work, CPU, Memory, Disks, Network, GPU, Displays, Inputs,
//! Sensors, System and the Graphs of your choosing. (Processes is
//! processes.rs.)

use std::rc::Rc;

use olo_theme::Palette;
use olo_ui::{
    Menu, agent,
    files::format_size,
    gpui::{
        AnyElement, App, Bounds, Context, Entity, FontWeight, HitboxBehavior, Hsla, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels,
        SharedString, Styled as _, Window, canvas, div, point, px, transparent_black,
    },
    menu, metrics as m,
    style::{self, c},
    theme,
    widgets::{
        Button, CheckBox, Column, ColumnListView, Graph, HEADER_HEIGHT, LegendItem, MenuField, SCROLL_BAR, ScrollView, Series, StringView, TextControl,
        charts::FACT_ROW,
        dashboard::{self, Card, Facts, GAP, meter},
        legend, legend_height, nice_ceiling,
    },
};

use crate::{
    Access, Activity, Section,
    actions::Action,
    sample::{Group, Unit},
};

/// A big graph's height in a card.
const GRAPH: f32 = 120.0;
/// A small one's (each core, each sensor).
const MINI_GRAPH: f32 = 44.0;
const MINI_CELL: f32 = 16.0 + MINI_GRAPH + 4.0;
const MINI_MIN_WIDTH: f32 = 150.0;
const OVERVIEW_CARD: f32 = 196.0;
const CONNECTIONS_HEIGHT: f32 = 320.0;

type Me = Entity<Activity>;

/// A source's series for a graph: its name, colour and samples.
fn series(app: &Activity, p: &Palette, key: &str, fill: bool) -> Option<Series> {
    let i = app.source(key)?;
    let s = &app.sources[i];
    Some(Series::new(s.name.clone(), c(p.hue(s.color)), app.history[i].clone()).fill(fill))
}

fn value(app: &Activity, key: &str) -> String {
    let unit = app.source(key).map(|i| app.sources[i].unit);
    match (app.latest(key), unit) {
        (Some(v), Some(u)) => u.format(v),
        _ => "–".into(),
    }
}

fn legend_items(app: &Activity, p: &Palette, keys: &[&str], names: &[&str]) -> Vec<LegendItem> {
    keys.iter()
        .zip(names)
        .filter_map(|(k, n)| {
            let i = app.source(k)?;
            Some(LegendItem { color: c(p.hue(app.sources[i].color)), name: SharedString::from(n.to_string()), value: value(app, k).into() })
        })
        .collect()
}

/// A figure with no line on the graph: no swatch.
fn figure(name: &str, value: impl Into<SharedString>) -> LegendItem {
    LegendItem { color: transparent_black(), name: SharedString::from(name.to_owned()), value: value.into() }
}

/// A graph of `keys` (the first filled, its unit the graph's), scaled
/// to `max` or what it shows, `h` tall; a click looks back to then
/// (`drill`: the app to tell).
fn graph(app: &Activity, drill: Option<&Me>, keys: &[&str], max: Option<f64>, h: f32, cx: &App) -> AnyElement {
    let p = theme::palette(cx);
    let s: Vec<Series> = keys.iter().enumerate().filter_map(|(i, k)| series(app, &p, k, i == 0)).collect();
    let unit = keys.first().and_then(|k| app.source(k)).map_or(Unit::Percent, |i| app.sources[i].unit);
    let id = SharedString::from(format!("activity-{}", keys.join(",")));
    let mut g = Graph::new(id, s, app.now()).span(app.settings.span as f64).grid(grid_for(app.settings.span)).unit(move |v| unit.format(v)).smooth(app.settings.smooth).plain();
    if let Some(m) = max {
        g = g.max(m);
    }
    if let Some(me) = drill {
        let me = me.clone();
        g = g.on_click(move |t, _, cx| {
            me.update(cx, |a, cx| {
                a.drill = Some(t);
                cx.notify();
            })
        });
    }
    div().h(px(h)).flex_shrink_0().child(g.on_layer(&app.shelf)).into_any_element()
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

/// A legend of `items`, as wide as `width`.
fn legend_el(items: Vec<LegendItem>, width: f32, cx: &App) -> AnyElement {
    let h = legend_height(cx, width, &items);
    canvas(|_, _, _| {}, move |b, _, window, cx| legend(window, cx, b, &items)).w_full().h(px(h)).flex_shrink_0().into_any_element()
}

/// A bar of parts of a whole (`dashboard::meter`), `h` tall.
fn meter_el(parts: Vec<(Hsla, f64)>, total: f64, h: f32) -> AnyElement {
    canvas(|_, _, _| {}, move |b, _, window, cx| meter(window, cx, b, &parts, total)).w_full().h(px(h)).flex_shrink_0().into_any_element()
}

fn facts(rows: Vec<(String, String)>) -> Facts {
    Facts::new(rows.into_iter().map(|(a, b)| (SharedString::from(a), SharedString::from(b))).collect::<Vec<_>>())
}

/// Facts in two columns side by side (the first takes the odd one).
fn facts_two(rows: Vec<(String, String)>) -> AnyElement {
    let half = rows.len().div_ceil(2);
    let mut rows = rows;
    let right = rows.split_off(half);
    div().flex().flex_row().gap(px(2.0 * m::SPACING)).child(div().flex_1().min_w_0().child(facts(rows))).child(div().flex_1().min_w_0().child(facts(right))).into_any_element()
}

/// Text wrapped to the card, quiet or not.
fn para(text: impl Into<SharedString>, color: Hsla) -> AnyElement {
    StringView::new(text).wrap(true).color(color).into_any_element()
}

/// A small graph with its name and value over it (a core, a sensor).
fn mini(app: &Activity, key: &str, label: String, max: Option<f64>, cx: &App) -> AnyElement {
    if app.source(key).is_none() {
        return div().into_any_element();
    }
    let v = value(app, key);
    let head = canvas(
        |_, _, _| {},
        move |b, _, window, cx| {
            let p = theme::palette(cx);
            let ink = style::card_ink(&p);
            let font = style::ui_font(FontWeight::NORMAL);
            let vr = style::label(window, cx, b.right(), b.top() + px(8.0), style::Align::Right, v.clone(), style::SMALL_SIZE, font.clone(), ink.text);
            let l =
                style::label_fit(window, cx, b.left(), b.top() + px(8.0), style::Align::Left, label.clone(), style::SMALL_SIZE, font, ink.dim, (vr.left() - px(m::SPACING) - b.left()).max(px(0.0)));
            agent::record(window, cx, agent::Node { value: Some(v.clone().into()), ..agent::Node::label(label.clone(), l) });
        },
    )
    .w_full()
    .h(px(16.0))
    .flex_shrink_0();
    div().flex().flex_col().h(px(MINI_CELL)).child(head).child(graph(app, None, &[key], max, MINI_GRAPH, cx)).into_any_element()
}

/// How many columns of at least `min` go in `width` (at most `max`), and
/// each one's width (the last takes what rounding left).
fn columns(width: f32, n: usize, min: f32, max: usize) -> Vec<f32> {
    let cols = (((width + GAP) / (min + GAP)).floor() as usize).clamp(1, max.max(1)).min(n.max(1));
    let w = ((width - GAP * (cols - 1) as f32) / cols as f32).floor();
    (0..cols).map(|i| if i == cols - 1 { width - w * (cols - 1) as f32 - GAP * (cols - 1) as f32 } else { w }).collect()
}

/// `items` in rows of as many columns as fit, cells in a row as tall as
/// the tallest.
fn flow(items: Vec<AnyElement>, width: f32, min: f32, max: usize) -> AnyElement {
    let widths = columns(width, items.len(), min, max);
    let mut out = div().flex().flex_col().gap(px(GAP));
    let mut items = items.into_iter().peekable();
    while items.peek().is_some() {
        let mut row = div().flex().flex_row().gap(px(GAP));
        for (w, item) in widths.iter().zip(items.by_ref()) {
            row = row.child(div().w(px(*w)).flex_shrink_0().flex().flex_col().child(item));
        }
        out = out.child(row);
    }
    out.into_any_element()
}

/// Small graphs in a grid across `width`.
fn mini_grid(cells: Vec<AnyElement>, width: f32) -> AnyElement {
    flow(cells, width, MINI_MIN_WIDTH, 8)
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

/// The section shown (but Processes): its cards, for a cached `Part`
/// under its graphs' layer.
pub fn cards(app: &mut Activity, window: &mut Window, cx: &mut Context<Activity>) -> AnyElement {
    let width = f32::from(window.viewport_size().width) - crate::SIDEBAR_WIDTH;
    let s = app.settings.section;
    view(app, s, width, cx)
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
        Section::Supervisor => match (app.supervisor.running, app.supervisor.findings.len()) {
            (false, _) => "Off".to_owned(),
            (true, 0) => "On".to_owned(),
            (true, n) => n.to_string(),
        },
        // Dropped frames in the last minute, if any.
        Section::Displays => match app.displays.iter().map(|d| d.late_minute).sum::<u32>() {
            0 => String::new(),
            n => format!("{n} dropped"),
        },
        // The pointer's reports going missing, if they are.
        Section::Inputs => match app.pointer_health.gaps_minute {
            0 => String::new(),
            1 => "1 gap".to_owned(),
            n => format!("{n} gaps"),
        },
        // Of the suggestions shown, how many were taken.
        Section::Ai => match &app.model {
            Some(s) if s.offered > 0 => percent(s.accepted as f64 / s.offered as f64),
            _ if !app.model_on => "Off".to_owned(),
            _ => String::new(),
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

/// Section `s`, `width` wide: cards on the gutter, scrolling.
pub fn view(app: &mut Activity, s: Section, width: f32, cx: &mut Context<Activity>) -> AnyElement {
    let me = cx.entity();
    // Its graphs put on the layer anew as it's laid out.
    app.shelf.clear();
    let p = theme::palette(cx);
    // Inside the gutter round the cards (and the scroll bar, but for the
    // Graphs, which fit), and the cards' inset.
    if s == Section::Graphs {
        return graphs(app, &me, width - 2.0 * GAP - 2.0 * m::CARD_INSET, cx);
    }
    let area = width - SCROLL_BAR - 2.0 * GAP;
    let inner = area - 2.0 * m::CARD_INSET;
    let cards: Vec<AnyElement> = match s {
        Section::Overview => overview(app, &me, area, cx),
        Section::Cpu => cpu(app, &me, inner, cx),
        Section::Memory => memory(app, area, inner, cx),
        Section::Disks => disks(app, &me, inner, cx),
        Section::Network => network(app, &me, inner, cx),
        Section::Gpu => gpu(app, &me, area, inner, cx),
        Section::Displays => displays(app, &me, inner, cx),
        Section::Inputs => inputs(app, inner, cx),
        Section::Ai => ai(app, inner, cx),
        Section::Sensors => sensors(app, inner, cx),
        Section::System => system(app, &me, inner, cx),
        Section::Supervisor => supervisor(app, &me, cx),
        Section::Processes | Section::Graphs => Vec::new(),
    };
    let body = div().flex().flex_col().gap(px(GAP)).p(px(GAP)).children(cards);
    ScrollView::new(SharedString::from(format!("activity-section-{}", s.name())), &app.scroll).border(false).background(dashboard::gutter(&p)).child(body).into_any_element()
}

// ---- Overview --------------------------------------------------------

fn overview(app: &Activity, me: &Me, area: f32, cx: &App) -> Vec<AnyElement> {
    let p = theme::palette(cx);
    let sys = &app.system;
    let mut cards: Vec<Section> = vec![Section::Cpu, Section::Memory, Section::Network, Section::Disks];
    if app.source("gpu-busy").is_some() {
        cards.push(Section::Gpu);
    }
    if app.source("battery").is_some() || hottest(app).is_some() {
        cards.push(Section::Sensors);
    }
    let widths = columns(area, cards.len(), 250.0, 3);
    let mut out = Vec::new();
    for (k, s) in cards.iter().enumerate() {
        let card_inner = widths[k % widths.len()] - 2.0 * m::CARD_INSET;
        let mem_total = sys.memory.iter().find(|(k, _)| k == "MemTotal").map_or(0, |(_, v)| *v);
        let (title, figure_text, caption, keys, names, max): (&str, String, String, Vec<&str>, Vec<&str>, Option<f64>) = match s {
            Section::Cpu => (
                "CPU",
                value(app, "cpu"),
                format!("{} cores{} · load {:.2}", sys.cores, app.latest("cpu-freq").map(|f| format!(" · {}", ghz(f))).unwrap_or_default(), sys.load[0]),
                vec!["cpu", "cpu-system", "cpu-iowait"],
                vec!["Busy", "System", "I/O wait"],
                Some(100.0),
            ),
            Section::Memory => {
                ("Memory", value(app, "memory"), format!("of {} · {} swap used", format_size(mem_total), value(app, "swap")), vec!["memory", "cached"], vec!["Used", "Cached"], Some(mem_total as f64))
            }
            Section::Network => (
                "Network",
                Unit::Rate.format(app.latest("net-rx").unwrap_or(0.0) + app.latest("net-tx").unwrap_or(0.0)),
                sys.interfaces.iter().filter(|i| i.up).map(|i| i.name.clone()).collect::<Vec<_>>().join(", "),
                vec!["net-rx", "net-tx"],
                vec!["In", "Out"],
                None,
            ),
            Section::Disks => (
                "Disks",
                Unit::Rate.format(app.latest("disk-read").unwrap_or(0.0) + app.latest("disk-write").unwrap_or(0.0)),
                sys.volumes.first().and_then(|v| Some(format!("{} free on {}", format_size(v.free?), v.mount.split(", ").next().unwrap_or(&v.mount)))).unwrap_or_default(),
                vec!["disk-read", "disk-write"],
                vec!["Read", "Written"],
                None,
            ),
            Section::Gpu => ("GPU", value(app, "gpu-busy"), sys.gpu.as_ref().map(|g| format!("{} · {}", g.name, g.driver)).unwrap_or_default(), vec!["gpu-busy"], vec!["Busy"], Some(100.0)),
            _ => match &sys.battery {
                Some(b) => ("Battery", value(app, "battery"), format!("{} · {}", b.status, Unit::Watts.format(b.watts.abs())), vec!["battery"], vec!["Charge"], Some(100.0)),
                None => {
                    let (name, v) = hottest(app).unwrap_or_default();
                    ("Temperature", Unit::Celsius.format(v), format!("Hottest: {name}"), vec![], vec![], None)
                }
            },
        };
        let mut items = legend_items(app, &p, &keys, &names);
        if *s == Section::Gpu {
            items.extend(legend_items(app, &p, &["gpu-power"], &["Power"]));
        }
        if *s == Section::Sensors && sys.battery.is_some() {
            items.extend(legend_items(app, &p, &["battery-power"], &["Power"]));
        }
        let lh = legend_height(cx, card_inner, &items);
        let head = dashboard::Card::height(0.0, !caption.is_empty());
        let graph_h = (OVERVIEW_CARD - head - lh - m::TIGHT).max(20.0);
        let go = me.clone();
        let section = *s;
        let mut card = Card::new(format!("overview-{title}"), title).figure(figure_text).caption(caption).on_click(move |_, cx| go.update(cx, |a, cx| a.set_section(section, cx)));
        if !keys.is_empty() {
            card = card.child(graph(app, Some(me), &keys, max, graph_h, cx));
        } else if *s == Section::Sensors {
            // No battery: the hottest few sensors' graph.
            let mut temps: Vec<(usize, f64)> = app.sources.iter().enumerate().filter(|(_, s)| s.unit == Unit::Celsius).filter_map(|(i, _)| Some((i, app.history[i].back()?.1))).collect();
            temps.sort_by(|a, b| b.1.total_cmp(&a.1));
            let keys: Vec<&str> = temps.iter().take(3).map(|(i, _)| app.sources[*i].key.as_str()).collect();
            card = card.child(graph(app, None, &keys, None, graph_h, cx));
        }
        card = card.child(div().h(px(m::TIGHT))).child(legend_el(items, card_inner, cx));
        out.push(div().h(px(OVERVIEW_CARD)).flex().flex_col().child(card).into_any_element());
    }
    vec![flow(out, area, 250.0, 3), busiest(app, me)]
}

/// A process and its figure: (pid, name, figure).
type Busy = (i32, String, String);

/// The busiest processes now, or at the moment chosen on a graph.
fn busiest(app: &Activity, me: &Me) -> AnyElement {
    let rows = 6;
    let moment = app.drill.and_then(|t| app.moments.iter().rev().find(|m| m.at <= t + 0.01).or(app.moments.front()));
    let (title, caption) = match moment {
        Some(m) => {
            let ago = app.t() - m.at;
            (format!("At {}", clock(wall(app, m.at))), format!("{} ago · click a graph for another moment", duration(ago)))
        }
        None => ("Busiest Now".to_owned(), "Click a graph to see what was busy then".to_owned()),
    };
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
    let lists = div().flex().flex_row().gap(px(2.0 * m::SPACING)).child(div().flex_1().min_w_0().child(process_rows(me, "CPU", cpu))).child(div().flex_1().min_w_0().child(process_rows(
        me,
        other_title,
        other,
    )));
    let card = Card::new("busiest", title).caption(caption).child(lists);
    if moment.is_none() {
        return card.into_any_element();
    }
    // Now, at the head's right, back to the present.
    let now = me.clone();
    div()
        .relative()
        .flex()
        .flex_col()
        .child(card)
        .child(div().absolute().top(px(m::CARD_INSET - 2.0)).right(px(m::CARD_INSET)).child(Button::new("Now").on_click(move |_, cx| {
            now.update(cx, |a, cx| {
                a.drill = None;
                cx.notify();
            })
        })))
        .into_any_element()
}

/// A small list of processes (name and a figure) under a quiet head,
/// each a way to it in Processes: drawn as one, each row `FACT_ROW` from
/// the last as facts are (rows laid out apart came out a pixel uneven).
fn process_rows(me: &Me, head: &str, rows: Vec<Busy>) -> AnyElement {
    let head = SharedString::from(head.to_owned());
    let n = rows.len();
    let row = |b: Bounds<Pixels>, i: usize| Bounds::from_corners(point(b.left(), b.top() + px((i + 1) as f32 * FACT_ROW)), point(b.right(), b.top() + px((i + 2) as f32 * FACT_ROW)));
    let me = me.clone();
    canvas(
        move |b, window, _| (0..n).map(|i| window.insert_hitbox(row(b, i), HitboxBehavior::Normal)).collect::<Vec<_>>(),
        move |b, hitboxes, window, cx| {
            let p = theme::palette(cx);
            let ink = style::card_ink(&p);
            let font = style::ui_font(FontWeight::NORMAL);
            let h = style::label(window, cx, b.left(), b.top() + px(FACT_ROW / 2.0), style::Align::Left, head.clone(), style::SMALL_SIZE, font.clone(), ink.dim);
            agent::record(window, cx, agent::Node::label(head.clone(), h));
            let hovered = hitboxes.iter().position(|h| h.is_hovered(window));
            for (i, (_, name, fig)) in rows.iter().enumerate() {
                let r = row(b, i);
                if hovered == Some(i) {
                    let lit = Bounds::from_corners(point(r.left() - px(4.0), r.top()), point(r.right() + px(4.0), r.bottom()));
                    window.paint_quad(olo_ui::gpui::fill(style::snap(window, lit), ink.hover).corner_radii(px(3.0)));
                }
                let mid = r.top() + px(FACT_ROW / 2.0);
                let f = style::label(window, cx, r.right(), mid, style::Align::Right, fig.clone(), style::PLAIN_SIZE, font.clone(), ink.text);
                style::label_fit(window, cx, r.left(), mid, style::Align::Left, name.clone(), style::PLAIN_SIZE, font.clone(), ink.text, (f.left() - px(m::SPACING) - r.left()).max(px(0.0)));
                agent::record(window, cx, agent::Node { value: Some(fig.clone().into()), ..agent::Node::button(name.clone(), r) });
            }
            // The pointer moving onto another row: drawn again.
            let hbs = hitboxes.clone();
            window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, _| {
                if phase.bubble() && hbs.iter().position(|h| h.is_hovered(window)) != hovered {
                    window.refresh();
                }
            });
            let (me, pids): (Me, Vec<i32>) = (me.clone(), rows.iter().map(|(pid, ..)| *pid).collect());
            window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                if phase.bubble()
                    && e.button == MouseButton::Left
                    && let Some(i) = hitboxes.iter().position(|h| h.is_hovered(window))
                {
                    let pid = pids[i];
                    me.update(cx, |a, cx| a.command(&format!("select:{pid}"), None, cx));
                }
            });
        },
    )
    .w_full()
    .h(px((n + 1) as f32 * FACT_ROW))
    .flex_shrink_0()
    .into_any_element()
}

// ---- CPU -------------------------------------------------------------

fn cpu(app: &Activity, me: &Me, inner: f32, cx: &App) -> Vec<AnyElement> {
    let p = theme::palette(cx);
    let sys = &app.system;
    let mut out = Vec::new();
    // Everything.
    let keys = ["cpu", "cpu-user", "cpu-system", "cpu-iowait"];
    let items = legend_items(app, &p, &keys, &["Busy", "User", "System", "I/O wait"]);
    let caption = if sys.cpu_model.is_empty() { format!("{} cores", sys.cores) } else { sys.cpu_model.clone() };
    out.push(
        Card::new("cpu", "CPU")
            .figure(value(app, "cpu"))
            .caption(caption)
            .child(graph(app, Some(me), &keys, Some(100.0), GRAPH + 40.0, cx))
            .child(div().h(px(m::TIGHT)))
            .child(legend_el(items, inner, cx))
            .into_any_element(),
    );

    // Each core.
    let cores: Vec<usize> = app.sources.iter().enumerate().filter(|(_, s)| s.group == Group::Cpus).map(|(i, _)| i).collect();
    let cells: Vec<AnyElement> = cores
        .iter()
        .enumerate()
        .map(|(k, &i)| {
            let freq = app.latest(&format!("freq{k}")).map(|f| format!(" · {}", ghz(f))).unwrap_or_default();
            mini(app, &app.sources[i].key, format!("{}{freq}", app.sources[i].name), Some(100.0), cx)
        })
        .collect();
    out.push(Card::new("cores", "Cores").child(mini_grid(cells, inner)).into_any_element());

    // Clusters: frequencies and governors.
    if !sys.policies.is_empty() {
        let freq_keys: Vec<String> = sys.policies.iter().filter_map(|pol| pol.cores.first().map(|c| format!("freq{c}"))).collect();
        let fk: Vec<&str> = freq_keys.iter().map(String::as_str).collect();
        let names: Vec<String> = sys.policies.iter().map(cluster_name).collect();
        let nk: Vec<&str> = names.iter().map(String::as_str).collect();
        let items = legend_items(app, &p, &fk, &nk);
        let mut card = Card::new("frequencies", "Frequencies").caption("Each cluster's cores change speed together; the governor decides how");
        let field_w = 220.0f32.min(inner / 2.0);
        for (k, pol) in sys.policies.iter().enumerate() {
            let label: SharedString = format!("{}: {} – {}, now {}", names[k], ghz(pol.min), ghz(pol.max), ghz(pol.now)).into();
            let chosen = pol.governors.iter().position(|g| *g == pol.governor).unwrap_or(0);
            let options: Vec<SharedString> = pol.governors.iter().map(|g| SharedString::from(g.clone())).collect();
            let text = canvas(
                |_, _, _| {},
                move |b, _, window, cx| {
                    let p = theme::palette(cx);
                    let r = style::label_fit(
                        window,
                        cx,
                        b.left(),
                        b.center().y,
                        style::Align::Left,
                        label.clone(),
                        style::PLAIN_SIZE,
                        style::ui_font(FontWeight::NORMAL),
                        style::card_ink(&p).text,
                        b.size.width,
                    );
                    agent::record(window, cx, agent::Node::label(label.clone(), r));
                },
            )
            .flex_1()
            .min_w_0()
            .h_full();
            let mut row = div().flex().flex_row().items_center().gap(px(m::SPACING)).h(px(m::CONTROL_HEIGHT)).mb(px(m::TIGHT)).child(text);
            if !options.is_empty() {
                let (policy, governors, act) = (pol.id, pol.governors.clone(), me.clone());
                row = row.child(MenuField::new(SharedString::from(format!("governor-{}", pol.id)), options, chosen).label("Governor:").width(field_w).on_change(move |i, _, cx| {
                    let governor = governors[i].clone();
                    act.update(cx, |a, cx| a.act(Action::Governor { policy, governor }, cx));
                }));
            }
            card = card.child(row);
        }
        card = card.child(div().h(px(m::TIGHT))).child(graph(app, None, &fk, None, GRAPH, cx)).child(div().h(px(m::TIGHT))).child(legend_el(items, inner, cx));
        out.push(card.into_any_element());
    }

    // Idle states: how deeply the cores sleep when there's nothing to do.
    if !sys.idle_states.is_empty() {
        let busy = app.latest("cpu").unwrap_or(0.0);
        let mut rows: Vec<(String, String)> = vec![("Running".into(), format!("{busy:.1}% of the time"))];
        for st in &sys.idle_states {
            let what = if st.what.is_empty() { st.name.clone() } else { format!("{} ({})", st.name, st.what) };
            rows.push((what, format!("{:.1}% of the time, waking in {} µs", st.share, st.latency_us)));
        }
        let hues = [olo_theme::Hue::Blue, olo_theme::Hue::Teal, olo_theme::Hue::Green, olo_theme::Hue::Purple, olo_theme::Hue::Gray];
        let mut parts = vec![(c(p.hue(hues[0])), busy)];
        parts.extend(sys.idle_states.iter().enumerate().map(|(i, st)| (c(p.hue(hues[(i + 1) % hues.len()])), st.share)));
        out.push(
            Card::new("idle", "Idle States")
                .caption("Where the cores' time goes: the deeper a state, the less power and the slower to wake")
                .child(meter_el(parts, 100.0, 10.0))
                .child(div().h(px(m::SPACING)))
                .child(facts(rows))
                .into_any_element(),
        );
    }

    // Load.
    let rows: Vec<(String, String)> = vec![
        ("Load average".into(), format!("{:.2}, {:.2}, {:.2} (1, 5 and 15 minutes)", sys.load[0], sys.load[1], sys.load[2])),
        ("Processes".into(), value(app, "processes")),
        ("Threads".into(), value(app, "threads")),
        ("Context switches".into(), value(app, "switches")),
        ("Interrupts".into(), value(app, "interrupts")),
        ("Page faults".into(), value(app, "faults")),
    ];
    out.push(Card::new("load", "Load").child(facts_two(rows)).child(div().h(px(m::SPACING))).child(graph(app, None, &["load"], None, GRAPH, cx)).into_any_element());
    out
}

fn cluster_name(pol: &crate::sample::Policy) -> String {
    match (pol.cores.first(), pol.cores.last()) {
        (Some(a), Some(b)) if a != b => format!("Cores {}–{}", a + 1, b + 1),
        (Some(a), _) => format!("Core {}", a + 1),
        _ => format!("Cluster {}", pol.id),
    }
}

// ---- Memory ----------------------------------------------------------

fn memory(app: &Activity, area: f32, inner: f32, cx: &App) -> Vec<AnyElement> {
    let p = theme::palette(cx);
    let sys = &app.system;
    let field = |k: &str| sys.memory.iter().find(|(n, _)| n == k).map_or(0, |(_, v)| *v);
    let total = field("MemTotal");
    let mut out = Vec::new();
    let keys = ["memory", "cached", "swap"];
    let items = legend_items(app, &p, &keys, &["Used", "Cached", "Swap"]);
    let caption = format!("of {} · {} available", format_size(total), format_size(field("MemAvailable")));
    let used = app.latest("memory").unwrap_or(0.0);
    let cached = app.latest("cached").unwrap_or(0.0);
    let hue = |k: &str| app.source(k).map_or(olo_ui::gpui::opaque_grey(0.5, 1.0), |i| c(p.hue(app.sources[i].color)));
    out.push(
        Card::new("memory", "Memory")
            .figure(value(app, "memory"))
            .caption(caption)
            .child(graph(app, None, &keys, Some(total as f64), GRAPH + 40.0, cx))
            .child(div().h(px(m::SPACING)))
            .child(meter_el(vec![(hue("memory"), used), (hue("cached"), cached)], total as f64, 10.0))
            .child(div().h(px(m::TIGHT)))
            .child(legend_el(items, inner, cx))
            .into_any_element(),
    );

    // zram and pressure, side by side when there's room.
    let mut rows: Vec<(String, String)> = sys
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
    if rows.is_empty() {
        rows.push(("Swap".into(), format!("{} of {}", value(app, "swap"), format_size(field("SwapTotal")))));
    }
    let prow: Vec<(String, String)> = sys
        .pressure
        .iter()
        .find(|p| p.resource == "memory")
        .map(|pr| {
            let mut v = vec![("Some waiting".to_owned(), format!("{:.1}%, {:.1}%, {:.1}%", pr.some[0], pr.some[1], pr.some[2]))];
            if let Some(f) = pr.full {
                v.push(("All waiting".into(), format!("{:.1}%, {:.1}%, {:.1}%", f[0], f[1], f[2])));
            }
            v
        })
        .unwrap_or_default();
    let zcap = if sys.zram.is_empty() { "No compressed swap" } else { "Swap in memory, compressed" };
    let zram = Card::new("zram", "Compressed Swap").caption(zcap).child(facts(rows)).into_any_element();
    let pressure = Card::new("memory-pressure", "Memory Pressure")
        .caption("Time tasks waited for memory: 10 s, 1 min, 5 min")
        .child(facts(prow))
        .child(div().h(px(m::SPACING)))
        .child(graph(app, None, &["psi-memory", "psi-memory-full"], None, GRAPH * 0.6, cx))
        .into_any_element();
    out.push(flow(vec![zram, pressure], area, 300.0, 2));

    // Every counter the kernel keeps.
    let all: Vec<(String, String)> = sys.memory.iter().map(|(k, v)| (k.clone(), if k.starts_with("HugePages_") { v.to_string() } else { format_size(*v) })).collect();
    out.push(Card::new("meminfo", "Every Counter").caption("/proc/meminfo").child(facts_two(all)).into_any_element());
    out
}

// ---- Disks -----------------------------------------------------------

fn disks(app: &Activity, me: &Me, inner: f32, cx: &App) -> Vec<AnyElement> {
    let p = theme::palette(cx);
    let sys = &app.system;
    let mut out = Vec::new();
    for d in &sys.disks {
        let keys = [format!("disk:{}:read", d.name), format!("disk:{}:write", d.name), format!("disk:{}:busy", d.name)];
        let k: Vec<&str> = keys.iter().map(String::as_str).collect();
        let mut items = legend_items(app, &p, &k[..2], &["Read", "Written"]);
        items.push(figure("Busy", value(app, &keys[2])));
        items.push(figure("Requests", format!("{:.0}/s", d.iops)));
        items.push(figure("Each takes", format!("{:.2} ms", d.latency_ms)));
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
        let kind = if d.rotational {
            "hard disk"
        } else if d.removable {
            "removable"
        } else {
            "solid-state"
        };
        let caption = format!("{} · {} {kind}", if d.model.is_empty() { "Disk" } else { &d.model }, format_size(d.size));
        let figure_text = Unit::Rate.format(app.latest(&keys[0]).unwrap_or(0.0) + app.latest(&keys[1]).unwrap_or(0.0));
        let mut card = Card::new(format!("disk-{}", d.name), d.name.clone())
            .figure(figure_text)
            .caption(caption)
            .child(graph(app, Some(me), &k[..2], None, GRAPH, cx))
            .child(div().h(px(m::TIGHT)))
            .child(legend_el(items, inner, cx));
        if !hrows.is_empty() {
            card = card.child(div().h(px(m::SPACING))).child(facts_two(hrows));
        }
        out.push(card.into_any_element());
    }
    // Volumes.
    let mut volumes = Card::new("volumes", "Volumes");
    for v in &sys.volumes {
        let title: SharedString = format!("{}{}", v.mount, if v.read_only { " (read-only)" } else { "" }).into();
        let detail: SharedString = match (v.size, v.free) {
            (Some(s), Some(f)) => format!("{} free of {}", format_size(f), format_size(s)),
            _ => "On the network".into(),
        }
        .into();
        let sub: SharedString = format!("{} · {}", v.kind, v.device).into();
        let bar = match (v.size, v.free) {
            (Some(s), Some(f)) => {
                let used = s.saturating_sub(f) as f64;
                let hue = if used / s.max(1) as f64 > 0.9 { p.hue(olo_theme::Hue::Red) } else { p.hue(olo_theme::Hue::Blue) };
                Some((vec![(c(hue), used)], s as f64))
            }
            _ => None,
        };
        volumes = volumes.child(
            canvas(
                |_, _, _| {},
                move |r, _, window, cx| {
                    let p = theme::palette(cx);
                    let ink = style::card_ink(&p);
                    let (plain, bold) = (style::ui_font(FontWeight::NORMAL), style::ui_font(FontWeight::BOLD));
                    let d = style::label(window, cx, r.right(), r.top() + px(9.0), style::Align::Right, detail.clone(), style::PLAIN_SIZE, plain.clone(), ink.text);
                    let t = style::label_fit(
                        window,
                        cx,
                        r.left(),
                        r.top() + px(9.0),
                        style::Align::Left,
                        title.clone(),
                        style::PLAIN_SIZE,
                        bold,
                        ink.text,
                        (d.left() - px(m::SPACING) - r.left()).max(px(0.0)),
                    );
                    let s = style::label_fit(window, cx, r.left(), r.top() + px(24.0), style::Align::Left, sub.clone(), style::SMALL_SIZE, plain, ink.dim, r.size.width * 0.5);
                    agent::record(window, cx, agent::Node { value: Some(detail.clone()), ..agent::Node::label(title.clone(), t) });
                    agent::record(window, cx, agent::Node::label(sub.clone(), s));
                    if let Some((parts, total)) = &bar {
                        let b = olo_ui::gpui::Bounds::from_corners(olo_ui::gpui::point(r.left() + r.size.width * 0.5, r.top() + px(21.0)), olo_ui::gpui::point(r.right(), r.top() + px(27.0)));
                        meter(window, cx, b, parts, *total);
                    }
                },
            )
            .w_full()
            .h(px(40.0))
            .flex_shrink_0(),
        );
    }
    out.push(volumes.into_any_element());
    let items = legend_items(app, &p, &["psi-io", "psi-io-full"], &["Some waiting", "All waiting"]);
    out.push(
        Card::new("io-pressure", "I/O Pressure")
            .caption("Time tasks waited for disks")
            .child(graph(app, None, &["psi-io", "psi-io-full"], None, GRAPH * 0.7, cx))
            .child(div().h(px(m::TIGHT)))
            .child(legend_el(items, inner, cx))
            .into_any_element(),
    );
    out
}

// ---- Network ---------------------------------------------------------

fn network(app: &Activity, me: &Me, inner: f32, cx: &App) -> Vec<AnyElement> {
    let p = theme::palette(cx);
    let mut out = Vec::new();
    for n in &app.system.interfaces {
        let keys = [format!("net:{}:rx", n.name), format!("net:{}:tx", n.name)];
        let k: Vec<&str> = keys.iter().map(String::as_str).collect();
        let mut items = legend_items(app, &p, &k, &["In", "Out"]);
        items.push(figure("Received", format_size(n.received)));
        items.push(figure("Sent", format_size(n.sent)));
        if n.errors + n.drops > 0 {
            items.push(figure("Errors", n.errors.to_string()));
            items.push(figure("Dropped", n.drops.to_string()));
        }
        let mut cap = vec![if n.up { "Connected".to_owned() } else { "Not connected".to_owned() }];
        cap.extend(n.addresses.iter().filter(|a| !a.starts_with("fe80")).cloned());
        if !n.mac.is_empty() && n.mac != "00:00:00:00:00:00" {
            cap.push(n.mac.clone());
        }
        cap.push(format!("MTU {}", n.mtu));
        let figure_text = Unit::Rate.format(app.latest(&keys[0]).unwrap_or(0.0) + app.latest(&keys[1]).unwrap_or(0.0));
        out.push(
            Card::new(format!("net-{}", n.name), n.name.clone())
                .figure(figure_text)
                .caption(cap.join(" · "))
                .child(graph(app, Some(me), &k, None, GRAPH * 0.8, cx))
                .child(div().h(px(m::TIGHT)))
                .child(legend_el(items, inner, cx))
                .into_any_element(),
        );
    }
    // Who uses it most (TCP's bytes, `netdiag`).
    let mut busy: Vec<&crate::sample::Proc> = app.processes.iter().filter(|pr| pr.sent.unwrap_or(0.0) + pr.received.unwrap_or(0.0) > 0.0).collect();
    busy.sort_by(|a, b| (b.sent.unwrap_or(0.0) + b.received.unwrap_or(0.0)).total_cmp(&(a.sent.unwrap_or(0.0) + a.received.unwrap_or(0.0))));
    let rows: Vec<Busy> = busy
        .iter()
        .take(8)
        .map(|pr| (pr.pid, format!("{} ({})", pr.name, pr.pid), format!("{} in · {} out", Unit::Rate.format(pr.received.unwrap_or(0.0)), Unit::Rate.format(pr.sent.unwrap_or(0.0)))))
        .collect();
    let caption = if app.access == Access::Full { "TCP's bytes, each second" } else { "TCP's bytes, each second (other users' need full access)" };
    let mut card = Card::new("net-programs", "Programs Using the Network").caption(caption);
    card = if rows.is_empty() { card.child(StringView::new("None just now").height(FACT_ROW).color(style::card_ink(&p).dim)) } else { card.child(process_rows(me, "Received and sent", rows)) };
    out.push(card.into_any_element());
    // Connections: who talks to whom.
    let full = app.access == Access::Full;
    let caption = if full { "Every program's" } else { "Programs of other users show no name without full access" };
    let needle = olo_ui::search::Needle::new(app.connections_search.read(cx).text().trim());
    let mut rows: Vec<&crate::sample::Connection> =
        app.connections.iter().filter(|cn| needle.is_empty() || [cn.local.as_str(), &cn.remote, &cn.state, &cn.process, cn.proto.as_str()].iter().any(|f| needle.in_text(f))).collect();
    let (col, asc) = app.connections_list.read(cx).state.sort.or(app.settings.connections_sort).unwrap_or((4, true));
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
    let cells: Rc<Vec<[String; 5]>> = Rc::new(
        rows.iter()
            .map(|cn| {
                let prog = match cn.pid {
                    Some(pid) => format!("{} ({pid})", cn.process),
                    None => "—".into(),
                };
                // Not connected to anything (listening, or UDP unconnected).
                let remote = if cn.remote.ends_with(":0") && (cn.remote.starts_with("0.0.0.0") || cn.remote.starts_with("[::]")) { "—".into() } else { cn.remote.clone() };
                [cn.proto.clone(), cn.local.clone(), remote, cn.state.clone(), prog]
            })
            .collect(),
    );
    let pids: Vec<Option<i32>> = rows.iter().map(|cn| cn.pid).collect();
    let columns = vec![Column::new("Protocol", 64.0), Column::new("Local", 170.0), Column::new("Remote", 170.0), Column::new("State", 90.0), Column::new("Program", 150.0)];
    let list_cells = cells.clone();
    let (sorted, opened) = (me.clone(), me.clone());
    let list = ColumnListView::new("activity-connections", &app.connections_list, columns, cells.len(), move |cell, window, cx| {
        if let Some(row) = list_cells.get(cell.row) {
            cell.text(window, cx, row[cell.column].clone());
        }
    })
    .empty_text("No connections match.")
    .on_sort(move |_, cx| {
        sorted.update(cx, |a, cx| {
            a.settings.connections_sort = a.connections_list.read(cx).state.sort;
            a.store();
            cx.notify();
        })
    })
    .on_open(move |row, _, cx| {
        if let Some(Some(pid)) = pids.get(row) {
            let pid = *pid;
            opened.update(cx, |a, cx| a.inspect(pid, cx));
        }
    });
    out.push(
        Card::new("connections", "Connections")
            .caption(caption)
            .child(div().w(px(320.0f32.min(inner))).child(TextControl::new(app.connections_search.clone()).label("Search:")))
            .child(div().h(px(m::SPACING)))
            .child(div().h(px(CONNECTIONS_HEIGHT)).flex().flex_col().child(div().flex_1().min_h_0().flex().flex_col().child(list)))
            .into_any_element(),
    );
    out
}

// ---- GPU -------------------------------------------------------------

fn gpu(app: &Activity, me: &Me, area: f32, inner: f32, cx: &App) -> Vec<AnyElement> {
    let p = theme::palette(cx);
    let sys = &app.system;
    let mut out = Vec::new();
    let g_info = sys.gpu.clone().unwrap_or_default();
    let keys = ["gpu-busy"];
    let mut items = legend_items(app, &p, &keys, &["Busy"]);
    items.extend(legend_items(app, &p, &["gpu-jobs"], &["Jobs"]));
    out.push(
        Card::new("gpu", "GPU")
            .figure(value(app, "gpu-busy"))
            .caption(format!("{} · {} driver", g_info.name, g_info.driver))
            .child(graph(app, Some(me), &keys, Some(100.0), GRAPH + 40.0, cx))
            .child(div().h(px(m::TIGHT)))
            .child(legend_el(items, inner, cx))
            .into_any_element(),
    );
    let mut pair = Vec::new();
    if app.source("gpu-power").is_some() {
        pair.push(
            Card::new("gpu-power", "Power").figure(value(app, "gpu-power")).caption("What the GPU draws").child(graph(app, None, &["gpu-power"], None, GRAPH * 0.8 + 16.0, cx)).into_any_element(),
        );
    }
    if app.source("gpu-memory").is_some() {
        let caption = if g_info.memory_limit > 0 { format!("of {} it may use", format_size(g_info.memory_limit)) } else { String::new() };
        let mut card = Card::new("gpu-memory", "Memory").figure(value(app, "gpu-memory")).caption(caption);
        if g_info.memory_limit > 0 {
            card = card.child(meter_el(vec![(c(p.hue(olo_theme::Hue::Red)), g_info.memory_used as f64)], g_info.memory_limit as f64, 8.0)).child(div().h(px(m::SPACING)));
        }
        pair.push(card.child(graph(app, None, &["gpu-memory"], None, GRAPH * 0.8, cx)).into_any_element());
    }
    if !pair.is_empty() {
        out.push(flow(pair, area, 300.0, 2));
    }
    // Who uses it.
    let clients = sys.gpu_clients.clone().unwrap_or_default();
    let rows: Vec<Busy> = clients
        .iter()
        .map(|cl| {
            let busy = cl.busy.map(|b| format!(" · {b:.0}%")).unwrap_or_default();
            (cl.pid, format!("{} ({})", cl.name, cl.pid), format!("{}{busy}", format_size(cl.memory)))
        })
        .collect();
    out.push(
        Card::new("gpu-programs", "Programs Using the GPU")
            .caption(if app.access == Access::Full { "Its memory each holds" } else { "Its memory each holds (other users' need full access)" })
            .child(process_rows(me, "Memory on the GPU", rows))
            .into_any_element(),
    );
    out
}

// ---- Displays --------------------------------------------------------

/// Each display: its frames a second, the ones dropped, what its frames
/// cost app_server and how it paces them (app_server frames.rs), and the
/// switch that has every drop logged with what went into it.
/// A graph of figures kept from app_server's answers
/// (`Activity::frame_history`): each `(key, name, hue)` a line, the first
/// filled, read by `unit`, `GRAPH` tall over the span chosen, with a
/// legend of each line's latest under it.
fn history_graph(app: &Activity, id: &str, lines: &[(&str, &str, olo_theme::Hue)], unit: fn(f64) -> String, max: Option<f64>, inner: f32, cx: &App) -> Vec<AnyElement> {
    let p = theme::palette(cx);
    let series: Vec<Series> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, (key, name, hue))| Some(Series::new(SharedString::from(name.to_string()), c(p.hue(*hue)), app.frame_history.get(*key)?.clone()).fill(i == 0)))
        .collect();
    let items: Vec<LegendItem> = lines
        .iter()
        .map(|(key, name, hue)| {
            let latest = app.frame_history.get(*key).and_then(|q| q.back()).map(|(_, v)| unit(*v)).unwrap_or_else(|| "—".into());
            LegendItem { color: c(p.hue(*hue)), name: SharedString::from(name.to_string()), value: latest.into() }
        })
        .collect();
    let mut g =
        Graph::new(SharedString::from(format!("activity-{id}")), series, app.t()).span(app.settings.span as f64).grid(grid_for(app.settings.span)).unit(unit).smooth(app.settings.smooth).plain();
    if let Some(m) = max {
        g = g.max(m);
    }
    vec![div().h(px(GRAPH)).flex_shrink_0().child(g.on_layer(&app.shelf)).into_any_element(), legend_el(items, inner, cx)]
}

fn ms_value(v: f64) -> String {
    if v < 1.0 { format!("{:.0} µs", v * 1000.0) } else { format!("{v:.1} ms") }
}

fn whole(v: f64) -> String {
    style::thousands(v.max(0.0).round() as usize)
}

fn displays(app: &Activity, me: &Me, inner: f32, cx: &App) -> Vec<AnyElement> {
    let p = theme::palette(cx);
    let ink = style::card_ink(&p);
    let ms = |us: u64| format!("{:.1} ms", us as f64 / 1000.0);
    let mut out = Vec::new();
    if app.displays.is_empty() {
        out.push(
            Card::new("displays-none", "Displays")
                .caption("Nothing to show yet")
                .child(para("app_server hasn't said yet, or this session has no displays of its own (a headless one).", ink.dim))
                .into_any_element(),
        );
    }
    for d in &app.displays {
        let hz = d.refresh_mhz as f64 / 1000.0;
        let frame_ms = 1000.0 / hz.max(1.0);
        let rows: Vec<(String, String)> = vec![
            ("Refresh".into(), format!("{hz:.0} Hz, {frame_ms:.1} ms a frame")),
            ("Now".into(), if d.resting { "Resting: nothing new on it".into() } else { format!("{:.0} frames a second", d.fps) }),
            ("Dropped".into(), format!("{} in the last minute, {} of {} in all", d.late_minute, style::thousands(d.late as usize), style::thousands(d.frames as usize))),
            ("Drawing".into(), format!("{} typically, {} at the 95th percentile, {} to {}", ms(d.render_p50_us), ms(d.render_p95_us), ms(d.render_min_us), ms(d.render_max_us))),
            ("Pacing".into(), format!("{} of each frame kept for drawing: a frame starts {} after the vblank", ms(d.budget_us), ms(d.delay_us))),
            ("Pointer".into(), if d.cursor_plane { "On a plane of its own: moving it draws nothing".into() } else { "Drawn into each frame".into() }),
            ("Scan-out".into(), if d.scanout.is_empty() { "—".into() } else { d.scanout.clone() }),
        ];
        let figure = if d.resting { "resting".to_owned() } else { format!("{:.0} fps", d.fps) };
        let caption = match d.late_minute {
            0 => "Every frame on time this last minute".to_owned(),
            1 => "A frame dropped in the last minute".to_owned(),
            n => format!("{n} frames dropped in the last minute"),
        };
        let mut card = Card::new(format!("display-{}", d.output), d.output.clone()).figure(figure).caption(caption);
        if d.late_minute > 0 {
            card = card.figure_ink(c(p.hue(olo_theme::Hue::Red)));
        }
        let (fps, late, p95) = (format!("display.{}.fps", d.output), format!("display.{}.late", d.output), format!("display.{}.p95", d.output));
        let graph = history_graph(
            app,
            &format!("display-{}", d.output),
            &[(&fps, "Frames a second", olo_theme::Hue::Blue), (&late, "Dropped in the last minute", olo_theme::Hue::Red)],
            whole,
            None,
            inner,
            cx,
        );
        let drawing = history_graph(app, &format!("display-draw-{}", d.output), &[(&p95, "Drawing a frame, 95th percentile", olo_theme::Hue::Teal)], ms_value, None, inner, cx);
        out.push(card.child(facts(rows)).children(graph).children(drawing).into_any_element());
    }
    let toggle = me.clone();
    out.push(
        Card::new("displays-capture", "When a Frame Drops")
            .caption("For finding out why")
            .child(
                CheckBox::new("Capture debug information whenever a frame drops", app.frames_capture)
                    .on_toggle(move |on, _, cx| toggle.update(cx, |a, cx| a.set_frames_capture(on, cx))),
            )
            .child(div().h(px(m::TIGHT)))
            .child(para(
                "Every dropped frame is then logged with what went into it: how long it took to draw and the parts of that, how far past its vblank it showed, and what else held app_server up meanwhile. Open Logs and filter for \"frames\". It stays on until app_server restarts.",
                ink.dim,
            ))
            .into_any_element(),
    );
    out
}

// ---- AI -------------------------------------------------------------

/// "98.8%", "100%".
fn percent(v: f64) -> String {
    let p = v * 100.0;
    if !(0.05..99.95).contains(&p) { format!("{p:.0}%") } else { format!("{p:.1}%") }
}

fn us_text(us: u32) -> String {
    match us {
        0 => "under 1 µs".into(),
        1..1000 => format!("{us} µs"),
        _ => format!("{:.1} ms", us as f64 / 1000.0),
    }
}

/// Preferences ▸ Terminal, its checkbox lit.
fn terminal_preferences() -> Button {
    Button::new("Terminal Preferences…").on_click(|_, _| {
        let args = ["olo-preferences", "terminal", "--show", olo_complete::protocol::OPTION].map(std::ffi::OsString::from);
        let _ = olo_xdg::open::spawn_app(&args, "olo.preferences");
    })
}

/// The models on this computer: brush's command suggestions, what they
/// offered and what was taken, how quickly, and how they learn.
fn ai(app: &Activity, inner: f32, cx: &App) -> Vec<AnyElement> {
    let p = theme::palette(cx);
    let ink = style::card_ink(&p);
    let Some(s) = app.model.as_deref() else {
        let (caption, about) = if app.model_on {
            ("Not running", "On, but olo-complete isn’t running: brush starts it as you next type in it.")
        } else {
            (
                "Off",
                "brush’s grey hint after what you type can come from a tiny model of your own commands, here on this computer: what’s typed, the folder, the commands before it in that shell, the hour and the day. It’s turned on in Preferences ▸ Terminal.",
            )
        };
        let card = Card::new("ai-commands", "Command Suggestions")
            .figure(if app.model_on { "On" } else { "Off" })
            .caption(caption)
            .child(para(about, ink.dim))
            .child(div().h(px(m::SPACING)))
            .child(buttons(vec![terminal_preferences()]));
        return vec![card.into_any_element()];
    };
    let share = |n: u64| if s.offered > 0 { format!(" ({})", percent(n as f64 / s.offered as f64)) } else { String::new() };
    let rows: Vec<(String, String)> = vec![
        ("Offered".into(), format!("{} suggestions since it started", style::thousands(s.offered as usize))),
        ("Taken".into(), format!("{} whole{}, and {} a word at a time", style::thousands(s.accepted as usize), share(s.accepted), style::thousands(s.accepted_words as usize))),
        ("Knows".into(), format!("{} different commands, of {} run", style::thousands(s.known), style::thousands(s.seen as usize))),
        ("Computes with".into(), format!("The {}", s.backend)),
    ];
    let figure = if s.offered > 0 { format!("{} taken", percent(s.accepted as f64 / s.offered as f64)) } else { "nothing yet".into() };
    let mut out = vec![
        Card::new("ai-commands", "Command Suggestions")
            .figure(figure)
            .caption("brush’s grey hints, from a tiny model of your own commands")
            .child(facts(rows))
            .child(div().h(px(m::SPACING)))
            .child(buttons(vec![terminal_preferences()]))
            .into_any_element(),
    ];
    // How quickly: olo-complete's own time for each (brush waits 8 ms at
    // most, then shows its own hint).
    let none = s.latency_count == 0;
    let rows: Vec<(String, String)> = if none {
        vec![("Suggestions".into(), "None asked for yet: type in brush".into())]
    } else {
        vec![
            ("Typically".into(), us_text(s.latency_p50_us)),
            ("95th percentile".into(), us_text(s.latency_p95_us)),
            ("Slowest".into(), format!("{} (of the last {})", us_text(s.latency_worst_us), style::thousands(s.latency_count))),
        ]
    };
    let slow = s.latency_p95_us > 8_000;
    let mut card = Card::new("ai-latency", "Latency").figure(if none { "—".into() } else { us_text(s.latency_p50_us) }).caption(if slow {
        "Slower than brush waits: some hints are brush’s own"
    } else {
        "Each suggestion, from the question to the answer sent"
    });
    if slow {
        card = card.figure_ink(c(p.failure));
    }
    let graph = history_graph(app, "ai-latency", &[("ai.p50", "Typically", olo_theme::Hue::Green), ("ai.p95", "95th percentile", olo_theme::Hue::Orange)], ms_value, None, inner, cx);
    out.push(card.child(facts(rows)).children(graph).into_any_element());
    // How it learns.
    let mut rows: Vec<(String, String)> = vec![(
        "Weights".into(),
        match (s.tuned, s.fine_tunes) {
            (false, _) => "The hand-written ones: not fine-tuned yet".into(),
            (true, 0) => "Yours, as last learned".into(),
            (true, 1) => "Yours: fine-tuned once since it started".into(),
            (true, n) => format!("Yours: fine-tuned {} times since it started", style::thousands(n as usize)),
        },
    )];
    let mut figure = "—".to_owned();
    if let Some(t) = &s.last_tune {
        let when = jiff::Timestamp::from_second(s.last_tune_at).ok().map(|w| w.to_zoned(jiff::tz::TimeZone::system()).strftime("%-d %B at %H:%M").to_string()).unwrap_or_default();
        let ms = t.micros as f64 / 1000.0;
        rows.push(("Last".into(), format!("{when}: {} examples, {} steps, {ms:.1} ms on the {}", style::thousands(t.examples), style::thousands(t.steps), t.backend)));
        // (What's in use: the new weights if kept, else those before.)
        let in_use = if t.kept { t.after.top1 } else { t.before.top1 } as f64;
        rows.push(("Right first".into(), format!("{} of your newest commands, kept back to test it ({} hand-written)", percent(in_use), percent(t.baseline.top1 as f64))));
        rows.push((
            "Kept".into(),
            if t.kept { format!("Yes ({} before)", percent(t.before.top1 as f64)) } else { format!("No: it did no better ({}), so the weights before stay", percent(t.after.top1 as f64)) },
        ));
        figure = format!("{} right first", percent(in_use));
    }
    rows.push(("Waiting".into(), if s.pending > 0 { format!("{} new examples, learned within the minute", style::thousands(s.pending)) } else { "Nothing new to learn".into() }));
    out.push(Card::new("ai-learning", "Learning").figure(figure).caption("Fine-tuned on your newest commands a minute after you type them").child(facts(rows)).into_any_element());
    out
}

// ---- Inputs -----------------------------------------------------------

/// The pointer's and the touchpad's reports as app_server gets them, and
/// what's done with each kind of input, over time.
fn inputs(app: &Activity, inner: f32, cx: &App) -> Vec<AnyElement> {
    let p = theme::palette(cx);
    let ink = style::card_ink(&p);
    let mut out = Vec::new();
    // The pointer's reports, as app_server gets them: the mouse and its
    // link, which no frame can make up for.
    {
        let h = &app.pointer_health;
        let rate = if h.interval_us > 0 { format!("{} Hz", (1_000_000 / h.interval_us.max(1)).min(8000)) } else { "—".to_owned() };
        let ms = |us: u64| format!("{:.0} ms", us as f64 / 1000.0);
        let rows: Vec<(String, String)> = vec![
            ("Reports".into(), format!("{} in the last second, at {rate}", h.reports_second)),
            ("Gaps".into(), if h.gaps_minute == 0 { "None in the last minute".into() } else { format!("{} over 5 ms in the last minute, the longest {}", h.gaps_minute, ms(h.longest_gap_us)) }),
        ];
        let figure = if h.reports_second > 0 { rate.clone() } else { "still".to_owned() };
        let caption = if h.gaps_minute > 0 {
            "Reports going missing while the mouse moves: a receiver asleep (USB autosuspend; Olo's udev rule keeps it awake) or a 2.4 GHz link dropping packets (move the receiver nearer)"
        } else {
            "The mouse's reports arrive as they should"
        };
        let mut card = Card::new("inputs-pointer", "Pointer").figure(figure).caption(caption);
        if h.gaps_minute > 0 {
            card = card.figure_ink(c(p.hue(olo_theme::Hue::Red)));
        }
        let graph = history_graph(app, "pointer", &[("pointer.hz", "Reports a second", olo_theme::Hue::Blue)], whole, None, inner, cx);
        out.push(card.child(facts(rows)).children(graph).into_any_element());
    }
    // The touchpad's reports, through Olo's touch filter and the palm
    // model: judged as each frame comes, on app_server's main thread,
    // so as quick as that thread is free (it draws frames too).
    let t = &app.touch_health;
    if t.filter || t.reports_minute > 0 {
        let us = |us: u64| if us < 1000 { format!("{us} µs") } else { format!("{:.1} ms", us as f64 / 1000.0) };
        let palms = if t.model && t.trained == 0 {
            format!("Olo's own palm model, made from {} touches of real hands (Preferences ▸ Touchpad & Mouse ▸ Touch trains one of yours)", t.contacts)
        } else if t.model {
            let when = jiff::Timestamp::from_second(t.trained as i64).ok().map(|w| w.to_zoned(jiff::tz::TimeZone::system()).strftime("%-d %B %Y").to_string()).unwrap_or_default();
            format!("Your palm model, trained {when} on {} touches", t.contacts)
        } else if t.filter {
            "By their size alone: the palm model is off (Preferences ▸ Touchpad & Mouse ▸ Touch)".into()
        } else {
            "libinput's own: Olo's touch filter is off".into()
        };
        let core = match t.util_min {
            1024.. => "on a fast core at a high clock".to_owned(),
            1.. => format!("taken as at least {}% busy", t.util_min * 100 / 1024),
            0 => "where the scheduler puts it".to_owned(),
        };
        let mut rows: Vec<(String, String)> = vec![
            ("Reports".into(), if t.reports_second > 0 { format!("{} in the last second", t.reports_second) } else { "None in the last second".into() }),
            (
                "Latency".into(),
                if t.reports_minute > 0 {
                    format!(
                        "{} typically, {} at the 99th percentile, the longest {} ({} reports in the last minute)",
                        us(t.latency_p50_us),
                        us(t.latency_p99_us),
                        us(t.latency_max_us),
                        style::thousands(t.reports_minute as usize)
                    )
                } else {
                    "—".into()
                },
            ),
            ("Palms".into(), palms),
        ];
        if t.model {
            rows.push(("Tested".into(), format!("{:.0}% of palms caught, {:.1}% of fingers taken for palms, on touches kept back", t.palms_caught * 100.0, t.fingers_lost * 100.0)));
        }
        rows.push(("Runs on".into(), format!("app_server's main thread: nice {}{}, {core}", t.nice, if t.realtime { ", realtime" } else { "" })));
        // (Half a frame at 120 Hz: a touch waiting that long shows.)
        let slow = t.reports_minute > 0 && t.latency_p99_us > 4_000;
        let figure = if t.reports_minute > 0 { us(t.latency_p50_us) } else { "still".to_owned() };
        let caption = if slow {
            "Some touches wait for app_server: the pad's reports are judged on the thread that draws, and it was busy"
        } else if t.reports_minute > 0 {
            "Touches reach app_server as they come"
        } else {
            "Touch the pad to see how quickly its reports arrive"
        };
        let mut card = Card::new("inputs-touch", "Touchpad").figure(figure).caption(caption);
        if slow {
            card = card.figure_ink(c(p.hue(olo_theme::Hue::Red)));
        }
        let graph = history_graph(
            app,
            "touch",
            &[("touch.p50", "Reaching app_server, typically", olo_theme::Hue::Green), ("touch.p99", "At the 99th percentile", olo_theme::Hue::Orange)],
            ms_value,
            None,
            inner,
            cx,
        );
        out.push(card.child(facts(rows)).children(graph).into_any_element());
    }
    // What's done with each kind of input, a minute back (how many,
    // never which key).
    {
        let a = &app.inputs;
        let rows: Vec<(String, String)> = vec![
            ("Clicks".into(), format!("{} in the last minute", style::thousands(a.clicks_minute as usize))),
            ("Scrolling".into(), format!("{} steps in the last minute (a wheel's notches, a touchpad's moves)", style::thousands(a.scrolls_minute as usize))),
            ("Gestures".into(), format!("{} begun in the last minute (swipes, pinches, holds)", style::thousands(a.gestures_minute as usize))),
            ("Keys".into(), format!("{} pressed in the last minute", style::thousands(a.keys_minute as usize))),
        ];
        let busy = a.clicks_minute + a.scrolls_minute + a.gestures_minute + a.keys_minute;
        let card = Card::new("inputs-activity", "What You Did")
            .figure(if busy > 0 { format!("{} a minute", style::thousands(busy as usize)) } else { "nothing".into() })
            .caption("Each kind of input in the last minute: how many, never which keys");
        let graph = history_graph(
            app,
            "inputs",
            &[
                ("inputs.keys", "Keys", olo_theme::Hue::Purple),
                ("inputs.scrolls", "Scrolling", olo_theme::Hue::Blue),
                ("inputs.clicks", "Clicks", olo_theme::Hue::Orange),
                ("inputs.gestures", "Gestures", olo_theme::Hue::Green),
            ],
            whole,
            None,
            inner,
            cx,
        );
        out.push(card.child(facts(rows)).children(graph).into_any_element());
    }
    let _ = ink;
    out
}

// ---- Sensors ---------------------------------------------------------

fn sensors(app: &Activity, inner: f32, cx: &App) -> Vec<AnyElement> {
    let p = theme::palette(cx);
    let sys = &app.system;
    let mut out = Vec::new();
    if let Some(b) = &sys.battery {
        let mut rows: Vec<(String, String)> = vec![("Status".into(), b.status.clone())];
        if !b.health.is_empty() {
            rows.push(("Health".into(), b.health.clone()));
        }
        if b.design > 0.0 {
            rows.push(("Holds".into(), format!("{:.1} Wh of {:.1} Wh new ({:.0}%)", b.full, b.design, b.full / b.design * 100.0)));
        }
        rows.push(("Now".into(), format!("{:.1} Wh", b.energy)));
        rows.push(("Cycles".into(), b.cycles.to_string()));
        rows.push(("Power".into(), Unit::Watts.format(b.watts.abs())));
        rows.push(("Voltage".into(), format!("{:.2} V", b.volts)));
        if b.watts.abs() > 0.5 {
            let hours = if b.status == "Charging" { (b.full - b.energy) / b.watts.abs() } else { b.energy / b.watts.abs() };
            let label = if b.status == "Charging" { "Full in" } else { "Lasts" };
            rows.push((label.into(), duration(hours * 3600.0)));
        }
        let hue = if b.capacity < 15.0 { olo_theme::Hue::Red } else { olo_theme::Hue::Green };
        out.push(
            Card::new("battery", "Battery")
                .figure(value(app, "battery"))
                .caption(b.model.clone())
                .child(meter_el(vec![(c(p.hue(hue)), b.capacity)], 100.0, 10.0))
                .child(div().h(px(m::SPACING)))
                .child(facts_two(rows))
                .child(div().h(px(m::SPACING)))
                .child(graph(app, None, &["battery"], Some(100.0), GRAPH, cx))
                .into_any_element(),
        );
    }
    for (unit, title, caption) in [(Unit::Celsius, "Temperatures", "Every thermal sensor"), (Unit::Watts, "Power", "What each part draws"), (Unit::Rpm, "Fans", "")] {
        let keys: Vec<(String, String)> = app.sources.iter().filter(|s| s.group == Group::Sensors && s.unit == unit).map(|s| (s.key.clone(), s.name.clone())).collect();
        if keys.is_empty() {
            continue;
        }
        let cells: Vec<AnyElement> = keys
            .iter()
            .map(|(key, name)| {
                // The card says what kind; each says which.
                let short = ["temperature", "Temperature", "Temp", "temp", "power", "Power", "fan"].iter().fold(name.clone(), |n, w| n.replace(&format!(" {w}"), ""));
                mini(app, key, short, None, cx)
            })
            .collect();
        out.push(Card::new(format!("sensors-{title}"), title).caption(caption).child(mini_grid(cells, inner)).into_any_element());
    }
    out
}

// ---- System ----------------------------------------------------------

fn system(app: &Activity, me: &Me, inner: f32, cx: &App) -> Vec<AnyElement> {
    let p = theme::palette(cx);
    let mut out = Vec::new();
    let sys = &app.system;
    let mem = sys.memory.iter().find(|(k, _)| k == "MemTotal").map_or(0, |(_, v)| *v);
    let access = match app.access {
        Access::Full => "Full: every process, every setting",
        Access::Asking => "Asking for the password…",
        Access::Mine => "This account's processes",
    };
    let rows: Vec<(String, String)> = vec![
        ("Model".into(), sys.model.clone()),
        ("Name".into(), sys.host.clone()),
        ("Kernel".into(), sys.kernel.clone()),
        ("Up for".into(), duration(sys.uptime)),
        ("Processor".into(), if sys.cpu_model.is_empty() { format!("{} cores", sys.cores) } else { sys.cpu_model.clone() }),
        ("Memory".into(), format_size(mem)),
        ("Open files".into(), if sys.open_files.1 >= 1 << 62 { format!("{}, no limit", sys.open_files.0) } else { format!("{} of {}", sys.open_files.0, sys.open_files.1) }),
        ("Access".into(), access.to_owned()),
    ];
    let mut card = Card::new("this-computer", "This Computer").child(facts_two(rows));
    if app.access == Access::Mine && crate::helper::way() != crate::helper::Way::Off {
        let me = me.clone();
        card = card.child(div().h(px(m::SPACING))).child(div().flex().child(Button::new("Get Full Access…").min_width(140.0).on_click(move |_, cx| me.update(cx, |a, cx| a.get_full_access(cx)))));
    }
    out.push(card.into_any_element());

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
    let keys = ["psi-cpu", "psi-memory", "psi-io"];
    let items = legend_items(app, &p, &keys, &["CPU", "Memory", "I/O"]);
    out.push(
        Card::new("pressure", "Pressure")
            .caption("Time tasks waited for each, over 10 s, 1 min and 5 min")
            .child(facts(rows))
            .child(div().h(px(m::SPACING)))
            .child(graph(app, None, &keys, None, GRAPH, cx))
            .child(div().h(px(m::TIGHT)))
            .child(legend_el(items, inner, cx))
            .into_any_element(),
    );

    // Tuning: the kernel's knobs.
    let list_h = (sys.tunables.len() as f32 * m::LIST_ROW + HEADER_HEIGHT + 6.0).min(360.0);
    let caption = if app.access == Access::Full { "Double-click one to change it" } else { "Double-click one to change it (with full access)" };
    let cells: Rc<Vec<[String; 3]>> =
        Rc::new(sys.tunables.iter().map(|(k, v)| [k.clone(), v.clone(), crate::sample::TUNABLES.iter().find(|(n, _)| n == k).map_or("", |(_, d)| *d).to_owned()]).collect());
    let columns = vec![Column::new("Setting", 230.0), Column::new("Value", 100.0), Column::new("What it does", 320.0)];
    let (list_cells, open_cells, menu_cells) = (cells.clone(), cells.clone(), cells.clone());
    let (opened, menued) = (me.clone(), me.clone());
    let list = ColumnListView::new("activity-tunables", &app.tunables_list, columns, cells.len(), move |cell, window, cx| {
        if let Some(row) = list_cells.get(cell.row) {
            cell.text(window, cx, row[cell.column].clone());
        }
    })
    .on_open(move |row, _, cx| {
        if let Some(cl) = open_cells.get(row) {
            let name = cl[0].clone();
            opened.update(cx, |a, cx| a.tune(&name, cx));
        }
    })
    .on_context(move |row, at, window, cx| {
        let Some(cl) = menu_cells.get(row) else { return };
        let to = menued.clone();
        menu::context_menu(window, cx, Menu::new("").item("Change…", format!("tune:{}", cl[0])), at, move |cmd, window, cx| to.update(cx, |a, cx| a.command(cmd, Some(window), cx)));
    });
    out.push(Card::new("tuning", "Tuning").caption(caption).child(div().h(px(list_h)).flex().flex_col().child(div().flex_1().min_h_0().flex().flex_col().child(list))).into_any_element());
    out
}

// ---- Graphs ----------------------------------------------------------

/// Haiku's ActivityMonitor: graphs of whatever you choose (right-click a
/// graph to choose; add and remove graphs), sharing the window's height
/// (each card's graph as tall as the others').
fn graphs(app: &Activity, me: &Me, inner: f32, cx: &App) -> AnyElement {
    let p = theme::palette(cx);
    let span = app.settings.span as f64;
    let now = app.now();
    let mut col = div().flex().flex_col().gap(px(GAP)).p(px(GAP)).size_full().bg(c(dashboard::gutter(&p)));
    for (v, keys) in app.settings.views.iter().enumerate() {
        let items: Vec<LegendItem> = keys
            .iter()
            .filter_map(|k| {
                let i = app.source(k)?;
                Some(LegendItem { color: c(p.hue(app.sources[i].color)), name: app.sources[i].name.clone().into(), value: value(app, k).into() })
            })
            .collect();
        let lh = legend_height(cx, inner, &items).max(FACT_ROW);
        // Each source scaled to its own range (its fixed top, or what it
        // shows), so CPU and memory share a graph.
        let s: Vec<Series> = keys
            .iter()
            .filter_map(|k| app.source(k))
            .map(|i| {
                let src = &app.sources[i];
                let unit = src.unit;
                let seen = app.history[i].iter().filter(|(t, _)| *t >= now - span).map(|(_, v)| *v).filter(|v| v.is_finite()).fold(0.0, f64::max);
                Series::new(src.name.clone(), c(p.hue(src.color)), app.history[i].clone()).top(src.max.unwrap_or_else(|| nice_ceiling(seen))).format(move |x| unit.format(x))
            })
            .collect();
        let g = Graph::new(SharedString::from(format!("activity-view-{v}")), s, now).span(span).grid(grid_for(app.settings.span)).smooth(app.settings.smooth).plain();
        let menu_me = me.clone();
        let card = canvas(|_, _, _| {}, |b, _, window, cx| style::card(window, b, &theme::palette(cx), false, false)).absolute().inset_0();
        // (Untitled cards: what isn't graph is the same in each, so the
        // room left is shared evenly between the graphs.)
        let fixed = 2.0 * m::CARD_INSET + m::TIGHT + lh;
        col = col.child(
            div()
                .id(SharedString::from(format!("activity-view-card-{v}")))
                .relative()
                .flex()
                .flex_col()
                .flex_grow()
                .flex_basis(px(fixed))
                .min_h(px(fixed + 48.0))
                .p(px(m::CARD_INSET))
                .on_mouse_down(MouseButton::Right, move |e: &MouseDownEvent, window, cx| {
                    let menu = view_menu(menu_me.read(cx), v);
                    let to = menu_me.clone();
                    menu::context_menu(window, cx, menu, e.position, move |cmd, window, cx| to.update(cx, |a, cx| a.command(cmd, Some(window), cx)));
                })
                .child(card)
                .child(div().flex_1().min_h_0().child(g.on_layer(&app.shelf)))
                .child(div().h(px(m::TIGHT)).flex_shrink_0())
                .child(legend_el(items, inner, cx)),
        );
    }
    col.into_any_element()
}

/// The context menu of graph `v`: every source, by what it's about.
fn view_menu(app: &Activity, v: usize) -> Menu {
    let shown = &app.settings.views[v];
    let item = |menu: Menu, s: &crate::sample::Source| menu.item(s.name.clone(), format!("toggle:{v}:{}", s.key)).checked(shown.contains(&s.key));
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
    menu.separator().item("Add Graph", "add-graph").item("Remove Graph", format!("remove-graph:{v}")).enabled(app.settings.views.len() > 1)
}

/// What the supervisor is asked to change (on a worker: its settings
/// file).
pub enum Supervise {
    On(bool),
    Ignore(String),
    Preferences,
}

/// Do `change`: Preferences opened, or the settings file changed on a
/// worker (the supervisor follows it).
pub fn supervise(change: Supervise) {
    if let Supervise::Preferences = change {
        if let Err(e) = olo_xdg::spawn_detached("olo-preferences performance") {
            tracing::warn!("activity: can't open Preferences: {e}");
        }
        return;
    }
    std::thread::spawn(move || {
        let mut s = olo_protocol::performance::PerformanceSettings::load();
        match change {
            Supervise::On(on) => s.enabled = on,
            Supervise::Ignore(name) if !s.ignored.contains(&name) => s.ignored.push(name),
            _ => return,
        }
        if let Err(e) = s.save() {
            tracing::warn!("activity: the supervisor's settings: {e}");
        }
    });
}

/// Buttons in a row that wraps (`button_flow`'s).
fn buttons(row: Vec<Button>) -> AnyElement {
    div().flex().flex_row().flex_wrap().gap(px(m::SPACING)).children(row).into_any_element()
}

/// The supervisor (Preferences ▸ Performance): whether it's on and what
/// it favours; each finding with how sure it is, why, and what can be
/// done; what it lowered; what it did lately.
fn supervisor(app: &Activity, me: &Me, cx: &App) -> Vec<AnyElement> {
    let p = theme::palette(cx);
    let ink = style::card_ink(&p);
    let st = app.supervisor.clone();
    let mut out = Vec::new();

    // On or off, and what it favours.
    let urgent = st.urgent();
    let favoured = st.favoured.as_ref().map_or_else(|| "—".to_owned(), |(pid, app_id)| format!("{app_id} ({pid})"));
    let rows: Vec<(String, String)> = if st.running {
        vec![
            ("Favoured".into(), favoured),
            (
                "Olo's own".into(),
                match st.raised.len() {
                    0 => "Each above everything else, as it started (rtkit)".to_owned(),
                    _ => st.raised.iter().map(|r| format!("{} to nice {}", r.name, r.nice)).collect::<Vec<_>>().join(", "),
                },
            ),
            (
                "Lowered".into(),
                match st.lowered.len() {
                    0 => "Nothing".to_owned(),
                    1 => "1 process".to_owned(),
                    n => format!("{n} processes"),
                },
            ),
            ("Memory free".into(), format!("{:.0}%", st.memory_available)),
            ("Memory stalls".into(), format!("{:.0}%", st.memory_stall)),
        ]
    } else {
        Vec::new()
    };
    let about = if st.running {
        "Builds and other background work are lowered so the active window’s app and Olo itself stay quick; stuck processes, leaks and memory hogs are watched for."
    } else {
        "Off. Turned on, it lowers builds and other background work so the active window’s app and Olo itself stay quick, and watches for stuck processes, leaks and memory hogs."
    };
    let figure_text = if st.running { "On" } else { "Off" };
    let caption = match (st.running, urgent, st.findings.len()) {
        (false, ..) => String::new(),
        (true, 0, 0) => "Nothing found".to_owned(),
        (true, 0, n) => format!("{n} finding{}", if n == 1 { "" } else { "s" }),
        (true, u, n) => format!("{n} finding{}, {u} urgent", if n == 1 { "" } else { "s" }),
    };
    let mut card = Card::new("supervisor", "Supervisor").figure(figure_text).caption(caption).child(para(about, ink.dim)).child(div().h(px(m::SPACING)));
    if !rows.is_empty() {
        card = card.child(facts_two(rows)).child(div().h(px(m::SPACING)));
    }
    let on = st.running;
    card = card.child(buttons(vec![
        Button::new(if st.running { "Turn Off" } else { "Turn On" }).on_click(move |_, _| supervise(Supervise::On(!on))),
        Button::new("Performance Preferences…").on_click(|_, _| supervise(Supervise::Preferences)),
    ]));
    out.push(card.into_any_element());

    // Each finding: why, how sure, and what to do.
    for f in &st.findings {
        let title = format!("{}: {}", f.kind.title(), f.name);
        let since = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(f.since);
        let ago = std::time::SystemTime::now().duration_since(since).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        let caption = format!("{}pid {} · found {} ago", if f.urgent { "Urgent · " } else { "" }, f.pid, duration(ago));
        let alarm = c(if f.urgent { p.failure } else { p.warning });
        let mut card = Card::new(format!("finding-{}-{}", f.pid, f.kind as u8), title)
            .figure(format!("{:.0}% sure", f.confidence * 100.0))
            .figure_ink(alarm)
            .caption(caption)
            .child(para(f.summary.clone(), ink.text))
            .child(div().h(px(m::TIGHT)));
        for e in &f.evidence {
            card = card.child(para(format!("•  {e}"), ink.dim));
        }
        let (pid, name) = (f.pid, f.name.clone());
        let (inspect, stop) = (me.clone(), me.clone());
        card = card.child(div().h(px(m::SPACING))).child(buttons(vec![
            Button::new("Inspect").id(format!("finding-inspect-{pid}")).on_click(move |_, cx| inspect.update(cx, |a, cx| a.inspect_at(pid, crate::inspector::SUPERVISOR_TAB, cx))),
            Button::new("Stop Process").id(format!("finding-stop-{pid}")).on_click(move |_, cx| stop.update(cx, |a, cx| a.act(Action::Signal { pid, signal: libc::SIGTERM }, cx))),
            Button::new("Ignore This Process").id(format!("finding-ignore-{pid}")).on_click(move |_, _| supervise(Supervise::Ignore(name.clone()))),
        ]));
        out.push(card.into_any_element());
    }

    // What it lowered.
    if st.running && !st.lowered.is_empty() {
        const SHOWN: usize = 12;
        // Each wrapped: what it is, then what was done and why.
        let mut card = Card::new("lowered", "Lowered").caption("Background work, so the rest stays quick");
        for l in st.lowered.iter().take(SHOWN) {
            card = card.child(para(format!("{} ({}): nice {} → {}, {} scheduling, {} I/O, as {}", l.name, l.pid, l.nice_before, l.nice, l.policy, l.io, l.why), ink.text));
        }
        let more = st.lowered.len().saturating_sub(SHOWN);
        if more > 0 {
            card = card.child(para(format!("And {more} more"), ink.text));
        }
        out.push(card.into_any_element());
    }

    // What it did lately, newest first.
    if !st.log.is_empty() {
        const SHOWN: usize = 12;
        let rows: Vec<(String, String)> = st.log.iter().rev().take(SHOWN).map(|e| (clock(e.at as f64), e.text.clone())).collect();
        out.push(Card::new("lately", "Lately").child(facts(rows)).into_any_element());
    }
    out
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

    #[test]
    fn columns_fit() {
        let w = columns(800.0, 6, 250.0, 3);
        assert_eq!(w.len(), 3);
        assert_eq!(w.iter().sum::<f32>() + 2.0 * GAP, 800.0);
        assert_eq!(columns(400.0, 6, 250.0, 3).len(), 1);
        assert_eq!(columns(800.0, 2, 250.0, 3).len(), 2);
    }
}
