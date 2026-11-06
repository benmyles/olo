# Original text intrinsic-width fixtures

`cases.json` is original authored input; `chrome.json` holds independent,
unmodified min-content and max-content bounding widths from
Chrome157.0.8103.38, Chromium a733b5afbb3ba618844ff8876b89ad1a774b2f18.
The capture embeds the exact `../inline-fixtures/{plain,ligature,deletion}.ttf`
bytes, whose original generator and provenance live beside them. Input and font
SHA256 hashes are recorded. No system-font substitution supplies these widths.

60 cases ×5 live Wayland DPRs (1,1.25,1.5,1.75,2) give300 min/max pairs,
600 widths. Cases include collapsed/preserved/hanging/break-spaces whitespace,
nowrap and forced breaks, empty text, zero-width opportunities, space-spanning
and ordinary ligatures, deleted glyphs, zero/small sizes and fractional sizes
near font-cache and device-grid buckets. A first28-case capture was expanded;
every original140 row remains byte-for-byte equivalent as parsed JSON. Native
comparison tolerance0.0001 CSS px covers float32 exposure and is smaller than a
layout-grid unit; references were not adjusted to native output.

Run `capture.py` using `/usr/bin/python3` and the documented arguments after
`eval "$(olo-screen env NAME)"`, starting Chrome only with `olo-screen run NAME
olo-chrome --user-data-dir=/var/tmp/OWNED/profile --remote-debugging-port=0`.
The driver verifies its owned window/PID/profile, actual loaded fixture and
font status, clears device emulation and asserts every native DPR. It restores
settings, closes its tab/browser and waits for process exit. Stop the owned
screen after use. The legacy X11 spelling in Chrome's UA is not the backend.

The Rust operation uses original retained cluster edges, not reshaped line-cut
endpoints. Minimum ranges subtract enclosed prefix positions; maximum hard
lines enclose full advances, with separate leading preserved-space items when
wrapping. Font metrics retain float centipixel effective-size arithmetic,
26.6 device sizes and16.16 linear advances before layout enclosure. Matching
this fixture family does not establish arbitrary GPOS/variable-font accuracy.

Read primary sources at the captured revision:
- [CSS intrinsic sizing](https://drafts.csswg.org/css-sizing-3/#intrinsic-sizes)
- [Chrome minimum text ranges](https://chromium.googlesource.com/chromium/src/+/a733b5afbb3ba618844ff8876b89ad1a774b2f18/third_party/blink/renderer/core/layout/inline/line_breaker.cc)
- [Retained shape positions](https://chromium.googlesource.com/chromium/src/+/a733b5afbb3ba618844ff8876b89ad1a774b2f18/third_party/blink/renderer/platform/fonts/shaping/shape_result.cc)
- [Effective font-size precision](https://chromium.googlesource.com/chromium/src/+/a733b5afbb3ba618844ff8876b89ad1a774b2f18/third_party/blink/renderer/platform/fonts/font_description.cc)
- [Skia linear FreeType metrics](https://skia.googlesource.com/skia/+/ca84ce0a6f1659748188469a89361ccea71e3a64/src/ports/SkFontHost_FreeType.cpp)

Initial Rust scope is homogeneous horizontal LTR text with uniform whitespace
and one shaping item per hard line. Bidi/multiple adjacent font/script runs,
inline objects/decorations, discretionary hyphens, mixed policies and full
inline item sizing remain explicit unsupported contexts. No CSS min-content
keyword, flex formatter or website acceptance is supplied by this operation.
