These are **library geometry references**, not browser or pixel acceptance.

`generate.py` builds three original tiny TrueType fonts using fontTools
4.60.1. All source geometry and GSUB rules are here; no third-party font
artwork is embedded. The fonts share Olo's repository license. Fixed head
creation/modification times make regeneration byte reproducible. `plain`
measures literal advances, `ligature` substitutes across a space (so a longer
candidate can be narrower), and `deletion` removes a contextual leading glyph
(which must reappear when a boundary splits that context).

`reference.py`, run with `/usr/bin/python3`, uses the independent native
HarfBuzz 15.0.0 C API helper in `../shaping-fixtures/native_harfbuzz.py`. It
never invokes Olo. It reshapes every candidate of 216 normal-whitespace
single-script layouts, including candidates after an overflowing prefix, and
stores the selected ranges, advances and glyph IDs/positions/clusters in
`harfbuzz.json`. Font inputs are SHA-256 pinned there. The tests run each
reference with both one-candidate and 128-candidate worker slices. They check
geometry within 1e-9 CSS px (font-unit conversion arithmetic), and glyph
integers exactly. Unsafe-break metadata is deliberately excluded from output
equality: a shared original slice and an independently shaped slice can have
different boundary flags without different glyph geometry.

Regenerate fonts with an isolated fontTools 4.60.1 environment under your own
`/var/tmp` directory, then regenerate the oracle with `/usr/bin/python3`.
Noto input provenance and licenses are recorded in the shaping fixture set.
This oracle has no CSS box tree, rasterizer, Skia subpixel scale, Chromium
layout, mixed-script bidi oracle or website coverage. The separately authored
literal tests cover LF, space hanging, break-spaces, bidi/alignment, computed
style contexts, cancellation and quotas; they are not independent browser
references.
