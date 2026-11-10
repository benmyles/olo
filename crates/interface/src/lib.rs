//! Olo's Interface Kit.
#![allow(clippy::type_complexity)]
#![allow(clippy::collapsible_else_if)]
#![allow(missing_docs)]

// (The style macros name their types by this crate's old name.)
extern crate self as gpui;

pub mod agent;
mod asset_cache;
mod assets;
mod bounds_tree;
mod color;
mod elastic;
mod executor;
mod geometry;
mod interactive;
mod outline;
mod painter;
mod path_builder;
mod platform;
mod scene;
mod shared_string;
mod shared_uri;
mod stats;
mod style;
mod styled;
mod svg_renderer;
mod text_system;
mod util;
mod yuv;

pub use anyhow::Result;
pub use asset_cache::*;
pub use assets::*;
pub use color::*;
pub use elastic::*;
pub(crate) use executor::*;
pub use geometry::*;
pub use interactive::*;
pub use painter::*;
pub use path_builder::*;
pub use platform::*;
pub use refineable::*;
pub use scene::*;
pub use shared_string::*;
pub use shared_uri::*;
pub use stats::FrameStats;
pub use style::*;
pub use styled::*;
pub(crate) use svg_renderer::*;
pub use text_system::*;
pub use util::{FluentBuilder, arc_cow::ArcCow};
pub use yuv::{YuvFrame, YuvMatrix, YuvMemory};
