//! Screen Saver: R5's two tabs. Fade: when the saver starts, when the
//! screen turns off and locks, and the corners that start it at once or
//! hold it off. Modules: the R5 modules with a live preview in a little
//! monitor, their settings, and Test.

use std::time::Instant;

use tabula_kit::{
    egui::{Align2, Color32, Rect, Sense, Shape, Stroke, Ui, UiBuilder, pos2, vec2},
    metrics as m,
    style::{self, c},
    widgets::{BBox, Button, CheckBox, MenuField, ScrollView, Slider, StringView, TabView, TextControl},
};
use tabula_protocol::{Corner, Settings};
use tabula_screensavers::{Frame, MODULES, Module, Opt, Options};

use crate::server::Server;

#[derive(Default)]
pub struct State {
    tab: usize,
    /// The module drawing in the preview, with what it was made from.
    preview: Option<(String, Options, Box<dyn Module>, Instant)>,
    /// What "Turn off screen" goes back to when ticked again.
    last_display_off: Option<u32>,
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
fn minutes_slider(ui: &mut Ui, label: &str, value: &mut u32) {
    let mut i = MINUTES.iter().position(|m| m >= value).unwrap_or(MINUTES.len() - 1) as f32;
    let text = minutes_text(MINUTES[i as usize]);
    ui.add(Slider::new(&mut i, 0.0..=(MINUTES.len() - 1) as f32).step(1.0).label(label).value_text(text));
    *value = MINUTES[(i.round() as usize).min(MINUTES.len() - 1)];
}

pub fn ui(ui: &mut Ui, s: &mut Settings, server: &Server, st: &mut State) {
    let mut tab = st.tab;
    TabView::new(&mut tab, &["Fade", "Modules"]).show(ui, |ui, tab| match tab {
        0 => fade(ui, s, st),
        _ => modules(ui, s, server, st),
    });
    st.tab = tab;
}

fn fade(ui: &mut Ui, s: &mut Settings, st: &mut State) {
    ui.spacing_mut().item_spacing.y = m::SPACING;
    ui.add(CheckBox::new(&mut s.screen_saver.enabled, "Enable Screen Saver"));
    let enabled = s.screen_saver.enabled;
    let ss = &mut s.screen_saver;
    ui.add_enabled_ui(enabled, |ui| {
        minutes_slider(ui, "Run module", &mut ss.run_minutes);
    });
    // "Turn off screen" is Power's display-off time (0: never).
    let mut off = s.power.display_off_minutes > 0;
    let before = off;
    ui.add(CheckBox::new(&mut off, "Turn off screen"));
    if off != before {
        if off {
            s.power.display_off_minutes = st.last_display_off.unwrap_or(10);
        } else {
            st.last_display_off = Some(s.power.display_off_minutes);
            s.power.display_off_minutes = 0;
        }
    }
    ui.add_enabled_ui(off, |ui| {
        let mut v = s.power.display_off_minutes.max(1);
        minutes_slider(ui, "Turn off after", &mut v);
        if off {
            s.power.display_off_minutes = v;
        }
    });
    let ss = &mut s.screen_saver;
    ui.add_enabled_ui(enabled, |ui| {
        ui.add(CheckBox::new(&mut ss.lock, "Password lock"));
        ui.add_enabled_ui(ss.lock, |ui| minutes_slider(ui, "Lock after", &mut ss.lock_minutes));
    });
    tabula_kit::widgets::separator(ui);
    ui.horizontal(|ui| {
        ui.add_enabled_ui(enabled, |ui| {
            corner_monitor(ui, "fade-now", &mut ss.fade_now, "Fade now when\nmouse is here");
            ui.add_space(m::SPACING * 3.0);
            corner_monitor(ui, "never-fade", &mut ss.never_fade, "Don't fade when\nmouse is here");
        });
    });
    if ss.fade_now.is_some() && ss.fade_now == ss.never_fade {
        ss.never_fade = None;
    }
}

/// A small monitor: click a corner of its screen to choose it, the middle
/// for none (shown, as on R5, by a "no" sign).
fn corner_monitor(ui: &mut Ui, id: &str, corner: &mut Option<Corner>, label: &str) {
    let p = style::palette(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(56.0, 50.0), Sense::click());
    let painter = ui.painter();
    let body = Rect::from_min_size(rect.min, vec2(56.0, 42.0));
    painter.rect_filled(body, 2.0, Color32::from_gray(200));
    style::bevel(painter, body, Color32::from_gray(240), Color32::from_gray(110));
    style::frame_rect(painter, body.expand(1.0), Color32::from_gray(70));
    let screen = body.shrink(5.0);
    painter.rect_filled(screen, 0.0, Color32::from_rgb(51, 102, 152));
    style::frame_rect(painter, screen, Color32::from_gray(60));
    let stand = Rect::from_center_size(pos2(body.center().x, body.bottom() + 4.0), vec2(22.0, 4.0));
    painter.rect_filled(stand, 1.0, Color32::from_gray(150));
    style::frame_rect(painter, stand, Color32::from_gray(80));
    let enabled = ui.is_enabled();
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
            let pts = vec![pos2(x, y), pos2(x + 14.0 * dx, y), pos2(x, y + 14.0 * dy)];
            painter.add(Shape::convex_polygon(pts, Color32::from_rgb(255, 203, 0), Stroke::new(1.0, Color32::BLACK)));
        }
        None => {
            let red = if enabled { Color32::from_rgb(220, 30, 30) } else { Color32::from_rgb(170, 110, 110) };
            painter.circle_stroke(screen.center(), 8.0, Stroke::new(2.0, red));
            let d = 8.0 * std::f32::consts::FRAC_1_SQRT_2;
            painter.line_segment([screen.center() + vec2(-d, -d), screen.center() + vec2(d, d)], Stroke::new(2.0, red));
        }
    }
    if resp.clicked()
        && let Some(at) = resp.interact_pointer_pos()
    {
        let (fx, fy) = ((at.x - screen.left()) / screen.width(), (at.y - screen.top()) / screen.height());
        let side = |f: f32| if f < 0.34 { Some(false) } else if f > 0.66 { Some(true) } else { None };
        *corner = match (side(fx), side(fy)) {
            (Some(false), Some(false)) => Some(Corner::TopLeft),
            (Some(true), Some(false)) => Some(Corner::TopRight),
            (Some(false), Some(true)) => Some(Corner::BottomLeft),
            (Some(true), Some(true)) => Some(Corner::BottomRight),
            _ => None,
        };
    }
    let _ = id;
    let text_color = if enabled { c(p.panel_text) } else { style::disabled_text(&p) };
    let galley = ui.painter().layout(label.to_owned(), style::plain_font(), text_color, 140.0);
    let (r, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(pos2(r.left(), rect.center().y - galley.size().y / 2.0), galley, text_color);
}

const LIST_W: f32 = 150.0;

fn modules(ui: &mut Ui, s: &mut Settings, server: &Server, st: &mut State) {
    let p = style::palette(ui.ctx());
    let area = ui.available_rect_before_wrap();
    let left = Rect::from_min_size(area.min, vec2(LIST_W, area.height()));
    let right = Rect::from_min_max(pos2(left.right() + m::SPACING * 2.0, area.top()), area.max);
    let ss = &mut s.screen_saver;
    let current = MODULES.iter().position(|m| m.name == ss.module).unwrap_or(0);
    let info = &MODULES[current];
    let options = ss.options.get(info.name).cloned().unwrap_or_default();

    // The preview monitor.
    let monitor = Rect::from_min_size(left.min, vec2(LIST_W, 110.0));
    let screen = Rect::from_min_size(monitor.min + vec2(9.0, 8.0), vec2(LIST_W - 18.0, 80.0));
    let painter = ui.painter();
    let bezel = Rect::from_min_max(monitor.min, pos2(monitor.right(), screen.bottom() + 12.0));
    painter.rect_filled(bezel, 4.0, Color32::from_gray(150));
    style::bevel(painter, bezel, Color32::from_gray(205), Color32::from_gray(90));
    style::frame_rect(painter, bezel.expand(1.0), Color32::from_gray(40));
    painter.rect_filled(screen, 0.0, Color32::BLACK);
    for (i, col) in [Color32::from_rgb(60, 220, 60), Color32::from_gray(100)].iter().enumerate() {
        painter.circle_filled(pos2(bezel.right() - 22.0 + i as f32 * 7.0, bezel.bottom() - 6.0), 1.6, *col);
    }
    let stale = st.preview.as_ref().is_none_or(|(n, o, _, _)| n != info.name || *o != options);
    if st.preview.is_none() {
        // Opening the tab: show the chosen module in the list.
        ScrollView::reveal(ui.ctx(), "modules", current as f32 * m::LIST_ROW, (current + 1) as f32 * m::LIST_ROW);
    }
    if stale {
        st.preview = Some((info.name.to_owned(), options.clone(), info.make(&options, tabula_screensavers::seed()), Instant::now()));
    }
    if let Some((_, _, module, last)) = &mut st.preview {
        let now = Instant::now();
        let dt = (now - *last).as_secs_f32();
        *last = now;
        let clipped = ui.painter().with_clip_rect(screen);
        module.draw(&Frame { painter: &clipped, rect: screen, dt, desktop: None });
        ui.ctx().request_repaint();
    }

    // The module list.
    let list = Rect::from_min_max(pos2(left.left(), monitor.bottom() + m::SPACING), pos2(left.right(), left.bottom() - m::CONTROL_HEIGHT - m::SPACING));
    let mut chosen = current;
    ui.scope_builder(UiBuilder::new().max_rect(list), |ui| {
        // Rows touch (show_rows counts the spacing into each row).
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollView::new("modules").background(p.document_background).show_rows(ui, m::LIST_ROW, MODULES.len(), |ui, range| {
            for i in range {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), m::LIST_ROW), Sense::click());
                if i == chosen {
                    ui.painter().rect_filled(r, 0.0, c(p.list_selected_background));
                }
                let color = if i == chosen { c(p.list_selected_item_text) } else { c(p.document_text) };
                ui.painter().text(pos2(r.left() + 6.0, r.center().y), Align2::LEFT_CENTER, MODULES[i].name, style::plain_font(), color);
                if resp.clicked() {
                    chosen = i;
                }
            }
        });
    });
    if chosen != current {
        ss.module = MODULES[chosen].name.to_owned();
    }
    let test = Rect::from_min_size(pos2(left.left(), left.bottom() - m::CONTROL_HEIGHT), vec2(m::BUTTON_MIN_WIDTH, m::CONTROL_HEIGHT));
    if ui.put(test, Button::new("Test")).clicked() {
        server.start_screen_saver();
    }

    // Its settings.
    let info = &MODULES[chosen];
    ui.scope_builder(UiBuilder::new().max_rect(right), |ui| {
        BBox::new("Module settings").show(ui, |ui| {
            ui.add(StringView::new(info.about));
            let opts = ss.options.entry(info.name.to_owned()).or_default();
            for opt in info.options {
                option_ui(ui, info.name, opt, opts);
            }
            if info.uses_desktop {
                ui.add(StringView::new("Works on a picture of the desktop.").dim(true));
            }
            ui.allocate_space(vec2(ui.available_width(), ui.available_height() - 4.0));
        });
    });
    // Drop empty option maps, so an untouched module leaves no trace.
    ss.options.retain(|_, o| !o.is_empty());
}

fn option_ui(ui: &mut Ui, module: &str, opt: &Opt, opts: &mut Options) {
    match *opt {
        Opt::Check { key, label, default } => {
            let mut v = tabula_screensavers::get_bool(opts, key, default);
            if ui.add(CheckBox::new(&mut v, label)).changed() {
                opts.insert(key.into(), v.to_string());
            }
        }
        Opt::Choice { key, label, choices, default } => {
            let mut i = tabula_screensavers::get_usize(opts, key, default);
            let before = i;
            ui.add(MenuField::new((module, key), &mut i, choices).label(label).divider(90.0));
            if i != before {
                opts.insert(key.into(), i.to_string());
            }
        }
        Opt::Slider { key, label, min, max, default } => {
            let mut v = tabula_screensavers::get_f32(opts, key, default);
            let before = v;
            let whole = max - min >= 4.0;
            let text = if whole { format!("{v:.0}") } else { format!("{v:.2}") };
            let step = if whole { 1.0 } else { 0.05 };
            ui.add(Slider::new(&mut v, min..=max).step(step).label(label).value_text(text));
            if v != before {
                opts.insert(key.into(), v.to_string());
            }
        }
        Opt::Text { key, label, default } => {
            let mut v = tabula_screensavers::get_text(opts, key, default).to_owned();
            ui.add(StringView::new(label));
            if ui.add(TextControl::new(&mut v)).changed() {
                opts.insert(key.into(), v);
            }
        }
    }
}
