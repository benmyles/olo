# Fractional-scale text fitting

Baseline `48a059b4`. Source work was integrated with upstream Application Kit
commit `65e122fa` through merge `5a3f25a4`; the browser binary is byte-identical
before/after that merge (`merge-binary.json`). Only the owned `codex-browser`
Agent Screen was used; no personal session install or reload.

## Diagnosis and correction

Stripe's offscreen Jobber author line wrapped at 1.75×. Its extra 25.2px enlarged
every testimonial card and the enclosing business section. A reduced natural-
width test reproduced 40 extra wraps among 555 case/scale observations. Tracing
showed apparent shortages as small as 2.84e-14px after padding arithmetic.
`fit-probe.log` and `fit-roundoff.log` retain the evidence.

Line fitting now tolerates machine roundoff and the one-device-layout-unit
allowance also observed in Chrome. Hanging-space, word-splitting and overflow
checks use the same fit limit. Chrome's
[AvailableWidthToFit implementation](https://chromium.googlesource.com/chromium/src/+/eb59c72731d60b8f7d2e13189246a7fd2bbe0e26/third_party/blink/renderer/core/layout/inline/line_breaker.h)
also adds its layout-unit epsilon.

Independent boundary tests then exposed 432 width discrepancies: mixed-style
measurement skipped centipixel effective-font-size quantization already used
by single-run intrinsic measurement. Both now share `LinearMetric`, including
f32 cache-bucket arithmetic. See Chromium's
[effective font size](https://chromium.googlesource.com/chromium/src/+/500947f508e484696c809b5ea84568bfd51b320a/third_party/blink/renderer/platform/fonts/font_description.cc)
and [font cache precision](https://chromium.googlesource.com/chromium/src/+/11023945ec0c56553bf2e39c5d422b1bcdcb6aa2/third_party/blink/renderer/platform/fonts/font_cache_key.h).
This changes web text measurement, not global kit hinting or font rasterization.
No new timer, background worker or idle work is added.

## Independent evidence

`fractional-fit-fixtures` records 1,350 Chrome observations:45 sizing/text/font
cases ×6 sequential width states ×5 actual live DPRs. Fresh/reused native
widths and heights match with the fixture's documented allowances. Every
one-unit shortage stays on one line in both engines; two/four units wrap.
The machine-epsilon-only intermediate run had 657 discrepancies; adding the
layout-unit allowance left 432 width discrepancies; sharing metric calculation
cleared them. `fit-shared-metric.log` is an intermediate compile error from a
duplicated module qualifier; `fit-shared-metric2.log` passes. The Chrome oracle
was never generated from native output.

Before/after each have 15 native and 15 focused Chrome screenshots plus geometry
and clean native reviews: light/dark/night ×1/1.25/1.5/1.75/2. Windows are at
odd x49, viewport 1000×578. Final captures assert matching cohorts, DPR, client
size and scroll (within 0.001px). Chrome is focused and two animation frames
complete before capture. Initial results/logs are retained separately: Chrome
scroll anchoring moved its viewport during the initial baseline matrix, so
those are not the final comparison. Live remote content remains nondeterministic.

At 1.75× the Jobber author is now 25.2px tall instead of 50.4px; Chrome 25.19643px.
Business-section height error falls 25.3555→0.1555px in all three themes at that
scale. Other scales still differ by up to 0.42px; line-height rounding and full
pixel matching are not solved. The first card's visible navigation strip moves
up as the shared carousel loses its extra height. The source author is offscreen
in these screenshots; its geometry is recorded explicitly.

## Remaining live failures

After scrolling through the full page and back, main height is 13331.68366px
native versus 13033.390625px Chrome at 1×:298.29304px difference. The next large
section differences are `.events-section-container` (1765.3 versus 1648.875px)
and `.footer-cta-section__grid` (560.375 versus 379.1875px). The section records
hash heading text instead of reproducing the site's copy. Recheck structure
and dynamic content before attributing each remaining difference to layout.

The structured "Show testimonial from Jobber" button activates in focused
Chrome, but native stays on Mindbody. Native's capture-phase click listener
receives the SVG target inside the correct button; `HTMLElement.click()` also
reaches it with BUTTON as target, yet still does not switch. This is an open
page-script/interaction failure, not a successful journey. The first attempted
Chrome click targeted an occluded window; the focused retry and resulting
states are retained separately. Next: trace native testimonial activation,
then the events/footer layouts. Stripe remains 0/30; corpus 0/100.

## Validation and costs

Release workspace build and tests pass before and after merging main. After
merge:2,220 passed,0 failed,136 ignored in 258 suites. Both 1280×800 and 1440×900
layout lints pass. Timing evidence is recorded alongside this file; shared-host
microbenchmarks do not establish speed superiority over Chrome.

Twelve release process runs: ABBA at 1.25× and 2×, then another ABBA at
1.25× after an isolated 176104 KiB candidate RSS sample. All samples are kept.
Pooled medians below exclude each process's first JavaScript timing batch.
Both binaries use `OLO_BROWSER_TIMING=1`; costs include diagnostic logging.

| Metric, before → after | 1.25× | 2× |
| --- | --- | --- |
| 9,000 cached geometry reads, ms | 6.014 → 5.828 | 5.765 → 5.816 |
| 20 font-size changes/layouts, ms | 3.695 → 3.807 | 3.666 → 3.783 |
| CPU per resize, ms | 10.000 → 11.250 | 19.375 → 16.250 |
| RSS after resizing, KiB | 154936 → 155908 | 163910 → 164184 |
| Drag first movement, ms | 36.90 → 36.85 | 35.10 → 36.75 |
| Wheel first movement, ms | 17.35 → 17.45 | 16.25 → 16.65 |

The baseline incorrectly wraps the 12px mutation at 1.25×; the candidate
keeps all twenty mutations' expected heights. Candidate 1.25× RSS samples
are 176104, 155108, 155888 and 155928 KiB; the outlier remains unexplained.
All idle samples report zero CPU and page frames, with existing watchdog
wakeups. Scroll hitches are zero in both; maximum recorded draw is
4.20 → 6.12 ms, with eleven → eight stationary wheel frames across six runs
per binary. Resize tool latency is about 410 ms including deliberate settling;
it is not input latency. The small mixed timing changes on this shared host
support neither a general speedup nor a resolved performance claim.

The first timing capture completed its JavaScript workload but its verbose
console record was truncated; the driver could not parse it. That failed
capture is retained. The final driver records compact line heights, validating
all twenty mutations per batch, and measures the same workload in both binaries.
`matrix.py`, `common.py` and `measure.py` retain their original scratch paths;
replay only with a newly owned screen/profile and updated paths.
