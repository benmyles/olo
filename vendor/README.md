# Patched dependencies

Crates here replace their crates.io release through `[patch.crates-io]`
in the workspace `Cargo.toml`. Drop each one once upstream has the fix.

- **epaint 0.37.0** (MIT OR Apache-2.0): its glyph rasterizer makes
  `vello_cpu` render contexts with default settings and renders them
  without `flush()`. When another crate in the build turns on vello_cpu's
  `multithreading` feature (Servo's canvas does, so building the browser
  turns it on for every app built alongside it), those contexts get
  worker threads and the first glyph panics with "attempted to rasterize
  before flushing". The patch (`single_threaded()` in
  `src/text/font_face.rs`) asks for single-threaded contexts, as a glyph
  is far too small for threads anyway.
