//! Pictures in documents (`doc::Picture`, a style of their character):
//! each file decoded on a worker (`olo_images`: what the image crate
//! reads, and JPEG XL, AVIF and HEIC), made no bigger than 2048 pixels a
//! side and kept as a texture; one still loading shows as a frame. The
//! bytes a picture goes into other formats as (its file's own when PNG or
//! JPEG, else made a PNG); its natural size; and where pictures with no
//! file of their own (pasted, or from an imported document) are kept
//! until their document is saved, then beside it.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use olo_kit::{Proxy, egui};

/// A picture decoded on a worker (sent to the app).
pub struct Loaded(pub PathBuf, pub Result<egui::ColorImage, String>);

enum Slot {
    Loading,
    Ready(egui::TextureHandle),
    Failed,
}

/// The pictures' textures, by file.
#[derive(Default)]
pub struct Pictures {
    slots: HashMap<PathBuf, Slot>,
    proxy: Option<Proxy>,
}

/// The longest side a picture's texture has.
const MOST: u32 = 2048;

impl Pictures {
    /// Where decoded pictures are sent (`Loaded`, to the app).
    pub fn set_proxy(&mut self, proxy: Proxy) {
        self.proxy = Some(proxy);
    }

    /// `path`'s texture once it's loaded (its loading started the first
    /// time it's asked for).
    pub fn texture(&mut self, path: &Path) -> Option<egui::TextureId> {
        match self.slots.get(path) {
            Some(Slot::Ready(t)) => Some(t.id()),
            Some(_) => None,
            None => {
                self.slots.insert(path.to_owned(), Slot::Loading);
                if let Some(proxy) = self.proxy.clone() {
                    let path = path.to_owned();
                    std::thread::spawn(move || {
                        let r = decode(&path);
                        proxy.send(Loaded(path, r));
                    });
                }
                None
            }
        }
    }

    /// A worker's picture, as a texture.
    pub fn loaded(&mut self, ctx: &egui::Context, l: Loaded) {
        let slot = match l.1 {
            Ok(image) => Slot::Ready(ctx.load_texture(l.0.to_string_lossy(), image, egui::TextureOptions::LINEAR)),
            Err(_) => Slot::Failed,
        };
        self.slots.insert(l.0, slot);
    }
}

fn decode(path: &Path) -> Result<egui::ColorImage, String> {
    let (img, _) = olo_images::open(path)?;
    let img = if img.width().max(img.height()) > MOST { img.thumbnail(MOST, MOST) } else { img };
    let rgba = img.to_rgba8();
    Ok(egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

/// The points a picture `px` pixels big takes: its pixels at 96 to the
/// inch (as Word and web pages take them), no wider than `room` points
/// nor taller than `tall` (its shape kept).
pub fn fit(px: (u32, u32), room: f32, tall: f32) -> (f32, f32) {
    let (w, h) = (px.0.max(1) as f32 * 0.75, px.1.max(1) as f32 * 0.75);
    let k = (room / w).min(tall / h).min(1.0);
    ((w * k).round().max(1.0), (h * k).round().max(1.0))
}

/// A picture file's size in pixels (from its header when the image crate
/// knows the format, else decoded). On a worker.
pub fn pixels(path: &Path) -> Option<(u32, u32)> {
    image::image_dimensions(path).ok().or_else(|| olo_images::open(path).ok().map(|(i, _)| (i.width(), i.height())))
}

/// The bytes `path` goes into a Word, OpenDocument or RTF file or a web
/// page as: its own if it's a PNG or JPEG (or, `any`, a GIF or WebP too),
/// else made a PNG.
pub fn export_bytes(path: &Path, any: bool) -> Option<Vec<u8>> {
    let data = std::fs::read(path).ok()?;
    match olo_richtext::kind_of_image(&data) {
        Some(("png" | "jpeg", _)) => return Some(data),
        Some(("gif" | "webp", _)) if any => return Some(data),
        _ => {}
    }
    let (img, _) = olo_images::open(path).ok()?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Where pictures with no file of their own wait for their document to
/// be saved: Olo's cache, by their bytes' hash.
pub fn holding() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("olo/styled-edit/pictures")
}

/// Keep picture bytes `data` as a file (in `holding`): its path.
pub fn hold(data: &[u8]) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let ext = olo_richtext::kind_of_image(data).map_or("png", |k| k.0);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut h);
    let dir = holding();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{:016x}.{ext}", h.finish()));
    if !path.exists() {
        let tmp = path.with_extension("saving");
        std::fs::write(&tmp, data).ok()?;
        std::fs::rename(&tmp, &path).ok()?;
    }
    Some(path)
}

/// The folder a saved document's held pictures go in, beside it:
/// "Report pictures" for "Report.txt".
pub fn folder_for(doc: &Path) -> PathBuf {
    let stem = doc.file_stem().map_or_else(|| "Document".into(), |s| s.to_string_lossy().into_owned());
    doc.with_file_name(format!("{stem} pictures"))
}

/// `runs` as saved with the document at `doc`: each picture's path from
/// the document's folder when it's in it (absolute, else); one with no
/// file of its own (`holding`) copied into the folder beside it
/// (`folder_for`) first. On a worker.
pub fn settle(runs: &[crate::doc::Run], doc: &Path) -> Vec<crate::doc::Run> {
    let dir = doc.parent();
    let held = holding();
    let mut runs = runs.to_vec();
    for r in &mut runs {
        let Some(pic) = &mut r.style.picture else { continue };
        let mut abs = PathBuf::from(&pic.path);
        if abs.starts_with(&held)
            && let Some(name) = abs.file_name()
        {
            let to = folder_for(doc).join(name);
            if to.exists() || std::fs::create_dir_all(folder_for(doc)).and_then(|_| std::fs::copy(&abs, &to)).is_ok() {
                abs = to;
            }
        }
        pic.path = match dir.and_then(|d| abs.strip_prefix(d).ok()) {
            Some(rel) => rel.to_string_lossy().into_owned(),
            None => abs.to_string_lossy().into_owned(),
        };
    }
    runs
}

/// `runs` as read from the document at `doc`: each picture's path made
/// absolute.
pub fn absolute(runs: &mut [crate::doc::Run], doc: &Path) {
    for r in runs {
        if let Some(pic) = &mut r.style.picture {
            pic.path = pic.resolve(doc.parent()).to_string_lossy().into_owned();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitting() {
        // 96 to the inch, and kept within the room, its shape kept.
        assert_eq!(fit((96, 48), 468.0, 600.0), (72.0, 36.0));
        assert_eq!(fit((1248, 624), 468.0, 600.0), (468.0, 234.0));
        assert_eq!(fit((100, 2000), 468.0, 600.0), (30.0, 600.0));
    }

    #[test]
    fn saved_paths_are_relative_and_held_ones_copied() {
        use crate::doc::{Picture, Run, Style};
        let dir = std::env::temp_dir().join(format!("se-pictures-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let near = dir.join("a.png");
        std::fs::write(&near, b"\x89PNG").unwrap();
        let held = hold(b"\x89PNG held picture").unwrap();
        let pic = |p: &Path| Style { picture: Some(Picture { path: p.to_string_lossy().into_owned(), width: 1.0, height: 1.0, alt: String::new() }), ..Style::default() };
        let runs = vec![Run { len: 3, style: pic(&near) }, Run { len: 3, style: pic(&held) }, Run { len: 3, style: pic(Path::new("/elsewhere/b.png")) }];
        let doc = dir.join("Report.txt");
        let mut saved = settle(&runs, &doc);
        let paths: Vec<String> = saved.iter().map(|r| r.style.picture.as_ref().unwrap().path.clone()).collect();
        let name = held.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(paths, ["a.png".to_owned(), format!("Report pictures/{name}"), "/elsewhere/b.png".to_owned()]);
        assert!(dir.join("Report pictures").join(&name).exists());
        absolute(&mut saved, &doc);
        assert_eq!(saved[0].style.picture.as_ref().unwrap().path, near.to_string_lossy());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folders_beside_documents() {
        assert_eq!(folder_for(Path::new("/x/Report.txt")), Path::new("/x/Report pictures"));
    }
}
