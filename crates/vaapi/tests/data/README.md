# Streams the VA-API decoder is checked against

From cros-codecs 0.0.6's test data (BSD-3-Clause, The ChromiumOS Authors),
each with the MD5 of every picture it shows, as NV12 (luma rows then Cb
and Cr interleaved, packed), as FFmpeg's `framehash` gives it:

- `av1-25fps.ivf`: cros-codecs' `test-25fps.ivf.av1`, 320 × 240 AV1, 250
  pictures.
- `vp9-25fps.ivf`: Chromium's `test-25fps.vp9`, 320 × 240 VP9, 250
  pictures.
- `vp9-show-existing.ivf`: libvpx's `vp90-2-10-show-existing-frame`,
  VP9 frames shown again (`show_existing_frame`) and hidden ones in
  superframes.
- `h264-25fps.h264`: Chromium's `test-25fps.h264`, 320 × 240 H.264
  (Annex B), 250 pictures.
- `h264-ipbp-high.h264`: cros-codecs' `64x64-I-P-B-P-high.h264`, H.264
  High with a B-frame.
- `hevc-test-25fps.h265`: Chromium's `test-25fps.hevc`, 320 × 240 HEVC,
  250 pictures.
- `hevc-64x64-I-P-B-P.h265`: HEVC with a B-frame.
- `hevc-bbb.h265`, `hevc-bear.h265`: short HEVC films (Big Buck Bunny,
  Chromium's bear), 60 and 30 pictures.

H.264 and HEVC need a driver with them (Fedora's iHD has neither:
`LIBVA_DRIVERS_PATH=DIR` points the test at RPM Fusion's
`intel-media-driver` unpacked there); without one their tests say so
and pass.
