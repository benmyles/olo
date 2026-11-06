//! libva's declarations (Intel's VA-API, libva 2.24: va.h, va_drm.h,
//! va_drmcommon.h and va_enc_av1.h; MIT, LICENSE): the types and
//! constants olo-video's AV1 encoder (`vaapi`) uses, from bindgen (types
//! only, see README.md). No functions: the library is opened at run time,
//! never linked, so Olo builds and starts where there is no libva.
#![allow(warnings)]
#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case)]

#[rustfmt::skip]
mod va;

pub use va::*;
