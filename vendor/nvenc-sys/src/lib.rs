//! NVIDIA's NVENC API (nvEncodeAPI.h, Video Codec SDK 12): its types, the
//! function table `NvEncodeAPICreateInstance` fills, the GUIDs and the
//! structures' versions. From nvidia-video-codec-sdk 0.4.0's bindgen
//! output (MIT, Viliam Vadocz; LICENSE), its `extern` block left out: the
//! library is opened at run time by olo-video (`nvenc`), never linked.
#![allow(warnings)]
#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case)]

#[rustfmt::skip]
pub mod nvEncodeAPI;
mod guid;
mod version;

pub use guid::*;
pub use nvEncodeAPI::*;
pub use version::*;
