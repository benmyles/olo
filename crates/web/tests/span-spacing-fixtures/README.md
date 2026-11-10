# Inline text spacing and retained text items

These are component geometry checks, not website or pixel acceptance.
`capture.py` measures original fixtures in Olo Chrome on an owned Agent
Screen at actual live DPR1,1.25,1.5,1.75,2. It captures80 observations:
12 sequential restyles of the plain-font document and4 of the ligature-font
document, with eight element rectangles each. It waits for the web font.
The original fonts are from `../inline-fixtures`; hashes of the font,
HTML and cases accompany the Chrome version and observations.

The native release test compares x/y/width/height with a0.02 CSS-pixel
absolute allowance, and checks exact equality between reused and fresh
measurement after every restyle. Cases cover positive/negative letter and
word spacing, parent overrides, nested sizes and em values, wrapping,
inline edges, nowrap, RTL and restoration. The two paragraphs intentionally
share text but differ in spacing to exercise the shared preparation cache.

The ligature fixture keeps `fi` across a transparent inline element, then
forces it onto separate lines with parent and child letter spacing. The
reshaped glyphs must retain their spacing. Optional-ligature suppression for
nonzero letter spacing and arbitrary Unicode spacing remain outside this
fixture's coverage; this does not assert complete CSS Text conformance.

To recapture, start Olo Chrome with a debugging port on an owned screen,
then run from this directory:

```
/usr/bin/python3 capture.py --screen YOUR_SCREEN --port YOUR_PORT
```

The driver checks the Chrome window's process/port/profile, creates a
background tab, restores display settings and closes that tab when done.
It never uses native output to construct expected geometry.
