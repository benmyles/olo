# Live page environment updates

Baseline: `37497f51`. This fixes the main page's frozen scripting viewport,
DPR, media queries and measurement environment. It does **not** accept Stripe
or any other website. Corpus acceptance remains **0/100**.

## Behavior and evidence

The worker shares its current viewport/palette with the persistent measurement
host before updating the realm. The realm invalidates geometry without
rebinding the document or resetting its clock. Resize and visual-viewport
notifications precede changed media-query notifications and animation
callbacks. Changes before a frame coalesce. DPR-only changes notify resize,
as observed with actual Olo Chrome display-scale changes. Theme-only changes
notify the affected media queries. The implementation follows the
[CSSOM View event model](https://drafts.csswg.org/cssom-view/#events).

Media-query matches are current before event delivery; readonly accessors,
MediaQueryListEvent, legacy listeners, handler objects, once/abort/removal,
and listener ownership are exercised. Unobserved lists are weakly held;
lists with change listeners remain alive. Dead registry entries are reaped
on environment changes and in bounded batches on query creation, without
timers or depending on finalizer scheduling. Up to 128 query strings of at
most 4,096 code units share answers within an environment.

The first GUI attempt updated JS values but exposed another stale cache:
Measurer's media-filtered stylesheet rules. `first-attempt-results.json`
retains that failure. Measurer now reselects rules and conditional font faces,
retaining unchanged font sources/scopes. Browser host revisions also include
scheme and relevant system colors. ResizeObserver compares the requested box,
so device-pixel observations change at a new DPR even when CSS dimensions do not.

`viewport.html` is an independent component fixture. `before.json` remains
at its initial values through display changes. Final `after.json` and
`chrome.json` contain 15 live light/dark/night × 1/1.25/1.5/1.75/2× observations,
followed by four width changes. All 19 agree on canvas backing dimensions,
box dimensions (within 0.001 CSS px), DPR, media matches and device-pixel
observations. Native page identity/time origin remain unchanged. Window
heights differ because the two browsers have different native toolbars;
those heights are recorded, not asserted equal. Run `verify.py` to check
the retained observations.

`stripe.json` and `stripe-chrome.json` capture the actual live homepage.
Native DPR/theme/resolution queries follow all 15 changes; the hero remains
colored. Chrome's hero also uses roughly one backing pixel per CSS pixel,
so this site's canvas need not multiply its dimensions by DPR. A separate
remaining difference is visible: Chrome's classic scrollbar reserves about
15 CSS px (hero width ~985), while Olo's overlay scrollbar leaves 1000.
The hero heights differ too. These are diagnostics, not pixel-equality passes.

All 30 native widget reviews (fixture and Stripe), at odd x=49, are clean.
Live Browser lints at 1280×800 and 1440×900 report zero errors. Matrix images
are overviews; original before/after fixture shots are in the visual journal.

## Release costs

`measure.py` runs ABBA at each scale on the owned Agent Screen, with frozen
release binaries and hashes in `costs.json`. The page has 400 scroll rows,
canvas updates and observers. Event batches dispatch 5,000 events to 16
listeners; query batches construct/read 1,000 identical media queries.
Each process also performs 16 live width changes. Medians:

| Measurement | 1.25× before → after | 2× before → after |
|---|---:|---:|
| 5,000 event dispatches, ms | 1.568 → 1.536 | 1.153 → 1.326 |
| 1,000 repeated queries, ms | 0.993 → 0.517 | 1.046 → 0.474 |
| Whole-process CPU per resize, ms | 15.625 → 42.812 | 24.688 → 51.250 |
| RSS after resizing, KiB | 165,436 → 183,654 | 174,048 → 184,068 |
| Resize command plus idle-wait, ms | 409.371 → 409.315 | 409.369 → 409.273 |

The resize wall time includes the tool's quiet wait and IPC; it is **not**
an application paint-latency measurement. The baseline omitted resize
callbacks and observer work, so the CPU increase buys missing behavior;
it is not an equivalent-work speed comparison. The repeated-query cache
improves this microbenchmark; no whole-browser speedup is claimed.

Every idle sample has zero measured CPU and page frames. Existing watchdog
wakes remain (2–4 in three seconds). `costs-pilot.json` is an earlier variant,
including samples overlapping builds; it is not the final comparison.
The first 2× scroll probes used a point outside the scaled output and moved
nothing. Those errors remain in `costs.json`; corrected probes use an interior
point and are recorded separately in `scroll-2x.json`.
All 16 valid drag/wheel probes have zero hitches; the slowest draw is 3.77 ms.
Median first movement at 1.25× is drag 28.45→27.0 ms, wheel 9.05→9.05 ms;
at 2×, drag 28.6→25.2 ms and wheel 9.05→8.85 ms. These short shared-host
samples do not establish a latency improvement.

## Validation and remaining work

Release workspace build succeeded. Full release workspace suite: **2,193
passed, 0 failed, 136 existing ignored**. After the final JS cache/dispatch
changes, all 76 JS tests passed again. The shared test runner later queued
without starting its process; the final compiled JS test binaries were run
directly and completed successfully. New tests cover coalescing, frame order,
same-document state, listener ownership/GC, invalid input, and media-rule
reselection without reloading unchanged font sources.

Remaining: scrollbar/layout-viewport agreement with Chrome; iframe resize
currently restarts child workers and does not use this propagation path;
responsive-image reselection; complete VisualViewport, observer and Web IDL
semantics; Stripe's other journeys/configurations; security and broad browser
acceptance. Connected offscreen image pixel recovery and existing watchdog
wakes also remain open. No personal desktop installation/reload occurred.

The scripts use `/var/tmp/codex-browser-viewport`, frozen `browser-before`
and `browser-after`, and the owned `codex-browser` Agent Screen. Chrome drivers
verify the screen's PID and dedicated profile before CDP access. Launch Chrome
only through `olo-screen run`; the probe scripts never start a system browser.
