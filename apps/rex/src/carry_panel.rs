//! The Agent Settings conflict review window (`WinRole::Carry`):
//! shows all differences between this computer's carried agent settings
//! and a far host in one multi-select batch list (`⇧`, `⌘`, `⌘A`), lets
//! the person apply a strategy (`Copy`, `Skip`, `Merge`) and `Remember
//! choice` toggle to any subset via selection-scoped radio buttons,
//! aligns and highlights line-by-line differences side by side or as the
//! resulting file on the host, and lists and forgets remembered choices
//! in its `Remembered` tab.

use std::{collections::HashSet, rc::Rc};

use olo_kit::{
    Bounds, Element, FontWeight, Hsla, InteractiveElement as _, IntoElement as _, ParentElement as _, ScrollHandle, SharedString, Styled as _, WindowCx, canvas, div, icons, metrics as m, point, px,
    size, style, theme,
    widgets::{Button, CheckBox, Column, ColumnListView, ListModel, RadioButton, ScrollView, StringView, TabView, button_width, list_height, separator},
};
use olo_protocol::RexCarryChoice as Choice;
use olo_rex_client::carry::{DiffItem, DiffKind};

use crate::{
    carry::Shown,
    panels::{self, Light},
};

/// Window width: generous enough for crisp side-by-side aligned diffs and
/// full column headers without horizontal scrolling, while fitting on 1280 × 800.
pub const CARRY_WIDTH: f32 = 740.0;

const ICON: f32 = 32.0;
const NAME_SIZE: f32 = style::PLAIN_SIZE + 3.0;
const NAME_ROW: f32 = 20.0;
const STATE_ROW: f32 = 14.0;
/// Readable 11 pt monospace with 15 pt leading in diff/result wells.
const DIFF_MONO_SIZE: f32 = style::SMALL_SIZE;
const DIFF_LINE_H: f32 = 15.0;
const WELL_INSET: f32 = 8.0;
const DIFF_BAR_W: f32 = 3.0;
const SWATCH: f32 = 8.0;
/// Uniform gap between grouped buttons across the dialog.
const BTN_GAP: f32 = 8.0;
/// Height of `separator()` in `olo_kit::widgets`.
const SEP_H: f32 = 6.0;
/// Spacers above/below the separator so visual air from Select All's bottom
/// (0.5 + 13 + 2.5 = 16 pt) matches air to the radio tops (2.5 + 10 + 3.5 = 16 pt).
const SEP_ABOVE: f32 = 13.0;
const SEP_BELOW: f32 = 10.0;

/// Total width of list columns inside `CARRY_WIDTH` minus outer inset (32),
/// `TabView` panel inset + bevel (36), list frame (4), and scrollbar (14) = 654 pt.
const LIST_COLUMNS_W: f32 = 640.0;
const COL_ITEM_W: f32 = 340.0;
const COL_STATUS_W: f32 = 116.0;
const COL_ACTION_W: f32 = 108.0;
const COL_REM_W: f32 = LIST_COLUMNS_W - COL_ITEM_W - COL_STATUS_W - COL_ACTION_W;

/// Full-height list rows for the Remembered tab and resolved empty state so
/// every tab fills the same vertical frame with zero dead void.
const FULL_TAB_LIST_ROWS: usize = 19;

/// Actions emitted by the Carry review window.
#[derive(Clone, Debug, PartialEq)]
pub enum CarryDo {
    Tab(usize),
    Check,
    SelectAll,
    SetChoice(Choice),
    SetRemember(bool),
    SetPreviewResult(bool),
    Apply,
    Close,
    ForgetSelected,
    ForgetAll,
}

/// One row of the `Changes` column list.
#[derive(Clone, Debug, PartialEq)]
pub struct ChangeRow {
    pub id: String,
    pub display: String,
    pub status_light: Light,
    pub status: String,
    pub choice: Choice,
    pub choice_label: String,
    /// True when explicitly decided (or New / Remembered); false draws
    /// suggested action in italic secondary ink so undecided conflicts stand out.
    pub decided: bool,
    pub remember: bool,
    /// True when this item is already backed by a persisted remembered choice
    /// (drawn in secondary ink so newly toggled ticks match `Remember N`).
    pub already_remembered: bool,
    pub can_merge: bool,
}

/// One row of the `Remembered` column list.
#[derive(Clone, Debug, PartialEq)]
pub struct RememberedRow {
    pub id: String,
    pub display: String,
    pub choice_label: String,
    pub status_light: Light,
    pub status: String,
}

pub fn choice_label(choice: Choice) -> &'static str {
    match choice {
        Choice::TakeDesktop => olo_kit::words::tr("Copy"),
        Choice::KeepHost => olo_kit::words::tr("Skip"),
        Choice::Merge => olo_kit::words::tr("Merge"),
    }
}

fn display_for_id(id: &str) -> String {
    match id.split_once("::") {
        Some((file, item)) => format!("{file} ▸ {item}"),
        None => id.to_owned(),
    }
}

pub fn change_rows(shown: &Shown) -> Vec<ChangeRow> {
    use olo_kit::words::tr;
    shown
        .ordered_pending()
        .into_iter()
        .map(|it| {
            let choice = shown.choice_of(it);
            let (status_light, status) = if it.remembered.is_some() {
                (Light::On, tr("Remembered").to_owned())
            } else if it.kind == DiffKind::Conflict {
                (Light::Waiting, tr("Conflict").to_owned())
            } else {
                (Light::Off, tr("New").to_owned())
            };
            ChangeRow {
                id: it.id.clone(),
                display: it.display_name(),
                status_light,
                status,
                choice,
                choice_label: choice_label(choice).to_owned(),
                decided: shown.is_decided(it),
                remember: shown.remember_of(&it.id),
                already_remembered: it.remembered.is_some(),
                can_merge: it.can_merge,
            }
        })
        .collect()
}

pub fn remembered_rows(shown: &Shown) -> Vec<RememberedRow> {
    use olo_kit::words::tr;
    shown
        .remembered
        .iter()
        .map(|rem| {
            // Green dot for remembered rules that differ and will update on Apply;
            // neutral grey dot when the host already matches.
            let (status_light, status) = match &shown.plan {
                None => (Light::Off, "—".to_owned()),
                Some(plan) => match plan.items.iter().find(|it| it.id == rem.item) {
                    Some(it) if !it.resolved_same => (Light::On, tr("Differs").to_owned()),
                    _ => (Light::Off, tr("Same").to_owned()),
                },
            };
            RememberedRow { id: rem.item.clone(), display: display_for_id(&rem.item), choice_label: choice_label(rem.choice).to_owned(), status_light, status }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Line-by-line aligned diff & provenance highlighting
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineOrigin {
    Both,
    Here,
    There,
    Filler,
}

#[derive(Clone, Debug)]
struct AlignedRow {
    left: Option<String>,
    left_origin: LineOrigin,
    right: Option<String>,
    right_origin: LineOrigin,
}

/// Soft-wrap a single monospace line to `cols` characters, indenting wrapped
/// continuation lines by 2 spaces so long lines stay readable with zero
/// horizontal scrolling.
fn wrap_mono_line(line: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(16);
    if line.chars().count() <= cols {
        return vec![line.to_owned()];
    }
    let mut out = Vec::new();
    let mut rest = line;
    let mut first = true;
    while !rest.is_empty() {
        let budget = if first { cols } else { cols.saturating_sub(2).max(12) };
        let count = rest.chars().count();
        if count <= budget {
            out.push(if first { rest.to_owned() } else { format!("  {rest}") });
            break;
        }
        // Prefer breaking on whitespace within the second half of the budget.
        let mut byte_end = rest.char_indices().nth(budget).map_or(rest.len(), |(i, _)| i);
        if let Some(sp) = rest[..byte_end].rfind(' ')
            && sp >= byte_end / 2
        {
            byte_end = sp + 1;
        }
        let chunk = rest[..byte_end].trim_end_matches(' ');
        out.push(if first { chunk.to_owned() } else { format!("  {chunk}") });
        rest = rest[byte_end..].trim_start_matches(' ');
        first = false;
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Estimate monospace columns that fit inside a preview well of logical width `well_w`
/// (with sunken 2 pt frame on both sides, permanent 14 pt vertical scrollbar, and
/// 8 pt left and right text insets).
fn mono_cols(well_w: f32) -> usize {
    let ten = f32::from(style::text_width("0000000000", DIFF_MONO_SIZE, style::mono_font(FontWeight::NORMAL)));
    let cw = (ten / 10.0).max(5.5);
    let inner = well_w - 4.0 - olo_kit::widgets::SCROLL_BAR - 2.0 * WELL_INSET;
    ((inner / cw).floor() as usize).max(24)
}

/// Compute line-by-line LCS alignment between `here` and `there`, then expand
/// wrapped visual rows in lockstep so matching lines share the exact same Y
/// baseline in both side-by-side panes.
fn align_diff(here: &str, there: Option<&str>, cols: usize) -> Vec<AlignedRow> {
    let a: Vec<&str> = if here.is_empty() { Vec::new() } else { here.lines().collect() };
    let b: Vec<&str> = match there {
        Some(t) if !t.is_empty() => t.lines().collect(),
        _ => Vec::new(),
    };

    // Expand one logical pair of lines into synchronized wrapped visual rows.
    let push_pair = |out: &mut Vec<AlignedRow>, l: Option<&str>, lo: LineOrigin, r: Option<&str>, ro: LineOrigin| {
        let lw = l.map(|s| wrap_mono_line(s, cols)).unwrap_or_default();
        let rw = r.map(|s| wrap_mono_line(s, cols)).unwrap_or_default();
        let n = lw.len().max(rw.len()).max(1);
        for k in 0..n {
            out.push(AlignedRow {
                left: lw.get(k).cloned(),
                left_origin: if k < lw.len() { lo } else { LineOrigin::Filler },
                right: rw.get(k).cloned(),
                right_origin: if k < rw.len() { ro } else { LineOrigin::Filler },
            });
        }
    };

    let mut rows = Vec::new();
    if there.is_none() {
        for line in &a {
            push_pair(&mut rows, Some(line), LineOrigin::Here, None, LineOrigin::Filler);
        }
        return rows;
    }

    // Standard LCS DP for normal settings/instruction sizes (<= 300 lines per side).
    if a.len() <= 300 && b.len() <= 300 {
        let (m, n) = (a.len(), b.len());
        let mut dp = vec![vec![0u32; n + 1]; m + 1];
        for i in (0..m).rev() {
            for j in (0..n).rev() {
                dp[i][j] = if a[i] == b[j] { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
            }
        }
        let (mut i, mut j) = (0usize, 0usize);
        let mut del_buf: Vec<&str> = Vec::new();
        let mut add_buf: Vec<&str> = Vec::new();
        let flush_hunk = |rows: &mut Vec<AlignedRow>, del: &mut Vec<&str>, add: &mut Vec<&str>| {
            let k = del.len().max(add.len());
            for idx in 0..k {
                let l = del.get(idx).copied();
                let r = add.get(idx).copied();
                push_pair(rows, l, if l.is_some() { LineOrigin::Here } else { LineOrigin::Filler }, r, if r.is_some() { LineOrigin::There } else { LineOrigin::Filler });
            }
            del.clear();
            add.clear();
        };
        while i < m && j < n {
            if a[i] == b[j] {
                flush_hunk(&mut rows, &mut del_buf, &mut add_buf);
                push_pair(&mut rows, Some(a[i]), LineOrigin::Both, Some(b[j]), LineOrigin::Both);
                i += 1;
                j += 1;
            } else if dp[i + 1][j] >= dp[i][j + 1] {
                del_buf.push(a[i]);
                i += 1;
            } else {
                add_buf.push(b[j]);
                j += 1;
            }
        }
        while i < m {
            del_buf.push(a[i]);
            i += 1;
        }
        while j < n {
            add_buf.push(b[j]);
            j += 1;
        }
        flush_hunk(&mut rows, &mut del_buf, &mut add_buf);
    } else {
        let k = a.len().max(b.len());
        for idx in 0..k {
            let l = a.get(idx).copied();
            let r = b.get(idx).copied();
            if l == r {
                push_pair(&mut rows, l, LineOrigin::Both, r, LineOrigin::Both);
            } else {
                push_pair(&mut rows, l, if l.is_some() { LineOrigin::Here } else { LineOrigin::Filler }, r, if r.is_some() { LineOrigin::There } else { LineOrigin::Filler });
            }
        }
    }
    rows
}

/// Lines for the `Result on {host}` view with exact per-line provenance.
fn result_lines(item: &DiffItem, choice: Choice, cols: usize) -> Vec<(String, LineOrigin)> {
    let here_set: HashSet<&str> = item.here.lines().map(str::trim).collect();
    let there_set: HashSet<&str> = item.there.as_deref().unwrap_or("").lines().map(str::trim).collect();
    let raw: Vec<(&str, LineOrigin)> = match choice {
        Choice::TakeDesktop => item
            .here
            .lines()
            .map(|l| {
                let origin = if item.there.is_some() && there_set.contains(l.trim()) { LineOrigin::Both } else { LineOrigin::Here };
                (l, origin)
            })
            .collect(),
        Choice::KeepHost => match &item.there {
            Some(t) => t
                .lines()
                .map(|l| {
                    let origin = if here_set.contains(l.trim()) { LineOrigin::Both } else { LineOrigin::There };
                    (l, origin)
                })
                .collect(),
            None => Vec::new(),
        },
        Choice::Merge => {
            let text = item.merged_preview.as_deref().unwrap_or(&item.here);
            text.lines()
                .map(|l| {
                    let in_h = here_set.contains(l.trim());
                    let in_t = there_set.contains(l.trim());
                    let origin = match (in_h, in_t) {
                        (true, true) => LineOrigin::Both,
                        (true, false) => LineOrigin::Here,
                        (false, true) => LineOrigin::There,
                        (false, false) => LineOrigin::Both,
                    };
                    (l, origin)
                })
                .collect()
        }
    };
    let mut out = Vec::new();
    for (line, origin) in raw {
        for wrapped in wrap_mono_line(line, cols) {
            out.push((wrapped, origin));
        }
    }
    out
}

/// Wash background and 3 pt left accent bar colors for each diff line origin.
fn diff_colors(p: &olo_theme::Palette, origin: LineOrigin) -> (Option<Hsla>, Option<Hsla>) {
    let dark = p.mode == olo_theme::ThemeMode::Dark;
    match origin {
        LineOrigin::Both => (None, None),
        LineOrigin::Here => {
            let blue = p.keyboard_navigation;
            let wash = if dark { olo_theme::mix(blue, p.document_background, 0.80) } else { olo_theme::mix(p.list_selected_background, p.document_background, 0.40) };
            let bar = if dark { olo_theme::mix(blue, olo_theme::WHITE, 0.20) } else { blue };
            (Some(style::c(wash)), Some(style::c(bar)))
        }
        LineOrigin::There => {
            let yellow = p.hue(olo_theme::Hue::Yellow);
            let wash = if dark {
                // Warm amber wash (~#4D3E23) matching the dark blue wash's saturation and lightness.
                olo_theme::mix([224, 152, 12, 255], p.document_background, 0.76)
            } else {
                olo_theme::mix(yellow, p.document_background, 0.80)
            };
            let bar = if dark { yellow } else { olo_theme::mix(yellow, p.warning, 0.45) };
            (Some(style::c(wash)), Some(style::c(bar)))
        }
        LineOrigin::Filler => {
            let filler = olo_theme::mix(p.panel_background, p.document_background, if dark { 0.55 } else { 0.45 });
            (Some(style::c(filler)), None)
        }
    }
}

/// 8×8 pt colour swatch before each pane's label as the diff legend.
fn legend_swatch(origin: LineOrigin) -> Element {
    canvas(move |g, b| {
        let p = theme::palette();
        let (wash, bar) = diff_colors(&p, origin);
        if let Some(w) = wash {
            style::fill_rect(g, b, w);
        }
        if let Some(accent) = bar {
            let left = Bounds::from_corners(b.origin, point(b.left() + px(2.5), b.bottom()));
            style::fill_rect(g, left, accent);
            style::frame_rect(g, b, accent.opacity(0.65));
        }
    })
    .paint_key(style::key(("carry-swatch", origin as u8)))
    .w(px(SWATCH))
    .h(px(SWATCH))
    .flex_shrink_0()
}

fn diff_well(scroll: &ScrollHandle, other: Option<ScrollHandle>, height: f32, rows: Vec<(Option<String>, LineOrigin)>, empty_note: Option<String>) -> Element {
    let p = theme::palette();
    let content = if rows.is_empty() {
        let note = empty_note.unwrap_or_default();
        div().w_full().px(px(WELL_INSET)).py(px(6.0)).child(StringView::new(note).size(DIFF_MONO_SIZE).dim(true).wrap(true))
    } else {
        let mut col = div().w_full().flex().flex_col().py(px(4.0));
        for (line, origin) in rows {
            let (wash, bar) = diff_colors(&p, origin);
            let text = line.unwrap_or_default();
            let mut row = div().relative().w_full().h(px(DIFF_LINE_H)).flex().flex_row().items_center();
            if let Some(bg) = wash {
                row = row.bg(bg);
            }
            if let Some(accent) = bar {
                row = row.child(div().w(px(DIFF_BAR_W)).h_full().bg(accent).flex_shrink_0());
            } else {
                row = row.child(div().w(px(DIFF_BAR_W)).h_full().flex_shrink_0());
            }
            if !text.is_empty() {
                row = row.child(div().flex_1().min_w_0().pl(px(WELL_INSET - DIFF_BAR_W)).pr(px(WELL_INSET)).child(StringView::new(text).mono(true).size(DIFF_MONO_SIZE).height(DIFF_LINE_H)));
            }
            col = col.child(row);
        }
        col
    };
    let mut scroller = ScrollView::new(scroll).vertical(true).auto_hide(false).background(p.document_background);
    if let Some(peer) = other {
        scroller = scroller.on_scroll(move |off, cx| {
            if (f32::from(peer.offset().y) - f32::from(off.y)).abs() > 0.5 {
                peer.set_offset(cx, point(peer.offset().x, off.y));
            }
        });
    }
    div().w_full().h(px(height)).child(scroller.child(content))
}

fn head_title_and_status(shown: &Shown) -> (String, Option<Light>, bool, String) {
    use olo_kit::words::{count, tr};
    if shown.checking {
        return (olo_kit::tr!("Checking “{}”…", shown.name).to_string(), None, true, tr("Comparing agent settings…").to_owned());
    }
    if shown.applying {
        return (olo_kit::tr!("Updating “{}”…", shown.name).to_string(), None, true, tr("Applying settings…").to_owned());
    }
    if let Some(err) = &shown.error {
        return (olo_kit::tr!("Agent Settings on “{}”", shown.name).to_string(), Some(Light::Down), false, err.clone());
    }
    let Some(plan) = &shown.plan else {
        return (olo_kit::tr!("Agent Settings on “{}”", shown.name).to_string(), Some(Light::Off), false, "—".to_owned());
    };
    let pending = shown.ordered_pending();
    let s = plan.summary();
    if pending.is_empty() {
        let status = if !shown.remembered.is_empty() {
            count(shown.remembered.len() as u64, "1 remembered choice", "{n} remembered choices")
        } else {
            count(s.same_files as u64, "1 setting checked", "{n} settings checked")
        };
        return (tr("Same as this computer").to_owned(), Some(Light::On), false, status);
    }

    let title = count(pending.len() as u64, "1 setting differs from this computer", "{n} settings differ from this computer");
    let (total_conf, decided_conf) = shown.conflict_progress();
    let mut parts = Vec::new();
    if total_conf > 0 {
        let base = count(total_conf as u64, "1 conflict", "{n} conflicts");
        if decided_conf > 0 {
            parts.push(olo_kit::tr!("{}, {} decided", base, decided_conf).to_string());
        } else {
            parts.push(base);
        }
    }
    if s.new_items > 0 {
        parts.push(count(s.new_items as u64, "1 new", "{n} new"));
    }
    if s.auto_updates > 0 {
        parts.push(count(s.auto_updates as u64, "1 remembered", "{n} remembered"));
    }
    if s.same_files > 0 {
        parts.push(olo_kit::tr!("{} same", s.same_files).to_string());
    }
    let light = if total_conf > 0 && decided_conf < total_conf { Light::Waiting } else { Light::On };
    (title, Some(light), false, parts.join("  ·  "))
}

fn foot_summary(shown: &Shown) -> (Option<Light>, bool, String) {
    use olo_kit::words::{count, tr};
    if shown.applying {
        return (None, true, tr("Applying settings…").to_owned());
    }
    if let Some(err) = &shown.error {
        return (Some(Light::Down), false, err.clone());
    }
    if let Some(note) = &shown.applied_note {
        // The header already carries the green status dot in the resolved state.
        return (None, false, note.clone());
    }
    let pending = shown.ordered_pending();
    if pending.is_empty() {
        return (None, false, tr("Same as this computer.").to_owned());
    }
    let (mut copy_n, mut merge_n, mut skip_n) = (0usize, 0usize, 0usize);
    for it in &pending {
        match shown.choice_of(it) {
            Choice::TakeDesktop => copy_n += 1,
            Choice::Merge => merge_n += 1,
            Choice::KeepHost => skip_n += 1,
        }
    }
    let mut parts = Vec::new();
    if copy_n > 0 {
        parts.push(olo_kit::tr!("Copy {}", copy_n).to_string());
    }
    if merge_n > 0 {
        parts.push(olo_kit::tr!("Merge {}", merge_n).to_string());
    }
    if skip_n > 0 {
        parts.push(olo_kit::tr!("Skip {}", skip_n).to_string());
    }
    let rem_saves = shown.remember_edits().iter().filter(|(_, v)| v.is_some()).count();
    if rem_saves > 0 {
        parts.push(count(rem_saves as u64, "Remember 1 choice", "Remember {n} choices"));
    }
    (None, false, parts.join("  ·  "))
}

#[allow(clippy::too_many_arguments)]
pub fn render_carry_panel(
    cx: &olo_kit::Cx<crate::ui::RexWindow>,
    shown: &Shown,
    changes_list: &ListModel,
    changes_rows: &Rc<Vec<ChangeRow>>,
    remembered_list: &ListModel,
    remembered_rows: &Rc<Vec<RememberedRow>>,
    here_scroll: &ScrollHandle,
    there_scroll: &ScrollHandle,
    on_changes_select: impl Fn(&(), &mut WindowCx) + 'static,
    on_rem_select: impl Fn(&(), &mut WindowCx) + 'static,
    on_do: impl Fn(&CarryDo, &mut WindowCx) + 'static,
) -> Element {
    let on_do: Rc<dyn Fn(&CarryDo, &mut WindowCx)> = Rc::new(on_do);
    let busy = shown.checking || shown.applying;
    let resolved = !shown.checking && shown.plan.is_some() && changes_rows.is_empty();
    let std_btn_w = button_width("Check Again", false).max(m::BUTTON_MIN_WIDTH);

    // 1. Header: distinct settings icon, situation-stating title, conflict progress line.
    let (head_title, head_light, head_busy, head_text) = head_title_and_status(shown);
    let head_lamp = if head_busy { panels::busy_of(panels::LIGHT) } else { panels::light(head_light.unwrap_or(Light::Off)) };
    let head = div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::SPACING))
        .child(
            canvas(|g, b| {
                icons::paint(g, b, "preferences");
            })
            .paint_key(0)
            .w(px(ICON))
            .h(px(ICON))
            .flex_shrink_0(),
        )
        .child(
            div().flex_1().min_w_0().h(px(NAME_ROW + STATE_ROW)).flex().flex_col().child(StringView::new(head_title).bold(true).size(NAME_SIZE).cut(true).height(NAME_ROW)).child(
                div()
                    .w_full()
                    .h(px(STATE_ROW))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(m::TIGHT))
                    .child(head_lamp)
                    .child(div().flex_1().min_w_0().child(StringView::new(head_text).size(style::SMALL_SIZE).dim(true).cut(true).height(STATE_ROW))),
            ),
        );

    let rem_list_h = list_height(FULL_TAB_LIST_ROWS, m::LIST_ROW, true);
    let tab_body_h = rem_list_h + m::SPACING + m::CONTROL_HEIGHT;

    // 2. Tab 0: Changes (full clean resolved view when empty, else batch list + radio bar + aligned diff preview).
    let changes_page = if resolved {
        let p = theme::palette();
        let back = canvas(|g, b| {
            let p = theme::palette();
            style::fill_rect(g, b, style::c(p.document_background));
            style::sunken(g, b, &p, false);
        })
        .paint_key(0)
        .absolute()
        .inset_0();
        div()
            .relative()
            .w_full()
            .h(px(tab_body_h))
            .flex()
            .items_center()
            .justify_center()
            .child(back)
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.0))
                    .child(StringView::new(olo_kit::words::tr("Nothing to review").to_owned()).size(style::PLAIN_SIZE).color(style::c(p.document_text)).height(16.0))
                    .child(StringView::new(olo_kit::tr!("Settings that exist only on {} are left alone.", shown.name).to_string()).size(style::SMALL_SIZE).dim(true).height(STATE_ROW)),
            )
            .into_element()
    } else {
        let sel_indices: Vec<usize> = changes_list.state().selected.iter().copied().filter(|&i| i < changes_rows.len()).collect();
        let sel_count = sel_indices.len();
        let all_can_merge = sel_count > 0 && sel_indices.iter().all(|&i| changes_rows.get(i).is_some_and(|r| r.can_merge));
        let common_choice: Option<Choice> = sel_indices.first().and_then(|&first| {
            let c = changes_rows.get(first)?.choice;
            sel_indices.iter().all(|&i| changes_rows.get(i).is_some_and(|r| r.choice == c)).then_some(c)
        });
        let rem_on = sel_indices.iter().filter(|&&i| changes_rows.get(i).is_some_and(|r| r.remember)).count();
        let rem_all = sel_count > 0 && rem_on == sel_count;
        let rem_mixed = rem_on > 0 && rem_on < sel_count;

        let focused_idx = changes_list.state().cursor().filter(|&i| i < changes_rows.len()).or_else(|| sel_indices.first().copied()).or_else(|| (!changes_rows.is_empty()).then_some(0));
        let focused_item = focused_idx.and_then(|i| changes_rows.get(i)).and_then(|r| shown.plan.as_ref().and_then(|p| p.items.iter().find(|it| it.id == r.id)));

        let changes_view = ColumnListView::new(
            cx,
            "rex-carry-changes",
            changes_list,
            vec![Column::new("Item", COL_ITEM_W), Column::new("Status", COL_STATUS_W), Column::new("Action", COL_ACTION_W), Column::new("Remember", COL_REM_W).center()],
            changes_rows.len(),
            |v: &crate::ui::RexWindow, cell, g| {
                let Some(r) = v.carry_changes_rows.get(cell.row) else { return };
                let p = theme::palette();
                match cell.column {
                    0 => cell.text(g, r.display.clone()),
                    1 => panels::lit_cell(g, cell, r.status_light, &r.status),
                    2 => {
                        let (font, color) = if r.decided {
                            (style::ui_font(FontWeight::NORMAL), cell.text_color)
                        } else if cell.selected {
                            (style::ui_font(FontWeight::NORMAL).italic(), cell.text_color)
                        } else {
                            (style::ui_font(FontWeight::NORMAL).italic(), style::secondary_text(&p))
                        };
                        cell.text_with(g, r.choice_label.clone(), font, color);
                    }
                    _ => {
                        if r.remember {
                            // Drawn native BeOS checkbox tick centred in cell; already-remembered
                            // items use secondary ink so newly toggled ticks match "Remember N".
                            let c = cell.bounds.center();
                            let mark = Bounds::centered_at(c, size(px(8.5), px(7.5)));
                            let ink = if !r.already_remembered {
                                cell.text_color
                            } else if cell.selected {
                                cell.text_color.opacity(0.65)
                            } else {
                                style::secondary_text(&p)
                            };
                            style::check_mark(g, mark, ink);
                            cell.say("Remembered");
                        }
                    }
                }
            },
        )
        .row_key(|v: &crate::ui::RexWindow, row| v.carry_changes_rows.get(row).map_or(0, |r| style::key((&r.id, r.status_light, &r.status, r.choice, r.decided, r.remember, r.already_remembered))))
        .empty_text("Checking differences…")
        .on_select(on_changes_select);
        let changes_list_h = list_height(7, m::LIST_ROW, true);
        let changes_list_box = div().w_full().h(px(changes_list_h)).flex_shrink_0().child(changes_view);

        // Selection-scoped batch bar: "{n} selected: ◉ Copy  ○ Skip  ○ Merge    [✓] Remember choice      [Select All]"
        let on_copy = on_do.clone();
        let on_skip = on_do.clone();
        let on_merge = on_do.clone();
        let on_rem = on_do.clone();
        let on_all = on_do.clone();
        let sel_label = olo_kit::tr!("{} selected:", sel_count).to_string();
        let batch_bar = div()
            .w_full()
            .h(px(m::CONTROL_HEIGHT))
            .flex()
            .flex_row()
            .items_center()
            .child(StringView::new(sel_label).dim(true).height(m::CONTROL_HEIGHT))
            .child(div().w(px(12.0)))
            .child(
                RadioButton::new(olo_kit::words::tr("Copy").to_string(), common_choice == Some(Choice::TakeDesktop))
                    .id("carry-action-copy")
                    .height(m::CONTROL_HEIGHT)
                    .enabled(sel_count > 0 && !busy)
                    .on_select(move |_, cx| on_copy(&CarryDo::SetChoice(Choice::TakeDesktop), cx)),
            )
            .child(div().w(px(16.0)))
            .child(
                RadioButton::new(olo_kit::words::tr("Skip").to_string(), common_choice == Some(Choice::KeepHost))
                    .id("carry-action-skip")
                    .height(m::CONTROL_HEIGHT)
                    .enabled(sel_count > 0 && !busy)
                    .on_select(move |_, cx| on_skip(&CarryDo::SetChoice(Choice::KeepHost), cx)),
            )
            .child(div().w(px(16.0)))
            .child(
                RadioButton::new(olo_kit::words::tr("Merge").to_string(), common_choice == Some(Choice::Merge))
                    .id("carry-action-merge")
                    .height(m::CONTROL_HEIGHT)
                    .enabled(all_can_merge && !busy)
                    .on_select(move |_, cx| on_merge(&CarryDo::SetChoice(Choice::Merge), cx)),
            )
            .child(div().w(px(24.0)))
            .child(
                CheckBox::new(olo_kit::words::tr("Remember choice").to_string(), rem_all)
                    .id("carry-remember")
                    .mixed(rem_mixed)
                    .height(m::CONTROL_HEIGHT)
                    .enabled(sel_count > 0 && !busy)
                    .on_toggle(move |checked, cx| on_rem(&CarryDo::SetRemember(*checked), cx)),
            )
            .child(div().flex_1())
            .child(
                Button::new(olo_kit::words::tr("Select All").to_string())
                    .id("carry-select-all")
                    .min_width(std_btn_w)
                    .enabled(changes_rows.len() > 1 && !busy)
                    .on_click(move |_, cx| on_all(&CarryDo::SelectAll, cx)),
            );

        // Stretch the diff wells so Changes, Remembered, and Resolved fill the exact same
        // tab body height (uniform 16 pt bottom inset inside TabView, zero jump on tab switch).
        let preview_h = (tab_body_h - changes_list_h - m::SPACING - m::CONTROL_HEIGHT - SEP_ABOVE - SEP_H - SEP_BELOW - m::MARK_ROW - m::TIGHT - STATE_ROW - 4.0).max(150.0);

        // Preview header + aligned side-by-side diff OR Result on {host} (with provenance tints).
        let preview_section = match focused_item {
            Some(item) => {
                let on_both = on_do.clone();
                let on_res = on_do.clone();
                let both_label = olo_kit::words::tr("Both versions");
                let res_label = olo_kit::words::tr("Result");
                let here_label = olo_kit::words::tr("This computer");
                let radio_w = |s: &str| m::MARK_SIZE + m::MARK_GAP + f32::from(style::text_width(s.to_owned(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL))).ceil();
                let legend_w = |s: &str| SWATCH + 6.0 + f32::from(style::text_width(s.to_owned(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL))).ceil();
                let col1_w = radio_w(both_label).max(legend_w(here_label));
                let col2_w = radio_w(res_label).max(legend_w(&shown.name).min(130.0));
                let preview_head = div()
                    .w_full()
                    .h(px(m::MARK_ROW))
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(div().flex_1().min_w_0().pr(px(m::TIGHT)).child(StringView::new(item.display_name()).bold(true).size(style::PLAIN_SIZE).cut(true).height(m::MARK_ROW)))
                    .child(
                        div().w(px(col1_w)).flex_shrink_0().flex().flex_row().items_center().child(
                            RadioButton::new(both_label.to_string(), !shown.preview_result)
                                .id("carry-view-both")
                                .height(m::MARK_ROW)
                                .on_select(move |_, cx| on_both(&CarryDo::SetPreviewResult(false), cx)),
                        ),
                    )
                    .child(div().w(px(16.0)).flex_shrink_0())
                    .child(div().w(px(col2_w)).flex_shrink_0().flex().flex_row().items_center().child(
                        RadioButton::new(res_label.to_string(), shown.preview_result).id("carry-view-result").height(m::MARK_ROW).on_select(move |_, cx| on_res(&CarryDo::SetPreviewResult(true), cx)),
                    ));

                let inner_w = CARRY_WIDTH - 4.0 * m::WINDOW_INSET - 4.0;
                let wells = if shown.preview_result {
                    let choice = shown.choice_of(item);
                    let cols = mono_cols(inner_w);
                    let rlines: Vec<(Option<String>, LineOrigin)> = result_lines(item, choice, cols).into_iter().map(|(s, o)| (Some(s), o)).collect();
                    let empty_note = if choice == Choice::KeepHost && item.there.is_none() { Some(olo_kit::tr!("Skipped — not copied to {}.", shown.name).to_string()) } else { None };
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            div()
                                .w_full()
                                .h(px(STATE_ROW))
                                .flex()
                                .flex_row()
                                .items_center()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .pr(px(m::TIGHT))
                                        .child(StringView::new(olo_kit::tr!("Result on {}", shown.name).to_string()).size(style::SMALL_SIZE).dim(true).cut(true).height(STATE_ROW)),
                                )
                                .child(
                                    div()
                                        .w(px(col1_w))
                                        .flex_shrink_0()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(legend_swatch(LineOrigin::Here))
                                        .child(StringView::new(here_label.to_owned()).size(style::SMALL_SIZE).dim(true).height(STATE_ROW)),
                                )
                                .child(div().w(px(16.0)).flex_shrink_0())
                                .child(
                                    div()
                                        .w(px(col2_w))
                                        .flex_shrink_0()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(legend_swatch(LineOrigin::There))
                                        .child(div().flex_1().min_w_0().child(StringView::new(shown.name.clone()).size(style::SMALL_SIZE).dim(true).cut(true).height(STATE_ROW))),
                                ),
                        )
                        .child(diff_well(here_scroll, None, preview_h, rlines, empty_note))
                        .into_element()
                } else {
                    let side_w = (inner_w - m::SPACING) / 2.0;
                    let cols = mono_cols(side_w);
                    let aligned = align_diff(&item.here, item.there.as_deref(), cols);
                    let left_rows: Vec<(Option<String>, LineOrigin)> = aligned.iter().map(|r| (r.left.clone(), r.left_origin)).collect();
                    let right_rows: Vec<(Option<String>, LineOrigin)> = if item.there.is_some() { aligned.iter().map(|r| (r.right.clone(), r.right_origin)).collect() } else { Vec::new() };
                    let there_empty = Some(olo_kit::tr!("Not on {} yet.", shown.name).to_string());
                    div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .gap(px(m::SPACING))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .w_full()
                                        .h(px(STATE_ROW))
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(legend_swatch(LineOrigin::Here))
                                        .child(StringView::new(here_label.to_owned()).size(style::SMALL_SIZE).dim(true).height(STATE_ROW)),
                                )
                                .child(diff_well(here_scroll, Some(there_scroll.clone()), preview_h, left_rows, None)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .w_full()
                                        .h(px(STATE_ROW))
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(legend_swatch(LineOrigin::There))
                                        .child(div().flex_1().min_w_0().child(StringView::new(shown.name.clone()).size(style::SMALL_SIZE).dim(true).cut(true).height(STATE_ROW))),
                                )
                                .child(diff_well(there_scroll, Some(here_scroll.clone()), preview_h, right_rows, there_empty)),
                        )
                        .into_element()
                };
                div().w_full().flex().flex_col().gap(px(m::TIGHT)).child(preview_head).child(wells)
            }
            None => div().w_full().h(px(preview_h + m::MARK_ROW + STATE_ROW + m::TIGHT + 4.0)),
        };

        div()
            .w_full()
            .flex()
            .flex_col()
            .child(changes_list_box)
            .child(div().h(px(m::SPACING)))
            .child(batch_bar)
            .child(div().h(px(SEP_ABOVE)))
            .child(separator())
            .child(div().h(px(SEP_BELOW)))
            .child(preview_section)
            .into_element()
    };

    // 3. Tab 1: Remembered choices — exact same column positions as Changes, full-height list, single caption.
    let rem_sel: Vec<usize> = remembered_list.state().selected.iter().copied().filter(|&i| i < remembered_rows.len()).collect();
    let rem_view = ColumnListView::new(
        cx,
        "rex-carry-remembered",
        remembered_list,
        vec![Column::new("Item", COL_ITEM_W), Column::new("Status", COL_STATUS_W), Column::new("Action", LIST_COLUMNS_W - COL_ITEM_W - COL_STATUS_W)],
        remembered_rows.len(),
        |v: &crate::ui::RexWindow, cell, g| {
            let Some(r) = v.carry_rem_rows.get(cell.row) else { return };
            match cell.column {
                0 => cell.text(g, r.display.clone()),
                1 => panels::lit_cell(g, cell, r.status_light, &r.status),
                _ => cell.text(g, r.choice_label.clone()),
            }
        },
    )
    .row_key(|v: &crate::ui::RexWindow, row| v.carry_rem_rows.get(row).map_or(0, |r| style::key((&r.id, &r.choice_label, r.status_light, &r.status))))
    .empty_text("No remembered choices for this host.")
    .on_select(on_rem_select);
    let rem_list_h_px = px(rem_list_h);
    let on_forget = on_do.clone();
    let on_forget_all = on_do.clone();
    let rem_bar = div()
        .w_full()
        .h(px(m::CONTROL_HEIGHT))
        .flex()
        .flex_row()
        .items_center()
        .child(
            Button::new(olo_kit::words::tr("Forget").to_string())
                .id("carry-forget")
                .min_width(std_btn_w)
                .enabled(!rem_sel.is_empty() && !busy)
                .on_click(move |_, cx| on_forget(&CarryDo::ForgetSelected, cx)),
        )
        .child(div().w(px(BTN_GAP)))
        .child(
            Button::new(olo_kit::words::tr("Forget All").to_string())
                .id("carry-forget-all")
                .min_width(std_btn_w)
                .enabled(!remembered_rows.is_empty() && !busy)
                .on_click(move |_, cx| on_forget_all(&CarryDo::ForgetAll, cx)),
        )
        .child(div().w(px(16.0)))
        .child(
            div().flex_1().min_w_0().child(
                StringView::new(olo_kit::words::tr("Remembered choices apply without asking. Forget one to be asked again.").to_owned())
                    .size(style::SMALL_SIZE)
                    .beside(style::PLAIN_SIZE)
                    .dim(true)
                    .cut(true)
                    .height(m::CONTROL_HEIGHT),
            ),
        );
    let rem_page = div().w_full().flex().flex_col().gap(px(m::SPACING)).child(div().w_full().h(rem_list_h_px).child(rem_view)).child(rem_bar).into_element();

    let on_tab = on_do.clone();
    let rem_tab_title: SharedString = if shown.remembered.is_empty() { olo_kit::words::tr("Remembered").into() } else { olo_kit::tr!("Remembered ({})", shown.remembered.len()).into() };
    let tabs = TabView::new("rex-carry-tabs", vec![SharedString::from(olo_kit::words::tr("Changes")), rem_tab_title], shown.tab)
        .pages([changes_page, rem_page])
        .grows()
        .on_select(move |i, cx| on_tab(&CarryDo::Tab(*i), cx));

    // 4. Footer: Check Again bottom-left (as in Account), right-aligned plan/result caption, and Cancel+Apply (or default Close when resolved).
    let (foot_light, foot_busy, foot_text) = foot_summary(shown);
    let can_apply = shown.can_apply();
    let on_check = on_do.clone();
    let on_close = on_do.clone();
    let on_apply = on_do.clone();
    let check_btn = Button::new(olo_kit::words::tr("Check Again").to_string()).id("carry-check").min_width(std_btn_w).enabled(!busy).on_click(move |_, cx| on_check(&CarryDo::Check, cx));

    let plan_right = foot_light.is_none() && !foot_busy;
    let foot_label = if plan_right {
        StringView::new(foot_text).size(style::SMALL_SIZE).beside(style::PLAIN_SIZE).dim(true).height(m::CONTROL_HEIGHT)
    } else {
        StringView::new(foot_text).size(style::SMALL_SIZE).beside(style::PLAIN_SIZE).dim(foot_light != Some(Light::Down)).cut(true).height(m::CONTROL_HEIGHT)
    };
    let foot_status = if plan_right {
        div().flex_1().min_w_0().flex().flex_row().items_center().justify_end().child(foot_label)
    } else {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::TIGHT))
            .children(if foot_busy { Some(panels::busy_of(panels::LIGHT)) } else { foot_light.map(panels::light) })
            .child(div().flex_1().min_w_0().child(foot_label))
    };

    let mut foot = div().w_full().flex().flex_row().items_center().pr(px(3.0)).child(check_btn).child(div().w(px(12.0))).child(foot_status).child(div().w(px(12.0)));

    if resolved {
        foot = foot.child(Button::new(olo_kit::words::tr("Close").to_string()).id("carry-close").min_width(std_btn_w).default(true).on_click(move |_, cx| on_close(&CarryDo::Close, cx)));
    } else {
        let apply_label = if shown.files_to_write() == 0 && !shown.remember_edits().is_empty() { olo_kit::words::tr("Save Choices").to_owned() } else { olo_kit::words::tr("Apply").to_owned() };
        let action_w = button_width(&apply_label, false).max(std_btn_w);
        foot = foot
            .child(Button::new(olo_kit::words::tr("Cancel").to_string()).id("carry-close").min_width(action_w).enabled(!shown.applying).on_click(move |_, cx| on_close(&CarryDo::Close, cx)))
            .child(div().w(px(BTN_GAP)))
            .child(Button::new(apply_label).id("carry-apply").min_width(action_w).default(can_apply).enabled(can_apply).on_click(move |_, cx| on_apply(&CarryDo::Apply, cx)));
    }

    div().w_full().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).agent("Group", "carry-diff").agent_value(shown.agent_json()).child(head).child(tabs).child(foot)
}
