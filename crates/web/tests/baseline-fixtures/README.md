# Native Chrome text baselines

Captured in Chrome 157.0.8103.38 on an owned Wayland Olo Agent Screen at
real output scales 1, 1.25, 1.5, 1.75 and 2. CDP metric emulation was cleared;
each observation waited for the actual DPR, font readiness and two frames.
The window was at odd x=49. Version/revision and source hashes are in JSON.

`baselines.html` embeds the existing, original `inline-fixtures/plain.ttf`
font. Its 41 cases cover ten sizes, three explicit line heights and normal, plus
a 35px normal-height case that exposes a 61-device-pixel round trip at 1.75x.
The zero-height inline-block marker reports the baseline independently of
glyph ink. Chrome confirms a custom Olo Inline plain face. `chrome.json`
contains 205 DOM observations. The integration test compares baselines and
inline content bounds, retaining one document through ascending and descending
scale changes. Downloaded fonts use Chrome's default render style; they must
not acquire the installed-font adjustment merely because DPR exceeds one.

`installed-chrome.json` records a separate 120-case probe using Arial,
Times New Roman and Courier New, resolved to Liberation Sans/Serif/Mono.
It includes independently read head/hhea/OS2 values and file hashes. The unit
test checks 580 single-line content metrics without requiring installed fonts;
the last four cases wrap, so their span rectangle cannot give a single-line
ascent. Full page_dump comparisons include those four cases by comparing the
last baseline. All 600 installed-font baselines match after the change (354
before). Absolute tolerance 0.0002 CSS px accommodates Chrome's float output
at large y coordinates, not a visible-pixel tolerance.

To recapture the downloaded-font probe, launch `olo-chrome` through
`olo-screen run` on your own screen with a fresh `/var/tmp` profile and CDP
port, then use that screen's environment:

```sh
/usr/bin/python3 crates/web/tests/baseline-fixtures/capture.py \
  --screen NAME --pid OWN_CHROME_PID --profile /var/tmp/OWN/profile \
  --port OWN_PORT --out /var/tmp/OWN/capture
```

The capture refuses an unrelated profile/PID or stopped screen and waits for
the person to release control. It restores display settings on exit. The
installed fixture, original capture scripts, before/after page_dump reports,
native screenshots, costs and reviews are retained under
`docs/browser/measurements/2026-11-10-text-baselines/`. The repetitive native
font reports are losslessly compressed as `.json.gz` (Python
`json.load(gzip.open(path))`); decompression reproduces the original export.

The policy follows Chromium's
[font metrics](https://chromium.googlesource.com/chromium/src/+/a733b5afbb3ba618844ff8876b89ad1a774b2f18/third_party/blink/renderer/platform/fonts/font_metrics.cc),
[Linux render parameters](https://chromium.googlesource.com/chromium/src/+/a733b5afbb3ba618844ff8876b89ad1a774b2f18/ui/gfx/font_render_params_linux.cc)
and [system versus downloaded faces](https://chromium.googlesource.com/chromium/src/+/a733b5afbb3ba618844ff8876b89ad1a774b2f18/third_party/blink/renderer/platform/fonts/font_platform_data.cc).
Installed text above 1x borrows one device pixel from ascent when rounding
would shorten descent. Normal line height sums rounded metrics in device
space before conversion to CSS space, advancing to the adjacent float only when
needed to preserve that device integer on a round trip, avoiding half-leading
underflow. BeOS's separation of ascent, descent and leading remains useful
(`vendor/bebook/BFont.md`, GetHeight); web placement follows the Chrome oracle.

This is baseline/content-metric coverage, not a raster or website parity pass.
Hinted/VDMX metrics, font-rendering preferences, `text-rendering` overrides,
glyph masks, color glyphs and arbitrary font-table policies remain outside
this correction. No test was relaxed to excuse a pixel mismatch.
