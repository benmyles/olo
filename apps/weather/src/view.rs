//! The window: the places kept down the left (Tracker's sources), the
//! weather now across the top (the sky's picture, the temperature, what
//! it's doing; cards for the clouds, visibility, wind, precipitation, the
//! air and the sun; the next hour minute by minute), then the rest of
//! today, the next 3 days or the next 7 (the kit's `Timeline`: every
//! layer by the hour in panels on one time axis, the nights shaded, what
//! the assistant pointed to marked; and for 7 days, a row a day). The
//! assistant on the right: questions and answers, and a field to ask.

use std::rc::Rc;

use olo_kit::{
    Bounds, ClickEvent, Cx, Element, FontWeight, Hsla, InteractiveElement as _, IntoElement as _, KeyDownEvent, MenuBar, ParentElement as _, Pixels, Styled as _, View as _, canvas, div, icons,
    metrics as m, point, px, size, style, theme,
    widgets::{
        Button, EmptyState, LinkButton, ScrollView, StringView, TabView, TextControl,
        dashboard::Card,
        sidebar::{Row, Sidebar},
        timeline::{Kind as PanelKind, Panel, Scale, Span, Timeline, Track},
    },
};
use serde_json::{Value, json};

use crate::{
    Weather,
    model::{self, Day, Forecast, Kind, Layer, Quantity, Stat, Step},
    places::Place,
    provider::{self, Error},
    units::Units,
};

/// The places' column, and the assistant's (narrower, down to its
/// least, before the forecast between them is).
const SIDEBAR: f32 = 184.0;
const ASSISTANT: f32 = 320.0;
const ASSISTANT_MIN: f32 = 260.0;
/// The least the forecast needs: its cards three across with their
/// titles and figures whole.
const MAIN_MIN: f32 = 560.0;
/// The window's least size: every column at its least, and the chart
/// still a few panels tall.
pub const MIN_WIDTH: f32 = 2.0 * m::WINDOW_INSET + SIDEBAR + MAIN_MIN + ASSISTANT_MIN + 2.0 * m::SPACING;
pub const MIN_HEIGHT: f32 = 660.0;
/// The header's height.
const HEADER: f32 = 116.0;

pub(crate) fn edge(cx: &olo_kit::WindowCx) -> f32 {
    let s = cx.scale_factor();
    style::line_pixels(s) * 2.0 / s
}

/// A sunken well of `background` with `content` inside its lines.
pub(crate) fn well(e: f32, background: olo_theme::Rgba, content: impl olo_kit::IntoElement) -> Element {
    let ground = canvas(move |g, b| {
        let p = theme::palette();
        let b = style::snap(g, b);
        style::fill_rect(g, b, style::c(background));
        style::sunken(g, b, &p, false);
    })
    .paint_key(style::key(background))
    .absolute()
    .inset_0();
    div().relative().size_full().child(ground).child(div().absolute().left(px(e)).top(px(e)).right(px(e)).bottom(px(e)).overflow_hidden().child(content)).into_element()
}

/// The place's clock: a time as seconds from its own 1970 midnight.
fn local_of(place: &Place, f: &Forecast) -> impl Fn(f64) -> i64 + Clone + 'static + use<> {
    let place = place.clone();
    let fallback = f.offset;
    move |t: f64| t as i64 + place.offset_at_or(t as i64, fallback) as i64
}

/// "14:05" in the place's clock.
fn hhmm(local: i64) -> String {
    let (h, mi, _, _) = olo_kit::widgets::timeline::parts(local);
    format!("{h:02}:{mi:02}")
}

pub fn body(w: &mut Weather, cx: &mut Cx<Weather>) -> Element {
    let bar = MenuBar::of(w, cx, |w, _| w.menus());
    let p = theme::palette();
    let e = edge(cx);
    let sidebar = sidebar(w, cx);
    let main = main_area(w, cx);
    // Inset from the window's edges, the columns a spacing apart.
    let mut row = div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_row()
        .gap(px(m::SPACING))
        .px(px(m::WINDOW_INSET))
        .pt(px(m::SPACING))
        .pb(px(m::WINDOW_INSET))
        .child(div().w(px(SIDEBAR)).flex_shrink_0().h_full().flex().flex_col().child(sidebar))
        .child(div().flex_1().min_w(px(MAIN_MIN)).h_full().child(well(e, p.document_background, main)));
    if w.settings.assistant && !w.settings.places.is_empty() && !w.needs_key() {
        row = row.child(div().w(px(ASSISTANT)).min_w(px(ASSISTANT_MIN)).flex_shrink().h_full().child(assistant_pane(w, cx)));
    }
    // (Escape lets an answer coming go, as Assistant ▸ Stop does, from
    // wherever the keyboard is, if nothing there takes it.)
    let escape = cx.listener(|v: &mut Weather, e: &KeyDownEvent, cx| {
        if e.keystroke.key == "escape" && v.asking.is_some() {
            v.stop_asking();
            cx.invalidate();
        }
    });
    div().size_full().flex().flex_col().bg(style::c(p.panel_background)).on_window_key_down(escape).child(bar).child(row).into_element()
}

/// The places kept, each with its sky and temperature now; Add Place…
/// under them.
fn sidebar(w: &mut Weather, cx: &mut Cx<Weather>) -> Element {
    let p = theme::palette();
    let now = provider::now();
    let mut rows: Vec<Row> = vec![Row::heading("Places")];
    for place in &w.settings.places {
        let entry = w.entries.get(&crate::place_id(place));
        let f = entry.and_then(|e| e.forecast.clone());
        let (icon, temp) = match f.as_ref().and_then(|f| f.now(now).map(|s| (f, s))) {
            Some((f, s)) => (model::icon(s.code(), f.is_night(now)), s.get(Layer::Temperature).map(|t| w.settings.units.degrees(t)).unwrap_or_default()),
            None => ("weather", String::new()),
        };
        rows.push(Row::new(icon, place.title().to_owned()).value(temp));
    }
    let chosen = w.settings.chosen.min(w.settings.places.len().saturating_sub(1)) + 1;
    let list = Sidebar::new("weather-places", rows, chosen)
        .bare()
        .on_choose(cx.listener(|v: &mut Weather, i: &usize, cx| {
            if *i > 0 {
                v.choose(i - 1, cx);
            }
        }))
        // (Dragged into another order: the places' own, the heading
        // first.)
        .on_reorder(cx.listener(|v: &mut Weather, (from, to): &(usize, usize), cx| {
            if *from > 0 && *to > 0 {
                v.move_place(from - 1, to - 1, cx);
            }
        }))
        .on_menu(cx.listener(|v: &mut Weather, (i, at): &(usize, olo_kit::Point<Pixels>), cx| {
            if *i == 0 {
                return;
            }
            let i = i - 1;
            let last = v.settings.places.len().saturating_sub(1);
            let menu = olo_kit::Menu::new("Place")
                .item("Show", format!("place:{i}"))
                .item("Rename…", format!("rename-at:{i}"))
                .separator()
                .item("Move Up", format!("move-place:{i}:{}", i.saturating_sub(1)))
                .enabled(i > 0)
                .item("Move Down", format!("move-place:{i}:{}", (i + 1).min(last)))
                .enabled(i < last)
                .separator()
                .item("Remove Place", format!("remove:{i}"));
            let view = cx.view_ref();
            olo_kit::menu::context_menu(cx, menu, *at, move |cmd, cx| {
                let cmd = cmd.to_owned();
                view.update(cx, move |v: &mut Weather, cx| {
                    if let Some(i) = cmd.strip_prefix("remove:").and_then(|i| i.parse().ok()) {
                        v.remove_place(i, cx);
                    } else if let Some(i) = cmd.strip_prefix("rename-at:").and_then(|i| i.parse().ok()) {
                        v.open_rename(i, cx);
                    } else {
                        v.command(&cmd, cx);
                    }
                });
            });
        }))
        .build(cx);
    let e = edge(cx);
    div()
        .size_full()
        .flex()
        .flex_col()
        .gap(px(m::TIGHT))
        .child(div().flex_1().min_h_0().child(well(e, p.list_background, list)))
        .child(div().flex().child(Button::new("Add Place…").id("add-place").on_click(cx.listener(|v: &mut Weather, _: &ClickEvent, cx| v.open_add_place(cx)))))
        .into_element()
}

/// What the well beside the places shows.
fn main_area(w: &mut Weather, cx: &mut Cx<Weather>) -> Element {
    let provider = w.provider();
    if w.needs_key() {
        let detail = olo_kit::tr!(
            "Weather’s forecasts come from {}, which needs an API key (a free one takes a minute to make). Give it once and it’s kept in Passwords, or point to one already there.",
            provider.name()
        );
        return div()
            .size_full()
            .child(
                EmptyState::new(olo_kit::tr!("Forecasts from {}", provider.name()))
                    .icon("weather")
                    .detail(detail)
                    .action("Give a Key…", cx.listener(|v: &mut Weather, _: &ClickEvent, cx| v.open_key_dialog(cx))),
            )
            .into_element();
    }
    let Some(place) = w.place().cloned() else {
        return div()
            .size_full()
            .child(
                EmptyState::new("No places yet")
                    .icon("weather")
                    .detail("Add the places you want the weather for: a town, an airfield, the coordinates of a field. Each keeps its own clock.")
                    .action("Add Place…", cx.listener(|v: &mut Weather, _: &ClickEvent, cx| v.open_add_place(cx))),
            )
            .into_element();
    };
    let entry = w.entry();
    let forecast = entry.and_then(|e| e.forecast.clone());
    let status = status_line(w);
    let Some(f) = forecast else {
        let (title, detail) = match entry.and_then(|e| e.error.as_ref()) {
            Some(err) => ("The forecast can’t be fetched", err.to_string()),
            None => ("Fetching the forecast…", entry.and_then(|e| e.fetching.as_ref()).map(|(_, _, s)| s.to_string()).unwrap_or_default()),
        };
        let mut state = EmptyState::new(title).icon("weather").detail(detail);
        if entry.is_some_and(|e| e.error.is_some()) {
            state = state.action(
                "Try Again",
                cx.listener(|v: &mut Weather, _: &ClickEvent, cx| {
                    v.command("refresh", cx);
                }),
            );
            if matches!(entry.and_then(|e| e.error.as_ref()), Some(Error::Key(_))) {
                state = state.action("Data Provider…", cx.listener(|v: &mut Weather, _: &ClickEvent, cx| v.open_key_dialog(cx)));
            }
        }
        return div().size_full().child(state).into_element();
    };
    let units = w.settings.units;
    let now = provider::now();
    let header = header(&place, &f, &units, now, status);
    let cards = cards(&f, &units, now);
    let next = next_hour(&f, &units, now);
    let tabs: Rc<[olo_kit::SharedString]> = Rc::from(vec![olo_kit::words::tr("Rest of Today").into(), "Next 3 Days".into(), "Next 7 Days".into()]);
    let tab = w.settings.tab.min(2);
    let (sections, chart, axis) = tab_content(w, &place, &f, tab, now);
    // The chart's time axis stays at the top once the chart reaches it,
    // its panels scrolling under it (Ben, 2026-11-15: "the timeline should
    // be at the top and stay there as you scroll the data so that you
    // always know when it is").
    let scroller = ScrollView::new(&w.scrolls[tab]).border(false).auto_hide(true).background(theme::palette().document_background).children(sections).sticky(
        chart,
        olo_kit::widgets::timeline::AXIS_HEIGHT,
        div().w_full().pr(px(m::TIGHT)).child(axis),
    );
    let tab_view = TabView::new("weather-range", tabs, tab).on_select(cx.listener(|v: &mut Weather, i: &usize, cx| v.set_tab(*i, cx))).child(div().size_full().child(scroller));
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(div().px(px(m::WINDOW_INSET)).pb(px(m::TIGHT)).child(cards))
        .child(next)
        .child(div().flex_1().min_h_0().px(px(m::TIGHT)).pb(px(m::TIGHT)).child(tab_view))
        .into_element()
}

/// Under the place's name: when it was fetched, or what's under way, or
/// what went wrong.
fn status_line(w: &Weather) -> (String, bool) {
    let Some(entry) = w.entry() else { return (String::new(), false) };
    let provider = w.provider().name();
    if let Some((_, _, stage)) = &entry.fetching {
        return (format!("{stage} ({provider})"), false);
    }
    if let Some(err) = &entry.error {
        let kept = entry.forecast.as_ref().map(|f| format!(" {}", olo_kit::tr!("Showing the forecast from {}.", ago(provider::now() - f.fetched)))).unwrap_or_default();
        return (format!("{err}{kept}"), true);
    }
    match &entry.forecast {
        Some(f) => (olo_kit::tr!("Updated {} · {}", ago(provider::now() - f.fetched), provider), false),
        None => (String::new(), false),
    }
}

/// "just now", "a minute ago", "12 minutes ago", "2 hours ago": said
/// again as it changes (`olo_kit::ago`).
fn ago(seconds: i64) -> String {
    olo_kit::ago::age(std::time::Duration::from_secs(seconds.max(0) as u64), |age| match age.as_secs() {
        s if s < 60 => olo_kit::words::tr("just now").into(),
        s if s < 120 => olo_kit::words::tr("a minute ago").into(),
        s if s < 3600 => olo_kit::tr!("{} minutes ago", s / 60),
        s if s < 2 * 3600 => olo_kit::words::tr("an hour ago").into(),
        s if s < 86_400 => olo_kit::tr!("{} hours ago", s / 3600),
        s => olo_kit::tr!("{} days ago", s / 86_400),
    })
}

/// The sky now, the temperature, what it's doing; the place, its clock.
fn header(place: &Place, f: &Forecast, units: &Units, now: i64, (status, wrong): (String, bool)) -> Element {
    let step = f.now(now).cloned();
    let hour = f.hour(now).cloned();
    let night = f.is_night(now);
    let today = today(f, place, now).cloned();
    let local = local_of(place, f)(now as f64);
    let (h, mi, wd, d) = olo_kit::widgets::timeline::parts(local);
    let clock = olo_kit::tr!("{} {}, {} there", olo_kit::widgets::timeline::WEEKDAYS[wd], d, format!("{h:02}:{mi:02}"));
    let name = place.title().to_owned();
    let mut about = vec![];
    // (Its own name in the title: what it was found as beside, then.)
    match place.found_as() {
        Some(found) => about.push(found),
        None if !place.region.is_empty() => about.push(place.region.clone()),
        None => {}
    }
    about.push(place.coordinates());
    if let Some(e) = place.elevation {
        about.push(units.say(Quantity::Height, e / 1000.0));
    }
    let about = about.join(" · ");
    let units = *units;
    let (zone_place, fallback) = (place.clone(), f.offset);
    let place_offset = move |t: i64| zone_place.offset_at_or(t, fallback) as i64;
    let _ = &hour;
    let key = style::key((&step.as_ref().map(|s| (s.time, s.v.map(f32::to_bits))), &today.as_ref().map(|d| d.time), &name, &about, &clock, &status, wrong, format!("{units:?}"), night));
    canvas(move |g, b| {
        let p = theme::palette();
        let text = style::c(p.document_text);
        let dim = style::disabled_text(&p);
        let regular = style::ui_font(FontWeight::NORMAL);
        let bold = style::ui_font(FontWeight::BOLD);
        let x0 = b.left() + px(m::WINDOW_INSET);
        let right = b.right() - px(m::WINDOW_INSET);
        // The place, where it is, and its clock.
        let row1 = b.top() + px(22.0);
        let c = style::label_on_baseline(g, right, row1, style::Align::Right, clock.clone(), style::SMALL_SIZE, regular.clone(), dim);
        olo_kit::agent::record(g, olo_kit::agent::Node::label(clock.clone(), c));
        let room = (c.left() - x0 - px(m::SECTION)).max(px(80.0));
        let n = style::label_on_baseline_fit(g, x0, row1, style::Align::Left, name.clone(), style::LEAD_SIZE, bold.clone(), text, room);
        olo_kit::agent::record(g, olo_kit::agent::Node { role: "Heading", name: Some(name.clone().into()), bounds: n, enabled: true, ..Default::default() });
        let ax = n.right() + px(m::SPACING);
        let a = style::label_on_baseline_fit(g, ax, row1, style::Align::Left, about.clone(), style::SMALL_SIZE, regular.clone(), dim, (c.left() - ax - px(m::SECTION)).max(px(0.0)));
        olo_kit::agent::record(g, olo_kit::agent::Node::label(about.clone(), a));
        // The sky now, the temperature, what it's doing.
        let top = b.top() + px(34.0);
        let code = step.as_ref().map_or(0, |s| s.code());
        let tx = x0 + px(56.0 + m::SPACING);
        let temp = step.as_ref().and_then(|s| s.get(Layer::Temperature)).map(|t| units.degrees(t)).unwrap_or_else(|| "–".into());
        let r = style::label_on_baseline(g, tx, top + px(44.0), style::Align::Left, temp.clone(), 40.0, bold.clone(), text);
        // The sky's picture level with the temperature.
        let caps = top + px(44.0 - style::CAP_HEIGHT * 40.0 / 2.0);
        let icon_r = style::snap_centred(g, point(x0 + px(28.0), caps), 56.0, 56.0);
        icons::paint(g, icon_r, model::icon(code, night));
        olo_kit::agent::record(g, olo_kit::agent::Node { role: "Heading", name: Some(format!("{temp} {}", model::describe_said(code)).into()), bounds: r, enabled: true, ..Default::default() });
        let cx_ = r.right() + px(m::SPACING);
        let room = (right - cx_).max(px(40.0));
        style::label_on_baseline_fit(g, cx_, top + px(23.0), style::Align::Left, model::describe_said(code), style::LEAD_SIZE, bold.clone(), text, room);
        let mut line = Vec::new();
        if let Some(fl) = step.as_ref().and_then(|s| s.get(Layer::TemperatureApparent)) {
            line.push(olo_kit::tr!("Feels like {}", units.degrees(fl)));
        }
        if let Some(d) = &today
            && let (Some(hi), Some(lo)) = (d.get(Layer::Temperature, Stat::Max), d.get(Layer::Temperature, Stat::Min))
        {
            line.push(olo_kit::tr!("High {}, low {}", units.degrees(hi), units.degrees(lo)));
        }
        if let Some(d) = &today
            && let (Some(rise), Some(set)) = (d.sunrise, d.sunset)
        {
            let local = |t: i64| t + place_offset(t);
            line.push(olo_kit::tr!("Sun {}–{}", hhmm(local(rise)), hhmm(local(set))));
        }
        let line = line.join("  ·  ");
        let l = style::label_on_baseline_fit(g, cx_, top + px(43.0), style::Align::Left, line.clone(), style::PLAIN_SIZE, regular.clone(), dim, room);
        olo_kit::agent::record(g, olo_kit::agent::Node::label(line.clone(), l));
        // What's going on with the forecast.
        let ink = if wrong { style::c(p.failure) } else { dim };
        let s = style::label_fit(g, x0, b.bottom() - px(11.0), style::Align::Left, status.clone(), style::SMALL_SIZE, regular.clone(), ink, right - x0);
        olo_kit::agent::record(g, olo_kit::agent::Node { role: if wrong { "Alert" } else { "Label" }, name: Some(status.clone().into()), bounds: s, enabled: true, ..Default::default() });
    })
    .paint_key(key)
    .w_full()
    .h(px(HEADER))
    .flex_shrink_0()
    .into_element()
}

/// The day holding `now` in the place's clock.
fn today<'a>(f: &'a Forecast, _place: &Place, now: i64) -> Option<&'a Day> {
    f.daily.iter().rev().find(|d| d.time <= now).or(f.daily.first())
}

/// The cards of what matters now: clouds, visibility, wind,
/// precipitation, the air, the sun.
fn cards(f: &Forecast, units: &Units, now: i64) -> Element {
    let s = f.now(now).cloned().unwrap_or_else(|| Step::new(now));
    let h = f.hour(now).cloned().unwrap_or_else(|| s.clone());
    let get = |l: Layer| s.get(l).or_else(|| h.get(l));
    let say = |l: Layer| get(l).map(|v| units.say(l.info().quantity, v));
    let day = f.daily.iter().rev().find(|d| d.time <= now).or(f.daily.first());
    let mut out = Vec::new();
    // Clouds: the base, then the ceiling and the cover.
    let cover = get(Layer::CloudCover);
    let base = get(Layer::CloudBase);
    let (figure, caption) = match (base, cover) {
        (Some(b), _) => (
            units.say(Quantity::Height, b),
            [say(Layer::CloudCeiling).map(|c| olo_kit::tr!("Ceiling {}", c)), cover.map(|c| olo_kit::tr!("{}% cover", c.round()))].into_iter().flatten().collect::<Vec<_>>().join(" · "),
        ),
        (None, Some(c)) if c < 5.0 => (olo_kit::words::tr("Clear sky").into(), olo_kit::words::tr("No cloud").into()),
        (None, c) => ("–".into(), c.map(|c| olo_kit::tr!("{}% cover", c.round())).unwrap_or_default()),
    };
    out.push(("clouds", "Cloud base", figure, caption));
    // Visibility, and fog's chance by the dew point's spread.
    let vis = get(Layer::Visibility);
    let spread = get(Layer::DewPointDepression).or_else(|| Some(get(Layer::Temperature)? - get(Layer::DewPoint)?));
    let foggy = vis.is_some_and(|v| v < 1.0);
    let caption = match spread {
        Some(sp) if sp < 2.5 && !foggy => olo_kit::tr!("Fog likely · spread {}", units.say(Quantity::TemperatureSpread, sp)),
        Some(sp) => olo_kit::tr!("Dew point spread {}", units.say(Quantity::TemperatureSpread, sp)),
        None => String::new(),
    };
    out.push(("visibility", if foggy { "Visibility · fog" } else { "Visibility" }, say(Layer::Visibility).unwrap_or_else(|| "–".into()), caption));
    // Wind and gusts, and where from.
    let dir = get(Layer::WindDirection);
    let caption =
        [say(Layer::WindGust).map(|g| olo_kit::tr!("Gusts {}", g)), dir.map(|d| olo_kit::tr!("from {}", olo_kit::words::tr(model::compass(d))))].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    out.push(("wind", "Wind", say(Layer::WindSpeed).unwrap_or_else(|| "–".into()), caption));
    // What's falling, how hard, and the chances.
    let rate = s.precipitation().or_else(|| h.precipitation()).unwrap_or(0.0);
    let kind = if rate > 0.0 { s.kind() } else { Kind::None };
    let figure = if rate >= 0.05 { units.say(Quantity::Rate, rate) } else { "None".into() };
    let caption = [
        h.get(Layer::PrecipitationProbability).map(|c| olo_kit::tr!("{}% chance this hour", c.round())),
        h.get(Layer::ThunderstormProbability).filter(|c| *c >= 1.0).map(|c| format!("storms {}%", c.round())),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let title = match kind {
        Kind::None => "Precipitation",
        k => k.name(),
    };
    out.push(("precipitation", title, figure, caption));
    // The air: humidity, dew point, pressure.
    let caption = [say(Layer::DewPoint).map(|d| olo_kit::tr!("Dew point {}", d)), say(Layer::PressureSeaLevel)].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    out.push(("air", "Humidity", say(Layer::Humidity).unwrap_or_else(|| "–".into()), caption));
    // The sun: UV now, and the day's highest (its times are in the
    // header).
    let uv = get(Layer::UvIndex);
    let highest = day.and_then(|d| d.get(Layer::UvIndex, Stat::Max));
    let caption = [uv.map(|u| olo_kit::words::tr(model::uv_level(u)).to_owned()), highest.map(|h| olo_kit::tr!("up to {} today", h.round()))].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    out.push(("sun", "UV index", uv.map(|u| format!("{}", u.round())).unwrap_or_else(|| "–".into()), caption));
    // Three a row, however wide (their captions cut short when narrow).
    let mut row = div().grid().grid_cols(3).gap(px(olo_kit::widgets::dashboard::GAP));
    for (id, title, figure, caption) in out {
        row = row.child(div().min_w_0().child(Card::new(format!("now-{id}"), title).figure(figure).caption(caption)));
    }
    row.into_element()
}

/// The next hour, minute by minute: what's falling, and when it starts or
/// stops.
fn next_hour(f: &Forecast, units: &Units, now: i64) -> Element {
    let minutes: Vec<(f64, Kind)> = f.minutely.iter().filter(|s| s.time + 60 > now).take(60).map(|s| (s.precipitation().unwrap_or(0.0), s.kind())).collect();
    let said = next_hour_sentence(&minutes, f.hour(now));
    let units = *units;
    let peak = minutes.iter().map(|(r, _)| *r).fold(0.0, f64::max);
    let key = style::key((&said, minutes.iter().map(|(r, k)| ((r * 100.0) as i64, *k as u8)).collect::<Vec<_>>(), format!("{units:?}")));
    canvas(move |g, b| {
        let p = theme::palette();
        let x0 = b.left() + px(m::WINDOW_INSET);
        let cy = b.center().y;
        let s = style::label(g, x0, cy, style::Align::Left, said.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), style::c(p.document_text));
        olo_kit::agent::record(
            g,
            olo_kit::agent::Node {
                role: "Label",
                name: Some(said.clone().into()),
                value: Some(format!("peak {}", units.say(Quantity::Rate, peak)).into()),
                bounds: s,
                enabled: true,
                ..Default::default()
            },
        );
        if minutes.is_empty() {
            return;
        }
        // The minutes as bars, from now to an hour on.
        let left = (s.right() + px(m::SECTION)).max(b.left() + px(300.0));
        let right = b.right() - px(m::WINDOW_INSET);
        if right - left < px(120.0) {
            return;
        }
        let area = style::snap(g, Bounds { origin: point(left, b.top() + px(8.0)), size: size(right - left, b.size.height - px(26.0)) });
        let base = style::c(olo_theme::mix(p.document_text, p.document_background, 0.82));
        style::hline_above(g, area.left(), area.right(), area.bottom(), base);
        let top = peak.max(2.5);
        let w = f32::from(area.size.width) / 60.0;
        let sc = g.scale_factor();
        for (i, (r, k)) in minutes.iter().enumerate() {
            if *r <= 0.0 {
                continue;
            }
            let hgt = ((r / top).min(1.0) as f32 * f32::from(area.size.height)).max(1.0 / sc);
            let l = px((f32::from(area.left() + px(i as f32 * w)) * sc).round() / sc);
            let rr = px((f32::from(area.left() + px((i + 1) as f32 * w)) * sc).round() / sc);
            style::fill_rect(g, Bounds::from_corners(point(l, area.bottom() - px(hgt)), point(rr, area.bottom())), kind_color(&p, *k));
        }
        let dim = style::disabled_text(&p);
        for (frac, label) in [(0.0, olo_kit::words::tr("Now")), (0.5, olo_kit::words::tr("30 min")), (1.0, olo_kit::words::tr("60 min"))] {
            let x = area.left() + area.size.width * frac;
            let align = if frac == 0.0 {
                style::Align::Left
            } else if frac == 1.0 {
                style::Align::Right
            } else {
                style::Align::Center
            };
            style::label(g, x, b.bottom() - px(9.0), align, label, style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), dim);
        }
    })
    .paint_key(key)
    .w_full()
    .h(px(56.0))
    .flex_shrink_0()
    .into_element()
}

/// The next hour said: "Rain stopping in about 25 minutes".
pub fn next_hour_sentence(minutes: &[(f64, Kind)], hour: Option<&Step>) -> String {
    const WET: f64 = 0.05;
    if minutes.is_empty() {
        return match hour.and_then(|h| h.precipitation()) {
            Some(r) if r >= WET => olo_kit::tr!("{} this hour", hour.map_or(Kind::Rain, |h| h.kind()).said()),
            _ => olo_kit::words::tr("No precipitation this hour").into(),
        };
    }
    let wet_now = minutes[0].0 >= WET;
    let what = minutes.iter().find(|(r, _)| *r >= WET).map_or(Kind::Rain, |(_, k)| if *k == Kind::None { Kind::Rain } else { *k });
    let name = what.said();
    // The first minute it's dry (or wet) for ten minutes on.
    let turns = |wet: bool| (1..minutes.len()).find(|&i| minutes[i..(i + 10).min(minutes.len())].iter().all(|(r, _)| (*r >= WET) == wet));
    match (wet_now, turns(!wet_now)) {
        (true, Some(i)) => olo_kit::tr!("{} stopping in about {} minutes", name, i.max(1)),
        (true, None) => {
            let peak = minutes.iter().map(|(r, _)| *r).fold(0.0, f64::max);
            let strength = if peak >= 7.6 {
                2
            } else if peak >= 2.5 {
                1
            } else {
                0
            };
            olo_kit::tr!("{} for the next hour", what.said_with(strength))
        }
        (false, Some(i)) => olo_kit::tr!("{} starting in about {} minutes", name, i.max(1)),
        (false, None) => olo_kit::words::tr("No precipitation in the next hour").into(),
    }
}

fn kind_color(p: &olo_theme::Palette, k: Kind) -> Hsla {
    style::c(match k {
        Kind::Snow => p.hues.purple,
        Kind::Sleet => p.hues.teal,
        Kind::FreezingRain => p.hues.pink,
        _ => p.hues.blue,
    })
}

/// The hours a tab's chart shows: from this hour to midnight or twelve
/// hours on, whichever is later (Rest of Today), three days (Next 3
/// Days), or as far as the forecast goes by the hour (Next 7 Days).
pub fn tab_range(place: &Place, f: &Forecast, tab: usize, now: i64) -> (f64, f64) {
    let local = local_of(place, f);
    let hour0 = (now - now.rem_euclid(3600)) as f64;
    let last = f.hourly.last().map_or(hour0 + 3600.0, |h| (h.time + 3600) as f64);
    let end = match tab {
        0 => {
            let l = local(now as f64);
            let midnight = (l.div_euclid(86_400) + 1) * 86_400 - (l - now);
            (midnight as f64).max(hour0 + 12.0 * 3600.0)
        }
        1 => hour0 + 72.0 * 3600.0,
        _ => last,
    }
    .min(last);
    (hour0, end)
}

/// The narrowest tab whose chart shows all of `spans` (or as much as
/// the forecast reaches).
pub fn tab_for(place: &Place, f: &Forecast, spans: &[crate::assistant::Window], now: i64) -> Option<usize> {
    let last = spans.iter().map(|s| s.end).max()? as f64;
    Some((0..3).find(|t| tab_range(place, f, *t, now).1 >= last).unwrap_or(2))
}

/// When a span is, in the place's clock: "09:00–12:00" today, "tomorrow
/// 09:00–12:00", "Tue 22:00 – Wed 03:00".
pub fn span_when(local: impl Fn(f64) -> i64, now: i64, start: i64, end: i64) -> String {
    let today = local(now as f64).div_euclid(86_400);
    let day = |t: i64| {
        let d = local(t as f64).div_euclid(86_400);
        match d - today {
            0 => String::new(),
            1 => "tomorrow ".into(),
            _ => format!("{} ", olo_kit::widgets::timeline::WEEKDAYS[olo_kit::widgets::timeline::parts(local(t as f64)).2]),
        }
    };
    let (a, z) = (local(start as f64), local(end as f64));
    // (Ending at midnight is the same day's end.)
    let same_day = a.div_euclid(86_400) == (z - 1).div_euclid(86_400);
    if same_day { format!("{}{}–{}", day(start), hhmm(a), hhmm(z)) } else { format!("{}{} – {}{}", day(start), hhmm(a), day(end), hhmm(z)) }
}

/// The range chosen: its chart (and for 7 days, a row a day over it).
/// A tab's sections, in order (each a child of the scroll view), which of
/// them is the chart, and the chart's axis to keep in sight.
fn tab_content(w: &Weather, place: &Place, f: &Forecast, tab: usize, now: i64) -> (Vec<Element>, usize, Element) {
    let units = w.settings.units;
    let local = local_of(place, f);
    let (hour0, end) = tab_range(place, f, tab, now);
    let spans: Vec<Span> = w.spans.iter().map(|s| Span { start: s.start as f64, end: s.end as f64, name: s.label.clone().into() }).collect();
    let key = style::key((f.fetched, f.provider.as_str(), format!("{units:?}"), tab, w.spans.iter().map(|s| (s.start, s.end, s.label.clone())).collect::<Vec<_>>(), now / 600));
    let nights: Vec<(f64, f64)> = f.nights().into_iter().map(|(a, z)| (a as f64, z as f64)).collect();
    let chart = Timeline::new(format!("weather-chart-{tab}"), hour0, end, 3600.0, panels(f, &units), key).now(now as f64).nights(nights).spans(spans).local(local.clone()).axis_on_top(true);
    let axis = chart.axis();
    // (Each its own child of the scroll view, so it knows where the chart
    // starts: its axis is kept in sight from there. The chart's own
    // starts at its top, so the copy takes its place without a jump.)
    let mut sections = vec![div().w_full().h(px(m::TIGHT)).into_element()];
    if tab == 2 {
        sections.push(div().w_full().pr(px(m::TIGHT)).child(daily(f, &units, now, local.clone())).into_element());
        let note = if f.daily.len() > 5 { "By the hour for the next 5 days (the provider’s reach):" } else { "By the hour:" };
        sections.push(div().w_full().px(px(m::WINDOW_INSET)).pt(px(m::SPACING)).pb(px(m::TIGHT)).child(StringView::new(note).dim(true)).into_element());
    }
    let at = sections.len();
    sections.push(div().w_full().pr(px(m::TIGHT)).pb(px(m::SPACING)).child(chart).into_element());
    (sections, at, axis)
}

/// Every layer by the hour, as the chart's panels.
fn panels(f: &Forecast, u: &Units) -> Vec<Panel> {
    let p = theme::palette();
    let hue = |c: olo_theme::Rgba| style::c(c);
    let hours = &f.hourly;
    let series = |l: Layer| -> Vec<(f64, f64)> { hours.iter().map(|h| (h.time as f64, h.get(l).map_or(f64::NAN, |v| u.convert(l.info().quantity, v)))).collect() };
    let say = move |q: Quantity| {
        let u = *u;
        move |v: f64| u.say(q, u.unconvert(q, v))
    };
    let tick = move |q: Quantity| {
        let u = *u;
        move |v: f64| {
            let n = u.number(q, u.unconvert(q, v));
            if q == Quantity::Percent { format!("{n}%") } else { n }
        }
    };
    let icons: Vec<(f64, olo_kit::SharedString)> = hours.iter().map(|h| (h.time as f64, model::icon(h.code(), f.is_night(h.time + 1800)).into())).collect();
    let mut out = vec![Panel::icons(icons)];
    // Temperatures, with freezing marked.
    let t = Quantity::Temperature;
    let freezing = u.convert(t, 0.0);
    out.push(
        Panel::plot(olo_kit::tr!("Temperature ({})", u.unit(t)), 104.0, Scale::new(tick(t)))
            .track(Track::line("Temperature", hue(p.hues.orange), series(Layer::Temperature)).say(say(t)))
            .track(Track::line("Feels like", hue(p.hues.red), series(Layer::TemperatureApparent)).thin().say(say(t)))
            .track(Track::line("Dew point", hue(p.hues.teal), series(Layer::DewPoint)).thin().say(say(t)))
            .guide(freezing, "Freezing"),
    );
    // The clouds, as a meteogram draws them: each hour shaded from the
    // cloud's base up, as solid as its cover, the ceiling (broken or
    // overcast) darker over it; the clear air under is what's open to
    // fly. (A model's base leaps from hour to hour; a line through it
    // only zigzags.) The cover reads under the pointer.
    let hq = Quantity::Height;
    let cloud = hue(olo_theme::mix(p.hues.gray, p.document_background, 0.15));
    let alpha: Vec<f32> = hours.iter().map(|h| (0.12 + h.get(Layer::CloudCover).unwrap_or(0.0) / 100.0 * 0.5) as f32).collect();
    out.push(
        Panel::plot(olo_kit::tr!("Clouds ({} above the ground)", u.unit(hq)), 124.0, Scale::new(tick(hq)).at_least(u.convert(hq, 3.0)).from_zero())
            .track(Track::above("Cloud base", cloud, series(Layer::CloudBase)).alpha(alpha).say(say(hq)))
            .track(Track::above("Ceiling", hue(olo_theme::mix(p.hues.gray, p.document_text, 0.35)), series(Layer::CloudCeiling)).alpha(vec![0.3f32; hours.len()]).say(say(hq)))
            .track(Track::quiet("Cover", cloud, series(Layer::CloudCover)).say(say(Quantity::Percent))),
    );
    // Visibility, with fog marked; the dew point's spread on the right.
    let dq = Quantity::Distance;
    out.push(
        Panel::plot(olo_kit::tr!("Visibility ({})", u.unit(dq)), 84.0, Scale::new(tick(dq)).at_least(u.convert(dq, 10.0)))
            .right(Scale::new(tick(Quantity::TemperatureSpread)).at_least(u.convert(Quantity::TemperatureSpread, 6.0)))
            .track(Track::line("Visibility", hue(p.hues.purple), series(Layer::Visibility)).say(say(dq)))
            .track(Track::line("Dew point spread", hue(p.hues.brown), series(Layer::DewPointDepression)).thin().on_right().say(say(Quantity::TemperatureSpread)))
            .guide(u.convert(dq, 1.0), "Fog"),
    );
    // Precipitation: how hard, each hour coloured by what falls; the
    // chances on the right.
    let rq = Quantity::Rate;
    let rates: Vec<(f64, f64)> = hours.iter().map(|h| (h.time as f64, h.precipitation().map_or(f64::NAN, |v| u.convert(rq, v)))).collect();
    let colors: Vec<Hsla> = hours.iter().map(|h| kind_color(&p, h.kind())).collect();
    out.push(
        Panel::plot(olo_kit::tr!("Precipitation ({})", u.unit(rq)), 100.0, Scale::new(tick(rq)).at_least(u.convert(rq, 2.0)))
            .right(Scale::new(tick(Quantity::Percent)).range(0.0, 100.0))
            .track(Track::columns("Precipitation", hue(p.hues.blue), rates).colors(colors).say(say(rq)))
            .track(Track::line("Chance", hue(olo_theme::mix(p.hues.blue, p.document_text, 0.3)), series(Layer::PrecipitationProbability)).thin().on_right().say(say(Quantity::Percent)))
            .track(Track::line("Thunderstorms", hue(p.hues.yellow), series(Layer::ThunderstormProbability)).thin().on_right().say(say(Quantity::Percent))),
    );
    // Wind and gusts, and where it comes from.
    let sq = Quantity::Speed;
    out.push(
        Panel::plot(olo_kit::tr!("Wind ({})", u.unit(sq)), 90.0, Scale::new(tick(sq)).at_least(u.convert(sq, 8.0)))
            .track(Track::line("Wind", hue(p.hues.green), series(Layer::WindSpeed)).say(say(sq)))
            .track(Track::line("Gusts", hue(olo_theme::mix(p.hues.green, p.document_background, 0.35)), series(Layer::WindGust)).thin().say(say(sq))),
    );
    let dirs: Vec<(f64, f64)> = hours.iter().map(|h| (h.time as f64, h.get(Layer::WindDirection).unwrap_or(f64::NAN))).collect();
    out.push(Panel::arrows(dirs, hue(olo_theme::mix(p.hues.green, p.document_text, 0.4))));
    // The air: humidity, and the pressure on its own scale.
    let pq = Quantity::Pressure;
    out.push(
        Panel::plot(olo_kit::tr!("Humidity and pressure ({})", u.unit(pq)), 80.0, Scale::new(tick(pq)))
            .right(Scale::new(tick(Quantity::Percent)).range(0.0, 100.0))
            .track(Track::line("Pressure", hue(p.hues.purple), series(Layer::PressureSeaLevel)).say(say(pq)))
            .track(Track::line("Humidity", hue(p.hues.teal), series(Layer::Humidity)).thin().on_right().say(say(Quantity::Percent))),
    );
    // The sun's UV, each hour coloured by its level.
    let uv: Vec<(f64, f64)> = hours.iter().map(|h| (h.time as f64, h.get(Layer::UvIndex).unwrap_or(f64::NAN))).collect();
    let uv_colors: Vec<Hsla> = hours
        .iter()
        .map(|h| {
            let i = h.get(Layer::UvIndex).unwrap_or(0.0);
            hue(if i < 3.0 {
                p.hues.green
            } else if i < 6.0 {
                p.hues.yellow
            } else if i < 8.0 {
                p.hues.orange
            } else if i < 11.0 {
                p.hues.red
            } else {
                p.hues.purple
            })
        })
        .collect();
    out.push(
        Panel::plot("UV index", 56.0, Scale::new(|v| format!("{v}")).at_least(8.0))
            .track(Track::columns("UV index", hue(p.hues.yellow), uv).colors(uv_colors).say(|v| format!("{} ({})", v.round(), model::uv_level(v)))),
    );
    // Strips have no title; plots do.
    let _ = PanelKind::Icons;
    out
}

/// The next 7 days, a row each: the sky by day and night, what it does,
/// the temperatures' span among the week's, then every other layer.
fn daily(f: &Forecast, units: &Units, now: i64, local: impl Fn(f64) -> i64 + Clone + 'static) -> Element {
    let days: Vec<Day> = f.daily.iter().filter(|d| d.time + 86_400 > now).take(7).cloned().collect();
    let (week_lo, week_hi) =
        days.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), d| (lo.min(d.get(Layer::Temperature, Stat::Min).unwrap_or(lo)), hi.max(d.get(Layer::Temperature, Stat::Max).unwrap_or(hi))));
    let units = *units;
    const ROW: f32 = 68.0;
    let height = ROW * days.len() as f32;
    let key = style::key((f.fetched, format!("{units:?}"), now / 3600));
    let today_start = days.first().map(|d| d.time);
    canvas(move |g, b| {
        let p = theme::palette();
        let text = style::c(p.document_text);
        let dim = style::disabled_text(&p);
        let rule = style::c(olo_theme::mix(p.document_text, p.document_background, 0.88));
        let font = style::ui_font(FontWeight::NORMAL);
        let bold = style::ui_font(FontWeight::BOLD);
        let x0 = b.left() + px(m::WINDOW_INSET);
        let right = b.right() - px(m::WINDOW_INSET);
        for (i, d) in days.iter().enumerate() {
            let top = b.top() + px(i as f32 * ROW);
            let mid = top + px(17.0);
            let (_, _, wd, dd) = olo_kit::widgets::timeline::parts(local(d.time as f64 + 6.0 * 3600.0));
            let name = if Some(d.time) == today_start && d.time <= now { olo_kit::words::tr("Today").to_owned() } else { format!("{} {dd}", olo_kit::widgets::timeline::WEEKDAYS[wd]) };
            let n = style::label(g, x0, mid, style::Align::Left, name.clone(), style::PLAIN_SIZE, bold.clone(), text);
            // The sky by day, and by night smaller.
            let day_code = match d.code(Layer::WeatherCodeDay) {
                0 => d.code(Layer::WeatherCode),
                c => c,
            };
            let night_code = d.code(Layer::WeatherCodeNight);
            let icon_x = x0 + px(64.0);
            // (Centred on the capitals of the row's text as they're drawn,
            // their baseline on a device pixel.)
            let caps = style::baseline_at(g, mid, style::PLAIN_SIZE) - px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0);
            icons::paint(g, style::snap_centred(g, point(icon_x + px(12.0), caps), 24.0, 24.0), model::icon(day_code, false));
            if night_code != 0 {
                icons::paint(g, style::snap_centred(g, point(icon_x + px(36.0), caps), 16.0, 16.0), model::icon(night_code, true));
            }
            let what = model::describe_said(day_code);
            let tx = icon_x + px(50.0);
            // The temperatures' span, among the week's.
            let bar_w = px(130.0);
            let bar_r = right;
            let bar_l = bar_r - bar_w;
            let lo = d.get(Layer::Temperature, Stat::Min);
            let hi = d.get(Layer::Temperature, Stat::Max);
            let lo_s = lo.map(|v| units.degrees(v)).unwrap_or_default();
            let hi_s = hi.map(|v| units.degrees(v)).unwrap_or_default();
            style::label(g, bar_l - px(m::TIGHT + 30.0), mid, style::Align::Right, lo_s.clone(), style::PLAIN_SIZE, font.clone(), dim);
            style::label(g, bar_l - px(m::TIGHT), mid, style::Align::Right, hi_s.clone(), style::PLAIN_SIZE, bold.clone(), text);
            let track = style::snap(g, Bounds { origin: point(bar_l, mid - px(2.0)), size: size(bar_w, px(4.0)) });
            g.paint_quad(olo_kit::fill(track, style::c(olo_theme::mix(p.document_text, p.document_background, 0.88))).corner_radii(px(2.0)));
            if let (Some(lo), Some(hi)) = (lo, hi)
                && week_hi > week_lo
            {
                let at = |v: f64| bar_l + bar_w * (((v - week_lo) / (week_hi - week_lo)) as f32);
                let fill = style::snap(g, Bounds::from_corners(point(at(lo), mid - px(2.0)), point(at(hi).max(at(lo) + px(4.0)), mid + px(2.0))));
                g.paint_quad(olo_kit::fill(fill, style::c(p.hues.orange)).corner_radii(px(2.0)));
            }
            let what_room = (bar_l - px(m::TIGHT + 30.0) - px(34.0) - tx).max(px(40.0));
            let w_r = style::label_fit(g, tx, mid, style::Align::Left, what, style::PLAIN_SIZE, font.clone(), text, what_room);
            // The rest, a line under: every layer that tells the day.
            let mut more: Vec<String> = Vec::new();
            let chance = d.get(Layer::PrecipitationProbability, Stat::Max);
            let amounts =
                [(Kind::Rain, Layer::RainAccumulation), (Kind::Snow, Layer::SnowAccumulationLwe), (Kind::Sleet, Layer::SleetAccumulationLwe), (Kind::FreezingRain, Layer::IceAccumulationLwe)]
                    .map(|(k, l)| (k, d.get(l, Stat::Sum).unwrap_or(0.0)));
            let amount: f64 = amounts.iter().map(|(_, a)| a).sum();
            // Named by what falls most (rain, unless the day says else).
            let falls = amounts.iter().filter(|(_, a)| *a > 0.0).max_by(|a, b| a.1.total_cmp(&b.1)).map_or_else(|| day_kind(d), |(k, _)| *k);
            match chance {
                Some(c) if c < 5.0 && amount < 0.1 => more.push("Dry".into()),
                Some(c) if amount >= 0.1 => more.push(format!("{} {}%, {}", falls.name(), c.round(), units.say(Quantity::Amount, amount))),
                Some(c) => more.push(format!("{} {}%", falls.name(), c.round())),
                None => {}
            }
            if let Some(t) = d.get(Layer::ThunderstormProbability, Stat::Max).filter(|t| *t >= 5.0) {
                more.push(format!("storms {}%", t.round()));
            }
            let base = d.get(Layer::CloudBase, Stat::Min);
            let cover = d.get(Layer::CloudCover, Stat::Avg);
            match (cover, base) {
                (Some(c), Some(b)) => more.push(format!("cloud {}% from {}", c.round(), units.say(Quantity::Height, b))),
                (Some(c), None) => more.push(format!("cloud {}%", c.round())),
                _ => {}
            }
            // (Only when it's poor: 10 km and more is a clear day's.)
            if let Some(v) = d.get(Layer::Visibility, Stat::Min).filter(|v| *v < 10.0) {
                more.push(olo_kit::tr!("visibility down to {}", units.say(Quantity::Distance, v)));
            }
            // (The sky's on one line, the wind, sun and moon on the next.)
            let mut next: Vec<String> = Vec::new();
            if let (Some(wv), Some(gu)) = (d.get(Layer::WindSpeed, Stat::Avg), d.get(Layer::WindGust, Stat::Max)) {
                let dir = d.get(Layer::WindDirection, Stat::Avg).map(|x| format!(" {}", olo_kit::words::tr(model::compass(x)))).unwrap_or_default();
                next.push(olo_kit::tr!("Wind {}{}, gusts {}", units.say(Quantity::Speed, wv), dir, units.say(Quantity::Speed, gu)));
            }
            // (UV once it's worth minding: moderate and up.)
            if let Some(uv) = d.get(Layer::UvIndex, Stat::Max).filter(|uv| *uv >= 3.0) {
                next.push(format!("UV {}", uv.round()));
            }
            if let (Some(r), Some(s)) = (d.sunrise, d.sunset) {
                next.push(format!("sun {}–{}", hhmm(local(r as f64)), hhmm(local(s as f64))));
            }
            let moon = d.code(Layer::MoonPhase);
            if d.get(Layer::MoonPhase, Stat::Avg).is_some() {
                next.push(model::moon(moon).to_lowercase());
            }
            let first = more.join(" · ");
            let second = next.join(" · ");
            let l_r = style::label_fit(g, tx, top + px(36.0), style::Align::Left, first.clone(), style::SMALL_SIZE, font.clone(), dim, right - tx);
            style::label_fit(g, tx, top + px(52.0), style::Align::Left, second.clone(), style::SMALL_SIZE, font.clone(), dim, right - tx);
            let line = format!("{first}; {second}");
            olo_kit::agent::record(
                g,
                olo_kit::agent::Node {
                    role: "Row",
                    name: Some(format!("{name}: {what}, {lo_s} to {hi_s}").into()),
                    value: Some(line.clone().into()),
                    bounds: Bounds::from_corners(point(b.left(), top), point(b.right(), top + px(ROW))),
                    enabled: true,
                    ..Default::default()
                },
            );
            let _ = (n, w_r, l_r);
            if i + 1 < days.len() {
                style::hline_above(g, x0, right, top + px(ROW), rule);
            }
        }
    })
    .paint_key(key)
    .w_full()
    .h(px(height))
    .flex_shrink_0()
    .into_element()
}

/// What a day's precipitation is, by its weather code's first digit, when
/// the provider gave no amounts (tomorrow.io's: 5 snow, 6 freezing rain, 7
/// ice pellets; rain otherwise).
fn day_kind(d: &Day) -> Kind {
    let mut c = d.code(Layer::WeatherCodeDay).max(d.code(Layer::WeatherCode));
    while c >= 10 {
        c /= 10;
    }
    match c {
        5 => Kind::Snow,
        6 => Kind::FreezingRain,
        7 => Kind::Sleet,
        _ => Kind::Rain,
    }
}

/// The assistant: the conversation, suggestions before it, and the
/// field to ask in.
fn assistant_pane(w: &mut Weather, cx: &mut Cx<Weather>) -> Element {
    let p = theme::palette();
    let e = edge(cx);
    let mut conv = div().w_full().flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING));
    let failure = style::c(p.failure);
    let local = w.place().cloned().zip(w.entry().and_then(|e| e.forecast.clone())).map(|(pl, f)| local_of(&pl, &f));
    for (i, t) in w.turns.iter().enumerate() {
        conv = conv.child(StringView::new(t.question.clone()).bold(true).wrap(true));
        // (Stopped before a word came: said so, quietly.)
        let answer = if t.answer.is_empty() { StringView::new("Stopped.").dim(true) } else { StringView::new(t.answer.clone()).wrap(true) };
        conv = conv.child(if t.failed { answer.color(failure) } else { answer });
        // The spans it points to, close under it: each shows them on the
        // chart (marked, in the tab that has them).
        if let Some(local) = &local
            && !t.windows.is_empty()
        {
            let now = provider::now();
            let mut list = div().flex().flex_col().gap(px(m::TIGHT)).mt(px(-(m::SPACING - m::TIGHT)));
            for (j, win) in t.windows.iter().enumerate() {
                let when = format!("{} · {}", win.label, span_when(local.clone(), now, win.start, win.end));
                list = list.child(div().flex().child(LinkButton::new(when).id(format!("span-{i}-{j}")).on_click(cx.listener(move |v: &mut Weather, _: &ClickEvent, cx| v.show_spans(i, cx)))));
            }
            conv = conv.child(list);
        }
    }
    if let Some(a) = &w.asking {
        conv = conv.child(StringView::new(a.question.clone()).bold(true).wrap(true));
        let words = if a.so_far.is_empty() { if a.model.is_empty() { olo_kit::words::tr("Thinking…").to_owned() } else { olo_kit::tr!("Thinking ({})…", a.model) } } else { a.so_far.clone() };
        conv = conv.child(StringView::new(words).wrap(true).dim(a.so_far.is_empty()));
    }
    if w.turns.is_empty() && w.asking.is_none() {
        conv = conv.child(StringView::new("Ask anything about the forecast here: the assistant has all of it, hour by hour.").dim(true).wrap(true));
        // (For the place's time of day, and whether it's raining there.)
        let now = provider::now();
        let forecast = w.entry().and_then(|e| e.forecast.clone());
        let (hour, raining) = match (w.place(), forecast.as_ref()) {
            (Some(place), Some(f)) => {
                let local = local_of(place, f);
                let wet = f.minutely.iter().find(|s| s.time + 60 > now).and_then(|s| s.precipitation()).or_else(|| f.hour(now).and_then(|h| h.precipitation())).is_some_and(|r| r >= 0.05);
                (olo_kit::widgets::timeline::parts(local(now as f64)).0, wet)
            }
            _ => (12, false),
        };
        // (A question wider than the column wraps: the column's width as
        // the row gives it, less its well's edges, its padding and the
        // scroll bar's room while there is one.)
        let room = f32::from(cx.viewport_size().width) - 2.0 * m::WINDOW_INSET - SIDEBAR - 2.0 * m::SPACING;
        let column = (room - MAIN_MIN).clamp(ASSISTANT_MIN, ASSISTANT);
        let bar = if w.chat_scroll.max_offset().height > px(0.5) { olo_kit::widgets::scroll::SCROLL_BAR } else { 0.0 };
        let wrap = column - 2.0 * e - 2.0 * m::SPACING - bar;
        for (i, s) in crate::assistant::suggestions(hour, raining).into_iter().enumerate() {
            let q = s.clone();
            conv =
                conv.child(div().flex().child(LinkButton::new(s).wrap_at(wrap).id(format!("suggestion-{i}")).on_click(cx.listener(move |v: &mut Weather, _: &ClickEvent, cx| v.ask(q.clone(), cx)))));
        }
    }
    let scroller = ScrollView::new(&w.chat_scroll).border(false).auto_hide(true).background(p.document_background).child(conv);
    let asking = w.asking.is_some();
    let button = if asking {
        Button::new("Stop").id("stop-asking").on_click(cx.listener(|v: &mut Weather, _: &ClickEvent, cx| {
            v.stop_asking();
            cx.invalidate();
        }))
    } else {
        Button::new("Ask").id("ask").default(true).on_click(cx.listener(|v: &mut Weather, _: &ClickEvent, cx| v.ask_typed(cx)))
    };
    let title =
        div().flex().flex_row().items_center().h(px(m::CONTROL_HEIGHT)).child(StringView::new("Assistant").bold(true)).child(div().flex_1()).child(
            LinkButton::new("New Conversation").id("new-conversation").enabled(!w.turns.is_empty() || asking).on_click(cx.listener(|v: &mut Weather, _: &ClickEvent, cx| v.new_conversation(cx))),
        );
    div()
        .size_full()
        .flex()
        .flex_col()
        .gap(px(m::TIGHT))
        .child(title)
        .child(div().flex_1().min_h_0().child(well(e, p.document_background, scroller)))
        .child(div().flex().flex_row().gap(px(m::TIGHT)).items_center().child(div().flex_1().min_w_0().child(TextControl::new(cx, &w.question))).child(button))
        .into_element()
}

/// What's shown, as text (Edit ▸ Copy).
pub fn summary(w: &Weather) -> String {
    let (Some(place), Some(f)) = (w.place(), w.entry().and_then(|e| e.forecast.clone())) else { return String::new() };
    let u = &w.settings.units;
    let now = provider::now();
    let local = local_of(place, &f);
    let mut out = vec![match place.found_as() {
        Some(found) => format!("{} ({found}; {})", place.title(), place.coordinates()),
        None => format!("{} ({})", place.title(), place.coordinates()),
    }];
    if let Some(s) = f.now(now) {
        let say = |l: Layer| s.get(l).map(|v| format!("{} {}", l.info().label, u.say(l.info().quantity, v)));
        let parts: Vec<String> =
            [Layer::Temperature, Layer::TemperatureApparent, Layer::CloudBase, Layer::CloudCeiling, Layer::CloudCover, Layer::Visibility, Layer::WindSpeed, Layer::WindGust, Layer::Humidity]
                .iter()
                .filter_map(|l| say(*l))
                .collect();
        out.push(format!("Now ({}): {}. {}.", hhmm(local(now as f64)), model::describe(s.code()), parts.join(", ")));
    }
    for d in f.daily.iter().filter(|d| d.time + 86_400 > now) {
        let (_, _, wd, dd) = olo_kit::widgets::timeline::parts(local(d.time as f64 + 6.0 * 3600.0));
        let hi = d.get(Layer::Temperature, Stat::Max).map(|v| u.degrees(v)).unwrap_or_default();
        let lo = d.get(Layer::Temperature, Stat::Min).map(|v| u.degrees(v)).unwrap_or_default();
        let chance = d.get(Layer::PrecipitationProbability, Stat::Max).map(|c| format!(", precipitation {}%", c.round())).unwrap_or_default();
        out.push(format!("{} {dd}: {}, {lo} to {hi}{chance}", olo_kit::widgets::timeline::WEEKDAYS[wd], model::describe(d.code(Layer::WeatherCodeDay).max(d.code(Layer::WeatherCode)))));
    }
    out.join("\n")
}

/// A forecast as JSON for agents (`--print`): the place, and each step's
/// layers by name in the person's units, with the units said.
pub fn as_json(place: &Place, f: &Forecast, u: &Units) -> Value {
    let step = |s: &Step| {
        let mut o = serde_json::Map::new();
        o.insert("time".into(), json!(chrono::DateTime::from_timestamp(s.time, 0).map(|d| d.to_rfc3339())));
        for l in Layer::ALL {
            if let Some(v) = s.get(*l) {
                o.insert(l.key().into(), json!(u.convert(l.info().quantity, v)));
            }
        }
        Value::Object(o)
    };
    let units: serde_json::Map<String, Value> = Layer::ALL.iter().filter(|l| !u.unit(l.info().quantity).is_empty()).map(|l| (l.key().to_owned(), json!(u.unit(l.info().quantity)))).collect();
    let days: Vec<Value> = f
        .daily
        .iter()
        .map(|d| {
            let mut o = serde_json::Map::new();
            o.insert("time".into(), json!(chrono::DateTime::from_timestamp(d.time, 0).map(|t| t.to_rfc3339())));
            for l in Layer::ALL {
                for s in Stat::ALL {
                    if let Some(v) = d.get(*l, s) {
                        o.insert(format!("{}{}", l.key(), s.suffix()), json!(u.convert(l.info().quantity, v)));
                    }
                }
            }
            for (k, t) in [("sunrise", d.sunrise), ("sunset", d.sunset), ("moonrise", d.moonrise), ("moonset", d.moonset)] {
                if let Some(t) = t {
                    o.insert(k.into(), json!(chrono::DateTime::from_timestamp(t, 0).map(|t| t.to_rfc3339())));
                }
            }
            Value::Object(o)
        })
        .collect();
    json!({
        "place": place,
        "provider": f.provider,
        "fetched": chrono::DateTime::from_timestamp(f.fetched, 0).map(|t| t.to_rfc3339()),
        "units": units,
        "minutely": f.minutely.iter().map(step).collect::<Vec<_>>(),
        "hourly": f.hourly.iter().map(step).collect::<Vec<_>>(),
        "daily": days,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_next_hour_is_said() {
        let rain = |r: f64| (r, Kind::Rain);
        let stopping: Vec<_> = (0..60).map(|i| rain(if i < 25 { 1.0 } else { 0.0 })).collect();
        assert_eq!(next_hour_sentence(&stopping, None), "Rain stopping in about 25 minutes");
        let starting: Vec<_> = (0..60).map(|i| rain(if i >= 40 { 0.8 } else { 0.0 })).collect();
        assert_eq!(next_hour_sentence(&starting, None), "Rain starting in about 40 minutes");
        let all: Vec<_> = (0..60).map(|_| rain(3.0)).collect();
        assert_eq!(next_hour_sentence(&all, None), "Steady rain for the next hour");
        let dry: Vec<_> = (0..60).map(|_| rain(0.0)).collect();
        assert_eq!(next_hour_sentence(&dry, None), "No precipitation in the next hour");
        // A shower of a minute or two doesn't count as stopped.
        let patchy: Vec<_> = (0..60).map(|i| rain(if i < 30 && i % 7 != 3 { 1.0 } else { 0.0 })).collect();
        assert_eq!(next_hour_sentence(&patchy, None), "Rain stopping in about 30 minutes");
    }

    #[test]
    fn spans_say_their_day() {
        // (The place's clock is UTC here; day0 is a Sunday.)
        let local = |t: f64| t as i64;
        let day0 = 1_794_700_800;
        let now = day0 + 10 * 3600;
        let h = |n: i64| day0 + n * 3600;
        assert_eq!(span_when(local, now, h(12), h(13)), "12:00–13:00");
        assert_eq!(span_when(local, now, h(20), h(24)), "20:00–00:00");
        assert_eq!(span_when(local, now, h(24 + 9), h(24 + 12)), "tomorrow 09:00–12:00");
        assert_eq!(span_when(local, now, h(22), h(24 + 3)), "22:00 – tomorrow 03:00");
        assert_eq!(span_when(local, now, h(48 + 14), h(48 + 16)), "Tue 14:00–16:00");
    }

    #[test]
    fn spans_are_shown_in_the_narrowest_tab_that_has_them() {
        let place = Place::demo();
        let t = 1_794_711_600;
        let f = crate::provider::Provider::parse(&crate::demo::Demo, &crate::demo::raw(&place, t), t).unwrap();
        let span = |a: i64, z: i64| crate::assistant::Window { start: t + a * 3600, end: t + z * 3600, label: String::new() };
        assert_eq!(tab_for(&place, &f, &[span(1, 3)], t), Some(0));
        assert_eq!(tab_for(&place, &f, &[span(1, 3), span(30, 33)], t), Some(1));
        assert_eq!(tab_for(&place, &f, &[span(90, 93)], t), Some(2));
        assert_eq!(tab_for(&place, &f, &[], t), None);
        let (start, end) = tab_range(&place, &f, 1, t);
        assert_eq!(end - start, 72.0 * 3600.0);
    }

    #[test]
    fn a_days_precipitation_is_named_by_its_code() {
        let mut d = Day::new(0);
        d.set(Layer::WeatherCodeDay, Stat::Max, 51_001.0);
        assert_eq!(day_kind(&d), Kind::Snow);
        d.set(Layer::WeatherCodeDay, Stat::Max, 4_214.0);
        assert_eq!(day_kind(&d), Kind::Rain);
    }

    #[test]
    fn a_forecast_prints_in_the_persons_units() {
        let place = Place::demo();
        let t = 1_794_711_600;
        let f = crate::provider::Provider::parse(&crate::demo::Demo, &crate::demo::raw(&place, t), t).unwrap();
        let v = as_json(&place, &f, &Units::IMPERIAL);
        assert_eq!(v["units"]["temperature"], "°F");
        assert_eq!(v["units"]["cloudBase"], "ft");
        assert_eq!(v["hourly"].as_array().unwrap().len(), 120);
        assert!(v["daily"][0]["temperatureMax"].as_f64().unwrap() > 32.0);
    }
}
