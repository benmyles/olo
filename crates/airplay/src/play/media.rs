//! What a file is, by how it begins: a receiver plays a film itself
//! only when it's a kind it knows (it fetches the file and decodes it,
//! so nothing is converted and nothing is lost).

use std::{io::Read as _, path::Path};

use super::Error;

/// A kind of file a receiver plays: what it's served as.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Kind {
    pub mime: &'static str,
    pub ext: &'static str,
}

const fn kind(mime: &'static str, ext: &'static str) -> Kind {
    Kind { mime, ext }
}

/// The kind of what begins with these bytes; or why a receiver won't
/// play it by itself.
pub fn kind_of(start: &[u8]) -> Result<Kind, &'static str> {
    let has = |at: usize, what: &[u8]| start.get(at..at + what.len()) == Some(what);
    if has(4, b"ftyp") {
        return Ok(match start.get(8..12) {
            Some(b"qt  ") => kind("video/quicktime", "mov"),
            Some(b"M4A " | b"M4B ") => kind("audio/mp4", "m4a"),
            _ => kind("video/mp4", "mp4"),
        });
    }
    if has(0, b"#EXTM3U") {
        return Err("a playlist's parts aren't served yet: give its address on the web instead");
    }
    if has(0, &[0x47]) && has(188, &[0x47]) {
        return Ok(kind("video/mp2t", "ts"));
    }
    if has(0, b"RIFF") && has(8, b"WAVE") {
        return Ok(kind("audio/wav", "wav"));
    }
    if has(0, b"FORM") && (has(8, b"AIFF") || has(8, b"AIFC")) {
        return Ok(kind("audio/aiff", "aiff"));
    }
    if has(0, b"fLaC") {
        return Ok(kind("audio/flac", "flac"));
    }
    if has(0, b"ID3") {
        return Ok(kind("audio/mpeg", "mp3"));
    }
    if let Some(&[0xff, second]) = start.get(..2) {
        // (MPEG sound's sync: AAC by itself has no layer, MP3 has one.)
        if second & 0xf6 == 0xf0 {
            return Ok(kind("audio/aac", "aac"));
        }
        if second & 0xe0 == 0xe0 && second & 0x06 != 0 {
            return Ok(kind("audio/mpeg", "mp3"));
        }
    }
    Err(if has(0, &[0x1a, 0x45, 0xdf, 0xa3]) {
        "it's Matroska or WebM, which a receiver doesn't play by itself (converting it on the way isn't here yet)"
    } else if has(0, b"RIFF") && has(8, b"AVI ") {
        "it's AVI, which a receiver doesn't play by itself (converting it on the way isn't here yet)"
    } else if has(0, b"OggS") {
        "it's Ogg, which a receiver doesn't play by itself (converting it on the way isn't here yet)"
    } else {
        "it isn't a film or sound of a kind a receiver plays (MP4, QuickTime, MPEG-TS, MP3, AAC, WAV, AIFF, FLAC)"
    })
}

pub(crate) fn kind_of_file(path: &Path) -> Result<Kind, Error> {
    let mut start = [0u8; 512];
    let mut file = std::fs::File::open(path).map_err(|e| Error::File(format!("{}: {e}", path.display())))?;
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return Err(Error::File(format!("{} isn't a file", path.display())));
    }
    let mut n = 0;
    while n < start.len() {
        match file.read(&mut start[n..]) {
            Ok(0) => break,
            Ok(more) => n += more,
            Err(e) => return Err(Error::File(format!("{}: {e}", path.display()))),
        }
    }
    kind_of(&start[..n]).map_err(Error::CantPlay)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_by_how_they_begin() {
        assert_eq!(kind_of(b"\0\0\0\x20ftypisom\0\0\x02\0").unwrap().mime, "video/mp4");
        assert_eq!(kind_of(b"\0\0\0\x14ftypqt  \0\0\0\0").unwrap().ext, "mov");
        assert_eq!(kind_of(b"\0\0\0\x1cftypM4A \0\0\0\0").unwrap().mime, "audio/mp4");
        let mut ts = vec![0u8; 400];
        (ts[0], ts[188]) = (0x47, 0x47);
        assert_eq!(kind_of(&ts).unwrap().ext, "ts");
        assert_eq!(kind_of(b"RIFF\x24\0\0\0WAVEfmt ").unwrap().ext, "wav");
        assert_eq!(kind_of(b"ID3\x04\0\0\0\0\0\0").unwrap().ext, "mp3");
        assert_eq!(kind_of(&[0xff, 0xfb, 0x90, 0x00]).unwrap().ext, "mp3");
        assert_eq!(kind_of(&[0xff, 0xf1, 0x50, 0x80]).unwrap().ext, "aac");
        assert_eq!(kind_of(b"fLaC\0\0\0\x22").unwrap().ext, "flac");
        assert!(kind_of(&[0x1a, 0x45, 0xdf, 0xa3, 1, 2, 3]).unwrap_err().contains("Matroska"));
        assert!(kind_of(b"RIFF\x24\0\0\0AVI LIST").unwrap_err().contains("AVI"));
        assert!(kind_of(b"#EXTM3U\n#EXT-X-VERSION:3").unwrap_err().contains("playlist"));
        assert!(kind_of(b"hello").is_err());
        assert!(kind_of(b"").is_err());
    }
}
