# Fractional-scale text fitting

Chrome independently records 1,350 observations: three sizing contexts
(centered column flex item, max-content block, inline-block), five font sizes,
three two-weight text samples, six sequential width states and five actual
live device scales 1/1.25/1.5/1.75/2. The font is the repository's Noto Sans
variable font; HTML, cases and font hashes accompany the Chrome version.

For each engine, the journey measures the natural border-box width, then sets
it one, two and four device layout units narrower, one unit wider, and restores
natural sizing. A unit is 1/(64×DPR) CSS pixels. Chrome keeps one line at a
one-unit shortage, and wraps at two/four units for every case. Expected results
come only from Chrome. Each engine derives its own subsequent width inputs
from its natural-width measurement, just as the identical JavaScript does.

The Rust test checks width against Chrome within 0.02 CSS pixels, line height
within two lines' known 1/64-device rounding difference, and exact equality of
fresh/reused native widths and heights after each mutation. Line-height
quantization remains a separate open issue; the allowance cannot conceal an
extra line. These component tests do not accept a website or prove glyph-pixel
identity, complete line-breaking conformance, or arbitrary font coverage.

Two engine gaps were exposed: strict floating comparison wrapped a naturally
sized line after adding/subtracting padding, and mixed-run measurement skipped
the centipixel effective-font-size quantization used by the single-run
intrinsic path. Both now share the same metric calculation. Line fitting
allows Chrome's one-device-layout-unit margin plus machine-roundoff tolerance;
the two-unit controls continue to wrap.

Recapture using Olo Chrome already running on your own Agent Screen:

```
/usr/bin/python 3 capture.py --screen YOUR_SCREEN --port YOUR_PORT
```

The driver checks the owned screen, Chrome port/profile, loads the exact font,
changes actual display scales, restores settings and closes its temporary tab.
