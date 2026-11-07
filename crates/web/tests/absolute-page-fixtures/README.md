# Absolute page comparisons

25 original HTML/CSS cases captured in owned Olo Chrome at native Wayland DPR
1/1.25/1.5/1.75/2: 125 pages and 545 border rectangles. All four coordinates
match within 0.0001 CSSpx. Cases cover positioned padding containing blocks,
auto-height parents, percentage insets/sizes, border-box sizing, auto margins,
LTR/RTL overconstraints, min/max, nested absolute descendants, static
intermediate ancestors, viewport fallback and initial flex exclusion/reflow.
This is geometry coverage, not independent pixels or website acceptance.

`capture.py` uses an owned Agent Screen and isolated /var/tmp Chrome profile.
See its docstring. It verifies process/profile/window ownership, actual DPR,
fixture readiness and loaded local font, restores settings, closes and awaits
Chrome. `chrome.json` records browser identity and cases SHA-256. Rust verifies
source identity and every rectangle, with no ignored geometry failures.

The CSS viewport is reconstructed from visualViewport size times verified native
DPR, rounded to device pixels and divided by DPR. Raw visualViewport values are
retained as reported_viewport. Integer innerWidth/innerHeight loses fractional
CSSpx at native scales; raw visualViewport f32 also loses one layout quantum
when directly supplied to the engine at 1.75. Two earlier captures using those
inputs were disqualified; no rectangle tolerance was widened. Only the final
125 pages count as coverage. Device emulation and page zoom are not used.

Static-position discovery, shrink-to-fit width, replaced positioning and an
absolute interruption inside a pending inline run remain unsupported. Numeric
axis cases in ../positioned-fixtures cover more equations separately.

Algorithm reference: https://www.w3.org/TR/CSS22/visudet.html sections 10.1,
10.3.7, 10.4, 10.6.4 and 10.7. This is not full modern positioned layout.
