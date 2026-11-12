//! Pictures in documents (`doc::Picture`, a style of their character):
//! each file decoded on a worker (`olo_images`: what the image crate
//! reads, and JPEG XL, AVIF and HEIC), made no bigger than 2048 pixels a
//! side and kept as a `RenderImage`; one still loading shows as a frame. The
//! bytes a picture goes into other formats as (its file's own when PNG or
//! JPEG, else made a PNG); its natural size; and where pictures with no
//! file of their own (pasted, or from an imported document) are kept
//! until their document is saved, then beside it. A picture on the web
//! (a Markdown document's `![](https://…)`) is fetched once, on the
//! worker that decodes it, into the cache (`web`), and read from there.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use olo_kit::RenderImage;

/// A picture decoded on a worker (sent to the app).
pub struct Loaded(pub PathBuf, pub Result<Arc<RenderImage>, String>);

enum Slot {
    Loading,
    Ready(Arc<RenderImage>),
    Failed,
}

/// The pictures, by file.
#[derive(Default)]
pub struct Pictures {
    slots: HashMap<PathBuf, Slot>,
    /// Where decoded pictures are sent (the document's window).
    send: Option<Arc<dyn Fn(Loaded) + Send + Sync>>,
    /// Pictures on the web: the file each is kept in, and its address.
    web: HashMap<PathBuf, String>,
}

/// The largest picture fetched from the web.
const MOST_FETCHED: u64 = 40 << 20;
/// Pictures fetched are kept this long after they were last fetched.
const KEEP_FETCHED: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 3600);

/// Where pictures fetched from the web are kept.
fn fetched_dir() -> PathBuf {
    olo_xdg::cache_dir().join("styled-edit").join("pictures")
}

/// The longest side a picture's texture has.
const MOST: u32 = 2048;

impl Pictures {
    /// Where decoded pictures are sent (`Loaded`, to the document's
    /// window).
    pub fn set_sender(&mut self, send: impl Fn(Loaded) + Send + Sync + 'static) {
        self.send = Some(Arc::new(send));
    }

    /// The file a picture at `url` (http or https) is kept in once
    /// fetched: asked for as any other ([`Pictures::image`]), it's
    /// fetched first.
    pub fn web(&mut self, url: &str) -> PathBuf {
        use sha2::Digest as _;
        let digest = sha2::Sha256::digest(url.as_bytes());
        let name: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        // (Its kind as its address says it, for the decoder.)
        let ext = url
            .split(['?', '#'])
            .next()
            .and_then(|p| p.rsplit('/').next())
            .and_then(|f| f.rsplit_once('.'))
            .map(|(_, e)| e.to_ascii_lowercase())
            .filter(|e| e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric()));
        let path = fetched_dir().join(match ext {
            Some(e) => format!("{name}.{e}"),
            None => name,
        });
        self.web.insert(path.clone(), url.to_owned());
        path
    }

    /// `path`'s picture once it's loaded (its loading started the first
    /// time it's asked for; one on the web fetched first).
    pub fn image(&mut self, path: &Path) -> Option<Arc<RenderImage>> {
        match self.slots.get(path) {
            Some(Slot::Ready(t)) => Some(t.clone()),
            Some(_) => None,
            None => {
                self.slots.insert(path.to_owned(), Slot::Loading);
                if let Some(send) = self.send.clone() {
                    let path = path.to_owned();
                    let url = self.web.get(&path).cloned();
                    std::thread::spawn(move || {
                        let r = match url {
                            Some(url) if !path.exists() => fetch(&url, &path).and_then(|_| decode(&path)),
                            _ => decode(&path),
                        };
                        send(Loaded(path, r));
                    });
                }
                None
            }
        }
    }

    /// A worker's picture, kept.
    pub fn loaded(&mut self, l: Loaded) {
        let slot = match l.1 {
            Ok(image) => Slot::Ready(image),
            Err(_) => Slot::Failed,
        };
        self.slots.insert(l.0, slot);
    }
}

/// The picture at `url` into `to` (through a sibling, renamed into
/// place), at most `MOST_FETCHED`; pictures fetched long ago go.
fn fetch(url: &str, to: &Path) -> Result<(), String> {
    use std::io::Read as _;
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .max_redirects(10)
        .user_agent("Olo")
        .timeout_connect(Some(std::time::Duration::from_secs(20)))
        .timeout_per_read(Some(std::time::Duration::from_secs(30)))
        .build()
        .into();
    // (Across a network change, on where it broke off: `olo_net`.)
    let body = olo_net::http::Download::start(&agent, url, Some("image/*"), None).map_err(|e| format!("{url}: {e}"))?;
    let mut bytes = Vec::new();
    body.take(MOST_FETCHED + 1).read_to_end(&mut bytes).map_err(|e| format!("{url}: {e}"))?;
    if bytes.len() as u64 > MOST_FETCHED {
        return Err(format!("{url} is too big a picture"));
    }
    let dir = to.parent().ok_or("no folder")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let part = to.with_extension("part");
    std::fs::write(&part, &bytes).and_then(|_| std::fs::rename(&part, to)).map_err(|e| e.to_string())?;
    // (Those not fetched for a month go.)
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|age| age > KEEP_FETCHED)) {
            let _ = std::fs::remove_file(e.path());
        }
    }
    Ok(())
}

fn decode(path: &Path) -> Result<Arc<RenderImage>, String> {
    let (img, _) = olo_images::open(path)?;
    let img = if img.width().max(img.height()) > MOST { img.thumbnail(MOST, MOST) } else { img };
    // (The kit's pictures are BGRA, alpha straight.)
    let mut bgra = img.to_rgba8();
    for p in bgra.pixels_mut() {
        p.0.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new([image::Frame::new(bgra)])))
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

#[cfg(test)]
mod web_tests {
    use super::*;

    /// A picture on the web is kept under its address's SHA-256, with its
    /// kind as the address says it (none for an odd one), the same each
    /// time; and loading it is asked of that file.
    #[test]
    fn a_picture_on_the_web_has_a_file_of_its_own() {
        let mut p = Pictures::default();
        let a = p.web("https://example.org/a/cat.JPG?size=2#top");
        assert_eq!(a.extension().and_then(|e| e.to_str()), Some("jpg"));
        assert_eq!(a.file_stem().and_then(|s| s.to_str()).map(str::len), Some(64));
        assert_eq!(a, p.web("https://example.org/a/cat.JPG?size=2#top"));
        assert_ne!(a, p.web("https://example.org/a/dog.jpg"));
        assert!(a.starts_with(fetched_dir()));
        let odd = p.web("https://example.org/picture?id=7");
        assert_eq!(odd.extension(), None);
        assert_eq!(p.web.get(&odd).map(String::as_str), Some("https://example.org/picture?id=7"));
    }
}
