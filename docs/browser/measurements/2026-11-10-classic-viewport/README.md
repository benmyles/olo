# Classic viewport geometry and CSSOM sizes

This checkpoint implements the engine's classic root-gutter layout and its
JavaScript client/VisualViewport reads. **Browser still uses overlay metrics.**
Enabling classic metrics needs its horizontal scroll offset, reserved-area
painting and native bar controls together. Stripe's ~15px width difference
therefore remains, and corpus acceptance remains **0/100**.

`Host::scrollbar_widths` opts an embedder into classic space (default zero).
The root layout separates full viewport, containing block, client space and
viewport-unit basis. Automatic bars are resolved in at most three layout
passes; prepared text and intrinsic widths are retained. Passes and overflow
scanning share one work budget and honor cancellation. Raw document extents
are cached on Layout, with device snapping at the root CSSOM boundary.

Automatic bars narrow layout/client space while 100vw remains full width.
Forced root bars and stable root gutters narrow viewport units as well;
body-forced bars do not. An empty stable gutter narrows layout but remains
inside client size. Both-edges shifts the containing block's origin. Nominal
15/10 CSS px metrics become device-snapped gutters; the forced unit basis uses
nominal metrics. Viewport RTL direction follows the displayed body independently
of overflow ownership, and document overflow follows its negative inline side.

JavaScript root client sizes round to CSS integers; VisualViewport getters
return current fractional client dimensions. Repeated reads reuse measurement.
Initial innerWidth/Height now round consistently with live environment updates.
Window/media dimensions continue to use the full viewport. Existing body-scroll
aliasing, RTL scrolling, nested gutters, vertical writing, containment overflow,
gutter-only VisualViewport resize events and stale reads after the existing
60-layout/0.25s JS budget remain open.

## Evidence

- `crates/web/tests/scrollbars.rs` checks all **100 existing Chrome observations**
  from [the preceding checkpoint](../2026-11-10-viewport-overflow/chrome.json)
  against live and fresh measurements at five actual display scales. The
  independent data predates this implementation. Root/body/percentage/vw/fixed
  rectangles, client sizes and CSSOM document extents are checked.
- **200 live render/measurement transitions**, including removal of bars,
  style changes and resizing, agree. Platform metric changes without document
  edits also update both paths. Work-limit, cancellation and invalid-metric
  checks pass. Four additional native Chrome RTL observations and their driver
  are retained here; they test direction/extent, not the still-open scroll API.
- `crates/js/tests/scrollbars.rs` checks live metrics at five scales, forced and
  stable units, zero-width bars, window/media dimensions, receiver validation
  and 2,000 repeated client reads without another measurement.
- The first workspace run found an older overflow test comparing raw fractional
  extent with integer CSSOM oracle data. It now compares the rounded CSSOM
  dimensions; the independent fractional rectangle/client checks remain strict.
- Thirty native reviews (before/after, light/dark/night × live 1, 1.25, 1.5,
  1.75, 2 scales at odd x=49) are clean. **All 15 browser client captures are
  pixel-identical.** One 1.25× light screenshot differs only in app_server's
  outer window frame; its browser client is identical. Both screen-size lints
  are clean. This is a regression check, not Chrome page-pixel acceptance.
  No screenshot-journal entry is needed because no browser content changed.

## Costs and reproduction

`perf.html` measures 3,000 sets of three cached JS size reads and twenty forced
style/layout reads per turn (seven rounds, 100ms apart). It stays below the
existing layout budget and checks alternating size results. `measure.py` uses
frozen before/after release Browser binaries in ABBA order at 1.25× and 2×,
recording process CPU/RSS, idle, app_server counters and scroll latency.
These Browser measurements exercise the default overlay path; they do not
measure classic scrollbar controls, which have not been activated.

`engine-cost.rs` separately measures the shared engine with overlay/classic
metrics in ABBA order at all five scales: 1,000 style/layout changes after
20 warmups, measured and rendered paths, with geometry agreement asserted.
It links the release olo-web rlib with rustc -O and its dependency directory;
set CARGO_MANIFEST_DIR to crates/web for its fixture font. Its absolute helper
path names this checkout. This isolates gutter overhead in a small component;
it is not a real-site or whole-browser speed comparison.

Drivers use owned `codex-browser` and disk scratch `/var/tmp/codex-browser-classic`.
Recreate that directory and copy the fixtures and frozen binaries before a
rerun. No personal session installation/reload or delegation. Fetched origin/main
was already contained (440 commits ahead, none behind).


Final release workspace validation: **2,203 passed, zero failed, 136 existing
ignored; 252 suites**. One runner attempt was terminated by SIGTERM without an
assertion failure after the rounding correction; the isolated retry without
the optional kache test runner completed with exit 0. Full log and counts are
retained. Release workspace build also completed successfully.

Final Browser medians, first benchmark round excluded (ABBA, two launches of
each variant at each scale):

| Cost | 1.25× before → after | 2× before → after |
|---|---:|---:|
| 9,000 cached JS size reads, ms | 4.002 → 5.148 | 5.310 → 5.906 |
| 20 forced style/layout reads, ms | 1.654 → 1.920 | 2.309 → 2.286 |
| CPU per resize, ms | 11.875 → 11.875 | 21.250 → 24.375 |
| RSS, KiB | 167434 → 167662 | 169090 → 170008 |
| Drag first movement, ms | 28.40 → 28.50 | 27.10 → 28.60 |
| Wheel first movement, ms | 7.30 → 9.05 | 9.00 → 9.60 |

All sixteen scroll probes moved without a hitch; the maximum draw was 0.94ms.
Idle samples have zero CPU and zero page frames; 2–4 existing watchdog wakeups
per three seconds remain. Resize tool duration includes quiet-wait overhead,
not paint latency. Correct client/visual reads now enter the retained native
measurement path; these measurements show additional read cost. No CPU, memory,
latency or whole-browser speed improvement is claimed.

In the separate small engine fixture, overlay → classic median measurement
was **0.00968 → 0.10711ms**, render **0.09792 → 0.18912ms**.
This fixture changes root gutter policy every iteration, including forced-unit
restyling. Median p95 measurement was 0.01075 → 0.11435ms; p95 render
0.10239 → 0.20580ms. Cached 100,000 native size/geometry reads were
0.3912 → 0.3925ms. Extra layout/style passes have a measurable
cost; profile real pages when classic metrics are enabled. These numbers are
component diagnostics, not browser acceptance.

Run `/usr/bin/python3 verify.py` here to check the retained counts, reviews,
pixel comparisons, moving scroll probes and benchmark provenance.
