//! Display: an arrangement map of the connected screens (click one to pick
//! it, drag one to move it; it snaps edge to edge with the others), and the
//! chosen screen's resolution, refresh rate and scale. Changes apply at
//! once through app_server's ConfigureOutput; a new resolution reverts by
//! itself unless kept within `KEEP_SECS`, in case the screen went dark.
//! With more than one GPU, what each draws (Graphics: each display is
//! composed by the GPU it's wired to).

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use olo_protocol::{Client, GpuInfo, Mode, OutputConfig, OutputInfo, PowerStatus, Reply, Request, color::ColorMode};
use olo_ui::{
    gpui::{
        Bounds, DispatchPhase, FontWeight, IntoElement as _, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, SharedString, Styled as _, canvas, div,
        point, px, rgb, size,
    },
    metrics as m,
    style::{self, c},
    theme,
    widgets::{Button, CheckBox, EmptyState, MenuField, Slider, StringView, TabView},
};

use crate::{PanelView, form::Ui, panel, server::Server};

#[derive(Default)]
pub struct State {
    selected: Option<String>,
    /// The screen whose brightness is being dragged, and the level
    /// (app_server's echo lags a little).
    brightness: Option<(String, f32, Instant)>,
    /// The screen whose SDR brightness in HDR is being dragged, as the
    /// brightness is.
    sdr_nits: Option<(String, f32, Instant)>,
    /// The tab beside the display's settings: its panel, or its colours.
    side_tab: usize,
    /// The screen being dragged in the map: where the press was and
    /// where the pointer is.
    drag: Option<(String, Point<Pixels>, Point<Pixels>)>,
    /// A change waiting to be kept: the screen, how to undo it (its
    /// previous mode, or HDR off), and when it changed.
    confirm: Option<(String, Undo, Instant)>,
    /// The GPUs (app_server's `Request::Gpus`), once asked: None until
    /// it answers.
    gpus: Arc<Mutex<Option<Vec<GpuInfo>>>>,
    asked_gpus: bool,
}

/// The Colour tab shown beside the display's settings (the lint's).
#[cfg(test)]
pub(crate) fn colour_tab(st: &mut State) {
    st.side_tab = 1;
}

/// How a change waiting to be kept is undone.
#[derive(Clone)]
enum Undo {
    /// Back to the mode that worked.
    Mode(Mode),
    /// HDR off again (a display that went dark, or wrong, as it came on).
    Hdr,
}

/// Ask app_server for the GPUs, on a thread (the window not shown until
/// they're in, briefly: `hold`).
fn ask_gpus(st: &State, proxy: olo_ui::Proxy, hold: Option<olo_ui::FirstShowHold>) {
    // Tests (the layout lint): a hybrid laptop's two.
    if cfg!(test) {
        let gpu = |pci: &str, name: &str, driver: &str, boot: bool, connectors: &[&str]| GpuInfo {
            pci: pci.into(),
            name: name.into(),
            driver: driver.into(),
            boot,
            rendering: boot,
            connectors: connectors.iter().map(|c| c.to_string()).collect(),
        };
        *st.gpus.lock().unwrap() = Some(vec![
            gpu("0000:00:02.0", "Intel Graphics", "xe", true, &["eDP-1", "DP-1", "DP-2", "DP-3", "HDMI-A-1"]),
            gpu("0000:01:00.0", "NVIDIA RTX PRO 2000 Blackwell Generation Laptop GPU", "nvidia", false, &["HDMI-A-2"]),
        ]);
        return;
    }
    let into = st.gpus.clone();
    std::thread::spawn(move || {
        let gpus = Client::connect().ok().and_then(|mut c| match c.request(&Request::Gpus) {
            Ok(Reply::Gpus { gpus }) => Some(gpus),
            _ => None,
        });
        if gpus.is_some() {
            *into.lock().unwrap() = gpus;
            proxy.send(crate::Msg::Redraw);
        }
        drop(hold);
    });
}

/// The panel's state in its view.
fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::Display(st) => Some(st),
        _ => None,
    }
}

/// How long a new resolution has to be kept before it reverts.
pub const KEEP_SECS: u64 = 15;

/// A logical rectangle (x, y, w, h).
type LRect = (f64, f64, f64, f64);

fn overlaps(a: LRect, b: LRect) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}

/// Where a screen of size (w, h) dropped at (x, y) goes: against the
/// nearest edge of another screen, sharing at least a strip of that edge,
/// lined up with its edges or centre when close, and never overlapping.
pub fn snap((x, y, w, h): LRect, others: &[LRect]) -> Option<(i32, i32)> {
    const SHARE: f64 = 64.0;
    let align = 0.06 * w.max(h);
    let near = |v: f64, targets: [f64; 3]| targets.into_iter().filter(|t| (v - t).abs() < align).min_by(|a, b| (v - a).abs().total_cmp(&(v - b).abs())).unwrap_or(v);
    let mut best: Option<((f64, f64), f64)> = None;
    for &(ox, oy, ow, oh) in others {
        let yy = near(y.clamp(oy - h + SHARE, oy + oh - SHARE), [oy, oy + oh - h, oy + (oh - h) / 2.0]);
        let xx = near(x.clamp(ox - w + SHARE, ox + ow - SHARE), [ox, ox + ow - w, ox + (ow - w) / 2.0]);
        for cand in [(ox + ow, yy), (ox - w, yy), (xx, oy + oh), (xx, oy - h)] {
            if others.iter().any(|&o| overlaps((cand.0, cand.1, w, h), o)) {
                continue;
            }
            let d = (cand.0 - x).hypot(cand.1 - y);
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((cand, d));
            }
        }
    }
    best.map(|((x, y), _)| (x.round() as i32, y.round() as i32))
}

pub const SCALES: &[f64] = &[1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0];

fn mode_text(mode: &Mode) -> String {
    format!("{} × {}", mode.width, mode.height)
}

fn hz_text(refresh: i32) -> String {
    let hz = refresh as f64 / 1000.0;
    if (hz - hz.round()).abs() < 0.05 { format!("{hz:.0} Hz") } else { format!("{hz:.2} Hz") }
}

fn scale_text(scale: f64) -> String {
    let pct = (scale * 100.0).round() as i32;
    if (scale - 2.0).abs() < 1e-6 { "200% (Retina)".into() } else { format!("{pct}%") }
}

/// Logical size (what apps lay out in) of an output at its mode, scale
/// and rotation.
pub(crate) fn logical(o: &OutputInfo) -> (f64, f64) {
    let (w, h) = o.current_mode.as_ref().map_or((1920, 1080), |m| (m.width, m.height));
    let (w, h) = if o.rotation % 180 == 90 { (h, w) } else { (w, h) };
    (w as f64 / o.scale.max(0.1), h as f64 / o.scale.max(0.1))
}

const ROTATION_NAMES: [&str; 4] = ["Standard", "90°", "180°", "270°"];

/// The name shown for `o`, with its connector when another screen would
/// have the same name.
/// A display's name, with the number it goes by among several ("D1
/// Studio Display", as Deskbar's workspace headings say "D1 Workspace
/// 2"); a mirror, which has none, with its connector if another is called
/// the same.
fn shown_name(o: &OutputInfo, all: &[OutputInfo]) -> String {
    let name = display_name(o);
    if let Some(n) = olo_protocol::display_number(all, &o.name) {
        return format!("{} {name}", olo_protocol::display_label(n));
    }
    if all.iter().filter(|x| display_name(x) == name).count() > 1 { format!("{name} ({})", o.name) } else { name }
}

fn display_name(o: &OutputInfo) -> String {
    o.display_name()
}

/// The panel: its window grows to a display's rows (its brightness, its
/// panel's notes), scrolling only past the screen's room (`panel::Fit`).
pub fn ui(ui: &mut Ui, outputs: &[OutputInfo], power: Option<&PowerStatus>, server: &Server, st: &mut State) {
    panel(ui, outputs, power, server, st);
}

/// A pop-up menu that does something other than change the settings.
fn menu(ui: &mut Ui, id: String, names: Vec<String>, selected: usize, label: &str, divider: f32, on: impl Fn(usize) + 'static) {
    let names = names.into_iter().map(SharedString::from).collect();
    let f = MenuField::new(id, names, selected).label(SharedString::from(label.to_owned())).divider(divider).enabled(ui.enabled()).on_change(move |i, _, _| on(i));
    ui.add(f);
}

fn panel(ui: &mut Ui, outputs: &[OutputInfo], power: Option<&PowerStatus>, server: &Server, st: &mut State) {
    if outputs.is_empty() {
        ui.add(EmptyState::new("No displays reported by app_server.").icon("display"));
        return;
    }
    if st.selected.as_ref().is_none_or(|n| !outputs.iter().any(|o| &o.name == n)) {
        st.selected = Some(outputs[0].name.clone());
    }
    let all = outputs;
    map(ui, all, server, st);
    for mirror in all.iter().filter(|o| o.mirror_of.is_some()) {
        let source = all.iter().find(|x| Some(&x.name) == mirror.mirror_of.as_ref()).map(|x| shown_name(x, all)).unwrap_or_default();
        let shown = st.selected.as_deref() == Some(&mirror.name);
        let name = mirror.name.clone();
        let pick = ui.on_click_view(move |v, _, cx| {
            if let Some(st) = state_of(v) {
                st.selected = Some(name.clone());
                cx.notify();
            }
        });
        ui.row(|ui| {
            ui.note(format!("{} mirrors {source}.", shown_name(mirror, all)));
            if !shown {
                ui.button("Settings", pick);
            }
        });
    }

    let Some(o) = outputs.iter().find(|o| st.selected.as_deref() == Some(&o.name)).cloned() else {
        return;
    };
    let label_w = 90.0;
    let title = shown_name(&o, outputs);
    // The display's settings, and beside them its panel: what it is and
    // how Olo draws for it (side by side, so a laptop's short screen has
    // room for both).
    let settings = ui.collect(|ui| ui.bbox(&title, |ui| display_box(ui, &o, outputs, power, server, st, label_w)));
    // The panel, and beside it in a tab its colours (when it says what
    // they are): the column as tall as one of them.
    let has_color = o.color.as_ref().is_some_and(|c| c.primaries().is_some());
    let facts = if has_color {
        let tab = st.side_tab.min(1);
        let items = ui.collect(|ui| if tab == 0 { panel_box(ui, &o, label_w, server) } else { color_box(ui, &o, label_w, server, st) });
        let pick = ui.on_view(|v, i: usize, _, cx| {
            if let Some(st) = state_of(v) {
                st.side_tab = i;
            }
            cx.notify();
        });
        let tabs: Vec<SharedString> = ["Panel", "Colour"].into_iter().map(SharedString::from).collect();
        vec![
            TabView::new(format!("display-side-{}", o.name), tabs, tab)
                .grows()
                .on_select(pick)
                .child(div().flex_1().flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING)).children(items))
                .into_any_element(),
        ]
    } else {
        ui.collect(|ui| ui.bbox("Panel", |ui| panel_box(ui, &o, label_w, server)))
    };
    let column = |items: Vec<olo_ui::gpui::AnyElement>| div().flex_1().min_w_0().flex().flex_col().children(items);
    ui.add(div().flex().flex_row().items_start().gap(px(m::SPACING)).child(column(settings)).child(column(facts)));
    keep_band(ui, outputs, server, st);
    graphics(ui, outputs, st);
}

/// Arrangement: every output to scale, in its logical position; a click
/// picks one, a drag moves it (snapped against the others when let go).
fn map(ui: &mut Ui, all: &[OutputInfo], server: &Server, st: &State) {
    // Mirrors show another screen's picture: they aren't in the arrangement.
    let placed: Vec<OutputInfo> = all.iter().filter(|o| o.mirror_of.is_none()).cloned().collect();
    let names: Vec<String> = placed.iter().map(|o| shown_name(o, all)).collect();
    let selected = st.selected.clone();
    let drag = st.drag.clone();
    let colour = ui.s.workspace_color(0);
    let view = ui.cx.entity().downgrade();
    let server = server.clone();
    let paint = move |map: Bounds<Pixels>, window: &mut olo_ui::gpui::Window, cx: &mut olo_ui::gpui::App| {
        let p = theme::palette(cx);
        style::fill_rect(window, map, c(p.document_background));
        style::sunken(window, map, &p, false);
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for o in &placed {
            let (w, h) = logical(o);
            x0 = x0.min(o.position.0 as f64);
            y0 = y0.min(o.position.1 as f64);
            x1 = x1.max(o.position.0 as f64 + w);
            y1 = y1.max(o.position.1 as f64 + h);
        }
        let inner = map.dilate(px(-16.0));
        let k = (f32::from(inner.size.width) as f64 / (x1 - x0)).min(f32::from(inner.size.height) as f64 / (y1 - y0)) as f32;
        let (cw, ch) = ((x1 - x0) as f32 * k, (y1 - y0) as f32 * k);
        let off = point(inner.center().x - px(cw / 2.0), inner.center().y - px(ch / 2.0));
        let rect_of = |o: &OutputInfo| {
            let (w, h) = logical(o);
            let b = Bounds { origin: point(off.x + px((o.position.0 as f64 - x0) as f32 * k), off.y + px((o.position.1 as f64 - y0) as f32 * k)), size: size(px(w as f32 * k), px(h as f32 * k)) };
            // (Displays side by side kept `TIGHT` apart: told apart at a
            // glance.)
            b.dilate(px(-m::TIGHT / 2.0))
        };
        // The dragged screen is drawn last, over the others.
        let mut order: Vec<usize> = (0..placed.len()).collect();
        order.sort_by_key(|&i| drag.as_ref().is_some_and(|(n, ..)| *n == placed[i].name));
        let mut hits = Vec::new();
        for i in order {
            let o = &placed[i];
            let mut r = style::snap(window, rect_of(o));
            hits.push((o.name.clone(), r));
            if let Some((n, from, to)) = &drag
                && *n == o.name
            {
                let moved = Bounds { origin: point(r.origin.x + (to.x - from.x), r.origin.y + (to.y - from.y)), size: r.size };
                r = style::snap(window, moved.intersect(&map.dilate(px(-2.0))));
            }
            let chosen = selected.as_deref() == Some(&o.name);
            let [rr, gg, bb] = colour;
            style::fill_rect(window, r, rgb((u32::from(rr) << 16) | (u32::from(gg) << 8) | u32::from(bb)).into());
            let border = if chosen { c(p.keyboard_navigation) } else { c(p.frame_focused[0]) };
            style::frame_rect(window, r, border);
            if chosen {
                style::frame_rect(window, style::inset(window, r, 1.0), border);
            }
            // A strip along the top for Deskbar: every display has its own.
            let line = style::hairline(window) * if chosen { 2.0 } else { 1.0 };
            let bar = Bounds { origin: point(r.left() + line, r.top() + line), size: size(r.size.width - line * 2.0, px(4.0)) };
            style::fill_rect(window, bar, c(p.panel_background));
            // (Cut short to its screen, which may be small beside a big one.)
            let room = (r.size.width - px(2.0 * m::TIGHT)).max(px(0.0));
            style::label_fit(window, cx, r.center().x, r.center().y, style::Align::Center, names[i].clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), c(p.desktop_text), room);
            olo_ui::agent::record(
                window,
                cx,
                olo_ui::agent::Node { role: "Button", name: Some(names[i].clone().into()), bounds: r, enabled: true, clickable: true, selected: Some(chosen), ..Default::default() },
            );
        }
        let draggable = placed.len() > 1;
        let (down, moved, up) = (view.clone(), view.clone(), view.clone());
        window.on_mouse_event(move |e: &MouseDownEvent, phase, _, cx| {
            if phase != DispatchPhase::Bubble || e.button != MouseButton::Left || !map.contains(&e.position) {
                return;
            }
            let Some((name, _)) = hits.iter().rev().find(|(_, r)| r.contains(&e.position)).cloned() else { return };
            let _ = down.update(cx, |v, cx| {
                if let Some(st) = state_of(v) {
                    st.selected = Some(name.clone());
                    if draggable {
                        st.drag = Some((name, e.position, e.position));
                    }
                    cx.notify();
                }
            });
        });
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let _ = moved.update(cx, |v, cx| {
                if let Some(st) = state_of(v)
                    && let Some(d) = st.drag.as_mut()
                {
                    d.2 = e.position;
                    cx.notify();
                }
            });
        });
        let placed = placed.clone();
        let server = server.clone();
        window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
            if phase != DispatchPhase::Bubble || e.button != MouseButton::Left {
                return;
            }
            let _ = up.update(cx, |v, cx| {
                if let Some(st) = state_of(v)
                    && let Some((name, from, to)) = st.drag.take()
                {
                    let d = (f32::from(to.x - from.x), f32::from(to.y - from.y));
                    if d != (0.0, 0.0) {
                        rearrange(&placed, &name, d, k, &server);
                    }
                    cx.notify();
                }
            });
        });
    };
    ui.add(canvas(|_, _, _| {}, move |b, _, window, cx| paint(b, window, cx)).w_full().h(px(150.0)).flex_shrink_0());
}

/// The chosen display's resolution, refresh rate, scale, rotation, use,
/// main display and brightness.
fn display_box(ui: &mut Ui, o: &OutputInfo, outputs: &[OutputInfo], power: Option<&PowerStatus>, server: &Server, st: &mut State, label_w: f32) {
    // Resolutions, largest first; the refresh rate is a separate choice.
    let mut sizes: Vec<(i32, i32)> = o.modes.iter().map(|m| (m.width, m.height)).collect();
    sizes.sort_by(|a, b| (b.0 * b.1).cmp(&(a.0 * a.1)).then(b.0.cmp(&a.0)));
    sizes.dedup();
    let Some(current) = o.current_mode.clone().or_else(|| o.modes.first().cloned()) else {
        ui.note("This display reports no modes.");
        return;
    };
    let names: Vec<String> = sizes.iter().map(|&(w, h)| mode_text(&Mode { width: w, height: h, refresh: 0 })).collect();
    let i = sizes.iter().position(|&(w, h)| (w, h) == (current.width, current.height)).unwrap_or(0);
    // A new mode, to be kept or reverted (to what worked before, even
    // after several changes).
    let change = {
        let (name, current, server) = (o.name.clone(), current.clone(), server.clone());
        move |v: &mut PanelView, chosen: Mode| {
            if chosen == current {
                return;
            }
            if let Some(st) = state_of(v) {
                if st.confirm.as_ref().is_none_or(|(n, u, _)| *n != name || !matches!(u, Undo::Mode(_))) {
                    st.confirm = Some((name.clone(), Undo::Mode(current.clone()), Instant::now()));
                } else if let Some(c) = st.confirm.as_mut() {
                    c.2 = Instant::now();
                }
            }
            server.configure_output(&name, OutputConfig { mode: Some(chosen), ..Default::default() });
        }
    };
    let change = std::rc::Rc::new(change);
    {
        let (modes, sizes, current, change) = (o.modes.clone(), sizes.clone(), current.clone(), change.clone());
        let on = ui.on_view(move |v, i: usize, _, cx| {
            let (w, h) = sizes[i];
            // Keep the refresh rate if the new size offers it, else the fastest.
            let chosen = modes.iter().filter(|m| (m.width, m.height) == (w, h)).min_by_key(|m| ((m.refresh - current.refresh).abs(), -m.refresh)).cloned().unwrap_or(current.clone());
            change(v, chosen);
            cx.notify();
        });
        let names = names.into_iter().map(SharedString::from).collect();
        ui.add(MenuField::new(format!("res-{}", o.name), names, i).label("Resolution:").divider(label_w).on_change(on));
    }
    let mut rates: Vec<i32> = o.modes.iter().filter(|m| (m.width, m.height) == (current.width, current.height)).map(|m| m.refresh).collect();
    rates.sort_by(|a, b| b.cmp(a));
    rates.dedup();
    let rate_names: Vec<SharedString> = rates.iter().map(|&r| hz_text(r).into()).collect();
    let ri = rates.iter().position(|&r| r == current.refresh).unwrap_or(0);
    let several = rates.len() > 1;
    {
        let (rates, current, change) = (rates.clone(), current.clone(), change.clone());
        let on = ui.on_view(move |v, i: usize, _, cx| {
            change(v, Mode { refresh: rates[i], ..current.clone() });
            cx.notify();
        });
        ui.add(MenuField::new(format!("hz-{}", o.name), rate_names, ri).label("Refresh rate:").divider(label_w).enabled(several).on_change(on));
    }
    // A display that can vary its refresh rate (VRR): each frame shown as
    // it comes, within its range.
    if let Some(vrr) = o.vrr {
        let (name, to) = (o.name.clone(), server.clone());
        let check =
            CheckBox::new("Variable refresh rate", vrr).id(format!("vrr-{}", o.name)).on_toggle(move |on, _, _| to.configure_output(&name, OutputConfig { vrr: Some(on), ..Default::default() }));
        ui.add(div().pl(px(label_w)).child(check));
    }

    // A mirror shows its source as it is: no scale or rotation of its own.
    let si = SCALES.iter().position(|&s| (s - o.scale).abs() < 0.01).unwrap_or(4);
    if o.mirror_of.is_none() {
        let (name, to) = (o.name.clone(), server.clone());
        menu(ui, format!("scale-{}", o.name), SCALES.iter().map(|&s| scale_text(s)).collect(), si, "Scale:", label_w, move |i| {
            to.configure_output(&name, OutputConfig { scale: Some(SCALES[i]), ..Default::default() });
        });
        let ri = olo_protocol::ROTATIONS.iter().position(|&r| r == o.rotation).unwrap_or(0);
        let (name, to) = (o.name.clone(), server.clone());
        menu(ui, format!("rotation-{}", o.name), ROTATION_NAMES.iter().map(|s| s.to_string()).collect(), ri, "Rotation:", label_w, move |i| {
            to.configure_output(&name, OutputConfig { rotation: Some(olo_protocol::ROTATIONS[i]), ..Default::default() });
        });
    }
    // Extend the desktop, or show another display's picture.
    if outputs.len() > 1 {
        let sources: Vec<OutputInfo> = outputs.iter().filter(|x| x.name != o.name && x.mirror_of.is_none()).cloned().collect();
        let mut uses: Vec<String> = vec!["Extended desktop".into()];
        uses.extend(sources.iter().map(|x| format!("Mirror of {}", shown_name(x, outputs))));
        let now = o.mirror_of.as_ref().and_then(|m| sources.iter().position(|x| &x.name == m)).map_or(0, |i| i + 1);
        // A display others mirror can't mirror in turn.
        let mirrored = outputs.iter().any(|x| x.mirror_of.as_ref() == Some(&o.name));
        let (name, server) = (o.name.clone(), server.clone());
        ui.enabled_if(!mirrored, |ui| {
            menu(ui, format!("use-{}", o.name), uses, now, "Use as:", label_w, move |i| {
                let mirror = match i.checked_sub(1).and_then(|i| sources.get(i)) {
                    None => String::new(),
                    Some(x) if x.key.is_empty() => x.name.clone(),
                    Some(x) => x.key.clone(),
                };
                server.configure_output(&name, OutputConfig { mirror: Some(mirror), ..Default::default() });
            });
        });
    }
    if o.mirror_of.is_none() {
        let (cw, ch) = if o.rotation % 180 == 90 { (current.height, current.width) } else { (current.width, current.height) };
        let (lw, lh) = (cw as f64 / SCALES[si], ch as f64 / SCALES[si]);
        // Under the fields' text.
        ui.add(div().pl(px(label_w + m::TEXT_INSET)).child(StringView::new(format!("Looks like {lw:.0} × {lh:.0}")).dim(true)));
    }
    if outputs.len() > 1 && o.mirror_of.is_none() {
        let key = if o.key.is_empty() { o.name.clone() } else { o.key.clone() };
        let make_main = ui.edit(move |s, on: bool| {
            if on {
                s.display.main = Some(key.clone());
            }
        });
        let server = server.clone();
        // The note under the fields' text, the check box under the fields.
        let left = if o.main {
            div().pl(px(m::TEXT_INSET)).child(StringView::new("Main display: the desktop is here.").dim(true)).into_any_element()
        } else {
            CheckBox::new("Make this the main display", false).on_toggle(make_main).into_any_element()
        };
        ui.add(div().flex().flex_row().items_center().pl(px(label_w)).child(div().flex_1().child(left)).child(Button::new("Identify").on_click(move |_, _| server.identify_outputs())));
    }

    // Each display whose brightness app_server can set (the built-in
    // one's backlight, monitors controlled over USB).
    let builtin = power.and_then(|p| p.brightness).filter(|_| display_name(o) == "Built-in Display" || outputs.len() == 1);
    if let Some(level) = o.brightness.or(builtin) {
        if st.brightness.as_ref().is_some_and(|(_, _, at)| at.elapsed() > Duration::from_millis(600)) {
            st.brightness = None;
        }
        let v = st.brightness.as_ref().filter(|(n, ..)| *n == o.name).map_or(level, |(_, l, _)| *l);
        let (name, server) = (o.name.clone(), server.clone());
        let on = ui.on_view(move |view, v: f32, _, cx| {
            if let Some(st) = state_of(view) {
                st.brightness = Some((name.clone(), v, Instant::now()));
            }
            server.set_output_brightness(&name, v);
            cx.notify();
        });
        ui.add(Slider::new(format!("brightness-{}", o.name), v, 0.0..=1.0).step(1.0 / 64.0).label("Brightness").value_text(format!("{:.0}%", v * 100.0)).on_change(on));
    }
}

/// The display's panel: what it is, how Olo draws for it and why, and
/// the person's corrections (`olo_protocol::panels`). (Its rows: in a
/// box of their own, or a tab beside Colour's.)
fn panel_box(ui: &mut Ui, o: &OutputInfo, label_w: f32, server: &Server) {
    use olo_protocol::panels::{Source, Subpixel, Technology, TextMode};
    let Some(panel) = o.panel.clone() else { return };
    let from = |source: Source| match source {
        Source::Display => "the display says",
        Source::Guess => "Olo's guess",
        Source::Setting => "your choice",
    };
    (|ui: &mut Ui| {
        // A label in the fields' label column, its value under their text,
        // a control's height (so beside the display's settings, the rows
        // go in step).
        let row = |ui: &mut Ui, label: &str, value: &str| {
            ui.add(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .min_h(px(m::CONTROL_HEIGHT))
                    .child(div().w(px(label_w + m::TEXT_INSET)).flex_shrink_0().child(StringView::new(label.to_owned())))
                    .child(div().flex_1().min_w_0().child(StringView::new(value.to_owned()).wrap(true))),
            );
        };
        let maker = [o.make.as_str(), o.model.as_str()].iter().filter(|s| !s.is_empty() && **s != "Unknown").copied().collect::<Vec<_>>().join(" ");
        row(ui, "Make:", if maker.is_empty() { "Not told" } else { &maker });
        let (w, h) = o.physical_size_mm;
        let size = if w > 0 && h > 0 {
            let inches = ((w * w + h * h) as f64).sqrt() / 25.4;
            match panel.ppi {
                Some(ppi) => format!("{inches:.0}″ ({w} × {h} mm), {ppi:.0} pixels per inch"),
                None => format!("{inches:.0}″ ({w} × {h} mm)"),
            }
        } else {
            "Not told".into()
        };
        row(ui, "Size:", &size);
        row(ui, "Refresh:", &format!("{:.0} Hz", panel.refresh_hz));

        // Each correction: Automatic (Olo's, with what it is) or a choice.
        const TECHS: [Technology; 3] = [Technology::Lcd, Technology::MiniLed, Technology::Oled];
        let auto = |what: &str, why: &str| format!("Automatic: {what} ({why})");
        let mut names = vec![if panel.technology_from == Source::Setting { "Automatic".to_string() } else { auto(panel.technology.label(), from(panel.technology_from)) }];
        names.extend(TECHS.iter().map(|t| t.label().to_string()));
        let i = if panel.technology_from == Source::Setting { TECHS.iter().position(|t| *t == panel.technology).map_or(0, |i| i + 1) } else { 0 };
        let (name, server2) = (o.name.clone(), server.clone());
        menu(ui, format!("technology-{}", o.name), names, i, "Technology:", label_w, move |i| {
            let technology = Some(if i == 0 { Technology::Unknown } else { TECHS[i - 1] });
            server2.configure_output(&name, OutputConfig { technology, ..Default::default() });
        });

        const STRIPES: [Subpixel; 5] = [Subpixel::Rgb, Subpixel::Bgr, Subpixel::VerticalRgb, Subpixel::VerticalBgr, Subpixel::None];
        let mut names = vec![if panel.subpixel_from == Source::Setting { "Automatic".to_string() } else { auto(panel.subpixel.label(), from(panel.subpixel_from)) }];
        names.extend(STRIPES.iter().map(|s| s.label().to_string()));
        let i = if panel.subpixel_from == Source::Setting { STRIPES.iter().position(|s| *s == panel.subpixel).map_or(0, |i| i + 1) } else { 0 };
        let (name, server2) = (o.name.clone(), server.clone());
        menu(ui, format!("stripes-{}", o.name), names, i, "Stripes:", label_w, move |i| {
            let subpixel = Some(if i == 0 { Subpixel::Unknown } else { STRIPES[i - 1] });
            server2.configure_output(&name, OutputConfig { subpixel, ..Default::default() });
        });

        const MODES: [TextMode; 4] = [TextMode::Hinted, TextMode::Unhinted, TextMode::Subpixel(Subpixel::Rgb), TextMode::Subpixel(Subpixel::Bgr)];
        let label = |t: TextMode| match t {
            TextMode::Subpixel(Subpixel::Bgr) => "Subpixel, BGR order".to_string(),
            TextMode::Subpixel(_) => "Subpixel, RGB order".to_string(),
            t => t.label().to_string(),
        };
        let current = panel.text.unwrap_or(TextMode::Hinted);
        let mut names = vec![if panel.text_from == Source::Setting { "Automatic".to_string() } else { format!("Automatic: {}", label(current)) }];
        names.extend(MODES.iter().map(|t| label(*t)));
        let i = if panel.text_from == Source::Setting { MODES.iter().position(|t| *t == current).map_or(0, |i| i + 1) } else { 0 };
        let (name, server2) = (o.name.clone(), server.clone());
        menu(ui, format!("text-{}", o.name), names, i, "Text:", label_w, move |i| {
            let text = Some(if i == 0 { TextMode::Auto } else { MODES[i - 1] });
            server2.configure_output(&name, OutputConfig { text, ..Default::default() });
        });
        // Why, under the fields' text.
        ui.add(div().pl(px(label_w + m::TEXT_INSET)).child(StringView::new(panel.text_why.clone()).dim(true).wrap(true)));
    })(ui);
}

/// The display's colours: what it can show, how sRGB's colours are shown
/// on a wide one, and HDR on one that has it (app_server's `color`). (Its
/// rows, for the tab beside the panel's.)
fn color_box(ui: &mut Ui, o: &OutputInfo, label_w: f32, server: &Server, st: &mut State) {
    let Some(color) = o.color.clone() else { return };
    let Some(primaries) = color.primaries() else { return };
    (|ui: &mut Ui| {
        let row = |ui: &mut Ui, label: &str, value: String| {
            ui.add(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .min_h(px(m::CONTROL_HEIGHT))
                    .child(div().w(px(label_w + m::TEXT_INSET)).flex_shrink_0().child(StringView::new(label.to_owned())))
                    .child(div().flex_1().min_w_0().child(StringView::new(value).wrap(true))),
            );
        };
        let gamut = match color.gamut() {
            Some((name, c)) if c < 0.995 => format!("{name} ({:.0}%)", c * 100.0),
            Some((name, _)) => name.to_string(),
            None => format!("Narrower than sRGB ({:.0}% of it)", primaries.coverage(&olo_color::Primaries::SRGB) * 100.0),
        };
        row(ui, "Colours:", gamut);
        if let Some(peak) = color.peak_nits {
            let light = match color.full_nits.filter(|f| (f - peak).abs() > 1.0) {
                Some(full) => format!("{peak:.0} cd/m² at most, {full:.0} over the whole screen"),
                None => format!("{peak:.0} cd/m²"),
            };
            row(ui, "Light:", light);
        }
        // A wide display: sRGB's colours as meant, or stretched.
        if let Some(mode) = o.color_mode {
            let (name, to) = (o.name.clone(), server.clone());
            let i = usize::from(mode == ColorMode::Vivid);
            menu(ui, format!("colors-{}", o.name), vec!["Accurately".into(), "Vivid".into()], i, "Show sRGB:", label_w, move |i| {
                let color_mode = Some(if i == 1 { ColorMode::Vivid } else { ColorMode::Accurate });
                to.configure_output(&name, OutputConfig { color_mode, ..Default::default() });
            });
            let why = match mode {
                ColorMode::Accurate => "What's made for sRGB (nearly everything) looks as it was made; wider colours are for what has them.",
                ColorMode::Vivid => "sRGB's colours stretched over the panel's: brighter and more saturated than anything was made.",
            };
            ui.add(div().pl(px(label_w + m::TEXT_INSET)).child(StringView::new(why).dim(true).wrap(true)));
        }
        // HDR, kept only once seen to work (the band above reverts it).
        if let Some(hdr) = o.hdr {
            let (name, to) = (o.name.clone(), server.clone());
            let on = ui.on_view(move |v, on: bool, _, cx| {
                if on && let Some(st) = state_of(v) {
                    st.confirm = Some((name.clone(), Undo::Hdr, Instant::now()));
                }
                to.configure_output(&name, OutputConfig { hdr: Some(on), ..Default::default() });
                cx.notify();
            });
            ui.add(div().pl(px(label_w)).child(CheckBox::new("High dynamic range (HDR)", hdr).id(format!("hdr-{}", o.name)).on_toggle(on)));
            if hdr {
                let (name, to) = (o.name.clone(), server.clone());
                let nits = st.sdr_nits.as_ref().filter(|(n, _, at)| *n == o.name && at.elapsed() < Duration::from_millis(600)).map_or(o.sdr_nits, |(_, v, _)| *v);
                let set = ui.on_view(move |view, v: f32, _, cx| {
                    if let Some(st) = state_of(view) {
                        st.sdr_nits = Some((name.clone(), v, Instant::now()));
                    }
                    to.configure_output(&name, OutputConfig { sdr_nits: Some(v), ..Default::default() });
                    cx.notify();
                });
                ui.add(Slider::new(format!("sdr-{}", o.name), nits, 80.0..=480.0).step(1.0).label("Everything else's brightness").value_text(format!("{nits:.0} cd/m²")).on_change(set));
                let peak = color.peak_nits.unwrap_or(1000.0);
                ui.add(div().pl(px(m::TEXT_INSET)).child(StringView::new(format!("HDR video and pictures shine up to {peak:.0} cd/m²; everything else is shown this bright.")).dim(true).wrap(true)));
            }
        }
    })(ui);
}

/// Move `name` by `delta` points in the map (`k` points per logical
/// pixel), snapped against the others. Every screen's position is sent so
/// the arrangement doesn't depend on which ones had been placed before.
fn rearrange(outputs: &[OutputInfo], name: &str, delta: (f32, f32), k: f32, server: &Server) {
    let Some(o) = outputs.iter().find(|o| o.name == name) else { return };
    let (w, h) = logical(o);
    let x = o.position.0 as f64 + (delta.0 / k) as f64;
    let y = o.position.1 as f64 + (delta.1 / k) as f64;
    let others: Vec<LRect> = outputs
        .iter()
        .filter(|x| x.name != name)
        .map(|x| {
            let (w, h) = logical(x);
            (x.position.0 as f64, x.position.1 as f64, w, h)
        })
        .collect();
    let Some(at) = snap((x, y, w, h), &others) else { return };
    if at == o.position {
        return;
    }
    for x in outputs.iter().filter(|x| x.name != name) {
        server.configure_output(&x.name, OutputConfig { position: Some(x.position), ..Default::default() });
    }
    server.configure_output(name, OutputConfig { position: Some(at), ..Default::default() });
}

/// "Keep this resolution?" (or HDR) with a countdown; reverts by itself.
fn keep_band(ui: &mut Ui, outputs: &[OutputInfo], server: &Server, st: &mut State) {
    let Some((name, undo, at)) = st.confirm.clone() else { return };
    if !outputs.iter().any(|o| o.name == name) {
        st.confirm = None;
        return;
    }
    let question = match undo {
        Undo::Mode(_) => "Keep this resolution?",
        Undo::Hdr => "Keep HDR on?",
    };
    let revert = {
        let (name, server) = (name.clone(), server.clone());
        move || match &undo {
            Undo::Mode(previous) => server.configure_output(&name, OutputConfig { mode: Some(previous.clone()), ..Default::default() }),
            Undo::Hdr => server.configure_output(&name, OutputConfig { hdr: Some(false), ..Default::default() }),
        }
    };
    let left = KEEP_SECS.saturating_sub(at.elapsed().as_secs());
    if left == 0 {
        revert();
        st.confirm = None;
        return;
    }
    // Drawn again as the seconds go.
    ui.cx
        .spawn(async move |view, cx| {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            let _ = view.update(cx, |_, cx| cx.notify());
        })
        .detach();
    let keep = ui.on_click_view(|v, _, cx| {
        if let Some(st) = state_of(v) {
            st.confirm = None;
        }
        cx.notify();
    });
    let back = ui.on_click_view(move |v, _, cx| {
        revert();
        if let Some(st) = state_of(v) {
            st.confirm = None;
        }
        cx.notify();
    });
    ui.add(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            .pt(px(m::SPACING))
            .child(div().flex_1().child(StringView::new(format!("{question} Reverting in {left} s."))))
            .child(Button::new("Revert").on_click(back))
            .child(Button::new("Keep").default(true).on_click(keep)),
    );
}

/// With more than one GPU: what each drives (each display is composed by
/// the GPU it's wired to; app_server's `render_surface`).
fn graphics(ui: &mut Ui, outputs: &[OutputInfo], st: &mut State) {
    if !st.asked_gpus {
        st.asked_gpus = true;
        let hold = olo_ui::hold_first_show(ui.window);
        ask_gpus(st, olo_ui::proxy(ui.cx), hold);
    }
    let Some(gpus) = st.gpus.lock().unwrap().clone() else { return };
    if gpus.len() < 2 {
        return;
    }
    let label_w = 90.0;
    ui.bbox("Graphics", |ui| {
        for g in &gpus {
            // The displays connected to it now, by name; else its ports.
            // (Label and value laid out alike, as paragraphs: their first
            // lines level however the value wraps.)
            let shown: Vec<String> = outputs.iter().filter(|o| g.connectors.contains(&o.name)).map(|o| shown_name(o, outputs)).collect();
            let what = if shown.is_empty() { format!("nothing now (its ports: {})", if g.connectors.is_empty() { "none".into() } else { g.connectors.join(", ") }) } else { shown.join(", ") };
            ui.add(
                div()
                    .flex()
                    .flex_row()
                    .child(div().w(px(label_w + m::TEXT_INSET)).flex_shrink_0().child(StringView::new(format!("{}:", if g.boot { "Built in" } else { "Second" })).wrap(true)))
                    .child(div().flex_1().min_w_0().child(StringView::new(format!("{} ({} driver) draws {what}", g.name, g.driver)).wrap(true))),
            );
        }
        ui.add(
            div()
                .pl(px(label_w + m::TEXT_INSET))
                .child(StringView::new("Each display is drawn by the graphics it's wired to, so no frame crosses between them; the second sleeps while nothing uses it.").dim(true).wrap(true)),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_edge_to_edge() {
        let laptop = (0.0, 0.0, 1728.0, 1117.0);
        // Dropped a little above and right of the laptop's right edge: snaps against it.
        assert_eq!(snap((1750.0, -30.0, 1920.0, 1080.0), &[laptop]), Some((1728, 0)));
        // Dropped above, roughly centred: centred exactly on top.
        assert_eq!(snap((-80.0, -1050.0, 1920.0, 1080.0), &[laptop]), Some((-96, -1080)));
        // Dropped on top of it: moved to the nearest free edge.
        let (x, y) = snap((100.0, 900.0, 1920.0, 1080.0), &[laptop]).unwrap();
        assert!(!overlaps((x as f64, y as f64, 1920.0, 1080.0), laptop));
        assert_eq!(y, 1117);
        // Far away: still touching, sharing at least a strip of edge.
        assert_eq!(snap((1728.0, 5000.0, 1920.0, 1080.0), &[laptop]), Some((1728 - 64, 1117)));
    }
}
