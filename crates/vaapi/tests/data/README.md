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
