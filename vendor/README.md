# Patched dependencies

Crates here replace their crates.io release through `[patch.crates-io]`
in the workspace `Cargo.toml`. Drop each one once upstream has the fix.

- **blocking 1.7.0** (Apache-2.0 OR MIT): an idle pool thread stopped
  only when there were more threads than the limit, and the limit is
  set to 500 when the first thread starts, so an idle thread never
  stopped: each woke every half second for good, in every program that
  had once read a file or socket through it (zbus for UDisks, smol in
  GPUI): DriveSetup idled at 4 wakeups a second from two of them. The
  patch (`Executor::main_loop` in `src/lib.rs`) stops a thread that is
  over the limit *or* has had nothing to do for half a second, as its
  comment says it should.
- **egui-wgpu 0.37.0** (MIT OR Apache-2.0): each frame's vertices and
  indices went to the GPU through `Queue::write_buffer_with`, which
  allocates a staging buffer every time, and a window full of text (3
  MB of vertices) waited up to 9 ms a frame for the driver to allocate
  it. The patch (`Renderer::staging` in `src/renderer.rs`) writes them
  through a `wgpu::util::StagingBelt`, whose buffers are used again
  once the GPU has copied from them. And its vertex and index buffers
  grew only once full, to exactly what that frame needed, so a scroll a
  few vertices bigger waited 9 ms for the driver to allocate new GPU
  memory: they grow to half again what's needed, and a buffer twice as
  big is made on a worker once one is three quarters full
  (`SlicedBuffer::reserve`).
- **epaint 0.37.0** (MIT OR Apache-2.0): text laid out for editing
  (`keep_trailing_whitespace`, as the kit's editor and egui's TextEdit
  do) that was centred, right-aligned or justified hung a paragraph's
  indent off the left edge, counted the space a row wraps after (so
  wrapped rows ended a space short of the right edge) and justified by
  spacing letters out as well as words. The patch
  (`halign_and_justify_edited_row` in `src/text/text_layout.rs`, with
  its test) keeps the indent, lets a wrapped row's trailing space hang
  past the edge and widens only the spaces between words. Its benches
  are left out. Also, for speed: `FontsView::texture_atlas` (in
  `src/text/fonts.rs`), which `Fonts` has, as the kit tessellates a
  frame's shapes on several threads and a tessellator needs the atlas;
  `ShapedCache` (in `src/text/text_layout.rs`, kept by the galley cache),
  text shaped as it was before by what shapes it, not the width it wraps
  at or its colours, so a live resize or a theme switch only breaks
  lines again (30 ms a frame of Notifications' resize was shaping);
  sections differing only in how they're painted (a line highlighted in
  colours) shaped together (`shape`); and a harfrust shape plan made
  once per face and script (`FontFace::shape_plan`), not for every run
  (nearly all the time a highlighted line took). And text only moved
  (`TextShape::transform` with no scaling, in `src/shapes/text_shape.rs`)
  keeps its galley, which was copied row by row and glyph by glyph: a
  list's cards shown again moved (`olo_kit::recorded`) made their frames
  slower, not faster. Text and rects on a half pixel (a third of a point
  at 1.5×) rounded up or down by the last bit of the sum that put them
  there; a half now always rounds up (`round_to_pixels` in
  `src/tessellator.rs`), so a card painted moved and painted there come
  out the same. And `Fonts::galley_generation` (in
  `src/text/fonts.rs`) changes whenever galleys laid out before stop
  painting right (the atlas emptied when full, the fonts changed), for
  those kept across frames. And tab stops, which egui hasn't (a tab was
  four spaces wherever it fell): `LayoutJob::tabs` (`TabStops`: stops
  lining text up on their left, centre, right or decimal point, then one
  every so many points) widens each tab to take the text after it to its
  stop, measured from its row's start, once a paragraph is shaped and
  again from each new row's start as rows wrap (`set_tabs` in
  `src/text/text_layout.rs`, with `test_tab_stops`); justified rows widen
  only the spaces after their last tab. (Run epaint's tests from
  vendor/epaint with `--features default_fonts` and a `CARGO_TARGET_DIR`
  of its own: it isn't a workspace member.)
- **matroska-demuxer 0.8.1** (Zlib OR MIT OR Apache-2.0): seeking with
  cue points that carry a CueRelativePosition (as ffmpeg writes in WebM)
  read the position against the file's first cluster instead of the cue's
  own, so a seek past the first cluster landed in the wrong place and the
  Viewer showed no picture. The patch (`seek_broad_phase` in `src/lib.rs`)
  uses the cue's cluster. Its tests (and their 1 MB of sample files) are
  left out.
- **wgpu-hal 30.0.1** (MIT OR Apache-2.0): a texture too big to share
  one of gpu-allocator's blocks (a photo) got memory of its own while
  holding the allocator's lock, and the driver (Honeykrisp) takes 17 ms
  to allocate 100 MB: every buffer another thread made meanwhile (a
  window's vertices, each frame) waited for it, and the frame missed
  its time. The patch (`create_texture` in `src/vulkan/device.rs`)
  allocates memory of their own for textures of 16 MB and up without
  taking the lock, as wgpu-hal already does for imported ones, and for
  buffers of 4 MB and up the CPU doesn't map (`create_buffer`: egui's
  renderer makes its bigger ones on a worker). The
  backends Olo doesn't build (DirectX 12, Metal) and the examples are
  left out.

## Not crates

- **bebook/**: the Be Book as Markdown, for reference (see its README).
