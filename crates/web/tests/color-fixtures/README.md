# Independent color data

These are data ports for native grammar/coordinate tests, not execution of
upstream JS, WPT pages, CSSOM serialization, layout or browser rendering.
`SOURCE.json` pins byte hashes and repository revisions. Imported resources
are unmodified, including whitespace. The production engine is original Rust.

- Thirteen `color_*.json` files from Simon Sapin's
  [css-parsing-tests](https://github.com/SimonSapin/css-parsing-tests/tree/203ce36bffd617db7f118c551e32794561fb273d),
  CC0 (`CC0-LICENSE`): 8,061 input/expected pairs. All are accounted for.
- Nine resources under `wpt/` from
  [WPT css/css-color/parsing](https://github.com/web-platform-tests/wpt/tree/d7bb5bb848fcd304c10669288a211dbafd106d90/css/css-color/parsing),
  BSD-3-Clause (`WPT-LICENSE.md`). The local literal extractor ports 4,598
  assertions and retains one unported direct expectation in the denominator.
- Unmodified `colorjs-conversions.js` from Color.js's
  [test/conversions.js](https://github.com/color-js/color.js/blob/dbbaf2de8ee5a6ac82a335711230d24dd44fc16a/test/conversions.js),
  MIT (`COLORJS-LICENSE`). Forty-three standard CSS absolute coordinate
  vectors are selected by input contract, before running Olo. The 212 other
  objects are inventoried in `derived-conversions.json`: non-CSS spaces or
  conversion hooks, custom-space aliases lacking a CSS profile, and missing-
  component propagation (a different contract from explicit absolute
  conversion). This selection does not establish whole-library conformance.

Run `/usr/bin/python3 crates/web/tests/color-fixtures/extract.py` to reproduce
both derived JSON files. It parses only literal data and known literal
color-space template environments. It never runs JS, a color library, or
the engine, and never creates golden values from Olo output. Original line
numbers/inputs identify the independent assertions. The one unported WPT
expectation (`rgb(2.5,3.4,4.6)`) uses an array of allowed serializations.

`support/color_fixtures.rs` independently decodes restricted canonical
numeric reference strings. It does not call the engine parser or converter
to generate expected coordinates. Generic comparison tolerance is
`1e-6 + abs(reference)*1e-7`; alpha allows `6e-7` for independently rounded
six-decimal hex references plus f32 storage. WPT legacy RGB assertions compare
the explicit clipped byte endpoint and alpha, not CSSOM text. Other computed
colors compare space, coordinates and missing mask. Validity-only assertions
check acceptance/rejection. Color.js vectors retain their own epsilon
(default `0.0001`), absent coordinates are skipped, and null denotes a
powerless/missing output component.

The old generic corpus predates current Lab-family parse-time lightness/
chroma clamps and hue normalization. The adapter transforms only those
numeric expectations according to CSS Color 4, independently of the engine:
L clamps to [0,100] for Lab/LCH, [0,1] for OKLab/OKLCH; chroma is nonnegative;
polar hues normalize to [0,360). The 2,760 affected cases are reported
`updated_specification`, separately from 9,728 direct passes. The adapter can
map obsolete Color 3 alpha-hex rejection to its identical input's Color 4
reference; none of this pinned subset needs that mapping.

Current total: 12,703 cases; 9,728 direct passes, 2,760 reviewed current-spec
passes, 215 explicit non-passes. Those gaps are 195 math expressions, two
substitutions, six custom-profile values, eleven device-color values and
one unported expectation. Unexpected invalid results, errors, mismatches
and panics fail regression tests. `color_eval` always exits 1 while any
case remains unsupported. These counts do not imply complete CSS Color
conformance, dark-mode adaptation or any working websites.

Normative references consulted 2026-11-05:
[CSS Color 4](https://www.w3.org/TR/2026/CRD-css-color-4-20261103/),
[CSS Color 5 light-dark](https://drafts.csswg.org/css-color-5/#light-dark),
[CSS Values 4 precision](https://drafts.csswg.org/css-values-4/#range).
