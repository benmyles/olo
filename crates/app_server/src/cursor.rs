//! The pointer: Haiku's system cursors (`tabula-hvif`), drawn at each
//! output's scale and kept once drawn. Named cursors, from our own
//! decorations or from clients over cursor-shape-v1, map onto them.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    input::pointer::{CursorIcon, CursorImageAttributes, CursorImageStatus},
    utils::{Logical, Point, Transform},
    wayland::compositor,
};
pub use tabula_hvif::cursor::{Cursor, SIZE, Shape};

/// The Haiku cursor for a CSS / cursor-shape-v1 name.
pub fn shape_for(icon: CursorIcon) -> Shape {
    match icon {
        CursorIcon::ContextMenu => Shape::ContextMenu,
        CursorIcon::Help => Shape::Help,
        CursorIcon::Pointer => Shape::FollowLink,
        CursorIcon::Progress | CursorIcon::Wait => Shape::Progress,
        CursorIcon::Cell | CursorIcon::Crosshair => Shape::CrossHair,
        CursorIcon::Text => Shape::IBeam,
        CursorIcon::VerticalText => Shape::IBeamHorizontal,
        CursorIcon::Alias => Shape::CreateLink,
        CursorIcon::Copy => Shape::Copy,
        CursorIcon::Move | CursorIcon::AllScroll => Shape::Move,
        CursorIcon::NoDrop | CursorIcon::NotAllowed => Shape::NotAllowed,
        CursorIcon::Grab => Shape::Grab,
        CursorIcon::Grabbing => Shape::Grabbing,
        CursorIcon::EResize => Shape::ResizeEast,
        CursorIcon::NResize => Shape::ResizeNorth,
        CursorIcon::NeResize => Shape::ResizeNorthEast,
        CursorIcon::NwResize => Shape::ResizeNorthWest,
        CursorIcon::SResize => Shape::ResizeSouth,
        CursorIcon::SeResize => Shape::ResizeSouthEast,
        CursorIcon::SwResize => Shape::ResizeSouthWest,
        CursorIcon::WResize => Shape::ResizeWest,
        CursorIcon::EwResize | CursorIcon::ColResize => Shape::ResizeEastWest,
        CursorIcon::NsResize | CursorIcon::RowResize => Shape::ResizeNorthSouth,
        CursorIcon::NeswResize => Shape::ResizeNorthEastSouthWest,
        CursorIcon::NwseResize => Shape::ResizeNorthWestSouthEast,
        CursorIcon::ZoomIn => Shape::ZoomIn,
        CursorIcon::ZoomOut => Shape::ZoomOut,
        _ => Shape::SystemDefault,
    }
}

/// The built-in cursor `status` asks for, if any (none when hidden or
/// when the client draws its own).
pub fn named(status: &CursorImageStatus) -> Option<Shape> {
    match status {
        CursorImageStatus::Named(icon) => Some(shape_for(*icon)),
        _ => None,
    }
}

/// What to draw for the pointer in a screenshot: the built-in cursor,
/// standing in for one a client draws itself; nothing when hidden.
pub fn pictured(status: &CursorImageStatus) -> Option<Shape> {
    match status {
        CursorImageStatus::Hidden => None,
        _ => Some(named(status).unwrap_or(Shape::SystemDefault)),
    }
}

/// Where the pointer's hot pixel is in its image, for either kind.
pub fn hotspot(status: &CursorImageStatus) -> Point<i32, Logical> {
    match status {
        CursorImageStatus::Surface(surface) => compositor::with_states(surface, |states| {
            states.data_map.get::<std::sync::Mutex<CursorImageAttributes>>().map(|a| a.lock().unwrap_or_else(|e| e.into_inner()).hotspot).unwrap_or_default()
        }),
        CursorImageStatus::Named(icon) => {
            let (x, y) = shape_for(*icon).hotspot();
            (x as i32, y as i32).into()
        }
        CursorImageStatus::Hidden => (0, 0).into(),
    }
}

/// `shape` drawn for a screen of `scale`; drawn once, then shared.
pub fn image(shape: Shape, scale: f64) -> Arc<Cursor> {
    type Drawn = HashMap<(Shape, u64), Arc<Cursor>>;
    static CACHE: OnceLock<Mutex<Drawn>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    cache.entry((shape, scale.to_bits())).or_insert_with(|| Arc::new(tabula_hvif::cursor::render(shape, scale))).clone()
}

thread_local! {
    static BUFFERS: std::cell::RefCell<HashMap<(Shape, u64), MemoryRenderBuffer>> = Default::default();
}

/// `shape` at `scale` as a buffer for the renderer (premultiplied, its
/// texture kept once uploaded).
pub fn buffer(shape: Shape, scale: f64) -> MemoryRenderBuffer {
    BUFFERS.with_borrow_mut(|b| {
        b.entry((shape, scale.to_bits()))
            .or_insert_with(|| {
                let c = image(shape, scale);
                MemoryRenderBuffer::from_slice(&c.rgba, Fourcc::Abgr8888, (c.size as i32, c.size as i32), 1, Transform::Normal, None)
            })
            .clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_map_to_haiku_cursors() {
        assert_eq!(shape_for(CursorIcon::Default), Shape::SystemDefault);
        assert_eq!(shape_for(CursorIcon::Text), Shape::IBeam);
        assert_eq!(shape_for(CursorIcon::WResize), Shape::ResizeWest);
        let a = image(Shape::IBeam, 2.0);
        assert!(Arc::ptr_eq(&a, &image(Shape::IBeam, 2.0)));
        assert_eq!(a.size, 44);
    }
}
