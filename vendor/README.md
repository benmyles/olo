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

- **servo-constellation 0.7.0** (MPL-2.0): in multiprocess mode it never
  notices a content process dying (crashing, or stopped by NetPositive for
  hanging or using too much memory): its pages stay attached to the dead
  event loop, so they never show the crash page and reloading them goes
  nowhere. The patch has the process manager remember which event loop
  each process runs (`process_manager.rs`, `event_loop.rs`) and, when a
  process's lifeline breaks while pages still use it, treats it as that
  event loop panicking (`Request::RemoveProcess` in `constellation.rs`).

- **Page processes after an upgrade.** Servo starts each content process
  from `current_exe()`, which names a deleted file once a rebuild or
  upgrade has replaced the binary, so every new page process failed (and
  the constellation panicked). On Linux the patch starts them from
  `/proc/self/exe`: this very binary, which also guarantees the two
  sides speak the same IPC.
