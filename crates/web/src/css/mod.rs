//! Original CSS syntax. Token/rule construction does not validate selectors,
//! property grammars or implement cascade/layout. All long APIs are worker-only.
mod bytes;
pub use bytes::{ByteDecoder, ByteOptions, ByteParseError, ByteSyntax, EncodingChoice, EncodingSource, encoding};
pub mod cascade;
pub mod color;
pub mod fonts;
pub mod math;
pub mod media;
mod parser;
pub mod properties;
pub mod selectors;
pub mod style;
mod syntax;
pub mod variables;
pub use parser::{ParseError, Statement, StatementKind, SyntaxError};
mod tokenizer;
pub use syntax::{Component, Components, Kind, Memory, Syntax};
pub use tokenizer::{Diagnostic, DiagnosticKind, Numeric, Token, TokenKind, Tokenizer};

use std::fmt;
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub input_bytes: usize,
    pub tokens: usize,
    pub decoded_bytes: usize,
    pub depth: usize,
    pub diagnostics: usize,
    pub items: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self { input_bytes: 32 * 1024 * 1024, tokens: 2_000_000, decoded_bytes: 64 * 1024 * 1024, depth: 512, diagnostics: 65_536, items: 1_000_000 }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InputLimit,
    TokenLimit,
    DecodedLimit,
    DepthLimit,
    DiagnosticLimit,
    ItemLimit,
    Cancelled,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CSS {self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
