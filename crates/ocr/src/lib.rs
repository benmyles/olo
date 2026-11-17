//! The text in a picture, read in process: where each line and each of
//! its characters is, and what it says. For the Viewer's and Quick
//! Look's selecting and copying of the text in a picture (Ben,
//! 2026-11-16: "when viewing an image with text in it, selecting and
//! copying that text -- much like how it works in preview in macos").
//!
//! The recogniser is Robert Knight's Ocrs: a model that finds where the
//! words are, the words gathered into lines, and a model that reads
//! each line, both run by his RTen, in Rust, in this process. Its two
//! models are Olo's to ship (`data/ocr`, installed to
//! `share/olo/ocr`; CC BY-SA 4.0, see the README there): Latin letters,
//! digits and punctuation. Nothing leaves the computer.
//!
//! Reading takes a good part of a second of every core for a
//! screenful: never on a UI thread ([`read`] is for a worker), once a
//! picture (whoever shows it keeps what was read), and the models,
//! twelve megabytes and more once loaded, are let go again when nothing
//! has been read for a while ([`let_go`]).

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem};

/// A character read, and where it is in the picture (x, y, width,
/// height, in the pixels of the picture that was read).
#[derive(Debug, Clone, PartialEq)]
pub struct Char {
    pub ch: char,
    pub rect: [f32; 4],
}

/// A line of text, its characters in reading order (the spaces between
/// its words among them).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Line {
    pub chars: Vec<Char>,
}

impl Line {
    pub fn text(&self) -> String {
        self.chars.iter().map(|c| c.ch).collect()
    }

    /// The box round the whole line.
    pub fn rect(&self) -> [f32; 4] {
        let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for c in &self.chars {
            let [x, y, w, h] = c.rect;
            (l, t, r, b) = (l.min(x), t.min(y), r.max(x + w), b.max(y + h));
        }
        if self.chars.is_empty() { [0.0; 4] } else { [l, t, r - l, b - t] }
    }
}

/// Where the models are: `OLO_OCR_MODELS` (a folder), else `ocr` in
/// Olo's data folders (`olo_xdg::data_dirs`: the repository's `data`
/// above a build run from its tree, `share/olo` installed).
pub fn models() -> Option<(PathBuf, PathBuf)> {
    let both = |dir: PathBuf| {
        let (detection, recognition) = (dir.join("text-detection.onnx"), dir.join("text-recognition.onnx"));
        (detection.is_file() && recognition.is_file()).then_some((detection, recognition))
    };
    if let Some(dir) = std::env::var_os("OLO_OCR_MODELS").filter(|d| !d.is_empty()) {
        return both(dir.into());
    }
    olo_xdg::data_dirs().iter().map(|d| d.join("ocr")).find_map(both)
}

/// The recogniser, its models loaded.
pub struct Engine(OcrEngine);

impl Engine {
    pub fn load() -> Result<Engine, String> {
        let (detection, recognition) = models().ok_or("The models that read text aren’t installed.")?;
        let load = |path: &PathBuf| rten::Model::load_file(path).map_err(|e| format!("{}: {e}", path.display()));
        let engine = OcrEngine::new(OcrEngineParams { detection_model: Some(load(&detection)?), recognition_model: Some(load(&recognition)?), ..Default::default() }).map_err(|e| e.to_string())?;
        Ok(Engine(engine))
    }

    /// The text in a picture: `pixels` are its rows, `width` by
    /// `height`, three bytes a pixel (red, green, blue) or four (alpha
    /// too). The lines from the top, each with its characters.
    pub fn read(&self, pixels: &[u8], width: u32, height: u32) -> Result<Vec<Line>, String> {
        let err = |e: &dyn std::fmt::Display| e.to_string();
        let source = ImageSource::from_bytes(pixels, (width, height)).map_err(|e| err(&e))?;
        let input = self.0.prepare_input(source).map_err(|e| err(&e))?;
        let words = self.0.detect_words(&input).map_err(|e| err(&e))?;
        let lines = self.0.find_text_lines(&input, &words);
        let read = self.0.recognize_text(&input, &lines).map_err(|e| err(&e))?;
        let mut out: Vec<Line> = read
            .into_iter()
            .flatten()
            .map(|line| Line { chars: line.chars().iter().map(|c| Char { ch: c.char, rect: [c.rect.left() as f32, c.rect.top() as f32, c.rect.width() as f32, c.rect.height() as f32] }).collect() })
            .filter(|l: &Line| !l.text().trim().is_empty())
            .collect();
        // (From the top; lines level with each other, from the left.)
        out.sort_by(|a, b| {
            let (ra, rb) = (a.rect(), b.rect());
            let level = (ra[1] + ra[3] / 2.0 - (rb[1] + rb[3] / 2.0)).abs() < ra[3].min(rb[3]) / 2.0;
            if level { ra[0].total_cmp(&rb[0]) } else { ra[1].total_cmp(&rb[1]) }
        });
        Ok(out)
    }
}

/// The recogniser the process shares, while it's loaded.
static SHARED: Mutex<Option<Arc<Engine>>> = Mutex::new(None);

/// The text in a picture ([`Engine::read`]), by the recogniser the
/// process shares, loaded at the first reading (a fifth of a second).
/// For a worker thread.
pub fn read(pixels: &[u8], width: u32, height: u32) -> Result<Vec<Line>, String> {
    let engine = {
        let mut shared = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        match &*shared {
            Some(engine) => engine.clone(),
            None => {
                let start = std::time::Instant::now();
                let engine = Arc::new(Engine::load()?);
                tracing::debug!("ocr: models loaded in {:?}", start.elapsed());
                *shared = Some(engine.clone());
                engine
            }
        }
    };
    let read = engine.read(pixels, width, height);
    drop(engine);
    give_back();
    read
}

/// Let the shared recogniser's models go (nothing has been read for a
/// while): loaded again at the next reading.
pub fn let_go() {
    *SHARED.lock().unwrap_or_else(|e| e.into_inner()) = None;
    give_back();
}

/// Hand the memory a reading used back to the system. A reading of a
/// big picture takes a couple of hundred megabytes for a moment (the
/// picture in grey, each line's pixels, the models' working arrays),
/// freed when it's done, but the allocator keeps freed memory for the
/// next asking, on every thread that helped: a file manager that had
/// looked at one screenshot stayed 240 MB the bigger. `malloc_trim`
/// gives it back (glibc; a few milliseconds).
fn give_back() {
    // SAFETY: glibc's own call, with no arguments of ours to get wrong.
    #[cfg(target_env = "gnu")]
    unsafe {
        libc::malloc_trim(0);
    }
}

/// How many threads read ([`init`]).
const THREADS: &str = "4";

/// Call once as a program that reads text starts, before it has other
/// threads: makes the recogniser's pool of threads, four of them.
///
/// RTen makes its pool at the first reading, a thread a core, and keeps
/// it for good: a reading then takes every core for its moment (the
/// program's own drawing waits its turn), and the program has sixteen
/// more threads from then on. Four read a screenful in a fraction of a
/// second all the same (a 4K page of eighty lines, the worst there is,
/// in 3.3 s for 1.7), and leave the other cores alone. The pool's size
/// is read from the environment, once, so it's made here, where setting
/// that is safe (no other thread reads the environment yet), and the
/// setting taken out again (the programs this one starts don't inherit
/// it). Nothing is made where the models aren't installed.
///
/// What stays resident after a reading, the models let go: 10 to 25 MB
/// (2026-11-17, release, the Viewer on a 1920 × 1200 screenshot: 92 to
/// 103 MB unread, 110 to 122 MB read; no less with four threads than
/// with sixteen, within what runs differ by): the pool's threads and
/// what the allocator keeps of a reading's working memory after
/// `malloc_trim` (without it, 240 MB after a 4K page).
pub fn init() {
    if !available() || std::env::var_os("RTEN_NUM_THREADS").is_some() {
        return;
    }
    // SAFETY: called before the program has other threads (its contract).
    unsafe { std::env::set_var("RTEN_NUM_THREADS", THREADS) };
    let _ = rten::thread_pool();
    // SAFETY: as above.
    unsafe { std::env::remove_var("RTEN_NUM_THREADS") };
}

/// Whether text can be read here (the models are installed).
pub fn available() -> bool {
    models().is_some()
}
