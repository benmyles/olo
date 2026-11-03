//! The palm trainer's typing exercise, as the typing tutors of the 1990s
//! had it: a lesson on ruled paper with the next letter marked, and a
//! keyboard below with each finger's keys in its colour, the next key
//! lit and the finger for it ringed, the hands resting on the home row
//! and their heels on the pad; speed and accuracy as you go. Something to
//! type takes the thinking out of typing, so the palms rest as they do.
//! Drawn in Olo's own style from the palette's hues.

use std::time::Instant;

use olo_theme::{Hue, Palette, Rgba, mix, readable};
use olo_ui::{
    agent,
    gpui::{App, Bounds, FontWeight, IntoElement, Pixels, Styled as _, Window, canvas, point, px, size},
    style,
};

/// What to type, in turn: whole sentences, lower case, the letters
/// spread over every finger.
pub const LINES: [&str; 6] = [
    "the quick brown fox jumps over the lazy dog",
    "pack my box with five dozen liquor jugs",
    "how vexingly quick daft zebras jump",
    "sphinx of black quartz judge my vow",
    "the five boxing wizards jump quickly",
    "jackdaws love my big sphinx of quartz",
];

/// The typing so far.
#[derive(Default)]
pub struct Tutor {
    pub line: usize,
    pub typed: usize,
    /// When a wrong key was last pressed (the next letter shows red).
    pub wrong: Option<Instant>,
    correct: usize,
    misses: usize,
    started: Option<Instant>,
}

impl Tutor {
    pub fn text(&self) -> &'static str {
        LINES[self.line % LINES.len()]
    }

    pub fn next(&self) -> Option<char> {
        self.text().chars().nth(self.typed)
    }

    /// A key typed: the next letter (on to the one after; a line done,
    /// the next line), or a miss.
    pub fn key(&mut self, ch: char) {
        self.started.get_or_insert_with(Instant::now);
        if self.next() == Some(ch.to_ascii_lowercase()) {
            self.typed += 1;
            self.correct += 1;
            self.wrong = None;
            if self.typed >= self.text().chars().count() {
                self.line += 1;
                self.typed = 0;
            }
        } else {
            self.misses += 1;
            self.wrong = Some(Instant::now());
        }
    }

    /// Words a minute (five letters a word), once there's a little to go
    /// on.
    pub fn wpm(&self) -> Option<f32> {
        let mins = self.started?.elapsed().as_secs_f32() / 60.0;
        (self.correct >= 5 && mins > 0.0).then(|| self.correct as f32 / 5.0 / mins)
    }

    pub fn accuracy(&self) -> Option<f32> {
        let all = self.correct + self.misses;
        (all > 0).then(|| self.correct as f32 / all as f32)
    }

    pub fn wrong_now(&self) -> bool {
        self.wrong.is_some_and(|w| w.elapsed().as_millis() < 400)
    }
}

/// Which finger types `ch` (touch typing's map): 0 to 3 the left hand's
/// little finger to index, 4 to 7 the right's index to little, 8 the
/// thumbs.
fn finger(ch: char) -> usize {
    match ch {
        '`' | '1' | 'q' | 'a' | 'z' => 0,
        '2' | 'w' | 's' | 'x' => 1,
        '3' | 'e' | 'd' | 'c' => 2,
        '4' | '5' | 'r' | 't' | 'f' | 'g' | 'v' | 'b' => 3,
        '6' | '7' | 'y' | 'u' | 'h' | 'j' | 'n' | 'm' => 4,
        '8' | 'i' | 'k' | ',' => 5,
        '9' | 'o' | 'l' | '.' => 6,
        ' ' => 8,
        _ => 7,
    }
}

/// Each finger's colour, the little fingers' to the thumbs'.
const FINGER_HUES: [Hue; 9] = [Hue::Red, Hue::Orange, Hue::Yellow, Hue::Green, Hue::Teal, Hue::Blue, Hue::Purple, Hue::Pink, Hue::Gray];

/// A key: what it types (none: a modifier), its label, and its width in
/// keys.
type Key = (Option<char>, &'static str, f32);

/// A Mac keyboard's rows, each 14.5 keys wide.
fn rows() -> [Vec<Key>; 5] {
    let keys = |s: &'static str| -> Vec<Key> {
        s.char_indices().map(|(i, c)| (Some(c), &s[i..i + c.len_utf8()], 1.0)).collect()
    };
    let mut r0 = keys("`1234567890-=");
    r0.push((None, "delete", 1.5));
    let mut r1 = vec![(None, "tab", 1.5)];
    r1.extend(keys("qwertyuiop[]\\"));
    let mut r2 = vec![(None, "caps", 1.75)];
    r2.extend(keys("asdfghjkl;'"));
    r2.push((None, "return", 1.75));
    let mut r3 = vec![(None, "shift", 2.25)];
    r3.extend(keys("zxcvbnm,./"));
    r3.push((None, "shift", 2.25));
    let r4 = vec![(None, "⌃", 1.25), (None, "⌥", 1.25), (None, "⌘", 1.75), (Some(' '), "", 6.0), (None, "⌘", 1.75), (None, "⌥", 1.25), (None, "⌃", 1.25)];
    [r0, r1, r2, r3, r4]
}

fn c(p: Rgba) -> olo_ui::gpui::Hsla {
    style::c(p)
}

/// The paper a lesson is on: the page's colour, warmed.
fn paper(p: &Palette) -> Rgba {
    mix(p.document_background, p.hue(Hue::Yellow), 0.08)
}

/// The lesson: the line to type on a strip of ruled paper (a red margin,
/// a blue rule), typed letters in ink, the next one marked as a
/// highlighter would (red after a wrong key), the rest pale.
pub fn lesson(tutor: &Tutor) -> impl IntoElement {
    let text = tutor.text();
    let typed = tutor.typed.min(text.len());
    let wrong = tutor.wrong_now();
    let speed = match (tutor.wpm(), tutor.accuracy()) {
        (Some(w), Some(a)) => format!("{w:.0} words a minute · {:.0}% right", a * 100.0),
        (None, Some(a)) => format!("{:.0}% right", a * 100.0),
        _ => "Start typing".to_owned(),
    };
    canvas(
        |_, _, _| {},
        move |b, _, window, cx| {
            let p = olo_ui::theme::palette(cx);
            let r = style::snap(window, b);
            let sheet = paper(&p);
            style::fill_rect(window, r, c(sheet));
            style::sunken(window, r, &p, false);
            let inner = style::inset(window, r, 2.0);
            let size_pt = 22.0;
            let font = style::mono_font(FontWeight::NORMAL);
            let baseline_y = inner.top() + px(f32::from(inner.size.height) * 0.48);
            // The rule under the line, and the margin.
            let rule = mix(sheet, p.hue(Hue::Blue), 0.35);
            style::fill_rect(window, style::snap(window, Bounds::new(point(inner.left(), baseline_y + px(size_pt * 0.42)), size(inner.size.width, px(1.0)))), c(rule));
            let margin_x = inner.left() + px(48.0);
            style::fill_rect(window, style::snap(window, Bounds::new(point(margin_x, inner.top()), size(px(1.0), inner.size.height))), c(mix(sheet, p.hue(Hue::Red), 0.45)));
            let ink = readable(p.document_text, sheet, 7.0);
            let pale = mix(ink, sheet, 0.6);
            let mut x = margin_x + px(16.0);
            let (done, rest) = text.split_at(typed);
            let mut chars = rest.chars();
            let next = chars.next();
            let after: String = chars.collect();
            let y = baseline_y - px(size_pt * style::CAP_HEIGHT / 2.0);
            if !done.is_empty() {
                x = style::label(window, cx, x, y, style::Align::Left, done.to_owned(), size_pt, font.clone(), c(ink)).right();
            }
            if let Some(n) = next {
                let shown = if n == ' ' { "␣".to_owned() } else { n.to_string() };
                let w = style::text_width(window, "m", size_pt, font.clone());
                let mark = if wrong { p.failure } else { p.hue(Hue::Yellow) };
                let block = style::snap(window, Bounds::new(point(x - px(1.0), y - px(size_pt * 0.62)), size(w + px(2.0), px(size_pt * 1.24))));
                style::fill_rect(window, block, c(mix(sheet, mark, 0.55)));
                // The caret under it.
                style::fill_rect(window, style::snap(window, Bounds::new(point(block.left(), block.bottom() - px(2.0)), size(block.size.width, px(2.0)))), c(readable(mark, sheet, 3.0)));
                let r = style::label(window, cx, x, y, style::Align::Left, shown, size_pt, font.clone(), c(ink));
                x = r.left() + w;
            }
            if !after.is_empty() {
                style::label(window, cx, x, y, style::Align::Left, after.clone(), size_pt, font.clone(), c(pale));
            }
            // Speed and accuracy, small at the lower right.
            style::label(window, cx, inner.right() - px(12.0), inner.bottom() - px(12.0), style::Align::Right, speed.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), c(mix(ink, sheet, 0.35)));
            agent::record(window, cx, agent::Node { role: "Label", name: Some("Type this".into()), value: Some(text.into()), bounds: b, enabled: true, ..Default::default() });
        },
    )
    .w_full()
    .h(px(96.0))
}

/// The keyboard, each finger's keys in its colour, the next key (`next`)
/// lit; the fingers resting on the home row (the one for the next key
/// ringed) and the heels of the hands below, on the pad.
pub fn keyboard(window: &mut Window, cx: &mut App, b: Bounds<Pixels>, next: Option<char>, p: &Palette) {
    let r = style::snap(window, b);
    style::sunken(window, r, p, false);
    let deck = style::inset(window, r, 2.0);
    style::fill_rect(window, deck, style::shade(p, p.panel_background, 0.94));
    let (dw, dh) = (f32::from(deck.size.width), f32::from(deck.size.height));
    let u = (dw * 0.92 / 14.5).min(dh * 0.62 / 5.0);
    let left = deck.left() + px((dw - u * 14.5) / 2.0);
    let top = deck.top() + px(dh * 0.06);
    let font = style::ui_font(FontWeight::NORMAL);
    let label_pt = (u * 0.4).clamp(8.0, 13.0);
    let next_finger = next.map(finger);
    let mut homes: Vec<(usize, olo_ui::gpui::Point<Pixels>)> = Vec::new();
    for (row, keys) in rows().iter().enumerate() {
        let mut x = 0.0;
        for &(ch, label, w) in keys {
            let k = style::snap(window, Bounds::new(point(left + px(x * u + 1.5), top + px(row as f32 * u + 1.5)), size(px(w * u - 3.0), px(u - 3.0))));
            x += w;
            let hue = match ch {
                Some(ch) => p.hue(FINGER_HUES[finger(ch)]),
                None => p.hue(FINGER_HUES[if x < 7.0 { 0 } else { 7 }]),
            };
            let lit = ch.is_some() && ch == next;
            let fill = if lit { hue } else { mix(p.control_background, hue, if ch.is_some() { 0.32 } else { 0.14 }) };
            style::fill_rect(window, k, c(fill));
            style::raised(window, k, p);
            if lit {
                style::frame_rect(window, style::inset(window, k, -2.0), c(p.keyboard_navigation));
            }
            let ink = readable(p.control_text, fill, 4.5);
            let pt = if label.chars().count() > 1 { label_pt * 0.75 } else { label_pt };
            style::label(window, cx, k.center().x, k.center().y, style::Align::Center, label.to_owned(), pt, font.clone(), c(ink));
            // The home keys' bumps, and where the fingers rest.
            if let Some(ch) = ch {
                if matches!(ch, 'f' | 'j') {
                    style::fill_rect(window, style::snap(window, Bounds::new(point(k.center().x - px(u * 0.15), k.bottom() - px(u * 0.2)), size(px(u * 0.3), px(1.5)))), c(ink));
                }
                if "asdfjkl;".contains(ch) || (ch == ' ' && row == 4) {
                    homes.push((finger(ch), k.center()));
                }
            }
        }
    }
    // The fingertips on the home row (both thumbs on the space bar), the
    // one for the next key ringed.
    for (f, at) in homes {
        let at = if f == 8 { at } else { point(at.x, at.y - px(u * 0.05)) };
        let tips: Vec<olo_ui::gpui::Point<Pixels>> = if f == 8 { vec![point(at.x - px(u * 1.2), at.y), point(at.x + px(u * 1.2), at.y)] } else { vec![at] };
        for t in tips {
            let ink = c(p.hue(FINGER_HUES[f])).opacity(0.55);
            crate::palm_trainer::ellipse(window, t, u * 0.22, u * 0.22, ink);
            if next_finger == Some(f) {
                crate::palm_trainer::ring(window, t, u * 0.34, u * 0.34, c(p.keyboard_navigation));
            }
        }
    }
    // The heels of the hands, resting below the keys (on the pad).
    let heel_y = top + px(5.0 * u + (dh * 0.94 - 5.0 * u) * 0.55);
    for side in [-1.0f32, 1.0] {
        let at = point(deck.center().x + px(side * u * 3.4), heel_y);
        crate::palm_trainer::ellipse(window, at, u * 1.5, u * 0.75, c(p.warning).opacity(0.75));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_as_wide_as_a_keyboard() {
        for row in rows() {
            let w: f32 = row.iter().map(|k| k.2).sum();
            assert!((w - 14.5).abs() < 1e-4, "{w}");
        }
    }

    #[test]
    fn typing_follows_the_line() {
        let mut t = Tutor::default();
        for ch in "the quick".chars() {
            t.key(ch);
        }
        assert_eq!(t.typed, 9);
        t.key('x');
        assert_eq!(t.typed, 9);
        assert!(t.wrong_now());
        assert_eq!(t.accuracy(), Some(0.9));
        // A line done, the next.
        for ch in t.text()[9..].chars().collect::<Vec<_>>() {
            t.key(ch);
        }
        assert_eq!((t.line, t.typed), (1, 0));
        // Every key of every line has a finger and a key on the board.
        let all: String = rows().iter().flatten().filter_map(|k| k.0).collect();
        for line in LINES {
            assert!(line.chars().all(|ch| all.contains(ch)), "{line}");
        }
    }
}
