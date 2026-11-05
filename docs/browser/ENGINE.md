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
framesets, table modes and foster parenting, active formatting reconstruction
and adoption, and doctype
mode selection. Initial `selectedcontent` contents are cloned from the
selected option. Live selection state, events and mutation hooks remain.
SVG/MathML document parsing includes namespace-sensitive scope/adoption,
HTML/MathML integration points, self-closing foreign elements, CDATA and
foreign spelling/attribute adjustments. Template document parsing redirects
insertions into owned content roots,
tracks nested insertion modes, and unwinds unclosed templates at EOF.
Declarative shadow roots in documents and template content patching still
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
NOTICE.md pins the source revision. Current coverage is **3,898/3,904
static tree cases**, with **six unsupported requiring JavaScript execution**, not passed. Both scripting
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

Examples `tokenize_bench`, `dom_bench`, `append_bench`, `parse_bench`,
`table_bench`, `foreign_bench`, `template_bench` and `fragment_bench`
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

Table buffering merges contiguous source spans without copying. Decoded or
discontiguous runs use one growing buffer charged against the owned-text
quota, and fostered text merges with the preceding text node. The table
dispatcher and foster insertion paths stay separate from ordinary inserts:
release comparison caught a compiler inlining/stack regression when merged.
See `measurements/2026-11-05-table-common.json` and
`measurements/2026-11-05-tables.json` for every observation.

Foreign attribute namespace bits share the interned-name key: attribute
slots remain 16 bytes, node slots 40 bytes. The name arena bounds IDs to
30 bits. `attributes_ns` returns qualified/local names, prefix, value and
recognized XLink/XML/XMLNS identity; `attribute_ns` looks up expanded
identity. Qualified-name reads/writes/removal and cloning preserve the
stored namespace. These are storage primitives: arbitrary XML namespace
URIs, namespaced Web IDL setters and XML parsing remain unfinished.
Tokenizer CDATA permission follows current-node changes; text and ignored
tokens reuse it. Foreign dispatch is separate from HTML mode reprocessing.

A template has a distinct content fragment exposed by `template_contents`.
That fragment's DOM parent is null; ordinary children remain separate.
A compact template variant and fragment host link keep node slots at
40 bytes without a side table. Ownership-aware iterative cloning and
reclamation include both content and ordinary children; pre-insertion
cycle/depth checks include the host relationship. Owned content roots
cannot be destroyed independently of their host. Both allocations are
preflighted against node/revision quotas. Inert owner-document/realm
semantics and web-facing template APIs remain unfinished.


## Standalone fragments

`parse_fragment(Arc<str>, &FragmentContext, ParseOptions, cancellation)`
returns `ParsedFragment { document, fragment, quirks }` in an independent
arena. `FragmentContext::new(name, namespace)` describes an unattached,
attribute-free element; `with_quirks` selects its document mode.
`FragmentContext::from_element(document, element, quirks, cancellation)`
borrows an immutable metadata snapshot, including expanded attribute
identity and form ancestry. It is cancellable; debug output omits values.
Build the snapshot and parse on workers. The source document stays untouched.

Document and fragment parsing share the token loop. Fragments use the
adjusted current node for foreign dispatch/CDATA and the context for mode
reset, with form/quirks inheritance and select/frameset/HTML fragment rules.
The initial RCDATA context has no appropriate end tag. Initial raw/script
contexts use the HTML Standard's permitted plaintext equivalent while
error diagnostics are not collected. Literal closing-tag text cannot end
that initial context. Declarative shadow roots default to disabled in
this inert API, so their markup remains an ordinary template; there is no
allow-shadow-root option yet. Script execution stays inert.

The returned fragment has a null parent and the arena's document root is
empty. Temporary context/root nodes are reclaimed; retained source spans
belong to the returned arena. Cross-document import/adoption, live mutation
hooks, Web IDL innerHTML/setHTML APIs, script execution and real owner-
document/realm semantics remain separate unfinished integration.
