# Viewport overflow ownership

This checkpoint fixes clipping and page-overflow measurement while preparing
classic scrollbar layout. **It does not fix Stripe's remaining 15-pixel width
difference or accept a website.** Corpus acceptance remains 0/100.

The box tree retains the root/first HTML body identities and resolves the
current overflow source from its current styles. Paint and measurement use
that same source. The selected element no longer clips the page to its own
box, while a body that is an independent scroll container clips and scrolls
normally in the scene. Page overflow is tracked per axis, including empty
boxes that paint nothing. No document walk is added per frame; source lookup
is constant-time and no idle timer is introduced.

`scrollbar-width` and `scrollbar-gutter` now have typed declarations, CSS-wide
values and canonical computed serialization. Width/gutter come from the root,
not the body, and are not inherited. Their space allocation/painting is still
open. The new policy API does not by itself enforce viewport wheel/scroll
restrictions; those and DOM body's existing viewport-scroll alias remain open.

## Reference and regression checks

- Twelve Chrome overflow observations and all 25 axis combinations are pinned
  under `crates/web/tests/overflow-fixtures`. Native tests cover each case at
  1×, 1.25×, 1.5×, 1.75×, 2×, including live/fresh layout agreement and four
  solid-color pixels per case. Root/body ownership also follows display:none
  and subsequent restoration. The reference for these twelve cases is 1×.
- Chrome 157 preserves clip alongside a scrollable axis; the current spec does
  too. An attempted clip→hidden change failed the independent oracle and was
  removed. The first parser attempt also failed because the property registry
  must remain alphabetically sorted; fixed before the passing suite.
- Release workspace build and all 250 test suites passed: **2,197 passed,
  zero failed, 136 existing ignored**. Native review: 15 before and 15 after
  (light/dark/night × five live scales), odd x=49, zero findings. Both
  1280×800 and 1440×900 window lints have zero errors.
- `visual.html` shows the concrete regression: the body is 80px high and its
  overflow propagates. Before, nearly everything below the heading was missing;
  after, the content remains visible, as in Chrome. Full captures and contact
  sheets are retained for all three variants. The page still reports 80
  existing fallbacks and differs from Chrome in gutter space, typography and
  row sizing; these are clipping checks, not whole-page pixel acceptance.

## Scrollbar geometry investigation, still to implement

`probe.html`, `oracle.py` and the top-level `chrome.json` contain **100 actual
Chrome observations**, twenty cases at five live native display scales.
`innerWidth`/media queries use the full viewport. Automatic classic scrollbars
reduce document/client/visual viewport space but do not reduce 100vw. Forced
root scrollbars and stable root gutters also reduce viewport-unit sizing;
body-forced scrolling does not. A stable empty gutter reduces layout while
root client/VisualViewport measurements still report full space. Both-edges
adds the opposite inline gutter. Root scrollbar-width does not inherit or
propagate from body. The standard gutter is round(15×DPR)/DPR CSS px, thin is
round(10×DPR)/DPR; forced viewport units in this Chrome use integer-CSS-pixel
gutters. Simply subtracting 15 from the Browser's existing viewport key would
break these distinctions. Horizontal scrolling and nested classic gutters
also need implementation.

## Costs

The first benchmark was invalid: seven batches of 100 synchronous style/layout
reads exhausted the existing 60-layout/0.25s per-advance budget, after which JS
read stale measurements. `perf-invalid-budget.html` and
`costs-invalid-budget.json` retain that failed experiment. It was also run
while the workspace suite was active. Do not use those timings as a comparison.
The corrected fixture uses twenty changes per turn, separated by 100ms, and
asserts alternating measured sizes after the fix. The final run starts after
the workspace suite and lint complete. The existing budget's stale-read
behavior remains a web-compatibility defect.

The benchmark and raw timing results follow in `costs.json` and
`cost-summary.json`. Each variant is a frozen release binary, ordered ABBA at
1.25× and 2×; byte hashes are in the raw data. Cached reads, forced layout,
whole-process resize CPU/RSS, idle wakeups, app_server timing counters and
scroll first-movement/draw latency are recorded. Resize tool duration includes
its quiet wait and must not be called paint latency. This component work makes
no whole-browser speedup claim.

No personal desktop install/reload and no delegation. Fetched origin/main was
already contained. Driver scratch paths intentionally name the owned disk
scratch directory; recreate it and copy the fixtures before repeating a run.

Final release medians (first benchmark round excluded; then the median of the
six remaining rounds per launch and of the two launches per variant):

| Cost | 1.25× before → after | 2× before → after |
|---|---:|---:|
| 3,000 cached document-size reads | 2.671 → 2.369 ms | 2.264 → 2.603 ms |
| 20 forced style/layout reads | 2.490 → 2.047 ms | 2.013 → 2.495 ms |
| Process CPU per resize | 14.375 → 14.375 ms | 22.500 → 20.625 ms |
| Process RSS | 158,568 → 158,614 KiB | 165,468 → 165,650 KiB |
| Drag first movement | 27.30 → 26.60 ms | 26.20 → 28.45 ms |
| Wheel first movement | 9.00 → 6.60 ms | 9.05 → 9.05 ms |

All sixteen scroll probes moved and had zero hitches; slowest draw 4.58 ms.
Idle samples show zero measured CPU and zero page frames, with 2–4 existing
watchdog wakes in three seconds. Resize-tool durations are about 409–410 ms,
including the quiet wait. Mixed changes across these short shared-host
samples do not establish a speedup or a latency improvement. The corrected
measurements do additional useful work: independent body clipping now changes
the document's measured extent, which the baseline got wrong.

The actual Stripe homepage was then checked through all fifteen live
scale/theme combinations. Its hero remains colored, the DPR/media state
follows changes, and all fifteen additional widget reviews are clean (45
total reviews). The hero still measures 1000 CSS px wide. These are regression
checks only; no Stripe configuration or journey is accepted. `stripe/` keeps
captures, observations and the live probe log.
