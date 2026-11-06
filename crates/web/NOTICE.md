# Third-party data and test fixtures

`src/html/entities.rs` is generated from the WHATWG HTML Standard's
[named character reference data](https://html.spec.whatwg.org/entities.json),
copyright WHATWG, distributed under
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
The generator records the source SHA-256 and date. It removes the leading
ampersand from keys and emits Rust string literals; values are unchanged.

The engine code is Olo's own implementation, under the workspace license.

`tests/html5lib` contains unmodified tokenizer fixtures from
[html5lib-tests](https://github.com/html5lib/html5lib-tests), revision
`c5495c539ca8a79c3c5107f5f14010bcff9a35d5`. Copyright (c) 2006–2013
James Graham, Geoffrey Sneddon and other contributors. MIT license,
reproduced in `tests/html5lib/LICENSE`. XML infoset coercion is outside
the HTML tokenizer scope. Non-scalar UTF-16 inputs and unsupported states
are reported explicitly; passing token comparisons does not imply
parse-error diagnostic conformance.

`tests/wpt-tree` contains unmodified tree-construction fixtures from
[Web Platform Tests](https://github.com/web-platform-tests/wpt/tree/d7bb5bb848fcd304c10669288a211dbafd106d90/html/syntax/parsing/resources),
revision `d7bb5bb848fcd304c10669288a211dbafd106d90`. Copyright WPT
contributors, BSD-3-Clause, reproduced in `tests/wpt-tree/LICENSE.md`.
The fixture format is in that directory's README.md. Tree comparisons
do not yet validate parse-error counts/codes/locations. Unsupported
algorithms/JavaScript/fragment cases remain explicit non-passes.


`tests/css-parsing-tests` contains unmodified generic CSS syntax fixtures
from [css-parsing-tests](https://github.com/SimonSapin/css-parsing-tests),
revision `203ce36bffd617db7f118c551e32794561fb273d`, written by Simon Sapin
and dedicated under CC0; LICENSE and README.rst are included. SOURCE.json
records every imported byte hash. This older corpus uses legacy delimiter
serialization and global unicode ranges. The native adapter documents its
format mapping and 12 exact, reviewed current-specification expectations;
all 28 byte-decoding fixtures are exercised. This is not CSS property,
selector, cascade or rendering conformance.

`tests/css-wpt` contains two unmodified CSS syntax WPT resources from the
same pinned WPT revision above, with BSD-3-Clause license and byte hashes.
Native tests port their syntax assertions/data, not their JavaScript,
CSSOM or browser execution. The identifier resource has a missing comma
in its JavaScript range list; the native test extracts its data linewise
and does not claim to execute that page.


`tests/selector-fixtures` imports unmodified cssselect2 test code/vectors
and static documents at `fc22accd5fdba764d95263fa8e6c97ce24f75f3e`.
CSSSELECT2-LICENSE is BSD-3-Clause; the WPT-derived vectors/documents have
the upstream tests/LICENSE notice and full WPT-LICENSE.md. An+B.json and
CC0-LICENSE use the same pinned css-parsing-tests revision above. The local
extraction script never executes a selector engine or creates expected
results from Olo. Its README and byte hashes distinguish literal native
ports/static XML models from upstream Python/JS, XML loading, CSSOM,
HTML parsing, rendering or full selector conformance. Production selector
compilation/matching is original Rust; no cssselect2 engine is embedded.

`src/css/color/named.rs` and `matrices.rs` contain normative named-color
and matrix facts from [CSS Color 4](https://drafts.csswg.org/css-color-4/),
sections 6.1 and 19, retrieved 2026-11-05 (source HTML SHA-256 recorded in
both files). Attribution: W3C and the CSS Working Group, under the
[W3C permissive document license](https://www.w3.org/copyright/document-license/).
The numeric tables are extracted facts; parsing, contextual resolution,
conversion control and SIMD batching are original Rust, not an embedded
color engine.
`tools/browser/generate-color-data.py` reproduces both fact tables from
the exact pinned HTML. It accepts only numeric/list arithmetic data,
never executes JS, and does not inspect engine outputs or test references.
Format its output with the repository's rustfmt.toml before comparison.

`tests/color-fixtures` contains thirteen unmodified color resources from
the pinned CC0 css-parsing-tests revision above, nine unmodified WPT color
resources from the pinned BSD-3-Clause WPT revision above, and an unmodified
MIT-licensed Color.js conversion test resource at
`dbbaf2de8ee5a6ac82a335711230d24dd44fc16a` (Copyright (c) 2021 Lea Verou,
Chris Lilley).
The three licenses are included. Its README, SOURCE.json and local
literal-only extractor describe the numeric native ports, reviewed
current-spec clamping, selections and explicit non-passes. No upstream
Python/JS library, CSSOM or WPT page is executed by these comparisons.

`tests/numeric-fixtures` imports 23 unmodified CSS Values WPT resources
and the unmodified numeric-testcommon.js helper from the same pinned WPT
revision above, under its included BSD-3-Clause license. SOURCE.json pins
all bytes; README.md documents literal native ports, explicit representative
host environments, helper expansion, numeric reference decoding and
unported cases. No upstream JS/CSSOM/page engine executes in the tests.
Production typed arithmetic is original Rust based on CSS Values 4/5
and the Typed OM numeric type rules, not an embedded third-party CSS engine.

`tests/variable-fixtures` imports 25 unmodified WPT CSS Variables resources
from revision `d7bb5bb848fcd304c10669288a211dbafd106d90` (BSD-3-Clause,
included `LICENSE.md`). Its README, literal extractor, SOURCE.json and
inventoried non-passes document the native grammar/cycle/substituted-name
ports. No upstream JS or page executes, and no third-party variable or
style engine is embedded. Production substitution and token assembly are
original Rust based on CSS Variables and CSS Values/Syntax.

The 126 property grammar/expansion/computed-reference WPT resources in
`tests/property-fixtures/wpt` are unmodified at the revision recorded in
`tests/property-fixtures/SOURCE.json`. The BSD license is reproduced beside
them. Literal extraction/native adapters are original Olo code; these ports
do not execute WPT's JS, CSSOM, style or layout drivers.

The 19 cascade test/reference resources in `tests/cascade-fixtures/wpt` are
unmodified WPT data at the revision in `tests/cascade-fixtures/SOURCE.json`.
Their BSD license is reproduced there. Native literal ports explicitly omit
JS/CSSOM, animations, layout and rasterization; a reftest's background-color
component does not establish that its rendered reference matches.

The nine logical-property documents/helpers in `tests/logical-fixtures/wpt`
are unchanged WPT data at the revision in `tests/logical-fixtures/SOURCE.json`.
The BSD license is reproduced there. Native ports retain literal mappings,
values and assertion loops; CSSOM drivers remain explicit non-passes. No JS,
live mutation, layout, text shaping or rasterization is executed by these ports.

The nine computed-style documents/helpers in `tests/style-fixtures/wpt` are
unchanged WPT resources at the revision in `tests/style-fixtures/SOURCE.json`,
under the included BSD license. The original literal-only extractor and native
adapter run Olo's document style job against selected semantic assertions.
Unimplemented contexts and browser drivers remain explicit non-passes. No
upstream JS, CSSOM serializer, layout or rasterization executes in these ports.

The nine font longhand pages and two helpers in `tests/font-fixtures/wpt` are
unchanged WPT data at the pinned revision in SOURCE.json, under the included
BSD license. SOURCE.json maps their original paths. Native literal grammar
ports check acceptance; selected computed ports run the original document
style job with supplied metrics. Missing original contexts and browser drivers
remain non-passes. No font files, JS, CSSOM serializer, face matching, shaping,
layout or rasterization is executed by these assertions.

Native font matching uses original Rust algorithms and ttf-parser 0.25.1
(MIT/Apache-2.0) for OpenType binary table access. The generated fonts in
`tests/font-native-fixtures/fonts` are original literal geometry/metadata under
the repository license; the independent generator uses fontTools 4.60.1.
Existing Noto data used by references stays under its SIL OFL in `data/fonts`.
Unicode 18.0.0 default case-fold facts are generated from the unchanged normative
CaseFolding.txt in `tests/font-native-fixtures/ucd`, with Unicode License V3
included there. SOURCE.json records the data hashes and retrieval provenance.


Native glyph shaping uses HarfRust 0.15.0 (MIT), read-fonts 0.46.0
(MIT/Apache-2.0), and unicode-bidi 0.3.18 (MIT/Apache-2.0), with our pinned
Unicode 18 property data replacing the older bundled bidi table. Original
forward grapheme segmentation and source/font/cache orchestration are Olo code.
Unicode properties and unchanged official tests in tests/shaping-fixtures/ucd
follow the included Unicode License V3. The four unmodified Noto fonts under
shaping-fixtures/fonts retain each family's OFL license and upstream provenance
in SOURCE.json. Native HarfBuzz 15 C API and uharfbuzz 0.55.0 (bundled 14.2.1)
are independent test-data oracles only; their libraries are not embedded into
this engine. Glyph data generation never executes Olo. Font-unit vectors and
Unicode checks are not layout/rasterization/CSS/website conformance claims.
