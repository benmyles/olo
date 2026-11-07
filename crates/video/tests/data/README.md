# Streams every video decoder is checked against

Each with the reference decoder's MD5 of every picture it shows, as the
reference decoders give them (`vpxdec --md5 --i420`, `aomdec --md5`):
I420 planes, packed, a deep picture's samples 16-bit little-endian.
`tests/reference.rs` decodes each on every device here (the processor,
Intel's GPU through VA-API, NVIDIA's through NVDEC).

- `vp9-10bit.ivf`: libvpx's `vp92-2-20-10bit-yuv420.webm` (BSD-3-Clause,
  the WebM Project), VP9 profile 2, 10-bit 4:2:0, 160 × 90, 10 pictures;
  its frames moved from WebM into IVF, unchanged.
- `vp9-tiles.ivf`: libvpx's `vp90-2-08-tile-4x4.webm`, VP9 1920 × 1080 in
  4 × 4 tiles, 5 pictures; likewise moved into IVF.
- `av1-10bit.ivf`: libaom's `av1-1-b10-00-quantizer-00.ivf` (BSD-2-Clause,
  the Alliance for Open Media), AV1 10-bit 4:2:0, 640 × 360, 2 pictures.
- `hevc-10bit.h265`: the HEVC conformance suite's
  `WPP_A_ericsson_MAIN10_2.bit` (ITU-T / ISO/IEC JCT-VC), Main 10, 416 ×
  240 in wavefronts, 48 pictures; its MD5s each picture's of the
  suite's reference YUV (whose own MD5 is the suite's).

From `storage.googleapis.com/downloads.webmproject.org/test_data/libvpx`
and `storage.googleapis.com/aom-test-data`, with their `.md5` files, and
the ITU's JCT-VC site (`bitstream_exchange/draft_conformance/HEVC_v1`).
