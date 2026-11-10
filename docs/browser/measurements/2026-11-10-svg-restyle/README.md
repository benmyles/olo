# Retained SVG styles and cancelled transition completion — 2026-11-10

Two independent bugs left Stripe’s testimonial logos showing their previous
selection. A paint-only restyle shared compiled SVG sources with baked old
colors. After fixing that, cancelling a render at a transition’s completion
still consumed its final sample, leaving the retry on the previous color.

SVG compilation now records its style dependencies, including referenced
paint servers, masks, use targets and cached clip definitions. A reverse index
selects affected sources from the changed style chunks. Other SVGs and HTML
geometry remain shared; text outlines survive paint-only changes when their
sources are unchanged. Compiled paint signatures include their immutable style
inputs. In-place measurement patches refresh the same sources. Reference id
changes trigger recompilation even without a computed-value difference, and
a failed attempt to borrow styles mutably leaves the previous revision intact.
Compilation remains on the worker with node/work/payload budgets and cancellation.
Fallback findings use the existing conservative partial-stage accounting.

Rendering now samples animation state, animated-node membership and container
sizes into pending copies, committing them with a successfully rendered scene.
A cancelled pass neither consumes a transition completion nor publishes its
completion event. The retry paints the terminal value and emits one event.

## Evidence

The original minimal regression failed red instead of blue at1×. The expanded
SVG regression compares120 cached-versus-fresh pixel states at five scales,
covering fills, currentColor, strokes, group opacity, external gradient stops,
shared clips, masks and text visibility. It checks immutable old snapshots and
sharing of unrelated SVGs. Other regressions cover id retargeting, in-place
measurement and a cancelled transition completion. The latter failed purple
instead of blue before the transactional change. Existing independent SVG
fixture gaps remain explicit; fresh-versus-cached tests do not establish new
standards conformance by themselves.

Owned Agent Screen `codex-browser`, odd window x=49, live scale changes at
1,1.25,1.5,1.75,2× in light/dark/night. Native and Chrome viewports and anonymous
experiment cohorts match. Real Jobber button clicks reach Chrome’s scroll
endpoint in every setting. Pixel checks find the active Jobber fill in10/15
baseline captures,14/15 with SVG invalidation alone, and15/15 with both fixes.
These are captures of an intermittent failure, not statistical reliability
estimates. `selection-colors.json` records actual solid-color pixel counts in
fixed logo regions; it is not a whole-image similarity tolerance. All15 final
native widget reviews have no findings. The four testimonial buttons also
match positions and computed fills in the1× journey.

`before`, `after-svg-only` and `after` preserve the three stages. The first
full workspace test run aborted while the DiskBackup process shut down after
all ten tests passed; both the exact binary and its cargo invocation passed
on retry. Its failure log is retained separately from final validation. A subsequent run
caught malformed unquoted SVG attributes in a new fixture (the closing slash
was part of the value); the markup was corrected, and the targeted tests pass.
The latest shared-kit work through97541be6 was merged as343b606b; Browser’s
binary was unchanged by that integration.

This fixes the diagnosed selection failures. It does not accept Stripe:
events/footer geometry, other journeys, vertical writing, complete SVG behavior,
page darkening, security/isolation and broad browser compatibility remain open.
Stripe remains0/30; the corpus remains0/100.

The capture and measurement scripts retain their original scratch root under
`/var/tmp`; create your own owned screen/profile and adjust paths before replay.
No personal session was installed into or reloaded.

## Final validation

`cargo build --release --workspace` and `cargo test --release --workspace`
both exit 0: **2,273 passed, 0 failed, 140 ignored across 262 suites**.
The final targeted measurement/SVG suites pass all 48 tests. Browser layout
lints at 1280×800, 1440×900 and 2880×1800 report zero errors. Existing ignored
tests remain nonpasses. `validation.json`, `source-hashes.json` and
`binary-identities.json` identify the tested sources, commands and binaries.
Run `/usr/bin/python3 docs/browser/measurements/2026-11-10-svg-restyle/verify.py`
from the repository root to check the retained evidence.

## Release costs

ABBA order at each scale, eight processes total, with 121 SVGs and one changing
source. Each process performs seven batches of 3,000 cached geometry reads,
20 SVG class mutations with synchronous geometry reads, and 20 unrelated HTML
class mutations with reads. The table pools the last six batches per process;
the first is warmup. It also records 90 animation-frame callbacks, eight
resizes, three idle seconds, compositor timings and root drag/wheel checks.

| Measurement | 1.25× before → after | 2× before → after |
| --- | ---: | ---: |
| 3,000 cached reads, median ms | 1.443 → 1.437 | 1.727 → 1.631 |
| 20 SVG mutations, median ms | 1.031 → 1.172 | 0.998 → 1.251 |
| 20 unrelated mutations, median ms | 0.726 → 0.742 | 0.741 → 0.753 |
| Animation callback interval, median ms | 8.333 → 8.334 | 8.333 → 8.322 |
| CPU per resize, median ms | 16.250 → 18.125 | 22.500 → 20.000 |
| RSS after resizes, median KiB | 161968 → 168132 | 164972 → 165662 |
| Drag first movement, median ms | 33.85 → 34.50 | 34.45 → 36.75 |
| Wheel first movement, median ms | 17.50 → 17.10 | 17.10 → 17.10 |
| Scroll hitches, total | 0 → 1 | 0 → 0 |
| Slowest scroll draw, ms | 5.03 → 9.55 | 7.45 → 2.63 |
| Idle CPU / frames | 0 / 0 → 0 / 0 | 0 / 0 → 0 / 0 |

The fixed engine performs SVG work that the incorrect baseline skipped.
There is **one remaining 9.55 ms draw at 1.25×**, exceeding the 8.3 ms budget;
this checkpoint does not pass every performance gate. RSS varies between runs
(the two 1.25× candidate samples are 177480 and 158784 KiB). The external
resize command's ~410 ms settling time is not an input-latency measurement,
and JavaScript callback intervals are not presentation-frame intervals.
The existing script watchdog still wakes about twice per second. No new idle
timer was introduced. These measurements establish no speed advantage over Chrome.

A separate 2× run updates one SVG for 1,200 animation frames. RSS growth from
frame 900 to 1200 is 124 KiB before and 136 KiB after; settled RSS is 158092
and 157132 KiB. This is comparable short-run retention, not proof against all
leaks. Both idle samples again record zero CPU and zero frames.

The initial benchmark driver timed out because its unrounded array of frame
intervals exceeded Browser's console-record length. Rounding only the serialized
intervals to three decimal milliseconds fixed the driver; the workload stayed
the same. The first retention driver omitted the console timing switch and
also timed out; the corrected run enabled it for both builds. Initial scripts
and failure logs are retained. Diagnostic style checks reported existing
animated-span/footer differences; no tile-check difference was reported.

Captured text logs have only trailing blank lines trimmed for repository whitespace checks.
