# Native logical-property ports

Nine unchanged WPT resources are pinned at
`d7bb5bb848fcd304c10669288a211dbafd106d90` (BSD license included).
Four box-property documents plus their two shared helpers supply the literal
mapping tables, values and assertion loops for margin, padding, inset and
preferred/minimum/maximum sizing. Three documents cover deferred writing mode,
variables and shorthand rollback. This is a selected subset of CSS logical WPT.

`extract.py` ports 5,726 numeric assertions without executing JavaScript or
reading engine output. It reads the original literal writing-mode tables,
transcribes the helper's assertion loops, and preserves literal expected
values in the three specialized documents. It creates the explicit inert DOM
that the helper would append. Each mutation is an independent frozen input.
The 31 specified-value/shorthand serialization drivers remain unsupported.
No unsupported driver is counted as a pass.

The shared native cascade adapter uses original HTML, stylesheet/selector,
property/variable and paired-cascade implementations. Parent axes are computed
before child projection. An inherited result uses its declared source with
the parent's axes. Numeric references use independent decimal parsing; no
engine-generated goldens are used. This does not run JS, CSSOM, live mutation,
font metrics, layout, shaping, rasterization, UI or websites. It supplies no
Chrome compatibility evidence or all-site dark-mode evidence.

`SOURCE.json` hashes all local/original/derived files except itself. Re-running
the extractor must reproduce `derived.json` exactly. The inventory is 5,757
native assertions/drivers, not 5,757 complete upstream browser tests.

```sh
/usr/bin/python3 crates/web/tests/logical-fixtures/extract.py
cargo run --release -p olo-web --example logical_eval -- /var/tmp/logical-report.json
```

The evaluator exits 1 while any inventoried driver is unsupported.
