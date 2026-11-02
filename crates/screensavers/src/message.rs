//! Message: a fortune, its words scattered huge across a field of one
//! colour, and the whole text in a box at the bottom. (R5's took fortune's
//! output; ours carries its own sayings, or your text.)

use crate::{Anchor, Color32, Frame, Info, Module, Opt, Rect, Rng, get_text, hsv, pos2, vec2};

pub const MESSAGE: Info = Info {
    name: "Message",
    about: "Message, by Jon Watte",
    options: &[Opt::Text { key: "text", label: "Text (empty for sayings)", default: "" }],
    uses_desktop: false,
    make: |o, seed| {
        let mut m = Message {
            rng: Rng::new(seed),
            custom: get_text(o, "text", "").trim().to_owned(),
            page: Page::default(),
            next: 0.0,
        };
        m.turn();
        Box::new(m)
    },
};

/// Short sayings in the spirit of fortune(6).
const SAYINGS: &[&str] = &[
    "The best way to predict the future is to invent it.\n    -- Alan Kay",
    "Simplicity is prerequisite for reliability.\n    -- Edsger W. Dijkstra",
    "Well begun is half done.\n    -- Aristotle",
    "Nothing in life is to be feared, it is only to be understood.\n    -- Marie Curie",
    "The only way to do great work is to love what you do.",
    "We are what we repeatedly do.",
    "Knowledge speaks, but wisdom listens.\n    -- Jimi Hendrix",
    "Do not go where the path may lead, go instead where there is no path and leave a trail.\n    -- Ralph Waldo Emerson",
    "It always seems impossible until it's done.\n    -- Nelson Mandela",
    "Be curious, not judgmental.\n    -- Walt Whitman",
    "The journey of a thousand miles begins with one step.\n    -- Lao Tzu",
    "Quality is not an act, it is a habit.",
    "Premature optimization is the root of all evil.\n    -- Donald Knuth",
    "Any sufficiently advanced technology is indistinguishable from magic.\n    -- Arthur C. Clarke",
    "Imagination is more important than knowledge.\n    -- Albert Einstein",
    "The computer was born to solve problems that did not exist before.\n    -- Bill Gates",
    "In the middle of difficulty lies opportunity.",
    "Make it work, make it right, make it fast.\n    -- Kent Beck",
    "Talk is cheap. Show me the code.\n    -- Linus Torvalds",
    "Programs must be written for people to read, and only incidentally for machines to execute.\n    -- Harold Abelson",
    "Be the change that you wish to see in the world.",
    "There is no place like 127.0.0.1",
    "If at first you don't succeed, call it version 1.0.",
    "Hofstadter's Law: It always takes longer than you expect, even when you take into account Hofstadter's Law.",
    "Your hard work is only as valuable as it is lucrative. Buy Now!",
    "Life is what happens while you are busy making other plans.\n    -- John Lennon",
    "A ship in harbor is safe, but that is not what ships are built for.",
    "Fortune favors the bold.",
];

#[derive(Default)]
struct Page {
    text: String,
    hue: f32,
    /// Words placed so far: text, position (fraction), size, value.
    words: Vec<(String, (f32, f32), f32, f32)>,
    /// Words still to place, and time to the next.
    queue: Vec<String>,
    age: f32,
}

struct Message {
    rng: Rng,
    custom: String,
    page: Page,
    next: f32,
}

const PAGE: f32 = 15.0;
/// Word sizes at k = 1 (few, so the glyph atlas stays small).
const SIZES: [f32; 3] = [56.0, 110.0, 200.0];

impl Message {
    fn turn(&mut self) {
        let text = if self.custom.is_empty() {
            SAYINGS[self.rng.below(SAYINGS.len())].to_owned()
        } else {
            self.custom.clone()
        };
        let words: Vec<String> = text
            .split_whitespace()
            .filter(|w| *w != "--")
            .map(str::to_owned)
            .collect();
        // Enough words to cover the screen: the message, again and again.
        let queue: Vec<String> = words.iter().cycle().take(words.len().max(1) * (40 / words.len().max(1) + 1)).cloned().collect();
        self.page = Page { text, hue: self.rng.f32(), words: Vec::new(), queue, age: 0.0 };
        self.page.queue.reverse();
    }
}

impl Module for Message {
    fn draw(&mut self, f: &Frame) {
        self.page.age += f.dt;
        if self.page.age > PAGE {
            self.turn();
        }
        self.next -= f.dt;
        while self.next <= 0.0 {
            self.next += 0.12;
            let Some(word) = self.page.queue.pop() else { break };
            let size = SIZES[self.rng.below(SIZES.len())];
            let at = (self.rng.range(-0.15, 0.95), self.rng.range(-0.05, 1.0));
            let value = self.rng.range(0.5, 0.7);
            self.page.words.push((word, at, size, value));
        }
        let hue = self.page.hue;
        f.canvas.rect_filled(f.rect, 0.0, hsv(hue, 1.0, 0.44));
        let k = f.k();
        // The words, hundreds of pixels high.
        for (word, at, size, value) in &self.page.words {
            let size = (size * k).round().max(2.0);
            let pos = pos2(f.rect.left() + at.0 * f.rect.width(), f.rect.top() + at.1 * f.rect.height());
            let color = hsv(hue, 1.0, *value).gamma_multiply(0.45);
            f.canvas.big_text(word, size, pos, Anchor::LeftCenter, color);
        }
        // The message itself, in a dark box near the bottom.
        let font = (24.0 * k).round().max(2.0);
        let wrap = f.rect.width() * 0.7;
        let text = f.canvas.measure(&self.page.text, font, wrap);
        let pad = vec2(14.0, 8.0) * k;
        let size = text + pad * 2.0;
        let r = Rect::from_min_size(
            pos2(f.rect.center().x - size.x / 2.0, f.rect.bottom() - size.y - 0.08 * f.rect.height()),
            size,
        );
        f.canvas.rect_filled(r, 6.0 * k, Color32::from_black_alpha(90));
        f.canvas.text(r.min + pad, &self.page.text, font, wrap, hsv(hue, 0.55, 1.0));
    }
}
