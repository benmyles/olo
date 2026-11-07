# SVG overflow comparisons

18 original authored cases × five native Wayland DPRs (1/1.25/1.5/1.75/2):
90 CSS border rectangles match within 0.0001 CSSpx and 80 full 128×80 CSSpx
image crops match exactly. Ten crops remain explicit pixel nonpasses in
REFERENCE-GAPS.json: fractional edges and curves at each scale. The regression
test preserves these counts/details, and OLO_SVG_REQUIRE_COMPLETE=1 fails.
These component results are not website acceptance.

Cases cover visible/hidden/clip and independently clipped axes, ancestor clips,
inline roots, padding/borders, viewBox none/meet/slice and negative origins,
zero viewBoxes/widths, distant ink, fractional placement, curves and later
siblings. CSS geometry is checked separately from expanded ink query bounds.
An authored regression checks rasterizing a tile wholly beyond the CSS SVG box
and the original page viewport. The earlier SVG reference set remains unchanged
with its 28 pixel nonpasses.

Capture: start an owned Agent Screen and Olo Chrome with an isolated /var/tmp
profile and remote-debugging-port=0; eval the screen environment; run capture.py
with --screen, --profile and --out in owned scratch. Its ownership/readiness/DPR
checks use no device emulation or zoom; it restores screen settings and closes
and awaits Chrome. chrome.json pins the Chrome revision and source SHA-256;
all image hashes are checked. The local font is loaded from inline-fixtures.

Reference: https://www.w3.org/TR/SVG2/render.html#OverflowAndClipProperties
(section 3.11). Scrollable SVG viewports, full shape/paint/transform support and
exact fractional/curve rasterization remain incomplete.
