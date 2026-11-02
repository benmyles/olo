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
- **matroska-demuxer 0.8.1** (Zlib OR MIT OR Apache-2.0): seeking with
  cue points that carry a CueRelativePosition (as ffmpeg writes in WebM)
  read the position against the file's first cluster instead of the cue's
  own, so a seek past the first cluster landed in the wrong place and the
  Viewer showed no picture. The patch (`seek_broad_phase` in `src/lib.rs`)
  uses the cue's cluster. Its tests (and their 1 MB of sample files) are
  left out.
- **bebook/**: the Be Book as Markdown, for reference (see its README).
