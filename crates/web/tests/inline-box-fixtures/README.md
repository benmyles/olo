These are **component geometry observations**, not pixel or website acceptance.

`generate.py` authors 45 horizontal inline-spacing inputs: positive/negative/
auto margins, horizontal and percentage padding, nested and empty boxes,
block interruptions, whitespace collapse, wrapping, ignored vertical margins
and fractional lengths. The explicit base stylesheet and original fixed-metric
font come from the existing flow/inline fixture sets.

The independent `probe.html` runs in Olo's Chrome on an owned Agent Screen.
It measures the actual iframe viewport with an out-of-flow ruler, every ASCII
letter's DOM Range and the element rectangles. Captures cover live scales
1, 1.25, 1.5, 1.75 and 2. `SOURCE.json` pins the exact generator, inputs, probe,
server, base, font, reports and non-pass ledger. No Olo output generates the
Chrome observations.

The native test runs every input at both one- and 128-action worker budgets.
It checks all line/block heights and letter Y coordinates. Letter X is compared
after CSSOM float32 projection, allowing one device layout unit (1/64 pixel)
because a DOM Range can enclose a caret outward from its glyph origin. This
does not make it a glyph-raster or pixel comparison.

**219/225 case/scale observations match that definition. Six are explicit
non-passes**, each with one X-coordinate mismatch at 1.25 or 1.75. Device-grid
font metric corrections resolve the other 21 cases. Transparent inline item
boundaries still need separate used advances. `known-nonpasses.json` pins each
wrong native coordinate and independent Chrome coordinate, with a 1e-9 numeric
comparison tolerance for JSON float round trips. This ledger is a regression
guard, not an acceptance oracle: a changed non-pass requires investigation,
and no additional X/Y/wrap errors in the same case are silently accepted.

Inline fragment rectangles, backgrounds/borders, bidi inline edges, arbitrary
fonts, mixed typography, complete CSS, scripts, native UI and websites remain
outside this component's acceptance scope. Element inline rectangles are
captured for later work but are not yet matched by the native test.

To recapture, run `/usr/bin/python3 generate.py`, then the local `serve.py`
with an owned output directory under `/var/tmp`. Launch `olo-chrome` only with
`olo-screen run` on an owned Agent Screen. Change that screen's scale live,
reload the probe and verify its saved count through the structured tree.
Stop Chrome and the fixture server afterward. Keep any new failures explicit;
do not substitute native output for independent observations.
