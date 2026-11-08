//! /ask's answer in the palette (`ask.rs` asks): the prose as it comes,
//! then each snippet numbered, previewed (`PREVIEW` lines) or shown whole
//! (its number pressed), and along the foot the keys that work now. Laid
//! out by measuring (`lay_out`: the prose wrapped to the panel, the code
//! a line a row), so the panel is as tall as the answer, up to a share of
//! the screen, scrolled past that.

use std::sync::{Arc, atomic::AtomicBool};

use olo_ui::{
    gpui::{self, AnyElement, App, Bounds, Entity, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Pixels, ScrollWheelEvent, Styled as _, canvas, div, px},
    metrics as m,
    style::{self, c},
    theme,
    widgets::{KeyHint, paragraph_height},
};

use crate::{Palette, ask::Snippet};

/// A snippet previewed shows this many lines at most.
pub const PREVIEW: usize = 6;
/// A snippet's head (its number, title and language) and its padding.
const HEAD: f32 = 24.0;
const PAD: f32 = 8.0;
/// The keys' row along the foot.
pub const FOOT: f32 = 22.0;

/// An answer, as it comes and once it has.
pub struct Answering {
    /// Its number (`Palette::asks`), and the conversation before it.
    pub n: u64,
    pub history: Vec<olo_ai::Turn>,
    pub question: String,
    /// The model asked, once known.
    pub model: String,
    pub prose: String,
    pub snippets: Vec<Snippet>,
    /// The whole answer's JSON, once it has come (a follow-up's history).
    pub json: Option<String>,
    pub error: Option<String>,
    /// The snippet shown whole, and how far the answer is scrolled.
    pub open: Option<usize>,
    pub scroll: f32,
    /// Typing a follow-up (Space).
    pub following: bool,
    /// What was just copied, said in the foot.
    pub copied: Option<&'static str>,
    pub cancel: Arc<AtomicBool>,
}

impl Answering {
    pub fn new(n: u64, question: String, history: Vec<olo_ai::Turn>, cancel: Arc<AtomicBool>) -> Answering {
        Answering { n, history, question, model: String::new(), prose: String::new(), snippets: Vec::new(), json: None, error: None, open: None, scroll: 0.0, following: false, copied: None, cancel }
    }

    pub fn done(&self) -> bool {
        self.json.is_some() || self.error.is_some()
    }

    /// What Return copies: the snippet shown, else the only one, else the
    /// answer's words; and what to say of it.
    pub fn to_copy(&self) -> Option<(&'static str, String)> {
        match (self.open, self.snippets.as_slice()) {
            (Some(i), s) => s.get(i).map(|s| ("Copied", s.code.clone())),
            (None, [one]) => Some(("Copied", one.code.clone())),
            (None, _) if !self.prose.is_empty() && self.done() => Some(("Copied the answer", self.prose.clone())),
            _ => None,
        }
    }

    /// What shows where the answer goes, before any of it has come.
    fn status(&self) -> String {
        match (&self.error, self.model.is_empty()) {
            (Some(e), _) => e.clone(),
            (None, true) => "Asking…".into(),
            (None, false) => format!("Asking {}…", self.model),
        }
    }
}

/// A line of code's height.
pub fn mono_line() -> f32 {
    (style::MONO_SIZE * style::LINE_HEIGHT).ceil()
}

/// A snippet's block: its head, `lines` of code, and a line saying how many
/// more there are (`more`).
fn snippet_height(lines: usize, more: bool) -> f32 {
    HEAD + (lines + more as usize) as f32 * mono_line() + 2.0 * PAD - 4.0
}

/// How the answer fits `width`, at most `most` tall.
pub struct Laid {
    /// Each block's height: the words (unless a snippet is shown whole),
    /// then the snippets.
    blocks: Vec<f32>,
    /// What shows of it, and how far it's scrolled (held within).
    pub area: f32,
    pub scroll: f32,
}

pub fn lay_out(a: &Answering, width: f32, most: f32, cx: &App) -> Laid {
    let mut blocks = Vec::new();
    match a.open.and_then(|i| a.snippets.get(i)) {
        Some(s) => blocks.push(snippet_height(s.code.lines().count().max(1), false)),
        None => {
            let words = if a.prose.is_empty() { a.status() } else { a.prose.clone() };
            blocks.push(paragraph_height(cx, &words, width));
            for s in &a.snippets {
                let n = s.code.lines().count().max(1);
                blocks.push(snippet_height(n.min(PREVIEW), n > PREVIEW));
            }
        }
    }
    let content = blocks.iter().sum::<f32>() + m::SPACING * blocks.len().saturating_sub(1) as f32;
    let area = content.min(most).ceil();
    let scroll = a.scroll.clamp(0.0, (content - area).max(0.0));
    Laid { blocks, area, scroll }
}

/// The answer drawn in `at` (the panel's, under the field).
pub fn view(a: &Answering, laid: &Laid, at: Bounds<Pixels>, me: Entity<Palette>, cx: &App) -> AnyElement {
    let p = theme::palette(cx);
    let (text, dim) = (c(p.panel_text), style::disabled_text(&p));
    let snippet = |i: usize, s: &Snippet, whole: bool, h: f32| -> AnyElement {
        let lines: Vec<&str> = s.code.lines().collect();
        let shown = if whole { lines.len() } else { lines.len().min(PREVIEW) };
        let more = lines.len() - shown;
        let code = div()
            .font_family(style::MONO_FONT)
            .text_size(px(style::MONO_SIZE))
            .line_height(px(mono_line()))
            .text_color(c(p.document_text))
            .children(lines.iter().take(shown).map(|l| div().whitespace_nowrap().overflow_hidden().child(if l.is_empty() { " ".to_string() } else { l.to_string() })))
            .when(more > 0, |d| d.child(div().text_color(dim).child(format!("{more} more line{}", if more == 1 { "" } else { "s" }))));
        let title = if s.title.is_empty() { "Code".to_string() } else { s.title.clone() };
        let (said, value) = (title.clone(), s.code.clone());
        let open = me.clone();
        div()
            .relative()
            .w_full()
            .h(px(h))
            .flex()
            .flex_col()
            .px(px(PAD))
            .pt(px(PAD - 4.0))
            // A sunken well, as the field's (on whole device pixels).
            .child(
                canvas(
                    |_, _, _| {},
                    |b, _, window, cx| {
                        let p = theme::palette(cx);
                        let r = style::snap(window, b);
                        style::fill_rect(window, r, c(p.document_background));
                        style::sunken(window, r, &p, false);
                    },
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div()
                    .h(px(HEAD))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(m::TIGHT))
                    .child(KeyHint::new(format!("{}", i + 1), title))
                    .child(div().flex_1())
                    .child(div().text_size(px(style::DETAIL_SIZE)).text_color(dim).child(s.language.clone())),
            )
            .child(code)
            // A click on it shows it whole.
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                open.update(cx, |p, cx| {
                    if let Some(a) = p.answer.as_mut() {
                        a.open = Some(i);
                        a.scroll = 0.0;
                        cx.notify();
                    }
                })
            })
            // For agents: the snippet and its code.
            .child(
                canvas(
                    |_, _, _| {},
                    move |b, _, window, cx| {
                        olo_ui::agent::record(
                            window,
                            cx,
                            olo_ui::agent::Node { role: "Item", name: Some(said.clone().into()), value: Some(value.clone().into()), bounds: b, enabled: true, clickable: true, ..Default::default() },
                        )
                    },
                )
                .absolute()
                .inset_0(),
            )
            .into_any_element()
    };
    let mut column = div().absolute().left_0().w_full().top(px(-laid.scroll)).flex().flex_col().gap(px(m::SPACING));
    match a.open.and_then(|i| a.snippets.get(i).map(|s| (i, s))) {
        Some((i, s)) => column = column.child(snippet(i, s, true, laid.blocks[0])),
        None => {
            let waiting = a.prose.is_empty();
            let words = if waiting { a.status() } else { a.prose.clone() };
            column = column.child(div().w_full().h(px(laid.blocks[0])).text_color(if waiting && a.error.is_none() { dim } else { text }).child(olo_ui::widgets::paragraph(words)));
            for (i, s) in a.snippets.iter().enumerate() {
                column = column.child(snippet(i, s, false, laid.blocks[i + 1]));
            }
        }
    }
    let wheel = me.clone();
    div()
        .absolute()
        .left(at.left())
        .top(at.top())
        .w(at.size.width)
        .h(at.size.height)
        .overflow_hidden()
        .child(column)
        .on_scroll_wheel(move |e: &ScrollWheelEvent, _, cx| {
            let dy = f32::from(e.delta.pixel_delta(px(mono_line())).y);
            wheel.update(cx, |p, cx| {
                if let Some(a) = p.answer.as_mut() {
                    a.scroll = (a.scroll - dy).max(0.0);
                    cx.notify();
                }
            });
        })
        .into_any_element()
}

/// The keys that work now, along the foot (and what was just copied).
pub fn foot(a: &Answering, at: Bounds<Pixels>, cx: &App) -> AnyElement {
    let p = theme::palette(cx);
    let mut hints: Vec<KeyHint> = Vec::new();
    if a.following {
        hints.push(KeyHint::new("Return", "Ask"));
        hints.push(KeyHint::new("Esc", "Back to the answer"));
    } else if !a.done() {
        hints.push(KeyHint::new("Esc", "Stop"));
    } else {
        if a.open.is_none() && a.snippets.len() > 1 {
            hints.push(KeyHint::new(format!("1–{}", a.snippets.len()), "Show"));
        }
        if let Some((_, _)) = a.to_copy() {
            hints.push(KeyHint::new("Return", if a.open.is_some() || a.snippets.len() == 1 { "Copy" } else { "Copy the answer" }));
        }
        hints.push(KeyHint::new("Space", "Follow up"));
        hints.push(KeyHint::new("Esc", "Back"));
    }
    div()
        .absolute()
        .left(at.left())
        .top(at.top())
        .w(at.size.width)
        .h(at.size.height)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::SPACING))
        .children(hints)
        .child(div().flex_1())
        .children(a.copied.map(|what| div().text_size(px(style::DETAIL_SIZE)).text_color(c(p.panel_text)).child(what)))
        .into_any_element()
}

use gpui::prelude::FluentBuilder as _;
