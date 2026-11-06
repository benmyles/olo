These are independent **DOM rectangle** observations from Olo's Chrome, not
pixel references or website passes. The native engine does not call Chrome.

`generate.py` authors 139 normal-flow inputs: width/auto margins/overconstraints
in both directions, percentages, min/max and content/border sizing, adjoining
positive/negative margins, empty blocks, parent collapse barriers, definite/
indefinite percentage heights, relative offsets, display:none/contents,
block-in-inline splits, wrapping, fractional dimensions and border widths.
`base.css` intentionally supplies the display/reset/font rules explicitly;
this does not verify an HTML UA stylesheet. The font is the original fixed-
metric `../inline-fixtures/plain.ttf`; its provenance is there.

`probe.html` creates each document in a fixed-size iframe, waits for that
font, and saves getBoundingClientRect results. A separate fixed-position,
inset-zero ruler measures the actual initial containing block: at fractional
iframe sizes its viewport can differ from the iframe's CSS width because the
embedding viewport needs whole device pixels. This measurement is independent
of the in-flow boxes under test. The fixtures record both the requested size
and the observed layout viewport.

The reports were captured on the owned `browser-flow-1` Olo Agent Screen at
1×, 1.5× and 2×, stopping/restarting that screen between scales. No personal
browser/session was used. `SOURCE.json` pins the exact reports, inputs, probe,
generator and font and records the user agent. All 417 observations are tested
at one- and 128-action native worker budgets. Comparison projects native
geometry to Chrome's observed CSSOM export precision: float32 physical rect
coordinates multiplied by float32 reciprocal zoom, with 1e-9 tolerance after
that projection. Native calculations retain f64 values on a 1/64-device-pixel
layout grid. This checks box geometry, not antialiasing, glyph rasterization,
colors or native UI. In particular it does not validate arbitrary fonts,
fractional typographic shaping/line metrics, mixed typography, flex/grid/table,
replaced objects, clipping, scrolling, scripts or any website.

Regenerate inputs with `/usr/bin/python3 generate.py`. For an independent
capture, run `/usr/bin/python3 serve.py /var/tmp/YOUR_CAPTURE_DIR`, then launch
`olo-chrome` **only through `olo-screen run`** on an owned screen. Read the
screen's tree for the saved-case count; save all three reports, stop that
screen and the fixture server. Update the provenance hashes and review the
raw observations before changing expected values. No Olo output belongs in
the reference-generation path.
