# Independent selector fixtures

Unmodified `test_cssselect2.py`, `w3_selectors.py`, `content.xhtml`,
`ids.html`, `shakespeare.html` and `LICENSE` come from Kozea/cssselect2
revision `fc22accd5fdba764d95263fa8e6c97ce24f75f3e`. The test code is covered
by the project BSD-3-Clause license in CSSSELECT2-LICENSE. The older W3
selector vectors/documents are WPT-derived under BSD-3-Clause, documented
by the upstream tests/LICENSE; the full WPT license is WPT-LICENSE.md.

`An+B.json` and CC0-LICENSE come from SimonSapin/css-parsing-tests revision
`203ce36bffd617db7f118c551e32794561fb273d`. They contain 128 independent
microsyntax inputs with explicit expected coefficients or invalidity.

`generate.py` uses Python's literal AST extraction and ElementTree to
produce `derived.json`. It does not import pytest/cssselect2, execute their
parser/matcher, or generate expected matches from Olo. Run it with
`/usr/bin/python3 crates/web/tests/selector-fixtures/generate.py`.
The namespace-test nodes and inclusive Shakespeare body scope reproduce
the upstream setup. Comments and text are preserved. SOURCE.json records
all upstream and derived byte hashes, including the extraction script.

The native adapter evaluates 463 literal query/invalid-selector cases,
including the original exclusions and upstream xfail annotations. No xfail
is silently accepted: actual mismatches/errors/panics fail the unit gate.
A fragment-only vector uses a virtual root with no outside ancestors.
The XML fixture trees test the generic tree interface, not XML loading,
HTML parsing, JS/CSSOM, rendering, or upstream Python execution. Native
DOM integration is tested separately against Olo's actual document arena.

Two exact modern grammar adjustments are explicit in the native adapter:
`test_select` 69 and 71 pass `of .e` to nth-of-type/nth-last-of-type. The
Selectors 4 grammar has no `of` clause on these two pseudo-classes; their
modern expectation is invalid. The original upstream expectations remain
unchanged and are recorded in the report.

Current gate: 432 direct matches, two modern invalidity expectations,
29 explicit non-passes. Missing live state, shadow/functional pseudo-
elements, nesting, one HTML fixture context and one upstream matches-only
scope remain unsupported. Those cases stay in the evaluator's denominator,
and `selector_eval` exits 1 until every imported case is actually checked.
The unit gate protects all implemented cases and the exact unsupported
count; it does not represent complete Selectors conformance.
