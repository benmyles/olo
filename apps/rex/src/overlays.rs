//! Native Olo overlays inside Rex workspace windows:
//! - Command Palette (`⇧⌘P`)
//! - Go to Anything (`⌘P`)
//! - `rex.ui` Custom Prompts (`input` & `pick` with `multiple`, `allow_other`, 16 Rex icons)
//! - Which-Key Sequence Hints HUD (`KeyHint`) & Active Key Mode Pill
//! - Integrated Path Browser (`⌃⌘I`) & Go to Directory (`⇧⌘G`)
//! - Quick Look (`⌘Y`) image & text preview
//! - In-App Floating Notification Toast (6 anchors)
//! - Auto-Approve 30 s Countdown Pill & Password Prompt Autofill Banner

use std::{collections::HashSet, sync::Arc, time::Instant};

use olo_kit::{
    Bounds, Child, CursorStyle, Element, FontWeight, InteractiveElement as _, MouseButton, ParentElement as _, Pixels, RenderImage, SharedString, Styled as _, WindowCx, agent, canvas, div, icons,
    metrics as m, point, px, size, style, theme,
    widgets::{Button, KeyHint, StringView, TextControl, TextField},
};
use olo_rex_client::{ClientPromptRequested, DirEntry};

use crate::{
    agent::AgentActivity,
    hub::BlockKey,
    keymap::{ActionSpec, SequenceHint},
    settings::NotifyAnchor,
    terminal_pane::rgba_to_render_image,
};

/// Active modal/floating overlay open inside a Rex workspace window.
pub enum ActiveOverlay {
    CommandPalette { query: String, selected: usize },
    GoToAnything { query: String, selected: usize, items: Vec<JumpItem> },
    RexPrompt { host_key: String, req: ClientPromptRequested, query: String, selected_idx: usize, checked_ids: HashSet<String> },
    PathBrowser { mode: PathBrowserMode, block: BlockKey, dir: String, query: String, entries: Vec<DirEntry>, selected: usize },
    QuickLook { path: String, content: QuickLookContent },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathBrowserMode {
    InsertPath,
    GoToDir,
}

pub enum QuickLookContent {
    Loading,
    Image { image: Arc<RenderImage>, width: u32, height: u32 },
    Text { lines: Vec<String>, scroll: usize },
    Binary { size: usize, content_type: Option<String> },
}

impl QuickLookContent {
    pub fn from_bytes(path: &str, bytes: Vec<u8>, content_type: Option<String>) -> Self {
        let lower = path.to_ascii_lowercase();
        let looks_image = content_type.as_deref().is_some_and(|ct| ct.starts_with("image/"))
            || lower.ends_with(".png")
            || lower.ends_with(".jpg")
            || lower.ends_with(".jpeg")
            || lower.ends_with(".gif")
            || lower.ends_with(".webp")
            || lower.ends_with(".bmp");

        if looks_image && let Ok(img) = image::load_from_memory(&bytes) {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            if let Some(ri) = rgba_to_render_image(rgba.as_raw(), w, h) {
                return Self::Image { image: ri, width: w, height: h };
            }
        }
        if let Ok(text) = String::from_utf8(bytes.clone()) {
            let lines = text.lines().take(2000).map(str::to_owned).collect();
            return Self::Text { lines, scroll: 0 };
        }
        Self::Binary { size: bytes.len(), content_type }
    }
}

/// Entry shown in Go to Anything (`⌘P`).
#[derive(Debug, Clone)]
pub struct JumpItem {
    pub title: String,
    pub subtitle: String,
    pub badge: Option<String>,
    pub activity: Option<AgentActivity>,
    pub target: JumpTarget,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum JumpTarget {
    Pane(BlockKey),
    Window { host_key: String, session_id: String, window_id: String },
    Session { host_key: String, session_id: String },
    Host(String),
    Directory(String),
}

/// Ephemeral in-app floating toast notification.
#[derive(Debug, Clone)]
pub struct ToastItem {
    pub id: u64,
    pub title: String,
    pub body: String,
    pub target: Option<BlockKey>,
    pub created: Instant,
}

/// Map Rex's 16 official `rex.ui` prompt icon names to Olo SVG icon names.
pub fn map_rex_icon(name: Option<&str>) -> &'static str {
    match name.unwrap_or_default() {
        "terminal" | "shell" => "terminal",
        "folder" | "directory" => "folder",
        "file" | "document" => "document",
        "git" | "branch" | "repo" => "git",
        "server" | "host" | "computer" => "network",
        "globe" | "web" | "link" => "network",
        "key" | "lock" | "secret" => "passwords",
        "gear" | "settings" | "wrench" => "preferences",
        "search" | "find" => "search",
        "bolt" | "zap" | "activity" => "activity",
        "warning" | "alert" | "error" => "warning",
        "check" | "done" => "check",
        "plus" | "add" => "plus",
        "trash" | "delete" => "trash",
        "star" | "bookmark" => "star",
        _ => "rex",
    }
}

/// Simple crisp fuzzy matcher scoring matches by word boundary and substring proximity.
pub fn fuzzy_score(haystack: &str, needle: &str) -> Option<i32> {
    let q = needle.trim().to_ascii_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    let h = haystack.to_ascii_lowercase();
    if h == q {
        return Some(1000);
    }
    if h.starts_with(&q) {
        return Some(800 - h.len() as i32);
    }
    if let Some(idx) = h.find(&q) {
        return Some(600 - idx as i32 * 2);
    }
    let mut score = 0i32;
    let mut hi = h.chars();
    for nc in q.chars() {
        let mut found = false;
        let mut skipped = 0;
        for hc in hi.by_ref() {
            if hc == nc {
                found = true;
                score += 20 - skipped.min(18);
                break;
            }
            skipped += 1;
        }
        if !found {
            return None;
        }
    }
    Some(score)
}

/// Filter and rank the action catalog for Command Palette (`⇧⌘P`).
pub fn filter_actions<'a>(catalog: &'a [ActionSpec], query: &str) -> Vec<&'a ActionSpec> {
    let mut scored: Vec<(i32, &ActionSpec)> = catalog
        .iter()
        .filter_map(|a| {
            let joined = format!("{} {} {} {}", a.title, a.category, a.id, a.keywords.join(" "));
            fuzzy_score(&joined, query).map(|s| (s, a))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.title.cmp(b.1.title)));
    scored.into_iter().map(|(_, a)| a).collect()
}

/// Paint a crisp Olo beveled overlay card backdrop.
pub fn overlay_card_back(label: &'static str) -> Element {
    canvas(move |g, b| {
        let p = theme::palette();
        style::fill_rect(g, b, style::c(p.panel_background));
        style::raised(g, b, &p);
        g.agent(agent::Node { role: "Dialog", name: Some(label.into()), bounds: b, enabled: true, ..Default::default() });
    })
    .paint_key(style::key(label))
    .absolute()
    .inset_0()
}

/// Render the centered modal overlay scrim + card for Command Palette, Go to Anything,
/// `rex.ui` Prompt, Path Browser, or Quick Look.
#[allow(clippy::too_many_arguments)]
pub fn render_active_overlay(
    cx: &WindowCx,
    overlay: &ActiveOverlay,
    field: &Child<TextField>,
    catalog: &[ActionSpec],
    shortcuts: &dyn Fn(&str) -> Option<String>,
    on_dismiss: impl Fn(&(), &mut WindowCx) + 'static,
    on_pick_index: impl Fn(&usize, &mut WindowCx) + 'static,
    on_toggle_check: impl Fn(&String, &mut WindowCx) + 'static,
    on_confirm: impl Fn(&(), &mut WindowCx) + 'static,
) -> Element {
    let on_dismiss = std::rc::Rc::new(on_dismiss);
    let on_pick_index = std::rc::Rc::new(on_pick_index);
    let on_toggle_check = std::rc::Rc::new(on_toggle_check);
    let on_confirm = std::rc::Rc::new(on_confirm);

    let dismiss_bg = on_dismiss.clone();
    let scrim = div()
        .absolute()
        .inset_0()
        .child(
            canvas(|g, b| {
                g.paint_quad(olo_kit::fill(b, style::c([0, 0, 0, 96])));
            })
            .paint_key(1)
            .size_full(),
        )
        .on_mouse_down(MouseButton::Left, move |_, cx| {
            dismiss_bg(&(), cx);
            cx.stop_propagation();
        });

    let card: Element = match overlay {
        ActiveOverlay::CommandPalette { query, selected } => {
            let filtered = filter_actions(catalog, query);
            let shown_len = filtered.len().min(12);
            let mut list = div().flex().flex_col().gap(px(2.0)).w_full();
            for (idx, act) in filtered.iter().take(shown_len).enumerate() {
                let is_sel = idx == *selected;
                let title = act.title.to_owned();
                let cat = act.category.to_owned();
                let sc = shortcuts(act.id);
                let pick = on_pick_index.clone();
                list = list.child(palette_row(title, cat, sc, None, is_sel, move |cx| pick(&idx, cx)));
            }
            if filtered.is_empty() {
                list = list.child(div().py(px(m::SECTION)).flex().justify_center().child(StringView::new("No matching commands").dim(true)));
            }
            modal_shell(
                "Command Palette",
                540.0,
                div().flex().flex_col().gap(px(m::SPACING)).w_full().child(TextControl::new(cx, field)).child(list).child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .pt(px(m::TIGHT))
                        .child(div().flex().flex_row().gap(px(m::SECTION)).child(KeyHint::new("↑↓", "Select")).child(KeyHint::new("↩", "Run")).child(KeyHint::new("Esc", "Close")))
                        .child(StringView::new(format!("{} actions", filtered.len())).dim(true)),
                ),
            )
        }
        ActiveOverlay::GoToAnything { query, selected, items } => {
            let mut scored: Vec<(i32, usize, &JumpItem)> = items
                .iter()
                .enumerate()
                .filter_map(|(i, it)| {
                    let hay = format!("{} {} {}", it.title, it.subtitle, it.badge.as_deref().unwrap_or(""));
                    fuzzy_score(&hay, query).map(|s| (s, i, it))
                })
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0));
            let mut list = div().flex().flex_col().gap(px(2.0)).w_full();
            for (rank, (_, orig_idx, item)) in scored.iter().take(12).enumerate() {
                let is_sel = rank == *selected;
                let pick = on_pick_index.clone();
                let idx_copy = *orig_idx;
                let dot = item.activity.map(activity_dot_color);
                list = list.child(palette_row(item.title.clone(), item.subtitle.clone(), item.badge.clone(), dot, is_sel, move |cx| pick(&idx_copy, cx)));
            }
            if scored.is_empty() {
                list = list.child(div().py(px(m::SECTION)).flex().justify_center().child(StringView::new("No matching sessions, tabs, or panes").dim(true)));
            }
            modal_shell(
                "Go to Anything",
                560.0,
                div()
                    .flex()
                    .flex_col()
                    .gap(px(m::SPACING))
                    .w_full()
                    .child(TextControl::new(cx, field))
                    .child(list)
                    .child(div().flex().flex_row().gap(px(m::SECTION)).pt(px(m::TIGHT)).child(KeyHint::new("↑↓", "Move")).child(KeyHint::new("↩", "Jump")).child(KeyHint::new("Esc", "Close"))),
            )
        }
        ActiveOverlay::RexPrompt { req, query, selected_idx, checked_ids, .. } => {
            let prompt = &req.prompt;
            let confirm_lbl = prompt.confirm.as_deref().unwrap_or("Confirm").to_owned();
            let cancel_cb = on_dismiss.clone();
            let ok_cb = on_confirm.clone();
            let mut body = div().flex().flex_col().gap(px(m::SPACING)).w_full().child(StringView::new(prompt.title.clone()).bold(true)).child(TextControl::new(cx, field));

            if prompt.kind == "pick" {
                let items = prompt.items.as_deref().unwrap_or(&[]);
                let filtered: Vec<(usize, &olo_rex_client::PromptItem)> = items
                    .iter()
                    .enumerate()
                    .filter(|(_, it)| {
                        let hay = format!("{} {} {}", it.title, it.subtitle.as_deref().unwrap_or(""), it.detail.as_deref().unwrap_or(""));
                        fuzzy_score(&hay, query).is_some()
                    })
                    .collect();
                let mut list = div().flex().flex_col().gap(px(2.0)).w_full();
                for (rank, (orig_idx, it)) in filtered.iter().take(10).enumerate() {
                    let is_sel = rank == *selected_idx;
                    let is_checked = checked_ids.contains(&it.id);
                    let icon_name = map_rex_icon(it.icon.as_deref());
                    let title = if prompt.multiple { format!("{} {}", if is_checked { "☑" } else { "☐" }, it.title) } else { it.title.clone() };
                    let sub = it.subtitle.clone().unwrap_or_default();
                    let detail = it.detail.clone();
                    let id_clone = it.id.clone();
                    let multiple = prompt.multiple;
                    let pick = on_pick_index.clone();
                    let tog = on_toggle_check.clone();
                    let oi = *orig_idx;
                    list = list.child(prompt_item_row(title, sub, detail, icon_name, is_sel, move |cx| {
                        if multiple {
                            tog(&id_clone, cx);
                        } else {
                            pick(&oi, cx);
                        }
                    }));
                }
                body = body.child(list);
            }

            body = body.child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(m::SPACING))
                    .pt(px(m::TIGHT))
                    .child(Button::new("Cancel").on_click(move |_, cx| cancel_cb(&(), cx)))
                    .child(Button::new(confirm_lbl).default(true).on_click(move |_, cx| ok_cb(&(), cx))),
            );
            modal_shell("Rex Prompt", 520.0, body)
        }
        ActiveOverlay::PathBrowser { mode, dir, query, entries, selected, .. } => {
            let heading = match mode {
                PathBrowserMode::InsertPath => format!("Path Browser — {dir}"),
                PathBrowserMode::GoToDir => format!("Go to Directory — {dir}"),
            };
            let filtered: Vec<(usize, &DirEntry)> = entries
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    if *mode == PathBrowserMode::GoToDir && e.file {
                        return false;
                    }
                    fuzzy_score(&e.name, query).is_some()
                })
                .collect();
            let mut list = div().flex().flex_col().gap(px(2.0)).w_full();
            for (rank, (orig_idx, e)) in filtered.iter().take(12).enumerate() {
                let is_sel = rank == *selected;
                let pick = on_pick_index.clone();
                let oi = *orig_idx;
                let icon = if e.file { "document" } else { "folder" };
                let kind_lbl = if e.symlink {
                    "Symlink"
                } else if e.file {
                    "File"
                } else {
                    "Folder"
                };
                list = list.child(prompt_item_row(e.name.clone(), kind_lbl.to_owned(), None, icon, is_sel, move |cx| pick(&oi, cx)));
            }
            if filtered.is_empty() {
                list = list.child(div().py(px(m::SECTION)).flex().justify_center().child(StringView::new("No matching entries").dim(true)));
            }
            let action_lbl = match mode {
                PathBrowserMode::InsertPath => "Insert Path",
                PathBrowserMode::GoToDir => "Open Folder",
            };
            modal_shell(
                "Path Browser",
                540.0,
                div()
                    .flex()
                    .flex_col()
                    .gap(px(m::SPACING))
                    .w_full()
                    .child(StringView::new(heading).bold(true).cut(true))
                    .child(TextControl::new(cx, field))
                    .child(list)
                    .child(div().flex().flex_row().gap(px(m::SECTION)).pt(px(m::TIGHT)).child(KeyHint::new("↩", action_lbl)).child(KeyHint::new("Esc", "Close"))),
            )
        }
        ActiveOverlay::QuickLook { path, content } => {
            let dismiss = on_dismiss.clone();
            let body_el: Element = match content {
                QuickLookContent::Loading => div().h(px(220.0)).flex().items_center().justify_center().child(StringView::new("Downloading preview from Rex server…").dim(true)),
                QuickLookContent::Image { image, width, height } => {
                    let img = image.clone();
                    let (iw, ih) = (*width as f32, *height as f32);
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(m::TIGHT))
                        .w_full()
                        .child(
                            canvas(move |g, b| {
                                let p = theme::palette();
                                style::fill_rect(g, b, style::c(p.document_background));
                                style::sunken(g, b, &p, false);
                                let inner = style::inset(g, b, 2.0);
                                let scale = (f32::from(inner.size.width) / iw.max(1.0)).min(f32::from(inner.size.height) / ih.max(1.0)).min(1.0);
                                let dw = iw * scale;
                                let dh = ih * scale;
                                let x = f32::from(inner.left()) + (f32::from(inner.size.width) - dw) / 2.0;
                                let y = f32::from(inner.top()) + (f32::from(inner.size.height) - dh) / 2.0;
                                let target = Bounds::new(point(px(x), px(y)), size(px(dw), px(dh)));
                                let _ = g.paint_image(target, olo_kit::Corners::default(), img.clone(), 0, false);
                            })
                            .paint_key(style::key((path.as_str(), *width, *height)))
                            .w_full()
                            .h(px(340.0)),
                        )
                        .child(StringView::new(format!("{width} × {height} px")).dim(true))
                }
                QuickLookContent::Text { lines, scroll } => {
                    let shown: Vec<String> = lines.iter().skip(*scroll).take(22).cloned().collect();
                    let key = style::key((path.as_str(), *scroll, shown.len()));
                    canvas(move |g, b| {
                        let p = theme::palette();
                        style::fill_rect(g, b, style::c(p.document_background));
                        style::sunken(g, b, &p, false);
                        let font = style::mono_font(FontWeight::NORMAL);
                        let ink = style::c(p.document_text);
                        let mut y = f32::from(b.top()) + 12.0;
                        for line in &shown {
                            style::label_fit(g, b.left() + px(8.0), px(y), style::Align::Left, line.clone(), 11.0, font.clone(), ink, b.size.width - px(16.0));
                            y += 15.0;
                        }
                    })
                    .paint_key(key)
                    .w_full()
                    .h(px(340.0))
                }
                QuickLookContent::Binary { size: sz, content_type } => div()
                    .h(px(140.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(m::TIGHT))
                    .child(StringView::new(format!("Binary file ({sz} bytes)")).bold(true))
                    .child(StringView::new(content_type.clone().unwrap_or_else(|| "application/octet-stream".into())).dim(true)),
            };
            modal_shell(
                "Quick Look",
                640.0,
                div()
                    .flex()
                    .flex_col()
                    .gap(px(m::SPACING))
                    .w_full()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(px(m::SPACING))
                            .child(div().flex_1().min_w_0().child(StringView::new(path.clone()).bold(true).cut(true)))
                            .child(Button::new("Close").small().on_click(move |_, cx| dismiss(&(), cx))),
                    )
                    .child(body_el),
            )
        }
    };

    div().absolute().inset_0().child(scrim).child(div().absolute().inset_0().flex().flex_col().items_center().pt(px(56.0)).px(px(m::WINDOW_INSET)).child(card))
}

fn modal_shell(label: &'static str, max_w: f32, content: Element) -> Element {
    div().relative().w(px(max_w)).max_w_full().p(px(m::WINDOW_INSET)).on_mouse_down(MouseButton::Left, |_, cx| cx.stop_propagation()).child(overlay_card_back(label)).child(content)
}

fn palette_row(title: String, subtitle: String, shortcut: Option<String>, dot: Option<[u8; 4]>, selected: bool, on_click: impl Fn(&mut WindowCx) + 'static) -> Element {
    let key = style::key((&title, &subtitle, &shortcut, dot, selected));
    let t_paint = title.clone();
    let s_paint = subtitle.clone();
    let sc_paint = shortcut.clone();
    canvas(move |g, b| {
        let p = theme::palette();
        if selected {
            style::fill_rect(g, b, style::c(p.list_selected_background));
        }
        let ink = style::c(if selected { p.list_selected_item_text } else { p.panel_text });
        let quiet = if selected { ink.opacity(0.8) } else { style::secondary_text(&p) };
        let mut left = b.left() + px(m::SPACING);
        if let Some(dc) = dot {
            let r = style::snap_centred(g, point(left + px(4.0), b.center().y), 7.0, 7.0);
            style::fill_rect(g, r, style::c(dc));
            left = left + px(14.0);
        }
        let mut right = b.right() - px(m::SPACING);
        if let Some(sc) = &sc_paint {
            let sc_r = style::label(g, right, b.center().y, style::Align::Right, sc.clone(), style::SMALL_SIZE, style::mono_font(FontWeight::NORMAL), quiet);
            right = sc_r.left() - px(m::SPACING);
        }
        let sub_r = if !s_paint.is_empty() {
            let sr = style::label(g, right, b.center().y, style::Align::Right, s_paint.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), quiet);
            sr.left() - px(m::SPACING)
        } else {
            right
        };
        style::label_fit(
            g,
            left,
            b.center().y,
            style::Align::Left,
            t_paint.clone(),
            style::PLAIN_SIZE,
            style::ui_font(if selected { FontWeight::BOLD } else { FontWeight::NORMAL }),
            ink,
            (sub_r - left).max(px(40.0)),
        );
        g.agent(agent::Node { role: "ListItem", name: Some(t_paint.clone().into()), bounds: b, selected: Some(selected), enabled: true, clickable: true, ..Default::default() });
    })
    .paint_key(key)
    .w_full()
    .h(px(26.0))
    .cursor(CursorStyle::PointingHand)
    .on_mouse_down(MouseButton::Left, move |_, cx| {
        on_click(cx);
        cx.stop_propagation();
    })
}

fn prompt_item_row(title: String, subtitle: String, detail: Option<String>, icon_name: &'static str, selected: bool, on_click: impl Fn(&mut WindowCx) + 'static) -> Element {
    let key = style::key((&title, &subtitle, &detail, icon_name, selected));
    let t_paint = title.clone();
    let s_paint = subtitle.clone();
    let d_paint = detail.clone();
    canvas(move |g, b| {
        let p = theme::palette();
        if selected {
            style::fill_rect(g, b, style::c(p.list_selected_background));
        }
        let ink = style::c(if selected { p.list_selected_item_text } else { p.panel_text });
        let quiet = if selected { ink.opacity(0.8) } else { style::secondary_text(&p) };
        let icon_r = style::snap_centred(g, point(b.left() + px(16.0), b.center().y), 16.0, 16.0);
        if !icons::paint(g, icon_r, icon_name) {
            icons::paint(g, icon_r, "terminal");
        }
        let left = b.left() + px(30.0);
        let mut right = b.right() - px(m::SPACING);
        if let Some(d) = &d_paint {
            let dr = style::label(g, right, b.center().y, style::Align::Right, d.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), quiet);
            right = dr.left() - px(m::SPACING);
        }
        if !s_paint.is_empty() {
            let sr = style::label(g, right, b.center().y, style::Align::Right, s_paint.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), quiet);
            right = sr.left() - px(m::SPACING);
        }
        style::label_fit(
            g,
            left,
            b.center().y,
            style::Align::Left,
            t_paint.clone(),
            style::PLAIN_SIZE,
            style::ui_font(if selected { FontWeight::BOLD } else { FontWeight::NORMAL }),
            ink,
            (right - left).max(px(40.0)),
        );
    })
    .paint_key(key)
    .w_full()
    .h(px(26.0))
    .cursor(CursorStyle::PointingHand)
    .on_mouse_down(MouseButton::Left, move |_, cx| {
        on_click(cx);
        cx.stop_propagation();
    })
}

/// Which-Key Sequence Hints floating HUD anchored bottom-center.
pub fn render_which_key_hud(prefix: &str, hints: &[SequenceHint]) -> Element {
    let mut row = div().flex().flex_row().flex_wrap().items_center().gap(px(m::SECTION));
    for h in hints.iter().take(10) {
        let lbl = if h.is_prefix { format!("+{}", h.label) } else { h.label.clone() };
        row = row.child(KeyHint::new(h.next_key.clone(), lbl));
    }
    row = row.child(KeyHint::new("Esc", "Cancel"));

    div().absolute().bottom(px(m::WINDOW_INSET)).left(px(m::WINDOW_INSET)).right(px(m::WINDOW_INSET)).flex().justify_center().child(
        div()
            .relative()
            .px(px(m::WINDOW_INSET))
            .py(px(m::SPACING))
            .flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .child(overlay_card_back("Sequence Hints"))
            .child(StringView::new(format!("Sequence: {prefix} …")).bold(true))
            .child(row),
    )
}

/// Active Key Mode or Copy Mode status badge anchored bottom-left of the pane area.
pub fn render_mode_pill(text: String) -> Element {
    let key = style::key(&text);
    let label: SharedString = text.into();
    div().absolute().left(px(m::SPACING)).bottom(px(m::SPACING)).child(
        canvas(move |g, b| {
            let p = theme::palette();
            style::fill_rect(g, b, style::c(p.tab_focused));
            style::raised(g, b, &p);
            style::label(g, b.center().x, b.center().y, style::Align::Center, label.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::BOLD), style::c(p.tab_text_focused));
            g.agent(agent::Node { role: "Status", name: Some(label.clone()), bounds: b, enabled: true, ..Default::default() });
        })
        .paint_key(key)
        .px(px(10.0))
        .h(px(22.0))
        .w(px(190.0)),
    )
}

/// Floating in-app notification toast anchored to the user's chosen `NotifyAnchor`.
pub fn render_toast(toast: &ToastItem, anchor: NotifyAnchor, on_click: impl Fn(&(), &mut WindowCx) + 'static, on_dismiss: impl Fn(&(), &mut WindowCx) + 'static) -> Element {
    let mut wrap = div().absolute().p(px(m::WINDOW_INSET));
    wrap = match anchor {
        NotifyAnchor::TopLeft => wrap.top(px(0.0)).left(px(0.0)),
        NotifyAnchor::TopCenter => wrap.top(px(0.0)).left(px(0.0)).right(px(0.0)).flex().justify_center(),
        NotifyAnchor::TopRight => wrap.top(px(0.0)).right(px(0.0)),
        NotifyAnchor::BottomLeft => wrap.bottom(px(0.0)).left(px(0.0)),
        NotifyAnchor::BottomCenter => wrap.bottom(px(0.0)).left(px(0.0)).right(px(0.0)).flex().justify_center(),
        NotifyAnchor::BottomRight => wrap.bottom(px(0.0)).right(px(0.0)),
    };

    wrap.child(
        div()
            .relative()
            .w(px(320.0))
            .p(px(m::SPACING))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            .cursor(CursorStyle::PointingHand)
            .on_mouse_down(MouseButton::Left, move |_, cx| {
                on_click(&(), cx);
                cx.stop_propagation();
            })
            .child(overlay_card_back("Notification"))
            .child(
                div().flex_1().min_w_0().flex().flex_col().gap(px(2.0)).child(StringView::new(toast.title.clone()).bold(true).cut(true)).child(StringView::new(toast.body.clone()).dim(true).cut(true)),
            )
            .child(Button::new("×").small().on_click(move |_, cx| {
                on_dismiss(&(), cx);
                cx.stop_propagation();
            })),
    )
}

/// Auto-Approve 30-second countdown pill anchored bottom-right over a blocked pane.
pub fn render_auto_approve_pill(screen: Bounds<Pixels>, remaining_secs: u32, on_stop: impl Fn(&(), &mut WindowCx) + 'static) -> Element {
    let text: SharedString = format!("Auto Approve: Yes in {remaining_secs} s").into();
    let name = text.clone();
    let back = canvas(move |g, b| {
        let p = theme::palette();
        olo_kit::widgets::banner::banner_back(g, b, None, &p);
        g.agent(agent::Node { role: "Status", name: Some(name.clone()), bounds: b, enabled: true, ..Default::default() });
    })
    .paint_key(style::key(&text))
    .absolute()
    .inset_0();
    div().absolute().left(screen.left()).top(screen.top()).w(screen.size.width).h(screen.size.height).child(
        div()
            .absolute()
            .right(px(m::SPACING))
            .bottom(px(m::SPACING))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            .pl(px(m::SPACING))
            .pr(px(m::TIGHT))
            .py(px(m::TIGHT))
            .child(back)
            .child(StringView::new(text))
            .child(Button::new("Stop").small().on_click(move |_, cx| {
                on_stop(&(), cx);
                cx.stop_propagation();
            })),
    )
}

pub fn activity_dot_color(act: AgentActivity) -> [u8; 4] {
    match act {
        AgentActivity::Blocked => [230, 145, 24, 255],
        AgentActivity::Error => [216, 56, 56, 255],
        AgentActivity::Working => [52, 132, 240, 255],
        AgentActivity::Done => [56, 170, 92, 255],
        AgentActivity::Idle => [140, 140, 148, 160],
    }
}
