//! Screen Saver: R5's two tabs. Fade: when the saver starts, when the
//! screen turns off and locks, and the corners that start it at once or
//! hold it off. Modules: the R5 modules with a live preview in a little
//! monitor, their settings, and Test.

use std::{cell::RefCell, rc::Rc, time::Instant};

use olo_kit::{
    Bounds, Cx, Element, FontWeight, InteractiveElement as _, MouseButton, MouseDownEvent, ParentElement as _, PathBuilder, Pixels, ScrollHandle, SharedString, Styled as _, WindowCx, canvas, div,
    metrics as m, point, px, rgb, size,
    style::{self, c},
    theme,
    widgets::{BBox, Button, CheckBox, ScrollView, TabView, separator},
};
use olo_protocol::{Corner, Settings};
use olo_screensavers::{Frame, MODULES, Module, Opt, Options, kit_canvas::KitCanvas};

use crate::{PanelView, form::Ui, panel, server::Server};

/// The module drawing in the preview, with what it was made from.
type Preview = Rc<RefCell<Option<(String, Options, Box<dyn Module>, Instant)>>>;

pub struct State {
    tab: usize,
    preview: Preview,
    /// What "Turn off screen" goes back to when ticked again.
    last_display_off: Option<u32>,
    scroll: ScrollHandle,
}

impl State {
    pub fn new(_: &mut Cx<PanelView>) -> State {
        State { tab: 0, preview: Rc::default(), last_display_off: None, scroll: ScrollHandle::new() }
    }
}

fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::ScreenSaver(st) => Some(st),
        _ => None,
    }
}

/// The minutes the sliders step through.
const MINUTES: &[u32] = &[1, 2, 3, 4, 5, 10, 15, 20, 30, 45, 60, 90, 120, 180];

fn minutes_text(m: u32) -> String {
    match m {
        1 => "1 minute".into(),
        60 => "1 hour".into(),
        m if m >= 60 && m % 60 == 0 => format!("{} hours", m / 60),
        m if m > 60 => format!("{} h {} min", m / 60, m % 60),
        m => format!("{m} minutes"),
    }
}

/// A slider over [`MINUTES`] (the nearest step to `value`).
fn minutes_slider(ui: &mut Ui, id: &str, label: &str, value: u32, set: impl Fn(&mut Settings, u32) + 'static) {
    let i = MINUTES.iter().position(|&m| m >= value).unwrap_or(MINUTES.len() - 1);
    let text = minutes_text(MINUTES[i]);
    let label = SharedString::from(label.to_owned());
    ui.slider(id, i as f32, 0.0..=(MINUTES.len() - 1) as f32, |s| s.step(1.0).label(label).value_text(text), move |s, v| set(s, MINUTES[(v.round() as usize).min(MINUTES.len() - 1)]));
}

pub fn ui(ui: &mut Ui, server: &Server, st: &mut State) {
    let items = ui.collect(|ui| match st.tab {
        0 => fade(ui),
        _ => modules(ui, server, st),
    });
    let pick = ui.on_view(|v, i: &usize, cx| {
        if let Some(st) = state_of(v) {
            st.tab = *i;
        }
        cx.notify();
    });
    let tabs: Vec<SharedString> = ["Fade", "Modules"].into_iter().map(SharedString::from).collect();
    let content = if st.tab == 0 { div().flex_1().flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING)) } else { div().flex_1().flex().flex_row().gap(px(m::SPACING * 2.0)).p(px(m::SPACING)) };
    ui.add(div().flex_1().flex().flex_col().child(TabView::new("screensaver-tabs", tabs, st.tab).grows().on_select(pick).child(content.children(items))));
}

fn fade(ui: &mut Ui) {
    let s = ui.s.clone();
    let ss = s.screen_saver.clone();
    ui.check("Enable Screen Saver", ss.enabled, |s, v| s.screen_saver.enabled = v);
    ui.enabled_if(ss.enabled, |ui| minutes_slider(ui, "run-minutes", "Run module", ss.run_minutes, |s, v| s.screen_saver.run_minutes = v));
    // "Turn off screen" is Power's display-off time (0: never).
    let off = s.power.display_off_minutes > 0;
    let toggle = ui.on_view(move |v, on: &bool, cx| {
        let Some(st) = state_of(v) else { return };
        let back = st.last_display_off.unwrap_or(10);
        let mut remember = None;
        let on = *on;
        crate::prefs().edit_as("", "Screen Saver", |s| {
            if on {
                s.power.display_off_minutes = back;
            } else {
                remember = Some(s.power.display_off_minutes);
                s.power.display_off_minutes = 0;
            }
        });
        if let (Some(st), Some(m)) = (state_of(v), remember) {
            st.last_display_off = Some(m);
        }
        cx.notify();
    });
    ui.add(CheckBox::new("Turn off screen", off).on_toggle(toggle));
    ui.enabled_if(off, |ui| minutes_slider(ui, "off-minutes", "Turn off after", s.power.display_off_minutes.max(1), |s, v| s.power.display_off_minutes = v));
    ui.enabled_if(ss.enabled, |ui| {
        ui.check("Password lock", ss.lock, |s, v| s.screen_saver.lock = v);
        ui.enabled_if(ss.lock, |ui| minutes_slider(ui, "lock-minutes", "Lock after", ss.lock_minutes, |s, v| s.screen_saver.lock_minutes = v));
    });
    ui.add(separator());
    let desktop = s.workspace_color(0);
    ui.enabled_if(ss.enabled, |ui| {
        let now = corner_monitor(ui, ss.fade_now, "Fade now when\nmouse is here", desktop, |s, k| {
            s.screen_saver.fade_now = k;
            if k.is_some() && s.screen_saver.never_fade == k {
                s.screen_saver.never_fade = None;
            }
        });
        let never = corner_monitor(ui, ss.never_fade, "Don't fade when\nmouse is here", desktop, |s, k| {
            s.screen_saver.never_fade = k;
            if k.is_some() && s.screen_saver.fade_now == k {
                s.screen_saver.fade_now = None;
            }
        });
        ui.add(div().flex().flex_row().gap(px(m::SPACING * 3.0)).child(now).child(never));
    });
}

/// A small monitor, its screen in the desktop's colour: click a corner of
/// its screen to choose it, the middle for none (shown, as on R5, by a
/// "no" sign); its label beside it.
fn corner_monitor(ui: &mut Ui, corner: Option<Corner>, label: &str, desktop: [u8; 3], set: impl Fn(&mut Settings, Option<Corner>) + 'static) -> Element {
    let enabled = ui.enabled();
    let pick: Rc<dyn Fn(&Option<Corner>, &mut WindowCx)> = Rc::new(ui.bind(corner, move |s, k: Option<Corner>| set(s, k)));
    let monitor = canvas(move |g, rect: Bounds<Pixels>| {
        let p = theme::palette();
        let body = Bounds { origin: rect.origin, size: size(px(56.0), px(42.0)) };
        // The case in the panel's colours, bevelled as a raised control.
        g.paint_quad(olo_kit::fill(body, c(p.control_background)).corner_radii(px(2.0)));
        style::bevel(g, body, c(p.shine), style::shade(&p, p.control_background, 1.45));
        // The case's edge a line outside its bevel, on device pixels.
        let edge = style::inset(g, body, -1.0);
        style::frame_rect(g, edge, c(p.control_border));
        let screen = body.dilate(px(-4.0));
        let [red, green, blue] = desktop;
        style::fill_rect(g, screen, rgb((u32::from(red) << 16) | (u32::from(green) << 8) | u32::from(blue)).into());
        style::frame_rect(g, screen, c(p.frame_focused[0]));
        // The stand hangs from the case's edge, on device pixels (it
        // was a pixel off it at 1.25×).
        let stand = style::snap(g, Bounds::from_corners(point(body.center().x - px(12.0), edge.bottom()), point(body.center().x + px(12.0), edge.bottom() + px(4.0))));
        g.paint_quad(olo_kit::fill(stand, style::shade(&p, p.control_background, 1.2)).corner_radii(px(1.0)));
        style::frame_rect(g, stand, c(p.control_border));
        match corner {
            Some(k) => {
                let (x, y) = match k {
                    Corner::TopLeft => (screen.left(), screen.top()),
                    Corner::TopRight => (screen.right(), screen.top()),
                    Corner::BottomLeft => (screen.left(), screen.bottom()),
                    Corner::BottomRight => (screen.right(), screen.bottom()),
                };
                let dx = if x == screen.left() { 1.0 } else { -1.0 };
                let dy = if y == screen.top() { 1.0 } else { -1.0 };
                let mut tri = PathBuilder::fill();
                tri.move_to(point(x, y));
                tri.line_to(point(x + px(14.0 * dx), y));
                tri.line_to(point(x, y + px(14.0 * dy)));
                tri.close();
                if let Ok(path) = tri.build() {
                    g.paint_path(path, c(p.tab_focused));
                }
            }
            None => {
                let red = if enabled { c(p.failure) } else { c(olo_kit::palette::mix(p.failure, p.panel_background, 0.5)) };
                let ctr = screen.center();
                g.paint_quad(olo_kit::outline(Bounds::centered_at(ctr, size(px(16.0), px(16.0))), red, olo_kit::BorderStyle::Solid).border_widths(px(2.0)).corner_radii(px(8.0)));
                let d = 8.0 * std::f32::consts::FRAC_1_SQRT_2;
                let mut slash = PathBuilder::stroke(px(2.0));
                slash.move_to(point(ctr.x - px(d), ctr.y - px(d)));
                slash.line_to(point(ctr.x + px(d), ctr.y + px(d)));
                if let Ok(path) = slash.build() {
                    g.paint_path(path, red);
                }
            }
        }
    })
    .w(px(56.0))
    .h(px(50.0))
    .flex_shrink_0()
    // (Its case and screen where it was drawn: the listener's bounds.)
    .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, cx| {
        let rect = cx.listener_bounds();
        let body = Bounds { origin: rect.origin, size: size(px(56.0), px(42.0)) };
        let screen = body.dilate(px(-4.0));
        if !enabled || !body.contains(&e.position) {
            return;
        }
        let f = |v: Pixels, lo: Pixels, w: Pixels| f32::from(v - lo) / f32::from(w);
        let (fx, fy) = (f(e.position.x, screen.left(), screen.size.width), f(e.position.y, screen.top(), screen.size.height));
        let side = |f: f32| {
            if f < 0.34 {
                Some(false)
            } else if f > 0.66 {
                Some(true)
            } else {
                None
            }
        };
        let k = match (side(fx), side(fy)) {
            (Some(false), Some(false)) => Some(Corner::TopLeft),
            (Some(true), Some(false)) => Some(Corner::TopRight),
            (Some(false), Some(true)) => Some(Corner::BottomLeft),
            (Some(true), Some(true)) => Some(Corner::BottomRight),
            _ => None,
        };
        pick(&k, cx);
    });
    let p = theme::palette();
    let ink = if enabled { c(p.panel_text) } else { style::disabled_text(&p) };
    let text = div().flex().flex_col().justify_center().line_height(olo_kit::relative(style::LINE_HEIGHT)).text_color(ink).children(label.lines().map(|l| l.to_owned()));
    div().flex().flex_row().items_center().gap(px(m::SPACING)).child(monitor).child(text)
}

const LIST_W: f32 = 150.0;

fn modules(ui: &mut Ui, server: &Server, st: &mut State) {
    let ss = ui.s.screen_saver.clone();
    let current = MODULES.iter().position(|m| m.name == ss.module).unwrap_or(0);
    let info = &MODULES[current];
    let options = ss.options.get(info.name).cloned().unwrap_or_default();

    // The preview monitor: its screen draws the module, a frame each time
    // the display shows one, while the tab is open.
    let stale = st.preview.borrow().as_ref().is_none_or(|(n, o, _, _)| n != info.name || *o != options);
    if stale {
        *st.preview.borrow_mut() = Some((info.name.to_owned(), options.clone(), info.make(&options, olo_screensavers::seed()), Instant::now()));
    }
    let preview = st.preview.clone();
    let monitor = canvas(move |g, b| {
        let p = theme::palette();
        let screen = Bounds { origin: point(b.left() + px(10.0), b.top() + px(8.0)), size: size(px(LIST_W - 20.0), px(80.0)) };
        let bezel = Bounds::from_corners(b.origin, point(b.right(), screen.bottom() + px(12.0)));
        let case = olo_kit::palette::tint(p.control_background, 1.25);
        g.paint_quad(olo_kit::fill(bezel, c(case)).corner_radii(px(4.0)));
        style::bevel(g, bezel, c(p.shine), style::shade(&p, case, 1.4));
        let edge = style::inset(g, bezel, -1.0);
        style::frame_rect(g, edge, c(p.control_border));
        // The screen itself: off, as a screen saver's starts.
        style::fill_rect(g, screen, olo_kit::black());
        for (i, col) in [c(p.success), style::shade(&p, case, 1.6)].into_iter().enumerate() {
            let at = point(bezel.right() - px(22.0 - i as f32 * 7.0), bezel.bottom() - px(6.0));
            g.paint_quad(olo_kit::fill(Bounds::centered_at(at, size(px(3.2), px(3.2))), col).corner_radii(px(1.6)));
        }
        if let Some((_, _, module, last)) = preview.borrow_mut().as_mut() {
            let now = Instant::now();
            let dt = (now - *last).as_secs_f32();
            *last = now;
            let rect = olo_screensavers::Rect::from_min_max(
                olo_screensavers::pos2(f32::from(screen.left()), f32::from(screen.top())),
                olo_screensavers::pos2(f32::from(screen.right()), f32::from(screen.bottom())),
            );
            let canvas = KitCanvas::new(g, rect, None);
            module.draw(&Frame { canvas: &canvas, rect, dt });
            canvas.finish();
            g.paint_again_within(1.0 / 120.0);
        }
    })
    .w(px(LIST_W))
    .h(px(110.0))
    .flex_shrink_0();

    // The module list.
    let p = theme::palette();
    let rows = MODULES.iter().enumerate().map(|(i, module)| {
        let chosen = i == current;
        let name = module.name;
        let pick = ui.bind_click(chosen, move |s| s.screen_saver.module = name.to_owned());
        div()
            .id(SharedString::from(format!("module-{name}")))
            .h(px(m::LIST_ROW))
            .w_full()
            .child(
                canvas(move |g, r| {
                    let p = theme::palette();
                    if chosen {
                        style::fill_rect(g, r, c(p.list_selected_background));
                    }
                    let color = if chosen { c(p.list_selected_item_text) } else { c(p.document_text) };
                    style::label(g, r.left() + px(m::CELL_INSET), r.center().y, style::Align::Left, name, style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), color);
                    olo_kit::agent::record(
                        g,
                        olo_kit::agent::Node { role: "ListItem", name: Some(name.into()), bounds: r, enabled: true, clickable: true, selected: Some(chosen), ..Default::default() },
                    );
                })
                .size_full(),
            )
            .on_click(pick)
    });
    let list = div().flex_1().min_h_0().child(ScrollView::new(&st.scroll).background(p.document_background).child(div().flex().flex_col().children(rows.collect::<Vec<_>>())));
    let server = server.clone();
    let left = div()
        .w(px(LIST_W))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(m::SPACING))
        .child(monitor)
        .child(list)
        .child(Button::new("Test").min_width(m::BUTTON_MIN_WIDTH).on_click(move |_, _| server.start_screen_saver()));
    ui.add(left);

    // Its settings.
    let settings = ui.collect(|ui| {
        ui.label(info.about);
        for opt in info.options {
            option_ui(ui, info.name, opt, &options);
        }
        if info.uses_desktop {
            ui.note("Works on a picture of the desktop.");
        }
    });
    ui.add(div().flex_1().child(BBox::new("Module settings").children(settings)));
}

/// Module `module`'s option `key` set to `value` (an untouched module
/// leaves no trace).
fn set_option(s: &mut Settings, module: &str, key: &str, value: String) {
    s.screen_saver.options.entry(module.to_owned()).or_default().insert(key.into(), value);
}

fn option_ui(ui: &mut Ui, module: &'static str, opt: &Opt, opts: &Options) {
    match *opt {
        Opt::Check { key, label, default } => {
            let v = olo_screensavers::get_bool(opts, key, default);
            ui.check(label, v, move |s, v| set_option(s, module, key, v.to_string()));
        }
        Opt::Choice { key, label, choices, default } => {
            let i = olo_screensavers::get_usize(opts, key, default);
            let names: Vec<SharedString> = choices.iter().map(|c| SharedString::from(*c)).collect();
            ui.menu(&format!("{module}-{key}"), names, i, |f| f.label(label).divider(90.0), move |s, i| set_option(s, module, key, i.to_string()));
        }
        Opt::Slider { key, label, min, max, default } => {
            let v = olo_screensavers::get_f32(opts, key, default);
            let whole = max - min >= 4.0;
            let text = if whole { format!("{v:.0}") } else { format!("{v:.2}") };
            let step = if whole { 1.0 } else { 0.05 };
            ui.slider(&format!("{module}-{key}"), v, min..=max, |s| s.step(step).label(label).value_text(text), move |s, v| set_option(s, module, key, v.to_string()));
        }
        Opt::Text { key, label, default } => {
            let v = olo_screensavers::get_text(opts, key, default).to_owned();
            ui.label(label);
            let id = format!("{module}-{key}");
            ui.field(&id, &v, |f| f, move |s, v| set_option(s, module, key, v));
            let control = ui.control_of(&id);
            ui.add(control);
        }
    }
}
