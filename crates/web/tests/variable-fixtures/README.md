# Independent custom-property ports

The 25 unmodified resources under `wpt/` come from
[WPT css/css-variables](https://github.com/web-platform-tests/wpt/tree/d7bb5bb848fcd304c10669288a211dbafd106d90/css/css-variables),
revision `d7bb5bb848fcd304c10669288a211dbafd106d90`, BSD-3-Clause
(`LICENSE.md`). Selection was pinned before evaluating the engine.
`SOURCE.json` authenticates imported, local and derived files.

`/usr/bin/python3 crates/web/tests/variable-fixtures/extract.py` reproduces
`derived.json` using literal parsing only. It ports the parsing helper calls,
cycle declarations/expected lists (including their helper's sanity check),
and literal computed() inputs/references from the substituted-name resource.
It does not run JS or Olo to construct expectations. Other selected resources
retain each test call form, or their document/reftest driver, as an explicit
unported item. These are native assertions/driver forms, not an upstream WPT
page or subtest count.

The native host supplies the resource's literal target/base declarations and
one inline declaration block as a winning snapshot. It honors duplicate
order/importance within that provided block, without claiming selectors,
cascade origins/layers, CSSOM or layout. Cycle groups use one memoizing
resolver for all original property queries. Name tests compare the sole
canonical numeric/ident token to the literal expected string; no engine
parser/converter generates the reference. Their initialWidth reference is
ported only as the expected guaranteed-invalid substitution, not as a
measured width or layout result. Registered-property and other missing host
contexts remain explicit non-passes. Unimplemented arbitrary substitution
functions report Unsupported, not ordinary missing variables.

Current inventory: 65 parsing/name cases, 60 assertions in eleven cycle
groups and 26 unported drivers. All remain in the evaluator denominator.
Current outcome: **119 passes and 32 explicit non-passes**, with no unexpected
mismatch/error/panic. Six non-passes require attribute/conditional/random/
registered-property hosts; 26 are unported drivers. The evaluator exits 1
until all are supported. Native validity
ports validate generic declaration/var() argument grammar, not the complete
grammar of the surrounding width property. Empty values remain distinct
from guaranteed-invalid. No website rendering or compatibility follows.

Primary references consulted 2026-11-05:
[CSS Variables 1](https://drafts.csswg.org/css-variables-1/),
[CSS Values 5 substitution](https://drafts.csswg.org/css-values-5/#arbitrary-substitution),
[CSS token serialization boundaries](https://drafts.csswg.org/css-syntax-3/#serializing).
