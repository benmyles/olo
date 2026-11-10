# Ancestor scrolling and testimonial activation — 2026-11-10

Stripe's Jobber navigation handler called an empty `scrollIntoView` stub. The
engine now plans inner-to-outer scrolling from a retained layout snapshot,
including start/center/end/nearest, the nearest-container option, directional
axes, target scroll margins and scrollport padding. Visible descendant overflow
is aggregated once per measurement; clipped descendants stop propagation.
Nested scroll ranges include negative RTL coordinates and device-grid rounding.

Programmatic smooth scrolling requests frames only while active, uses retained
geometry and coalesces scroll/scrollend notifications. The ScrollResult promise
reports completion or interruption; argument/receiver errors reject it. Instant
scrolls, host input and detached targets cancel active animations. The curve is
based on Chromium's distance-based cubic curve; exact timing, velocity-preserving
retargeting and every user-interruption ordering are not established.

A separate bug found by the RTL fixture is fixed: incremental styles used
script mutation order, so a child changed before its parent could keep old
inherited direction. The patch queue now processes pending ancestors first.

## Independent reference

[Component fixtures](../../../../crates/js/tests/scroll-into-view-fixtures/README.md)
record Chrome 157.0.8103.38 at five actual display scales. Of 140 geometry
cases, 135 pass in fresh and reused documents. Five vertical-rl-start cases
remain explicit ignored nonpasses. Completion/interruption and IDL observations
are independently captured. Native tests also check that a completed animation
has no pending frame/timer and does not remeasure layout on each frame.

The first full suite caught stale registry counts after adding 17 longhands and
six shorthands. The first promise comparison caught scrollIntoView.length=1
instead of 0; both are fixed. Diagnostic logs retain those failures. A smooth
reference run while Chrome was occluded had no intermediate sample; the final
capture explicitly focuses Chrome and records intermediate motion.

## Scope

This closes the diagnosed testimonial-button stub, not Stripe acceptance.
Transformed ancestor coordinates, nested classic gutters, iframe traversal,
snap positions, vertical block layout, body/quirks semantics, keyboard defaults
and complete interruption/event timing remain open. Events/footer layout and
smaller text/pixel differences remain. Stripe is 0/30; corpus is 0/100.

Reference: [CSSOM View](https://drafts.csswg.org/cssom-view/#dom-element-scrollintoview),
[Chromium scroll curve](https://chromium.googlesource.com/chromium/src/+/HEAD/cc/animation/scroll_offset_animation_curve.cc).
All GUI work used the owned codex-browser Agent Screen. No personal session
was reloaded or installed into. Scripts retain their original scratch paths;
update them for a newly owned screen/profile before replay.

## Live observations

Actual structured pointer clicks at an odd x=49: baseline stays on Mindbody
in all15 settings; candidate reaches Jobber at Chrome's offset in all15.
Offsets are951,951.2,951.3333,952,951 CSS px for scales1,1.25,1.5,1.75,2.
Viewport/client sizes and anonymous server experiment cohorts match.
Thirty native reviews have no kit findings; sixty screenshots (thirty engine pairs) are kept.
The first candidate attempt sampled950 before completion at951; the driver now
waits for scrollend. Additional settling reduces, but does not eliminate,
selection-highlight paint lag in the captures. Computed SVG fills match Chrome
for all four controls in the1× journey (`carousel.json`), and a later capture
shows the correct paint. This unresolved timing difference prevents a pixel
parity claim. Page darkening is not supplied by this change.

`animation-check.py` uses a quiet local page and an actual button click, then
reads its process log without sending further input. The candidate produces
27 intermediate scroll events, first at16.45ms, completes at457.63ms and ends
at[700,300]. Its historical `before-clock` filename denotes a diagnostic of
the candidate before considering a host-clock change; no such change was
needed or made. This is not a Chrome timing comparison.

The main integration (`e4e86aca`) initially broke workspace test compilation:
Interface Kit's copied executor referenced absent test dispatch support and
its line wrapper referenced the old TestAppContext. Restore its test dispatcher
and dependencies, and construct the same NoopTextSystem directly. All37
Interface Kit unit tests pass, with3 existing ignored tests. Production Browser
bytes are unchanged by that test-support repair.

## Validation

Final `cargo build --release --workspace` and `cargo test --release --workspace`
pass:2,262 tests passed,0 failed,140 ignored across260 suites. The ignored
count includes the retained vertical-writing nonpass and three Interface Kit
tests. Both Browser layout lints pass at1280×800 and1440×900.

`preliminary-*` records the obsolete registry-count failure;
`main-integration-*` records the missing Interface Kit test support. The final
logs and exit files are separate. Test-support repairs change no production
Browser bytes (SHA-256 checked against the captured candidate).

The first performance capture exceeded the browser console's diagnostic line
limit while serializing every destination. The failed run and truncated console
are retained. The final driver validates all twenty destinations in each batch
and emits compact first/last coordinates and a validity flag, preserving the
same measured operations.

The first scroll-check location fell inside the nested HTML scroller, so its
root-scroll telemetry reported “nothing moved.” Those records are retained in
`costs.json` and are not latency results. `latency.py` measures root input at
[900,500], outside that nested box, and rejects runs without measured movement.

## Release costs

ABBA at1.25× and2×, same integrated baseline and candidate, eight processes;
another eight processes for valid root-scroll latency. Pooled medians exclude
the first JS batch in each process. Both binaries have diagnostic timing enabled.

| Metric, before → after | 1.25× | 2× |
| --- | --- | --- |
| 6,000 cached nested geometry reads, ms | 33.877 → 12.289 | 34.826 → 12.684 |
| 20 mutations + scrollIntoView calls, ms | 0.163 → 4.824 | 0.190 → 4.980 |
| CPU per resize, ms | 10.625 → 10.625 | 18.750 → 16.875 |
| RSS after resize, KiB | 157284 → 157506 | 165166 → 165830 |
| Root drag first movement, ms | 28.650 → 28.650 | 28.500 → 28.850 |
| Root wheel first movement, ms | 17.350 → 17.100 | 17.600 → 14.100 |

Cached reads are faster in this workload. The mutation/call batch is not equal
work: baseline scrollIntoView is a stub and does not force layout, while the
candidate measures and scrolls. Its twenty destinations are validated.
Resize tool latency is about410ms including settling, not input latency.
Every idle sample reports0 CPU and0 page frames, with the existing watchdog
wakeups. Both have0 root-scroll hitches and16 stationary frames in the measured
runs; maximum recorded draw is9.04ms before and3.49ms after. Small mixed CPU/RSS
changes on this shared host do not establish general speed or memory superiority
over Chrome.
