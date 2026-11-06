# Native font longhand ports

Nine unchanged HTML resources and two helpers are pinned at WPT revision
`d7bb5bb848fcd304c10669288a211dbafd106d90`, under the included BSD license.
SOURCE.json maps original paths and hashes every original/local/derived file
except itself. `extract.py` reads literal assertions and expands the original
width resource's two-property loop; no upstream JS or font engine executes.
Re-extraction must reproduce derived.json exactly.

**140 native assertions plus nine unported browser drivers: 137 pass, 12
unsupported, no unexpected mismatch/error/panic.** Parsing assertions check
acceptance only, not CSSOM serialization/round-trip. The computed adapter
applies the ported input to original inert HTML, compiles original context
styles and runs Olo's production document style job. Independent canonical
family references preserve named/generic identity; width references compare
percent quantities and style references compare keywords/degree angles.
Expected values never come from the engine parser or serializer.

Recognizing the original container-type contexts enables44 additional style/
width assertions; three literal assertions and nine drivers remain unsupported.
The adapter does not ignore context declarations even for constant examples. Ten family
computed assertions pass with supplied numeric test metrics; no assertion
claims actual font loading, matching, shaping, CSSOM, layout or rasterization.
Browser drivers are separate explicit non-passes. The native host supplies
medium=16px, scale=1 and Light CanvasText/Canvas; it ignores family/style/width
for test metrics, since passing family assertions concern identity only.

Selected local Chrome comparisons are separate observations; that build lacks
font-width and newer generic/style values. They are not counted as upstream
WPT passes. The full browser and 100-site acceptance remain open.

```sh
/usr/bin/python3 crates/web/tests/font-fixtures/extract.py
cargo run --release -p olo-web --example font_eval -- /var/tmp/font-report.json
```

The evaluator exits 1 while any inventoried assertion/driver is unsupported.
