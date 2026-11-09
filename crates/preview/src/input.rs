//! Where a media file's bytes come from: a file on disk, or anything else
//! that can be read and seeked.
//! Each decoder (pictures, sound) opens its own reader.

use std::{
    io::{self, Read, Seek},
    path::Path,
    sync::Arc,
};

pub trait ReadSeek: Read + Seek + Send + Sync {}
impl<T: Read + Seek + Send + Sync> ReadSeek for T {}

type Opener = dyn Fn() -> io::Result<(Box<dyn ReadSeek>, u64)> + Send + Sync;

#[derive(Clone)]
pub struct Input {
    open: Arc<Opener>,
    /// The format, as a file extension ("mp4", "webm", "mp3"…), a hint.
    pub ext: String,
    /// What to call it in warnings.
    pub name: String,
    /// A web page's Media Source Extensions stream, appended to as it
    /// plays, instead of bytes to read.
    stream: Option<Arc<olo_mse::Stream>>,
}

impl Input {
    pub fn file(path: &Path) -> Input {
        let p = path.to_owned();
        Input {
            open: Arc::new(move || {
                let f = std::fs::File::open(&p)?;
                let size = f.metadata()?.len();
                Ok((Box::new(f) as Box<dyn ReadSeek>, size))
            }),
            ext: path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default(),
            name: path.display().to_string(),
            stream: None,
        }
    }

    /// `open` gives a fresh reader at the start, and the size in bytes.
    pub fn new(ext: &str, name: &str, open: impl Fn() -> io::Result<(Box<dyn ReadSeek>, u64)> + Send + Sync + 'static) -> Input {
        Input { open: Arc::new(open), ext: ext.to_lowercase(), name: name.into(), stream: None }
    }

    /// A page's MediaSource stream (pictures only: its frames come as the
    /// page appends them, and the player waits for them).
    pub fn media_source(stream: Arc<olo_mse::Stream>, name: &str) -> Input {
        Input { open: Arc::new(|| Err(io::Error::other("a media source stream is not read as a file"))), ext: "mse".into(), name: name.into(), stream: Some(stream) }
    }

    pub fn media_stream(&self) -> Option<&Arc<olo_mse::Stream>> {
        self.stream.as_ref()
    }

    pub fn open(&self) -> Result<(Box<dyn ReadSeek>, u64), String> {
        (self.open)().map_err(|e| e.to_string())
    }

    /// Matroska or WebM rather than MPEG-4.
    pub fn matroska(&self) -> bool {
        matches!(self.ext.as_str(), "mkv" | "webm" | "mka")
    }
}

/// A reader as symphonia takes it.
pub struct Source {
    pub reader: Box<dyn ReadSeek>,
    pub size: u64,
}

impl Read for Source {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buf)
    }
}

impl Seek for Source {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        self.reader.seek(pos)
    }
}

impl symphonia::core::io::MediaSource for Source {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.size)
    }
}
