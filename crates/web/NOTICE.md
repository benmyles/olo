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
