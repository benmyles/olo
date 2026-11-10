These are rejected raster experiments, not the renderer shipped by this change.
`probe.rs` consumes the font diagnostics exported by page_dump (the prior
text-baselines/native-after-*.json.gz, decompressed) and uses Zeno 0.3.3 analytic
coverage. Run PROBE SCALE MODE; it reads input-SCALE.json and writes zeno-SCALE-MODE.png
in /var/tmp/codex-browser-glyphs (the scratch root is explicit in its source).
Modes vary glyph phase and outline snapping. Mode 4 also snaps outline/control
points to quarter-pixel Y and truncated 1/256-pixel X. This is not Skia's curve
subdivision algorithm, and its new curve outliers prevent a parity claim.

The two result files retain the comparisons against the prior checkpoint's
Chrome images. Aggregate error reductions did not yield exact glyph pixels.
No Zeno dependency or analytic rendering policy was added to olo-web.

Primary sources inspected on 2026-11-10:
- https://developer.chrome.com/blog/memory-safety-fonts?hl=en
- https://chromium.googlesource.com/chromium/src/+/refs/heads/main/content/common/features.cc
- https://skia.googlesource.com/skia/+/refs/heads/main/src/ports/SkTypeface_fontations.cpp
- https://skia.googlesource.com/skia/+/refs/heads/main/src/core/SkAnalyticEdge.cpp
- https://skia.googlesource.com/skia/+/refs/heads/main/src/core/SkScan_AAAPath.cpp

These moving source references are investigative leads, not proof of the exact
Skia revision used by the stored Chrome build. A future matching rasterizer must
pin that revision and independently compare curves and all render policies.
