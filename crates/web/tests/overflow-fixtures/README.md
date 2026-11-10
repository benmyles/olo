# Viewport overflow reference

Captured on the owned `codex-browser` Agent Screen on 2026-11-10 with Olo's
Chrome **157.0.8103.38**, native Wayland, 1000 × 613 CSS-pixel page viewport.
`overflow.html` is the source; `chrome.json` records twelve live style changes,
document scroll sizes, computed values and four hit-test points. `axes.json`
records all 25 overflow-axis pairs. Each green hit lies inside the solid lime
content; the native regression test also checks the corresponding raster pixel.
The same CSS-coordinate checks run at five native layout scales. The retained
geometry reference is 1×; this is not a claim of a five-scale Chrome recapture
of these twelve cases.

`capture.py` requires an already owned running `codex-browser` screen and its
Chrome on CDP port 9338, with the isolated profile under
`/var/tmp/codex-browser-scrollbars/chrome-profile`. It verifies ownership before
using CDP. Copy `overflow.html` to that scratch directory before running it from
the repository root. It writes its captures there, not over these references.

Chrome preserves `clip` on an axis alongside `auto` or `scroll`, matching the
current [CSS Overflow draft](https://drafts.csswg.org/css-overflow-3/#overflow-properties).
An attempted conversion to `hidden` failed this oracle and was removed.
The [viewport propagation rule](https://drafts.csswg.org/css-overflow-3/#overflow-propagation)
applies only the selected element's overflow to the viewport; the body's
independent clipping still applies when the root owns viewport overflow.

`tests/overflow.rs` compares live and fresh measurements, pixels, scroll
container counts, root-only scrollbar property propagation, non-inheritance,
and unpainted overflow on the unclipped axis. CSS containment, arbitrary
positioning/transforms in scrollable overflow, classic scrollbar gutters and
DOM body scroll aliases remain outside this fixture's acceptance claim.
