mod dispatcher;
mod headless;
mod keyboard;
mod platform;
#[cfg(feature = "wayland")]
mod text_system;
#[cfg(feature = "wayland")]
mod wayland;

#[cfg(feature = "wayland")]
mod xdg_desktop_portal;

pub(crate) use dispatcher::*;
pub(crate) use headless::*;
pub(crate) use keyboard::*;
pub(crate) use platform::*;
#[cfg(feature = "wayland")]
pub(crate) use text_system::*;
#[cfg(feature = "wayland")]
pub(crate) use wayland::*;

#[cfg(feature = "screen-capture")]
pub(crate) type PlatformScreenCaptureFrame = scap::frame::Frame;
#[cfg(not(feature = "screen-capture"))]
pub(crate) type PlatformScreenCaptureFrame = ();
