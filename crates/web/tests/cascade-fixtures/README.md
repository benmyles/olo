# Native cascade ports

The 19 unchanged WPT resources under `wpt/` are pinned at
`d7bb5bb848fcd304c10669288a211dbafd106d90`, with BSD license reproduced here.
Selection includes literal basic/important/nested layer cases, inline style,
revert-rule combinations/chains/custom properties/cycles, and selected
revert-layer green-square references. This is not the entire cascade suite.

`extract.py` does not execute JavaScript. It ports complete literal testCases
arrays and their two target classes, literal ID/property assertions, the
alias-based layers-basic references, and **only the background-color component**
of eight reftests. The unchanged reference explicitly specifies green. Those
eight geometry/rasterization drivers remain separate unported cases. Source
locations, array case/title, styles, expected values and reference provenance
are retained. There are 128 native assertions and eight unported drivers.

The Rust adapter parses inert source HTML with the original engine, compiles
the extracted styles in document order, selects actual target elements,
applies inline declarations, and uses the production cascade/defaulting and
variable resolver. It recursively provides already-computed parent custom
properties and resolves inherited colors, so parent expressions are not
reevaluated against a child's context. All stylesheet issues except ordinary
invalid syntax/declarations make the port unsupported. Unknown/unimplemented
properties or at-rules cannot silently manufacture a pass.

Expected RGB/decimal values use independent literal parsing. Custom-reference
ports compare canonical decoded identifier tokens, not CSSOM serialization.
The numeric host explicitly supplies a fixed 16px root-font factor for the
4rem reference; no font metrics/layout are implemented. The color host uses
Light with CanvasText black/Canvas white. These supplied native contexts are
not the person's theme, browser defaults measurement or all-theme evidence.

**128 passes and eight explicit unsupported drivers**, no unexpected
mismatch/error/panic. These checks do not execute JS, CSSOM, animations,
shadow DOM, layout, rasterization or a website. A green background token is
not a rendered green square. The tentative rollback cycle resource remains
identified as tentative rather than asserted to settle every future spec case.
Every original/local/derived byte is checked against `SOURCE.json`.

```sh
/usr/bin/python3 crates/web/tests/cascade-fixtures/extract.py
cargo run --release -p olo-web --example cascade_eval -- /var/tmp/cascade-report.json
```

The evaluator exits 1 until all inventoried cases are supported. The native
test rejects unexpected mismatch/error/panic and checks the complete counts.
