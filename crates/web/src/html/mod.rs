//! HTML tokenization, independently of tree construction. The tree
//! builder selects raw-text modes; a tokenizer cannot infer namespaces
//! or insertion modes from a start tag alone.

mod entities;
mod tokenizer;

pub use tokenizer::{Attribute, Doctype, Error, Limits, Mode, Tag, Token, Tokenizer};
