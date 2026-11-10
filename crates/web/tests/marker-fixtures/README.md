# Outside marker geometry

`chrome.json` was captured from **Chrome 157.0.8103.38**, launched with
`olo-screen run codex-browser olo-chrome` on an owned Agent Screen and a
private profile under `/var/tmp/codex-browser-resume`. It records the browser
revision and source/font SHA-256 hashes. No Olo measurements generated these
expectations.

The local `markers.html` loads the original `inline-fixtures/plain.ttf` font.
Its text uses that font's supported characters, avoiding machine-dependent
fallback fonts. CDP `Emulation.setDeviceMetricsOverride` sets a 400 × 700 CSS
pixel viewport at each DPR: 1, 1.25, 1.5, 1.75, 2. After navigation and
`document.fonts.ready`, evaluate:

```js
Array.from(document.querySelectorAll('[id]'), e => ({
  id: e.id,
  rect: e.getBoundingClientRect().toJSON(),
  marker: getComputedStyle(e, '::marker').fontSize
}))
```

`tests/measure.rs` checks all 80 rectangles (320 coordinates), within 1/64 CSS
pixel, plus incremental restyles and empty-item text. The cases cover empty
and whitespace-only items, hidden/absolute children, large/small marker fonts,
block children, multiline items, inside markers, and `list-style-type:none`.

These are geometry checks, not a marker pixel pass or website acceptance.
Default bullet glyphs/gaps still differ from Chrome; complete marker content,
images, counters, writing modes and restriction of marker properties remain
open. Separate native screenshots/reviews cover live scale and theme changes.
