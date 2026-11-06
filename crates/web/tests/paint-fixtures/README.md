# Independent static page pixels

Twelve authored 100×100 CSS-pixel documents, captured at emulated DPR 1,
1.5 and 2 in Olo Chrome 157.0.8103.38 on the owned `browser-paint` Agent
Screen. Chrome ran as a Wayland client with an explicit isolated profile
under owned `/var/tmp`; no personal window/profile was used. `chrome.json`
records the precise browser revision, viewport checks and PNG hashes.
`../inline-fixtures/plain.ttf` is our original fixed-metric outline font.

Thirty of 36 observations match exactly. All six fractional/multicolour
border observations **fail**: antialias coverage and border widths/corners
still differ. They remain in the independent capture and comparison;
`independent_chrome_fractional_pixels_not_yet_matching` is explicitly ignored
with its known reason, not counted as a passing comparison. No tolerance,
mask, shifted alignment or excluded pixels are used. This is no evidence of
arbitrary-font fidelity or website compatibility.

Reproduce on an owned screen only:

```
olo-screen run OWN_SCREEN olo-chrome --user-data-dir=/var/tmp/OWNED/profile --remote-debugging-port=18739 about:blank
/usr/bin/python3 crates/web/tests/paint-fixtures/capture.py /var/tmp/OWNED/chrome
```

The capture script serves only the local probe/font, checks font readiness
and viewport/DPR, and uses Chrome's DevTools screenshot API. Stop the screen
after capturing. Never connect the script to the person's Chrome.

To compare the native library:

```
cargo build --release -p olo-web --example paint_reference
# First eval "$(olo-screen env OWN_SCREEN)" for owned native binaries.
target/release/examples/paint_reference /var/tmp/OWNED/native
/usr/bin/python3 crates/web/tests/paint-fixtures/compare.py /var/tmp/OWNED/native /var/tmp/OWNED/report.json
```

The comparator exits nonzero while any observation differs. The regular
release test checks all 30 currently matching observations; explicitly run
its ignored test to see the unresolved exact failure. Existing 417 Chrome
DOM geometry observations are a separate, earlier suite.

Primary method references:
https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-captureScreenshot
https://chromedevtools.github.io/devtools-protocol/tot/Emulation/#method-setDeviceMetricsOverride
https://html.spec.whatwg.org/multipage/rendering.html#the-page
