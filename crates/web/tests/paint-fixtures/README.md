# Independent native-scale page pixels

Eighteen authored 100×100 CSS-pixel documents, captured in Olo Chrome
157.0.8103.38 at actual Wayland output scales 1, 1.25, 1.5, 1.75 and 2 on
the owned `browser-edge` Agent Screen. `chrome.json` records the exact
browser/GPU, source and PNG hashes, native/frame DPR, viewport dimensions,
DOM rectangles and computed border widths. The original fixed-metric font
is `../inline-fixtures/plain.ttf`. These are not arbitrary-font or website
acceptance tests.

**54 of 90 exact comparisons pass; 36 fail.** The same native captures compare
36/90 against the preceding renderer. No masks, tolerance, alignment shifts
or discarded pixels are used. All observations remain in the full comparator.
The regular release test checks the 54 matching observations; its companion
exact test for the 36 known gaps is explicitly ignored, not a passing result.

Remaining exact failures:

| Inputs | Scales | Scope |
| --- | --- | --- |
| glyphs, colors, border, clip, normal-order, hidden | 1.25, 1.75 | Font mask rasterization |
| fractional | all five | Font masks after device baseline placement |
| multicolor-border | 1.25, 1.5, 1.75, 2 | GPU corner coverage/quantization, up to 1 byte level |
| asymmetric | all five | Different corner coverage filter |
| transparent-side | all five | Alpha/coverage quantization |
| unequal-alpha | all five | Retained colour/premultiplication precision |

The border implementation now uses disjoint strips and a premultiplied
corner mix before one source-over, without temporary border surfaces.
Uniform half-alpha joins, collapsed inner boxes and clipped coloured borders
have independent exact matches at all five scales. Separate regression tests
check tiled/whole equality across asymmetric translucent borders and empty
clipped edges, source-safe paint caches, cancellation and distant coordinates.

## Why the earlier captures are archived

`emulated/` retains all 36 earlier captures, their original inputs, script and
browser metadata. DevTools `Emulation.setDeviceMetricsOverride` changes DPR
without reproducing native border snapping: a 3px border at actual scale 1.5
computes to 2.66667 CSS px and occupies four physical pixels; the emulated
capture retained 3 CSS px. Those observations cannot validate the native
style host at that scale. They remain historical evidence, not discarded
unfavourable comparisons. The production style snapping rule was already
correct. Native observations and methods replace the unsuitable comparison.

## Reproduce

Use only an owned screen and isolated profile:

```
olo-screen start OWN_SCREEN --size 1280x800 --scale 2
olo-screen run OWN_SCREEN olo-chrome --user-data-dir=/var/tmp/OWNED/profile --remote-debugging-port=18739 about:blank
# Wait for that Chrome's DevTools endpoint to be ready.
eval "$(olo-screen env OWN_SCREEN)"
/usr/bin/python3 crates/web/tests/paint-fixtures/capture.py /var/tmp/OWNED/chrome --screen OWN_SCREEN
```

The script checks the DevTools browser PID against this screen's windows,
uses no emulated device metrics, checks real DPR and font readiness inside a
100×100 iframe, respects Take Control via `wait --free`, and restores the
screen's original settings. Stop the screen after capturing. Never connect
the script to the person's Chrome.

```
cargo build --release -p olo-web --example paint_reference
# Use an owned screen environment for native test programs.
target/release/examples/paint_reference /var/tmp/OWNED/native
/usr/bin/python3 crates/web/tests/paint-fixtures/compare.py /var/tmp/OWNED/native /var/tmp/OWNED/report.json
```

The comparator exits nonzero while any observation differs. The manifest test
checks all 90 immutable reference images, input coverage, dimensions, native
DPR and capture-source hashes. Existing 417 independent Chrome DOM geometry
observations are a separate suite.

Primary references:
https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-captureScreenshot
https://drafts.csswg.org/css-values-4/#snap-a-length-as-a-line-width
https://drafts.csswg.org/css-backgrounds-3/#color-transition
