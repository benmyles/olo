//! Terminal-themed live-streaming Transcript view (`render_transcript_view`)
//! and harmonized in-panel Find bar (`render_transcript_find_bar`).
//!
//! Styled directly from the active [`TerminalTheme`] (`background`, `foreground`,
//! and contrast-verified ANSI 16 accents reserved for meaning: speaker identity,
//! state, diff signs, and Find highlights) while keeping all chrome in Olo's
//! native BeOS palette and UI sans font.

use std::{collections::HashSet, rc::Rc, sync::Arc};

use olo_kit::{
    Bounds, Child, CursorStyle, Element, FontWeight, Hsla, InteractiveElement as _, KeyDownEvent, MouseButton, ParentElement as _, PathBuilder, Pixels, Point, ScrollHandle, SharedString, Styled as _,
    TextRun, WindowCx, canvas, control_canvas, div, icons, metrics as m, point, px, relative, size, style, styled_text, theme, tip,
    widgets::{Button, ScrollView, Segment, SegmentedControl, StringView, TextControl, TextField},
};
use olo_rex_client::activity::{
    BodySegment, DiffLine, InlineKind, InlineSpan, MatchField, SearchMatch, Speaker, ToolCategory, ToolStatus, TranscriptEntry, classify_tool, extract_tool_diff, format_code_lang, format_tool_input,
    friendly_tool_name, parse_inline_markdown,
};
use olo_rex_vt::{Rgb, TerminalTheme};
use olo_sync::Mutex;

use crate::{
    activity_watch::TranscriptSession,
    themes::{hsla, legible, mix},
};

/// Actions emitted by the Transcript viewer header and body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranscriptDo {
    ShowTerminal,
    ShowTranscript,
    ToggleExpand(usize),
    ToggleFollow,
    JumpToLive,
    ToggleFind,
    Close,
}

/// Comfortable prose measure (~90 characters) while tool rows, code, and diffs keep full width.
const PROSE_MAX_W: f32 = 540.0;
/// Shared horizontal gutter inside the stream: disclosure triangles hang in
/// `0..LATCH_W` while speaker names, prose, tool labels, and all text inside
/// expanded wells align to the exact same `x = m::SPACING + LATCH_W` axis.
const LATCH_W: f32 = 14.0;
const LABEL_COL_W: f32 = 72.0;
const EVENT_ROW_H: f32 = 24.0;
const MONO_SIZE: f32 = style::SMALL_SIZE;

/// Contrast-verified Waiting orange on Olo's panel chrome (`>= 4.5:1` in light & dark).
pub fn waiting_chrome_color(p: &olo_theme::Palette) -> Hsla {
    let raw = p.hue(olo_theme::Hue::Orange);
    if p.mode == olo_theme::ThemeMode::Dark {
        style::c(olo_theme::mix(raw, p.panel_text, 0.22))
    } else {
        // Darken light-mode orange clearly against the cool light-grey panel background.
        style::c(olo_theme::mix(raw, p.panel_text, 0.44))
    }
}

/// Semantic palette derived from the active [`TerminalTheme`]:
/// - One contrast-verified secondary text role (`>= 4.6:1` against `bg`).
/// - ANSI hues reserved strictly for meaning (speakers, state, diff signs, Find hits),
///   each verified `>= 4.5:1` against `bg` and falling back toward `fg` if weak.
#[derive(Clone, Copy)]
struct TailPalette {
    key: u64,
    bg_rgba: olo_theme::Rgba,
    bg: Hsla,
    fg: Hsla,
    secondary: Hsla,
    surface: Hsla,
    surface_border: Hsla,
    rule: Hsla,
    red: Hsla,
    green: Hsla,
    blue: Hsla,
    cyan: Hsla,
    code_pill_bg: Hsla,
    diff_add_bg: Hsla,
    diff_del_bg: Hsla,
    diff_add_sign: Hsla,
    diff_del_sign: Hsla,
    err_well_bg: Hsla,
    find_hit_bg: Hsla,
    find_hit_ink: Hsla,
    find_act_bg: Hsla,
    find_act_ink: Hsla,
}

impl TailPalette {
    fn from_theme(t: &TerminalTheme) -> Self {
        let bg = t.background;
        let fg = t.foreground;
        let is_light = t.is_light();
        let pal = t.palette_256();
        let pick = |norm: usize, bright: usize| -> Rgb {
            let raw = if is_light { pal[norm] } else { pal[bright] };
            legible(raw, bg, 4.6)
        };
        let red_rgb = pick(1, 9);
        let green_rgb = pick(2, 10);
        let blue_rgb = pick(4, 12);
        let cyan_rgb = pick(6, 14);

        let surface_rgb = if is_light { mix(fg, bg, 0.04) } else { mix(fg, bg, 0.055) };
        let border_rgb = mix(fg, bg, if is_light { 0.16 } else { 0.15 });
        let rule_rgb = mix(fg, bg, if is_light { 0.13 } else { 0.12 });
        // Single contrast-verified secondary text role (>= 4.6:1 at 1×, 1.5×, and 2×).
        let secondary_rgb = legible(mix(fg, bg, if is_light { 0.62 } else { 0.65 }), bg, 4.7);

        let code_pill_rgb = mix(fg, bg, if is_light { 0.075 } else { 0.11 });
        // Subtle ~12% diff wash so successful edits stay calm beneath the conversation.
        let diff_add_rgb = mix(pal[2], surface_rgb, if is_light { 0.11 } else { 0.13 });
        let diff_del_rgb = mix(pal[1], surface_rgb, if is_light { 0.11 } else { 0.13 });
        let diff_add_sign_rgb = legible(if is_light { pal[2] } else { pal[10] }, diff_add_rgb, 4.8);
        let diff_del_sign_rgb = legible(if is_light { pal[1] } else { pal[9] }, diff_del_rgb, 4.8);
        // Subtle error tint on failed blocks; neutral outline + 2pt red left accent rail.
        let err_well_rgb = mix(pal[1], surface_rgb, if is_light { 0.055 } else { 0.095 });

        let find_hit_bg_rgb = mix(pal[3], bg, if is_light { 0.34 } else { 0.40 });
        let find_hit_ink_rgb = legible(fg, find_hit_bg_rgb, 4.6);
        let find_act_bg_rgb = if is_light { pal[3] } else { pal[11] };
        let find_act_ink_rgb = legible(Rgb::new(16, 18, 22), find_act_bg_rgb, 4.6);

        let key = style::key((bg.r, bg.g, bg.b, fg.r, fg.g, fg.b, pal[1].r, pal[2].g, pal[4].b, pal[6].g, is_light));
        Self {
            key,
            bg_rgba: [bg.r, bg.g, bg.b, 255],
            bg: hsla(bg),
            fg: hsla(fg),
            secondary: hsla(secondary_rgb),
            surface: hsla(surface_rgb),
            surface_border: hsla(border_rgb),
            rule: hsla(rule_rgb),
            red: hsla(red_rgb),
            green: hsla(green_rgb),
            blue: hsla(blue_rgb),
            cyan: hsla(cyan_rgb),
            code_pill_bg: hsla(code_pill_rgb),
            diff_add_bg: hsla(diff_add_rgb),
            diff_del_bg: hsla(diff_del_rgb),
            diff_add_sign: hsla(diff_add_sign_rgb),
            diff_del_sign: hsla(diff_del_sign_rgb),
            err_well_bg: hsla(err_well_rgb),
            find_hit_bg: hsla(find_hit_bg_rgb),
            find_hit_ink: hsla(find_hit_ink_rgb),
            find_act_bg: hsla(find_act_bg_rgb),
            find_act_ink: hsla(find_act_ink_rgb),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HeaderStatus {
    Working,
    #[allow(dead_code)]
    Waiting,
    Done,
    Failed,
}

/// Shared BeOS `STRIP_HEIGHT (34 pt)` viewer header strip:
/// - Leading group (identity + state): `Back` + icon + title + 12 pt gap + live status (`● Working`, `● Waiting`, `Done`, `● Failed`).
/// - Trailing group (actions): `Find` (transcript only), plus `16 pt` gap + `Transcript | Terminal` ONLY when the view actually has a terminal.
#[allow(clippy::too_many_arguments)]
fn viewer_header(
    icon_name: &'static str,
    title: String,
    status: HeaderStatus,
    read_only_suffix: bool,
    badge_tip: Option<(&'static str, &'static str)>,
    is_tmux: bool,
    has_transcript: bool,
    has_terminal: bool,
    find_open: Option<bool>,
    on_do: Arc<dyn Fn(&TranscriptDo, &mut WindowCx)>,
) -> Element {
    let head_bg = canvas(|g, b| {
        let p = theme::palette();
        style::fill_rect(g, b, style::c(p.panel_background));
        let edge = if p.mode == olo_theme::ThemeMode::Dark { style::c(olo_theme::mix(p.panel_background, [255, 255, 255, 255], 0.14)) } else { style::shade(&p, p.panel_background, 1.28) };
        style::hline_above(g, b.left(), b.right(), b.bottom(), edge);
    })
    .paint_key(901)
    .absolute()
    .inset_0();

    let cb_close = on_do.clone();
    let back_btn = Button::new(olo_kit::words::tr("Back").to_string()).id("rex-tr-close").min_width(0.0).on_click(move |_, cx| cb_close(&TranscriptDo::Close, cx));

    let icon_el = canvas(move |g, b| {
        let text_middle = style::baseline_at(g, b.center().y, style::PLAIN_SIZE) - px(style::PLAIN_SIZE * style::CAP_HEIGHT / 2.0);
        let icon_b = style::snap_centred(g, point(b.left() + px(8.0), text_middle), 16.0, 16.0);
        icons::paint(g, icon_b, icon_name);
    })
    .paint_key(style::key(("tr-head-icon-clean", icon_name)))
    .w(px(16.0))
    .h(px(m::CONTROL_HEIGHT))
    .flex_shrink_0();

    // Measure natural title width so the status indicator sits 12 pt right after
    // the title (grouped with identity on the left rather than crowding Find).
    let title_nat_w = if let Some((lead, rest)) = title.split_once(" — ") {
        let w1 = f32::from(style::text_width(lead.to_owned(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD)));
        let w2 = f32::from(style::text_width(format!(" — {rest}"), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL)));
        (w1 + w2).ceil() + 2.0
    } else {
        f32::from(style::text_width(title.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD))).ceil() + 2.0
    };

    let title_for_paint = title.clone();
    let title_el = canvas(move |g, b| {
        let p = theme::palette();
        let ink = style::c(p.panel_text);
        if let Some((lead, rest)) = title_for_paint.split_once(" — ") {
            let full = format!("{lead} — {rest}");
            let tail_len = full.len() - lead.len();
            let runs = [
                TextRun { len: lead.len(), font: style::ui_font(FontWeight::BOLD), color: ink, background_color: None, underline: None, strikethrough: None, size: None },
                TextRun { len: tail_len, font: style::ui_font(FontWeight::NORMAL), color: ink, background_color: None, underline: None, strikethrough: None, size: None },
            ];
            style::label_runs_fit(g, b.left(), b.center().y, style::Align::Left, SharedString::from(full), style::PLAIN_SIZE, &runs, b.size.width);
        } else {
            style::label_fit(g, b.left(), b.center().y, style::Align::Left, title_for_paint.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), ink, b.size.width);
        }
    })
    .paint_key(style::key(("tr-head-title", &title)))
    .w_full()
    .h(px(m::CONTROL_HEIGHT));

    let p = theme::palette();
    let (dot_color, word, word_color, bold_word) = match status {
        HeaderStatus::Working => (Some(p.success), olo_kit::words::tr("Working"), None, false),
        HeaderStatus::Waiting => (Some(p.hue(olo_theme::Hue::Orange)), olo_kit::words::tr("Waiting"), Some(waiting_chrome_color(&p)), true),
        HeaderStatus::Done => (None, olo_kit::words::tr("Done"), None, false),
        HeaderStatus::Failed => (Some(p.failure), olo_kit::words::tr("Failed"), Some(style::c(p.failure)), true),
    };
    let badge_text = if read_only_suffix { format!("{word} · {}", olo_kit::words::tr("Read-only")) } else { word.to_owned() };

    let status_inner = {
        let mut row = div().flex().flex_row().items_center().gap(px(5.0)).flex_shrink_0();
        if let Some(dc) = dot_color {
            let dot = canvas(move |g, b| {
                let mid = style::baseline_at(g, b.center().y, style::PLAIN_SIZE) - px(style::SMALL_SIZE * style::CAP_HEIGHT / 2.0);
                style::dot(g, point(b.center().x, mid), 6.0, style::c(dc));
            })
            .paint_key(style::key(("tr-head-dot", status as u8)))
            .w(px(8.0))
            .h(px(m::CONTROL_HEIGHT))
            .flex_shrink_0();
            row = row.child(dot);
        }
        let mut sv = StringView::new(badge_text).size(style::SMALL_SIZE).beside(style::PLAIN_SIZE).bold(bold_word).height(m::CONTROL_HEIGHT);
        sv = match word_color {
            Some(c) => sv.color(c),
            None => sv.dim(true),
        };
        row.child(sv)
    };
    let status_el = match badge_tip {
        Some((k, t)) => tip::on(status_inner, k, t),
        None => status_inner,
    };

    let mut controls = div().flex().flex_row().items_center().gap(px(12.0)).flex_shrink_0();
    if let Some(is_find) = find_open {
        let cb_find = on_do.clone();
        controls = controls.child(Button::new(olo_kit::words::tr("Find").to_string()).id("rex-tr-find").min_width(0.0).pushed(is_find).on_click(move |_, cx| cb_find(&TranscriptDo::ToggleFind, cx)));
    }
    // Show Transcript | Terminal switch ONLY when both modes exist (never render
    // a one-sided control with a pale disabled Terminal button).
    if has_terminal {
        let cb_seg = on_do.clone();
        let mode_switch = SegmentedControl::new("rex-viewer-mode", [Segment::new("Transcript").enabled(has_transcript), Segment::new("Terminal").enabled(true)], if is_tmux { 1 } else { 0 })
            .on_choose(move |idx: &usize, cx| {
                if *idx == 0 {
                    cb_seg(&TranscriptDo::ShowTranscript, cx);
                } else {
                    cb_seg(&TranscriptDo::ShowTerminal, cx);
                }
            });
        controls = controls.child(mode_switch);
    }

    div()
        .relative()
        .w_full()
        .h(px(olo_kit::widgets::STRIP_HEIGHT))
        .flex_shrink_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(m::SPACING))
        .child(head_bg)
        .child(back_btn)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .child(
                    div()
                        .min_w_0()
                        .max_w(px(title_nat_w + 16.0 + m::TIGHT))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(m::TIGHT))
                        .child(icon_el)
                        .child(div().flex_1().min_w_0().w(px(title_nat_w)).child(title_el)),
                )
                .child(status_el),
        )
        .child(controls)
}

/// Compact joined up/down BeOS push button (`24 × 24`) matching `CONTROL_HEIGHT`
/// for stepping through Transcript Find matches (`join_left` / `join_right` share
/// the middle hairline seamlessly).
fn step_triangle_button(id: &'static str, name: &'static str, up: bool, enabled: bool, join_right: bool, join_left: bool, on_click: impl Fn(&mut WindowCx) + 'static) -> Element {
    let cb = Rc::new(on_click);
    let mut el = control_canvas(move |g, r, s| {
        let p = theme::palette();
        let base = p.control_background;
        let bg = p.panel_background;
        let down = s.pressed && s.enabled;
        let dark = p.mode == olo_theme::ThemeMode::Dark;
        let (t0, t1) = if down {
            (style::shade(&p, base, 1.15), style::shade(&p, base, 1.05))
        } else if dark {
            (style::c(olo_theme::mix(base, p.panel_text, 0.08)), style::c(base))
        } else {
            (style::tint(base, 0.88), style::shade(&p, base, 1.06))
        };
        let inner = style::inset(g, r, 1.0);
        style::vgradient(g, inner, t0, t1);
        let frame = style::shade(&p, bg, 1.55);
        style::hline(g, r.left(), r.right(), r.top(), frame);
        style::hline_above(g, r.left(), r.right(), r.bottom(), frame);
        if !join_left {
            style::vline(g, r.left(), r.top(), r.bottom(), frame);
        }
        style::vline_left_of(g, r.right(), r.top(), r.bottom(), frame);
        let _ = join_right;
        if down {
            style::bevel(g, inner, style::shade(&p, base, 1.3), style::shade(&p, base, 1.05));
        } else {
            let shine = if dark { style::c(olo_theme::mix(base, [255, 255, 255, 255], 0.2)) } else { style::c(p.shine) };
            style::bevel(g, inner, shine, style::shade(&p, base, 1.18));
        }
        let color = if s.enabled { style::c(p.control_text) } else { style::disabled_text(&p) };
        let c = r.center() + if down { point(px(0.5), px(0.5)) } else { point(px(0.0), px(0.0)) };
        let (hw, hh) = (px(3.5), px(2.2));
        let pts = if up { [point(c.x - hw, c.y + hh), point(c.x + hw, c.y + hh), point(c.x, c.y - hh)] } else { [point(c.x - hw, c.y - hh), point(c.x + hw, c.y - hh), point(c.x, c.y + hh)] };
        let mut pb = PathBuilder::fill();
        pb.move_to(pts[0]);
        pb.line_to(pts[1]);
        pb.line_to(pts[2]);
        pb.close();
        if let Ok(path) = pb.build() {
            g.paint_path(path, color);
        }
        g.agent(olo_kit::agent::Node { role: "Button", name: Some(name.into()), bounds: r, enabled: s.enabled, clickable: s.enabled, focused: s.focused, ..Default::default() });
    })
    .paint_key(style::key((id, up, enabled, join_left, join_right)))
    .id(SharedString::from(id))
    .w(px(m::CONTROL_HEIGHT))
    .h(px(m::CONTROL_HEIGHT))
    .flex_shrink_0();
    if !enabled {
        return el.disabled(true).unfaded();
    }
    el = el.tab_stop().keeps_focus().cursor(CursorStyle::PointingHand).on_mouse_down(MouseButton::Left, move |_, cx| {
        cb(cx);
        cx.stop_propagation();
    });
    el
}

/// Harmonized `STRIP_HEIGHT (34 pt)` Find bar placed directly under the
/// Transcript header: field aligned left under `Back`, joined ▲▼ stepper pair,
/// secondary match counter (`1 of 3` / `Not found`), and trailing `Done`.
pub fn render_transcript_find_bar(
    cx: &WindowCx,
    field: &Child<TextField>,
    said: String,
    found: bool,
    step: impl Fn(&bool, &mut WindowCx) + 'static,
    close: impl Fn(&(), &mut WindowCx) + 'static,
) -> Element {
    let step = Rc::new(step);
    let close = Rc::new(close);
    let (back, next, before, esc, done) = (step.clone(), step.clone(), step, close.clone(), close);

    let bg = canvas(|g, b| {
        let p = theme::palette();
        style::fill_rect(g, b, style::c(p.panel_background));
        let edge = if p.mode == olo_theme::ThemeMode::Dark { style::c(olo_theme::mix(p.panel_background, [255, 255, 255, 255], 0.14)) } else { style::shade(&p, p.panel_background, 1.28) };
        style::hline_above(g, b.left(), b.right(), b.bottom(), edge);
    })
    .paint_key(905)
    .absolute()
    .inset_0();

    let steppers = div()
        .flex()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .child(step_triangle_button("rex-tr-find-prev", "Previous Match", true, found, true, false, move |cx| back(&true, cx)))
        .child(step_triangle_button("rex-tr-find-next", "Next Match", false, found, false, true, move |cx| next(&false, cx)));

    div()
        .relative()
        .w_full()
        .h(px(olo_kit::widgets::STRIP_HEIGHT))
        .flex_shrink_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::TIGHT))
        .px(px(m::SPACING))
        .child(bg)
        .on_key_down(move |e: &KeyDownEvent, cx| {
            if e.keystroke.key == "escape" {
                esc(&(), cx);
                cx.stop_propagation();
            } else if e.keystroke.key == "enter" && e.keystroke.modifiers.shift {
                before(&true, cx);
                cx.stop_propagation();
            }
        })
        .child(div().w(px(220.0)).flex_shrink_0().child(TextControl::new(cx, field)))
        .child(steppers)
        .child(div().pl(px(2.0)).flex_1().min_w_0().child(StringView::new(said).size(style::SMALL_SIZE).dim(true).cut(true).height(m::CONTROL_HEIGHT)))
        .child(Button::new(olo_kit::words::tr("Done").to_string()).id("rex-tr-find-done").min_width(0.0).on_click(move |_, cx| done(&(), cx)))
}

/// Render the terminal-themed live-streaming conversation transcript tail.
/// Live / Paused state and `Jump to Live` live in the window status bar so the
/// transcript viewport keeps one steady height without shifting on toggle.
/// Returns the rendered element plus an optional direct-child index inside the
/// `ScrollView` that should be scrolled into view (e.g., active Find match).
#[allow(clippy::too_many_arguments)]
pub fn render_transcript_view(
    session: &Arc<Mutex<TranscriptSession>>,
    scroll: &ScrollHandle,
    active_theme: &TerminalTheme,
    find_open: bool,
    find_bar: Option<Element>,
    on_do: impl Fn(&TranscriptDo, &mut WindowCx) + 'static,
) -> (Element, Option<usize>) {
    let on_do: Arc<dyn Fn(&TranscriptDo, &mut WindowCx)> = Arc::new(on_do);
    let (target, title, agent_label, model, loading, streaming, error, truncated, entries, expanded, collapsed_errors, collapsed_running, matches, active_match_idx, scroll_to_entry, agent_json) = {
        let mut g = session.lock().unwrap_or_else(|e| e.into_inner());
        let mut exp: HashSet<usize> = g.expanded.clone();
        if let Some(auto_id) = g.find_auto_expanded {
            exp.insert(auto_id);
        }
        let target_eid = g.scroll_to_entry.take();
        (
            g.target.clone(),
            g.target.title(),
            g.doc.agent_label.clone(),
            g.doc.model.clone(),
            g.loading,
            g.streaming,
            g.error.clone(),
            g.doc.truncated,
            g.doc.entries.clone(),
            exp,
            g.collapsed_errors.clone(),
            g.collapsed_running.clone(),
            g.matches.clone(),
            g.match_idx,
            target_eid,
            g.agent_json(),
        )
    };

    let tp = TailPalette::from_theme(active_theme);
    let agent_icon = if target.has_terminal() { "terminal" } else { "text" };

    let hdr_status = if error.is_some() {
        HeaderStatus::Failed
    } else if streaming {
        HeaderStatus::Working
    } else {
        HeaderStatus::Done
    };

    let header = viewer_header(agent_icon, title, hdr_status, false, None, false, true, target.has_terminal(), Some(find_open), on_do.clone());

    // Direct children inside `ScrollView` so `scroll_to_item` targets exact entries
    // (for Find match navigation) while keeping outer horizontal padding aligned with
    // `Back` and the Find field (`m::SPACING = 12 pt`).
    let mut stream_items: Vec<Element> = Vec::new();
    let mut scroll_to_child: Option<usize> = None;
    let wrap_row = |el: Element| div().w_full().px(px(m::SPACING)).flex().flex_col().child(el);
    stream_items.push(div().w_full().h(px(12.0)));

    if truncated {
        stream_items.push(wrap_row(
            div()
                .w_full()
                .pl(px(LATCH_W))
                .pb(px(8.0))
                .child(StringView::new(olo_kit::words::tr("Showing the latest 1 MB of a long transcript.").to_owned()).size(style::SMALL_SIZE).color(tp.secondary).height(m::LABEL_ROW)),
        ));
    }

    if let Some(err) = error {
        stream_items.push(wrap_row(
            div()
                .w_full()
                .py(px(m::SECTION))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.0))
                .child(StringView::new(olo_kit::words::tr("Transcript unavailable").to_owned()).bold(true).color(tp.red))
                .child(StringView::new(err).size(style::SMALL_SIZE).color(tp.secondary).wrap(true)),
        ));
    } else if entries.is_empty() {
        let note = if loading { olo_kit::words::tr("Loading transcript…") } else { olo_kit::words::tr("Waiting for the agent’s first turn…") };
        stream_items.push(wrap_row(div().w_full().py(px(m::SECTION)).flex().items_center().justify_center().child(StringView::new(note.to_owned()).color(tp.secondary))));
    } else {
        let mut active_turn: Option<Speaker> = None;
        let mut prev_ts: Option<String> = None;
        let mut shown_model = false;
        let mut prev_block_kind: Option<BlockKind> = None;

        for entry in &entries {
            let eid = entry.id();
            let entry_ts = entry_timestamp(entry);
            let turn_kind = match entry {
                TranscriptEntry::Message { speaker: Speaker::User, .. } => Some(Speaker::User),
                TranscriptEntry::Divider { .. } => None,
                _ => Some(Speaker::Assistant),
            };

            if let Some(kind) = turn_kind {
                let starts_new_turn = active_turn != Some(kind) || kind == Speaker::User;
                if starts_new_turn {
                    if active_turn.is_some() {
                        stream_items.push(wrap_row(turn_separator(&tp)));
                    }
                    let (speaker_name, ts_for_turn) = match entry {
                        TranscriptEntry::Message { label, timestamp, .. } => (label.clone(), timestamp.clone()),
                        _ => (agent_label.clone(), entry_ts.to_owned()),
                    };
                    let model_tag = if kind == Speaker::Assistant && !shown_model && !model.is_empty() {
                        shown_model = true;
                        Some(model.clone())
                    } else {
                        None
                    };
                    stream_items.push(wrap_row(render_speaker_line(&tp, kind, agent_icon, speaker_name, model_tag, ts_for_turn)));
                    active_turn = Some(kind);
                    prev_block_kind = None;
                }
            } else {
                active_turn = None;
                prev_block_kind = None;
            }

            let duration = prev_ts.as_deref().and_then(|p_ts| elapsed_seconds(p_ts, entry_ts));
            if !entry_ts.is_empty() {
                prev_ts = Some(entry_ts.to_owned());
            }

            // Failed tools and currently Running tools auto-expand so errors and live
            // streaming output are visible right away (user can still toggle either).
            let is_failed_tool = matches!(entry, TranscriptEntry::ToolCall { status: ToolStatus::Error, .. });
            let is_running_tool = matches!(entry, TranscriptEntry::ToolCall { status: ToolStatus::Running, .. });
            let is_open = if is_failed_tool {
                !collapsed_errors.contains(&eid) || expanded.contains(&eid)
            } else if is_running_tool {
                !collapsed_running.contains(&eid) || expanded.contains(&eid)
            } else {
                expanded.contains(&eid)
            };
            let user_expanded_full = expanded.contains(&eid);

            let (el, end_kind) = match entry {
                TranscriptEntry::Message { segments, .. } => {
                    let top_gap = if prev_block_kind.is_some() { 10.0 } else { 0.0 };
                    (render_message_body(&tp, eid, segments, top_gap, &matches, active_match_idx), BlockKind::ProseOrWell)
                }
                TranscriptEntry::Thinking { summary, text, .. } => {
                    let top_gap = if matches!(prev_block_kind, Some(BlockKind::ProseOrWell)) { 10.0 } else { 0.0 };
                    let kind = if is_open { BlockKind::ProseOrWell } else { BlockKind::CollapsedRow };
                    (render_thinking_item(&tp, eid, summary, text, is_open, top_gap, &matches, active_match_idx, on_do.clone()), kind)
                }
                TranscriptEntry::ToolCall { name, summary, input, output, status, .. } => {
                    let top_gap = if matches!(prev_block_kind, Some(BlockKind::ProseOrWell)) { 10.0 } else { 0.0 };
                    let has_well =
                        is_open && (extract_tool_diff(name, input).is_some() || !format_tool_input(name, summary, input).is_empty() || output.as_deref().is_some_and(|s| !s.trim().is_empty()));
                    let kind = if has_well { BlockKind::ProseOrWell } else { BlockKind::CollapsedRow };
                    let cap_lines = if is_failed_tool && !user_expanded_full {
                        Some(8)
                    } else if is_running_tool && !user_expanded_full {
                        Some(6)
                    } else {
                        None
                    };
                    (
                        render_tool_item(&tp, eid, name, summary, input, output.as_deref(), *status, duration, is_open, cap_lines, is_running_tool, top_gap, &matches, active_match_idx, on_do.clone()),
                        kind,
                    )
                }
                TranscriptEntry::Divider { label, timestamp, .. } => (render_divider(&tp, label, timestamp), BlockKind::ProseOrWell),
            };
            if scroll_to_entry == Some(eid) {
                scroll_to_child = Some(stream_items.len());
            }
            stream_items.push(wrap_row(el));
            prev_block_kind = Some(end_kind);
        }
    }
    stream_items.push(div().w_full().h(px(24.0)));

    let scroll_for_cb = scroll.clone();
    let sess_scroll = session.clone();
    let scroller = ScrollView::new(scroll)
        .vertical(true)
        .auto_hide(false)
        .border(false)
        .background(tp.bg_rgba)
        .on_scroll(move |off, cx| {
            let cur_y = f32::from(off.y);
            let max_y = f32::from(scroll_for_cb.max_offset().height);
            let mut g = sess_scroll.lock().unwrap_or_else(|e| e.into_inner());
            let prev_y = g.last_scroll_y;
            let prev_max = g.last_max_y;
            g.last_scroll_y = cur_y;
            g.last_max_y = max_y;
            let scrollability_changed = (prev_max > 0.5) != (max_y > 0.5);
            let mut notify = scrollability_changed;

            if max_y <= 0.5 {
                if !g.follow {
                    g.follow = true;
                    g.paused_unseen = 0;
                    g.generation += 1;
                    notify = true;
                }
                drop(g);
                if notify {
                    crate::ui::activity_changed();
                }
                return;
            }

            // Offset y is 0 at top and -max_y at bottom; moving toward 0 means scrolling UP into history.
            let at_bottom = -cur_y >= max_y - 4.0;
            let moved_up = cur_y > prev_y + 0.5;
            if moved_up && !at_bottom {
                if g.follow {
                    g.follow = false;
                    g.generation += 1;
                    notify = true;
                }
            } else if at_bottom {
                if !g.follow {
                    g.follow = true;
                    g.paused_unseen = 0;
                    g.generation += 1;
                    notify = true;
                }
            } else if g.follow && max_y > prev_max + 0.5 {
                // Viewport shrank or content grew while pinned to live tail: stay at bottom.
                drop(g);
                scroll_for_cb.scroll_to_bottom(cx);
                if notify {
                    crate::ui::activity_changed();
                }
                return;
            }
            drop(g);
            if notify {
                crate::ui::activity_changed();
            }
        })
        .children(stream_items);

    let container_mark = canvas(|g, b| {
        olo_kit::review::container(g, b);
    })
    .paint_key(906)
    .absolute()
    .inset_0();

    // Flex column so `ScrollView` (`size_full`) inherits definite height cleanly;
    // follow pause/resume is driven strictly by actual `ScrollView` movement above
    // (never by wheel events falling through when `max_offset == 0`).
    let body_box = div().relative().flex_1().min_h_0().w_full().flex().flex_col().bg(tp.bg).child(scroller);

    let el = div().relative().size_full().flex().flex_col().agent("Group", "agent-transcript").agent_value(agent_json).child(container_mark).child(header).children(find_bar).child(body_box);
    (el, scroll_to_child)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    CollapsedRow,
    ProseOrWell,
}

fn entry_timestamp(e: &TranscriptEntry) -> &str {
    match e {
        TranscriptEntry::Message { timestamp, .. } | TranscriptEntry::Thinking { timestamp, .. } | TranscriptEntry::ToolCall { timestamp, .. } | TranscriptEntry::Divider { timestamp, .. } => {
            timestamp
        }
    }
}

fn parse_hms(ts: &str) -> Option<u32> {
    let mut parts = ts.split(':');
    let h: u32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let s: u32 = parts.next()?.parse().ok()?;
    Some(h * 3600 + m * 60 + s)
}

fn elapsed_seconds(prev: &str, curr: &str) -> Option<String> {
    let a = parse_hms(prev)?;
    let b = parse_hms(curr)?;
    if b > a && b - a < 3600 { Some(format!("{}s", b - a)) } else { None }
}

fn extract_exit_code(output: Option<&str>) -> Option<i32> {
    let text = output?;
    for line in text.lines().rev() {
        let lower = line.to_ascii_lowercase();
        for prefix in ["command exited with code ", "exit status ", "exited with code ", "exit code "] {
            if let Some(pos) = lower.find(prefix) {
                let rest = lower[pos + prefix.len()..].trim();
                let digits: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
                if let Ok(code) = digits.parse::<i32>() {
                    return Some(code);
                }
            }
        }
    }
    None
}

fn cap_head_lines(s: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= max_lines { s.to_owned() } else { format!("{}\n…", lines[..max_lines].join("\n")) }
}

fn cap_tail_lines(s: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= max_lines { s.to_owned() } else { format!("…\n{}", lines[lines.len() - max_lines..].join("\n")) }
}

fn turn_separator(tp: &TailPalette) -> Element {
    let rule_c = tp.rule;
    let key = style::key(("tr-turn-sep", tp.key));
    div().w_full().pt(px(14.0)).pb(px(10.0)).child(
        canvas(move |g, b| {
            style::hline(g, b.left(), b.right(), b.top(), rule_c);
        })
        .paint_key(key)
        .w_full()
        .h(px(1.0)),
    )
}

/// Clean speaker heading without stray vertical bars:
/// - User: bold ANSI blue `You` on the body column (`LATCH_W`).
/// - Assistant/subagent: crisp 16 pt icon inline before bold ANSI cyan name,
///   mono model tag, and UI-sans timestamp on the right edge.
fn render_speaker_line(tp: &TailPalette, speaker: Speaker, icon_name: &'static str, name: String, model: Option<String>, timestamp: String) -> Element {
    let accent = match speaker {
        Speaker::User => tp.blue,
        Speaker::Assistant => tp.cyan,
        Speaker::System => tp.secondary,
    };
    let spk_size = style::PLAIN_SIZE + 1.0;

    let mut row = div().w_full().pl(px(LATCH_W)).pb(px(4.0)).flex().flex_row().items_center();

    if speaker == Speaker::Assistant {
        let icon_el = canvas(move |g, b| {
            let text_mid = style::baseline_at(g, b.center().y, spk_size) - px(spk_size * style::CAP_HEIGHT / 2.0);
            let icon_b = style::snap_centred(g, point(b.left() + px(8.0), text_mid), 16.0, 16.0);
            icons::paint(g, icon_b, icon_name);
        })
        .paint_key(style::key(("tr-spk-icon", icon_name)))
        .w(px(16.0))
        .h(px(20.0))
        .mr(px(6.0))
        .flex_shrink_0();
        row = row.child(icon_el);
    }

    row = row.child(StringView::new(name).bold(true).size(spk_size).color(accent).height(20.0));

    if let Some(m) = model {
        row = row.child(div().pl(px(8.0)).child(StringView::new(m).mono(true).size(style::SMALL_SIZE).beside(spk_size).color(tp.secondary).height(20.0)));
    }
    row = row.child(div().flex_1());
    if !timestamp.is_empty() {
        row = row.child(StringView::new(timestamp).size(style::SMALL_SIZE).beside(spk_size).color(tp.secondary).height(20.0));
    }
    row
}

fn field_match_ranges(matches: &[SearchMatch], active_idx: Option<usize>, entry_id: usize, field: MatchField) -> Vec<(usize, usize, bool)> {
    matches.iter().enumerate().filter(|(_, m)| m.entry_id == entry_id && m.field == field).map(|(i, m)| (m.start, m.end, active_idx == Some(i))).collect()
}

/// Render a slice of pre-parsed inline markdown text (`plain[slice_start..slice_end]`)
/// with shifted span and Find-match ranges so split heading/bullet/paragraph blocks
/// preserve exact Find highlight byte offsets.
#[allow(clippy::too_many_arguments)]
fn render_rich_slice(
    tp: &TailPalette,
    plain: &str,
    md_spans: &[InlineSpan],
    is_mono: bool,
    font_size: f32,
    base_ink: Hsla,
    field_matches: &[(usize, usize, bool)],
    slice_start: usize,
    slice_end: usize,
) -> Element {
    let sub = &plain[slice_start..slice_end];
    if sub.is_empty() {
        return div().into();
    }
    let sub_len = sub.len();
    let mut bounds: Vec<usize> = vec![0, sub_len];
    for sp in md_spans {
        let s = sp.start.saturating_sub(slice_start).min(sub_len);
        let e = sp.end.saturating_sub(slice_start).min(sub_len);
        if s < e {
            bounds.push(s);
            bounds.push(e);
        }
    }
    for &(ms, me, _) in field_matches {
        let s = ms.saturating_sub(slice_start).min(sub_len);
        let e = me.saturating_sub(slice_start).min(sub_len);
        if s < e {
            bounds.push(s);
            bounds.push(e);
        }
    }
    bounds.sort_unstable();
    bounds.dedup();

    let ui_reg = style::ui_font(FontWeight::NORMAL);
    let ui_bold = style::ui_font(FontWeight::BOLD);
    let mono_reg = style::mono_font(FontWeight::NORMAL);
    let mono_bold = style::mono_font(FontWeight::BOLD);

    let mut runs = Vec::with_capacity(bounds.len().saturating_sub(1));
    for w in bounds.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b <= a {
            continue;
        }
        let ga = slice_start + a;
        let gb = slice_start + b;
        let in_code = is_mono || md_spans.iter().any(|sp| sp.kind == InlineKind::Code && ga >= sp.start && gb <= sp.end);
        let in_bold = md_spans.iter().any(|sp| sp.kind == InlineKind::Bold && ga >= sp.start && gb <= sp.end);
        let in_heading = md_spans.iter().any(|sp| sp.kind == InlineKind::Heading && ga >= sp.start && gb <= sp.end);
        let in_quote = md_spans.iter().any(|sp| sp.kind == InlineKind::Quote && ga >= sp.start && gb <= sp.end);
        let in_bullet = md_spans.iter().any(|sp| sp.kind == InlineKind::Bullet && ga >= sp.start && gb <= sp.end);
        let active_hit = field_matches.iter().any(|&(ms, me, act)| act && ga >= ms && gb <= me);
        let any_hit = active_hit || field_matches.iter().any(|&(ms, me, _)| ga >= ms && gb <= me);

        let (color, background_color) = if active_hit {
            (tp.find_act_ink, Some(tp.find_act_bg))
        } else if any_hit {
            (tp.find_hit_ink, Some(tp.find_hit_bg))
        } else if in_code && !is_mono {
            // Foreground mono on subtle chip (cyan reserved for speaker identity).
            (tp.fg, Some(tp.code_pill_bg))
        } else if in_heading {
            (tp.fg, None)
        } else if in_bullet || in_quote {
            (tp.secondary, None)
        } else {
            (base_ink, None)
        };

        let (font, size) = if in_code {
            let sz = if is_mono { font_size } else { (font_size - 0.5).max(10.0) };
            let f = if in_bold || in_heading { mono_bold.clone() } else { mono_reg.clone() };
            (f, Some(px(sz)))
        } else if in_heading || in_bold {
            // Headings stay at body size, bold (terminal-markdown convention so speakers outrank headings).
            (ui_bold.clone(), Some(px(font_size)))
        } else {
            (ui_reg.clone(), Some(px(font_size)))
        };
        runs.push(TextRun { len: b - a, font, color, background_color, underline: None, strikethrough: None, size });
    }

    div().w_full().line_height(relative(style::LINE_HEIGHT)).text_size(px(font_size)).child(styled_text(sub.to_owned(), runs))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MdBlockKind {
    Heading,
    Bullet,
    Normal,
}

/// Render plain or inline-markdown text (`code` chips, `**bold**`, body-size bold
/// headings with clean bottom breathing room, hanging-indent `•` bullets, quotes,
/// and exact Find highlight spans).
fn render_rich_text(tp: &TailPalette, raw: &str, is_mono: bool, parse_md: bool, font_size: f32, base_ink: Hsla, field_matches: &[(usize, usize, bool)]) -> Element {
    let (plain, md_spans) = if parse_md { parse_inline_markdown(raw) } else { (raw.to_owned(), Vec::new()) };
    if plain.is_empty() {
        return div().into();
    }
    let has_structure = parse_md && md_spans.iter().any(|sp| matches!(sp.kind, InlineKind::Heading | InlineKind::Bullet));
    if !has_structure {
        return render_rich_slice(tp, &plain, &md_spans, is_mono, font_size, base_ink, field_matches, 0, plain.len());
    }

    let mut blocks: Vec<(usize, usize, MdBlockKind)> = Vec::new();
    let mut chunk_start: Option<usize> = None;
    let mut chunk_end: usize = 0;
    let mut pos = 0usize;
    for raw_line in plain.split('\n') {
        let line_start = pos;
        let line_end = pos + raw_line.len();
        pos = (line_end + 1).min(plain.len());
        let is_heading = !raw_line.is_empty() && md_spans.iter().any(|sp| sp.kind == InlineKind::Heading && sp.start == line_start && sp.end == line_end);
        let is_bullet = raw_line.starts_with("• ");
        if is_heading || is_bullet {
            if let Some(cs) = chunk_start.take() {
                blocks.push((cs, chunk_end, MdBlockKind::Normal));
            }
            blocks.push((line_start, line_end, if is_heading { MdBlockKind::Heading } else { MdBlockKind::Bullet }));
        } else {
            if chunk_start.is_none() {
                chunk_start = Some(line_start);
            }
            chunk_end = line_end;
        }
    }
    if let Some(cs) = chunk_start {
        blocks.push((cs, chunk_end, MdBlockKind::Normal));
    }

    let mut col = div().w_full().flex().flex_col();
    for (idx, (mut s, mut e, kind)) in blocks.into_iter().enumerate() {
        while s < e && plain.as_bytes()[s] == b'\n' {
            s += 1;
        }
        while e > s && plain.as_bytes()[e - 1] == b'\n' {
            e -= 1;
        }
        if s >= e {
            continue;
        }
        match kind {
            MdBlockKind::Heading => {
                let el = render_rich_slice(tp, &plain, &md_spans, is_mono, font_size, base_ink, field_matches, s, e);
                let mut wrap = div().w_full().pb(px(5.0)).child(el);
                if idx > 0 {
                    wrap = wrap.pt(px(6.0));
                }
                col = col.child(wrap);
            }
            MdBlockKind::Bullet => {
                // Hanging indent: `•` sits on the body column, text begins 14 pt in
                // so wrapped lines stay aligned cleanly under the bullet text.
                let bullet_prefix = "• ".len();
                let text_start = (s + bullet_prefix).min(e);
                let text_el = render_rich_slice(tp, &plain, &md_spans, is_mono, font_size, base_ink, field_matches, text_start, e);
                let glyph = div().w(px(14.0)).flex_shrink_0().line_height(relative(style::LINE_HEIGHT)).text_size(px(font_size)).child(styled_text(
                    "•".to_owned(),
                    vec![TextRun {
                        len: "•".len(),
                        font: style::ui_font(FontWeight::NORMAL),
                        color: tp.secondary,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                        size: Some(px(font_size)),
                    }],
                ));
                col = col.child(div().w_full().pb(px(2.0)).flex().flex_row().items_start().child(glyph).child(div().flex_1().min_w_0().child(text_el)));
            }
            MdBlockKind::Normal => {
                let el = render_rich_slice(tp, &plain, &md_spans, is_mono, font_size, base_ink, field_matches, s, e);
                col = col.child(el);
            }
        }
    }
    col
}

/// Terminal-themed recessed surface well with a crisp neutral 1-pt outline and
/// optional 2.5-pt left rail. All inner text aligns to `LATCH_W` from the well's
/// left edge so prose, filepath headers, diff signs, and output share one column.
fn terminal_well(tp: &TailPalette, top_gap: f32, accent_rail: Option<Hsla>, is_error: bool, caption: Option<String>, raw_padded: bool, body: Element) -> Element {
    let fill = if is_error { tp.err_well_bg } else { tp.surface };
    let border = tp.surface_border;
    let key = style::key(("tr-term-well", tp.key, is_error, accent_rail.map(|c| style::ink_key(Some(c)))));
    let frame = canvas(move |g, b| {
        style::fill_rect(g, b, fill);
        style::frame_rect(g, b, border);
        if let Some(rail) = accent_rail {
            let strip = Bounds::new(point(b.left() + px(1.0), b.top() + px(1.0)), size(px(2.5), (b.size.height - px(2.0)).max(px(0.0))));
            style::fill_rect(g, strip, rail);
        }
    })
    .paint_key(key)
    .absolute()
    .inset_0();

    let mut inner = div().relative().w_full().flex().flex_col().gap(px(4.0)).py(px(8.0));
    if !raw_padded {
        inner = inner.pl(px(LATCH_W)).pr(px(12.0));
    }
    if let Some(cap) = caption
        && !cap.is_empty()
    {
        let cap_el = StringView::new(cap).size(style::SMALL_SIZE).color(tp.secondary).height(15.0);
        inner = inner.child(if raw_padded { div().pl(px(LATCH_W)).pr(px(12.0)).child(cap_el) } else { div().child(cap_el) });
    }
    inner = inner.child(body);
    div().relative().w_full().mt(px(top_gap)).child(frame).child(inner)
}

/// Render unified diff lines with code text in `tp.fg`, hue ONLY on `-`/`+` signs,
/// subtle ~12% background washes extending cleanly from the block's left rail, and
/// `-`/`+` aligned to the exact same `LATCH_W` body column as filepath and Output.
fn render_diff_lines(tp: &TailPalette, lines: &[DiffLine]) -> Element {
    let mut col = div().w_full().flex().flex_col();
    for line in lines.iter().take(120) {
        let is_hunk = matches!(line, DiffLine::Hunk(_));
        match line {
            DiffLine::Hunk(h) => {
                let run = TextRun { len: h.len(), font: style::mono_font(FontWeight::BOLD), color: tp.fg, background_color: None, underline: None, strikethrough: None, size: Some(px(MONO_SIZE)) };
                col = col.child(div().w_full().pl(px(LATCH_W)).pr(px(12.0)).pb(px(5.0)).line_height(relative(1.36)).text_size(px(MONO_SIZE)).child(styled_text(h.clone(), vec![run])));
            }
            DiffLine::Remove(r) => {
                let sign = "- ";
                let full = format!("{sign}{r}");
                let runs = vec![
                    TextRun {
                        len: sign.len(),
                        font: style::mono_font(FontWeight::BOLD),
                        color: tp.diff_del_sign,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                        size: Some(px(MONO_SIZE)),
                    },
                    TextRun { len: r.len(), font: style::mono_font(FontWeight::NORMAL), color: tp.fg, background_color: None, underline: None, strikethrough: None, size: Some(px(MONO_SIZE)) },
                ];
                // Band sits strictly inside the well's 2.5pt left rail and 1pt right outline;
                // text starts at `3.5 + (LATCH_W - 3.5) = LATCH_W`.
                col =
                    col.child(
                        div().w_full().pl(px(3.5)).pr(px(1.0)).child(
                            div().w_full().pl(px(LATCH_W - 3.5)).pr(px(11.0)).py(px(1.0)).bg(tp.diff_del_bg).line_height(relative(1.36)).text_size(px(MONO_SIZE)).child(styled_text(full, runs)),
                        ),
                    );
            }
            DiffLine::Add(a) => {
                let sign = "+ ";
                let full = format!("{sign}{a}");
                let runs = vec![
                    TextRun {
                        len: sign.len(),
                        font: style::mono_font(FontWeight::BOLD),
                        color: tp.diff_add_sign,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                        size: Some(px(MONO_SIZE)),
                    },
                    TextRun { len: a.len(), font: style::mono_font(FontWeight::NORMAL), color: tp.fg, background_color: None, underline: None, strikethrough: None, size: Some(px(MONO_SIZE)) },
                ];
                col =
                    col.child(
                        div().w_full().pl(px(3.5)).pr(px(1.0)).child(
                            div().w_full().pl(px(LATCH_W - 3.5)).pr(px(11.0)).py(px(1.0)).bg(tp.diff_add_bg).line_height(relative(1.36)).text_size(px(MONO_SIZE)).child(styled_text(full, runs)),
                        ),
                    );
            }
            DiffLine::Context(c) => {
                let full = format!("  {c}");
                let run = TextRun {
                    len: full.len(),
                    font: style::mono_font(FontWeight::NORMAL),
                    color: tp.secondary,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                    size: Some(px(MONO_SIZE)),
                };
                col = col.child(div().w_full().pl(px(LATCH_W)).pr(px(12.0)).py(px(1.0)).line_height(relative(1.36)).text_size(px(MONO_SIZE)).child(styled_text(full, vec![run])));
            }
        }
        let _ = is_hunk;
    }
    if lines.len() > 120 {
        col = col.child(div().pl(px(LATCH_W)).pr(px(12.0)).child(StringView::new(format!("… {} more diff lines", lines.len() - 120)).size(style::SMALL_SIZE).color(tp.secondary).height(16.0)));
    }
    col
}

fn render_message_body(tp: &TailPalette, eid: usize, segments: &[BodySegment], top_gap: f32, matches: &[SearchMatch], active_match_idx: Option<usize>) -> Element {
    let mut col = div().w_full().flex().flex_col().gap(px(10.0));
    if top_gap > 0.0 {
        col = col.pt(px(top_gap));
    }
    for (seg_idx, seg) in segments.iter().enumerate() {
        let seg_hits = field_match_ranges(matches, active_match_idx, eid, MatchField::MessageSegment(seg_idx));
        match seg {
            BodySegment::Text(text) => {
                let prose = render_rich_text(tp, text.trim(), false, true, style::PLAIN_SIZE, tp.fg, &seg_hits);
                col = col.child(div().w_full().max_w(px(PROSE_MAX_W + LATCH_W)).pl(px(LATCH_W)).child(prose));
            }
            BodySegment::Code { lang, code } => {
                let cleaned = code.trim_end_matches('\n');
                let cap = format_code_lang(lang);
                let is_diff = matches!(lang.trim().to_ascii_lowercase().as_str(), "diff" | "patch") && seg_hits.is_empty();
                if is_diff {
                    let parsed: Vec<DiffLine> = cleaned
                        .lines()
                        .map(|l| {
                            if l.starts_with("@@") || l.starts_with("+++") || l.starts_with("---") {
                                DiffLine::Hunk(l.to_owned())
                            } else if let Some(r) = l.strip_prefix('+') {
                                DiffLine::Add(r.to_owned())
                            } else if let Some(r) = l.strip_prefix('-') {
                                DiffLine::Remove(r.to_owned())
                            } else {
                                DiffLine::Context(l.to_owned())
                            }
                        })
                        .collect();
                    col = col.child(terminal_well(tp, 0.0, None, false, (!cap.is_empty()).then_some(cap), true, render_diff_lines(tp, &parsed)));
                } else {
                    let code_el = render_rich_text(tp, cleaned, true, false, MONO_SIZE, tp.fg, &seg_hits);
                    col = col.child(terminal_well(tp, 0.0, None, false, (!cap.is_empty()).then_some(cap), false, code_el));
                }
            }
        }
    }
    col
}

/// Drawn BeOS 8-pt disclosure latch (`style::latch`) hanging cleanly in `0..LATCH_W`,
/// always neutral secondary grey across Thinking and every tool row.
fn event_latch(tp: &TailPalette, open: bool) -> Element {
    let tint = tp.secondary;
    let key = style::key(("tr-latch", tp.key, open));
    canvas(move |g, b| {
        let r = Bounds::new(point(b.left(), b.top()), size(px(10.0), b.size.height));
        style::latch(g, r, open, tint);
    })
    .paint_key(key)
    .w(px(LATCH_W))
    .h(px(EVENT_ROW_H))
    .flex_shrink_0()
}

#[allow(clippy::too_many_arguments)]
fn render_thinking_item(
    tp: &TailPalette,
    eid: usize,
    summary: &str,
    text: &str,
    open: bool,
    top_gap: f32,
    matches: &[SearchMatch],
    active_match_idx: Option<usize>,
    on_do: Arc<dyn Fn(&TranscriptDo, &mut WindowCx)>,
) -> Element {
    let cb = on_do.clone();
    let mut header = div()
        .w_full()
        .h(px(EVENT_ROW_H))
        .flex()
        .flex_row()
        .items_center()
        .cursor(CursorStyle::PointingHand)
        .on_mouse_down(MouseButton::Left, move |_, cx| {
            cb(&TranscriptDo::ToggleExpand(eid), cx);
            cx.stop_propagation();
        })
        .child(event_latch(tp, open))
        .child(
            div()
                .w(px(LABEL_COL_W))
                .flex_shrink_0()
                .child(StringView::new(olo_kit::words::tr("Thinking").to_owned()).bold(true).size(style::SMALL_SIZE).color(tp.secondary).cut(true).height(EVENT_ROW_H)),
        );

    if !open && !summary.is_empty() {
        header = header.child(div().flex_1().min_w_0().child(StringView::new(summary.to_owned()).size(style::SMALL_SIZE).color(tp.secondary).cut(true).height(EVENT_ROW_H)));
    }

    let mut col = div().w_full().flex().flex_col();
    if top_gap > 0.0 {
        col = col.pt(px(top_gap));
    }
    col = col.child(header);
    if open {
        let hits = field_match_ranges(matches, active_match_idx, eid, MatchField::ThinkingText);
        let body = render_rich_text(tp, text.trim(), false, true, style::PLAIN_SIZE - 0.5, tp.secondary, &hits);
        col = col.child(terminal_well(tp, 3.0, Some(tp.surface_border), false, None, false, div().w_full().max_w(px(PROSE_MAX_W)).child(body)));
    }
    col
}

#[allow(clippy::too_many_arguments)]
fn render_tool_item(
    tp: &TailPalette,
    eid: usize,
    name: &str,
    summary: &str,
    raw_input: &str,
    output: Option<&str>,
    status: ToolStatus,
    duration: Option<String>,
    open: bool,
    cap_lines: Option<usize>,
    is_running_tail: bool,
    top_gap: f32,
    matches: &[SearchMatch],
    active_match_idx: Option<usize>,
    on_do: Arc<dyn Fn(&TranscriptDo, &mut WindowCx)>,
) -> Element {
    let cat = classify_tool(name);
    let badge_label = friendly_tool_name(name);

    let cb = on_do.clone();
    let mut header = div()
        .w_full()
        .h(px(EVENT_ROW_H))
        .flex()
        .flex_row()
        .items_center()
        .cursor(CursorStyle::PointingHand)
        .on_mouse_down(MouseButton::Left, move |_, cx| {
            cb(&TranscriptDo::ToggleExpand(eid), cx);
            cx.stop_propagation();
        })
        .child(event_latch(tp, open))
        // Tool names stay foreground bold; hue is reserved for state, speakers, diff signs, and Find.
        .child(div().w(px(LABEL_COL_W)).flex_shrink_0().child(StringView::new(badge_label).bold(true).size(style::SMALL_SIZE).color(tp.fg).cut(true).height(EVENT_ROW_H)))
        .child(div().flex_1().min_w_0().child(StringView::new(summary.to_owned()).mono(true).size(style::SMALL_SIZE).color(tp.fg).cut(true).height(EVENT_ROW_H)));

    match status {
        ToolStatus::Running => {
            let dot_c = tp.green;
            let live_dot = canvas(move |g, b| {
                style::dot(g, b.center(), 6.0, dot_c);
            })
            .paint_key(style::key(("tr-live-dot", tp.key)))
            .w(px(8.0))
            .h(px(EVENT_ROW_H))
            .flex_shrink_0();
            header = header
                .child(div().w(px(8.0)))
                .child(live_dot)
                .child(div().w(px(4.0)))
                .child(StringView::new(olo_kit::words::tr("Running").to_owned()).bold(true).size(style::SMALL_SIZE).color(tp.green).height(EVENT_ROW_H));
            if let Some(d) = duration {
                header = header.child(StringView::new(format!(" · {d}")).size(style::SMALL_SIZE).color(tp.secondary).height(EVENT_ROW_H));
            }
        }
        ToolStatus::Error => {
            // Red ONLY on the word "Failed"; exit code in regular secondary UI sans (no redundant red dot).
            header = header.child(div().w(px(8.0))).child(StringView::new(olo_kit::words::tr("Failed").to_owned()).bold(true).size(style::SMALL_SIZE).color(tp.red).height(EVENT_ROW_H));
            if let Some(code) = extract_exit_code(output) {
                header = header.child(StringView::new(format!(" · exit {code}")).size(style::SMALL_SIZE).color(tp.secondary).height(EVENT_ROW_H));
            }
        }
        ToolStatus::Ok => {
            let ok_text = match duration {
                Some(dur) => format!("✓ {dur}"),
                None => "✓".to_owned(),
            };
            header = header.child(div().w(px(8.0))).child(StringView::new(ok_text).size(style::SMALL_SIZE).color(tp.secondary).height(EVENT_ROW_H));
        }
    }

    let mut col = div().w_full().flex().flex_col();
    if top_gap > 0.0 {
        col = col.pt(px(top_gap));
    }
    col = col.child(header);
    if open {
        let diff_lines = extract_tool_diff(name, raw_input);
        let input_rows = if diff_lines.is_some() { Vec::new() } else { format_tool_input(name, summary, raw_input) };
        let out_clean = output.map(|s| s.trim_end_matches('\n')).filter(|s| !s.is_empty());
        let has_in = diff_lines.is_some() || !input_rows.is_empty();
        let has_out = out_clean.is_some();

        if has_in || has_out {
            // Use raw-padded inner layout whenever a diff is shown so diff bands
            // extend cleanly to the left rail while every text line aligns to `LATCH_W`.
            let use_raw = diff_lines.is_some();
            let wrap_pad = |el: Element| -> Element { if use_raw { div().w_full().pl(px(LATCH_W)).pr(px(12.0)).child(el) } else { el } };
            let mut inner = div().w_full().flex().flex_col().gap(px(6.0));
            if let Some(dlines) = &diff_lines {
                let in_hits = field_match_ranges(matches, active_match_idx, eid, MatchField::ToolInput);
                if in_hits.is_empty() {
                    inner = inner.child(render_diff_lines(tp, dlines));
                } else {
                    let flat = olo_rex_client::activity::tool_input_display_text(name, summary, raw_input);
                    inner = inner.child(wrap_pad(render_rich_text(tp, &flat, true, false, MONO_SIZE, tp.fg, &in_hits)));
                }
            } else if !input_rows.is_empty() {
                let in_hits = field_match_ranges(matches, active_match_idx, eid, MatchField::ToolInput);
                if in_hits.is_empty() {
                    let mut kv_col = div().w_full().flex().flex_col().gap(px(3.0));
                    for (k, v) in &input_rows {
                        if k.is_empty() {
                            let prefix_val = if cat == ToolCategory::Shell { format!("$ {v}") } else { v.clone() };
                            kv_col = kv_col.child(render_rich_text(tp, &prefix_val, true, false, MONO_SIZE, tp.fg, &[]));
                        } else {
                            kv_col = kv_col.child(
                                div()
                                    .w_full()
                                    .flex()
                                    .flex_row()
                                    .items_start()
                                    .gap(px(8.0))
                                    .child(StringView::new(format!("{k}:")).size(style::SMALL_SIZE).color(tp.secondary).height(15.0))
                                    .child(div().flex_1().min_w_0().child(render_rich_text(tp, v, true, false, MONO_SIZE, tp.fg, &[]))),
                            );
                        }
                    }
                    inner = inner.child(wrap_pad(kv_col));
                } else {
                    let flat = olo_rex_client::activity::tool_input_display_text(name, summary, raw_input);
                    inner = inner.child(wrap_pad(render_rich_text(tp, &flat, true, false, MONO_SIZE, tp.fg, &in_hits)));
                }
            }
            if has_in && has_out {
                let sep_c = tp.rule;
                let sep_key = style::key(("tr-well-sep", tp.key));
                inner = inner.child(wrap_pad(canvas(move |g, b| style::hline(g, b.left(), b.right(), b.top(), sep_c)).paint_key(sep_key).w_full().h(px(1.0))));
                inner = inner.child(wrap_pad(div().child(StringView::new(olo_kit::words::tr("Output").to_owned()).size(style::SMALL_SIZE).color(tp.secondary).height(15.0))));
            }
            if let Some(out) = out_clean {
                let out_hits = field_match_ranges(matches, active_match_idx, eid, MatchField::ToolOutput);
                let shown_out = match cap_lines {
                    Some(n) if out_hits.is_empty() => {
                        if is_running_tail {
                            cap_tail_lines(out, n)
                        } else {
                            cap_head_lines(out, n)
                        }
                    }
                    _ => out.to_owned(),
                };
                inner = inner.child(wrap_pad(render_rich_text(tp, &shown_out, true, false, MONO_SIZE, tp.fg, &out_hits)));
            }
            // Error gets the one red 2pt left bar; running/normal blocks use a quiet neutral rail.
            let rail = if status == ToolStatus::Error { Some(tp.red) } else { Some(tp.surface_border) };
            col = col.child(terminal_well(tp, 3.0, rail, status == ToolStatus::Error, None, use_raw, inner));
        }
    }
    col
}

fn render_divider(tp: &TailPalette, label: &str, timestamp: &str) -> Element {
    let line_c = tp.rule;
    let key = style::key(("tr-div-rule", tp.key));
    let rule = move || {
        canvas(move |g, b| {
            style::hline(g, b.left(), b.right(), b.center().y, line_c);
        })
        .paint_key(key)
        .flex_1()
        .h(px(14.0))
    };
    let text = if timestamp.is_empty() { label.to_owned() } else { format!("{label} · {timestamp}") };
    div().w_full().flex().flex_row().items_center().gap(px(m::TIGHT)).py(px(8.0)).child(rule()).child(StringView::new(text).size(style::SMALL_SIZE).color(tp.secondary).height(14.0)).child(rule())
}

/// Draw a small BeOS disclosure triangle (`▸` collapsed / `▾` expanded) snapped to device pixels.
pub fn paint_disclosure(g: &mut olo_kit::Painter, center: Point<Pixels>, expanded: bool, ink: Hsla) {
    let r = Bounds::new(point(center.x - px(5.0), center.y - px(5.0)), size(px(10.0), px(10.0)));
    style::latch(g, r, expanded, ink);
}
