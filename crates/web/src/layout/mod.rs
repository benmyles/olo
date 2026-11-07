//! Original DOM box construction and normal flow geometry. Worker-only.
//! This is an incremental implementation; unsupported layout is explicit.
mod containers;
pub mod flex;
mod flow;
mod images;
pub mod positioned;
mod tree;
mod used;
pub use crate::text::prepare::ControlKind;
use crate::{
    css::{self, style},
    dom, text,
};
pub use flow::{Cache, Geometry, Job, Layout, PrepareJob, Progress, Sticky};
use std::sync::atomic::{AtomicBool, Ordering};
pub use tree::{BoxId, BoxKind, BoxNode, BuildJob, BuildLimits, BuildProgress, Control, InlineBox, SourceSpan, TextSource, Tree};
#[derive(Debug)]
pub enum Error {
    Invalid,
    Stale,
    Limit,
    Cancelled,
    Incomplete,
    Failed,
    Unsupported(&'static str),
    /// Identify the actual rejected element without retaining its document or
    /// copying attribute/content data into a diagnostic.
    UnsupportedForeign(Box<ForeignElement>),
    UnsupportedProperty(Box<UnsupportedProperty>),
    Dom(dom::Error),
    Style(style::Error),
    Numeric(css::properties::Error),
    Prepare(text::prepare::Error),
    Shaping(text::shaping::Error),
    Break(text::linebreak::Error),
    Text(text::layout::Error),
}
/// One thing the engine does not implement yet, and the node it met it on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fallback {
    pub what: &'static str,
    pub node: Option<dom::NodeId>,
}
/// What this page needed that the engine does not implement yet, and that it
/// rendered with a substitute instead (a fixed box as absolute, an unknown
/// SVG element skipped, a dashed border solid). A browser renders what it
/// understands and keeps going; refusing the page would hide everything else.
/// Counts are complete; the sample of nodes is bounded. Agents and the eval
/// read these, so a page that looks right while listing fallbacks is suspect,
/// and a page that lists none rendered with what the engine knows.
#[derive(Clone, Debug, Default)]
pub struct Fallbacks {
    counts: Vec<(&'static str, u32)>,
    sample: Vec<Fallback>,
}
impl Fallbacks {
    pub const SAMPLE: usize = 64;
    pub fn note(&mut self, what: &'static str, node: Option<dom::NodeId>) {
        match self.counts.iter_mut().find(|(w, _)| std::ptr::eq(*w, what) || *w == what) {
            Some((_, n)) => *n = n.saturating_add(1),
            None => self.counts.push((what, 1)),
        }
        if self.sample.len() < Self::SAMPLE {
            self.sample.push(Fallback { what, node });
        }
    }
    pub fn merge(&mut self, other: &Fallbacks) {
        for &(what, n) in &other.counts {
            match self.counts.iter_mut().find(|(w, _)| *w == what) {
                Some((_, m)) => *m = m.saturating_add(n),
                None => self.counts.push((what, n)),
            }
        }
        for f in &other.sample {
            if self.sample.len() >= Self::SAMPLE {
                break;
            }
            self.sample.push(*f);
        }
    }
    pub fn total(&self) -> u32 {
        self.counts.iter().fold(0u32, |a, (_, n)| a.saturating_add(*n))
    }
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }
    /// Each distinct fallback with how often it was taken, most frequent first.
    pub fn counts(&self) -> Vec<(&'static str, u32)> {
        let mut counts = self.counts.clone();
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        counts
    }
    pub fn sample(&self) -> &[Fallback] {
        &self.sample
    }
    pub fn payload_bytes(&self) -> usize {
        self.counts.len() * size_of::<(&'static str, u32)>() + self.sample.len() * size_of::<Fallback>()
    }
}
impl std::fmt::Display for Fallbacks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, (what, n)) in self.counts().into_iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{what} ×{n}")?;
        }
        Ok(())
    }
}
/// Allocated only on failure, so adding diagnostic context does not enlarge
/// every successful layout/paint Result or worker state containing one.
#[derive(Debug)]
pub struct ForeignElement {
    pub node: dom::NodeId,
    pub namespace: dom::Namespace,
    pub name: Box<str>,
}
/// Failure-only context identifying the first unsupported layout property.
#[derive(Debug)]
pub struct UnsupportedProperty {
    pub node: dom::NodeId,
    pub name: Box<str>,
    pub property: style::Property,
    pub value: style::Value,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "flow layout {self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
macro_rules! error_from {
    ($ty:ty,$variant:ident) => {
        impl From<$ty> for Error {
            fn from(e: $ty) -> Self {
                Self::$variant(e)
            }
        }
    };
}
error_from!(dom::Error, Dom);
error_from!(style::Error, Style);
error_from!(css::properties::Error, Numeric);
error_from!(text::prepare::Error, Prepare);
error_from!(text::shaping::Error, Shaping);
error_from!(text::linebreak::Error, Break);
error_from!(text::layout::Error, Text);
fn check(c: Option<&AtomicBool>) -> Result<()> {
    if c.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Edges {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}
impl Edges {
    pub fn horizontal(self) -> f64 {
        self.left + self.right
    }
    pub fn vertical(self) -> f64 {
        self.top + self.bottom
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub width: f64,
    pub height: f64,
    pub scale: f64,
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub boxes: usize,
    pub depth: usize,
    pub work: usize,
    pub text_bytes: usize,
    pub cached_texts: usize,
    pub text: text::layout::Limits,
}
impl Default for Limits {
    fn default() -> Self {
        Self { boxes: 1_000_000, depth: 1024, work: 4_000_000, text_bytes: 16 * 1024 * 1024, cached_texts: 4096, text: Default::default() }
    }
}
