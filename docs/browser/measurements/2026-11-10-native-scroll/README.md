# Native classic viewport scrolling

Browser now reserves classic root scrollbar space and uses the shared kit's
native horizontal/vertical controls. Scroll offsets reach script APIs, retained
tiles, fixed layers, hit testing, pointer coordinates and persisted history.
RTL horizontal offsets extend left from zero. Script requests clamp and snap to
the device grid; native position updates do not echo requests. Scroll events
coalesce. Root wheel defaults follow overflow policy and page cancellation.

This does **not** complete a site or make a production-quality browser.
Acceptance remains **0/100**. Stripe's hero now follows client width at all
15 live theme/scale configurations. A fresh Chrome 157 comparison at the same
1000×578 viewport gives the same 985×542.03125 CSS-pixel hero rectangle at 1×.
Olo still incorrectly has a horizontal scrollbar there (client height 563,
Chrome 578). Investigate document overflow next; no configuration is accepted.

## Correctness and visual evidence

- `oracle.json`: 100 independent Chrome scroll observations, LTR/RTL at five
  actual device scales. Negative half-pixel ties round towards positive infinity.
  `oracle-scrollby.json` adds nonfinite, omitted and dictionary arguments.
  `crates/js/tests/scrollbars.rs` checks clamping/snapping, options, event
  coalescing, native updates, fixed/transformed containing blocks, coordinates
  and wheel cancellation. Chrome float32 roundoff in fractional exposed values
  remains a known difference; tests do not claim exact binary equality.
- `before/`: 15 captures/reviews. `after/`: 60 captures/reviews (origin, scrolled,
  RTL and thin bars), light/dark/night × live 1, 1.25, 1.5, 1.75, 2 scales, odd x49.
  All widget reviews are clean. Fixtures also verify hidden bars, blocked wheel
  motion, allowed touchpad motion and structured fixed controls after scrolling.
- `controls/`: five scale checks of native arrow buttons, horizontal thumb drag
  and RTL arrow motion. `nested/`: five scale checks that wheel cancellation
  keeps a child container stationary and allowed three-notch gestures move it
  approximately 144 CSS pixels. These do not establish nested-scroll smoothness.
- `reload.json`: position 640,720 survives Back, page reload and an owned full
  tsession reload after a workspace build. Driver takes its Browser PID as an
  argument; start `browser-scroll-reload --session` with the fixture first.
- `stripe/`: 15 live environment/canvas observations, captures and clean reviews.
  These are diagnostic captures, not matched journey/configuration acceptance.
  `stripe-chrome-check.json` records the additional matched-size Chrome geometry.
- Screenshot journal has before/after native viewport captures. No personal
  session was installed into or reloaded; all GUI work used owned screens.

## Wheel regression and measurements

A worker decision cannot block the UI. Applying a wheel default before asking
JavaScript ignored preventDefault, but simply deferring it lost GPUI's eased
remainder: the frame loop interpreted an unchanged frame as the scroll edge.
The first release ABBA probe exposed that regression (about 16–17 pixels from
three notches; ~250ms first movement). `wheel-regression-costs.json` retains
those failed samples; they are not final performance evidence.

`Window::defer_wheel()` now returns an owned decision token for eased wheel
steps. While pending, no further step is consumed. Applying the accepted motion
and completing it resumes after its draw; dropping/rejecting it cancels the
remainder. Touchpad input has no such token. Stop also rejects late default
replies for input queued before it: the slow-handler regression moved 8 pixels
after Stop even though the token had been dropped. `stop-before-exact.json`
and `stop-after.json` retain the frame-mark proof. This adds no idle timer. Browser
also avoids redraws for wheel acknowledgments and unchanged cached flat-page
replies; metadata, changed labels, tiles, status and layers still trigger updates.

`measure.py` compares frozen 97c0a043 and final release binaries in ABBA order
at 1.25×/2×. `perf.html` checks seven separate rounds of 9,000 cached JS reads
and 20 alternating forced layouts, below the existing layout budget. It records
process CPU/RSS, compositor timings, three-second idle samples and scroll-check
latency/smoothness. Resize-tool time includes quiet-wait overhead, not paint
latency. Additional classic layout passes and worker wheel decisions have a
cost; no whole-browser speedup is claimed. See final costs and validation below.

Drivers use owned `codex-browser` and scratch `/var/tmp/codex-browser-native-scroll`.
Recreate that directory and fixtures, and freeze before/after release binaries
with reflink copies to reproduce. `kit-bench.py` is the shared benchmark with
an owned scratch/session name. Original transient failure logs are retained.

Open: erroneous Stripe horizontal overflow; nested classic gutters and complete
nested/iframe wheel behavior; keyboard defaults; smooth scrolling/scrollend;
vertical writing; transformed/contained overflow; gradients/images in empty
stable gutters; body scroll aliases; float32 scroll exposure; gutter-only
VisualViewport resize events; stale reads after 60 layouts/0.25s in one advance.

## Final release results

Workspace tests: **2,206 passed, zero failed, 136 existing ignored, 252 suites**.
An initial run had a token-pool daemon-discovery assertion return None; its
unchanged 24-test target and then the complete workspace passed on retry.
Both logs remain. After the final Stop guard, Browser's 19 tests passed again
(one existing ignored), and the workspace release build passed. Both screen-size
lints passed. The full workspace binary was also run through all 60 fixture
captures; the later Stop guard changes no geometry. No test was bypassed.

Medians, first JS benchmark round excluded, ABBA at both scales:

| Cost | 1.25× before → after | 2× before → after |
|---|---:|---:|
| 9,000 cached JS reads, ms | 5.301 → 5.976 | 5.110 → 5.716 |
| 20 forced style/layout reads, ms | 1.949 → 9.413 | 1.907 → 9.464 |
| CPU per resize, ms | 13.125 → 13.125 | 20.625 → 23.125 |
| RSS, KiB | 160070 → 159970 | 168022 → 169352 |
| Drag first movement, ms | 29.05 → 37.35 | 25.60 → 33.60 |
| Wheel first movement, ms | 9.15 → 17.25 | 9.10 → 17.35 |

All sixteen probes moved. Wheel distance recovered to about 130–134 pixels
from the regression's 16–17 (the old implementation also measured about
130–133). Before has two drag hitches; after has one. After has nine stationary
wheel frames across four probes; before has zero. Slowest after draw is 0.84ms,
before 6.41ms. These samples do not establish smoothness or latency acceptance;
profile the remaining stationary frames and avoid unnecessary worker round trips
where cancellation cannot apply. No CPU-frequency or whole-machine isolation
was used. Do not attribute isolated hitches to a cause without a trace.

Every three-second idle sample reports zero CPU and zero page frames, with
existing watchdog wakeups. Resize-tool duration is about 410ms, mostly quiet-wait
overhead. Classic forced layouts do additional gutter/style passes and are
about five times slower in this fixture. The cost needs optimization; no
whole-browser efficiency or Chrome-performance claim is made.

The shared-kit gallery/StyledEdit benchmark at 1×/1.5×/2× ran **51 steps,
3,571 frames, six failing steps**. Typing reached13.92ms and gallery scrolling
10.04ms; scroll hitches and glide deviations also failed. `kit-bench.json` and
`kit-bench-final.log` retain every result. This is a failing check, not a pass
and not proof of a patch-caused regression without a matched kit baseline.
Gallery/editor idle notes also include frames (their fixtures have changing
controls/carets); the zero-idle claim above applies only to the Browser fixture.
