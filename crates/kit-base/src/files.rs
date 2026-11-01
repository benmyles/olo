//! Files as every app sees them: what kind a file is (by name, mode and,
//! for extension-less files, a sniff of its start), and sizes and times
//! formatted the Olo way.

use std::{path::Path, time::SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Folder,
    Text,
    Image,
    Audio,
    Video,
    Archive,
    Pdf,
    Application,
    Executable,
    Document,
    /// Word processing (Word, OpenDocument text, RTF).
    WordDocument,
    Spreadsheet,
    Presentation,
    Font,
    /// A backup of Olo's settings (Preferences ▸ Backup).
    SettingsBackup,
}

impl Kind {
    pub fn icon(self) -> &'static str {
        match self {
            Kind::Folder => "folder",
            Kind::Text => "text",
            Kind::Image => "image",
            Kind::Audio => "audio",
            Kind::Video => "video",
            Kind::Archive => "archive",
            Kind::Pdf => "pdf",
            Kind::Application => "application",
            Kind::Executable => "executable",
            Kind::Document => "document",
            Kind::WordDocument => "word-document",
            Kind::Spreadsheet => "spreadsheet",
            Kind::Presentation => "presentation",
            Kind::Font => "font",
            Kind::SettingsBackup => "backup",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Folder => "Folder",
            Kind::Text => "Text",
            Kind::Image => "Image",
            Kind::Audio => "Audio",
            Kind::Video => "Video",
            Kind::Archive => "Archive",
            Kind::Pdf => "PDF Document",
            Kind::Application => "Application",
            Kind::Executable => "Executable",
            Kind::Document => "Document",
            Kind::WordDocument => "Word Document",
            Kind::Spreadsheet => "Spreadsheet",
            Kind::Presentation => "Presentation",
            Kind::Font => "Font",
            Kind::SettingsBackup => "Olo Settings Backup",
        }
    }
}

pub fn kind_for(path: &Path, is_dir: bool, mode: u32) -> Kind {
    if is_dir {
        return Kind::Folder;
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    // Executable scripts run when opened, so show them as executables.
    if mode & 0o111 != 0 && matches!(ext.as_str(), "sh" | "py" | "pl" | "rb" | "lua" | "") {
        return Kind::Executable;
    }
    match ext.as_str() {
        "txt" | "md" | "rs" | "toml" | "json" | "yaml" | "yml" | "c" | "h" | "cpp" | "hpp" | "py" | "sh" | "js"
        | "ts" | "html" | "css" | "xml" | "conf" | "ini" | "log" | "csv" | "go" | "java" | "lua" | "rb" | "desktop" => {
            Kind::Text
        }
        "png" | "jpg" | "jpeg" | "jxl" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "svg" | "heic" | "heif" | "hif" | "avif" | "ico" => Kind::Image,
        "mp3" | "flac" | "ogg" | "oga" | "wav" | "m4a" | "aac" | "opus" | "aiff" | "aif" => Kind::Audio,
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" => Kind::Video,
        "zip" | "tar" | "gz" | "xz" | "bz2" | "zst" | "7z" | "rar" | "tgz" | "rpm" | "deb" => Kind::Archive,
        "pdf" => Kind::Pdf,
        "docx" | "doc" | "odt" | "rtf" | "pages" => Kind::WordDocument,
        "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" | "numbers" => Kind::Spreadsheet,
        "pptx" | "ppt" | "odp" | "key" => Kind::Presentation,
        "ttf" | "otf" | "ttc" | "otc" | "woff" | "woff2" => Kind::Font,
        "tsv" => Kind::Text,
        "appimage" => Kind::Application,
        "olo-backup" => Kind::SettingsBackup,
        _ if mode & 0o111 != 0 => Kind::Executable,
        "" => if sniff_text(path) { Kind::Text } else { Kind::Document },
        _ => Kind::Document,
    }
}

/// Extension-less files that look like text (README, Makefile, …).
fn sniff_text(path: &Path) -> bool {
    use std::io::Read;
    let mut buf = [0u8; 512];
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let n = f.read(&mut buf).unwrap_or(0);
    n > 0 && std::str::from_utf8(&buf[..n]).is_ok() && !buf[..n].contains(&0)
}

/// Names in the order people expect: ignoring case, and runs of digits
/// by their value ("file-2" before "file-10", "cpuhp/9" before
/// "cpuhp/10"), as Finder and modern file managers sort. Equal names
/// otherwise (leading zeros, case) fall back to plain order.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                // Each run of digits as a number: without its leading
                // zeros, the longer is the bigger, else digit by digit.
                let run = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(&c) = it.peek().filter(|c| c.is_ascii_digit()) {
                        digits.push(c);
                        it.next();
                    }
                    digits.trim_start_matches('0').to_owned()
                };
                let (m, n) = (run(&mut x), run(&mut y));
                let o = m.len().cmp(&n.len()).then_with(|| m.cmp(&n));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_lowercase().cmp(d.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// What Chrome writes a download to until it is done (`NAME.crdownload`,
/// renamed to `NAME` then): Tracker shows such a file as being written,
/// under the name it will have.
pub const DOWNLOADING: &str = ".crdownload";

/// The extended attribute a file being written carries with the size it
/// will be, in bytes (decimal digits): olo-chrome marks Chrome's
/// downloads so, and Tracker shows how far along they are.
pub const EXPECTED_SIZE: &str = "user.olo.expected_size";

/// The size `path` will be, if it is being written and says
/// ([`EXPECTED_SIZE`]).
pub fn expected_size(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let (path, name) = (std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?, std::ffi::CString::new(EXPECTED_SIZE).ok()?);
    let mut buf = [0u8; 24];
    // SAFETY: two C strings and a buffer of the length given, alive for the call.
    let n = unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
    std::str::from_utf8(buf.get(..usize::try_from(n).ok()?)?).ok()?.trim().parse().ok()
}

/// Mark `path`, being written, with the size it will be
/// ([`EXPECTED_SIZE`]); true once it is.
pub fn set_expected_size(path: &Path, size: u64) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let (Ok(path), Ok(name)) = (std::ffi::CString::new(path.as_os_str().as_bytes()), std::ffi::CString::new(EXPECTED_SIZE)) else { return false };
    let value = size.to_string();
    // SAFETY: two C strings and a buffer of the length given, alive for the call.
    unsafe { libc::setxattr(path.as_ptr(), name.as_ptr(), value.as_ptr().cast(), value.len(), 0) == 0 }
}

/// "3.1 of 10 MB" (both in `total`'s unit, short enough for a Size
/// column), for a file being written.
pub fn format_progress(done: u64, total: u64) -> String {
    let whole = format_size(total);
    let Some((_, unit)) = whole.split_once(' ') else { return whole };
    let scale = match unit {
        "KB" => 1024f64,
        "MB" => 1024f64.powi(2),
        "GB" => 1024f64.powi(3),
        "TB" => 1024f64.powi(4),
        _ => return format!("{done} of {whole}"),
    };
    let figure = |v: f64| if v >= 10.0 { format!("{v:.0}") } else { format!("{v:.1}") };
    format!("{} of {} {unit}", figure(done as f64 / scale), figure(total as f64 / scale))
}

pub fn format_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n} bytes");
    }
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v >= 100.0 { format!("{v:.0} {}", UNITS[u]) } else { format!("{v:.1} {}", UNITS[u]) }
}

pub fn format_time(t: Option<SystemTime>) -> String {
    let Some(t) = t else { return String::new() };
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    // SAFETY: localtime_r fills the zeroed tm we own.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let t = secs as libc::time_t;
        libc::localtime_r(&t, &mut tm);
        tm
    };
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let h = tm.tm_hour % 12;
    format!(
        "{} {}, {}, {}:{:02} {}",
        MONTHS[tm.tm_mon as usize % 12],
        tm.tm_mday,
        tm.tm_year + 1900,
        if h == 0 { 12 } else { h },
        tm.tm_min,
        if tm.tm_hour < 12 { "AM" } else { "PM" }
    )
}


#[cfg(test)]
mod tests {
    #[test]
    fn progress_and_expected_sizes() {
        assert_eq!(super::format_progress(3 * 1024 * 1024 + 100_000, 10 * 1024 * 1024), "3.1 of 10 MB");
        assert_eq!(super::format_progress(512 * 1024 * 1024, 4 * 1024 * 1024 * 1024 + 300 * 1024 * 1024), "0.5 of 4.3 GB");
        assert_eq!(super::format_progress(10, 500), "10 of 500 bytes");
        let dir = std::env::temp_dir().join(format!("olo-kit-expected-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.zip.crdownload");
        std::fs::write(&p, b"..").unwrap();
        assert_eq!(super::expected_size(&p), None);
        // (Where the file system takes user attributes.)
        if super::set_expected_size(&p, 123_456) {
            assert_eq!(super::expected_size(&p), Some(123_456));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn natural_order() {
        use std::cmp::Ordering::*;
        assert_eq!(super::natural_cmp("file-2", "file-10"), Less);
        assert_eq!(super::natural_cmp("cpuhp/10", "cpuhp/9"), Greater);
        assert_eq!(super::natural_cmp("Apple", "banana"), Less);
        assert_eq!(super::natural_cmp("a01", "a1"), Less);
        assert_eq!(super::natural_cmp("a", "a1"), Less);
        assert_eq!(super::natural_cmp("x", "x"), Equal);
    }
}

/// `file:///path`, with what URIs can't hold percent-encoded.
pub fn file_uri(path: &std::path::Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~!$&'()*+,;=:@".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The local paths in a `text/uri-list`: one URI a line, `#` comments.
pub fn uri_list_paths(list: &str) -> Vec<std::path::PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    list.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let rest = l.strip_prefix("file://")?;
            // file://host/path: only this computer's.
            let path = if rest.starts_with('/') { rest } else { &rest[rest.find('/')?..] };
            let bytes = path.as_bytes();
            let mut out = Vec::with_capacity(bytes.len());
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%'
                    && i + 2 < bytes.len()
                    && let Ok(b) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?, 16)
                {
                    out.push(b);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            Some(std::path::PathBuf::from(std::ffi::OsString::from_vec(out)))
        })
        .collect()
}
