//! Native font data, CSS face matching and font-relative metrics. Worker-only;
//! no system process, browser/font engine, shaping, layout or rasterizer.
mod casefold;
mod catalog;
mod matching;
mod metrics;
mod scope;
pub mod system;
pub mod woff;
use crate::css::properties::{Family, GenericFamily};
use crate::css::style::{FontMetrics, FontRequest, FontSlant};
pub use catalog::{Builder, Catalog, Descriptor, Face, FaceId, LoadProgress};
pub use matching::{Coordinate, MatchOptions, Selection, Slope, Synthesis};
pub use metrics::{Fallbacks, ResolvedMetrics};
pub use scope::CacheStats;
pub use scope::{FontSet, GenericPolicy, Scope};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

#[derive(Debug)]
pub enum Error {
    Invalid,
    Unsupported(&'static str),
    Missing,
    Stale,
    Limit,
    Cancelled,
    Io(std::io::Error),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "font {self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub file_bytes: usize,
    pub retained_bytes: usize,
    pub faces: usize,
    pub collection_faces: usize,
    pub tables: usize,
    pub axes: usize,
    pub names: usize,
    pub name_bytes: usize,
    pub work: usize,
    pub cache_entries: usize,
    pub cache_bytes: usize,
    /// Simple-glyph plus whole gvar bytes allowed for optional metric inference;
    /// at most 4096 points. Composite/CFF outlines use the CSS estimate.
    pub outline_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            file_bytes: 64 * 1024 * 1024,
            retained_bytes: 256 * 1024 * 1024,
            faces: 16384,
            collection_faces: 256,
            tables: 512,
            axes: 63,
            names: 2048,
            name_bytes: 64 * 1024,
            work: 8_000_000,
            cache_entries: 4096,
            cache_bytes: 4 * 1024 * 1024,
            outline_bytes: 256 * 1024,
        }
    }
}
fn check(cancelled: Option<&AtomicBool>) -> Result<()> {
    if cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
fn identity() -> Result<u64> {
    NEXT_ID.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1)).map_err(|_| Error::Limit)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub min: f64,
    pub max: f64,
}
impl Span {
    pub fn point(value: f64) -> Self {
        Self { min: value, max: value }
    }
    pub fn new(min: f64, max: f64) -> Self {
        Self { min: min.min(max), max: max.max(min) }
    }
    fn contains(self, value: f64) -> bool {
        (self.min..=self.max).contains(&value)
    }
    fn clamp(self, value: f64) -> f64 {
        value.clamp(self.min, self.max)
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axis {
    pub tag: [u8; 4],
    pub range: Span,
    pub default: f64,
}
#[derive(Clone, Copy, Debug)]
pub struct Slopes {
    pub oblique: Option<Span>,
    pub italic: Option<Span>,
}
