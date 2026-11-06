# Native image pixels

`generate.py` creates original RGBA inputs, PNG data URLs and authored HTML/CSS.
`capture.py` captures independent Chrome pixels on an owned Agent Screen, using
actual Wayland output scales 1/1.25/1.5/1.75/2. It verifies screen/Chrome ownership,
decoded image readiness, native DPR and frame dimensions. No device emulation.
`capture.json` retains browser/GPU information; `SOURCE.json` pins all inputs
and captures. `image_reference` exports the engine's corresponding pixels.

Current result: 53 of 60 captures match exactly. `nonpasses.json` records the
seven remaining failures, which the test pins separately rather than accepting
as rendering passes. Five involve minification (at most two channel levels);
two fractional opaque cases differ by one channel level at one pixel. The old
49/60 bilinear result and its eleven failures (up to 64 levels) remain in
`nonpasses-bilinear.json` and the provenance. Fixing these
requires changing their explicit classification. No tolerance is applied to the
53 matching captures. This Intel/Mesa/ANGLE reference is not a universal GPU
rounding guarantee and establishes no website acceptance.

Sampling uses snapped device edges and 1/256 filter fractions, with normalized
binary16 texel values looked up from compact premultiplied RGBA8. Minification
uses bounded, demand-created source mipmaps and a -0.5 sharpening LOD bias,
as documented in the pinned Skia sources. Precise GPU LOD/interpolation
rounding and broader odd/anisotropic source cases remain open. CPU tile
boundaries never change sampling coordinates. Scalar and packed SIMD ingestion
and mip reduction must produce identical bytes.
Explicit four-channel vector interpolation was measured slower than the
compiler-selected arithmetic and rejected; the initial cost artifact is retained.
