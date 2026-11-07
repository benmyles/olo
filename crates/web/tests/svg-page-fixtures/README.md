# Initial SVG page comparisons

22 original cases, each captured at native Wayland DPR 1, 1.25, 1.5, 1.75 and 2
in an owned Olo Chrome Agent Screen. `chrome.json` records Chrome identity,
case-source SHA-256, rectangles and PNG hashes. `capture.py` restores the screen
settings and closes its fixture tab. It verifies profile/window ownership and
actual DPR; it does not emulate a device. Launch only `olo-chrome` through
`olo-screen run` with an owned `/var/tmp` profile and remote debugging port 0.
Run the driver in that screen's environment; see its module usage. The fixture
loads `../inline-fixtures/plain.ttf` as Olo Inline plain. The body width is fixed
at 128 CSS pixels so percentages do not depend on the Chrome window width.

All 110 rectangles match within 0.0001 CSS pixel. 82 complete image crops are
exact; **28 pixel nonpasses remain** in `REFERENCE-GAPS.json`. Curve rasterization,
fractional viewport/edge quantization, border alignment and middle-aligned
viewport snapping remain unresolved. The normal test pins these differences so
changes require investigation; a passing regression test is not conformance.
`OLO_SVG_REQUIRE_COMPLETE=1 cargo test --release -p olo-web --test svg_page`
fails until every comparison passes. `OLO_SVG_REPORT=/var/tmp/OWNED` optionally
writes native PNGs and a machine-readable report (create that directory first).

The initial capture with window-dependent body width was discarded. The fixed
90-case observation set was expanded to 110 observations for middle alignment;
all original 90 decoded images and rectangles were verified unchanged.
No website, interaction, universal dark mode or full SVG acceptance is implied.
