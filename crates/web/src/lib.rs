//! Olo's web engine. This is an incremental implementation, not yet a
//! conforming browser. See docs/browser for measured compatibility.
#![forbid(unsafe_code)]

pub(crate) mod chunked;
/// Whether an environment variable is set (a debugging switch such as
/// `OLO_WEB_TIMING`), read once: `std::env::var_os` takes the environment's
/// lock and scans it, and these are asked per frame and per element.
#[macro_export]
macro_rules! env_on {
    ($name:literal) => {{
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var_os($name).is_some())
    }};
}
pub mod css;
pub mod dom;
pub mod font;
pub mod html;
pub mod image;
pub mod layout;
pub mod page;
pub mod paint;
pub mod srcset;
pub mod svg;
pub mod svg_image;

pub mod text;
