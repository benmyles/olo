//! HTML tokenization and static document/fragment tree construction.
//! The builder selects raw-text modes; a tokenizer cannot infer namespaces
//! or insertion modes from a start tag alone.

pub mod bytes;
mod entities;
mod tokenizer;
mod tree;

pub use tokenizer::{Attribute, Doctype, Error, Limits, Mode, Tag, Token, Tokenizer};
pub use tree::{FragmentContext, ParseError, ParseOptions, ParsedDocument, ParsedFragment, QuirksMode, parse, parse_fragment};
