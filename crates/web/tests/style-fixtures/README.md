# Native computed-style ports

Nine unchanged WPT documents/helpers are pinned at
`d7bb5bb848fcd304c10669288a211dbafd106d90`; the BSD license is included.
`extract.py` ports literal computed-value assertions, relative-weight inputs
and the display blockification helper's literal assertion loop. It executes
no JavaScript and reads no engine-generated expected values.

There are 316 native assertion inputs and 25 separate unported browser
drivers. The adapter parses original inert HTML, applies the ported mutation,
compiles original context styles, and runs the production document style job.
Parents, root fonts, logical mapping, inherited values, stylesheet matching,
inline declarations and computed values use the original engine. Unsupported
context declarations cannot disappear to manufacture a pass. The font-weight
and line-height resources currently require unimplemented CSS container
contexts, so all 73 of their native assertions stay unsupported.

**243 passes, 98 unsupported assertions/drivers, no unexpected mismatch,
error or panic.** These are selected native semantic/resolved-value ports,
not 341 complete upstream browser tests or CSSOM serialization tests. Display
references compare independent keyword/box tuples; numbers use decimal
parsing; shorthand references compare individual values. Percentage references
stay deferred and are checked at two independent bases. Letter-spacing's
resolved `normal` reference maps to computed zero under current CSS Text.
Implicit function closure at EOF is preserved when applying a CSSOM input.

The host explicitly supplies medium=16px, scale=1, Light CanvasText black /
Canvas white, and numeric metric ratios. Passing cases do not use the fake
metric ratios to manufacture font-dependent expectations; font selection,
keyword policy, presentational hints and browser/CSSOM drivers stay unported.
No layout, shaping, rasterization, live mutation or website evidence follows.

Every original/local/derived byte except `SOURCE.json` is hashed by that
manifest. Re-running extraction must reproduce `derived.json` exactly.

```sh
/usr/bin/python3 crates/web/tests/style-fixtures/extract.py
cargo run --release -p olo-web --example style_eval -- /var/tmp/style-report.json
```

The evaluator exits 1 while any inventoried assertion/driver is unsupported.
