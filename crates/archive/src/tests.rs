//! Round trips through every format written, the formats only read
//! (cpio, ISO 9660, a single compressed file), passwords, and archives
//! made to write outside where they're extracted.

use std::{
    fs,
    io::Write,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use super::*;

/// A folder of the test's own, beside its program (on disk, not /tmp).
fn scratch(name: &str) -> PathBuf {
    let d = std::env::current_exe().unwrap().parent().unwrap().join(format!("olo-archive-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// photos/ (a.jpg, sub/b.txt, an executable, a link), and notes.txt.
fn tree(root: &Path) -> Vec<PathBuf> {
    let p = root.join("photos");
    fs::create_dir_all(p.join("sub")).unwrap();
    fs::write(p.join("a.jpg"), vec![7u8; 300_000]).unwrap();
    fs::write(p.join("sub/b.txt"), "hello").unwrap();
    fs::write(p.join("run.sh"), "#!/bin/sh\necho hi\n").unwrap();
    fs::set_permissions(p.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("sub/b.txt", p.join("link")).unwrap();
    fs::write(root.join("notes.txt"), "notes").unwrap();
    sink::set_mtime(&root.join("notes.txt"), 1_700_000_000);
    vec![p, root.join("notes.txt")]
}

fn quiet() -> (AtomicBool, impl FnMut(&Progress)) {
    (AtomicBool::new(false), |_: &Progress| {})
}

#[test]
fn every_format_round_trips() {
    let d = scratch("round");
    let src = d.join("src");
    fs::create_dir_all(&src).unwrap();
    let sources = tree(&src);
    for format in Format::ALL {
        let out = d.join(format!("out{}", format.extension()));
        let (stop, mut said) = quiet();
        let mut seen = Vec::new();
        let mut progress = |p: &Progress| {
            seen.push(p.done);
            said(p)
        };
        compress(&sources, format, &out, None, &Control::new(&stop, &mut progress)).unwrap_or_else(|e| panic!("{format:?}: {e}"));
        assert!(seen.last().is_some_and(|&d| d >= 300_000), "{format:?} said how far it got");
        let k = kind(&out).unwrap_or_else(|| panic!("{format:?} not known"));
        let names: Vec<String> = list(&out, None, &stop).unwrap().into_iter().map(|e| e.name.trim_end_matches('/').to_owned()).collect();
        assert!(names.iter().any(|n| n == "photos/sub/b.txt") && names.iter().any(|n| n == "notes.txt"), "{format:?} ({k:?}) lists {names:?}");
        let into = d.join(format!("x{}", format.extension().replace('.', "-")));
        fs::create_dir_all(&into).unwrap();
        let (stop, mut said) = quiet();
        let made = extract(&out, &into, None, &Control::new(&stop, &mut said)).unwrap_or_else(|e| panic!("{format:?}: {e}"));
        // Two things at the top: in a folder named for the archive.
        assert_eq!(made, vec![into.join("out")], "{format:?}");
        let got = into.join("out");
        assert_eq!(fs::read(got.join("photos/a.jpg")).unwrap(), vec![7u8; 300_000], "{format:?}");
        assert_eq!(fs::read_to_string(got.join("photos/sub/b.txt")).unwrap(), "hello");
        assert_eq!(fs::metadata(got.join("photos/run.sh")).unwrap().permissions().mode() & 0o777, 0o755, "{format:?} kept the executable bit");
        if format != Format::SevenZip {
            assert_eq!(fs::read_link(got.join("photos/link")).unwrap(), PathBuf::from("sub/b.txt"), "{format:?} kept the link");
        }
        assert_eq!(fs::metadata(got.join("notes.txt")).unwrap().mtime(), 1_700_000_000, "{format:?} kept the time");
        // Nothing half made left behind.
        assert!(fs::read_dir(&into).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with('.')));
    }
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn one_thing_comes_out_beside_and_names_are_kept_free() {
    let d = scratch("one");
    fs::write(d.join("report.pdf"), "pdf").unwrap();
    let out = d.join(archive_name(&[d.join("report.pdf")], Format::Zip));
    assert!(out.ends_with("report.pdf.zip"));
    let (stop, mut said) = quiet();
    compress(&[d.join("report.pdf")], Format::Zip, &out, None, &Control::new(&stop, &mut said)).unwrap();
    // report.pdf is there already: the one coming out is "report 2.pdf".
    let made = extract(&out, &d, None, &Control::new(&stop, &mut said)).unwrap();
    assert_eq!(made, vec![d.join("report 2.pdf")]);
    assert_eq!(unique(&d.join("report.pdf.zip")), d.join("report.pdf 2.zip"));
    assert_eq!(base_name(Path::new("photos.tar.gz"), Kind::Tar(Codec::Gzip)), "photos");
    assert_eq!(base_name(Path::new("x.TGZ"), Kind::Tar(Codec::Gzip)), "x");
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn passwords_are_asked_for_and_checked() {
    let d = scratch("pw");
    let src = d.join("secret.txt");
    fs::write(&src, "the plans").unwrap();
    for format in [Format::Zip, Format::SevenZip] {
        let out = d.join(format!("s{}", format.extension()));
        let (stop, mut said) = quiet();
        compress(std::slice::from_ref(&src), format, &out, Some("hunter2"), &Control::new(&stop, &mut said)).unwrap();
        let into = d.join(format!("into{}", format.extension().replace('.', "-")));
        fs::create_dir_all(&into).unwrap();
        let none = extract(&out, &into, None, &Control::new(&stop, &mut said));
        assert!(matches!(none, Err(Error::Password(_))), "{format:?}: {none:?}");
        let wrong = extract(&out, &into, Some("nope"), &Control::new(&stop, &mut said));
        assert!(matches!(wrong, Err(Error::Password(true)) | Err(Error::Bad(_))), "{format:?}: {wrong:?}");
        assert!(!into.join("secret.txt").exists(), "{format:?}: nothing with a wrong password");
        extract(&out, &into, Some("hunter2"), &Control::new(&stop, &mut said)).unwrap();
        assert_eq!(fs::read_to_string(into.join("secret.txt")).unwrap(), "the plans", "{format:?}");
    }
    let _ = fs::remove_dir_all(&d);
}

/// A tar of `(name, kind byte, link, data)` written by hand: what tar
/// programs refuse to make.
fn raw_tar(entries: &[(&str, u8, &str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, kind, link, data) in entries {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..107].copy_from_slice(b"0000644");
        h[108..115].copy_from_slice(b"0000000");
        h[116..123].copy_from_slice(b"0000000");
        h[124..135].copy_from_slice(format!("{:011o}", data.len()).as_bytes());
        h[136..147].copy_from_slice(b"00000000000");
        h[156] = *kind;
        h[157..157 + link.len()].copy_from_slice(link.as_bytes());
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|b| *b as u32).sum();
        h[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
        out.extend_from_slice(&h);
        out.extend_from_slice(data);
        out.extend(std::iter::repeat_n(0u8, (512 - data.len() % 512) % 512));
    }
    out.extend([0u8; 1024]);
    out
}

#[test]
fn nothing_is_written_outside() {
    let d = scratch("evil");
    let into = d.join("into");
    fs::create_dir_all(&into).unwrap();
    let evil = raw_tar(&[
        ("../escaped.txt", b'0', "", b"out"),
        ("/tmp/olo-archive-absolute.txt", b'0', "", b"abs"),
        ("ok/fine.txt", b'0', "", b"fine"),
        ("ok/up", b'2', "../../escaped-link", b""),
        ("ok/etc", b'2', "/etc", b""),
        // A link to a folder, then a file "through" it: never followed.
        ("ok/sneaky", b'2', ".", b""),
        ("ok/sneaky/x.txt", b'0', "", b"x"),
    ]);
    let t = d.join("evil.tar");
    fs::write(&t, evil).unwrap();
    let (stop, mut said) = quiet();
    let made = extract(&t, &into, None, &Control::new(&stop, &mut said));
    // (One top-level thing, "ok": it comes out as it is.)
    let made = made.unwrap();
    assert_eq!(made, vec![into.join("ok")]);
    assert!(!d.join("escaped.txt").exists() && !Path::new("/tmp/olo-archive-absolute.txt").exists());
    assert_eq!(fs::read_to_string(into.join("ok/fine.txt")).unwrap(), "fine");
    assert!(into.join("ok/up").symlink_metadata().is_err(), "a link out isn't made");
    assert!(into.join("ok/etc").symlink_metadata().is_err(), "an absolute link isn't made");
    // ok/sneaky/x.txt went in a real folder (the link to "." made after, or not).
    assert!(!into.join("x.txt").exists());
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn a_single_compressed_file_and_cpio() {
    let d = scratch("single");
    let gz = d.join("log.txt.gz");
    let mut e = flate2::write::GzEncoder::new(fs::File::create(&gz).unwrap(), flate2::Compression::default());
    e.write_all(b"line\n").unwrap();
    e.finish().unwrap();
    assert_eq!(kind(&gz), Some(Kind::Single(Codec::Gzip)));
    let (stop, mut said) = quiet();
    let made = extract(&gz, &d, None, &Control::new(&stop, &mut said)).unwrap();
    assert_eq!(made, vec![d.join("log.txt")]);
    assert_eq!(fs::read_to_string(d.join("log.txt")).unwrap(), "line\n");

    // A newc cpio: a folder, a file in it, a link.
    let mut c = Vec::new();
    let mut add = |name: &str, mode: u32, data: &[u8]| {
        let ns = name.len() + 1;
        c.extend_from_slice(
            format!("070701{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}", 1, mode, 0, 0, 1, 1_600_000_000u32, data.len(), 0, 0, 0, 0, ns, 0).as_bytes(),
        );
        c.extend_from_slice(name.as_bytes());
        c.push(0);
        while c.len() % 4 != 0 {
            c.push(0);
        }
        c.extend_from_slice(data);
        while c.len() % 4 != 0 {
            c.push(0);
        }
    };
    add("init", 0o040755, b"");
    add("init/hello", 0o100644, b"hi there");
    add("init/link", 0o120777, b"hello");
    add("TRAILER!!!", 0, b"");
    let cp = d.join("root.cpio");
    fs::write(&cp, &c).unwrap();
    assert_eq!(kind(&cp), Some(Kind::Cpio(Codec::None)));
    let l = list(&cp, None, &stop).unwrap();
    assert_eq!(l.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["init", "init/hello", "init/link"]);
    let into = d.join("cp");
    fs::create_dir_all(&into).unwrap();
    extract(&cp, &into, None, &Control::new(&stop, &mut said)).unwrap();
    assert_eq!(fs::read_to_string(into.join("init/hello")).unwrap(), "hi there");
    assert_eq!(fs::read_link(into.join("init/link")).unwrap(), PathBuf::from("hello"));
    let _ = fs::remove_dir_all(&d);
}

/// The smallest ISO 9660 image: a root holding HELLO.TXT;1 and a folder
/// DOCS holding README.TXT;1.
fn tiny_iso() -> Vec<u8> {
    let mut img = vec![0u8; 24 * 2048];
    let rec = |lba: u32, len: u32, dir: bool, name: &[u8]| -> Vec<u8> {
        let nl = name.len();
        let rl = 33 + nl + (1 - nl % 2);
        let mut r = vec![0u8; rl];
        r[0] = rl as u8;
        r[2..6].copy_from_slice(&lba.to_le_bytes());
        r[6..10].copy_from_slice(&lba.to_be_bytes());
        r[10..14].copy_from_slice(&len.to_le_bytes());
        r[14..18].copy_from_slice(&len.to_be_bytes());
        r[18..25].copy_from_slice(&[126, 11, 15, 12, 0, 0, 0]);
        r[25] = if dir { 2 } else { 0 };
        r[32] = nl as u8;
        r[33..33 + nl].copy_from_slice(name);
        r
    };
    // Sector 16: the primary volume descriptor; 17: the terminator.
    img[16 * 2048] = 1;
    img[16 * 2048 + 1..16 * 2048 + 6].copy_from_slice(b"CD001");
    let root = rec(20, 2048, true, &[0]);
    img[16 * 2048 + 156..16 * 2048 + 156 + root.len()].copy_from_slice(&root);
    img[17 * 2048] = 255;
    img[17 * 2048 + 1..17 * 2048 + 6].copy_from_slice(b"CD001");
    // Sector 20: the root; 21: DOCS; 22, 23: the files.
    let mut at = 20 * 2048;
    for r in [rec(20, 2048, true, &[0]), rec(20, 2048, true, &[1]), rec(21, 2048, true, b"DOCS"), rec(22, 5, false, b"HELLO.TXT;1")] {
        img[at..at + r.len()].copy_from_slice(&r);
        at += r.len();
    }
    let mut at = 21 * 2048;
    for r in [rec(21, 2048, true, &[0]), rec(20, 2048, true, &[1]), rec(23, 6, false, b"README.TXT;1")] {
        img[at..at + r.len()].copy_from_slice(&r);
        at += r.len();
    }
    img[22 * 2048..22 * 2048 + 5].copy_from_slice(b"hello");
    img[23 * 2048..23 * 2048 + 6].copy_from_slice(b"readme");
    img
}

#[test]
fn a_disc_image_is_read() {
    let d = scratch("iso");
    let iso = d.join("disc.iso");
    fs::write(&iso, tiny_iso()).unwrap();
    assert_eq!(kind(&iso), Some(Kind::Iso));
    let (stop, mut said) = quiet();
    let mut names: Vec<String> = list(&iso, None, &stop).unwrap().into_iter().map(|e| e.name).collect();
    names.sort();
    assert_eq!(names, ["DOCS", "DOCS/README.TXT", "HELLO.TXT"]);
    let into = d.join("x");
    fs::create_dir_all(&into).unwrap();
    let made = extract(&iso, &into, None, &Control::new(&stop, &mut said)).unwrap();
    assert_eq!(made, vec![into.join("disc")]);
    assert_eq!(fs::read_to_string(into.join("disc/HELLO.TXT")).unwrap(), "hello");
    assert_eq!(fs::read_to_string(into.join("disc/DOCS/README.TXT")).unwrap(), "readme");
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn stopping_leaves_nothing() {
    let d = scratch("stop");
    let big = d.join("big.bin");
    fs::write(&big, vec![1u8; 4 << 20]).unwrap();
    let stop = AtomicBool::new(false);
    // (Stopped as soon as it starts on its file: progress is said at most
    // every 50 ms, and 4 MB is made faster than that.)
    let mut progress = |p: &Progress| {
        if !p.item.is_empty() {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    };
    let out = d.join("big.tar.gz");
    let r = compress(std::slice::from_ref(&big), Format::TarGz, &out, None, &Control::new(&stop, &mut progress));
    assert!(matches!(r, Err(Error::Cancelled)), "{r:?}");
    assert!(!out.exists());
    assert_eq!(fs::read_dir(&d).unwrap().count(), 1, "no half-made archive left");
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn safe_paths() {
    assert_eq!(safe_path("a/b/c.txt"), Some(PathBuf::from("a/b/c.txt")));
    assert_eq!(safe_path("./a//b"), Some(PathBuf::from("a/b")));
    assert_eq!(safe_path("a\\b.txt"), Some(PathBuf::from("a/b.txt")));
    for bad in ["../x", "a/../../x", "/etc/passwd", "C:/x", "", ".", "a\0b"] {
        assert_eq!(safe_path(bad), None, "{bad:?}");
    }
    assert_eq!(Format::of_word("tar.gz"), Some(Format::TarGz));
}

#[test]
fn rar_is_read() {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let d = scratch("rar");
    let (stop, mut said) = quiet();
    for name in ["solid.rar", "unicode.rar", "crypted.rar"] {
        let rar = data.join(name);
        assert_eq!(kind(&rar), Some(Kind::Rar), "{name}");
        let pw = (name == "crypted.rar").then_some("unrar");
        if pw.is_some() {
            let into = d.join("nopw");
            fs::create_dir_all(&into).unwrap();
            let r = extract(&rar, &into, None, &Control::new(&stop, &mut said));
            assert!(matches!(r, Err(Error::Password(_))), "{name}: {r:?}");
        }
        let entries = list(&rar, pw, &stop).unwrap();
        assert!(!entries.is_empty(), "{name}");
        let into = d.join(name.replace('.', "-"));
        fs::create_dir_all(&into).unwrap();
        let made = extract(&rar, &into, pw, &Control::new(&stop, &mut said)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(!made.is_empty(), "{name}");
        for e in entries.iter().filter(|e| e.kind == EntryKind::File) {
            let at = if made[0].is_dir() && made[0].file_name() != e.path.as_ref().unwrap().components().next().map(|c| c.as_os_str()) {
                made[0].join(e.path.as_ref().unwrap())
            } else {
                into.join(e.path.as_ref().unwrap())
            };
            assert_eq!(fs::metadata(&at).map(|m| m.len()).unwrap_or(u64::MAX), e.size, "{name}: {}", e.name);
        }
    }
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn some_of_it_comes_out() {
    let d = scratch("some");
    let src = d.join("src");
    fs::create_dir_all(&src).unwrap();
    let sources = tree(&src);
    let (stop, mut said) = quiet();
    for format in [Format::Zip, Format::TarGz, Format::SevenZip] {
        let out = d.join(format!("a{}", format.extension()));
        compress(&sources, format, &out, None, &Control::new(&stop, &mut said)).unwrap();
        let into = d.join(format!("some{}", format.extension().replace('.', "-")));
        fs::create_dir_all(&into).unwrap();
        // From inside "photos": its "sub" folder and a.jpg, as they are there.
        let made = extract_some(&out, &["photos/sub".into(), "photos/a.jpg".into()], "photos", &into, None, &Control::new(&stop, &mut said)).unwrap();
        let mut names: Vec<String> = made.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["a.jpg", "sub"], "{format:?}");
        assert_eq!(fs::read_to_string(into.join("sub/b.txt")).unwrap(), "hello", "{format:?}");
        assert!(!into.join("notes.txt").exists() && !into.join("run.sh").exists(), "{format:?}: only what was asked for");
    }
    let _ = fs::remove_dir_all(&d);
}
