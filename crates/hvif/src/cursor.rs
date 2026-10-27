//! Haiku's system cursors (from app_server's CursorData, MIT licensed),
//! rendered as Haiku's CursorManager does: the HVIF drawn so that 22
//! logical pixels hold its top-left 32 × 32 region, with a soft shadow
//! one device pixel down and right.

use tiny_skia::Pixmap;

use crate::Icon;

/// The cursors Haiku's app_server ships, by BCursorID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
    SystemDefault,
    ContextMenu,
    Copy,
    CreateLink,
    CrossHair,
    FollowLink,
    Grab,
    Grabbing,
    Help,
    IBeam,
    IBeamHorizontal,
    Move,
    NotAllowed,
    Progress,
    ResizeEast,
    ResizeEastWest,
    ResizeNorth,
    ResizeNorthEast,
    ResizeNorthEastSouthWest,
    ResizeNorthSouth,
    ResizeNorthWest,
    ResizeNorthWestSouthEast,
    ResizeSouth,
    ResizeSouthEast,
    ResizeSouthWest,
    ResizeWest,
    ZoomIn,
    ZoomOut,
}

impl Shape {
    pub const ALL: [Shape; 28] = [
        Shape::SystemDefault,
        Shape::ContextMenu,
        Shape::Copy,
        Shape::CreateLink,
        Shape::CrossHair,
        Shape::FollowLink,
        Shape::Grab,
        Shape::Grabbing,
        Shape::Help,
        Shape::IBeam,
        Shape::IBeamHorizontal,
        Shape::Move,
        Shape::NotAllowed,
        Shape::Progress,
        Shape::ResizeEast,
        Shape::ResizeEastWest,
        Shape::ResizeNorth,
        Shape::ResizeNorthEast,
        Shape::ResizeNorthEastSouthWest,
        Shape::ResizeNorthSouth,
        Shape::ResizeNorthWest,
        Shape::ResizeNorthWestSouthEast,
        Shape::ResizeSouth,
        Shape::ResizeSouthEast,
        Shape::ResizeSouthWest,
        Shape::ResizeWest,
        Shape::ZoomIn,
        Shape::ZoomOut,
    ];

    fn data(self) -> &'static [u8] {
        macro_rules! hvif {
            ($name:literal) => {
                include_bytes!(concat!("../cursors/", $name, ".hvif"))
            };
        }
        match self {
            Shape::SystemDefault => hvif!("SystemDefault"),
            Shape::ContextMenu => hvif!("ContextMenu"),
            Shape::Copy => hvif!("Copy"),
            Shape::CreateLink => hvif!("CreateLink"),
            Shape::CrossHair => hvif!("CrossHair"),
            Shape::FollowLink => hvif!("FollowLink"),
            Shape::Grab => hvif!("Grab"),
            Shape::Grabbing => hvif!("Grabbing"),
            Shape::Help => hvif!("Help"),
            Shape::IBeam => hvif!("IBeam"),
            Shape::IBeamHorizontal => hvif!("IBeamHorizontal"),
            Shape::Move => hvif!("Move"),
            Shape::NotAllowed => hvif!("NotAllowed"),
            Shape::Progress => hvif!("Progress"),
            Shape::ResizeEast => hvif!("ResizeEast"),
            Shape::ResizeEastWest => hvif!("ResizeEastWest"),
            Shape::ResizeNorth => hvif!("ResizeNorth"),
            Shape::ResizeNorthEast => hvif!("ResizeNorthEast"),
            Shape::ResizeNorthEastSouthWest => hvif!("ResizeNorthEastSouthWest"),
            Shape::ResizeNorthSouth => hvif!("ResizeNorthSouth"),
            Shape::ResizeNorthWest => hvif!("ResizeNorthWest"),
            Shape::ResizeNorthWestSouthEast => hvif!("ResizeNorthWestSouthEast"),
            Shape::ResizeSouth => hvif!("ResizeSouth"),
            Shape::ResizeSouthEast => hvif!("ResizeSouthEast"),
            Shape::ResizeSouthWest => hvif!("ResizeSouthWest"),
            // Haiku's ResizeWest data is a copy of ResizeEast; render()
            // mirrors it.
            Shape::ResizeWest => hvif!("ResizeEast"),
            Shape::ZoomIn => hvif!("ZoomIn"),
            Shape::ZoomOut => hvif!("ZoomOut"),
        }
    }

    /// The hotspot in 22 × 22 logical pixels.
    pub fn hotspot(self) -> (u32, u32) {
        match self {
            Shape::SystemDefault | Shape::ContextMenu | Shape::Copy | Shape::CreateLink | Shape::Grab | Shape::Grabbing => (1, 1),
            Shape::CrossHair => (10, 10),
            Shape::FollowLink => (5, 0),
            Shape::Help => (0, 8),
            Shape::IBeam => (7, 9),
            Shape::Progress => (7, 10),
            Shape::ZoomIn | Shape::ZoomOut => (6, 6),
            _ => (8, 8),
        }
    }
}

/// A rendered cursor: `size` × `size` premultiplied RGBA pixels.
#[derive(Debug, Clone)]
pub struct Cursor {
    pub size: u32,
    /// In device pixels.
    pub hotspot: (u32, u32),
    pub rgba: Vec<u8>,
}

/// The logical size of every cursor.
pub const SIZE: u32 = 22;

static FALLBACK: &[u8; 22 * 22 * 4] = include_bytes!("../cursors/FallbackBits.rgba");

/// Draw a cursor for a screen of the given scale (1.0, 1.5, 2.0, …).
pub fn render(shape: Shape, scale: f64) -> Cursor {
    let scale = scale.max(1.0);
    let size = (SIZE as f64 * scale) as u32;
    let (hx, hy) = shape.hotspot();
    let hotspot = ((hx as f64 * scale) as u32, (hy as f64 * scale) as u32);
    let Ok(mut icon) = Icon::parse(shape.data()) else {
        let rgba = FALLBACK.chunks(4).flat_map(|p| {
            let a = p[3] as u32;
            [p[0] as u32 * a / 255, p[1] as u32 * a / 255, p[2] as u32 * a / 255, a].map(|c| c as u8)
        });
        return Cursor { size: SIZE, hotspot: (hx, hy), rgba: rgba.collect() };
    };
    if shape == Shape::ResizeWest {
        // The east arrow spans x 4..36: turn it about its middle.
        icon.transform(tiny_skia::Transform::from_row(-1.0, 0.0, 0.0, 1.0, 40.0, 0.0));
    }
    // Haiku renders into a size × 32/22 square and keeps the top left.
    let render = (size as f32 * 32.0 / 22.0).floor();
    let mut pixmap = Pixmap::new(size, size).expect("non-zero size");
    icon.render_into(&mut pixmap, render / 64.0);
    let cursor = pixmap.data();

    // The shadow: the cursor's alpha, offset, blurred, at 30 %.
    let offset = (size / 32).max(1) as usize;
    let n = size as usize;
    let mut shadow = vec![0f32; n * n];
    for y in offset..n {
        for x in offset..n {
            shadow[y * n + x] = cursor[((y - offset) * n + (x - offset)) * 4 + 3] as f32;
        }
    }
    let shadow = blur(&shadow, n);

    let mut rgba = Vec::with_capacity(n * n * 4);
    for (i, c) in cursor.chunks(4).enumerate() {
        let ca = c[3] as f32 / 255.0;
        let sa = shadow[i] / 255.0 * 0.3;
        let a = ca + (1.0 - ca) * sa;
        rgba.extend_from_slice(&[c[0], c[1], c[2], (a * 255.0).round() as u8]);
    }
    Cursor { size, hotspot, rgba }
}

/// A [1 2 1] blur across and then down, standing in for agg's
/// recursive blur of radius 1.
fn blur(v: &[f32], n: usize) -> Vec<f32> {
    let at = |v: &[f32], x: isize, y: isize| if (0..n as isize).contains(&x) && (0..n as isize).contains(&y) { v[y as usize * n + x as usize] } else { 0.0 };
    let pass = |v: &[f32], dx: isize, dy: isize| -> Vec<f32> {
        (0..n * n)
            .map(|i| {
                let (x, y) = ((i % n) as isize, (i / n) as isize);
                (at(v, x - dx, y - dy) + 2.0 * at(v, x, y) + at(v, x + dx, y + dy)) / 4.0
            })
            .collect()
    };
    pass(&pass(v, 1, 0), 0, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cursor_parses_and_draws() {
        for shape in Shape::ALL {
            let icon = Icon::parse(shape.data()).unwrap_or_else(|e| panic!("{shape:?}: {e}"));
            assert!(!icon.shapes.is_empty(), "{shape:?}");
            for scale in [1.0, 1.5, 2.0] {
                let c = render(shape, scale);
                assert_eq!(c.rgba.len(), (c.size * c.size * 4) as usize);
                assert!(c.rgba.chunks(4).any(|p| p[3] == 255), "{shape:?} at {scale} has no solid pixels");
                // Premultiplied.
                assert!(c.rgba.chunks(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]));
            }
        }
        assert_eq!(render(Shape::IBeam, 2.0).hotspot, (14, 18));
    }

    /// `OLO_CURSOR_SHEET=out.png cargo test -p olo-hvif sheet` draws
    /// every cursor at 1×, 1.5× and 2× on light and dark grounds.
    #[test]
    fn sheet() {
        let Some(out) = std::env::var_os("OLO_CURSOR_SHEET") else { return };
        let cell = 56u32;
        let (cols, rows) = (Shape::ALL.len() as u32, 6u32);
        let mut sheet = Pixmap::new(cols * cell, rows * cell).unwrap();
        for row in 0..rows {
            let ground = if row % 2 == 0 { tiny_skia::Color::from_rgba8(216, 216, 216, 255) } else { tiny_skia::Color::from_rgba8(40, 40, 48, 255) };
            sheet.fill_rect(tiny_skia::Rect::from_xywh(0.0, (row * cell) as f32, (cols * cell) as f32, cell as f32).unwrap(), &tiny_skia::Paint { shader: tiny_skia::Shader::SolidColor(ground), ..Default::default() }, tiny_skia::Transform::identity(), None);
        }
        for (col, shape) in Shape::ALL.into_iter().enumerate() {
            for (row, scale) in [1.0, 1.0, 1.5, 1.5, 2.0, 2.0].into_iter().enumerate() {
                let c = render(shape, scale);
                let pix = tiny_skia::Pixmap::from_vec(c.rgba, tiny_skia::IntSize::from_wh(c.size, c.size).unwrap()).unwrap();
                sheet.draw_pixmap((col as u32 * cell + 4) as i32, (row as u32 * cell + 4) as i32, pix.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
            }
        }
        sheet.save_png(out).unwrap();
    }
}
