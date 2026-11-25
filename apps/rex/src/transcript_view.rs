//! Styled live-streaming Transcript view (`render_transcript_view`), harmonized
//! in-panel Find bar (`render_transcript_find_bar`), and bounded safe read-only
//! `tmux attach -r -f ignore-size` viewer (`render_tmux_viewer`).

use std::{collections::HashSet, rc::Rc, sync::Arc};

use olo_kit::{
    Bounds, Child, CursorStyle, Element, FontWeight, Hsla, InteractiveElement as _, IntoElement as _, KeyDownEvent, MouseButton, ParentElement as _, PathBuilder, Pixels, Point, ScrollHandle,
    SharedString, Styled as _, TextRun, WindowCx, canvas, control_canvas, div, icons, metrics as m, point, px, relative, size, style, styled_text, theme, tip,
    widgets::{Button, ScrollView, Segment, SegmentedControl, StringView, TextControl, TextField, agent_marks},
};
use olo_rex_client::activity::{BodySegment, MatchField, SearchMatch, Speaker, SubagentState, ToolStatus, TranscriptEntry, format_code_lang, format_tool_input, parse_inline_code};
use olo_rex_vt::TerminalTheme;
use olo_sync::Mutex;

use crate::{
    activity_watch::{TmuxViewer, TranscriptSession, ViewTarget},
    settings,
    terminal_pane::{self, Metrics, PanePaintArgs},
};

/// Actions emitted by the Transcript / read-only Tmux viewer header and body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranscriptDo {
    ShowTerminal,
    ShowTranscript,
    ToggleExpand(usize),
    ToggleFollow,
    ToggleFind,
    Close,
}

const STREAM_MAX_W: f32 = 760.0;
const PROSE_MAX_W: f32 = 560.0;
/// Shared horizontal inset inside the stream column: every speaker name, prose
/// line, event label, and well text line aligns on this exact left edge, while
/// 8-pt disclosure latches hang cleanly in the `0..TEXT_INSET` margin.
const TEXT_INSET: f32 = 14.0;
/// Fixed width for event row labels (`Thinking`, `Read`, `Bash`, `Edit`, …) so
/// every tool argument starts on one aligned column down the page.
const LABEL_COL_W: f32 = 64.0;
const EVENT_ROW_H: f32 = 22.0;
const MONO_SIZE: f32 = style::SMALL_SIZE;

pub fn subagent_mark(state: SubagentState) -> agent_marks::AgentMark {
    match state {
        SubagentState::Working => agent_marks::AgentMark::Working,
        SubagentState::Waiting => agent_marks::AgentMark::Asking,
        SubagentState::Done => agent_marks::AgentMark::None,
        SubagentState::Failed => agent_marks::AgentMark::Failed,
    }
}

pub fn subagent_state_word(state: SubagentState) -> (&'static str, Option<olo_theme::Rgba>) {
    let p = theme::palette();
    match state {
        SubagentState::Waiting => ("Waiting", Some(p.hue(olo_theme::Hue::Orange))),
        SubagentState::Failed => ("Failed", Some(p.failure)),
        SubagentState::Working => ("Working", None),
        SubagentState::Done => ("Done", None),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HeaderStatus {
    Working,
    Done,
    Failed,
}

/// Shared BeOS `STRIP_HEIGHT (34 pt)` viewer header strip:
/// - Leading edge: `Back` + icon + bold agent name / regular em-dash task title.
/// - Trailing edge: status readout (`● Working`, `Done`, `● Failed`, plus
///   `· Read-only` in Terminal mode), `Find` & `Follow` (transcript only), and
///   `Transcript | Terminal` ALWAYS pinned at the far right edge (`12 pt` inset)
///   so switching modes never shifts the control under the pointer.
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
    follow: Option<bool>,
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

    // Agent name bold + " — task" regular weight, fitted cleanly on one line.
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

    let cb_seg = on_do.clone();
    let mode_switch_raw = SegmentedControl::new("rex-viewer-mode", [Segment::new("Transcript").enabled(has_transcript), Segment::new("Terminal").enabled(has_terminal)], if is_tmux { 1 } else { 0 })
        .on_choose(move |idx: &usize, cx| {
            if *idx == 0 {
                cb_seg(&TranscriptDo::ShowTranscript, cx);
            } else {
                cb_seg(&TranscriptDo::ShowTerminal, cx);
            }
        });
    let mode_switch = if !has_terminal { tip::on(mode_switch_raw.into_element(), "rex-viewer-no-term", "Transcript only — no terminal") } else { mode_switch_raw.into_element() };

    // Pin `Transcript | Terminal` ALWAYS at the trailing right edge so switching
    // between Transcript and Terminal never moves the control under the pointer!
    let mut controls = div().flex().flex_row().items_center().gap(px(8.0)).flex_shrink_0();
    if let Some(is_find) = find_open {
        let cb_find = on_do.clone();
        controls = controls.child(Button::new(olo_kit::words::tr("Find").to_string()).id("rex-tr-find").min_width(0.0).pushed(is_find).on_click(move |_, cx| cb_find(&TranscriptDo::ToggleFind, cx)));
    }
    if let Some(is_following) = follow {
        let cb_follow = on_do.clone();
        controls = controls
            .child(Button::new(olo_kit::words::tr("Follow").to_string()).id("rex-tr-follow").min_width(0.0).pushed(is_following).on_click(move |_, cx| cb_follow(&TranscriptDo::ToggleFollow, cx)));
    }
    controls = controls.child(mode_switch);

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
        .child(div().flex_1().min_w_0().flex().flex_row().items_center().gap(px(m::TIGHT)).child(icon_el).child(div().flex_1().min_w_0().child(title_el)))
        .child(status_el)
        .child(controls)
}

/// Compact up/down BeOS push button (`24 × 24`) matching `CONTROL_HEIGHT` for
/// stepping through Transcript Find matches.
fn step_triangle_button(id: &'static str, name: &'static str, up: bool, enabled: bool, on_click: impl Fn(&mut WindowCx) + 'static) -> Element {
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
        style::frame_rect(g, r, style::shade(&p, bg, 1.55));
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
    .paint_key(style::key((id, up, enabled)))
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
/// Transcript header: field aligned left under `Back`, compact ▲/▼ steppers,
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
        .child(step_triangle_button("rex-tr-find-prev", "Previous Match", true, found, move |cx| back(&true, cx)))
        .child(step_triangle_button("rex-tr-find-next", "Next Match", false, found, move |cx| next(&false, cx)))
        .child(div().pl(px(2.0)).flex_1().min_w_0().child(StringView::new(said).size(style::SMALL_SIZE).dim(true).cut(true).height(m::CONTROL_HEIGHT)))
        .child(Button::new(olo_kit::words::tr("Done").to_string()).id("rex-tr-find-done").min_width(0.0).on_click(move |_, cx| done(&(), cx)))
}

/// Render the authentic BeOS/Olo live-streaming conversation transcript view.
pub fn render_transcript_view(
    session: &Arc<Mutex<TranscriptSession>>,
    scroll: &ScrollHandle,
    find_open: bool,
    find_bar: Option<Element>,
    on_do: impl Fn(&TranscriptDo, &mut WindowCx) + 'static,
) -> Element {
    let on_do: Arc<dyn Fn(&TranscriptDo, &mut WindowCx)> = Arc::new(on_do);
    let (target, title, agent_label, loading, streaming, error, truncated, follow, entries, expanded, collapsed_errors, matches, active_match_idx, agent_json) = {
        let g = session.lock().unwrap_or_else(|e| e.into_inner());
        let mut exp: HashSet<usize> = g.expanded.clone();
        if let Some(auto_id) = g.find_auto_expanded {
            exp.insert(auto_id);
        }
        (
            g.target.clone(),
            g.target.title(),
            g.doc.agent_label.clone(),
            g.loading,
            g.streaming,
            g.error.clone(),
            g.doc.truncated,
            g.follow,
            g.doc.entries.clone(),
            exp,
            g.collapsed_errors.clone(),
            g.matches.clone(),
            g.match_idx,
            g.agent_json(),
        )
    };

    let p = theme::palette();
    let dark = p.mode == olo_theme::ThemeMode::Dark;
    let hdr_status = if error.is_some() {
        HeaderStatus::Failed
    } else if let ViewTarget::SubagentTranscript { sub, .. } = &target {
        match sub.state {
            SubagentState::Failed => HeaderStatus::Failed,
            SubagentState::Done => HeaderStatus::Done,
            SubagentState::Working | SubagentState::Waiting => HeaderStatus::Working,
        }
    } else if streaming {
        HeaderStatus::Working
    } else {
        HeaderStatus::Done
    };

    let header = viewer_header("text", title, hdr_status, false, None, false, true, target.has_terminal(), Some(find_open), Some(follow), on_do.clone());

    let mut stream_col = div().w_full().max_w(px(STREAM_MAX_W)).flex().flex_col().px(px(20.0)).pt(px(16.0)).pb(px(24.0));

    if truncated {
        stream_col = stream_col.child(
            div()
                .w_full()
                .pl(px(TEXT_INSET))
                .pb(px(10.0))
                .child(StringView::new(olo_kit::words::tr("Showing the latest 1 MB of a long transcript.").to_owned()).size(style::SMALL_SIZE).dim(true).height(m::LABEL_ROW)),
        );
    }

    if let Some(err) = error {
        stream_col = stream_col.child(
            div()
                .w_full()
                .py(px(m::SECTION))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.0))
                .child(StringView::new(olo_kit::words::tr("Transcript unavailable").to_owned()).bold(true))
                .child(StringView::new(err).size(style::SMALL_SIZE).dim(true).wrap(true)),
        );
    } else if entries.is_empty() {
        let note = if loading { olo_kit::words::tr("Loading transcript…") } else { olo_kit::words::tr("Waiting for the agent’s first turn…") };
        stream_col = stream_col.child(div().w_full().py(px(m::SECTION)).flex().items_center().justify_center().child(StringView::new(note.to_owned()).dim(true)));
    } else {
        let mut active_turn: Option<Speaker> = None;
        let mut prev_ts: Option<String> = None;
        // Track whether the previous block inside the current turn ended with a well or prose
        // so we enforce exact two-value intra-turn rhythm:
        // - 4 pt between a latch row and its own well
        // - 12 pt between every other pair of blocks (prose/well, well/row, prose/row, row/prose)
        // - 0 pt between consecutive collapsed latch rows (tight 22 pt pitch).
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
                        stream_col = stream_col.child(turn_separator(&p, dark));
                    }
                    let (speaker_name, ts_for_turn) = match entry {
                        TranscriptEntry::Message { label, timestamp, .. } => (label.clone(), timestamp.clone()),
                        _ => (agent_label.clone(), entry_ts.to_owned()),
                    };
                    stream_col = stream_col.child(render_speaker_line(speaker_name, ts_for_turn));
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

            // Failed tool rows open by default (capped to 6 lines unless user explicitly toggled).
            let is_failed_tool = matches!(entry, TranscriptEntry::ToolCall { status: ToolStatus::Error, .. });
            let is_open = if is_failed_tool { !collapsed_errors.contains(&eid) || expanded.contains(&eid) } else { expanded.contains(&eid) };
            let user_expanded_full = expanded.contains(&eid);

            let (el, end_kind) = match entry {
                TranscriptEntry::Message { segments, .. } => {
                    let top_gap = if prev_block_kind.is_some() { 12.0 } else { 0.0 };
                    (render_message_body(&p, dark, eid, segments, top_gap, &matches, active_match_idx), BlockKind::ProseOrWell)
                }
                TranscriptEntry::Thinking { summary, text, .. } => {
                    let top_gap = if matches!(prev_block_kind, Some(BlockKind::ProseOrWell)) { 12.0 } else { 0.0 };
                    let kind = if is_open { BlockKind::ProseOrWell } else { BlockKind::CollapsedRow };
                    (render_thinking_item(&p, dark, eid, summary, text, is_open, top_gap, &matches, active_match_idx, on_do.clone()), kind)
                }
                TranscriptEntry::ToolCall { name, summary, input, output, status, .. } => {
                    let top_gap = if matches!(prev_block_kind, Some(BlockKind::ProseOrWell)) { 12.0 } else { 0.0 };
                    let has_well = is_open && (!format_tool_input(name, summary, input).is_empty() || output.as_deref().is_some_and(|s| !s.trim().is_empty()));
                    let kind = if has_well { BlockKind::ProseOrWell } else { BlockKind::CollapsedRow };
                    (
                        render_tool_item(
                            &p,
                            dark,
                            eid,
                            name,
                            summary,
                            input,
                            output.as_deref(),
                            *status,
                            duration,
                            is_open,
                            is_failed_tool && !user_expanded_full,
                            top_gap,
                            &matches,
                            active_match_idx,
                            on_do.clone(),
                        ),
                        kind,
                    )
                }
                TranscriptEntry::Divider { label, timestamp, .. } => (render_divider(&p, dark, label, timestamp), BlockKind::ProseOrWell),
            };
            stream_col = stream_col.child(el);
            prev_block_kind = Some(end_kind);
        }
    }

    let body_wrap = div().w_full().flex().flex_col().items_center().child(stream_col);
    let scroller = ScrollView::new(scroll).vertical(true).auto_hide(false).border(false).background(p.document_background).child(body_wrap);

    let container_mark = canvas(|g, b| {
        olo_kit::review::container(g, b);
    })
    .paint_key(906)
    .absolute()
    .inset_0();

    // Disable Follow automatically whenever the user scrolls up manually.
    let sess_wheel = session.clone();
    let body_box = div()
        .relative()
        .flex_1()
        .min_h_0()
        .w_full()
        .on_scroll_wheel(move |e: &olo_kit::ScrollWheelEvent, _| {
            let dy = f32::from(e.delta.pixel_delta(px(18.0)).y);
            if dy > 0.5 {
                let mut g = sess_wheel.lock().unwrap_or_else(|e| e.into_inner());
                if g.follow {
                    g.follow = false;
                    g.generation += 1;
                    drop(g);
                    crate::ui::activity_changed();
                }
            }
        })
        .child(scroller);

    div().relative().size_full().flex().flex_col().agent("Group", "agent-transcript").agent_value(agent_json).child(container_mark).child(header).children(find_bar).child(body_box)
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

/// Extract an exit code (e.g. `exit status 1` or `exited with code 2`) from stderr/output.
fn extract_exit_code(output: Option<&str>) -> Option<i32> {
    let text = output?;
    for line in text.lines().rev() {
        let lower = line.to_ascii_lowercase();
        for prefix in ["exit status ", "exited with code ", "exit code "] {
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

/// Cap default-opened error output to 6 lines (+ `"…"` when longer).
fn cap_error_lines(s: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= max_lines { s.to_owned() } else { format!("{}\n…", lines[..max_lines].join("\n")) }
}

/// Turn separator starting at the wells' & latches' exact outer left edge (`0`, not indented).
fn turn_separator(p: &olo_theme::Palette, dark: bool) -> Element {
    let rule_c = if dark { style::c(olo_theme::mix(p.document_background, p.panel_text, 0.16)) } else { style::shade(p, p.panel_background, 1.24) };
    div().w_full().pt(px(16.0)).pb(px(12.0)).child(
        canvas(move |g, b| {
            style::hline(g, b.left(), b.right(), b.top(), rule_c);
        })
        .paint_key(style::key(("tr-turn-sep", dark)))
        .w_full()
        .h(px(1.0)),
    )
}

fn render_speaker_line(speaker: String, timestamp: String) -> Element {
    let mut row = div().w_full().pl(px(TEXT_INSET)).pb(px(4.0)).flex().flex_row().items_center().gap(px(8.0)).child(StringView::new(speaker).bold(true).size(style::PLAIN_SIZE + 1.0).height(18.0));
    if !timestamp.is_empty() {
        row = row.child(StringView::new(timestamp).size(style::SMALL_SIZE).beside(style::PLAIN_SIZE + 1.0).dim(true).height(18.0));
    }
    row
}

fn field_match_ranges(matches: &[SearchMatch], active_idx: Option<usize>, entry_id: usize, field: MatchField) -> Vec<(usize, usize, bool)> {
    matches.iter().enumerate().filter(|(_, m)| m.entry_id == entry_id && m.field == field).map(|(i, m)| (m.start, m.end, active_idx == Some(i))).collect()
}

/// Build an inline-styled paragraph or code block with optional `` `inline code` ``
/// formatting (backticks stripped) AND exact per-character Find highlight spans
/// (no whole-block tint, no horizontal shift, crisp contrast in light & dark).
#[allow(clippy::too_many_arguments)]
fn render_rich_text(raw: &str, is_mono: bool, strip_inline_backticks: bool, font_size: f32, base_ink: Hsla, dark: bool, field_matches: &[(usize, usize, bool)]) -> Element {
    let (plain, code_spans) = if strip_inline_backticks { parse_inline_code(raw) } else { (raw.to_owned(), Vec::new()) };
    if plain.is_empty() {
        return div().into();
    }

    let mut bounds: Vec<usize> = vec![0, plain.len()];
    for &(s, e) in &code_spans {
        if s <= plain.len() && e <= plain.len() && s < e {
            bounds.push(s);
            bounds.push(e);
        }
    }
    for &(s, e, _) in field_matches {
        if s <= plain.len() && e <= plain.len() && s < e {
            bounds.push(s);
            bounds.push(e);
        }
    }
    bounds.sort_unstable();
    bounds.dedup();

    let (hit_bg, hit_ink) = if dark { (style::c([118, 86, 20, 255]), style::c([244, 246, 250, 255])) } else { (style::c([255, 241, 166, 255]), style::c(theme::palette().panel_text)) };
    let (act_bg, act_ink) = if dark { (style::c([242, 182, 44, 255]), style::c([20, 22, 26, 255])) } else { (style::c([246, 186, 42, 255]), style::c([20, 20, 20, 255])) };

    let ui_f = style::ui_font(FontWeight::NORMAL);
    let mono_f = style::mono_font(FontWeight::NORMAL);

    let mut runs = Vec::with_capacity(bounds.len().saturating_sub(1));
    for w in bounds.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b <= a {
            continue;
        }
        let in_code = is_mono || code_spans.iter().any(|&(cs, ce)| a >= cs && b <= ce);
        let active_hit = field_matches.iter().any(|&(ms, me, act)| act && a >= ms && b <= me);
        let any_hit = active_hit || field_matches.iter().any(|&(ms, me, _)| a >= ms && b <= me);

        let (color, background_color) = if active_hit {
            (act_ink, Some(act_bg))
        } else if any_hit {
            (hit_ink, Some(hit_bg))
        } else {
            (base_ink, None)
        };
        let (font, size) = if in_code {
            let sz = if is_mono { font_size } else { (font_size - 1.0).max(10.0) };
            (mono_f.clone(), Some(px(sz)))
        } else {
            (ui_f.clone(), Some(px(font_size)))
        };
        runs.push(TextRun { len: b - a, font, color, background_color, underline: None, strikethrough: None, size });
    }

    div().w_full().line_height(relative(style::LINE_HEIGHT)).text_size(px(font_size)).child(styled_text(plain, runs))
}

/// Authentic single BeOS sunken well (`style::sunken`) sitting clearly darker
/// than `document_background` in both light and dark modes — never nested inside
/// a second outline, with text aligned to the shared `+TEXT_INSET` vertical axis.
fn sunken_well(p: &olo_theme::Palette, dark: bool, top_gap: f32, caption: Option<String>, body: Element) -> Element {
    let well_bg = if dark {
        // Noticeably darker recessed fill than dark document_background.
        olo_theme::mix([0, 0, 0, 255], p.document_background, 0.56)
    } else {
        olo_theme::mix(p.panel_background, p.document_background, 0.42)
    };
    let frame = canvas(move |g, b| {
        let pal = theme::palette();
        style::fill_rect(g, b, style::c(well_bg));
        style::sunken(g, b, &pal, false);
    })
    .paint_key(style::key(("tr-sunken-well", dark)))
    .absolute()
    .inset_0();

    let mut inner = div().relative().w_full().flex().flex_col().gap(px(4.0)).pl(px(TEXT_INSET)).pr(px(12.0)).py(px(8.0));
    if let Some(cap) = caption
        && !cap.is_empty()
    {
        inner = inner.child(StringView::new(cap).size(style::SMALL_SIZE).dim(true).height(14.0));
    }
    inner = inner.child(body);
    div().relative().w_full().mt(px(top_gap)).child(frame).child(inner)
}

fn render_message_body(p: &olo_theme::Palette, dark: bool, eid: usize, segments: &[BodySegment], top_gap: f32, matches: &[SearchMatch], active_match_idx: Option<usize>) -> Element {
    let ink = style::c(p.panel_text);
    // Exact 12 pt spacing between prose and code wells inside a reply.
    let mut col = div().w_full().flex().flex_col().gap(px(12.0));
    if top_gap > 0.0 {
        col = col.pt(px(top_gap));
    }
    for (seg_idx, seg) in segments.iter().enumerate() {
        let seg_hits = field_match_ranges(matches, active_match_idx, eid, MatchField::MessageSegment(seg_idx));
        match seg {
            BodySegment::Text(text) => {
                let prose = render_rich_text(text.trim(), false, true, style::PLAIN_SIZE, ink, dark, &seg_hits);
                col = col.child(div().w_full().max_w(px(PROSE_MAX_W + TEXT_INSET)).pl(px(TEXT_INSET)).child(prose));
            }
            BodySegment::Code { lang, code } => {
                let cleaned = code.trim_end_matches('\n');
                let cap = format_code_lang(lang);
                let code_el = render_rich_text(cleaned, true, false, MONO_SIZE, ink, dark, &seg_hits);
                col = col.child(sunken_well(p, dark, 0.0, (!cap.is_empty()).then_some(cap), code_el));
            }
        }
    }
    col
}

/// Drawn BeOS 8-pt disclosure latch (`style::latch`) hanging in the `0..TEXT_INSET`
/// left margin of every event row.
fn event_latch(open: bool, dark: bool) -> Element {
    canvas(move |g, b| {
        let p = theme::palette();
        let ink = if dark { style::c(olo_theme::mix(p.panel_text, p.document_background, 0.30)) } else { style::secondary_text(&p) };
        let r = Bounds::new(point(b.left(), b.top()), size(px(10.0), b.size.height));
        style::latch(g, r, open, ink);
    })
    .paint_key(style::key(("tr-latch", open, dark)))
    .w(px(TEXT_INSET))
    .h(px(EVENT_ROW_H))
    .flex_shrink_0()
}

#[allow(clippy::too_many_arguments)]
fn render_thinking_item(
    p: &olo_theme::Palette,
    dark: bool,
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
        .child(event_latch(open, dark))
        .child(div().w(px(LABEL_COL_W)).flex_shrink_0().child(StringView::new(olo_kit::words::tr("Thinking").to_owned()).bold(true).size(style::SMALL_SIZE).dim(true).cut(true).height(EVENT_ROW_H)));

    if !open && !summary.is_empty() {
        header = header.child(div().flex_1().min_w_0().child(StringView::new(summary.to_owned()).size(style::SMALL_SIZE).dim(true).cut(true).height(EVENT_ROW_H)));
    }

    let mut col = div().w_full().flex().flex_col();
    if top_gap > 0.0 {
        col = col.pt(px(top_gap));
    }
    col = col.child(header);
    if open {
        let hits = field_match_ranges(matches, active_match_idx, eid, MatchField::ThinkingText);
        // Expanded Thinking stays quieter (secondary ink), matches prose size & 16 pt line
        // spacing, and wraps at the 560 pt prose cap inside the full-width sunken well.
        let body = render_rich_text(text.trim(), false, true, style::PLAIN_SIZE, style::secondary_text(p), dark, &hits);
        col = col.child(sunken_well(p, dark, 4.0, None, div().w_full().max_w(px(PROSE_MAX_W)).child(body)));
    }
    col
}

#[allow(clippy::too_many_arguments)]
fn render_tool_item(
    p: &olo_theme::Palette,
    dark: bool,
    eid: usize,
    name: &str,
    summary: &str,
    raw_input: &str,
    output: Option<&str>,
    status: ToolStatus,
    duration: Option<String>,
    open: bool,
    cap_err_preview: bool,
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
        .child(event_latch(open, dark))
        .child(div().w(px(LABEL_COL_W)).flex_shrink_0().child(StringView::new(name.to_owned()).bold(true).size(style::SMALL_SIZE).cut(true).height(EVENT_ROW_H)))
        .child(div().flex_1().min_w_0().child(StringView::new(summary.to_owned()).mono(true).size(style::SMALL_SIZE).dim(true).cut(true).height(EVENT_ROW_H)));

    // Right column: quiet duration when finished; green live dot + "Running · Xs"
    // while running; red dot + "Failed · exit N" on error (using exact `p.failure`).
    match status {
        ToolStatus::Running => {
            let live_dot = canvas(|g, b| {
                let pal = theme::palette();
                style::dot(g, b.center(), 6.0, style::c(pal.success));
            })
            .paint_key(907)
            .w(px(8.0))
            .h(px(EVENT_ROW_H))
            .flex_shrink_0();
            let running_label = match duration {
                Some(d) => format!("{} · {d}", olo_kit::words::tr("Running")),
                None => olo_kit::words::tr("Running…").to_owned(),
            };
            header = header.child(div().w(px(8.0))).child(live_dot).child(div().w(px(4.0))).child(StringView::new(running_label).size(style::SMALL_SIZE).dim(true).height(EVENT_ROW_H));
        }
        ToolStatus::Error => {
            let err_dot = canvas(|g, b| {
                let pal = theme::palette();
                style::dot(g, b.center(), 6.0, style::c(pal.failure));
            })
            .paint_key(908)
            .w(px(8.0))
            .h(px(EVENT_ROW_H))
            .flex_shrink_0();
            let fail_label = match extract_exit_code(output) {
                Some(code) => format!("{} · exit {code}", olo_kit::words::tr("Failed")),
                None => olo_kit::words::tr("Failed").to_owned(),
            };
            header = header
                .child(div().w(px(8.0)))
                .child(err_dot)
                .child(div().w(px(4.0)))
                .child(StringView::new(fail_label).bold(true).size(style::SMALL_SIZE).color(style::c(p.failure)).height(EVENT_ROW_H));
        }
        ToolStatus::Ok => {
            if let Some(dur) = duration {
                header = header.child(div().w(px(8.0))).child(StringView::new(dur).size(style::SMALL_SIZE).dim(true).height(EVENT_ROW_H));
            }
        }
    }

    let mut col = div().w_full().flex().flex_col();
    if top_gap > 0.0 {
        col = col.pt(px(top_gap));
    }
    col = col.child(header);
    if open {
        let input_rows = format_tool_input(name, summary, raw_input);
        let out_clean = output.map(|s| s.trim_end_matches('\n')).filter(|s| !s.is_empty());
        let has_in = !input_rows.is_empty();
        let has_out = out_clean.is_some();
        let ink = style::c(p.panel_text);

        if has_in || has_out {
            let mut inner = div().w_full().flex().flex_col().gap(px(6.0));
            if has_in {
                let in_hits = field_match_ranges(matches, active_match_idx, eid, MatchField::ToolInput);
                if in_hits.is_empty() {
                    let mut kv_col = div().w_full().flex().flex_col().gap(px(3.0));
                    for (k, v) in &input_rows {
                        if k.is_empty() {
                            kv_col = kv_col.child(render_rich_text(v, true, false, MONO_SIZE, ink, dark, &[]));
                        } else {
                            kv_col = kv_col.child(
                                div()
                                    .w_full()
                                    .flex()
                                    .flex_row()
                                    .items_start()
                                    .gap(px(8.0))
                                    .child(StringView::new(format!("{k}:")).size(MONO_SIZE).dim(true).height(15.0))
                                    .child(div().flex_1().min_w_0().child(render_rich_text(v, true, false, MONO_SIZE, ink, dark, &[]))),
                            );
                        }
                    }
                    inner = inner.child(kv_col);
                } else {
                    let flat = olo_rex_client::activity::tool_input_display_text(name, summary, raw_input);
                    inner = inner.child(render_rich_text(&flat, true, false, MONO_SIZE, ink, dark, &in_hits));
                }
            }
            if has_in && has_out {
                let sep_c = if dark { style::c(olo_theme::mix(p.document_background, p.panel_text, 0.16)) } else { style::shade(p, p.panel_background, 1.24) };
                inner = inner.child(canvas(move |g, b| style::hline(g, b.left(), b.right(), b.top(), sep_c)).paint_key(style::key(("tr-well-sep", dark))).w_full().h(px(1.0)));
                inner = inner.child(StringView::new(olo_kit::words::tr("Output").to_owned()).size(style::SMALL_SIZE).dim(true).height(14.0));
            }
            if let Some(out) = out_clean {
                let out_hits = field_match_ranges(matches, active_match_idx, eid, MatchField::ToolOutput);
                let shown_out = if cap_err_preview && out_hits.is_empty() { cap_error_lines(out, 6) } else { out.to_owned() };
                inner = inner.child(render_rich_text(&shown_out, true, false, MONO_SIZE, ink, dark, &out_hits));
            }
            // 4 pt gap directly under its own latch row!
            col = col.child(sunken_well(p, dark, 4.0, None, inner));
        }
    }
    col
}

fn render_divider(p: &olo_theme::Palette, dark: bool, label: &str, timestamp: &str) -> Element {
    let line_c = if dark { style::c(olo_theme::mix(p.document_background, p.panel_text, 0.16)) } else { style::shade(p, p.panel_background, 1.24) };
    let rule = move || {
        canvas(move |g, b| {
            style::hline(g, b.left(), b.right(), b.center().y, line_c);
        })
        .paint_key(903)
        .flex_1()
        .h(px(14.0))
    };
    let text = if timestamp.is_empty() { label.to_owned() } else { format!("{label} · {timestamp}") };
    div().w_full().flex().flex_row().items_center().gap(px(m::TIGHT)).py(px(8.0)).child(rule()).child(StringView::new(text).size(style::SMALL_SIZE).dim(true).height(14.0)).child(rule())
}

/// Render the bounded safe read-only `tmux attach -r -f ignore-size` viewer:
/// centers the remote pane's exact `cols × rows` BeOS sunken well inside the
/// canvas when it fits, or pins it 12 pt top-left with scrollbars when larger
/// than the viewport (never resizing the remote session), with a neutral cursor.
#[allow(clippy::too_many_arguments)]
pub fn render_tmux_viewer(
    viewer: &Arc<Mutex<TmuxViewer>>,
    visual: &Rc<terminal_pane::PaneVisualState>,
    scroll: &ScrollHandle,
    avail_w: f32,
    avail_h: f32,
    active_theme: &TerminalTheme,
    metrics: &Metrics,
    on_do: impl Fn(&TranscriptDo, &mut WindowCx) + 'static,
) -> Element {
    let on_do: Arc<dyn Fn(&TranscriptDo, &mut WindowCx)> = Arc::new(on_do);
    let (target, title, error, remote_dims) = {
        let g = viewer.lock().unwrap_or_else(|e| e.into_inner());
        let dims = match &g.target {
            ViewTarget::SubagentTmux { target, .. } => (target.cols, target.rows),
            _ => g.terminal.size(),
        };
        (g.target.clone(), g.target.title(), g.error.clone(), dims)
    };

    let hdr_status = if error.is_some() {
        HeaderStatus::Failed
    } else if let ViewTarget::SubagentTmux { sub, .. } = &target {
        match sub.state {
            SubagentState::Failed => HeaderStatus::Failed,
            SubagentState::Done => HeaderStatus::Done,
            SubagentState::Working | SubagentState::Waiting => HeaderStatus::Working,
        }
    } else {
        HeaderStatus::Working
    };

    let header = viewer_header(
        "terminal",
        title.clone(),
        hdr_status,
        true,
        Some(("rex-tmux-ro-tip", "Attached with tmux attach -r -f ignore-size; the agent’s pane size is unchanged.")),
        true,
        target.has_transcript(),
        true,
        None,
        None,
        on_do,
    );

    if let Some(err) = error {
        return div().size_full().flex().flex_col().child(header).child(
            div()
                .flex_1()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(6.0))
                .child(StringView::new(olo_kit::words::tr("Read-only tmux attach unavailable").to_owned()).bold(true))
                .child(StringView::new(err).size(style::SMALL_SIZE).dim(true)),
        );
    }

    // Neutral tertiary grey cursor for read-only viewer (never focus-accent blue).
    let mut ro_theme = active_theme.clone();
    let fg = ro_theme.foreground;
    let bg = ro_theme.background;
    ro_theme.cursor = Some(olo_rex_vt::Rgb { r: ((fg.r as u16 + bg.r as u16) / 2) as u8, g: ((fg.g as u16 + bg.g as u16) / 2) as u8, b: ((fg.b as u16 + bg.b as u16) / 2) as u8 });

    let (cols, rows) = remote_dims;
    let pad = settings::PADDING;
    let grid_w = (cols as f32 * metrics.cell.0 + 2.0 * pad.0).ceil();
    let grid_h = (rows as f32 * metrics.cell.1 + 2.0 * pad.1).ceil();

    let v_paint = viewer.clone();
    let theme_paint = ro_theme.clone();
    let metrics_paint = metrics.clone();
    let vis_paint = visual.clone();
    let title_agent = title.clone();

    let screen = canvas(move |g, rect| {
        let mut guard = v_paint.lock().unwrap_or_else(|e| e.into_inner());
        let v_ref = &mut *guard;
        if v_ref.current_theme != theme_paint {
            let _ = v_ref.terminal.apply_theme(&theme_paint);
            v_ref.current_theme = theme_paint.clone();
        }
        let _ = v_ref.frame.capture(&mut v_ref.render_state, &v_ref.terminal);
        let _ = terminal_pane::paint_pane(
            g,
            rect,
            PanePaintArgs {
                term: &mut v_ref.terminal,
                frame: &v_ref.frame,
                theme: &theme_paint,
                metrics: &metrics_paint,
                focused: false,
                window_active: true,
                copy_mode: None,
                search_matches: &[],
                hover_link: None,
                preedit: None,
                predicted: None,
            },
            &vis_paint,
        );
        g.agent(olo_kit::agent::Node {
            role: "Terminal",
            name: Some(format!("Read-only tmux: {title_agent}").into()),
            bounds: rect,
            focused: false,
            enabled: true,
            clickable: false,
            ..Default::default()
        });
    })
    .w(px(grid_w))
    .h(px(grid_h))
    .flex_shrink_0();

    let well_frame = canvas(|g, b| {
        let pal = theme::palette();
        style::sunken(g, b, &pal, false);
    })
    .paint_key(909)
    .absolute()
    .inset_0();

    let box_w = grid_w + 4.0;
    let box_h = grid_h + 4.0;
    let framed_pane = div().relative().p(px(2.0)).w(px(box_w)).h(px(box_h)).flex_shrink_0().child(screen).child(well_frame);

    let p = theme::palette();
    let canvas_h = (avail_h - olo_kit::widgets::STRIP_HEIGHT).max(100.0);
    let need_h = box_w + 24.0 > avail_w;
    let need_v = box_h + 24.0 > canvas_h;

    let body = if !need_h && !need_v {
        // Fits comfortably: center the bounded remote screen in the panel canvas!
        div().size_full().bg(style::c(p.panel_background)).flex().items_center().justify_center().child(framed_pane)
    } else {
        // Larger than viewport: pin 12 pt top-left with scrollbars so nothing is clipped!
        let stage = div().p(px(12.0)).flex_shrink_0().child(framed_pane);
        ScrollView::new(scroll).vertical(need_v).horizontal(need_h).auto_hide(false).border(false).background(p.panel_background).child(stage).into_element()
    };

    let container_mark = canvas(|g, b| {
        olo_kit::review::container(g, b);
    })
    .paint_key(910)
    .absolute()
    .inset_0();

    div()
        .relative()
        .size_full()
        .flex()
        .flex_col()
        .agent("Group", "agent-tmux-ro")
        .agent_value(title)
        .child(container_mark)
        .child(header)
        .child(div().relative().flex_1().min_h_0().w_full().child(body))
}

/// Draw a small BeOS disclosure triangle (`▸` collapsed / `▾` expanded) snapped to device pixels.
pub fn paint_disclosure(g: &mut olo_kit::Painter, center: Point<Pixels>, expanded: bool, ink: Hsla) {
    let r = Bounds::new(point(center.x - px(5.0), center.y - px(5.0)), size(px(10.0), px(10.0)));
    style::latch(g, r, expanded, ink);
}

/// Return bounds of the disclosure hit target at the left of a parent Activity row.
pub fn disclosure_hit_rect(row_bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds::new(point(row_bounds.left(), row_bounds.top()), size(px(m::WINDOW_INSET + 4.0), px(38.0)))
}
