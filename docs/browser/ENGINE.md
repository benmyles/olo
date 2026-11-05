# Engine implementation and checks

The engine crate is `olo-web`. It parses supported static HTML into its
own compact document storage and constructs generic CSS syntax. It has no
network, style calculation/layout, JavaScript execution, browser shell or
demonstrated website compatibility.

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


## Generic CSS syntax

`css::Tokenizer` consumes complete UTF-8 input with source-byte spans,
preprocessing, escapes, strings, URLs, hash flags, numbers/units and typed
punctuation. Unicode-range tokens are disabled normally and enabled only
through the explicit descriptor-mode option. Strings/names borrow input
unless decoding is necessary. Scans have cancellation checkpoints and
bounds for input, tokens, decoded text, diagnostics and nesting. Diagnostic
labels/offsets are exposed; their complete standards conformance is not
established by the fixture comparisons.

`css::Syntax::parse(Arc<str>, Limits, cancellation)` constructs an immutable
24-byte-per-component preorder arena. Children skip subtrees; containers
never recursively own nodes. Source spans/decoded buffers are retained
once. `parse_owned(String, …)` retains a decoded buffer without copying it;
`source()` returns `&str` for either ownership form, and `memory()` includes
reserved source capacity. Numeric values use node storage; dimension metadata is a sparse
12-byte entry. `memory()` uses retained counters/capacities, not a tree walk.
Read-only component views borrow their owning arena; public consumers
cannot forge cross-arena indices. Debug output omits source/value strings.

Syntax methods expose generic stylesheet/rule boundaries, declarations,
ordered mixed block contents and single-item entry points. They preserve
source spelling, trim declaration whitespace, handle important suffixes,
discard at-rules in declaration lists, and distinguish custom-property
blocks from nested rules. Invalid markers remain available for diagnostic
inspection. These methods deliberately do not validate selector/property
or at-rule context grammars. Consumers must validate and compile them
before applying style. Blocks are parsed on demand and the style compiler
must retain results rather than repeat work during layout/painting.
Property grammars, unicode-range descriptor retokenization, selectors,
cascade/layers, custom-property substitution, media queries, invalidation,
CSSOM and incremental tokenization remain unfinished. Streaming byte decoding
is implemented below; it does not make the tokenizer incremental.

Independent fixtures are pinned in `tests/css-parsing-tests` (177 cases).
The old serialization adapter folds adjacent literal delimiter pairs and
compares numeric values across JSON integer/float representations, while
retaining numeric type and original spelling. It runs the old corpus with
unicode ranges enabled, explicitly exercising the optional descriptor
mode. Default mode is checked separately. **165 cases match**, including
all **28 byte-input cases**, and **12 use reviewed current-specification
expectations**. No imported generic-syntax case is skipped. Those 12 exact mappings are in `css_fixtures.rs`:
modern non-ASCII ident ranges, declaration whitespace/semicolon handling,
and discarded at-rules in declaration lists. No emitted result is used as
an expected value. Source hashes reject fixture edits.

`tests/css-wpt` pins the two WPT resources used to port native syntax
assertions for identifier ranges and nine whitespace/important examples.
The port does not run their JS, CSSOM or page rendering. Its README/license
and source hashes document the distinction and the missing comma in the
upstream identifier script's data list.

`css_eval` includes every imported case and exits nonzero until all are
checked against pinned/reviewed expectations. It currently exits 0 for this
177-case generic-syntax corpus only:

```sh
cargo run --release -p olo-web --example css_eval -- /var/tmp/css-report.json
```

`css_bench` measures tokenization and generic syntax/root statements only,
including mixed-rule scaling; it does not compute style or recursively
compile every rule's declarations. `scalar-scans` supplies equivalent
scalar string/comment searches. `baseline-css-parser` reproduces the old
mixed-block lookahead cost, with the same results. Default uses early
exits and single-pass declaration-value checks. Raw release observations:
`measurements/2026-11-05-css.json`. No Chrome, rendering or compatibility
performance follows from these microbenchmarks.


## Text and CSS byte decoding

`text::Decoder` is a worker-only push decoder shared by future resource
loaders. The caller selects a standard encoding and removes any initial
BOM. UTF-8 borrows validated runs, carries at most three unfinished bytes,
and uses `simdutf8` for ordinary runs with scalar malformed-sequence
recovery. Legacy codecs use the pure Rust `encoding_rs` library already
used elsewhere in Olo, with a fixed 16 KiB staging buffer. No external
programs, I/O, threads, timers or polling are added by decoding. Its
144-byte state (on this x86-64 build) allocates no UTF-8 heap buffer.

`DecodeLimits` separately bounds received bytes and produced UTF-8 bytes.
Cancellation is checked within 64 KiB input spans and between callbacks.
Invalid sequences use standard replacement behavior; `had_errors` records
it. Sinks borrow valid text and should not block. A final push, fatal error
or rejected sink offer is terminal. Progress counts received input (even
if a later error stops that push) and bytes offered to the sink; callers
that can reject an offer must track accepted bytes themselves. `collect`
provides quota-aware, amortized growth for an optional owned destination.

`css::ByteDecoder` adds encoding selection with at most 1024 sniffing
bytes. Precedence is BOM, valid protocol label, exact leading ASCII
`@charset "LABEL";` inside those first 1024 bytes, referring environment,
then UTF-8. UTF-16 charset labels resolve to UTF-8 as required by CSS.
Labels use the standard alias table; invalid labels fall through.
The selected encoding/provenance/BOM length are available separately from
text. A BOM is stripped; generic syntax retains encoding declarations.
The later style compiler must discard `@charset` rather than apply it.

`Syntax::parse_bytes` decodes and builds a complete generic component
arena. Its input limit bounds both raw bytes and decoded source bytes.
It reserves an initial collector buffer once after quota/cancellation
preflight, trims spare capacity after decoding, and moves it into shared
owned storage. This avoids the previous full `String` → `Arc<str>` copy;
`parse(Arc<str>, …)` still borrows its caller's shared source. The
`baseline-css-byte-copy` control reproduces that extra copy, and
`baseline-utf8-errors` reproduces repeated SIMD error localization.

The 28 independent CSS byte fixtures verify expected syntax and canonical
encoding. Every possible two-chunk split also checks decoded text using
the fixture's expected codec, not a codec selected by the engine. Native
tests cover all 65,536 two-byte UTF-8 sequences, malformed/overlong and
surrogate sequences, split astral characters, all 40 standard encodings,
stateful/large-buffer drains, the exact 1024-byte boundary, precedence,
resource limits, terminal failures, cancellation and retained buffers.
Legacy-codec differential tests verify our wrapper/state/bounds against
`encoding_rs`; they do not independently revalidate that library's codec
tables. The original UTF-8 decoder is compared against that library.

Primary algorithms: [CSS input byte stream](https://drafts.csswg.org/css-syntax/#input-byte-stream)
and [Encoding decode](https://encoding.spec.whatwg.org/#decode), consulted
2026-11-05. HTML encoding sniffing/restarts, network loading, streaming
HTML/CSS tokenization, Web IDL `TextDecoder` and script execution remain
unimplemented. See `bytes_bench` and the journal for release measurements;
none establishes website, UI, GPU, layout or Chrome performance.
