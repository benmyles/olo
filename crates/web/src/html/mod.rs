//! HTML tokenization, independently of tree construction. The tree
//! builder selects raw-text modes; a tokenizer cannot infer namespaces
//! or insertion modes from a start tag alone.

mod entities;
mod tokenizer;
mod tree;

pub use tokenizer::{Attribute, Doctype, Error, Limits, Mode, Tag, Token, Tokenizer};
pub use tree::{ParseError, ParseOptions, ParsedDocument, QuirksMode, parse};
