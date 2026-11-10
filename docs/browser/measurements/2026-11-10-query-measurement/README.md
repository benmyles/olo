# Container queries in script measurements and paint

Baseline: `adc5996b`; release binaries frozen with reflinks. Only the owned
`codex-browser` Agent Screen was used. Main was fetched and is already an
ancestor (444 browser commits ahead). No personal session install or reload.

The script measurement path passed no container sizes to style matching,
so every size query evaluated false. Painting had a separate correction loop,
but stopped after three rounds even when nested query results were unsettled.
Container name changes could also reuse descendants' old matched rules when
width stayed the same.

Both paths now settle container sizes and viewport-unit bases, retaining prior
layout/style work and invalidating descendants of changed/removed containers.
Changing containment or names rematches the affected composed subtree. A
bounded 64-round correction returns a layout resource-limit error if still
unsettled; each worker pass remains cancellable. No timer or idle work is added.
This bound is a resource policy, not a claim of unlimited CSS conformance.

The independent Chrome fixture has 50 observations (ten sequential mutation
states, five live DPRs), fourteen element rectangles/paddings per observation.
Fresh/reused script layouts and paint mutations pass at 0.02 CSS-pixel
rectangle tolerance. Baseline: 705 script discrepancies and an unsettled
six-level paint cascade (leaf height10 instead of37). Intermediate tests caught
ten additional discrepancies when a same-width container was renamed.
`query-after2.log` records a compile error during implementation; subsequent
runs pass. No expected native result was substituted into the Chrome oracle.
See `crates/web/tests/query-measure-fixtures` for the reproducible capture.

## Live Stripe evidence

Before/after each have 15 native screenshots, 15 focused Chrome screenshots,
geometry and clean native reviews: light/dark/night ×1/1.25/1.5/1.75/2. Windows
sit at odd x49; viewports match1000×578, clients985×578. Anonymous server cohort
labels match (recorded in `environment.json`); no cookie values are retained.
Chrome is focused and two animation frames finish before screenshots. Live
content, animation and remote assets are still not a deterministic replay.

The first feature card's script padding changes16→32px, matching Chrome at all
15 settings; its height changes89.6→112px, within0.047 CSS px of Chrome. The
baseline screenshot already wraps its first card over five lines: the earlier
~22px short-section diagnosis was script geometry, not proof of incorrect
painted section height. Candidate screenshots also show the previously obscured
second/third cards clearly; this live difference alone is not a causal pixel
regression test.

Business-section height still differs by25.36px at1.75×, and up to0.42px at
other scales. That is an explicit nonpass, not rounding acceptance. Native
line-height rounding, full-page clipping/blur, scripts and interactions need
further matched checks. Stripe0/30 and corpus0/100 remain unchanged. The earlier
~420px main-content measurement has not been revalidated by this focused probe.

`matrix.py`, `common.py`, `measure.py` are exact diagnostic drivers with the
original owned scratch path. Use a newly owned screen/profile and update paths
before replaying. The initial matrix inspected the illustration above these
cards; final retained matrices target the actual feature-card content.

## Validation and costs

Release workspace build and layout lint at1280×800/1440×900 pass. Release workspace tests:2,211 passed,0 failed,136 ignored in254 suites.
The full run took261.44s. Synthetic costs do not establish browser speed
versus Chrome.

Eight release runs in ABBA order at1.25× and2×; seven batches per process,
first batch excluded from pooled medians. Our workspace build/tests finished
before measurement; the shared host was not isolated from other workloads.
The fixture changes grid/root-container widths twenty times per batch, reads
leaf geometry, and checks the candidate against independent Chrome values.
The baseline skips queries and returns incorrect geometry. The first attempted
run had an incorrect hand-calculated assertion (248 rather than Chrome252);
its log and single completed baseline sample are retained separately.

| Metric | 1.25× before → after | 2× before → after |
| --- | --- | --- |
| 9,000 cached reads, ms | 5.782 → 6.052 | 6.002 → 6.102 |
| 20 forced query layouts, ms | 2.099 → 13.315 | 2.430 → 14.329 |
| CPU per resize, ms | 5.625 → 5.000 | 8.750 → 10.000 |
| RSS, KiB | 155274 → 153866 | 162086 → 162050 |
| Drag first movement, ms | 36.65 → 36.25 | 36.70 → 35.10 |
| Wheel first movement, ms | 17.35 → 17.45 | 15.30 → 17.60 |

Resize command-plus-idle latency is about410ms in both variants, including the
settling wait; it is not input-to-frame latency. All eight three-second idle
samples have0 measured CPU and0 page frames; existing script-watchdog wakeups
remain. Scroll checks have0 hitches in both variants; two stationary wheel
frames before and zero after, maximum draw5.30→1.74ms. These small samples do
not prove a speedup; cached-read/resize/wheel regressions and broader earlier
performance failures stay open. Correct nested forced layout necessarily does
more work than the baseline that skipped it, and further optimization remains.

Run `verify.py` to validate retained source identities, configuration records,
screenshots, query padding/height assertions and release check outcomes.
