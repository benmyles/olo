//! The sections that are cards of graphs and facts: Overview, the
//! Supervisor's work, CPU, Memory, Disks, Network, GPU, Displays, Inputs,
//! Sensors, System and the Graphs of your choosing. (Processes is
//! processes.rs.)

use std::rc::Rc;

use olo_kit::{
    Bounds, Cx, Element, FontWeight, Hsla, InteractiveElement as _, IntoElement as _, Menu, MouseButton, ParentElement as _, Pixels, SharedString, Styled as _, ViewRef, agent, canvas, div,
    files::format_size,
    menu, metrics as m, point, px,
    style::{self, c},
    theme, transparent_black,
    widgets::{
        Button, CheckBox, Column, ColumnListView, Graph, HEADER_HEIGHT, LegendItem, MenuField, SCROLL_BAR, ScrollView, Series, StringView, TextControl,
        charts::FACT_ROW,
        dashboard::{self, Card, Facts, GAP, meter},
        legend, legend_height, nice_ceiling,
    },
};
use olo_theme::Palette;

use crate::{
    Access, Activity, Section,
    actions::Action,
    sample::{Group, Unit},
};

/// A big graph's height in a card.
const GRAPH: f32 = 120.0;
/// A small one's (each core, each sensor).
const MINI_GRAPH: f32 = 44.0;
const MINI_MIN_WIDTH: f32 = 150.0;
/// The room for a column of names and their values when two lists go abreast.
const FACTS_LEAST: f32 = 260.0;
const OVERVIEW_CARD: f32 = 196.0;
const CONNECTIONS_HEIGHT: f32 = 320.0;

type Me = ViewRef<Activity>;

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
fn graph(app: &Activity, drill: Option<&Me>, keys: &[&str], max: Option<f64>, h: f32, _: &Cx<Activity>) -> Element {
    let p = theme::palette();
    let s: Vec<Series> = keys.iter().enumerate().filter_map(|(i, k)| series(app, &p, k, i == 0)).collect();
    let unit = keys.first().and_then(|k| app.source(k)).map_or(Unit::Percent, |i| app.sources[i].unit);
    let id = SharedString::from(format!("activity-{}", keys.join(",")));
    let mut g = Graph::live(id, s, app.graph_clock()).span(app.settings.span as f64).grid(grid_for(app.settings.span)).unit(move |v| unit.format(v)).smooth(app.settings.smooth).plain();
    if let Some(m) = max {
        g = g.max(m);
    }
    if let Some(me) = drill {
        let me = *me;
        g = g.on_click(move |t, cx| {
            let t = *t;
            me.update(cx, move |a, cx| {
                a.drill = Some(t);
                cx.invalidate();
            })
        });
    }
    div().h(px(h)).flex_shrink_0().child(g).into_any_element()
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
fn legend_el(items: Vec<LegendItem>, width: f32, _: &Cx<Activity>) -> Element {
    let h = legend_height(width, &items);
    legend_row(items, h)
}

fn legend_row(items: Vec<LegendItem>, h: f32) -> Element {
    canvas(move |g, b| legend(g, b, &items)).w_full().h(px(h)).flex_shrink_0().into_any_element()
}

/// A bar of parts of a whole (`dashboard::meter`), `h` tall.
fn meter_el(parts: Vec<(Hsla, f64)>, total: f64, h: f32) -> Element {
    canvas(move |g, b| meter(g, b, &parts, total)).w_full().h(px(h)).flex_shrink_0().into_any_element()
}

/// Keep the kit's compact facts where every value fits. Longer accounts
/// (a display's pacing, the supervisor's log) wrap in the value column.
fn facts(rows: Vec<(String, String)>, width: f32) -> Element {
    let rows: Vec<_> = rows.into_iter().map(|(name, value)| (olo_kit::words::of(name), SharedString::from(value))).collect();
    let divider = dashboard::facts_label_width(rows.iter().map(|(label, _)| label.as_ref()));
    let font = style::ui_font(FontWeight::NORMAL);
    if rows.iter().all(|(_, v)| divider + f32::from(style::text_width(v.clone(), style::PLAIN_SIZE, font.clone())).ceil() <= width) {
        return Facts::new(rows).into_any_element();
    }
    let ink = style::card_ink(&theme::palette());
    let label_w = (divider - m::LABEL_GAP).min(width / 2.0);
    div()
        .flex()
        .flex_col()
        .children(rows.into_iter().map(|(label, value)| {
            div()
                .agent("Label", label.clone())
                .agent_value(value.clone())
                .flex()
                .flex_row()
                .min_h(px(FACT_ROW))
                .gap(px(m::LABEL_GAP))
                .child(div().w(px(label_w)).flex_shrink_0().child(para(label, ink.dim)))
                .child(div().flex_1().min_w_0().child(para(value, ink.text)))
        }))
        .into_any_element()
}

/// Facts in two columns where both fit, read down the first then the second.
fn facts_two(mut rows: Vec<(String, String)>, width: f32) -> Element {
    let widths = columns(width, rows.len(), FACTS_LEAST, 2);
    if widths.len() == 1 {
        return facts(rows, width);
    }
    let right = rows.split_off(rows.len().div_ceil(2));
    flow(vec![facts(rows, widths[0]), facts(right, widths[1])], width, FACTS_LEAST, 2)
}

/// A caption wider than its card continues underneath the heading.
trait CardCaption {
    fn caption_within(self, text: impl Into<SharedString>, width: f32) -> Self;
}

impl CardCaption for Card {
    fn caption_within(self, text: impl Into<SharedString>, width: f32) -> Self {
        let text = olo_kit::words::of(text);
        if f32::from(style::text_width(text.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL))).ceil() <= width {
            self.caption(text)
        } else {
            self.child(StringView::new(text).small(true).wrap(true).color(style::card_ink(&theme::palette()).dim)).child(div().h(px(m::TIGHT)))
        }
    }
}

/// A device or process can have a long name. If the heading would cut it
/// short, use the kind of item as the heading and put its name in the caption.
fn named_card(id: String, name: String, kind: &'static str, figure: Option<String>, caption: impl Into<SharedString>, inner: f32) -> Card {
    let font = style::ui_font(FontWeight::BOLD);
    let name = SharedString::from(name);
    let figure = figure.map(SharedString::from);
    let figure_w = figure.as_ref().map_or(0.0, |f| f32::from(style::text_width(f.clone(), dashboard::FIGURE_SIZE, font.clone())) + m::SPACING);
    let caption = olo_kit::words::of(caption);
    let (title, caption) =
        if f32::from(style::text_width(name.clone(), style::PLAIN_SIZE, font)).ceil() + figure_w <= inner { (name, caption) } else { (kind.into(), format!("{name} · {caption}").into()) };
    let card = Card::new(id, title).caption_within(caption, inner);
    if let Some(f) = figure { card.figure(f) } else { card }
}

/// Text wrapped to the card, quiet or not.
fn para(text: impl Into<SharedString>, color: Hsla) -> Element {
    StringView::new(text).wrap(true).color(color).into_any_element()
}

/// A small graph with its name and value over it (a core, a sensor).
fn mini(app: &Activity, key: &str, label: String, max: Option<f64>, cx: &Cx<Activity>) -> Element {
    if app.source(key).is_none() {
        return div().into_any_element();
    }
    let v = value(app, key);
    let ink = style::card_ink(&theme::palette());
    let head = div()
        .flex()
        .flex_row()
        .gap(px(m::SPACING))
        .agent("Label", label.clone())
        .agent_value(v.clone())
        .child(div().flex_1().min_w_0().child(StringView::new(label).small(true).wrap(true).color(ink.dim)))
        .child(StringView::new(v).small(true).height(m::LABEL_ROW).color(ink.text));
    div().flex().flex_col().pb(px(4.0)).child(head).child(graph(app, None, &[key], max, MINI_GRAPH, cx)).into_any_element()
}

/// How many columns of at least `min` go in `width` (at most `max`), and
/// each one's width (the last takes what rounding left).
fn columns(width: f32, n: usize, min: f32, max: usize) -> Vec<f32> {
    let cols = dashboard::across(width, min, max).min(n.max(1));
    let w = ((width - GAP * (cols - 1) as f32) / cols as f32).floor();
    (0..cols).map(|i| if i == cols - 1 { width - w * (cols - 1) as f32 - GAP * (cols - 1) as f32 } else { w }).collect()
}

/// `items` in rows of as many columns as fit, cells in a row as tall as
/// the tallest.
fn flow(items: Vec<Element>, width: f32, min: f32, max: usize) -> Element {
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
fn mini_grid(cells: Vec<Element>, width: f32) -> Element {
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
            _ if app.model_known && !app.model_on => "Off".to_owned(),
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
pub fn view(app: &mut Activity, s: Section, width: f32, cx: &mut Cx<Activity>) -> Element {
    let me = cx.view_ref();
    let p = theme::palette();
    // Inside the gutter round the cards, the scroll bar and the cards' inset.
    if s == Section::Graphs {
        return graphs(app, width, cx);
    }
    let area = width - SCROLL_BAR - 2.0 * GAP;
    let inner = area - 2.0 * m::CARD_INSET;
    let cards: Vec<Element> = match s {
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
        Section::Supervisor => supervisor(app, &me, inner, cx),
        Section::Processes | Section::Graphs => Vec::new(),
    };
    let body = div().flex().flex_col().gap(px(GAP)).p(px(GAP)).children(cards);
    ScrollView::new(&app.scroll).border(false).background(dashboard::gutter(&p)).child(body).into_any_element()
}

// ---- Overview --------------------------------------------------------

fn overview(app: &Activity, me: &Me, area: f32, cx: &Cx<Activity>) -> Vec<Element> {
    let p = theme::palette();
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
            Section::Memory => (
                "Memory",
                value(app, "memory"),
                olo_kit::tr!("of {} · {} swap used", format_size(mem_total), value(app, "swap")),
                vec!["memory", "cached"],
                vec!["Used", "Cached"],
                Some(mem_total as f64),
            ),
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
                sys.volumes.first().and_then(|v| Some(olo_kit::tr!("{} free on {}", format_size(v.free?), v.mount.split(", ").next().unwrap_or(&v.mount)))).unwrap_or_default(),
                vec!["disk-read", "disk-write"],
                vec!["Read", "Written"],
                None,
            ),
            Section::Gpu => {
                // (The one the computer started on; the others in the GPU section.)
                let more = sys.gpus.len().saturating_sub(1);
                let first = sys.gpus.first().map(|g| format!("{} · {}", g.name, g.driver)).unwrap_or_default();
                ("GPU", value(app, "gpu-busy"), if more > 0 { olo_kit::tr!("{} · and {} more", first, more) } else { first }, vec!["gpu-busy"], vec!["Busy"], Some(100.0))
            }
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
        let lh = legend_height(card_inner, &items);
        let head = dashboard::Card::height(0.0, !caption.is_empty());
        let graph_h = (OVERVIEW_CARD - head - lh - m::TIGHT).max(20.0);
        let go = *me;
        let section = *s;
        let mut card =
            Card::new(format!("overview-{title}"), title).figure(figure_text).caption_within(caption, card_inner).on_click(move |_, cx| go.update(cx, move |a, cx| a.set_section(section, cx)));
        if !keys.is_empty() {
            card = card.child(graph(app, Some(me), &keys, max, graph_h, cx));
        } else if *s == Section::Sensors {
            // No battery: the hottest few sensors' graph.
            let mut temps: Vec<(usize, f64)> = app.sources.iter().enumerate().filter(|(_, s)| s.unit == Unit::Celsius).filter_map(|(i, _)| Some((i, app.history[i].back()?.1))).collect();
            temps.sort_by(|a, b| b.1.total_cmp(&a.1));
            let keys: Vec<&str> = temps.iter().take(3).map(|(i, _)| app.sources[*i].key.as_str()).collect();
            card = card.child(graph(app, None, &keys, None, graph_h, cx));
        }
        card = card.child(div().h(px(m::TIGHT))).child(legend_row(items, lh));
        out.push(div().min_h(px(OVERVIEW_CARD)).flex().flex_col().child(card).into_any_element());
    }
    vec![flow(out, area, 250.0, 3), busiest(app, me, area - 2.0 * m::CARD_INSET)]
}

/// A process and its figure: (pid, name, figure).
type Busy = (i32, String, String);

/// The busiest processes now, or at the moment chosen on a graph.
fn busiest(app: &Activity, me: &Me, inner: f32) -> Element {
    let rows = 6;
    let moment = app.drill.and_then(|t| app.moments.iter().rev().find(|m| m.at <= t + 0.01).or(app.moments.front()));
    let (title, caption) = match moment {
        Some(m) => {
            let ago = app.t() - m.at;
            (format!("At {}", clock(wall(app, m.at))), olo_kit::tr!("{} ago · click a graph for another moment", duration(ago)))
        }
        None => (olo_kit::words::tr("Busiest Now").to_owned(), olo_kit::words::tr("Click a graph to see what was busy then").to_owned()),
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
    let widths = columns(inner, 2, FACTS_LEAST, 2);
    let lists = flow(vec![process_rows(me, "CPU", cpu, widths[0]), process_rows(me, other_title, other, *widths.last().unwrap())], inner, FACTS_LEAST, 2);
    let card = Card::new("busiest", title).caption_within(caption, inner).child(lists);
    if moment.is_none() {
        return card.into_any_element();
    }
    // Now, at the head's right, back to the present.
    let now = *me;
    div()
        .relative()
        .flex()
        .flex_col()
        .child(card)
        .child(div().absolute().top(px(m::CARD_INSET - 2.0)).right(px(m::CARD_INSET)).child(Button::new("Now").on_click(move |_, cx| {
            now.update(cx, move |a, cx| {
                a.drill = None;
                cx.invalidate();
            })
        })))
        .into_any_element()
}

/// A small list of processes (name and a figure) under a quiet head,
/// each a way to it in Processes. Single-line rows are drawn as one,
/// each `FACT_ROW` from the last as facts are.
fn process_rows(me: &Me, head: &str, rows: Vec<Busy>, width: f32) -> Element {
    let head = SharedString::from(head.to_owned());
    let n = rows.len();
    let button = |pid: i32| {
        let me = *me;
        olo_kit::control_canvas(|g, b, state| {
            if state.hovered {
                g.paint_quad(olo_kit::fill(style::snap(g, b), style::card_ink(&theme::palette()).hover).corner_radii(px(3.0)));
            }
        })
        .id(format!("busy-{head}-{pid}"))
        .paint_key(0)
        .on_mouse_down(MouseButton::Left, move |_, cx| me.update(cx, move |a, cx| a.run(&format!("select:{pid}"), cx)))
        .absolute()
        .left(px(-4.0))
        .right(px(-4.0))
    };
    let font = style::ui_font(FontWeight::NORMAL);
    let fits = |(_, name, fig): &Busy| {
        let measure = |text: &str| f32::from(style::text_width(text.to_owned(), style::PLAIN_SIZE, font.clone())).ceil();
        measure(name) + m::SPACING + measure(fig) <= width
    };
    if !rows.iter().all(fits) {
        let ink = style::card_ink(&theme::palette());
        let mut list = div().flex().flex_col().child(StringView::new(head.clone()).small(true).height(FACT_ROW).color(ink.dim));
        for (pid, name, fig) in rows {
            let fig_w = f32::from(style::text_width(fig.clone(), style::PLAIN_SIZE, font.clone())).ceil().min(width / 2.0);
            list = list.child(
                div()
                    .relative()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(m::SPACING))
                    .min_h(px(FACT_ROW))
                    .child(button(pid).top(px(0.0)).bottom(px(0.0)).agent("Button", name.clone()).agent_value(fig.clone()))
                    .child(div().flex_1().min_w_0().child(para(name, ink.text)))
                    .child(div().w(px(fig_w)).flex_shrink_0().child(para(fig, ink.text))),
            );
        }
        return list;
    }
    let row = |b: Bounds<Pixels>, i: usize| Bounds::from_corners(point(b.left(), b.top() + px((i + 1) as f32 * FACT_ROW)), point(b.right(), b.top() + px((i + 2) as f32 * FACT_ROW)));
    let mut out = div().relative().w_full().h(px((n + 1) as f32 * FACT_ROW)).flex_shrink_0();
    // Under each row, what lights it under the pointer and takes its
    // press (it alone is drawn again).
    for (i, (pid, _, _)) in rows.iter().enumerate() {
        out = out.child(button(*pid).top(px((i + 1) as f32 * FACT_ROW)).h(px(FACT_ROW)));
    }
    let key = style::key((&head, &rows));
    out.child(
        canvas(move |g, b| {
            let p = theme::palette();
            let ink = style::card_ink(&p);
            let font = style::ui_font(FontWeight::NORMAL);
            let h = style::label(g, b.left(), b.top() + px(FACT_ROW / 2.0), style::Align::Left, head.clone(), style::SMALL_SIZE, font.clone(), ink.dim);
            agent::record(g, agent::Node::label(head.clone(), h));
            for (i, (_, name, fig)) in rows.iter().enumerate() {
                let r = row(b, i);
                let mid = r.top() + px(FACT_ROW / 2.0);
                let f = style::label(g, r.right(), mid, style::Align::Right, fig.clone(), style::PLAIN_SIZE, font.clone(), ink.text);
                style::label_fit(g, r.left(), mid, style::Align::Left, name.clone(), style::PLAIN_SIZE, font.clone(), ink.text, (f.left() - px(m::SPACING) - r.left()).max(px(0.0)));
                agent::record(g, agent::Node { value: Some(fig.clone().into()), ..agent::Node::button(name.clone(), r) });
            }
        })
        .paint_key(key)
        .absolute()
        .inset_0(),
    )
    .into_any_element()
}

// ---- CPU -------------------------------------------------------------

fn cpu(app: &Activity, me: &Me, inner: f32, cx: &Cx<Activity>) -> Vec<Element> {
    let p = theme::palette();
    let sys = &app.system;
    let mut out = Vec::new();
    // Everything.
    let keys = ["cpu", "cpu-user", "cpu-system", "cpu-iowait"];
    let items = legend_items(app, &p, &keys, &["Busy", "User", "System", "I/O wait"]);
    let caption = if sys.cpu_model.is_empty() { format!("{} cores", sys.cores) } else { sys.cpu_model.clone() };
    out.push(
        Card::new("cpu", "CPU")
            .figure(value(app, "cpu"))
            .caption_within(caption, inner)
            .child(graph(app, Some(me), &keys, Some(100.0), GRAPH + 40.0, cx))
            .child(div().h(px(m::TIGHT)))
            .child(legend_el(items, inner, cx))
            .into_any_element(),
    );

    // Each core.
    let cores: Vec<usize> = app.sources.iter().enumerate().filter(|(_, s)| s.group == Group::Cpus).map(|(i, _)| i).collect();
    let cells: Vec<Element> = cores
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
        let mut card = Card::new("frequencies", "Frequencies").caption_within("Each cluster's cores change speed together; the governor decides how", inner);
        let field_w = 220.0f32.min(inner);
        for (k, pol) in sys.policies.iter().enumerate() {
            let label: SharedString = format!("{}: {} – {}, now {}", names[k], ghz(pol.min), ghz(pol.max), ghz(pol.now)).into();
            let chosen = pol.governors.iter().position(|g| *g == pol.governor).unwrap_or(0);
            let options: Vec<SharedString> = pol.governors.iter().map(|g| SharedString::from(g.clone())).collect();
            let label_w = f32::from(style::text_width(label.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL))).ceil();
            let mut row = if options.is_empty() || label_w + m::SPACING + field_w > inner {
                div().flex().flex_col().items_start().gap(px(m::TIGHT)).child(para(label, style::card_ink(&p).text))
            } else {
                div().flex().flex_row().items_center().justify_between().gap(px(m::SPACING)).child(StringView::new(label).color(style::card_ink(&p).text))
            }
            .mb(px(m::TIGHT));
            if !options.is_empty() {
                let (policy, governors, act) = (pol.id, pol.governors.clone(), *me);
                row = row.child(MenuField::new(SharedString::from(format!("governor-{}", pol.id)), options, chosen).label("Governor:").width(field_w).on_change(move |i, cx| {
                    let governor = governors[*i].clone();
                    act.update(cx, move |a, cx| a.act(Action::Governor { policy, governor }, cx));
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
        let mut rows: Vec<(String, String)> = vec![("Running".into(), olo_kit::tr!("{}% of the time", format!("{:.1}", busy)))];
        for st in &sys.idle_states {
            let what = if st.what.is_empty() { st.name.clone() } else { format!("{} ({})", st.name, st.what) };
            rows.push((what, olo_kit::tr!("{}% of the time, waking in {} µs", format!("{:.1}", st.share), st.latency_us)));
        }
        let hues = [olo_theme::Hue::Blue, olo_theme::Hue::Teal, olo_theme::Hue::Green, olo_theme::Hue::Purple, olo_theme::Hue::Gray];
        let mut parts = vec![(c(p.hue(hues[0])), busy)];
        parts.extend(sys.idle_states.iter().enumerate().map(|(i, st)| (c(p.hue(hues[(i + 1) % hues.len()])), st.share)));
        out.push(
            Card::new("idle", "Idle States")
                .caption_within("Where the cores' time goes: the deeper a state, the less power and the slower to wake", inner)
                .child(meter_el(parts, 100.0, 10.0))
                .child(div().h(px(m::SPACING)))
                .child(facts(rows, inner))
                .into_any_element(),
        );
    }

    // Load.
    let rows: Vec<(String, String)> = vec![
        (olo_kit::words::tr("Load average").into(), format!("{:.2}, {:.2}, {:.2} (1, 5 and 15 minutes)", sys.load[0], sys.load[1], sys.load[2])),
        ("Processes".into(), value(app, "processes")),
        ("Threads".into(), value(app, "threads")),
        (olo_kit::words::tr("Context switches").into(), value(app, "switches")),
        ("Interrupts".into(), value(app, "interrupts")),
        (olo_kit::words::tr("Page faults").into(), value(app, "faults")),
    ];
    out.push(Card::new("load", "Load").child(facts_two(rows, inner)).child(div().h(px(m::SPACING))).child(graph(app, None, &["load"], None, GRAPH, cx)).into_any_element());
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

fn memory(app: &Activity, area: f32, inner: f32, cx: &Cx<Activity>) -> Vec<Element> {
    let p = theme::palette();
    let sys = &app.system;
    let field = |k: &str| sys.memory.iter().find(|(n, _)| n == k).map_or(0, |(_, v)| *v);
    let total = field("MemTotal");
    let mut out = Vec::new();
    let keys = ["memory", "cached", "swap"];
    let items = legend_items(app, &p, &keys, &["Used", "Cached", "Swap"]);
    let caption = format!("of {} · {} available", format_size(total), format_size(field("MemAvailable")));
    let used = app.latest("memory").unwrap_or(0.0);
    let cached = app.latest("cached").unwrap_or(0.0);
    let hue = |k: &str| app.source(k).map_or(olo_kit::opaque_grey(0.5, 1.0), |i| c(p.hue(app.sources[i].color)));
    out.push(
        Card::new("memory", "Memory")
            .figure(value(app, "memory"))
            .caption_within(caption, inner)
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
                (olo_kit::words::tr("Compressed to").into(), format!("{} ({ratio:.1} : 1, {})", format_size(z.compressed), z.algorithm)),
                ("Using".into(), olo_kit::tr!("{} of memory", format_size(z.used))),
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
            let mut v = vec![(olo_kit::words::tr("Some waiting").to_owned(), format!("{:.1}%, {:.1}%, {:.1}%", pr.some[0], pr.some[1], pr.some[2]))];
            if let Some(f) = pr.full {
                v.push((olo_kit::words::tr("All waiting").into(), format!("{:.1}%, {:.1}%, {:.1}%", f[0], f[1], f[2])));
            }
            v
        })
        .unwrap_or_default();
    let zcap = if sys.zram.is_empty() { "No compressed swap" } else { "Swap in memory, compressed" };
    let widths = columns(area, 2, 300.0, 2);
    let zram_inner = widths[0] - 2.0 * m::CARD_INSET;
    let pressure_inner = widths.last().unwrap() - 2.0 * m::CARD_INSET;
    let zram = Card::new("zram", "Compressed Swap").caption_within(zcap, zram_inner).child(facts(rows, zram_inner)).into_any_element();
    let pressure = Card::new("memory-pressure", "Memory Pressure")
        .caption_within("Time tasks waited for memory: 10 s, 1 min, 5 min", pressure_inner)
        .child(facts(prow, pressure_inner))
        .child(div().h(px(m::SPACING)))
        .child(graph(app, None, &["psi-memory", "psi-memory-full"], None, GRAPH * 0.6, cx))
        .into_any_element();
    out.push(flow(vec![zram, pressure], area, 300.0, 2));

    // Every counter the kernel keeps.
    let all: Vec<(String, String)> = sys.memory.iter().map(|(k, v)| (k.clone(), if k.starts_with("HugePages_") { v.to_string() } else { format_size(*v) })).collect();
    out.push(Card::new("meminfo", "Every Counter").caption("/proc/meminfo").child(facts_two(all, inner)).into_any_element());
    out
}

// ---- Disks -----------------------------------------------------------

fn disks(app: &Activity, me: &Me, inner: f32, cx: &Cx<Activity>) -> Vec<Element> {
    let p = theme::palette();
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
                ("Health".into(), olo_kit::tr!("{}, {}% of its life used", h.verdict(), h.used)),
                ("Spare".into(), olo_kit::tr!("{}% left (warns under {}%)", h.spare, h.spare_threshold)),
                ("Written".into(), format_size(h.written_bytes.min(u64::MAX as u128) as u64)),
                ("Read".into(), format_size(h.read_bytes.min(u64::MAX as u128) as u64)),
                (olo_kit::words::tr("Powered on").into(), format!("{} hours, {} times", h.power_on_hours, h.power_cycles)),
                (olo_kit::words::tr("Unsafe shutdowns").into(), h.unsafe_shutdowns.to_string()),
                (olo_kit::words::tr("Media errors").into(), h.media_errors.to_string()),
                ("Temperature".into(), Unit::Celsius.format(h.temperature)),
            ],
            Some(None) => vec![("Health".into(), olo_kit::words::tr("Needs full access").into())],
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
        let mut card = named_card(format!("disk-{}", d.name), d.name.clone(), "Disk", Some(figure_text), caption, inner)
            .child(graph(app, Some(me), &k[..2], None, GRAPH, cx))
            .child(div().h(px(m::TIGHT)))
            .child(legend_el(items, inner, cx));
        if !hrows.is_empty() {
            card = card.child(div().h(px(m::SPACING))).child(facts_two(hrows, inner));
        }
        out.push(card.into_any_element());
    }
    // Volumes.
    let mut volumes = Card::new("volumes", "Volumes");
    for v in &sys.volumes {
        let title: SharedString = format!("{}{}", v.mount, if v.read_only { " (read-only)" } else { "" }).into();
        let detail: SharedString = match (v.size, v.free) {
            (Some(s), Some(f)) => olo_kit::tr!("{} free of {}", format_size(f), format_size(s)),
            _ => olo_kit::words::tr("On the network").into(),
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
        let measure = |s: &SharedString, size, weight| f32::from(style::text_width(s.clone(), size, style::ui_font(weight))).ceil();
        if measure(&title, style::PLAIN_SIZE, FontWeight::BOLD) + m::SPACING + measure(&detail, style::PLAIN_SIZE, FontWeight::NORMAL) > inner
            || measure(&sub, style::SMALL_SIZE, FontWeight::NORMAL) > inner / 2.0
        {
            let ink = style::card_ink(&p);
            let row = div()
                .flex()
                .flex_col()
                .pb(px(m::SPACING))
                .agent("Label", title.clone())
                .agent_value(detail.clone())
                .child(StringView::new(title).wrap(true).bold(true).color(ink.text))
                .child(para(detail, ink.text))
                .child(StringView::new(sub).wrap(true).small(true).color(ink.dim));
            volumes = volumes.child(row.children(bar.map(|(parts, total)| div().pt(px(m::TIGHT)).child(meter_el(parts, total, 6.0)))));
            continue;
        }
        volumes = volumes.child(
            canvas(move |g, r| {
                let p = theme::palette();
                let ink = style::card_ink(&p);
                let (plain, bold) = (style::ui_font(FontWeight::NORMAL), style::ui_font(FontWeight::BOLD));
                let d = style::label(g, r.right(), r.top() + px(9.0), style::Align::Right, detail.clone(), style::PLAIN_SIZE, plain.clone(), ink.text);
                let t = style::label_fit(g, r.left(), r.top() + px(9.0), style::Align::Left, title.clone(), style::PLAIN_SIZE, bold, ink.text, (d.left() - px(m::SPACING) - r.left()).max(px(0.0)));
                let s = style::label_fit(g, r.left(), r.top() + px(24.0), style::Align::Left, sub.clone(), style::SMALL_SIZE, plain, ink.dim, r.size.width * 0.5);
                agent::record(g, agent::Node { value: Some(detail.clone()), ..agent::Node::label(title.clone(), t) });
                agent::record(g, agent::Node::label(sub.clone(), s));
                if let Some((parts, total)) = &bar {
                    let b = olo_kit::Bounds::from_corners(olo_kit::point(r.left() + r.size.width * 0.5, r.top() + px(21.0)), olo_kit::point(r.right(), r.top() + px(27.0)));
                    meter(g, b, parts, *total);
                }
            })
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

fn network(app: &Activity, me: &Me, inner: f32, cx: &Cx<Activity>) -> Vec<Element> {
    let p = theme::palette();
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
        let mut cap = vec![if n.up { "Connected".to_owned() } else { olo_kit::words::tr("Not connected").to_owned() }];
        cap.extend(n.addresses.iter().filter(|a| !a.starts_with("fe80")).cloned());
        if !n.mac.is_empty() && n.mac != "00:00:00:00:00:00" {
            cap.push(n.mac.clone());
        }
        cap.push(format!("MTU {}", n.mtu));
        let figure_text = Unit::Rate.format(app.latest(&keys[0]).unwrap_or(0.0) + app.latest(&keys[1]).unwrap_or(0.0));
        out.push(
            named_card(format!("net-{}", n.name), n.name.clone(), "Network Interface", Some(figure_text), cap.join(" · "), inner)
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
    let mut card = Card::new("net-programs", "Programs Using the Network").caption_within(caption, inner);
    card = if rows.is_empty() { card.child(StringView::new("None just now").height(FACT_ROW).color(style::card_ink(&p).dim)) } else { card.child(process_rows(me, "Received and sent", rows, inner)) };
    out.push(card.into_any_element());
    // Connections: who talks to whom.
    let full = app.access == Access::Full;
    let caption = if full { "Every program's" } else { "Programs of other users show no name without full access" };
    let needle = olo_kit::search::Needle::new(app.connections_search.read(cx).text().trim());
    let mut rows: Vec<&crate::sample::Connection> =
        app.connections.iter().filter(|cn| needle.is_empty() || [cn.local.as_str(), &cn.remote, &cn.state, &cn.process, cn.proto.as_str()].iter().any(|f| needle.in_text(f))).collect();
    let (col, asc) = app.connections_list.state().sort.or(app.settings.connections_sort).unwrap_or((4, true));
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
    let (sorted, opened) = (*me, *me);
    let list = ColumnListView::new(cx, "activity-connections", &app.connections_list, columns, cells.len(), move |_: &Activity, cell, g| {
        if let Some(row) = list_cells.get(cell.row) {
            cell.text(g, row[cell.column].clone());
        }
    })
    .empty_text("No connections match.")
    .on_sort(move |_, cx| {
        sorted.update(cx, move |a, cx| {
            a.settings.connections_sort = a.connections_list.state().sort;
            a.store();
            cx.invalidate();
        })
    })
    .on_open(move |row, cx| {
        if let Some(Some(pid)) = pids.get(*row) {
            let pid = *pid;
            opened.update(cx, move |a, cx| a.inspect(pid, cx));
        }
    });
    out.push(
        Card::new("connections", "Connections")
            .caption_within(caption, inner)
            .child(div().w(px(320.0f32.min(inner))).child(TextControl::new(cx, &app.connections_search).label("Search:")))
            .child(div().h(px(m::SPACING)))
            .child(div().h(px(CONNECTIONS_HEIGHT)).flex().flex_col().child(div().flex_1().min_h_0().flex().flex_col().child(list)))
            .into_any_element(),
    );
    out
}

// ---- GPU -------------------------------------------------------------

fn gpu(app: &Activity, me: &Me, area: f32, inner: f32, cx: &Cx<Activity>) -> Vec<Element> {
    let p = theme::palette();
    let sys = &app.system;
    let mut out = Vec::new();
    let clients = sys.gpu_clients.clone().unwrap_or_default();
    // Each GPU in turn, the one the computer started on first: how busy,
    // what it draws and holds, and who uses it.
    for (i, g) in sys.gpus.iter().enumerate() {
        let k = |what: &str| format!("{}-{what}", g.prefix);
        let (busy, media, jobs, power, memory, freq, temp) = (k("busy"), k("media"), k("jobs"), k("power"), k("memory"), k("freq"), k("temp"));
        let (render, encode, decode) = (k("render"), k("encode"), k("decode"));
        // Its engines apart where it has several (the figure is the
        // busiest of them): Intel's render and media, NVIDIA's graphics,
        // encoder and decoder.
        let mut keys: Vec<&str> = vec![&busy];
        let mut names = vec!["Busy"];
        if app.source(&media).is_some() && app.source(&render).is_some() {
            keys = vec![&render, &media];
            names = vec!["Render", "Media"];
        } else if app.source(&encode).is_some() {
            keys = vec![&render, &encode, &decode];
            names = vec!["Graphics", "Encoder", "Decoder"];
        }
        let mut items = legend_items(app, &p, &keys, &names);
        items.extend(legend_items(app, &p, &[&jobs], &["Jobs"]));
        let state = if g.asleep { " · asleep: nothing is using it" } else { "" };
        out.push(
            named_card(format!("gpu-{i}"), g.name.clone(), "GPU", Some(value(app, &busy)), format!("{} driver{state}", g.driver), inner)
                .child(graph(app, if i == 0 { Some(me) } else { None }, &keys, Some(100.0), GRAPH + 40.0, cx))
                .child(div().h(px(m::TIGHT)))
                .child(legend_el(items, inner, cx))
                .into_any_element(),
        );
        let mut pair = Vec::new();
        if app.source(&power).is_some() {
            pair.push(
                Card::new(SharedString::from(power.clone()), "Power")
                    .figure(value(app, &power))
                    .caption("What it draws")
                    .child(graph(app, None, &[&power], None, GRAPH * 0.8 + 16.0, cx))
                    .into_any_element(),
            );
        }
        if app.source(&memory).is_some() {
            let caption = if g.memory_limit > 0 { olo_kit::tr!("of {} it may use", format_size(g.memory_limit)) } else { String::new() };
            let mut card = Card::new(SharedString::from(memory.clone()), "Memory").figure(value(app, &memory)).caption(caption);
            if g.memory_limit > 0 {
                card = card.child(meter_el(vec![(c(p.hue(olo_theme::Hue::Red)), g.memory_used as f64)], g.memory_limit as f64, 8.0)).child(div().h(px(m::SPACING)));
            }
            pair.push(card.child(graph(app, None, &[&memory], None, GRAPH * 0.8, cx)).into_any_element());
        }
        if app.source(&freq).is_some() {
            pair.push(
                Card::new(SharedString::from(freq.clone()), "Clock")
                    .figure(value(app, &freq))
                    .caption("How fast it runs now")
                    .child(graph(app, None, &[&freq], None, GRAPH * 0.8 + 16.0, cx))
                    .into_any_element(),
            );
        }
        if app.source(&temp).is_some() {
            pair.push(
                Card::new(SharedString::from(temp.clone()), "Temperature")
                    .figure(value(app, &temp))
                    .caption("Its own sensor")
                    .child(graph(app, None, &[&temp], None, GRAPH * 0.8 + 16.0, cx))
                    .into_any_element(),
            );
        }
        if !pair.is_empty() {
            out.push(flow(pair, area, 300.0, 2));
        }
        // Who uses it.
        let rows: Vec<Busy> = clients
            .iter()
            .filter(|cl| cl.gpu == i)
            .map(|cl| {
                let busy = cl.busy.map(|b| format!(" · {b:.0}%")).unwrap_or_default();
                (cl.pid, format!("{} ({})", cl.name, cl.pid), format!("{}{busy}", format_size(cl.memory)))
            })
            .collect();
        let caption = match (g.asleep, app.access == Access::Full) {
            (true, _) => "Nothing: it's asleep",
            (false, true) => "Its memory each holds",
            (false, false) => "Its memory each holds (other users' need full access)",
        };
        out.push(
            named_card(format!("gpu-programs-{i}"), olo_kit::tr!("Programs Using the {}", g.name), "Programs", None, caption, inner)
                .child(process_rows(me, "Memory on the GPU", rows, inner))
                .into_any_element(),
        );
    }
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
fn history_graph(app: &Activity, id: &str, lines: &[(&str, &str, olo_theme::Hue)], unit: fn(f64) -> String, max: Option<f64>, inner: f32, cx: &Cx<Activity>) -> Vec<Element> {
    let p = theme::palette();
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
    let mut g = Graph::live(SharedString::from(format!("activity-{id}")), series, app.time_clock())
        .span(app.settings.span as f64)
        .grid(grid_for(app.settings.span))
        .unit(unit)
        .smooth(app.settings.smooth)
        .plain();
    if let Some(m) = max {
        g = g.max(m);
    }
    vec![div().h(px(GRAPH)).flex_shrink_0().child(g).into_any_element(), legend_el(items, inner, cx)]
}

fn ms_value(v: f64) -> String {
    if v < 1.0 { format!("{:.0} µs", v * 1000.0) } else { format!("{v:.1} ms") }
}

fn whole(v: f64) -> String {
    style::thousands(v.max(0.0).round() as usize)
}

fn displays(app: &Activity, me: &Me, inner: f32, cx: &Cx<Activity>) -> Vec<Element> {
    let p = theme::palette();
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
            ("Refresh".into(), olo_kit::tr!("{} Hz, {} ms a frame", format!("{:.0}", hz), format!("{:.1}", frame_ms))),
            ("Now".into(), if d.resting { olo_kit::words::tr("Resting: nothing new on it").into() } else { olo_kit::tr!("{} frames a second", format!("{:.0}", d.fps)) }),
            ("Dropped".into(), olo_kit::tr!("{} in the last minute, {} of {} in all", d.late_minute, style::thousands(d.late as usize), style::thousands(d.frames as usize))),
            ("Drawing".into(), olo_kit::tr!("{} typically, {} at the 95th percentile, {} to {}", ms(d.render_p50_us), ms(d.render_p95_us), ms(d.render_min_us), ms(d.render_max_us))),
            ("Pacing".into(), olo_kit::tr!("{} of each frame kept for drawing: a frame starts {} after the vblank", ms(d.budget_us), ms(d.delay_us))),
            ("Pointer".into(), if d.cursor_plane { olo_kit::words::tr("On a plane of its own: moving it draws nothing").into() } else { olo_kit::words::tr("Drawn into each frame").into() }),
            ("Scan-out".into(), if d.scanout.is_empty() { "—".into() } else { d.scanout.clone() }),
        ];
        let figure = if d.resting { "resting".to_owned() } else { format!("{:.0} fps", d.fps) };
        let caption = match d.late_minute {
            0 => olo_kit::words::tr("Every frame on time this last minute").to_owned(),
            1 => olo_kit::words::tr("A frame dropped in the last minute").to_owned(),
            n => olo_kit::tr!("{} frames dropped in the last minute", n),
        };
        let mut card = named_card(format!("display-{}", d.output), d.output.clone(), "Display", Some(figure), caption, inner);
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
        out.push(card.child(facts(rows, inner)).children(graph).children(drawing).into_any_element());
    }
    let toggle = *me;
    let capture = "Capture debug information whenever a frame drops";
    let label_w = f32::from(style::text_width(olo_kit::words::of(capture), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL))).ceil();
    let label = if m::MARK_SIZE + m::MARK_GAP + label_w <= inner { capture } else { "Capture debug information" };
    out.push(
        Card::new("displays-capture", "When a Frame Drops")
            .caption("For finding out why")
            .child(
                CheckBox::new(label, app.frames_capture).id(capture)
                    .on_toggle(move |on, cx| {
                        let on = *on;
                        toggle.update(cx, move |a, cx| a.set_frames_capture(on, cx))
                    }),
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
fn ai(app: &Activity, inner: f32, cx: &Cx<Activity>) -> Vec<Element> {
    let p = theme::palette();
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
            .caption_within(caption, inner)
            .child(para(about, ink.dim))
            .child(div().h(px(m::SPACING)))
            .child(buttons(vec![terminal_preferences()]));
        return vec![card.into_any_element()];
    };
    let share = |n: u64| if s.offered > 0 { format!(" ({})", percent(n as f64 / s.offered as f64)) } else { String::new() };
    let rows: Vec<(String, String)> = vec![
        ("Offered".into(), olo_kit::tr!("{} suggestions since it started", style::thousands(s.offered as usize))),
        ("Taken".into(), olo_kit::tr!("{} whole{}, and {} a word at a time", style::thousands(s.accepted as usize), share(s.accepted), style::thousands(s.accepted_words as usize))),
        ("Knows".into(), olo_kit::tr!("{} different commands, of {} run", style::thousands(s.known), style::thousands(s.seen as usize))),
        (olo_kit::words::tr("Computes with").into(), format!("The {}", s.backend)),
    ];
    let figure = if s.offered > 0 { format!("{} taken", percent(s.accepted as f64 / s.offered as f64)) } else { olo_kit::words::tr("nothing yet").into() };
    let mut out = vec![
        Card::new("ai-commands", "Command Suggestions")
            .figure(figure)
            .caption_within("brush’s grey hints, from a tiny model of your own commands", inner)
            .child(facts(rows, inner))
            .child(div().h(px(m::SPACING)))
            .child(buttons(vec![terminal_preferences()]))
            .into_any_element(),
    ];
    // How quickly: olo-complete's own time for each (brush waits 8 ms at
    // most, then shows its own hint).
    let none = s.latency_count == 0;
    let rows: Vec<(String, String)> = if none {
        vec![("Suggestions".into(), olo_kit::words::tr("None asked for yet: type in brush").into())]
    } else {
        vec![
            ("Typically".into(), us_text(s.latency_p50_us)),
            (olo_kit::words::tr("95th percentile").into(), us_text(s.latency_p95_us)),
            ("Slowest".into(), olo_kit::tr!("{} (of the last {})", us_text(s.latency_worst_us), style::thousands(s.latency_count))),
        ]
    };
    let slow = s.latency_p95_us > 8_000;
    let mut card = Card::new("ai-latency", "Latency")
        .figure(if none { "—".into() } else { us_text(s.latency_p50_us) })
        .caption_within(if slow { "Slower than brush waits: some hints are brush’s own" } else { "Each suggestion, from the question to the answer sent" }, inner);
    if slow {
        card = card.figure_ink(c(p.failure));
    }
    let graph = history_graph(app, "ai-latency", &[("ai.p50", "Typically", olo_theme::Hue::Green), ("ai.p95", "95th percentile", olo_theme::Hue::Orange)], ms_value, None, inner, cx);
    out.push(card.child(facts(rows, inner)).children(graph).into_any_element());
    // How it learns.
    let mut rows: Vec<(String, String)> = vec![(
        "Weights".into(),
        match (s.tuned, s.fine_tunes) {
            (false, _) => olo_kit::words::tr("The hand-written ones: not fine-tuned yet").into(),
            (true, 0) => olo_kit::words::tr("Yours, as last learned").into(),
            (true, 1) => olo_kit::words::tr("Yours: fine-tuned once since it started").into(),
            (true, n) => olo_kit::tr!("Yours: fine-tuned {} times since it started", style::thousands(n as usize)),
        },
    )];
    let mut figure = "—".to_owned();
    if let Some(t) = &s.last_tune {
        let when = jiff::Timestamp::from_second(s.last_tune_at).ok().map(|w| w.to_zoned(jiff::tz::TimeZone::system()).strftime("%-d %B at %H:%M").to_string()).unwrap_or_default();
        let ms = t.micros as f64 / 1000.0;
        rows.push(("Last".into(), olo_kit::tr!("{}: {} examples, {} steps, {} ms on the {}", when, style::thousands(t.examples), style::thousands(t.steps), format!("{:.1}", ms), t.backend)));
        // (What's in use: the new weights if kept, else those before.)
        let in_use = if t.kept { t.after.top1 } else { t.before.top1 } as f64;
        rows.push((olo_kit::words::tr("Right first").into(), olo_kit::tr!("{} of your newest commands, kept back to test it ({} hand-written)", percent(in_use), percent(t.baseline.top1 as f64))));
        rows.push((
            "Kept".into(),
            if t.kept { format!("Yes ({} before)", percent(t.before.top1 as f64)) } else { olo_kit::tr!("No: it did no better ({}), so the weights before stay", percent(t.after.top1 as f64)) },
        ));
        figure = olo_kit::tr!("{} right first", percent(in_use));
    }
    rows.push((
        "Waiting".into(),
        if s.pending > 0 { olo_kit::tr!("{} new examples, learned within a second", style::thousands(s.pending)) } else { olo_kit::words::tr("Nothing new to learn").into() },
    ));
    out.push(Card::new("ai-learning", "Learning").figure(figure).caption_within("Fine-tuned on your newest commands a minute after you type them", inner).child(facts(rows, inner)).into_any_element());
    out
}

// ---- Inputs -----------------------------------------------------------

/// The pointer's and the touchpad's reports as app_server gets them, and
/// what's done with each kind of input, over time.
fn inputs(app: &Activity, inner: f32, cx: &Cx<Activity>) -> Vec<Element> {
    let p = theme::palette();
    let ink = style::card_ink(&p);
    let mut out = Vec::new();
    // The pointer's reports, as app_server gets them: the mouse and its
    // link, which no frame can make up for.
    {
        let h = &app.pointer_health;
        let rate = if h.interval_us > 0 { format!("{} Hz", (1_000_000 / h.interval_us.max(1)).min(8000)) } else { "—".to_owned() };
        let ms = |us: u64| format!("{:.0} ms", us as f64 / 1000.0);
        let rows: Vec<(String, String)> = vec![
            ("Reports".into(), olo_kit::tr!("{} in the last second, at {}", h.reports_second, rate)),
            (
                "Gaps".into(),
                if h.gaps_minute == 0 {
                    olo_kit::words::tr("None in the last minute").into()
                } else {
                    olo_kit::tr!("{} over 5 ms in the last minute, the longest {}", h.gaps_minute, ms(h.longest_gap_us))
                },
            ),
        ];
        let figure = if h.reports_second > 0 { rate.clone() } else { "still".to_owned() };
        let caption = if h.gaps_minute > 0 {
            "Reports going missing while the mouse moves: a receiver asleep (USB autosuspend; Olo's udev rule keeps it awake) or a 2.4 GHz link dropping packets (move the receiver nearer)"
        } else {
            "The mouse's reports arrive as they should"
        };
        let mut card = Card::new("inputs-pointer", "Pointer").figure(figure).caption_within(caption, inner);
        if h.gaps_minute > 0 {
            card = card.figure_ink(c(p.hue(olo_theme::Hue::Red)));
        }
        let graph = history_graph(app, "pointer", &[("pointer.hz", "Reports a second", olo_theme::Hue::Blue)], whole, None, inner, cx);
        out.push(card.child(facts(rows, inner)).children(graph).into_any_element());
    }
    // The touchpad's reports, through Olo's touch filter and the palm
    // model: judged as each frame comes, on app_server's main thread,
    // so as quick as that thread is free (it draws frames too).
    let t = &app.touch_health;
    if t.filter || t.reports_minute > 0 {
        let us = |us: u64| if us < 1000 { format!("{us} µs") } else { format!("{:.1} ms", us as f64 / 1000.0) };
        let palms = if t.model && t.trained == 0 {
            olo_kit::tr!("Olo's own palm model, made from {} touches of real hands (Preferences ▸ Touchpad & Mouse ▸ Touch trains one of yours)", t.contacts)
        } else if t.model {
            let when = jiff::Timestamp::from_second(t.trained as i64).ok().map(|w| w.to_zoned(jiff::tz::TimeZone::system()).strftime("%-d %B %Y").to_string()).unwrap_or_default();
            olo_kit::tr!("Your palm model, trained {} on {} touches", when, t.contacts)
        } else if t.filter {
            olo_kit::words::tr("By their size alone: the palm model is off (Preferences ▸ Touchpad & Mouse ▸ Touch)").into()
        } else {
            olo_kit::words::tr("libinput's own: Olo's touch filter is off").into()
        };
        let core = match t.util_min {
            1024.. => olo_kit::words::tr("on a fast core at a high clock").to_owned(),
            1.. => olo_kit::tr!("taken as at least {}% busy", t.util_min * 100 / 1024),
            0 => olo_kit::words::tr("where the scheduler puts it").to_owned(),
        };
        let mut rows: Vec<(String, String)> = vec![
            ("Reports".into(), if t.reports_second > 0 { olo_kit::tr!("{} in the last second", t.reports_second) } else { olo_kit::words::tr("None in the last second").into() }),
            (
                "Latency".into(),
                if t.reports_minute > 0 {
                    olo_kit::tr!(
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
            rows.push((
                "Tested".into(),
                olo_kit::tr!("{}% of palms caught, {}% of fingers taken for palms, on touches kept back", format!("{:.0}", t.palms_caught * 100.0), format!("{:.1}", t.fingers_lost * 100.0)),
            ));
        }
        // What the model makes of the touches now (judged here as the
        // filter judges them, while this section shows).
        if let Some(s) = &app.palm_stats {
            let judged = if s.touches == 0 {
                olo_kit::words::tr("No touches in the last minute").to_owned()
            } else {
                let size = if s.by_size > 0 { format!(" {}", olo_kit::tr!("({} by their size)", s.by_size)) } else { String::new() };
                olo_kit::tr!("{} touches in the last minute, {} taken for palms{}", s.touches, s.palms, size)
            };
            rows.push(("Judged".into(), judged));
            if s.model && s.us_p50 > 0 {
                rows.push((olo_kit::words::tr("The model").into(), olo_kit::tr!("{} µs a report typically, the longest {} µs", s.us_p50, s.us_max)));
            }
        }
        rows.push((olo_kit::words::tr("Runs on").into(), olo_kit::tr!("app_server's main thread: nice {}{}, {}", t.nice, if t.realtime { ", realtime" } else { "" }, core)));
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
        let mut card = Card::new("inputs-touch", "Touchpad").figure(figure).caption_within(caption, inner);
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
        out.push(card.child(facts(rows, inner)).children(graph).into_any_element());
    }
    // What's done with each kind of input, a minute back (how many,
    // never which key).
    {
        let a = &app.inputs;
        let rows: Vec<(String, String)> = vec![
            ("Clicks".into(), olo_kit::tr!("{} in the last minute", style::thousands(a.clicks_minute as usize))),
            ("Scrolling".into(), olo_kit::tr!("{} steps in the last minute (a wheel's notches, a touchpad's moves)", style::thousands(a.scrolls_minute as usize))),
            ("Gestures".into(), olo_kit::tr!("{} begun in the last minute (swipes, pinches, holds)", style::thousands(a.gestures_minute as usize))),
            ("Keys".into(), olo_kit::tr!("{} pressed in the last minute", style::thousands(a.keys_minute as usize))),
        ];
        let busy = a.clicks_minute + a.scrolls_minute + a.gestures_minute + a.keys_minute;
        let card = Card::new("inputs-activity", "What You Did")
            .figure(if busy > 0 { olo_kit::tr!("{} a minute", style::thousands(busy as usize)) } else { "nothing".into() })
            .caption_within("Each kind of input in the last minute: how many, never which keys", inner);
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
        out.push(card.child(facts(rows, inner)).children(graph).into_any_element());
    }
    let _ = ink;
    out
}

// ---- Sensors ---------------------------------------------------------

fn sensors(app: &Activity, inner: f32, cx: &Cx<Activity>) -> Vec<Element> {
    let p = theme::palette();
    let sys = &app.system;
    let mut out = Vec::new();
    if let Some(b) = &sys.battery {
        let mut rows: Vec<(String, String)> = vec![("Status".into(), b.status.clone())];
        if !b.health.is_empty() {
            rows.push(("Health".into(), b.health.clone()));
        }
        if b.design > 0.0 {
            rows.push(("Holds".into(), olo_kit::tr!("{} Wh of {} Wh new ({}%)", format!("{:.1}", b.full), format!("{:.1}", b.design), format!("{:.0}", b.full / b.design * 100.0))));
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
                .caption_within(b.model.clone(), inner)
                .child(meter_el(vec![(c(p.hue(hue)), b.capacity)], 100.0, 10.0))
                .child(div().h(px(m::SPACING)))
                .child(facts_two(rows, inner))
                .child(div().h(px(m::SPACING)))
                .child(graph(app, None, &["battery"], Some(100.0), GRAPH, cx))
                .into_any_element(),
        );
    }
    // The fans against what they cool, in place of a graph each.
    let fans = fans_card(app, inner, cx);
    let has_fans = fans.is_some();
    out.extend(fans);
    for (unit, title, caption) in [(Unit::Celsius, "Temperatures", "Every thermal sensor"), (Unit::Watts, "Power", "What each part draws"), (Unit::Rpm, "Fans", "")] {
        if unit == Unit::Rpm && has_fans {
            continue;
        }
        let keys: Vec<(String, String)> = app.sources.iter().filter(|s| s.group == Group::Sensors && s.unit == unit).map(|s| (s.key.clone(), s.name.clone())).collect();
        if keys.is_empty() {
            continue;
        }
        let cells: Vec<Element> = keys
            .iter()
            .map(|(key, name)| {
                // The card says what kind; each says which.
                let short = ["temperature", "Temperature", "Temp", "temp", "power", "Power", "fan"].iter().fold(name.clone(), |n, w| n.replace(&format!(" {w}"), ""));
                mini(app, key, short, None, cx)
            })
            .collect();
        out.push(Card::new(format!("sensors-{title}"), title).caption_within(caption, inner).child(mini_grid(cells, inner)).into_any_element());
    }
    out
}

/// "19,497": a count, its thousands apart.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// "1 time", "3 times".
fn times(n: u64) -> String {
    if n == 1 { "1 time".into() } else { format!("{} times", grouped(n)) }
}

/// What the fan profile and the service's level say, for people: the
/// card's caption, and the Profile and Level facts.
fn fan_words(cool: &crate::sample::Cooling) -> (String, Option<&'static str>, Option<String>) {
    use olo_system::fans::Profile;
    // (One of Olo's own curves, by its name: Cool, Cooler, Coolest.)
    let ours = Profile::parse(&cool.profile).filter(|p| p.ours());
    let fact = |p: Profile| match p {
        Profile::Cool => "Cool: a little ahead of the heat",
        Profile::Cooler => "Cooler: well ahead of the heat, and louder",
        _ => "Coolest: at the first sign of work, the loudest",
    };
    match (ours, cool.profile.as_str()) {
        // Chosen and not what the fans are doing: said, with why.
        (Some(p), _) if !cool.amiss.is_empty() => (olo_kit::tr!("{} profile · not in force: {}", p.label(), cool.amiss), Some(fact(p)), Some(olo_kit::tr!("Not in force: {}", cool.amiss))),
        (Some(p), _) => {
            let (short, level) = match cool.level.as_str() {
                "" | "auto" => (olo_kit::words::tr("left to the firmware for now").to_owned(), olo_kit::words::tr("Left to the firmware for now").to_owned()),
                "full-speed" => (olo_kit::words::tr("full speed").to_owned(), olo_kit::words::tr("Full speed").to_owned()),
                n => (olo_kit::tr!("level {} of 7", n), format!("{n} of 7")),
            };
            (format!("{} profile · {short}", p.label()), Some(fact(p)), Some(level))
        }
        (None, "") => (String::new(), None, None),
        (None, _) => (olo_kit::words::tr("Auto profile · the firmware's own curve").to_owned(), Some("Auto: the firmware's own curve"), None),
    }
}

/// The fans against what they cool: each fan's speed, the processor's
/// temperature and how often it was throttled for heat, over time (each
/// to its own scale, the fans to one); the fan profile in use, the
/// level Olo's fan service (`olo-fans`) holds and what asked for it;
/// and the throttling counted, so a curve can be seen to work. None
/// where there are no fans to read.
fn fans_card(app: &Activity, inner: f32, cx: &Cx<Activity>) -> Option<Element> {
    let p = theme::palette();
    let fans: Vec<usize> = app.sources.iter().enumerate().filter(|(_, s)| s.group == Group::Sensors && s.unit == Unit::Rpm).map(|(i, _)| i).collect();
    if fans.is_empty() {
        return None;
    }
    // (One that has never turned isn't a fan to watch, beside ones
    // that do: ACPI's `fan1` on a ThinkPad, whose own two say.)
    let turned = |i: &usize| app.history[*i].iter().any(|(_, v)| *v > 0.0);
    let fans: Vec<usize> = if fans.iter().any(turned) { fans.into_iter().filter(turned).collect() } else { fans };
    let cool = &app.system.cooling;
    let (span, now) = (app.settings.span as f64, app.now());
    let seen = |h: &std::collections::VecDeque<(f64, f64)>| h.iter().filter(|(t, _)| *t >= now - span).map(|(_, v)| *v).filter(|v| v.is_finite()).fold(0.0, f64::max);
    let latest = |h: &std::collections::VecDeque<(f64, f64)>| h.back().map(|(_, v)| *v).filter(|v| v.is_finite());
    let (mut series, mut items) = (Vec::new(), Vec::new());
    // The fans to one scale, so one is seen against another.
    let top = nice_ceiling(fans.iter().map(|&i| seen(&app.history[i])).fold(1000.0, f64::max));
    let hues = [olo_theme::Hue::Blue, olo_theme::Hue::Teal, olo_theme::Hue::Purple, olo_theme::Hue::Green];
    let mut fastest: f64 = 0.0;
    for (k, &i) in fans.iter().enumerate() {
        let (name, color) = (SharedString::from(app.sources[i].name.clone()), c(p.hue(hues[k % hues.len()])));
        let v = latest(&app.history[i]);
        fastest = fastest.max(v.unwrap_or(0.0));
        series.push(Series::new(name.clone(), color, app.history[i].clone()).top(top).format(|v| Unit::Rpm.format(v)).fill(k == 0));
        items.push(LegendItem { color, name, value: v.map_or_else(|| "–".into(), |v| Unit::Rpm.format(v)).into() });
    }
    // The processor's package where it's read (what throttles), else
    // the hottest sensor now.
    let package = app.sources.iter().position(|s| s.group == Group::Sensors && s.unit == Unit::Celsius && s.name.contains("Package"));
    let hottest_now = || {
        app.sources
            .iter()
            .enumerate()
            .filter(|(_, s)| s.group == Group::Sensors && s.unit == Unit::Celsius)
            .filter_map(|(i, _)| Some((i, latest(&app.history[i])?)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };
    if let Some(i) = package.or_else(hottest_now) {
        let (name, color) = (SharedString::from(app.sources[i].name.clone()), c(p.hue(olo_theme::Hue::Red)));
        series.push(Series::new(name.clone(), color, app.history[i].clone()).top(100.0).format(|v| Unit::Celsius.format(v)));
        items.push(LegendItem { color, name, value: latest(&app.history[i]).map_or_else(|| "–".into(), |v| Unit::Celsius.format(v)).into() });
    }
    if let Some(h) = app.frame_history.get("cooling.throttled") {
        let color = c(p.hue(olo_theme::Hue::Orange));
        series.push(Series::new("Throttled", color, h.clone()).top(nice_ceiling(seen(h)).max(1.0)).format(|v| Unit::PerSecond.format(v)));
        items.push(LegendItem { color, name: "Throttled".into(), value: latest(h).map_or_else(|| "–".into(), |v| Unit::PerSecond.format(v)).into() });
    }
    let g = Graph::live("activity-fans", series, app.graph_clock()).span(span).grid(grid_for(app.settings.span)).smooth(app.settings.smooth).plain();

    let (caption, profile, level) = fan_words(cool);
    let mut rows: Vec<(String, String)> = Vec::new();
    if let Some(profile) = profile {
        rows.push(("Profile".into(), profile.into()));
    }
    if let Some(level) = level {
        rows.push(("Level".into(), level));
    }
    if olo_system::fans::Profile::parse(&cool.profile).is_some_and(|p| p.ours()) && !cool.why.is_empty() {
        rows.push(("Because".into(), cool.why.clone()));
    }
    if let Some(n) = cool.throttled {
        let long = cool.throttled_ms.filter(|ms| *ms >= 1000).map(|ms| format!(", {}", olo_kit::tr!("{} in all", duration(ms as f64 / 1000.0)))).unwrap_or_default();
        rows.push(("Throttled".into(), if n == 0 { olo_kit::words::tr("Never since the computer started").into() } else { olo_kit::tr!("{} since the computer started{}", times(n), long) }));
        if let Some((minute, open)) = app.throttled_lately() {
            let lately = match (minute, open) {
                (_, 0) => olo_kit::words::tr("Not since this window opened").to_owned(),
                (0, open) => olo_kit::tr!("Not in the last minute; {} since this window opened", times(open)),
                (minute, open) => olo_kit::tr!("{} in the last minute, {} since this window opened", times(minute), times(open)),
            };
            rows.push(("Lately".into(), lately));
        }
    }
    let mut card = Card::new("fans", "Fans").figure(Unit::Rpm.format(fastest));
    if !caption.is_empty() {
        card = card.caption_within(caption, inner);
    }
    card = card.child(div().h(px(GRAPH)).flex_shrink_0().child(g)).child(legend_el(items, inner, cx));
    if !rows.is_empty() {
        card = card.child(div().h(px(m::SPACING))).child(facts(rows, inner));
    }
    Some(card.into_any_element())
}

// ---- System ----------------------------------------------------------

fn system(app: &Activity, me: &Me, inner: f32, cx: &Cx<Activity>) -> Vec<Element> {
    let p = theme::palette();
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
        (olo_kit::words::tr("Up for").into(), duration(sys.uptime)),
        ("Processor".into(), if sys.cpu_model.is_empty() { format!("{} cores", sys.cores) } else { sys.cpu_model.clone() }),
        ("Memory".into(), format_size(mem)),
        (olo_kit::words::tr("Open files").into(), if sys.open_files.1 >= 1 << 62 { olo_kit::tr!("{}, no limit", sys.open_files.0) } else { format!("{} of {}", sys.open_files.0, sys.open_files.1) }),
        ("Access".into(), access.to_owned()),
    ];
    let mut card = Card::new("this-computer", "This Computer").child(facts_two(rows, inner));
    if app.access == Access::Mine && crate::helper::way() != crate::helper::Way::Off {
        let me = *me;
        card = card.child(div().h(px(m::SPACING))).child(div().flex().child(Button::new("Get Full Access…").min_width(140.0).on_click(move |_, cx| me.update(cx, move |a, cx| a.get_full_access(cx)))));
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
            .caption_within("Time tasks waited for each, over 10 s, 1 min and 5 min", inner)
            .child(facts(rows, inner))
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
    let (opened, menued) = (*me, *me);
    let list = ColumnListView::new(cx, "activity-tunables", &app.tunables_list, columns, cells.len(), move |_: &Activity, cell, g| {
        if let Some(row) = list_cells.get(cell.row) {
            cell.text(g, row[cell.column].clone());
        }
    })
    .on_open(move |row, cx| {
        if let Some(cl) = open_cells.get(*row) {
            let name = cl[0].clone();
            opened.update(cx, move |a, cx| a.tune(&name, cx));
        }
    })
    .on_context(move |&(row, at), cx| {
        let Some(cl) = menu_cells.get(row) else { return };
        menu::context_menu(cx, Menu::new("").item("Change…", format!("tune:{}", cl[0])), at, crate::runs(menued));
    });
    out.push(
        Card::new("tuning", "Tuning").caption_within(caption, inner).child(div().h(px(list_h)).flex().flex_col().child(div().flex_1().min_h_0().flex().flex_col().child(list))).into_any_element(),
    );
    out
}

// ---- Graphs ----------------------------------------------------------

/// Haiku's ActivityMonitor: graphs of whatever you choose (right-click a
/// graph to choose; add and remove graphs), sharing the window's height
/// (each card's graph as tall as the others'). In a short tile, or with
/// many cards, each keeps a legible plot and the cards scroll.
fn graphs(app: &Activity, width: f32, cx: &mut Cx<Activity>) -> Element {
    let p = theme::palette();
    let span = app.settings.span as f64;
    let now = app.now();
    let bar = if app.scroll.max_offset().height > px(0.5) { SCROLL_BAR } else { 0.0 };
    let inner = width - bar - 2.0 * (GAP + m::CARD_INSET);
    let mut least = 2.0 * GAP + GAP * app.settings.views.len().saturating_sub(1) as f32;
    let mut col = div().flex().flex_col().gap(px(GAP)).p(px(GAP)).size_full();
    for (v, keys) in app.settings.views.iter().enumerate() {
        let items: Vec<LegendItem> = keys
            .iter()
            .filter_map(|k| {
                let i = app.source(k)?;
                Some(LegendItem { color: c(p.hue(app.sources[i].color)), name: app.sources[i].name.clone().into(), value: value(app, k).into() })
            })
            .collect();
        let lh = legend_height(inner, &items).max(FACT_ROW);
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
        let g = Graph::live(SharedString::from(format!("activity-view-{v}")), s, app.graph_clock()).span(span).grid(grid_for(app.settings.span)).smooth(app.settings.smooth).plain();
        let card = canvas(|g, b| style::card(g, b, &theme::palette(), false, false)).paint_key(0).absolute().inset_0();
        // (Untitled cards: what isn't graph is the same in each, so the
        // room left is shared evenly between the graphs.)
        let fixed = 2.0 * m::CARD_INSET + m::TIGHT + lh;
        let card_least = fixed + 48.0;
        least += card_least;
        col = col.child(
            div()
                .id(SharedString::from(format!("activity-view-card-{v}")))
                .relative()
                .flex()
                .flex_col()
                .flex_grow()
                .flex_basis(px(fixed))
                .min_h(px(card_least))
                .p(px(m::CARD_INSET))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |a: &mut Activity, e: &olo_kit::MouseDownEvent, cx| {
                        let (menu, to) = (view_menu(a, v), cx.view_ref());
                        menu::context_menu(cx, menu, e.position, crate::runs(to));
                    }),
                )
                .child(card)
                .child(div().flex_1().min_h_0().child(g))
                .child(div().h(px(m::TIGHT)).flex_shrink_0())
                .child(legend_row(items, lh)),
        );
    }
    ScrollView::new(&app.scroll).border(false).auto_hide(true).background(dashboard::gutter(&p)).child(col.min_h(px(least))).into_any_element()
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
fn buttons(row: Vec<Button>) -> Element {
    div().flex().flex_row().flex_wrap().gap(px(m::SPACING)).children(row).into_any_element()
}

/// The supervisor (Preferences ▸ Performance): whether it's on and what
/// it favours; each finding with how sure it is, why, and what can be
/// done; what it lowered; what it did lately.
fn supervisor(app: &Activity, me: &Me, inner: f32, _: &Cx<Activity>) -> Vec<Element> {
    let p = theme::palette();
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
                olo_kit::words::tr("Olo's own").into(),
                match st.raised.len() {
                    0 => olo_kit::words::tr("Each above everything else, as it started (rtkit)").to_owned(),
                    _ => st.raised.iter().map(|r| olo_kit::tr!("{} to nice {}", r.name, r.nice)).collect::<Vec<_>>().join(", "),
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
            (olo_kit::words::tr("Memory free").into(), format!("{:.0}%", st.memory_available)),
            (olo_kit::words::tr("Memory stalls").into(), format!("{:.0}%", st.memory_stall)),
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
        (true, 0, 0) => olo_kit::words::tr("Nothing found").to_owned(),
        (true, 0, n) => format!("{n} finding{}", if n == 1 { "" } else { "s" }),
        (true, u, n) => format!("{n} finding{}, {u} urgent", if n == 1 { "" } else { "s" }),
    };
    let mut card = Card::new("supervisor", "Supervisor").figure(figure_text).caption_within(caption, inner).child(para(about, ink.dim)).child(div().h(px(m::SPACING)));
    if !rows.is_empty() {
        card = card.child(facts_two(rows, inner)).child(div().h(px(m::SPACING)));
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
        let caption = olo_kit::tr!("{}pid {} · found {} ago", if f.urgent { "Urgent · " } else { "" }, f.pid, duration(ago));
        let alarm = c(if f.urgent { p.failure } else { p.warning });
        let mut card = named_card(format!("finding-{}-{}", f.pid, f.kind as u8), title, "Finding", Some(format!("{:.0}% sure", f.confidence * 100.0)), caption, inner)
            .figure_ink(alarm)
            .child(para(f.summary.clone(), ink.text))
            .child(div().h(px(m::TIGHT)));
        for e in &f.evidence {
            card = card.child(para(format!("•  {e}"), ink.dim));
        }
        let (pid, name) = (f.pid, f.name.clone());
        let (inspect, stop) = (*me, *me);
        card = card.child(div().h(px(m::SPACING))).child(buttons(vec![
            Button::new("Inspect").id(format!("finding-inspect-{pid}")).on_click(move |_, cx| inspect.update(cx, move |a, cx| a.inspect_at(pid, crate::inspector::SUPERVISOR_TAB, cx))),
            Button::new("Stop Process").id(format!("finding-stop-{pid}")).on_click(move |_, cx| stop.update(cx, move |a, cx| a.act(Action::Signal { pid, signal: libc::SIGTERM }, cx))),
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
            let unit = l.unit.as_ref().map(|u| format!(", {u} idle")).unwrap_or_default();
            card = card.child(para(format!("{} ({}): nice {} → {}, {} scheduling, {} I/O{unit}, as {}", l.name, l.pid, l.nice_before, l.nice, l.policy, l.io, l.why), ink.text));
        }
        let more = st.lowered.len().saturating_sub(SHOWN);
        if more > 0 {
            card = card.child(para(olo_kit::tr!("And {} more", more), ink.text));
        }
        out.push(card.into_any_element());
    }

    // What it did lately, newest first.
    if !st.log.is_empty() {
        const SHOWN: usize = 12;
        let rows: Vec<(String, String)> = st.log.iter().rev().take(SHOWN).map(|e| (clock(e.at as f64), e.text.clone())).collect();
        out.push(Card::new("lately", "Lately").child(facts(rows, inner)).into_any_element());
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

    #[test]
    fn cards_and_facts_take_the_row_in_a_vertical_third() {
        // (A vertical third of the smallest screen, 407 points of
        // content; beside Deskbar, 349; and the window's least.)
        for viewport in [407.0, 349.0, crate::MIN_WIDTH] {
            let (_, section) = crate::section_layout(viewport);
            let card = section - SCROLL_BAR - 2.0 * GAP;
            let inner = card - 2.0 * m::CARD_INSET;
            assert_eq!(columns(card, 6, 250.0, 3), [card]); // Overview
            assert_eq!(columns(card, 2, 300.0, 2), [card]); // Memory and GPU
            assert_eq!(columns(inner, 2, FACTS_LEAST, 2), [inner]);
            // (Two minis across where they fit, else one.)
            let minis = columns(inner, 8, MINI_MIN_WIDTH, 8);
            assert_eq!(minis.len(), if viewport >= 407.0 { 2 } else { 1 }, "{viewport}");
            assert!(minis.iter().all(|w| *w >= MINI_MIN_WIDTH));
            assert_eq!(minis.iter().sum::<f32>() + GAP * (minis.len() - 1) as f32, inner);
        }
    }
}
