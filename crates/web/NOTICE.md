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
