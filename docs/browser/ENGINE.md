# Engine implementation and checks

The engine crate is `olo-web`. It parses supported static HTML into its
own compact document storage, constructs generic CSS syntax and compiles/matches
selectors against a stable borrowed tree, and compiles/resolves typed colors.
It has no
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
Property grammars, unicode-range descriptor retokenization, complete selector
integrations, cascade/layers, custom-property substitution, media queries, invalidation,
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


## Selector compilation and matching

`css::selectors::Selectors::compile(Components, CompileOptions, CompileLimits,
cancellation)` consumes component views from a standalone selector or a
rule's prelude. It owns flat list/complex/step/test arenas and one shared
symbol pool, independent of the source Syntax's lifetime. Tests are
24 bytes on the measured x86-64 build. A temporary symbol lookup map is
released after compilation; invalid forgiving branches rewind all their
arenas/symbols. Unusually sparse arenas are trimmed after rollback.
`memory()` reports counts, symbol payload and accounted vector/header
bytes, excluding allocator bookkeeping, the root struct and temporary
compiler maps. Debug omits source, class/ID and attribute-value strings.

Implemented grammar covers lists, namespace-qualified types/universals,
ID/class/attribute operators and ASCII flags, descendant/child/adjacent/
subsequent sibling relations, structural/typed/filtered An+B selectors,
`:is`/`:where`/`:not`/relative `:has`, language ranges, host direction/state
hooks and common named pseudo-element targets. Specificity uses a saturating
lexicographic tuple; logical-function maxima and zero-specificity `:where`
are compiled once. Only `:is`/`:where` forgive invalid branches; unsupported
known features and resource errors remain failures. Nested `:has` and
pseudo-elements in real-selector argument lists are rejected. Default
namespace exceptions for logical subjects are explicit. Namespace bindings
are supplied after stylesheet namespace resolution; repeated prefixes use
the last binding. Parsing those at-rule contexts is still future work.

`AnB::parse` implements the token-level CSS microsyntax; `matches` uses
wide intermediate arithmetic and positive one-based indices. Coefficients
outside i64 are explicit `NumberLimit` failures, never silently saturated.
`:empty` currently uses the established nonempty-text-blocks-emptiness
behavior. The newer draft's whitespace exception remains unimplemented;
no complete Selectors conformance is claimed.

`matches` returns the maximum matching root-branch specificity. `query`
streams document-order matches through a nonblocking sink with explicit
include-root, context, cancellation and work/result limits. It allocates
no full result vector. A shared small backtracking stack walks long
complex selectors iteratively; only bounded logical/filter calls nest.
Both compilation and matching enforce a hard maximum nested-call depth
of 128 even if callers request more. All long APIs run on workers.

`Tree` borrows names, links, attributes and text with typed element IDs.
`DomTree` directly reads the compact Olo Document and validates scoped
handles; templates' content and shadow boundaries are not crossed as light
children. An explicit scoping handle is validated before matching, and a
virtual fragment root matches explicitly eligible `:scope` combinations,
including logical functions and guarded `:has`. Compiled featureless
eligibility prevents accidental matches such as `:not(.class)` and ignores
an implicit default namespace for an otherwise eligible virtual root. Document
HTML/quirks mode and optional normalized document language come from the
loader through `MatchOptions`; ancestor matching can extend outside the
queried subtree as required. A custom Tree supports arbitrary namespace
URIs without pretending the current DOM arena already supports XML loading.

Live focus, interaction, form, media, direction and other host states use
`Tree`/`StateSource`. Missing state yields `StateUnavailable`; it is never
invented from attributes or silently treated as false. Pseudo-state hooks
distinguish origin and pseudo-element states. Default public queries mask
`:visited` and treat `:link` via any-link state. `visited_style` is reserved
for a future private paint path with restricted visited-style effects;
it must not be exposed to DOM/agent queries. No renderer or live state
system is connected yet, and matching a pseudo target does not paint it.

Queries lazily build sibling indices only when an nth selector needs them.
They cache ordinary, per-type and selector-filtered positions/totals over
the stable borrowed tree. Index entries plus pending nested-filter entries
share one explicit quota; tables are dropped at query end, so later DOM or
state changes cannot reuse stale indices. Individual-element `matches`
uses a bounded scan rather than indexing an entire sibling group.
`baseline-selector-nth` reproduces repeated sibling scans with equivalent
results. The initial implementation was quadratic in sibling count;
release measurements and the final linear cases are in the journal.

Case-sensitive substring attributes use memchr's accelerated search;
ASCII-insensitive substrings use compiled prefix tables and linear KMP
recovery, with work/cancellation accounting. No per-match lowercase copies
or repeated-tail substring scans. HTML name folding applies to selectors,
preserving foreign-element case and namespaced attribute identity. HTML's
attribute-value table and quirks ID/class behavior are separate from explicit
`i`/`s` flags. UTF-8 substring boundaries are checked before slicing.

Independent evidence: `tests/selector-fixtures` pins cssselect2's original
literal vectors/documents, a deterministic static XML data adapter and
128 CC0 An+B cases. No third-party selector engine runs in production or
generates expectations. The native selector runner accounts for **463
cases: 432 direct matches, two exact modern grammar expectations and
29 unsupported cases**. Those non-passes include live state, shadow/
functional pseudo-elements, nesting and two fixture driver contexts.
The evaluator continues to exit 1; the regression gate accepts no new
mismatch/error/panic and protects the exact counts. See its README for
licenses, scope and the two nth-of-type grammar adjustments.

```sh
cargo run --release -p olo-web --example selector_eval -- /var/tmp/selectors.json
cargo test --release -p olo-web --test selectors
cargo build --release -p olo-web --example selector_bench
# Repeat with --features baseline-selector-nth for equivalent scan control.
taskset -c 0 /usr/bin/time -v target/release/examples/selector_bench 10
```

Native tests also cover DOM changes, stale/wrong-document/scoping handles,
virtual fragment scope, inert template boundaries, private Debug, host
state changes/history masking, namespace/specificity rules, Unicode and
case behavior, limits/sink errors/cancellation, nested index reservations,
10,001-compound chains, 5,000 nested functions and 10,000 malformed inputs.
No CSSOM/JS, dynamic invalidation, stylesheet rule indexing, property grammar,
cascade, style, layout, rendering, UI or website compatibility follows from
these selector tests. Primary references consulted 2026-11-05:
[Selectors 4](https://drafts.csswg.org/selectors/),
[An+B syntax](https://drafts.csswg.org/css-syntax/#anb-microsyntax),
[HTML selector case rules](https://html.spec.whatwg.org/multipage/semantics-other.html#case-sensitivity-of-selectors),
[HTML language determination](https://html.spec.whatwg.org/multipage/dom.html#language).


## Typed CSS colors

`css::color::Value::parse(Components, Limits, cancellation)` compiles a
complete color value on a worker. It validates named/hex colors, legacy
and modern RGB/HSL, HWB, Lab/LCH, OKLab/OKLCH, predefined `color()` spaces,
`currentColor`, system colors and nested `light-dark()`. Components borrow
generic Syntax only while compiling. Retained values own no source strings;
absolute/current/system colors need no additional heap allocation. Conditional
expressions use a shared flat node arena, with bounded compilation and
iterative selection/drop. Both conditional branches are grammar-checked.
Work/node/depth limits and cancellation stay explicit; nesting has a hard
maximum of 128 even if a caller requests more.

`Color` holds finite f32 coordinates and alpha, a color-space tag and a
separate four-bit missing-component mask. Alpha clamps to [0,1]; the parser
applies each grammar's specified channel clamps/percentage ranges/hue
normalization. The raw constructor permits out-of-range coordinates for
conversion, clamps finite magnitude to f32::MAX in both measurement modes,
maps NaN to zero and canonicalizes negative zero. `none` remains distinct
from numeric zero until a caller explicitly requests absolute conversion.
On the measured x86-64 build Color occupies 20 bytes and Value 24, compared
with 40/40 for the `baseline-color-f64` storage control. This is retained
value storage, not page or browser memory.

`resolve(&Context, …)` obtains the element's actual used light/dark scheme,
inherited/current color and system palette from the host. It walks only the
selected conditional branch; unselected context is never queried. It returns
`ContextUnavailable` rather than guessing an Olo palette. A night palette
can use the dark branch with host-supplied night colors. Legacy HSL/HWB with
no missing components compute to clipped sRGB; other identities/missing
components remain represented. There is no actual style host or all-site
dark/night adaptation yet. Author color-scheme selection/inheritance,
`prefers-color-scheme`, palette invalidation and forced adaptation still
need implementation and independent rendering checks.

`convert_absolute(Space)` uses f64 arithmetic and normative colorimetric
matrices/transfer functions across 15 RGB/XYZ/Lab/OKLab/hue spaces. Input
missing components become zero, while newly powerless output hues are
marked missing. Out-of-gamut coordinates remain unbounded; this is not
CSS interpolation's analogous missing-component propagation or destination
gamut mapping. `srgb8_clipped()` explicitly clips/quantizes and must not
be used as a substitute for the eventual renderer's profile/gamut policy.

`Conversion::new(&mut [Color], Space)` allocates nothing. `step(limit,
cancellation)` performs at most 4,096 colors, checks cancellation at least
every four, and returns converted/total progress. Completed work remains
converted after cancellation. Workers can yield/report counts between
steps and throttle UI notifications. Homogeneous batches in linear sRGB,
linear Display P3 and XYZ D50/D65 use safe four-lane f64 operations through
the Rust `wide` crate. Build-target AVX, paired SSE2/NEON or scalar fallback
preserve scalar operation order; mixed/nonlinear batches use the original
scalar conversion. No engine unsafe code, GPU dispatch, thread, timer,
polling or idle work is introduced. Batch result tests compare all 225
space pairs, masks, mixed spaces and short/tail lengths exactly.

Independent grammar/coordinate data accounts for **12,703 cases: 9,921
direct passes, 2,760 reviewed current-spec passes, 22 explicit non-passes**.
This includes 43 independent absolute conversion vectors, not just
round-trip tests. The fixture README documents licenses, literal extraction,
selection, numeric tolerances, current-spec changes and full non-pass
reasons. The remaining cases include substitution, profiles, device
colors, two nonfinite serialization references and one unported WPT assertion. Known unsupported grammar returns
`Unsupported`/`RequiresSubstitution`, separately from invalid input/resource
errors. Relative colors, mixing/interpolation, gamut mapping, CSSOM,
property compilation, cascade and painting remain unfinished. The native
ports do not execute WPT JS/pages, CSSOM serialization or rendering.

```sh
cargo test --release -p olo-web --test colors
cargo run --release -p olo-web --example color_eval -- /var/tmp/colors.json
# Exits 1 while any case remains unsupported.
cargo build --release -p olo-web --example color_bench
taskset -c 0 /usr/bin/time -v target/release/examples/color_bench 20
# Repeat with baseline-color-conversion or baseline-color-f64 controls.
```

Release observations, first-step latency, storage, RSS, controls, build
identity and raw samples are in `measurements/2026-11-05-colors.json`.
These are CPU microbenchmarks, not GPU/UI/Chrome performance. Native tests
also check 256 byte/alpha endpoints, changing/lazy host context, source
lifetime, deep flat expressions, finite extreme conversions, limits,
cancellation/resumption and 10,000 deterministic malformed values. Primary
references consulted 2026-11-05: [CSS Color 4](https://www.w3.org/TR/2026/CRD-css-color-4-20261103/),
[CSS Color 5](https://drafts.csswg.org/css-color-5/),
[CSS Values 4 precision](https://drafts.csswg.org/css-values-4/#range).


## Typed numeric calculations

`css::math::Value::parse(Components, Options, Limits, cancellation)` and
`compile(Component, …)` validate numeric values and math functions on a
worker. They consume borrowed syntax and retain no source strings.
Dimension algebra tracks seven integer exponents and a percentage hint in
an eight-byte type. Compatible absolute units canonicalize to px, deg, s,
Hz, dppx and fr; invalid dimensions remain invalid even when arithmetic
could eliminate them. Percentages resolve only according to the supplied
calculation context. Number/percentage mixing stays invalid; unitless zero
does not silently become a length inside calculations. Type exponents are
limited to [-127,127], nesting to a hard maximum of 128, with configurable
work, emitter storage and resolution-stack quotas.

The original grammar covers calc, min/max/clamp (including none bounds),
round/mod/rem, trigonometric/inverse functions, pow/sqrt/hypot/log/exp,
abs/sign, IEEE constants, progress/no-clamp and tree-counting functions.
Unknown syntax, known unsupported extensions, substitution requirements,
resource limits and unavailable context remain distinct errors. Media/
container progress, calc-size, device-pixel line-width rounding and
arbitrary substitution remain unfinished. Numeric type/property/cascade/
CSSOM integration is not implemented by these APIs.

Static subexpressions fold after type checking. Canonical constant sum
terms combine while contextual terms and meaningful zero values remain.
Zero coefficients of nonpercentage relative units need no environment;
percentage zero remains contextual where its basis/sign matters. Math
values occupy 16 bytes on the measured build. Static results retain no
additional heap allocation; dynamic values share a flat, 16-byte-instruction
postfix program. There is no recursively owned expression tree. Compilation
may spill its small temporary instruction buffer for complex inputs.

`resolve_raw(&Context, …)` returns a typed f64 quantity; `resolve_with`
reuses a caller-owned Scratch. Raw IEEE NaN, infinities and signed zero
remain inside calculations. `Quantity::materialize(min,max)` censors/clamps
only at the top-level/property boundary; callers supply valid bounds.
Font/root-font, viewport, container and percentage factors come from an
explicit stable host snapshot; missing data returns ContextUnavailable.
Physical unit factors are finite and nonnegative, with unsigned zero;
percentage bases can be negative. `Context::tree_count` receives a typed
request and must enforce stylesheet/element tree scopes and shadow
boundaries before returning a count. No DOM is automatically traversed.
Unit/tree dependency bits support future style invalidation. Real property,
style/layout hosts and live invalidation are still future work.

Independent numeric evidence: 1,581 literal native WPT ports and five
inventoried unported helper forms, **1,586 cases: 1,572 passes and 14 explicit
non-passes**. These test numeric grammar/types and raw coordinates in
explicit representative contexts. They do not execute JS, CSSOM or layout.
The fixture README describes original helper expansion, literal context
changes, static element models, independent canonical reference decoding,
tolerances and scope. Hashes reject fixture edits, and extraction reproduces
the derived data. Remaining non-passes are nonnumeric wrappers, serialization
and property-limit drivers. All seven native groups cover live context,
unit/type rules, IEEE edge cases, private Debug/source lifetime, limits,
cancellation, 10,000 nested inputs, 5,000-term flat evaluation and 10,000
malformed values. Color-channel integration now reuses these numeric values;
its contracts and measured costs follow below.

```sh
cargo test --release -p olo-web --test math
cargo run --release -p olo-web --example math_eval -- /var/tmp/math.json
# Exits 1 until all imported native cases are supported.
cargo build --release -p olo-web --example math_bench
taskset -c 0 /usr/bin/time -v target/release/examples/math_bench 20
```

`baseline-math-folding` retains operations instead of folding static
subtrees/zero unit coefficients. `baseline-math-sums` retains operator order
without canonical sum-term combination. All supported corpus results pass
in both controls. Sum reassociation follows CSS simplification and is not
a promise of bitwise equivalence to ordered IEEE arithmetic for arbitrary
extreme inputs. Raw measurements, first-call latencies, compiler/source/
binary identities and allocation-capacity accounting are in
`measurements/2026-11-05-math.json`. These controls include compiler/layout
effects; the measurements do not establish uniform speedups, UI latency,
GPU performance, browser memory or website compatibility.


## Numeric color channels and invalidation metadata

The color parser reuses typed `css::math` in numeric channels. Root number,
percentage and (for hue) angle types must match each channel grammar.
Percentage/number addition remains invalid; hue contexts prohibit percentage
leaves even if a calculation would cancel their dimensions. Legacy comma
forms retain their homogeneous-unit, percentage and `none` restrictions.
Math IEEE values remain raw until the shared color normalization/clamping
boundary, including the specified zero normalization of infinite hues.
Static calculations fold into the same heap-free absolute Value as literal
channels. They do not keep syntax, expression or numeric arenas.

Contextual channels retain four typed values and unit/space metadata in a
72-byte Functional. Root values share it by Arc. Conditional arenas store
an index into an exact boxed slice of Functional leaves, preserving 20-byte
nodes and a 24-byte Value on the default build. The example root font math
uses 280 extra retained bytes, including its numeric program and Arc headers;
the sample conditional with no math grows from 208 to 232 bytes for metadata.
Compiler temporaries stay inline for four numeric slots and one Functional;
large conditional lists may spill. The journal records the memory/performance
tradeoffs; this is not a statement about page/browser memory.

`Context::math_factor` and `math_tree_count` delegate to the same explicit
numeric snapshot contracts as the math core. No production font/container
metrics or tree scopes are guessed. Resolution queries only the chosen
branch, shares one temporary numeric stack across its channels and checks
cancellation inside numeric execution. Node/instruction budgets cover the
whole compiled color; actual numeric compiler work is charged to the
outer parse budget. Resolution deducts conditional visits and all selected
numeric instructions from one work budget. Nesting remains hard-bounded.

`Value::dependencies()` returns conservative scheme/current/system/math
metadata, including both branches. Conditional metadata is cached during
compilation; root Functional queries at most four channel dependency masks.
The future style host must use it with actual cascade/font/container/tree
changes. This API does not implement live style invalidation, author color-
scheme, forced dark mode or tree-scope enforcement.

The existing denominator now has 193 more direct passes: 155 computed-
coordinate and 38 validity-only assertions, including static math and
context-dependent computation. Literal
fixture environment extraction preserves source width changes; a documented
16px native default stands in for an unspecified WPT font. This is a supplied
numeric environment, not layout. Two infinite chroma serialization references
remain non-passes rather than being coerced into finite reference values.
Full counts and non-passes are in
`measurements/2026-11-05-color-math-coverage.json`; exact before/after binary
and source identities, raw observations, first-call latency and storage are
in `measurements/2026-11-05-color-math.json`.

```sh
cargo test --release -p olo-web --test colors
cargo test --release -p olo-web --test colors --all-features
cargo build --release -p olo-web --example color_math_bench
taskset -c 0 /usr/bin/time -v target/release/examples/color_math_bench 20
```


## Custom-property token values and substitution

`css::variables::Value::compile(Components, Limits, cancellation)` validates
generic declaration contents and current var() argument grammar, then copies
only those tokens into an owned compact Syntax. `compile_batch` shares one
source/token arena across values; each view is 16 bytes. Original stylesheet
Syntax/source is released. One live view retains its whole batch, so callers
should group values with compatible lifetimes. A pending bit shares the
end-index field; arenas are capped at 2^31-1 nodes. The root Syntax node
layout stays 24 bytes. Empty values are valid; only `Computed::invalid()`
denotes guaranteed-invalid.

The original assembler copies flat token structures and numeric/decoded
payloads directly, without concatenating variable strings or retokenizing
the expanded text. It rebuilds container ranges, normalizes implicit EOF
closures and preserves names, units, number spellings and casing. Required
boundary comments prevent new identifiers, dimensions, percentages, CDO/CDC
or comments from appearing across pasted tokens. `source_spelling()` exposes
the assembled value's own coherent spelling (linear in its top-level token
count), not CSSOM/author serialization or adjoining batch values. Normal
output-byte checks use a constant-time whole-arena preflight, avoiding a
spelling scan for values whose complete arena fits the limit.

var() supports empty/multiple-token fallbacks, current free-form {} wrappers,
case-sensitive decoded custom names and names produced by nested var().
Argument grammar is validated even in unused fallbacks. Selection resolves
only the needed fallback, so unused fallback cycles are irrelevant. Other
known arbitrary substitution functions remain pending, and selected
unimplemented functions return Unsupported rather than masquerading as
missing custom properties.

`Resolver::new(&Context, Limits, cancellation)` works on a supplied winning
unregistered-property snapshot. The host provides stable unique property IDs,
already-computed inherited values and animation-taint metadata. It must keep
that snapshot cheap and stable during an operation and advance `revision()`
for changes to lookup/inheritance/taint/cascade results. Revisions are checked
only on requests; changed snapshots discard cached results and a mid-operation
change returns ContextChanged. Operational failures discard partial caches.
There are no threads, timers, polling or idle work in the module.

Property results are memoized; static values and aliases share arenas without
copying tokens on repeated reads. Consumers should retain the resulting typed
property values after substitution, rather than rebuilding them each frame.
A custom property's direct or substituted sole CSS-wide keyword applies at
its property boundary: initial is invalid, inherit/unset use the supplied
parent, and revert-family keywords call the actual cascade host (default
RequiresCascade). A consumer's remaining property grammar/ranges and CSS-wide
behavior still belong to the future property compiler.

Active property guards detect demand-driven cycles. All cycle participants
remain invalid despite their fallbacks; callers outside a cycle may recover.
Merged cyclic cache entries retain links to earlier roots while guards
unwind, so repeated references and overlapping cycles stay correct. Marking
uses the active-suffix invariant to avoid rescanning an already-marked group.
Dependency reads propagate supplied animation taint through custom properties;
non-animatable consumers can exclude tainted var() references and use fallbacks.
This is taint/animation plumbing, not animation execution or registration.

Limits bound source/output bytes, emitted nodes, work, combined nesting,
cached-property count and unique cached arena reservation. The hard nesting
maximum is 256. Oversized expansions become guaranteed-invalid, enabling
surrounding var() fallbacks. Even after output is invalid/over-limit, remaining
references are visited to detect cycles; writing stops once its output cap
is exhausted. Work, cancellation and cache quotas remain separate operational
errors. Cache arena accounting includes shared input arenas once, and excludes
hash-table/control metadata, allocator bookkeeping and transient stacks;
property/guard counts bound that storage separately. Context callbacks must
not perform long/blocking work. The future style job must run these APIs on
workers, retain results, report progress and own cancellation.

Whitespace scans for single names and CSS-wide keywords are metered too.
Argument validation charges source spans, and a spelling scan needed for a
smaller output limit charges the value's node range before traversal. Cached
large values therefore cannot bypass the work budget through repeated scans.

Independent evidence: 25 pinned WPT resources, 65 parsing/name cases,
60 property assertions in eleven cycle groups and 26 unported drivers:
**151 inventoried native cases, 119 passes and 32 explicit non-passes**.
Four cases need other substitution functions, two need registered-property
hosts, and 26 JS/CSSOM/style/layout/document drivers are not ported. Native
reference decoding uses literal canonical numeric/ident strings, not engine
output. Given block winners honor duplicate/importance order in the adapter;
this is not cascade, JS, CSSOM or layout. The evaluator exits 1 until every
inventoried case is supported.

Twelve native groups also check source lifetime/Debug privacy, shared arenas,
typed color/math consumption, token boundaries/escapes/EOF recovery,
inheritance/keywords, taint, revisions, cancellation, output/cache/work limits,
10,000 nested inputs, 10,000 malformed values and 1,000 generated eight-node
graphs against independent transitive-closure cycle membership. The fixture
README describes source selection and scope; hashes reject changed bytes and
literal extraction reproduces derived data. See the variable coverage and
release measurement artifacts for exact identities and results.

```sh
cargo test --release -p olo-web --test variables
cargo run --release -p olo-web --example variables_eval -- /var/tmp/variables.json
# Exits 1 while any inventoried case remains unsupported.
cargo build --release -p olo-web --example variables_bench
taskset -c 0 /usr/bin/time -v target/release/examples/variables_bench 20 cold
taskset -c 0 /usr/bin/time -v target/release/examples/variables_bench 20 warm
```

Current CSS references consulted 2026-11-05:
[CSS Variables 1](https://drafts.csswg.org/css-variables-1/),
[CSS Values 5 substitution](https://drafts.csswg.org/css-values-5/#arbitrary-substitution),
[CSS Syntax boundaries](https://drafts.csswg.org/css-syntax-3/#serializing).
Full arbitrary-substitution early invocation/spread, registered properties,
other functions/attribute and URL taint, animation integration, CSSOM,
actual declaration/cascade/style hosts and live invalidation remain open.
No webpage, Chrome rendering, browser memory, UI latency or all-site dark
mode capability follows from these native ports or microbenchmarks.

## Typed property declarations

`css::properties::Declaration::compile(&Statement, Limits, cancellation)`
connects the generic declaration parser to original typed grammars. The
standalone `parse(decoded_name, Components, important, ...)` API takes an
already-decoded CSS property name; it is not a CSSOM setProperty/string-name
parser. Static ASCII case-insensitive binary lookup covers **88 longhands
and 15 shorthands**, with canonical names, inherited flags and initial-value
spellings. The registry intentionally exposes its implemented scope; fonts,
background images/layers, border-image/radius, transforms, grid templates,
alignment, animations, transitions, many shorthands and descriptors remain.
UnknownProperty, Invalid, Unsupported and operational failures are distinct.
An unrecognized property does not prove its grammar is invalid.

Typed values are CSS-wide keywords, registry keywords, original numeric/color
values, structured display box types, fit-content numeric arguments or shared
pending tokens. The compiler validates dimensional/range rules, accepts bare
zero for lengths, preserves percent/font/viewport/tree dependencies and keeps
logical longhands distinct from physical ones. Initial spellings are metadata
for a future shared default-style table, not a computed default style. There
is no property-to-element application yet.

Nonnegative literals are rejected when negative; numeric functions can be
out of range and clamp at computation. Numeric resolution uses the supplied
property-specific host and retained math scratch. Widths, margins, font-size
and line-height need different percentage/font snapshots. Opacity normalizes
percentages and clamps to [0,1]. Integers from functions round to nearest with
ties toward positive infinity; literal integer syntax is required otherwise.
IEEE specials are censored at the property boundary, after inner math;
unbounded literal overflow is accepted and materialized to finite f64 bounds.
Line-height numbers remain multipliers, distinct from length calculations;
actual used line metrics are not computed here.

Quad/pair shorthands expand atomically in physical/logical order; flex-flow
accepts its direction/wrap keywords in either order and fills omitted initial
values. Importance comes from the original declaration and applies to all
assignments. CSS-wide keywords expand over every implemented member. Display
retains outer/inner/list-item or internal table/ruby roles and recognizes
legacy aliases. No box tree, blockification, coupled overflow computation,
layout mapping of logical axes or CSSOM serializer is implemented.

The existing variable scanner validates generic contents and var() arguments
without copying tokens on an ordinary declaration. A pending shorthand holds
one Arc<Pending> shared by every longhand assignment. Its tokens are compact
owned copies, releasing the original stylesheet Syntax. The cascade host must
select winning assignments before `Pending::resolve(...)`, resolve each shared
pending value once for its snapshot, and retain the resulting typed values.
Invalid computed output produces unset for every affected longhand; it must
not expose an older declaration. Unsupported functions, resource limits and
cancellation stay explicit errors. Custom declarations preserve decoded
case-sensitive names, importance and distinct valid empty token values.

Limits preflight the flat node ranges and source spans, then bound the generic
scanner and each nested math/color/token compiler. These are per-invocation
bounds with fixed extra passes: at most four shorthand members. They are not
one exact combined instruction counter or a whole-stylesheet memory budget.
Default input is 1 MiB/65,536 nodes, work 1,000,000, nesting 64. The original
math/color/variable hard caps also apply. Cancellation is checked while
scanning/copying/evaluating. Worker ownership, batching/progress and lifecycle
invalidation belong to the future style job; this module adds no threads,
I/O, timers, polling or idle work.

`Value::dependencies()` reports numeric/color/substitution inputs, including
pending values explicitly. Property-specific font, cascade, logical-axis and
layout dependencies are additional host responsibilities, not inferred from
an empty token dependency mask. `reserved_bytes()` counts referenced heap
storage; clones/shared arenas must be counted once by their owner. The native
benchmark records independent retained declarations and excludes original
Syntax, allocator metadata and runtime host state.

Independent evidence comes from 126 pinned unmodified WPT resources. Literal
ports inventory **1,109 cases: 821 passes and 288 explicit non-passes**, with
no unexpected mismatch/error/panic. Declaration ports check acceptance only,
not the upstream helper's CSSOM serialization. Shorthand maps compare actual
member identities and independent literal values. Isolated numeric/keyword/
absolute-color references use no engine parser to interpret expected values;
font/percent/intrinsic/used-value/element/display-CSSOM contexts and nonliteral
JS drivers remain unsupported. This is no browser execution or rendering
claim, and the evaluator exits 1. Eight native groups also check source
release, dimensional/range/rounding behavior, shared pending values, invalid
computed output, logical identities, cancellation/quotas and 2,000 generated hostile
inputs. Both assignment-storage controls pass the same native checks.

```sh
cargo test --release -p olo-web --test properties
cargo run --release -p olo-web --example properties_eval -- /var/tmp/property-report.json
cargo run --release -p olo-web --example properties_bench -- 20000
```

Primary grammar references consulted 2026-11-05:
[CSS Sizing 3](https://drafts.csswg.org/css-sizing-3/),
[CSS Box 4](https://drafts.csswg.org/css-box-4/),
[CSS Display 3](https://drafts.csswg.org/css-display-3/),
[CSS Overflow 3](https://drafts.csswg.org/css-overflow-3/),
[CSS Cascade 5](https://drafts.csswg.org/css-cascade-5/) and
[CSS Values 4](https://drafts.csswg.org/css-values-4/) and
[CSS Values 5 substitution](https://drafts.csswg.org/css-values-5/#arbitrary-substitution).
