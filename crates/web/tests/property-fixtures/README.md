# Typed property native ports

The 126 HTML resources under `wpt/` are unmodified Web Platform Tests at
`d7bb5bb848fcd304c10669288a211dbafd106d90` (BSD license beside this file).
Selection covers valid/invalid/computed/shorthand resources for the box,
sizing, display, flex, gap, border, opacity, font size/weight and positioning
properties implemented by this stage. Both older box and modern sizing
resources are retained. This is a selected property corpus, not all of WPT.

`extract.py` reads literal helper calls without running JavaScript. It emits
1,090 native cases: 683 declaration-acceptance assertions, 381 computed
references and 26 shorthand maps. Nineteen nonliteral helper/driver forms
remain inventoried as unported. Source locations and original expected
serializations/arrays/maps are retained in `derived.json`.

The acceptance ports check declaration grammar and reject invalid values.
They do **not** check CSSOM serialization, even when the upstream helper
also does so. Shorthand ports check the actual longhand identities and
independent literal values. The numeric/computed ports interpret canonical
number/px references with Rust decimal parsing, use no font/viewport/percent/
tree context, and apply the property's range/integer rules. Simple keyword
and absolute rgb references are also compared independently. Reference
strings are never fed into the engine's property/color/math parser.

Missing font metrics, percentage computation, intrinsic/used sizes, coupled
overflow/positioning, compound computed values, display CSSOM and live
cascade/element contexts stay unsupported. Extra JS setup, nonliteral calls
and document drivers also stay unsupported. Parsing a display value does
not demonstrate that its box is generated or laid out. Imported sources
are not changed to manufacture matches; every byte is verified against
`SOURCE.json`. This suite is not JS, layout, CSSOM, rendering or page execution.

Reproduce from the repository root:

```sh
/usr/bin/python3 crates/web/tests/property-fixtures/extract.py
cargo run --release -p olo-web --example properties_eval -- /var/tmp/property-report.json
```

The evaluator exits 1 while any inventoried case is unsupported or fails.
A native test checks the exact coverage counts and rejects unexpected
mismatch/error/panic. The browser journal and coverage artifact record the
current results and remaining cases; no website pass is inferred from them.
