These are component geometry observations, not image or website acceptance.

The 76 authored inputs cover atomic objects between letters, adjacent objects,
LTR/RTL, wrapping and nowrap, fractional and zero sizes, positive and negative
margins, mixed heights, forced breaks and struts with line-height 0/10/20/40.
Chrome loads embedded red PNGs. The Rust component receives their used metrics
explicitly: image fetching, decoding, CSS replaced sizing and painting remain
outside this test.

`probe.html` independently measures DOM Range letter rectangles, image boxes
and paragraph heights in Olo Chrome on an owned Agent Screen. All 380 case/scale
observations match at scales 1, 1.25, 1.5, 1.75 and 2, using both one- and
128-action worker budgets. The parent computed style and font request pass
through the native CSS factories. `SOURCE.json` pins all inputs, reports, font
and base stylesheet. No native output generated the Chrome observations.

Image X/Y, paragraph height and letter Y allow 1e-5 CSS pixels after CSSOM float
projection. Letter X permits one device layout unit (1/64 pixel), accounting
for outward DOM Range enclosure. The iframe viewport is recovered on the
physical-pixel grid to avoid projecting its float32 export twice. These are
geometry checks, not glyph-mask or pixel comparisons. Arbitrary fonts, inline
alignment other than baseline, complete CSS/bidi and websites remain unverified.

To recapture, run `/usr/bin/python3 serve.py /var/tmp/OWNED_OUTPUT`, launch
`olo-chrome` through `olo-screen run` on an owned Agent Screen, and change its
scale live. Reload the probe at each scale and check the saved count through
the structured tree. Copy the independent captures, investigate differences,
then update provenance. Stop the screen and server afterward.
