# Grid track distribution reference

`tracks.html` contains 72 authored grids: columns and rows; thirds and
sevenths; unequal, sub-unit and zero flexible factors; fixed tracks among
flexible tracks; minmax constraints and frozen minima; stretched auto tracks;
fixed fractional tracks; and extreme finite flexible factors. Three sizes/gaps exercise exact and fractional
layout units. Containers overlap deliberately so each rectangle stays near
the origin; screenshots of this fixture are not visual acceptance evidence.

`chrome.json` records 1,440 independent border rectangles across 360 case/scale
observations in Chrome, with browser revision and the HTML's SHA-256.
Capture uses an owned Olo Agent Screen, actual Wayland output scale changes
at 1/1.25/1.5/1.75/2, no density emulation, verified devicePixelRatio, font
readiness and two animation frames. A repeated capture is byte-identical.
The test allows 0.0001 CSS px for Chrome's float DOMRect serialization;
a missing layout unit is larger than that allowance at every tested scale.

Flexible tracks carry discarded fractions to later flexible tracks, using
float layout-unit shares as the independent results require. In particular,
three 1fr tracks in 100 CSS px at native 1× remain slightly short of 100;
this is not a universal 'fill the final track' rule. Automatic stretching
has distinct rounding behavior. Fixed tracks and frozen minima do not donate
fractions. The grid quantizer also preserves an already-quantized size after
fractional-scale division or cancellation in a subtraction.

This covers track distribution, not complete CSS Grid: subgrid, writing
modes, intrinsic contributions, alignment, spans, placement and containment
need their own independent coverage. No website acceptance follows.

Algorithm investigation consulted the [Chromium track sizing implementation](https://chromium.googlesource.com/chromium/src/+/refs/heads/main/third_party/blink/renderer/core/layout/grid/grid_track_sizing_algorithm.cc)
and [CSS Grid flexible tracks](https://drafts.csswg.org/css-grid-2/#algo-flex-tracks).
Expected values come from the running Chrome, not from Olo's implementation.
