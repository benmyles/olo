//! Printing, natively: pages of positioned text and shapes made into a
//! PDF (`pdf`, fonts embedded and subset, text selectable), and sent to
//! a printer through CUPS over IPP (`ipp`), or saved. Paper sizes by
//! their PWG names, as CUPS gives them (`Paper`).

pub mod ipp;
pub mod panel;
mod pdf;

pub use pdf::pdf;

/// A colour, 0–255 each.
pub type Rgb = [u8; 3];

/// A font as a document names it: a family (empty is Olo's own), bold,
/// italic, or monospaced.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Font {
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
}

/// Something drawn on a page, in points from its top left.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// Text whose baseline starts at (`x`, `y`), `width` long (as the app
    /// laid it out: its underline and strike-through run so far).
    Text { x: f32, y: f32, text: String, font: Font, size: f32, color: Rgb, width: f32, underline: bool, strike: bool },
    Rect { x: f32, y: f32, w: f32, h: f32, color: Rgb },
    Line { from: (f32, f32), to: (f32, f32), width: f32, color: Rgb },
    /// A picture (a PNG, JPEG, GIF or WebP file's bytes) filling the
    /// box whose top left is (`x`, `y`).
    Image { x: f32, y: f32, w: f32, h: f32, data: std::sync::Arc<Vec<u8>> },
}

/// One page: its size (points) and what is on it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Page {
    pub width: f32,
    pub height: f32,
    pub items: Vec<Item>,
}

/// A paper size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Paper {
    /// Its PWG name ("iso_a4_210x297mm"), as CUPS knows it.
    pub pwg: &'static str,
    pub label: &'static str,
    /// Points.
    pub width: f32,
    pub height: f32,
}

pub const PAPERS: [Paper; 6] = [
    Paper { pwg: "na_letter_8.5x11in", label: "US Letter", width: 612.0, height: 792.0 },
    Paper { pwg: "na_legal_8.5x14in", label: "US Legal", width: 612.0, height: 1008.0 },
    Paper { pwg: "iso_a4_210x297mm", label: "A4", width: 595.28, height: 841.89 },
    Paper { pwg: "iso_a5_148x210mm", label: "A5", width: 419.53, height: 595.28 },
    Paper { pwg: "iso_a3_297x420mm", label: "A3", width: 841.89, height: 1190.55 },
    Paper { pwg: "na_executive_7.25x10.5in", label: "Executive", width: 522.0, height: 756.0 },
];

/// The paper people use where the person is: US Letter in the US,
/// Canada and a few others (`LC_PAPER`, else `LANG`), else A4.
pub fn local_paper() -> Paper {
    let lang = ["LC_PAPER", "LC_ALL", "LANG"].iter().filter_map(|v| std::env::var(v).ok()).find(|v| !v.is_empty() && v != "C").unwrap_or_default();
    let region = lang.split(['.', '@']).next().unwrap_or("").rsplit('_').next().unwrap_or("").to_ascii_uppercase();
    let letter = ["US", "CA", "MX", "PH", "CL", "CO", "VE", "PR"].contains(&region.as_str());
    PAPERS[if letter { 0 } else { 2 }]
}

/// A paper by its PWG name.
pub fn paper(pwg: &str) -> Option<Paper> {
    PAPERS.iter().find(|p| p.pwg == pwg).copied()
}
