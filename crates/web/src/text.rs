//! Worker text processing: streaming byte decoding and native glyph shaping.
//! No I/O, polling or UI work. UTF-8 borrows validated runs.
pub mod linebreak;
pub mod prepare;
pub mod shaping;
pub(crate) mod unicode;
use encoding_rs::{CoderResult, Encoding, UTF_8};
pub use encoding_rs::{Encoding as CharacterEncoding, UTF_8 as UTF8};
use std::sync::atomic::{AtomicBool, Ordering};
const INPUT_CHUNK: usize = 65536;
const OUTPUT_CHUNK: usize = 16384;
/// Limits count raw input bytes and emitted UTF-8 bytes separately.
#[derive(Clone, Copy, Debug)]
pub struct DecodeLimits {
    pub input_bytes: usize,
    pub output_bytes: usize,
}
impl Default for DecodeLimits {
    fn default() -> Self {
        Self { input_bytes: 32 * 1024 * 1024, output_bytes: 64 * 1024 * 1024 }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    InputLimit,
    OutputLimit,
    Cancelled,
    Finished,
}
impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Text decode {self:?}")
    }
}
impl std::error::Error for DecodeError {}
pub type DecodeResult<T> = Result<T, DecodeError>;
/// Input counts received bytes, including a pending multibyte sequence.
/// Output counts bytes offered to the sink; a sink may reject that offer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecodeProgress {
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub had_errors: bool,
}

/// Caller selects the encoding and removes its initial BOM, if any.
/// Sinks receive valid UTF-8 with bounded callback/cancellation intervals.
/// UTF-8 uses no heap buffer; other codecs use a fixed 16 KiB staging buffer.
/// Call on a worker; sinks must not block. `last` flushes pending state.
/// Fatal errors and finalization are terminal; subsequent calls never
/// resume from a partially consumed chunk or panic inside a finished codec.
pub struct Decoder<'a> {
    encoding: &'static Encoding,
    codec: Option<encoding_rs::Decoder>,
    scratch: String,
    pending: [u8; 4],
    pending_len: usize,
    limits: DecodeLimits,
    progress: DecodeProgress,
    cancelled: Option<&'a AtomicBool>,
    terminal: Option<DecodeError>,
}
impl<'a> Decoder<'a> {
    pub fn new(encoding: &'static Encoding, limits: DecodeLimits, cancelled: Option<&'a AtomicBool>) -> Self {
        let utf8 = encoding == UTF_8;
        Self {
            encoding,
            codec: (!utf8).then(|| encoding.new_decoder_without_bom_handling()),
            scratch: if utf8 { String::new() } else { "\0".repeat(OUTPUT_CHUNK) },
            pending: [0; 4],
            pending_len: 0,
            limits,
            progress: DecodeProgress::default(),
            cancelled,
            terminal: None,
        }
    }
    pub fn encoding(&self) -> &'static Encoding {
        self.encoding
    }
    pub fn progress(&self) -> DecodeProgress {
        self.progress
    }
    pub fn push(&mut self, bytes: &[u8], last: bool, mut sink: impl FnMut(&str) -> DecodeResult<()>) -> DecodeResult<()> {
        if let Some(error) = self.terminal {
            return Err(error);
        }
        let result = self.push_inner(bytes, last, &mut sink);
        if let Err(error) = result {
            self.terminal = Some(error);
        } else if last {
            self.terminal = Some(DecodeError::Finished);
        }
        result
    }
    fn check(&self) -> DecodeResult<()> {
        if self.cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(DecodeError::Cancelled) } else { Ok(()) }
    }
    fn push_inner(&mut self, bytes: &[u8], last: bool, sink: &mut impl FnMut(&str) -> DecodeResult<()>) -> DecodeResult<()> {
        self.check()?;
        let total = self.progress.input_bytes.checked_add(bytes.len()).ok_or(DecodeError::InputLimit)?;
        if total > self.limits.input_bytes {
            return Err(DecodeError::InputLimit);
        }
        self.progress.input_bytes = total;
        if self.encoding == UTF_8 {
            let mut at = 0;
            while at < bytes.len() {
                self.check()?;
                let end = (at + INPUT_CHUNK).min(bytes.len());
                self.utf8_chunk(&bytes[at..end], last && end == bytes.len(), sink)?;
                at = end;
            }
            if last && bytes.is_empty() {
                self.utf8_chunk(&[], true, sink)?;
            }
        } else {
            let mut at = 0;
            loop {
                self.check()?;
                let end = (at + INPUT_CHUNK).min(bytes.len());
                let final_chunk = last && end == bytes.len();
                let (result, read, written, errors) = self.codec.as_mut().unwrap().decode_to_str(&bytes[at..end], self.scratch.as_mut_str(), final_chunk);
                at += read;
                self.progress.had_errors |= errors;
                emit(&mut self.progress, self.limits, &self.scratch[..written], sink)?;
                if result == CoderResult::InputEmpty && at == bytes.len() {
                    break;
                }
            }
        }
        Ok(())
    }
    fn utf8_chunk(&mut self, bytes: &[u8], last: bool, sink: &mut impl FnMut(&str) -> DecodeResult<()>) -> DecodeResult<()> {
        let mut at = 0;
        while self.pending_len > 0 && at < bytes.len() {
            self.check()?;
            let old = self.pending_len;
            let take = (4 - old).min(bytes.len() - at);
            let mut combined = [0; 4];
            combined[..old].copy_from_slice(&self.pending[..old]);
            combined[old..old + take].copy_from_slice(&bytes[at..at + take]);
            at += take;
            self.pending_len = 0;
            self.utf8_piece::<{ !cfg!(feature = "scalar-scans") }>(&combined[..old + take], last && at == bytes.len(), sink)?;
        }
        if at < bytes.len() {
            self.utf8_piece::<{ !cfg!(feature = "scalar-scans") }>(&bytes[at..], last, sink)?;
        } else if last && self.pending_len > 0 {
            let pending = self.pending;
            let len = self.pending_len;
            self.pending_len = 0;
            self.utf8_piece::<{ !cfg!(feature = "scalar-scans") }>(&pending[..len], true, sink)?;
        }
        Ok(())
    }
    fn utf8_piece<const SIMD: bool>(&mut self, mut bytes: &[u8], last: bool, sink: &mut impl FnMut(&str) -> DecodeResult<()>) -> DecodeResult<()> {
        while !bytes.is_empty() {
            self.check()?;
            match validate::<SIMD>(bytes) {
                Ok(text) => {
                    emit(&mut self.progress, self.limits, text, sink)?;
                    break;
                }
                Err((valid, error_len)) => {
                    if valid > 0 {
                        let text = validate::<SIMD>(&bytes[..valid]).expect("UTF-8 validator supplied valid prefix");
                        emit(&mut self.progress, self.limits, text, sink)?;
                        bytes = &bytes[valid..];
                    }
                    if let Some(length) = error_len {
                        self.progress.had_errors = true;
                        emit(&mut self.progress, self.limits, "\u{fffd}", sink)?;
                        bytes = &bytes[length..];
                        // Enter a separately compiled scalar loop once;
                        // no validator dispatch for each malformed byte.
                        if SIMD && !cfg!(feature = "baseline-utf8-errors") {
                            return self.utf8_piece::<false>(bytes, last, sink);
                        }
                    } else if last {
                        self.progress.had_errors = true;
                        emit(&mut self.progress, self.limits, "\u{fffd}", sink)?;
                        break;
                    } else {
                        debug_assert!(bytes.len() <= 3);
                        self.pending[..bytes.len()].copy_from_slice(bytes);
                        self.pending_len = bytes.len();
                        break;
                    }
                }
            }
        }
        Ok(())
    }
}
fn emit(progress: &mut DecodeProgress, limits: DecodeLimits, text: &str, sink: &mut impl FnMut(&str) -> DecodeResult<()>) -> DecodeResult<()> {
    if text.is_empty() {
        return Ok(());
    }
    let total = progress.output_bytes.checked_add(text.len()).ok_or(DecodeError::OutputLimit)?;
    if total > limits.output_bytes {
        return Err(DecodeError::OutputLimit);
    }
    progress.output_bytes = total;
    sink(text)
}
// SIMD validates ordinary runs; dense malformed data benefits from the
// standard scalar recovery path. Reset at each <=64 KiB input piece so a
// damaged prefix cannot disable acceleration for the rest of a resource.
fn validate<const SIMD: bool>(bytes: &[u8]) -> Result<&str, (usize, Option<usize>)> {
    #[cfg(not(feature = "scalar-scans"))]
    if SIMD {
        return simdutf8::compat::from_utf8(bytes).map_err(|e| (e.valid_up_to(), e.error_len()));
    }
    std::str::from_utf8(bytes).map_err(|e| (e.valid_up_to(), e.error_len()))
}

/// Append without an unbounded geometric capacity increase. The decoder
/// bounds produced bytes independently; this also bounds collector capacity.
pub fn collect(output: &mut String, text: &str, limit: usize) -> DecodeResult<()> {
    let needed = output.len().checked_add(text.len()).ok_or(DecodeError::OutputLimit)?;
    if needed > limit {
        return Err(DecodeError::OutputLimit);
    }
    if output.capacity() < needed {
        let target = needed.max(output.len().saturating_mul(2)).min(limit);
        output.reserve_exact(target - output.len());
    }
    output.push_str(text);
    Ok(())
}
