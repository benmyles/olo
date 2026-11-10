//! Olo's Interface Kit.
#![allow(clippy::type_complexity)]
#![allow(clippy::collapsible_else_if)]
#![allow(missing_docs)]
#![allow(unused_mut)] // (False positives in the Wayland client.)

// (The style macros name their types by this crate's old name.)
extern crate self as gpui;

pub mod agent;
mod app;
mod asset_cache;
mod assets;
mod bounds_tree;
mod color;
mod dispatch;
mod display;
mod elastic;
mod element;
mod executor;
mod geometry;
mod interactive;
mod layout;
mod outline;
mod painter;
mod path_builder;
mod platform;
mod scene;
mod shared_string;
mod shared_uri;
mod stage;
mod stats;
mod style;
mod styled;
mod svg_renderer;
mod text_system;
mod tree;
mod util;
mod view;
mod window;
mod yuv;

pub use anyhow::Result;
pub use app::*;
pub use asset_cache::*;
pub use assets::*;
pub use color::*;
pub use elastic::*;
pub use element::{AnyElement, ControlState, Element, ElementId, InteractiveElement, IntoElement, ParentElement, canvas, control_canvas, div, styled_text};
pub(crate) use executor::*;
pub use geometry::*;
pub use interactive::*;
pub use layout::AvailableSpace;
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
pub use tree::{DISABLED_OPACITY, Measuring};
pub use util::{FluentBuilder, arc_cow::ArcCow};
pub use view::{AnyChild, Child, Cx, View, ViewRef};
pub use window::{LocalWindow, WindowCx, thread_text_system, with_window_on};
pub use yuv::{YuvFrame, YuvMatrix, YuvMemory};

pub use olo_app as app_kit;
pub use olo_app::{Looper, LooperHandle, Messenger, Task, TimerId, pool};
