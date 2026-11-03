//! Olo's pointer for other programs: the cursors of the "Olo" theme (in
//! `~/.local/share/icons/Olo/cursors`, `icon_theme.rs`), made from the Haiku cursors
//! app_server draws (`cursor.rs`), for the programs that draw their own
//! pointer rather than asking for one by name over cursor-shape-v1 (GTK 3,
//! SDL, Java, older toolkits). Each CSS cursor name has a file with the
//! cursor at every scale Olo draws it, the old X names link to them, and
//! `toolkits.rs` names the theme in GSettings (`cursor-theme`) and
//! olo-session in `XCURSOR_THEME`.

use std::{io, path::Path, str::FromStr};

use olo_hvif::cursor::Shape;
use smithay::input::pointer::CursorIcon;

/// Its size at 1× (the cursors' logical size): `cursor-size` and
/// `XCURSOR_SIZE`.
pub const SIZE: u32 = olo_hvif::cursor::SIZE;
const _: () = assert!(SIZE == olo_xdg::CURSOR_SIZE, "olo_xdg::CURSOR_SIZE is the cursors' size");

/// Raised whenever the cursors or this format change, so they are
/// written again.
pub const VERSION: u32 = 1;

/// The scales each cursor is drawn at (a program picks the size nearest
/// its cursor size times its scale).
const SCALES: [f64; 8] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0];

/// Every CSS cursor name (cursor-shape-v1's, and the freedesktop cursor
/// spec's).
const NAMES: [&str; 36] = [
    "default",
    "context-menu",
    "help",
    "pointer",
    "progress",
    "wait",
    "cell",
    "crosshair",
    "text",
    "vertical-text",
    "alias",
    "copy",
    "move",
    "no-drop",
    "not-allowed",
    "grab",
    "grabbing",
    "e-resize",
    "n-resize",
    "ne-resize",
    "nw-resize",
    "s-resize",
    "se-resize",
    "sw-resize",
    "w-resize",
    "ew-resize",
    "ns-resize",
    "nesw-resize",
    "nwse-resize",
    "col-resize",
    "row-resize",
    "all-scroll",
    "zoom-in",
    "zoom-out",
    "dnd-ask",
    "all-resize",
];

/// The Xcursor file for `shape`: the image at each of `SCALES`, its
/// nominal size its size in pixels.
pub fn file(shape: Shape) -> Vec<u8> {
    const IMAGE: u32 = 0xfffd_0002;
    let images: Vec<olo_hvif::cursor::Cursor> = {
        let mut v: Vec<_> = SCALES.iter().map(|&s| olo_hvif::cursor::render(shape, s)).collect();
        // (Two scales rounding to one size need it once.)
        v.dedup_by_key(|c| c.size);
        v
    };
    let mut out = Vec::new();
    let u32le = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
    out.extend_from_slice(b"Xcur");
    u32le(&mut out, 16);
    u32le(&mut out, 0x1_0000);
    u32le(&mut out, images.len() as u32);
    // The table of contents, then each image's chunk.
    let mut at = 16 + 12 * images.len() as u32;
    for c in &images {
        u32le(&mut out, IMAGE);
        u32le(&mut out, c.size);
        u32le(&mut out, at);
        at += 36 + c.size * c.size * 4;
    }
    for c in &images {
        for v in [36, IMAGE, c.size, 1, c.size, c.size, c.hotspot.0, c.hotspot.1, 0] {
            u32le(&mut out, v);
        }
        // Premultiplied ARGB, as the cursor is drawn (premultiplied RGBA).
        for p in c.rgba.chunks_exact(4) {
            u32le(&mut out, (p[3] as u32) << 24 | (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32);
        }
    }
    out
}

/// Write the cursors into `cursors` (the theme's folder of them,
/// `icon_theme.rs`).
pub fn write_cursors(cursors: &Path) -> io::Result<()> {
    std::fs::create_dir_all(cursors)?;
    let mut written: Vec<&str> = Vec::new();
    for name in NAMES {
        let Ok(icon) = CursorIcon::from_str(name) else { continue };
        put(&cursors.join(name), &file(crate::cursor::shape_for(icon)))?;
        written.push(name);
    }
    // The old names, each to the first CSS name that has it.
    let mut linked: Vec<&str> = Vec::new();
    for name in NAMES {
        let Ok(icon) = CursorIcon::from_str(name) else { continue };
        for alt in icon.alt_names() {
            if written.contains(alt) || linked.contains(alt) {
                continue;
            }
            let link = cursors.join(alt);
            if std::fs::read_link(&link).ok().as_deref() != Some(Path::new(name)) {
                let _ = std::fs::remove_file(&link);
                std::os::unix::fs::symlink(name, &link)?;
            }
            linked.push(alt);
        }
    }
    Ok(())
}

fn put(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if std::fs::read(path).is_ok_and(|b| b == bytes) {
        return Ok(());
    }
    let tmp = path.with_extension("new");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_xcursor() {
        let f = file(Shape::SystemDefault);
        assert_eq!(&f[..4], b"Xcur");
        let n = u32::from_le_bytes(f[12..16].try_into().unwrap()) as usize;
        assert!(n >= 6, "{n} sizes");
        // The first image: 22 px at 1×, its hot spot, then its pixels.
        let at = u32::from_le_bytes(f[24..28].try_into().unwrap()) as usize;
        let word = |i: usize| u32::from_le_bytes(f[at + i * 4..at + i * 4 + 4].try_into().unwrap());
        assert_eq!((word(0), word(1), word(2), word(4), word(5)), (36, 0xfffd_0002, SIZE, SIZE, SIZE));
        assert_eq!((word(6), word(7)), Shape::SystemDefault.hotspot());
        let last = u32::from_le_bytes(f[16 + 12 * (n - 1) + 8..16 + 12 * (n - 1) + 12].try_into().unwrap()) as usize;
        let size = u32::from_le_bytes(f[last + 8..last + 12].try_into().unwrap()) as usize;
        assert_eq!(f.len(), last + 36 + size * size * 4);
    }

    #[test]
    fn writes_the_cursors() {
        let dir = std::env::temp_dir().join(format!("olo-xcursor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_cursors(&dir).unwrap();
        assert!(dir.join("default").is_file() && dir.join("text").is_file());
        assert_eq!(std::fs::read_link(dir.join("left_ptr")).unwrap(), Path::new("default"));
        assert_eq!(std::fs::read_link(dir.join("xterm")).unwrap(), Path::new("text"));
        // "move" is a CSS name itself, not all-resize's old one.
        assert!(dir.join("move").symlink_metadata().unwrap().is_file());
        let before = std::fs::metadata(dir.join("default")).unwrap().modified().unwrap();
        write_cursors(&dir).unwrap();
        assert_eq!(std::fs::metadata(dir.join("default")).unwrap().modified().unwrap(), before);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
