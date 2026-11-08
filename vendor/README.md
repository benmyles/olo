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
- **velato 0.12.0** (Apache-2.0 OR MIT): it read 11 of Telegram
  Desktop's 52 animated stickers. 24 hit `todo!()` (a transform with no
  rotation, or a split one), 17 a position it required; and a frame
  landing on a property's last keyframe weighed it 0/0, NaN for every
  point after (each such path cost vello_cpu milliseconds, drawing
  nothing). The patch (`src/import/converters.rs`, `src/schema/helpers/
  transform.rs`, `src/runtime/model/value.rs`, each marked "Olo:"):
  no rotation is none, a split one its z, a missing position the origin,
  Add and Hard Mix blending drawn (Plus, Normal) rather than
  `unimplemented!()`, and a keyframe span of nothing weighs 0. All 52
  read and draw (olo-images' `lottie`). Drop it once upstream reads them.
- **matroska-demuxer 0.8.1** (Zlib OR MIT OR Apache-2.0): seeking with
  cue points that carry a CueRelativePosition (as ffmpeg writes in WebM)
  read the position against the file's first cluster instead of the cue's
  own, so a seek past the first cluster landed in the wrong place and the
  Viewer showed no picture. The patch (`seek_broad_phase` in `src/lib.rs`)
  uses the cue's cluster. Its tests (and their 1 MB of sample files) are
  left out. It also skipped each block's additions, and with them WebM's
  alpha (a second VP8 or VP9 stream, BlockAddID 1): `next_frame` now
  goes into BlockAdditions and BlockMore and keeps that BlockAdditional
  as `Frame::additional` (Telegram's video stickers, olo-images' `webm`).
- **mp4 0.14.0** (MIT): a video's sample entry (`avc1`, `hev1`,
  `vp09`) was read as if its configuration box came first, so an H.264
  MP4 whose `avc1` has its `pasp` or `colr` before its `avcC`, as many
  encoders write it, wouldn't open at all ("avcc not found": Chromium's
  bear-1280x720.mp4 among them), and `vp09` took whatever came first for
  its `vpcC`. The patch (`find_child` in `src/mp4box/mod.rs`, used by
  `avc1.rs`, `hev1.rs` and `vp09.rs`, marked "Olo:") looks through the
  entry's boxes for it. Its sample-file doc example fails as published
  (the file isn't in the crate).
- **brush-interactive 0.4.0** (MIT): brush's line editor gives no way in
  for its hints or for the lines entered. The patch adds `src/hooks.rs`
  (`set_hooks`: a program's own hinter, given the shell's folder and last
  status, each line as it's entered, and each prompt as it shows) and
  uses them in `src/reedline/input_backend.rs`; for Olo's model's
  suggestions in `apps/brush` (HACKING, "brush's suggestions"), and for
  olo-brush's process to go where the shell is at each prompt (brush
  keeps its folder for what it runs, not as its own process's, so
  `/proc/PID/cwd` said where it started). Drop it if upstream takes
  hooks like these (and changes its own folder on `cd`).
- **nvenc-sys/** (MIT): NVIDIA's NVENC API (nvEncodeAPI.h, Video Codec
  SDK 12) as declarations only: nvidia-video-codec-sdk 0.4.0's bindgen
  output (types, the function table, GUIDs, structure versions) with
  its `extern` block and build script left out, so nothing links
  libnvidia-encode and Olo builds and starts without NVIDIA's driver.
  olo-video's `nvenc` opens the library at run time and calls it through
  its function table.
- **va-sys/** (MIT): libva 2.24's declarations (va.h, va_drm.h,
  va_drmcommon.h, va_enc_av1.h) for what olo-video's VA-API AV1 encoder
  uses, from bindgen, types only (how: its README): nothing links libva;
  olo-video's `vaapi` opens libva.so.2 and libva-drm.so.2 at run time.
- **bebook/**: the Be Book as Markdown, for reference (see its README).

# Vendored libraries

Owned here outright (path dependencies), not tracking a release.

- **deep_filter/** (MIT OR Apache-2.0): DeepFilterNet's libDF
  (github.com/Rikorose/DeepFilterNet at d375b2d), the noise suppression in
  `crates/voice`: spectral transforms, the tract runtime, and the
  DeepFilterNet3 model (`models/`, 8 MB, in LFS). Olo's changes:
  - tract 0.23.9, pinned exactly (`=`): 0.21.10 to 0.21.19 build and run
    but the model's output is wrong (noise passes, or speech is muted); 0.22
    and 0.23 are right and faster. The 0.23 port: `TypedSimpleState`,
    `to_plain_array_view*`, `try_as_plain_ram`, `with_outputs_by_name`;
    ndarray 0.17 to match.
  - The encoder's input buffers are owned tensors (copied into a `TValue`
    each frame), not written through a shared `Arc` behind tract's back.
  - `process_with_gains`: per-frequency gains on the input spectrum right
    after analysis (residual echo suppression; see `crates/voice`).
  - No silence skip: upstream output zeros after five frames under -70
    dBFS without feeding the STFT or the look-ahead, so the frames after
    each pause came out of stale buffers, cutting word onsets.
  - The low-latency model is left out (measured: when another voice was
    heard first, even 40 dB quieter, it turns the next one down 5 dB).
  - Trimmed to inference: no dataset, capi, wasm, binaries.
- **sonora/** (BSD-3-Clause): a pure-Rust port of WebRTC's audio
  processing (github.com/dignifiedquire/sonora at 373f1d7), for AGC2's
  level normalization in `crates/voice` (its AEC3 was measured and not
  used: linear below 8 kHz only, ~10 s to converge on speech, 9 ms of
  delay, and its suppressor chops the near voice in double talk). Olo's
  changes:
  - AGC2's adaptive digital gain may go below 0 dB
    (`AdaptiveDigital::max_attenuation_db`, default 0 as WebRTC): WebRTC
    only ever raises the level, leaving loud talkers to the analog input
    volume, which Olo doesn't touch.
  - Manifests trimmed (no examples, benches, dev-dependencies).
- **rust_h265 0.4.1** (MIT OR Apache-2.0): its SPS parser skips the
  VUI's colour description, and many HEVC files say they're HDR10 (PQ in
  BT.2020) only there: Chromium's `bear-1280x720-hevc-10bit-hdr10.mp4`
  has no `colr` box. The patch (`src/sps.rs`: `StreamColour`, kept as the
  VUI is read; `src/lib.rs`: `stream_colour`, for an SPS NAL unit) keeps
  it, for Olo's HDR video (`olo_images::hevc`). Its tests, benches and
  examples are left out. Drop it if upstream reads the VUI.
