# Patched dependencies

Crates here replace their crates.io release through `[patch.crates-io]`
in the workspace `Cargo.toml`. Drop each one once upstream has the fix.

- **matroska-demuxer 0.8.1** (Zlib OR MIT OR Apache-2.0): seeking with
  cue points that carry a CueRelativePosition (as ffmpeg writes in WebM)
  read the position against the file's first cluster instead of the cue's
  own, so a seek past the first cluster landed in the wrong place and the
  Viewer showed no picture. The patch (`seek_broad_phase` in `src/lib.rs`)
  uses the cue's cluster. Its tests (and their 1 MB of sample files) are
  left out.

## Not crates

- **bebook/**: the Be Book as Markdown, for reference (see its README).
