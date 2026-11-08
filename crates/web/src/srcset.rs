//! Choosing an image's source (HTML "selecting an image source"): the first
//! `<source>` of its `<picture>` whose `media` and `type` fit, else the
//! image's own `srcset`, each candidate by its density (`2x`) or width
//! (`800w`, against `sizes`), the smallest that is at least the display's
//! density; `src` stands for `1x` beside density candidates.
use crate::{
    css::{self, media::Media},
    dom::{Document, NodeId},
};
use std::sync::atomic::AtomicBool;

/// One candidate: its URL as written and its density (device pixels per
/// CSS pixel it gives at its natural size).
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub url: String,
    pub density: f64,
}
/// The source `img` shows on `media`, or None when it names none.
pub fn choose(doc: &Document, img: NodeId, media: &Media, cancel: Option<&AtomicBool>) -> Option<Choice> {
    let attr = |n: NodeId, name: &str| doc.attribute(n, name).ok().flatten().map(str::to_owned);
    let src = attr(img, "src").filter(|s| !s.trim().is_empty());
    // A <picture>'s sources before the image, in order.
    if let Some(parent) = doc.parent(img).ok().flatten()
        && doc.name(parent).ok().flatten() == Some("picture")
        && let Ok(children) = doc.children(parent)
    {
        for child in children {
            if child == img {
                break;
            }
            if doc.name(child).ok().flatten() != Some("source") {
                continue;
            }
            if let Some(query) = attr(child, "media").filter(|m| !m.trim().is_empty())
                && !matches(&query, media, cancel)
            {
                continue;
            }
            if let Some(kind) = attr(child, "type").filter(|t| !t.trim().is_empty())
                && !supported_type(&kind)
            {
                continue;
            }
            let Some(set) = attr(child, "srcset").filter(|s| !s.trim().is_empty()) else { continue };
            let candidates = parse(&set, attr(child, "sizes").as_deref(), media, cancel);
            if let Some(choice) = pick(candidates, media.scale) {
                return Some(choice);
            }
        }
    }
    let mut candidates = attr(img, "srcset").map(|set| parse(&set, attr(img, "sizes").as_deref(), media, cancel)).unwrap_or_default();
    // `src` is the 1x candidate when the set has none (and no widths).
    if let Some(src) = &src
        && !candidates.iter().any(|c| c.width.is_some() || c.density == Some(1.))
    {
        candidates.push(Candidate { url: src.clone(), density: Some(1.), width: None, resolved: 1. });
    }
    pick(candidates, media.scale).or(src.map(|url| Choice { url, density: 1. }))
}
struct Candidate {
    url: String,
    density: Option<f64>,
    width: Option<f64>,
    /// Its density, a width's against the source size.
    resolved: f64,
}
/// The smallest candidate at least `scale` dense, else the densest.
fn pick(candidates: Vec<Candidate>, scale: f64) -> Option<Choice> {
    let fits = candidates.iter().filter(|c| c.resolved >= scale - 1e-9).min_by(|a, b| a.resolved.total_cmp(&b.resolved));
    let best = fits.or_else(|| candidates.iter().max_by(|a, b| a.resolved.total_cmp(&b.resolved)))?;
    Some(Choice { url: best.url.clone(), density: best.resolved })
}
/// HTML "parse a srcset attribute": URLs with optional `Nw`, `Nx` (or
/// ignored `Nh`) descriptors; widths resolved against `sizes`.
fn parse(set: &str, sizes: Option<&str>, media: &Media, cancel: Option<&AtomicBool>) -> Vec<Candidate> {
    let mut out = Vec::new();
    let size = sizes.map_or(media.width, |s| source_size(s, media, cancel));
    let bytes = set.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b',') {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut url = &set[start..i];
        if url.is_empty() {
            break;
        }
        let mut descriptors = String::new();
        if url.ends_with(',') {
            url = url.trim_end_matches(',');
        } else {
            // Descriptors to the next comma outside parentheses.
            let mut depth = 0;
            while i < bytes.len() {
                match bytes[i] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    b',' if depth <= 0 => {
                        i += 1;
                        break;
                    }
                    _ => {}
                }
                descriptors.push(bytes[i] as char);
                i += 1;
            }
        }
        let (mut density, mut width, mut bad) = (None, None, false);
        for d in descriptors.split_ascii_whitespace() {
            let (value, unit) = d.split_at(d.len().saturating_sub(1));
            match (unit, value.parse::<f64>()) {
                ("x", Ok(v)) if v > 0. && density.is_none() && width.is_none() => density = Some(v),
                ("w", Ok(v)) if v > 0. && width.is_none() && density.is_none() => width = Some(v),
                ("h", Ok(_)) => {}
                _ => bad = true,
            }
        }
        if bad || url.is_empty() {
            continue;
        }
        let resolved = match (density, width) {
            (Some(d), _) => d,
            (None, Some(w)) if size > 0. => w / size,
            (None, Some(_)) => 1.,
            (None, None) => 1.,
        };
        out.push(Candidate { url: url.to_owned(), density: density.or(width.is_none().then_some(1.)), width, resolved });
    }
    out
}
/// The width `sizes` gives (CSS px): its first entry whose condition holds,
/// else the last; 100vw when none can be read.
fn source_size(sizes: &str, media: &Media, cancel: Option<&AtomicBool>) -> f64 {
    let mut entries = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    for (i, c) in sizes.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                entries.push(&sizes[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    entries.push(&sizes[start..]);
    for entry in entries {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        // The length is the last component: after the condition's ')'.
        let (condition, length) = match entry.rfind(')') {
            Some(close) if !entry[close + 1..].trim().is_empty() => (&entry[..=close], entry[close + 1..].trim()),
            _ => ("", entry),
        };
        let Some(px) = length_px(length, media) else { continue };
        if condition.is_empty() || matches(condition, media, cancel) {
            return px;
        }
    }
    media.width
}
/// A source-size length in CSS px: px, vw, vh, em and rem (16px), or
/// `auto` and anything else read as 100vw.
fn length_px(text: &str, media: &Media) -> Option<f64> {
    let text = text.trim().to_ascii_lowercase();
    if text == "auto" || text.starts_with("calc(") || text.starts_with("min(") || text.starts_with("max(") || text.starts_with("clamp(") {
        return Some(media.width);
    }
    let unit = text.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-' || c == '+');
    let value: f64 = text[..text.len() - unit.len()].parse().ok()?;
    Some(match unit {
        "px" => value,
        "vw" => value * media.width / 100.,
        "vh" => value * media.height / 100.,
        "em" | "rem" => value * 16.,
        "" if value == 0. => 0.,
        _ => return None,
    })
}
/// Whether a media query list holds on `media`.
fn matches(query: &str, media: &Media, cancel: Option<&AtomicBool>) -> bool {
    css::media::matches(query, media, cancel)
}
/// The image types the engine decodes.
fn supported_type(kind: &str) -> bool {
    let kind = kind.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    matches!(kind.as_str(), "image/png" | "image/jpeg" | "image/jpg" | "image/webp" | "image/gif" | "image/svg+xml" | "image/avif" | "image/bmp" | "image/x-icon")
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    fn pick_in(html: &str, width: f64, height: f64, scale: f64) -> Option<Choice> {
        let doc = crate::html::parse(Arc::from(html), Default::default(), None).unwrap().document;
        let mut stack = vec![doc.root()];
        while let Some(n) = stack.pop() {
            if doc.name(n).ok().flatten() == Some("img") {
                return choose(&doc, n, &Media::screen(width, height, scale, false), None);
            }
            stack.extend(doc.children(n).unwrap());
        }
        None
    }
    #[test]
    fn sources_densities_and_widths_pick_as_html_says() {
        let picture = "<picture><source srcset='s.jpg, s2.jpg 2x' media='(max-width:734px)'><source srcset='m.jpg, m2.jpg 2x' media='(max-width:1068px) and (max-height:733px)'><source type='image/x-unknown' srcset='u.jpg'><img src='l.jpg'></picture>";
        assert_eq!(pick_in(picture, 600., 700., 1.), Some(Choice { url: "s.jpg".into(), density: 1. }));
        assert_eq!(pick_in(picture, 600., 700., 2.), Some(Choice { url: "s2.jpg".into(), density: 2. }));
        assert_eq!(pick_in(picture, 1000., 700., 1.), Some(Choice { url: "m.jpg".into(), density: 1. }));
        assert_eq!(pick_in(picture, 1400., 900., 1.), Some(Choice { url: "l.jpg".into(), density: 1. }));
        // Widths against sizes: 400px at 1x wants >= 400w; at 2x >= 800w.
        let widths = "<img srcset='a.jpg 300w, b.jpg 600w, c.jpg 1200w' sizes='(max-width: 500px) 100vw, 400px' src='z.jpg'>";
        assert_eq!(pick_in(widths, 1000., 700., 1.).unwrap().url, "b.jpg");
        assert_eq!(pick_in(widths, 1000., 700., 2.).unwrap().url, "c.jpg");
        assert_eq!(pick_in(widths, 300., 700., 1.).unwrap().url, "a.jpg");
        // src stands for 1x beside density candidates; a URL with commas.
        let dense = "<img src='one.png' srcset='data:image/png;base64,AA,BB 2x'>";
        assert_eq!(pick_in(dense, 1000., 700., 1.).unwrap().url, "one.png");
        assert_eq!(pick_in(dense, 1000., 700., 2.).unwrap().url, "data:image/png;base64,AA,BB");
        // Nothing fits: the densest.
        assert_eq!(pick_in("<img srcset='a.jpg 1x, b.jpg 1.5x'>", 1000., 700., 3.).unwrap().url, "b.jpg");
    }
}
