//! CSS byte-stream encoding selection and incremental decoding. Sniffing
//! is bounded to 1024 bytes and never runs CSS tokenization on byte labels.
use super::{Error, Limits, Syntax};
use crate::text::{self, DecodeError, DecodeLimits, DecodeProgress, DecodeResult, Decoder};
use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE};
use std::sync::atomic::{AtomicBool, Ordering};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodingSource {
    Bom,
    Protocol,
    Charset,
    Environment,
    Default,
}
#[derive(Clone, Copy, Debug)]
pub struct EncodingChoice {
    pub encoding: &'static Encoding,
    pub source: EncodingSource,
    pub bom_bytes: usize,
}
#[derive(Clone, Copy, Default)]
pub struct ByteOptions<'a> {
    pub protocol_encoding: Option<&'a str>,
    pub environment_encoding: Option<&'static Encoding>,
}
impl ByteOptions<'_> {
    fn protocol(&self) -> Option<&'static Encoding> {
        self.protocol_encoding.and_then(|label| Encoding::for_label(label.as_bytes()))
    }
}
/// Resolve a complete resource's encoding (or its complete first 1024 bytes).
/// A partial prefix needs `ByteDecoder` so a split BOM/label is not mistaken
/// for absent metadata. Labels use the Encoding Standard alias table.
pub fn encoding(bytes: &[u8], options: ByteOptions<'_>) -> EncodingChoice {
    if let Some((encoding, bom_bytes)) = Encoding::for_bom(bytes) {
        return EncodingChoice { encoding, source: EncodingSource::Bom, bom_bytes };
    }
    if let Some(encoding) = options.protocol() {
        return EncodingChoice { encoding, source: EncodingSource::Protocol, bom_bytes: 0 };
    }
    let prefix = &bytes[..bytes.len().min(1024)];
    if let Some(label) = prefix.strip_prefix(b"@charset \"")
        && let Some(end) = label.iter().position(|&b| b == b'"')
        && label.get(end + 1) == Some(&b';')
        && label[..end].is_ascii()
        && let Some(mut encoding) = Encoding::for_label(&label[..end])
    {
        if encoding == UTF_16BE || encoding == UTF_16LE {
            encoding = UTF_8;
        }
        return EncodingChoice { encoding, source: EncodingSource::Charset, bom_bytes: 0 };
    }
    if let Some(encoding) = options.environment_encoding {
        return EncodingChoice { encoding, source: EncodingSource::Environment, bom_bytes: 0 };
    }
    EncodingChoice { encoding: UTF_8, source: EncodingSource::Default, bom_bytes: 0 }
}
/// Worker-only incremental CSS decoding, not incremental tokenization.
/// Holds at most 1024 sniffing bytes, plus the selected text decoder.
/// BOMs are removed; an encoding declaration stays in the decoded source.
/// `last`, fatal errors and rejected sink offers are terminal.
pub struct ByteDecoder<'a> {
    protocol: Option<&'static Encoding>,
    environment: Option<&'static Encoding>,
    limits: DecodeLimits,
    cancelled: Option<&'a AtomicBool>,
    prefix: [u8; 1024],
    prefix_len: usize,
    received: usize,
    decoder: Option<Decoder<'a>>,
    choice: Option<EncodingChoice>,
    terminal: Option<DecodeError>,
}
impl<'a> ByteDecoder<'a> {
    pub fn new(options: ByteOptions<'_>, limits: DecodeLimits, cancelled: Option<&'a AtomicBool>) -> Self {
        Self { protocol: options.protocol(), environment: options.environment_encoding, limits, cancelled, prefix: [0; 1024], prefix_len: 0, received: 0, decoder: None, choice: None, terminal: None }
    }
    pub fn choice(&self) -> Option<EncodingChoice> {
        self.choice
    }
    /// Input counts bytes received, including any pending prefix/BOM;
    /// output counts decoded bytes. Sinks may keep their own delivered count.
    pub fn progress(&self) -> DecodeProgress {
        let mut p = self.decoder.as_ref().map_or(DecodeProgress::default(), Decoder::progress);
        p.input_bytes = self.received;
        p
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
    fn ready(&self, last: bool) -> bool {
        let p = &self.prefix[..self.prefix_len];
        if last || p.len() == 1024 || Encoding::for_bom(p).is_some() {
            return true;
        }
        if p.len() < 3 {
            return false;
        }
        if self.protocol.is_some() {
            return true;
        }
        let pattern = b"@charset \"";
        let n = p.len().min(pattern.len());
        if p[..n] != pattern[..n] {
            return true;
        }
        if p.len() >= pattern.len() {
            let label = &p[pattern.len()..];
            if let Some(end) = label.iter().position(|&b| b == b'"' || !b.is_ascii()) {
                return !label[end].is_ascii() || end + 1 < label.len();
            }
        }
        false
    }
    fn push_inner(&mut self, mut bytes: &[u8], last: bool, sink: &mut impl FnMut(&str) -> DecodeResult<()>) -> DecodeResult<()> {
        if self.cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(DecodeError::Cancelled);
        }
        let total = self.received.checked_add(bytes.len()).ok_or(DecodeError::InputLimit)?;
        if total > self.limits.input_bytes {
            return Err(DecodeError::InputLimit);
        }
        self.received = total;
        while self.decoder.is_none() {
            if self.ready(last && bytes.is_empty()) {
                let options = ByteOptions { protocol_encoding: None, environment_encoding: self.environment };
                let mut choice = encoding(&self.prefix[..self.prefix_len], options);
                if choice.source != EncodingSource::Bom
                    && let Some(protocol) = self.protocol
                {
                    choice = EncodingChoice { encoding: protocol, source: EncodingSource::Protocol, bom_bytes: 0 };
                }
                self.choice = Some(choice);
                self.decoder = Some(Decoder::new(choice.encoding, self.limits, self.cancelled));
                self.decoder.as_mut().unwrap().push(&self.prefix[choice.bom_bytes..self.prefix_len], last && bytes.is_empty(), &mut *sink)?;
                if bytes.is_empty() {
                    return Ok(());
                }
                break;
            }
            if bytes.is_empty() {
                return Ok(());
            }
            let target = if self.prefix_len < 3 { 3 } else { 1024 };
            let take = (target - self.prefix_len).min(bytes.len());
            self.prefix[self.prefix_len..self.prefix_len + take].copy_from_slice(&bytes[..take]);
            self.prefix_len += take;
            bytes = &bytes[take..];
        }
        self.decoder.as_mut().unwrap().push(bytes, last, sink)
    }
}
#[derive(Debug)]
pub enum ByteParseError {
    Decode(DecodeError),
    Syntax(Error),
}
impl From<DecodeError> for ByteParseError {
    fn from(e: DecodeError) -> Self {
        Self::Decode(e)
    }
}
impl From<Error> for ByteParseError {
    fn from(e: Error) -> Self {
        Self::Syntax(e)
    }
}
impl std::fmt::Display for ByteParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CSS byte parse {self:?}")
    }
}
impl std::error::Error for ByteParseError {}
pub struct ByteSyntax {
    pub syntax: Syntax,
    pub encoding: EncodingChoice,
    pub had_errors: bool,
}
impl Syntax {
    /// Decode a complete stylesheet and build its generic component arena.
    /// `Limits::input_bytes` bounds both raw input and decoded source bytes.
    /// This does not validate selectors/properties or compile/apply style.
    pub fn parse_bytes(bytes: &[u8], options: ByteOptions<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<ByteSyntax, ByteParseError> {
        let mut decoder = ByteDecoder::new(options, DecodeLimits { input_bytes: limits.input_bytes, output_bytes: limits.input_bytes }, cancelled);
        // The whole input is already available here. Reserve once after
        // the decoder's quota/cancellation preflight, not before it.
        let mut output = String::new();
        let mut first = true;
        decoder.push(bytes, true, |text| {
            if first {
                output.reserve_exact(bytes.len());
                first = false;
            }
            text::collect(&mut output, text, limits.input_bytes)
        })?;
        // Expansion/contraction is unknowable before decoding legacy or
        // malformed input. Keep exact retained storage, without converting
        // String's buffer to a separately allocated Arc<str>.
        output.shrink_to_fit();
        let choice = decoder.choice().expect("finalized decoder has selected an encoding");
        let had_errors = decoder.progress().had_errors;
        #[cfg(feature = "baseline-css-byte-copy")]
        let syntax = Syntax::parse(std::sync::Arc::from(output), limits, cancelled)?;
        #[cfg(not(feature = "baseline-css-byte-copy"))]
        let syntax = Syntax::parse_owned(output, limits, cancelled)?;
        Ok(ByteSyntax { syntax, encoding: choice, had_errors })
    }
}
