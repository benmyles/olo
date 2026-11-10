# Stripe implicit-grid image overflow

Stripe's 2464-pixel HTML image width bypassed percentage max-width compression
in the engine's intrinsic sizing. Its implicit grid track became 2464 pixels
wide inside a 919-pixel wrapper, making document scrollWidth 2497 instead of
985. Compression now also applies to definite preferred widths. Max-content
stays intact; cyclic min-width resolves against zero while retaining calc's
constant term. Broken images carry their identity in the box tree and retain
Chrome's different intrinsic behavior.

The horizontal scrollbar is gone at all 15 live light/dark/night × five-scale
configurations. Client height is now 578 rather than 562–563 CSS pixels at a
1000×578 full viewport. The wrapper, picture, image and overlapping content
match Chrome's x, width and height at every scale (919–920 pixels wide).
Their **vertical positions still differ by about 438 pixels**, and document
height differs. This closes a concrete overflow bug, not Stripe acceptance or
production readiness. Corpus acceptance remains **0/100**.

## Evidence

- `crates/web/tests/replaced-intrinsic-fixtures`: 48 loaded/broken image cases,
  240 independently captured Chrome observations at actual DPR 1, 1.25, 1.5,
  1.75 and 2. Four rectangles per case, width and height. Rust release tests
  compare the complete page pipeline. The initial loaded-only regression
  failed with 140 geometry differences (`test-before.log`). An expanded-test
  assertion initially failed on the expected missing-resource diagnostic for
  source-less images, not geometry (`test-expanded.log`, `test-detail.log`);
  the final test checks that diagnostic explicitly. See the fixture README
  for Chrome's cyclic calc behavior versus the current CSS Sizing draft.
- `native-unclipped.json` identifies the three actual overflowing elements;
  parent chains and matched Chrome CSS are retained beside it. This is not
  a clipping workaround or a site-specific selector in the engine.
- `stripe-before`, `stripe-after`, `stripe-after-card`: 45 captures and clean
  native-widget reviews at odd x49, changing scale and theme while running.
  `stripe-chrome` has 15 independent matched-viewport captures/geometry reads.
  Reviews cover native widgets, not web pixel correctness. The first card
  driver used a fixed 1.2-second wait and observed an old DPR while tests were
  running (`matrix-card-failed.log`); it now waits for the requested reported
  scale, with a bounded timeout. The complete retry passed unchanged code.
- Release workspace build and tests: **2,207 passed, zero failed, 136 existing
  ignored, 252 suites**. Browser layout lints pass at1280×800 and1440×900.
  Original logs and exit statuses are retained. Journal has before/after shots.
- `extra-constraints.json` records an additional Chrome check of min-height and
  decorated cyclic-width cases; it is diagnostic evidence, not added coverage
  in the 240-case Rust regression.

Run `/usr/bin/python3 docs/browser/measurements/2026-11-10-replaced-intrinsics/verify.py`
from the repository root to check the recorded evidence. GUI drivers use owned
`codex-browser` and `/var/tmp/codex-browser-overflow`. Freeze before commit
2a4d9d7d and the candidate release binaries with reflink copies; copy `perf.html`
and the probes into that scratch directory. `hero.js` is a read-only probe;
`live.js` positions the card for inspection. `stripe-check.py before|after|after-card`
expects a Browser launched with OLO_BROWSER_EVAL_LIVE pointing at the appropriate
probe. Chrome capture uses the owned profile and CDP port9338; replace the recorded
PID with the process you launched. No personal session was installed or reloaded.

## Performance

`measure.py` runs frozen release builds in ABBA order at1.25× and2×. Its fixture
has 12 overlapping image grids and forced classic bars on both builds. Seven
rounds measure 9,000 cached JS reads and 20 alternating forced width/layout
reads; the first round is excluded from medians. Each run also measures eight
resizes, three seconds idle, compositor timings and drag/wheel scroll-check
latency. Both variants' forced reads alternate sizes correctly. Resize tool
elapsed time includes about400ms of quiet-wait overhead.

The first comparison ran alongside screenshot-journal generation. It had two
candidate drag hitches and an8.92ms maximum draw (before: zero hitches,2.49ms),
with two stationary wheel frames in each variant. These samples remain in
`initial-costs.json`; they motivated a repeat without concurrent journal work.
Both sets are retained; neither is a whole-browser Chrome speed comparison.

Repeat medians (before → after), with builds/tests/journal generation finished:

| Cost | 1.25× | 2× |
|---|---:|---:|
| 9,000 cached JS reads, ms | 6.125 → 6.203 | 6.066 → 6.001 |
| 20 forced width/layout reads, ms | 1.500 → 1.622 | 1.565 → 1.583 |
| CPU per resize, ms | 18.125 → 17.500 | 45.625 → 42.500 |
| RSS, KiB | 156262 → 156532 | 164692 → 164402 |
| Drag first movement, ms | 36.75 → 36.80 | 35.45 → 36.80 |
| Wheel first movement, ms | 9.20 → 9.10 | 9.20 → 9.05 |

All16 probes moved; the repeat has **zero hitches in either variant**. Maximum
client draw was1.66ms before and5.33ms after, below8.3ms in this repeat. Each
variant still has two stationary wheel frames at2×; these remain an open
smoothness finding. All eight three-second idle samples report zero measured
CPU and zero page frames, with the existing watchdog wakes. No CPU-frequency
or machine isolation was used; these small samples do not establish general
latency, smoothness, memory improvement or Chrome performance superiority.

The earlier shared-kit benchmark failures and classic-layout/worker-round-trip
costs remain open in the preceding checkpoint. This patch changes engine
intrinsic sizing, not shared kit frame/input handling. No site, journey or
acceptance configuration has been marked complete. Fetched origin/main was
already contained (442 ahead,0 behind before this commit).
