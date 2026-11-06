# Native image pixels

`generate.py` creates original RGBA inputs, PNG data URLs and authored HTML/CSS.
`capture.py` captures independent Chrome pixels on an owned Agent Screen, using
actual Wayland output scales 1/1.25/1.5/1.75/2. It verifies screen/Chrome ownership,
decoded image readiness, native DPR and frame dimensions. No device emulation.
`capture.json` retains browser/GPU information; `SOURCE.json` pins all inputs
and captures. `image_reference` exports the engine's corresponding pixels.

Current result: 49 of 60 captures match exactly. `nonpasses.json` records the
11 remaining failures, which the test pins separately rather than accepting as
rendering passes. Nine involve minification (up to 64 channel levels); two
fractional opaque cases differ by one channel level at one pixel. Fixing these
requires changing their explicit classification. No tolerance is applied to the
49 matching captures. This Intel/Mesa/ANGLE reference is not a universal GPU
rounding guarantee and establishes no website acceptance.

Sampling uses snapped device edges and 1/256 filter fractions, with normalized
binary16 texel values looked up from compact premultiplied RGBA8. This reproduces
the observed reference for the matching cases; minification and some GPU
interpolation rounding remain open. CPU tile boundaries never change sampling
coordinates. Scalar and packed SIMD ingestion must produce identical bytes.
Explicit four-channel vector interpolation was measured slower than the
compiler-selected arithmetic and rejected; the initial cost artifact is retained.
