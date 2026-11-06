//! Face unlock, natively: the infrared camera over V4L2 (`camera`), its
//! emitter lit through its UVC extension unit (`emitter`), faces found
//! and recognised by two small models run in process (`model`), and
//! whether it's a person there (`look`).

pub mod camera;
pub mod emitter;
pub mod look;
pub mod model;
