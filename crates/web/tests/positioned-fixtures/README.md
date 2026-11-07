# Absolute axis comparisons

45 original cases captured in owned Olo Chrome at native Wayland DPR
1/1.25/1.5/1.75/2:225 observations of border position, content extent and both
used margins. All currently match within0.0001CSSpx. The cases cover all eight
auto inset/size combinations, horizontal LTR/RTL and vertical layout,
auto/negative margins, min/max constraints and fractional values. Intrinsic
widths come from two authored inline-block children separated by wbr; auto
height comes from a fixed-height block. Static positions in Chrome fixtures
are zero; a separate authored test checks a nonzero static end with different
containing-block direction. This is not independent static-position discovery
coverage, containing-block selection, page pipeline, paint or website acceptance.

`capture.py` uses an owned Agent Screen, Olo Chrome and an isolated /var/tmp
profile. See its docstring. It verifies process/profile/window ownership,
actual native DPR and fixture readiness, restores settings and closes/awaits
Chrome. `chrome.json` records browser identity and cases SHA-256. The Rust test
checks source identity and every measured component, with no ignored gaps.

Algorithm reference: https://www.w3.org/TR/CSS22/visudet.html sections10.3.7,
10.4,10.6.4 and10.7. These equations do not cover modern alignment/anchor
positioning, replaced intrinsic ratios or general writing modes.
