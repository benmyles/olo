# Patched dependencies

Crates here replace their crates.io release through `[patch.crates-io]`
in the workspace `Cargo.toml`. Drop each one once upstream has the fix.

- **epaint 0.37.0** (MIT OR Apache-2.0): text laid out for editing
  (`keep_trailing_whitespace`, as the kit's editor and egui's TextEdit
  do) that was centred, right-aligned or justified hung a paragraph's
  indent off the left edge, counted the space a row wraps after (so
  wrapped rows ended a space short of the right edge) and justified by
  spacing letters out as well as words. The patch
  (`halign_and_justify_edited_row` in `src/text/text_layout.rs`, with
  its test) keeps the indent, lets a wrapped row's trailing space hang
  past the edge and widens only the spaces between words. Its benches
  are left out.
- **matroska-demuxer 0.8.1** (Zlib OR MIT OR Apache-2.0): seeking with
  cue points that carry a CueRelativePosition (as ffmpeg writes in WebM)
  read the position against the file's first cluster instead of the cue's
  own, so a seek past the first cluster landed in the wrong place and the
  Viewer showed no picture. The patch (`seek_broad_phase` in `src/lib.rs`)
  uses the cue's cluster. Its tests (and their 1 MB of sample files) are
  left out.

## Not crates

- **bebook/**: the Be Book as Markdown, for reference (see its README).
