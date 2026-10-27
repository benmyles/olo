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
use olo_kit::files as fs;

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

pub fn archive(path: &Path, name: &str, cancel: &Arc<AtomicBool>) -> Result<Update, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut magic = [0u8; 6];
    let n = (&file).read(&mut magic).unwrap_or(0);
    let magic = &magic[..n];
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let reader = std::io::BufReader::new(file);
    let (kind, mut items) = if magic.starts_with(b"PK") {
        ("Zip archive", zip_items(reader)?)
    } else if magic.starts_with(&[b'7', b'z', 0xbc, 0xaf, 0x27, 0x1c]) {
        ("7-Zip archive", seven_items(path)?)
    } else if magic.starts_with(&[0x1f, 0x8b]) {
        tar_or_single("gzip", Box::new(flate2::read::MultiGzDecoder::new(reader)), name, ".gz", cancel)?
    } else if magic.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) {
        tar_or_single("xz", Box::new(liblzma::read::XzDecoder::new(reader)), name, ".xz", cancel)?
    } else if magic.starts_with(b"BZh") {
        tar_or_single("bzip2", Box::new(bzip2::read::MultiBzDecoder::new(reader)), name, ".bz2", cancel)?
    } else if magic.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        let d = zstd::stream::read::Decoder::new(reader).map_err(|e| e.to_string())?;
        tar_or_single("zstd", Box::new(d), name, ".zst", cancel)?
    } else if name.ends_with(".tar") {
        ("Tar archive", tar_items(Box::new(reader), cancel)?)
    } else {
        return Ok(Update::Body(Body::Card { note: "Archive (this kind can't be looked inside)".into() }));
    };
    let total = items.len();
    sort(&mut items);
    items.truncate(MAX_ITEMS);
    let unpacked: u64 = items.iter().map(|i| i.size).sum();
    let note = format!("{kind}, {total} item{}, {} unpacked", if total == 1 { "" } else { "s" }, fs::format_size(unpacked));
    Ok(Update::Body(Body::Listing { items, note }))
}

type Stream = Box<dyn Read + Send>;

/// A compressed stream is a tarball if it unpacks to one; otherwise it is
/// one compressed file.
fn tar_or_single(
    codec: &str,
    stream: Stream,
    name: &str,
    suffix: &str,
    cancel: &Arc<AtomicBool>,
) -> Result<(&'static str, Vec<Item>), String> {
    let is_tar = name.contains(".tar") || name.ends_with(".tgz") || name.ends_with(".tbz2") || name.ends_with(".txz");
    if is_tar {
        let kind = match codec {
            "gzip" => "Tar archive (gzip)",
            "xz" => "Tar archive (xz)",
            "bzip2" => "Tar archive (bzip2)",
            _ => "Tar archive (zstd)",
        };
        return Ok((kind, tar_items(stream, cancel)?));
    }
    // One file: say what it unpacks to, and how big (counting as we go).
    let mut size = 0u64;
    let mut stream = stream;
    let mut buf = vec![0u8; 1 << 16];
    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => size += n as u64,
        }
    }
    let inner = name.strip_suffix(suffix).unwrap_or(name).to_owned();
    let kind = match codec {
        "gzip" => "gzip file",
        "xz" => "xz file",
        "bzip2" => "bzip2 file",
        _ => "zstd file",
    };
    Ok((kind, vec![Item { name: inner, is_dir: false, size }]))
}

fn tar_items(stream: Stream, cancel: &Arc<AtomicBool>) -> Result<Vec<Item>, String> {
    let mut ar = tar::Archive::new(stream);
    let mut items = Vec::new();
    for e in ar.entries().map_err(|e| e.to_string())? {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(e) = e else { break };
        let is_dir = e.header().entry_type().is_dir();
        let name = e.path().map(|p| p.to_string_lossy().trim_end_matches('/').to_owned()).unwrap_or_default();
        if name.is_empty() || name == "." {
            continue;
        }
        items.push(Item { name, is_dir, size: e.size() });
    }
    Ok(items)
}

fn zip_items(reader: std::io::BufReader<std::fs::File>) -> Result<Vec<Item>, String> {
    let mut z = zip::ZipArchive::new(reader).map_err(|e| format!("This archive can't be read ({e})."))?;
    let mut items = Vec::new();
    for i in 0..z.len() {
        let Ok(f) = z.by_index_raw(i) else { continue };
        let name = f.name().map(|n| n.trim_end_matches('/').to_owned()).unwrap_or_default();
        items.push(Item { name, is_dir: f.is_dir(), size: f.size() });
    }
    Ok(items)
}

fn seven_items(path: &Path) -> Result<Vec<Item>, String> {
    let ar = sevenz_rust2::Archive::open(path).map_err(|e| format!("This archive can't be read ({e})."))?;
    Ok(ar
        .files
        .iter()
        .map(|f| Item { name: f.name().trim_end_matches('/').to_owned(), is_dir: f.is_directory(), size: f.size() })
        .collect())
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
