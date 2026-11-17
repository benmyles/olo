//! The text read in a picture, selected as text is: where a point
//! falls in it, what lies between two places, and the boxes that show a
//! selection. Only the geometry (in the picture's own pixels):
//! [`super::image_view::ImageView`] draws it and takes the pointer, and
//! whoever shows the picture has it read (olo-ocr).

/// A line of text in a picture: its characters in reading order (the
/// spaces between its words among them), each with where it is (x, y,
/// width, height, in the pixels of the picture it was read from).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextLine {
    pub chars: Vec<(char, [f32; 4])>,
}

/// A place in a picture's text: a line, and a boundary between its
/// characters (0 before its first, its length after its last).
pub type Place = (usize, usize);

impl TextLine {
    /// The box round the line.
    pub fn rect(&self) -> [f32; 4] {
        span(&self.chars).unwrap_or([0.0; 4])
    }

    pub fn text(&self) -> String {
        self.chars.iter().map(|c| c.0).collect()
    }
}

/// The box round some characters.
fn span(chars: &[(char, [f32; 4])]) -> Option<[f32; 4]> {
    let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (_, [x, y, w, h]) in chars {
        (l, t, r, b) = (l.min(*x), t.min(*y), r.max(x + w), b.max(y + h));
    }
    (!chars.is_empty()).then_some([l, t, r - l, b - t])
}

/// The line a point is on: inside its box grown by `slack` (a part of
/// the line's height) all round. None: not on text.
pub fn line_at(lines: &[TextLine], [x, y]: [f32; 2], slack: f32) -> Option<usize> {
    lines.iter().position(|line| {
        let [lx, ly, lw, lh] = line.rect();
        let s = lh * slack;
        !line.chars.is_empty() && x >= lx - s && x <= lx + lw + s && y >= ly - s && y <= ly + lh + s
    })
}

/// The place nearest a point: on the line the point is level with, or
/// the nearest above or below, at the boundary nearest its x. None with
/// no text.
pub fn place_at(lines: &[TextLine], [x, y]: [f32; 2]) -> Option<Place> {
    let away = |line: &TextLine| {
        let [_, ly, _, lh] = line.rect();
        if y < ly {
            ly - y
        } else if y > ly + lh {
            y - (ly + lh)
        } else {
            0.0
        }
    };
    let (i, line) = lines.iter().enumerate().filter(|(_, l)| !l.chars.is_empty()).min_by(|a, b| away(a.1).total_cmp(&away(b.1)))?;
    // (Past a character's middle: after it.)
    let at = line.chars.iter().position(|(_, [cx, _, cw, _])| x < cx + cw / 2.0).unwrap_or(line.chars.len());
    Some((i, at))
}

/// The word at a place: the run of characters that aren't spaces round
/// it (the place itself, on a space).
pub fn word_at(lines: &[TextLine], (i, at): Place) -> (Place, Place) {
    let Some(line) = lines.get(i) else { return ((i, at), (i, at)) };
    let space = |k: usize| line.chars.get(k).is_none_or(|c| c.0.is_whitespace());
    let at = at.min(line.chars.len());
    // (At a word's end: that word.)
    let on = if space(at) && at > 0 && !space(at - 1) { at - 1 } else { at };
    if space(on) {
        return ((i, at), (i, at));
    }
    let (mut from, mut to) = (on, on + 1);
    while from > 0 && !space(from - 1) {
        from -= 1;
    }
    while !space(to) {
        to += 1;
    }
    ((i, from), (i, to))
}

/// Two places in reading order.
fn ordered(a: Place, b: Place) -> (Place, Place) {
    if a <= b { (a, b) } else { (b, a) }
}

/// The text between two places, a line's end a new line.
pub fn text_between(lines: &[TextLine], a: Place, b: Place) -> String {
    let (a, b) = ordered(a, b);
    let mut out = String::new();
    for i in a.0..=b.0.min(lines.len().saturating_sub(1)) {
        let Some(line) = lines.get(i) else { break };
        let from = if i == a.0 { a.1.min(line.chars.len()) } else { 0 };
        let to = if i == b.0 { b.1.min(line.chars.len()) } else { line.chars.len() };
        out.extend(line.chars[from..to.max(from)].iter().map(|c| c.0));
        if i < b.0 {
            // (A line's own trailing space isn't kept before its break.)
            while out.ends_with(' ') {
                out.pop();
            }
            out.push('\n');
        }
    }
    out
}

/// The boxes that show what's between two places: one a line, round
/// its selected characters at the line's own height.
pub fn boxes_between(lines: &[TextLine], a: Place, b: Place) -> Vec<[f32; 4]> {
    let (a, b) = ordered(a, b);
    let mut out = Vec::new();
    for i in a.0..=b.0.min(lines.len().saturating_sub(1)) {
        let Some(line) = lines.get(i) else { break };
        let from = if i == a.0 { a.1.min(line.chars.len()) } else { 0 };
        let to = if i == b.0 { b.1.min(line.chars.len()) } else { line.chars.len() };
        if let Some([x, _, w, _]) = span(&line.chars[from..to.max(from)]) {
            let [_, y, _, h] = line.rect();
            out.push([x, y, w, h]);
        }
    }
    out
}

/// All of it: from the first line's start to the last's end.
pub fn all(lines: &[TextLine]) -> Option<(Place, Place)> {
    let last = lines.len().checked_sub(1)?;
    Some(((0, 0), (last, lines[last].chars.len())))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Hi there" at (10, 10) and "Olo" at (10, 40): each character 10
    /// wide and 20 tall.
    fn two() -> Vec<TextLine> {
        let line = |text: &str, y: f32| TextLine { chars: text.chars().enumerate().map(|(i, c)| (c, [10.0 + 10.0 * i as f32, y, 10.0, 20.0])).collect() };
        vec![line("Hi there", 10.0), line("Olo", 40.0)]
    }

    #[test]
    fn where_a_point_falls() {
        let t = two();
        assert_eq!(t[0].rect(), [10.0, 10.0, 80.0, 20.0]);
        assert_eq!(line_at(&t, [15.0, 15.0], 0.2), Some(0));
        assert_eq!(line_at(&t, [15.0, 45.0], 0.2), Some(1));
        assert_eq!(line_at(&t, [8.0, 8.0], 0.2), Some(0), "within the slack");
        assert_eq!(line_at(&t, [200.0, 15.0], 0.2), None);
        assert_eq!(line_at(&t, [15.0, 100.0], 0.2), None);
        // The boundary nearest: before a character's middle, before it.
        assert_eq!(place_at(&t, [12.0, 15.0]), Some((0, 0)));
        assert_eq!(place_at(&t, [16.0, 15.0]), Some((0, 1)));
        assert_eq!(place_at(&t, [500.0, 15.0]), Some((0, 8)), "past its end");
        assert_eq!(place_at(&t, [0.0, 300.0]), Some((1, 0)), "below it all: the last line");
        assert_eq!(place_at(&t, [44.0, 0.0]), Some((0, 3)), "above it all: the first");
        assert_eq!(place_at(&[], [0.0, 0.0]), None);
    }

    #[test]
    fn words_and_what_lies_between() {
        let t = two();
        assert_eq!(word_at(&t, (0, 4)), ((0, 3), (0, 8)), "there");
        assert_eq!(word_at(&t, (0, 1)), ((0, 0), (0, 2)), "Hi");
        assert_eq!(word_at(&t, (0, 2)), ((0, 0), (0, 2)), "at a word's end: that word");
        assert_eq!(word_at(&t, (0, 8)), ((0, 3), (0, 8)));
        assert_eq!(word_at(&t, (1, 0)), ((1, 0), (1, 3)));
        assert_eq!(text_between(&t, (0, 3), (0, 8)), "there");
        assert_eq!(text_between(&t, (1, 2), (0, 3)), "there\nOl", "either way round");
        assert_eq!(text_between(&t, (0, 0), (1, 3)), "Hi there\nOlo");
        assert_eq!(text_between(&t, (0, 2), (0, 2)), "");
        let (a, b) = all(&t).unwrap();
        assert_eq!(text_between(&t, a, b), "Hi there\nOlo");
        assert_eq!(all(&[]), None);
        // A box a line: the selected characters, the line's height.
        assert_eq!(boxes_between(&t, (0, 3), (1, 2)), [[40.0, 10.0, 50.0, 20.0], [10.0, 40.0, 20.0, 20.0]]);
        assert_eq!(boxes_between(&t, (0, 2), (0, 2)), [] as [[f32; 4]; 0]);
    }
}
