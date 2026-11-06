# Original definite single-line flex geometry

`cases.json` is original authored numeric input. `chrome.json` is independent,
unmodified geometry from Chrome157.0.8103.38, Chromium
a733b5afbb3ba618844ff8876b89ad1a774b2f18, captured before the native solver was
written.64 cases ×5 actual Wayland DPRs (1,1.25,1.5,1.75,2) give320 flex lines.
Cases cover unequal/partial/zero growth, weighted/partial shrinking, coupled
min/max freezing, conflicting min/max, zero bases/available space, negative
non-auto margins, padding/borders and fractional sizes/gaps.48 cases use a fixed
original random seed; the other16 are explicit examples. These are not WPT ports.

The original empty child divs use content-box sizing, definite numeric bases and
constraints, explicit row/nowrap/start alignment, margins/padding/borders and
column gap. Every native input comes from the authored numbers, with ordinary
used-length and border snapping. Native comparisons do not read Chrome's
computed widths as inputs. Bounding content widths subtract recorded computed
padding/border widths; expected rectangles and computed strings are unchanged.
Tolerance0.0001 CSS px handles float32 exposure, below a layout quantum. CSS
parsing, intrinsic/auto/percentage bases, item construction, wrapping, alignment,
border-box negative inner bases and website acceptance are outside this test.

Capture using `/usr/bin/python3 capture.py --screen NAME --profile
/var/tmp/OWNED/profile --out /var/tmp/OWNED/chrome.json` after evaluating
`olo-screen env NAME`. Start only `olo-screen run NAME olo-chrome
--user-data-dir=/var/tmp/OWNED/profile --remote-debugging-port=0 about:blank`.
The driver verifies window/PID/profile, loaded authored document and native
DPRs, clears emulation, restores settings, closes Chrome and awaits process exit.
Stop the owned screen afterward; no personal browser/session is involved.

The original Rust stage consumes used content bases/min/max, non-content extents
and grow/shrink factors, and distributes on a1/64-device-pixel grid. Length inputs
must already have their CSS used-value interpretation. It reconstructs checked
integer quanta, normalizes finite factors to float32, retains cumulative weights
in item order, and distributes from the end with rounding and remaining-space
updates. Constraint violations freeze only the appropriate items; scalar sums
are rebuilt after each freeze. Rounding across the entire set, rather than
rounding independent equal shares, is observable in the three-item cases.

Each worker budget visit handles at most one item, with work/size quotas,
cancellation and poisoned publication. Numeric length accumulators are bounded
to2^52 quanta for exact subtraction; default maximum size may be infinite. Input
bases/minima must be nonnegative. Only final sizes survive publication; the
input and prefix/status scratch is discarded. The stage is not called by page
layout yet and is not complete Flexbox support.

Primary references, read2026-11-06:
- [CSS Flexbox resolving lengths](https://drafts.csswg.org/css-flexbox-1/#resolve-flexible-lengths)
- [Reference rounding/freeze behavior](https://chromium.googlesource.com/chromium/src/+/a733b5afbb3ba618844ff8876b89ad1a774b2f18/third_party/blink/renderer/core/layout/flex/line_flexer.cc)
- [Reference layout-grid conversion](https://chromium.googlesource.com/chromium/src/+/a733b5afbb3ba618844ff8876b89ad1a774b2f18/third_party/blink/renderer/platform/geometry/layout_unit.h)
