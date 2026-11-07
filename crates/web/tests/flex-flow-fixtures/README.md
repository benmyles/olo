# Original integrated flex page references

52 authored pages × five verified native Wayland DPRs (1,1.25,1.5,1.75,2)
produce260 independent Chrome rectangle observations. `cases.json` is input;
`chrome.json` is unmodified reference output. Native tests run the production
HTML parser, stylesheet/cascade/computation, box builder, text preparation and
page formatter; they compare every named rectangle within0.0001 CSS px and
repeat with a different job budget and the published intrinsic cache.

These cover horizontal single-line LTR rows/reversed rows, empty/anonymous text
items, `display:contents`, preserved indentation, automatic and explicit numeric/
percentage/content bases, automatic text minima, width caps, decorations, gaps,
positive/negative/auto margins, initial alignment keywords, relative offsets,
cross-size constraints, auto-height stretch and percentage descendants, including
nested flex rows whose main sizes do not need nested intrinsic contributions.
The older64-case/320-line reference is also run through this complete production
path, comparing1,095 border widths and positions from the original inputs.

The tiny original TrueType font is `../inline-fixtures/plain.ttf` (generation
source/license documented there). The capture embeds those exact bytes and
checks loading and character coverage. An initial capture mistakenly used
characters outside that font, causing Chrome system fallback: it is disqualified,
not counted as an engine comparison. Its raw input/reference and140 mismatches
remain in owned disk scratch, with hashes in the measurement record. The corrected
36-case capture occurred after the initial implementation, before rounding fixes;
16 additional cases were then captured before their native validation. Do not
claim that all references predate implementation. The corrected180 cases exposed
two alignment differences; the expanded260 exposed18, all fixed without changing
reference observations or tolerance.

Use `/usr/bin/python3 capture.py --screen NAME --profile
/var/tmp/OWNED/profile --out /var/tmp/OWNED/chrome.json` after evaluating
`olo-screen env NAME` and starting only `olo-screen run NAME olo-chrome
--user-data-dir=/var/tmp/OWNED/profile --remote-debugging-port=0 about:blank`.
The driver asserts owned profile/PID/window, loaded authored URL/probe and native
DPRs, clears emulation, restores settings and closes/awaits its own Chrome.
Stop the owned screen afterward. No personal browser/profile is used.

This is component geometry, not website pixels or acceptance. Column/RTL flow,
wrapping, order, baseline/compound/safe alignment, replaced items, negative inner
border-box bases, nested/container-contained intrinsic contributions and complex
text intrinsic contexts remain explicit gaps. Existing ordinary text limitations
also apply. The initial border shorthand expands the registered width/style/
color longhands; complete border-image resets remain unimplemented.

Primary algorithm reference read2026-11-06:
[CSS Flexible Box Layout](https://drafts.csswg.org/css-flexbox-1/#layout-algorithm).
