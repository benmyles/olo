# Native font fixtures

`generate.py` creates original fonts from literal rectangle geometry, names,
OpenType metadata and variation masters using **fontTools 4.60.1**. It does not
read Olo output. The small generated fonts follow the repository license; no
third-party font outlines are copied into them. There are 20 font files:
static width/style/weight candidates; distinct first/reference fallback faces;
missing-height simple and composite outlines; localized/caseless names and
distinct canonically equivalent names; a shared-table TTC; an optical variable
font with HVAR/MVAR; the same font without HVAR (gvar phantom points); and a
metadata-only four-axis face with an intentionally extended weight range.

`optical-references.json` contains independent fontTools integer instancer
readings. `raw-references.json` independently evaluates HVAR/MVAR at normalized
2.14 coordinates, preserving fractional deltas. Normalization is truncated to
2.14 to match the table decoder's input representation. Neither represents
shaping, rasterization or whole-font sanitization. The real Noto references read
the existing `data/fonts/NotoSans[wght].ttf`, check its SHA-256, and do not copy
it. That repository file reports **wght 400–700**; the 900 request deliberately
tests clamping. Noto's license/provenance are in `data/fonts/{OFL.txt,README.md}`.

`ucd/CaseFolding.txt` is the unmodified Unicode **18.0.0** normative input,
retrieved 2026-11-05 from
<https://www.unicode.org/Public/UCD/latest/ucd/CaseFolding.txt>. Its header
identifies the version; SOURCE.json pins its hash. `UNICODE-LICENSE.txt` is the
Unicode License V3 from <https://www.unicode.org/license.txt>. Only default full
C/F records generate `src/font/casefold_data.rs`; no Turkic tailoring or Unicode
normalization. The unit test independently reads all 1,606 original records.

Run generators from the repository root. Use an isolated environment under
`/var/tmp`, with fontTools pinned as above. Do not install into Olo's session:

```sh
/var/tmp/OWN-DIRECTORY/venv/bin/python crates/web/tests/font-native-fixtures/generate.py
/usr/bin/python3 tools/browser/generate-casefold-data.py
cargo fmt -p olo-web
cargo test --release -p olo-web --test font_native
```

SOURCE.json records hashes of the checked-in files. Fixed font timestamps and
removed intermediate masters make regeneration deterministic.

The owned Chrome probe is checked in at
`docs/browser/measurements/2026-11-05-native-font-chrome.{html,json}`. Serve the
HTML with `fonts/optical.ttf` and the existing Noto file as `fonts/NotoSans.ttf`,
and open it via `olo-screen run OWN-SCREEN olo-chrome LOCAL-URL`. Its structured
paragraphs let `olo-screen tree` collect results without CDP or the person's
browser. Both faces must report loaded. It tests auto/none/explicit opsz at four
sizes. The native comparison covers 24 auto-sizing metric observations, with an
explicit one-font-unit plus 1/64 CSS-pixel tolerance for the reference's integer
rounding/layout quantization. It is not an exact-rendering or screenshot pass.
