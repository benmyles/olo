# Engine implementation and checks

The engine crate is `olo-web`. It currently parses supported static HTML
into its own compact document storage. It has no network, CSS/layout,
JavaScript execution, browser shell or demonstrated website compatibility.

## Parse API

`html::parse(Arc<str>, ParseOptions, Option<&AtomicBool>)` accepts a
complete UTF-8 buffer and returns `ParsedDocument { document, quirks }`.
Run it on a worker. It enforces input/token/attribute/DOM/stack/formatting
limits and checks cancellation during token processing, reprocessing,
control initialization and subtree cloning. Input decoding, incremental
network parsing and script execution are not implemented yet.

Implemented insertion rules include document/head/body, raw text/RCDATA,
plaintext, noscript's scripting flag, lists, forms, buttons, ruby,
framesets, active formatting reconstruction and adoption, and doctype
mode selection. Initial `selectedcontent` contents are cloned from the
selected option. Live selection state, events and mutation hooks remain.
Tables, SVG/MathML foreign parsing, templates and context fragments still
return explicit unsupported results; they cannot be compatibility passes.

`dom::Document` is storage, not the complete web-facing DOM API. Nodes
have scoped generational handles and flat 40-byte slots. Text/attribute
values reference the shared source where unchanged, own decoded buffers
when necessary, and promote to capacity-accounted growing strings for
appends. `clone_subtree` is iterative and keeps source ranges; owned
buffers remain independent. Node deletion and cloning avoid recursive
Rust tree ownership. The future V8/agent layers must preserve rooting,
origin/frame boundaries and secret masking around these primitives.

## Independent tree coverage

Unmodified WPT fixtures and their license are in `crates/web/tests/wpt-tree`;
NOTICE.md pins the source revision. Current coverage is **2,484/3,904
static tree cases**, with **1,420 unsupported**, not passed. Both scripting
flags run where the fixture calls for them. Parse-error diagnostics are
not checked. A native evaluator always exits nonzero until every static
tree case matches:

```sh
cargo run --release -p olo-web --example tree_eval -- /var/tmp/tree-report.json
```

Default `cargo test --release -p olo-web` checks all emitted trees for
mismatches/errors/panics and protects every previously matched case in
`tests/tree-coverage.json`. It still reports unsupported cases explicitly.
It is a regression gate, not an exemption for full conformance. This
command also requires all static cases to be supported:

```sh
OLO_TREE_REQUIRE_COMPLETE=1 cargo test --release -p olo-web wpt_static_tree_construction
```

The same distinction applies to tokenizer coverage: 7,042 emitted-token
cases match, four UTF-16 cases are unsupported, diagnostics unfinished.
Neither set of counts establishes website rendering, JS or DOM API support.

## Reproducible release measurements

Examples `tokenize_bench`, `dom_bench`, `append_bench` and `parse_bench`
emit JSON; run with `cargo run --release -p olo-web --example NAME`.
Pin to the same CPU and alternate baseline/default runs. Raw samples,
commands, machine and scope are recorded under `measurements/` and in
JOURNAL.md. Baseline features are measurement controls, absent by default:

- `scalar-scans`: portable byte scans instead of memchr's SIMD scans.
- `baseline-storage`: second text copy and heap allocation for a single insertion.
- `baseline-parser`: repeated attribute lookup and quadratic formatting-attribute comparison.

The measured parser stress cases motivated linear initial-attribute
construction and hashed comparison of large formatting-attribute sets.
Small tags keep their simpler path. These are static in-memory engine
measurements, with no network, CSS, layout, JS, UI frames or Chrome comparison.
