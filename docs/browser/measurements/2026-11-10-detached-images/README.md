# Detached image ownership — 2026-11-10

Stripe's live homepage uploaded `palette-light.3c0e6604.webp` from a detached
`new Image()`. The image remained complete, with a natural size of 480 × 480,
but `Realm::canvases` discarded its decoded pixels because it was disconnected.
Both `drawImage` and WebGL then received transparent pixels. The hero was black.

The fix makes wrapper identities weak and mirrors native DOM ownership with
traced V8 internal parent/child fields, including shadow and template content roots.
Connected nodes survive through the document; any referenced node keeps its
detached tree alive. Unreachable cycles can be collected. The collector queues
ids; the page worker releases decoded images and 2D canvases on subsequent
work. Image requests pin their element until dispatch. Shared decoded assets
are charged once to V8's external-memory accounting. There is no idle poll.
The unused strong shadow-root registry was removed and slot listener targets
are weak. Native DOM arena reclamation, WebGL context reclamation and complete
observer lifetime semantics remain separate work.

This follows HTML's [image lifetime requirement](https://html.spec.whatwg.org/multipage/images.html)
and V8's [weak persistent handle model](https://v8.dev/docs/embed).
The existing connected, offscreen-image decode eviction still needs a synchronous
pixel-source recovery path; this change excludes detached image sources from
that viewport policy. It does not claim general resource-lifetime completeness.

The independent Chrome test returns `[true,1,17,91,203,255]` after two animation
frames with an unconnected image. The native regression returned transparent
RGBA before the fix and the same opaque RGBA after it. Four samples from the
actual Stripe WebP also match Chrome exactly:

| Coordinates | Both browsers, straight RGBA |
| --- | --- |
| 0, 0 | 241, 246, 255, 255 |
| 100, 100 | 252, 179, 42, 255 |
| 240, 240 | 255, 213, 164, 255 |
| 479, 479 | 252, 176, 220, 255 |

The 15 native-widget reviews cover light/dark/night and live changes through
1, 1.25, 1.5, 1.75 and 2×, at window x=49. All are clean. The page's palette
remains correct. **The page's JavaScript `devicePixelRatio` remains stale at 1
when the display changes scale**, however: the native widgets and page raster
change scale but the scripting realm's viewport configuration does not. That
is an observed compatibility failure and the next work item. These screenshots
are not synchronized animation comparisons or site acceptance.

Release measurements and complete test totals are recorded with this checkpoint.

The first release stress run caught a V8 abort: deleting the private parent
property normalized an API-object map during marking and tripped its range
write barrier. The final representation uses three native internal fields
(node id, parent, child set), never property deletion, and bumps the prelude
snapshot format. `dom-churn.js` is now a regression: 8 rounds of 1,000 nodes,
fragment moves and removal, repeated in three realms, followed by forced GC
and an assertion that fewer than 10 wrappers survive. The crash stack is kept
as evidence of the rejected representation, not a passing run.

A second cold-start crash occurred in read-only heap allocation even with the
internal-field representation. An interleaved fresh-cache matrix reproduced
it in the unchanged baseline (2/8 failures) and the candidate (1/8). The
candidate with `--no-extensible-ro-snapshot` passed 8/8. V8's default isolate
group is shared by the asynchronous snapshot creator and ordinary pages;
`ReadOnlyHeap::DeserializeIntoIsolate` leaves that heap writable for an
extensible snapshot until finalization. The final runtime keeps the common
read-only heap sealed and bumps the snapshot format to 5. The rebuilt candidate
passed 32/32 further fresh-cache GUI starts without the experimental environment
flag. All runs are retained, including failures. The smaller synchronized
`read-only-order-probe.rs` did not reproduce the fault (both modes passed), so
it is not counted as a regression test. Nor did 36 headless, three-realm churn
processes reproduce it; the native browser cold-start matrix is the relevant
reproduction.

## Release cost and validation

`measure-lifetime.py` runs ABBA at 1.25× and 2× with separate warmed caches for
the frozen baseline and final candidate. Each page performs eight rounds of
1,000 node creations, moves through a fragment, and removals, followed by idle
and drag/wheel checks over 1,000 static rows. Phase values are medians across
16 rounds per variant/scale; startup/RSS/whole-process CPU are medians of two
processes. This was a shared development host, not an isolated benchmark host.
The whole-process CPU sample includes startup, rendering and scrolling.

| Scale | Phase | Before ms | After ms |
| --- | --- | ---: | ---: |
| 1.25× | Build 1,000 nodes | 4.768 | 5.859 |
| 1.25× | Move 1,000 nodes | 3.138 | 3.257 |
| 1.25× | Remove tree | 0.084 | 0.190 |
| 2× | Build 1,000 nodes | 6.060 | 7.775 |
| 2× | Move 1,000 nodes | 3.991 | 5.029 |
| 2× | Remove tree | 0.106 | 0.369 |

This correctness fix adds ownership work, rather than making DOM churn faster.
Median ready latency was 157.5→132.5 ms / 174→173.5 ms; RSS was
179,700→181,550 KiB / 185,538→188,038 KiB; whole-process CPU was
710→1,005 ms / 780→760 ms (1.25× / 2×). These small startup samples do not
establish a speedup. Drag first movement was 28.35→28.65 / 28.75→28.45 ms;
wheel first movement 8.95→5.35 / 6.10→9.75 ms. All 16 gesture checks had zero
hitches. All eight three-second idle samples had zero measured CPU and zero
frames, with 2–3 existing watchdog wakes. Eliminating those idle wakes remains
open. Compositor timing snapshots and full gesture distributions are in
`costs.json`; binary hashes identify the measured builds.

Final validation: `cargo build --release --workspace` succeeds;
`cargo test --release --workspace` has **2,189 passed, 0 failed, 136 existing
ignored**; 73 of the passing tests are in `olo-js`. Layout lint is clean at
1280×800 and 1440×900. All 15 final widget reviews are clean. The standalone
HTML regression also runs through the actual image decoder and frame loop:
the baseline displays `[true,1,0,0,0,0]`; the candidate and Chrome display
`[true,1,17,91,203,255]`. No install or reload of the person's desktop occurred.
`origin/main` was fetched and already contained in this branch.
