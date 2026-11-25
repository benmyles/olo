//! Files kept in step between this computer and a Rex host (TODO, "Files
//! from a host, and folders kept in step"; HACKING, Rex, "Files kept in
//! step"): what one side has (`Index`: each file's size, time, mode,
//! BLAKE3, and its content-defined chunks), the cheapest recipe for each
//! file the other side lacks or holds in another form, and the wire
//! between them. Built by `olo-rex-link sync` on a host and by the app.

pub mod chunk;
pub mod index;
pub mod recipe;
pub mod wire;

pub use chunk::{ChunkId, ChunkRef, FileDigest};
pub use index::{Entry, EntryKind, Filter, Index, MAX_SKIPPED_PATHS, Skipped, SkippedKind};
pub use recipe::{DataCodec, Piece, Plan, PlanOp, Recipe, RecipeKind, SuperChunk};
pub use wire::{FolderSource, ListEntry, ListSource, Options, PathError, Phase, Progress, Report, Source, receive, send};
