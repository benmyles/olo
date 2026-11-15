//! Folders and archives: what is inside, read natively (zip, tar with
//! gzip/xz/bzip2/zstd, 7-Zip).

use std::{
    io::Read,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use super::load::{Body, Update};
use olo_kit_base::files as fs;

/// List at most this many items (the count still covers everything).
const MAX_ITEMS: usize = 5000;

#[derive(Debug, Clone)]
pub struct Item {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

pub fn folder(path: &Path, cancel: &Arc<AtomicBool>) -> Result<Body, String> {
    let mut items = Vec::new();
    let mut total = 0;
    for e in std::fs::read_dir(path).map_err(|e| e.to_string())?.flatten() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        total += 1;
        if items.len() < MAX_ITEMS {
            let md = e.metadata().ok();
            let is_dir = md.as_ref().is_some_and(|m| m.is_dir()) || e.path().is_dir();
            items.push(Item { name, is_dir, size: md.filter(|m| !m.is_dir()).map_or(0, |m| m.len()) });
        }
    }
    sort(&mut items);
    let note = format!("Folder, {total} item{}", if total == 1 { "" } else { "s" });
    Ok(Body::Listing { items, note })
}

fn sort(items: &mut [Item]) {
    items.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
}

/// What's in an archive (`olo_archive`: zip, tar in its compressions,
/// 7-Zip, RAR, disc images, cpio), or what a single compressed file holds.
pub fn archive(path: &Path, name: &str, cancel: &Arc<AtomicBool>) -> Result<Update, String> {
    let _ = name;
    let Some(kind) = olo_archive::kind(path) else {
        return Ok(Update::Body(Body::Card { note: "Archive (this kind can't be looked inside)".into() }));
    };
    let mut items: Vec<Item> = if let olo_archive::Kind::Single(codec) = kind {
        // One file: say what it unpacks to, and how big (counting as we go).
        let mut size = 0u64;
        if let Ok(mut r) = std::fs::File::open(path).map_err(|e| e.to_string()).and_then(|f| codec.reader(f).map_err(|e| e.to_string())) {
            let mut buf = vec![0u8; 1 << 16];
            while !cancel.load(Ordering::Relaxed) {
                match r.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => size += n as u64,
                }
            }
        }
        vec![Item { name: olo_archive::base_name(path, kind), is_dir: false, size }]
    } else {
        match olo_archive::list(path, None, cancel) {
            Ok(entries) => entries
                .into_iter()
                .map(|e| Item { name: e.name.trim_end_matches('/').to_owned(), is_dir: e.kind == olo_archive::EntryKind::Dir, size: e.size })
                .filter(|i| !i.name.is_empty() && i.name != ".")
                .collect(),
            Err(olo_archive::Error::Password(_)) => return Ok(Update::Body(Body::Card { note: format!("{}, protected by a password", kind.name()) })),
            Err(olo_archive::Error::Cancelled) => Vec::new(),
            Err(e) => return Err(format!("This archive can't be read ({e}).")),
        }
    };
    let total = items.len();
    sort(&mut items);
    items.truncate(MAX_ITEMS);
    let unpacked: u64 = items.iter().map(|i| i.size).sum();
    let note = format!("{}, {total} item{}, {} unpacked", kind.name(), if total == 1 { "" } else { "s" }, fs::format_size(unpacked));
    Ok(Update::Body(Body::Listing { items, note }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_and_targz() {
        let dir = std::env::temp_dir().join(format!("ql-arch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let zp = dir.join("a.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
            w.add_directory("docs/", zip::write::SimpleFileOptions::default()).unwrap();
            w.start_file("docs/readme.txt", zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut w, b"hello").unwrap();
            w.finish().unwrap();
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let Ok(Update::Body(Body::Listing { items, note, .. })) = archive(&zp, "a.zip", &cancel) else { panic!() };
        assert!(note.contains("2 items"), "{note}");
        assert!(items[0].is_dir && items[1].name == "docs/readme.txt" && items[1].size == 5);

        let tp = dir.join("b.tar.gz");
        {
            let gz = flate2::write::GzEncoder::new(std::fs::File::create(&tp).unwrap(), flate2::Compression::fast());
            let mut t = tar::Builder::new(gz);
            let mut h = tar::Header::new_gnu();
            h.set_size(3);
            h.set_cksum();
            t.append_data(&mut h, "x.bin", &b"abc"[..]).unwrap();
            t.into_inner().unwrap().finish().unwrap();
        }
        let Ok(Update::Body(Body::Listing { items, note, .. })) = archive(&tp, "b.tar.gz", &cancel) else { panic!() };
        assert_eq!(items[0].name, "x.bin");
        assert!(note.starts_with("Tar archive (gzip)"), "{note}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
