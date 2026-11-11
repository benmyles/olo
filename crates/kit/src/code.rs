//! Source code's syntax ([`olo_syntax`]: tree-sitter's grammars, in
//! process): its languages, its highlighter and the classes it colours
//! by (`olo_theme::Palette::syntax` has each one's colour), for an app
//! with code to show (the palette's answers). The editor's `Code`, which
//! keeps a document's spans as it's typed in, comes with the editor.

pub use olo_syntax::{self as syntax, Changed, Class, Edit, Language, by_name, detect};
