# Engine implementation and checks

The engine crate is `olo-web`. It parses supported static HTML into its
own compact document storage, constructs generic CSS syntax and compiles/matches
selectors against a stable borrowed tree, compiles/resolves typed colors and
computes frozen styles over the supported property registry, selects native fonts
and prepares retained glyph runs with original Unicode text orchestration.
Initial normal-flow layout, retained static paint, a partial UA stylesheet and
native single-page development shell are implemented. Network, complete style/
layout/paint/live invalidation, JavaScript execution, full browser interaction
and demonstrated website compatibility remain required.

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
not perform long/blocking work. The style job must run these APIs on
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
parser. Static ASCII case-insensitive binary lookup covers **89 longhands
and 16 shorthands**, with canonical names, inherited flags and initial-value
spellings. The registry intentionally exposes its implemented scope; fonts,
background images/layers, border-image/radius, transforms, grid templates,
alignment, animations, transitions, many shorthands and descriptors remain.
UnknownProperty, Invalid, Unsupported and operational failures are distinct.
An unrecognized property does not prove its grammar is invalid.

Typed values are CSS-wide keywords, registry keywords, original numeric/color
values, structured display box types, fit-content numeric arguments or shared
pending tokens. The compiler validates dimensional/range rules, accepts bare
zero for lengths, preserves percent/font/viewport/tree dependencies and keeps
logical longhands distinct from physical ones. Initial spellings feed the shared
computed default-style table described below. This declaration compiler itself
does not apply properties to elements.

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

The all shorthand expands over the 88 implemented resettable longhands,
excluding direction and custom properties. Unicode-bidi is not implemented.
This does not imply complete property coverage. Quad/pair shorthands expand
atomically in physical/logical order; flex-flow
accepts its direction/wrap keywords in either order and fills omitted initial
values. Importance comes from the original declaration and applies to all
assignments. CSS-wide keywords expand over every implemented member. Display
retains outer/inner/list-item or internal table/ruby roles and recognizes
legacy aliases. Blockification and coupled overflow computation belong to the
style stage below; there is no box tree, layout or CSSOM serializer.

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
bounds with fixed extra passes: at most four numeric/color shorthand members, or the fixed all reset list
for CSS-wide keywords/pending values. They are not
one exact combined instruction counter or a whole-stylesheet memory budget.
Default input is 1 MiB/65,536 nodes, work 1,000,000, nesting 64. The original
math/color/variable hard caps also apply. Cancellation is checked while
scanning/copying/evaluating. Worker ownership, batching/progress and lifecycle
invalidation belong to the separate style job; this module adds no threads,
I/O, timers, polling or idle work. The initial document style job is described below.

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

## Frozen stylesheet sets and cascade

`css::cascade::SheetBuilder` compiles ordered sheets over the supported
property/selector engine. Named, nested and anonymous layers are ordered
across sheets within each origin; unlayered declarations use the implicit
outer layer. Layer names validate completely before changing layer order.
Namespaces are stylesheet-local, reset between sheets, and passed to every
selector root in that sheet. Resource failures poison a builder; finish
cannot publish its partial state. Invalid CSS is recorded/ignored, while
unimplemented properties, selectors, at-rules and nesting remain visible
issues. Imports, conditional rules, scope and keyframes are not implemented.
Callers/evaluators must inspect issues instead of treating partial sheets
as complete compatibility.

All rule roots share one original flat selector collection, symbol table and
arena capacities. A rejected root rewinds only its unpublished tail. Namespace
and cancellation borrows exist only during add(), avoiding self-referential
state; shared compiler quotas span the collection. Frozen declaration slices
retain their actual lengths instead of small Vec growth capacity. Original
CSS Syntax/source can be released after compilation. This does not implement
author stylesheet text, CSSOM rule identity or mutation APIs.

The conservative rule index selects a necessary subject ID, class or type
per root branch, otherwise universal. Logical-only subjects stay universal;
indexing never decides a match or substitutes maximum possible specificity
for the highest matching branch. ASCII folding admits harmless false positives
for quirks/HTML/XML cases; the existing matcher decides actual semantics.
One worker's MatchScratch reuses generation stamps/candidate capacities, with
explicit rollover clearing. baseline-cascade-index disables the index and
scans every rule as a measurement control, absent by default. Live state still
requires a coherent Tree/StateSource; selected missing state is an explicit
error. A private visited-paint path is not implemented or exposed.

`RuleSet::match_element(...)` produces a frozen candidate Cascade, retaining
borrowed typed programs from the rule set and optional inline declarations.
The caller must keep the tree/state/parent inputs coherent for the operation
and discard results when their document/style generation changes. This is
not automatic event invalidation, a persistent computed-style cache or a
complete live style job. No polling, threads, I/O, clocks or idle work are
added. All long APIs, large drops and host integration belong on workers;
progress/batching/lifecycle control belong to the separate style job.

Priority records origin/importance, encapsulation, inline attachment, layer,
actual matched specificity, supplied proximity, source/declaration order and
rule identity. UA/user/author importance reversal and presentational-hint/
animation/transition positions are explicit. Scope/encapsulation fields are
host metadata, not parsed @scope or shadow-tree support. Encapsulation rollback
and transition rollback are explicit unsupported contexts. Animation origin
ordering is not animation execution.

Candidates stay available for revert-family processing. Origin rollback
accounts for author hints/animation; layer rollback uses normal layer
boundaries even for important declarations, with the important-inline
exception. Rule rollback moves to lower-priority candidates outside that
rule. This strict descent fixes a chain found in the imported WPT resources.
Layer/rule combinations can re-enter a prior candidate; cycle guards default
to unset instead of recursing indefinitely.

`Cascade::resolver()` retains variable results and typed pending expansions.
A custom property's rollback callback selects lower bindings inside the same
variable dependency guards; callbacks cannot hide self/merged var() cycles.
A cycle made only from rollback keywords defaults to inherited/initial custom
behavior, distinct from a cyclic var()'s guaranteed-invalid result. Pending
shorthand expansion is shared once per pending identity and animation-taint
policy; different policies cannot reuse one another's result. The host may
supply taint metadata, but keyframes/registrations are still absent.

`Resolver::property(property, allow_animation_tainted)` returns a defaulted
Specified value, Initial marker or Inherit marker. Inheritance must copy the
parent's computed value, never rerun its specified font/unit expression in
the child's context. The parent callback supplies already-computed inherited
custom tokens; no whole parent style map is cloned by production code.
Invalid computed substitution becomes unset without exposing an older rule.
The all shorthand resets the implemented 88 resettable longhands, excluding
direction and custom properties; it does not complete the property registry.

Logical/physical declarations retain separate specified identities and are
projected together by the explicit API below. The separate style stage now
supplies frozen fonts/percentages/line metrics and coupled property transforms
over the supported registry. Missing hosts still include actual font selection,
used sizing, UA defaults/presentational-hint generation, flattened
inheritance, CSSOM, animation/transition execution and live invalidation. The
API is a cascade/defaulting stage, not a complete computed style or a browser.
These missing hosts remain acceptance work even when a native port passes.

Candidate, compile/rule/declaration/layer/issue, nesting and retained pending
limits bound work/storage. Private bottom-up merging checks work/cancellation
between comparisons, so sort failure cannot publish partially sorted state.
Matching sums actual selected-rule work against its operation budget; nested
property/math/color/variable compilers also enforce their own bounds. These
are bounded phases, not one exact combined instruction/memory counter for a
whole style job. Default cascade limits: 65,536 candidates, work 4,000,000,
rollback depth 128 (hard 256), typed pending reservation 16 MiB, plus the
original variable limits. Typed reservation is conservative: shared programs
may be counted repeatedly, while allocator/hash controls are separately
count-bounded. Rule and snapshot payload metrics explicitly exclude those
controls and borrowed Syntax/DOM/host state; they are not browser RSS.

The public Selectors::matches setup/return path remains identical to the
parent implementation. A shared accounting return initially introduced a
measured 4–13% slowdown; a generic output experiment only partly recovered it.
Keeping the original public path and a private accounted root path removes
unused result transport. Both use the same Machine matching algorithms.
The release artifact retains the failed experiments and final direct-match
checks, rather than silently discarding a regression.

Independent evidence: 19 unchanged pinned WPT resources, **136 inventoried
native cases: 128 passes, eight explicit unsupported rendering drivers**.
The adapter runs original inert HTML, stylesheet compilation, actual selector
matching/cascade/defaulting and supplied inherited custom/color contexts.
It independently reads literal RGB/decimal/identifier references, with Light
CanvasText/Canvas and an explicit 16px rem fixture basis. Reftest ports check
only their background-color component; geometry/rasterization stay unported.
No JS, CSSOM, animation, layout, screenshot or website execution is implied.
The evaluator exits 1, and the unpooled prototype has the same 128/8 outcomes.
Ten native groups check ranking, rollback chains/cycles, namespaces/layers,
real DOM/inline styles, taint-policy cache isolation, all exceptions and quotas.
Default/full-scan/all-feature modes pass the same native checks.

```sh
cargo test --release -p olo-web --test cascade
cargo test --release -p olo-web --test cascade --features baseline-cascade-index
cargo run --release -p olo-web --example cascade_eval -- /var/tmp/cascade-report.json
cargo run --release -p olo-web --example cascade_bench -- 3
cargo run --release -p olo-web --example selector_match_bench -- 200000
```

Primary references:
[CSS Cascade 5](https://drafts.csswg.org/css-cascade-5/),
[CSS Cascade 6](https://drafts.csswg.org/css-cascade-6/),
[CSS Namespaces](https://www.w3.org/TR/css-namespaces-3/) and the unchanged
WPT sources/metadata in tests/cascade-fixtures. Scope, identities and raw
measurements are recorded in the journal and release artifacts.


## Logical-property projection

`Resolver::axes(parent_axes, allow_animation_tainted)` resolves writing-mode,
direction and text-orientation through the ordinary cascade/variable machinery
before logical projection. Missing required parent axes return Unsupported;
a child cannot silently substitute the initial axes for a missing parent.
`Axes` retains computed direction, including RTL underneath upright text.
Upright changes used direction only in vertical typographic modes
(vertical-rl/vertical-lr); horizontal and sideways modes keep their direction.
This follows CSS Writing Modes 4 §5.1 and §6.4. No bidi/shaping or root/body
layout propagation is performed.

`Resolver::projected(property, axes, allow_animation_tainted)` combines the
two equivalent declared ranges by priority. All ten standard mode/direction
combinations are represented. The current registry has 20 pairs covering 40
longhands: preferred/minimum/maximum size, margins, padding, insets and
overflow. Border/corner logical longhands remain unimplemented. `Axes::pair`
returns physical then logical; `Axes::physical` provides the canonical name
for canonical computed storage. Unpaired properties pass through unchanged.
`Resolver::property` remains the raw declared-property query, useful for the
prerequisites and diagnostics; it does not perform paired projection.

`Projected { value, source }` retains the declaration identity that supplied
Initial/Inherit/Specified. An Inherit result must read `source` on the parent
using the parent's axes and copy the already-computed value. For example,
margin-inline-start: inherit in an RTL child reads the LTR parent's left
margin, whereas margin-right: inherit reads its right margin. Physical
longhands win equal priority when all expands both sets, so all: inherit
copies physical margins across changed axes. Ordinary source order still
wins between distinct logical/physical declarations.

Rollback searches both ranges under the same origin/layer/rule constraints
and cycle/work/cancellation guards. A pending logical shorthand selects its
actual declared member from the shared expansion, even when queried through
its physical alias. Missing/invalid substitution defaults the winning value
rather than exposing the older alias. Taint policy and retained limits remain
part of pending-cache identity. There is no extra persistent alias index,
whole-style clone, timer or per-frame work. The raw and projected return paths
use separate compile-time output types around the same resolver algorithm.

Nine unchanged pinned WPT resources contribute **5,757 inventoried native
assertions/drivers: 5,726 pass and 31 CSSOM drivers remain unsupported**.
The literal mapping/value tables and helper assertion loops independently
exercise every standard mode/direction combination, declaration order,
specificity, physical/logical shorthands, deferred/inherited writing mode,
variables and layer rollback. The adapter supplies explicit inert nodes that
the upstream helper creates with JS; each mutation is a fresh frozen input.
It now computes parent axes and follows inheritance source correctly.
Six focused native groups additionally check all/reset inheritance, upright
and sideways behavior, raw parent direction, missing-parent context,
important/origin/rule rollback, invalid substitution, shared cache/taint,
cancellation, work/depth and retained-memory limits. This is no JS/CSSOM,
live-invalidation, layout, text-shaping, rasterization or website evidence.

```sh
cargo test --release -p olo-web --test logical
/usr/bin/python3 crates/web/tests/logical-fixtures/extract.py
cargo run --release -p olo-web --example logical_eval -- /var/tmp/logical-report.json
cargo run --release -p olo-web --example cascade_property_bench -- 3000000
cargo run --release -p olo-web --example logical_bench -- 1000000
```

Primary references consulted 2026-11-05:
[CSS Logical Properties 1 §4](https://drafts.csswg.org/css-logical-1/#box),
[CSS Writing Modes 4 §5.1](https://drafts.csswg.org/css-writing-modes-4/#text-orientation)
and [§6.4](https://drafts.csswg.org/css-writing-modes-4/#logical-to-physical),
and [CSS Overflow 3](https://drafts.csswg.org/css-overflow-3/#overflow-properties).
Pinned originals, literal extraction, scope and hashes are under
`crates/web/tests/logical-fixtures`. Release measurements and coverage summary
are in `measurements/2026-11-05-logical*.json`; journal records the outcomes.

## Frozen initial computed styles

`css::style` computes immutable values for the current **94 longhands and 17
shorthands**, stored under **74 canonical physical/unpaired longhands**. This
is a bounded worker stage, not complete CSS or a rendering engine. `Defaults`
compiles typed initial spellings and their computed baseline once per immutable
`Host::revision`, shared across styles. Initial values are not an HTML UA
stylesheet: headings, body, controls, presentational hints and font-family
selection still need that layer.

`Host` supplies a coherent scheme/system palette, device scale including zoom,
UA font-size keyword policy and already-selected `FontMetrics` for a
`FontRequest { size, weight, width, slant, families, axes }`, borrowed by the
callback. Optional viewport/container/tree callbacks
are bound to the subject element; unavailable contexts return errors. No
guessed font or viewport is substituted. The host must change its revision when
any of these inputs change. This API does not select fonts, shape text, load
font files or implement author `color-scheme`.

`Style::compute(defaults, cascade, parent, root, host, options, limits, cancel)`
requires consistent parent/default/root identities. It resolves axes and font
prerequisites before ordinary properties, using parent font units for
font-affecting properties and the initial font for root-relative units during
root font computation. Inheritance copies computed values, not specified
expressions. Unitless line heights retain their multiplier; percentages and
lengths freeze against the subject's computed font. `normal` retains its marker
and the supplied used line metric. Numeric partial evaluation freezes
font/viewport/container/tree inputs while retaining layout percentages and the
original arithmetic evaluation order. Final range/rounding/IEEE normalization
happens at the property boundary, not inside partially folded expressions.

Computed `color` becomes concrete; other `currentcolor` values retain that
marker across inheritance and resolve against the subject through `used_color`.
System and light-dark choices freeze in the supplied scheme/palette. Text
spacing's computed `normal` is zero; percentages inherit intact and
`used_text_spacing` requires the subject's used font size. Text-align
`match-parent` uses the parent's computed direction, including under upright
text; the legacy `-webkit-match-parent` alias is recognized too.

Coupled transforms implement visible-to-auto overflow, absolute/fixed float
suppression, root/float/position/flex/grid blockification and inner flow-root
when the actual parent box has a different writing mode. `clip` remains `clip`
alongside a scrollable axis under the current Overflow specification. Border
keywords are 1/3/5 CSS px and computed widths snap to device pixels; none/hidden
styles zero **used** width through `used_border_width`, preserving computed
width for inheritance. `Options::layout_parent: Option<BoxParent>` carries the
nearest box parent's display and axes separately from the inheritance parent.
The document job skips `display:contents` for that context. This describes
computed values only; no CSS box tree or used sizing is produced.

Each Style owns an 80-byte header, shared Defaults/Font/custom dictionaries and
sparse immutable overrides. It retains no parent, source DOM or cascade.
Logical value queries project with its axes. Undeclared properties copy initial
or inherited computed values without rerunning the cascade resolver. Unchanged
fonts and inherited custom dictionaries share Arcs. Exact equal parent/adjacent
override arrays share too, independently of their custom bindings; numeric
program equality is conservative. Custom values are computed before inheritance
and never rebound against a descendant's declarations. Empty dictionaries and
default-only arrays share storage. This is not global interning or incremental
invalidation; temporary per-element vectors and nonadjacent sharing remain
profiling work. `Memory::for_styles` counts distinct input Style headers and
deduplicates referenced defaults, fonts, arrays, names, token arenas and numeric
programs, including Arc controls. Allocator/hash controls, DOM, stylesheets and
caller containers are excluded; this measure is not RSS.

`Job::new(&Document, &RuleSet, defaults, options, limits, cancel)` walks the
immutable connected light tree parent-before-child. `step(node_budget,
subject_host_callback)` computes a bounded prefix with progress; completed
prefix values can be read while work proceeds. It excludes disconnected and
template-content trees. Real selector matching/state snapshots, inline
declaration parsing, cascade/defaulting, computed inheritance and nearest box
parent context use production code. Invalid inline declarations are skipped
with counts; unknown/unsupported inline and stylesheet diagnostics remain
visible in `Report`. A resource, missing-context or cancellation error poisons
the job; `finish` rejects incomplete/failed results. Defaults bound individual
style properties/work/custom storage; job limits separately bound tree slots,
nodes/elements/depth/work, inline parsing and aggregate retained payload.

The finished `StyleTree` retains no document/stylesheet/state-source borrow.
`get(document, node, expected_generation)` rejects document identity/revision,
slot generation or style epoch mismatches. The caller must advance the style
epoch on stylesheet/state/environment changes and check it before installing
a worker result. This is an initial full pass, not automatic event-driven
invalidation, flattened/shadow inheritance, UA styles or animation execution.
There are no threads, timers, I/O or idle work inside the module; callers run
and schedule the job on a worker.

Independent ports in `tests/style-fixtures` pin nine unchanged WPT documents
and helpers: **243/341 pass, 98 unsupported**, no unexpected mismatch/error or
panic. There are 316 literal native assertions and 25 unported browser
drivers. All 73 weight/line-height assertions requiring unimplemented
`container-type` contexts remain unsupported, including assertions whose
scalar alone could otherwise be accepted. The adapter applies ported mutations
to original inert HTML and runs the production document job. Display tuples
and numeric references are decoded independently. These are semantic ports,
not 341 complete WPT browser tests or a CSSOM serializer. See the fixture README
for supplied font/palette context and denominator. Earlier logical/cascade
corpora still run through their original supplied-context adapters; their
counts do not imply that this new document stage has executed those corpora.

Owned Wayland Chrome 157.0.0.0 probes record **22 selected CSSOM observations in
15 local cases** in `measurements/2026-11-05-style*-chrome.{json,html}`. Native
tests match all observations. The reference supports prefixed match-parent,
not the unprefixed standard value; both native aliases are tested, with that
difference explicit. Effective schemes are supplied by the native host since
author color-scheme is unimplemented. These probes are structured CSSOM
comparisons at scale 1, with no screenshots or visual/website evidence. Native
border arithmetic additionally checks 1, 1.25, 1.5, 1.75 and 2×; this is not a
visual review at those scales.

```sh
cargo test --release -p olo-web --test style
cargo test --release -p olo-web --test style --features baseline-style-dense
/usr/bin/python3 crates/web/tests/style-fixtures/extract.py
cargo run --release -p olo-web --example style_eval -- /var/tmp/style-report.json
# The evaluator exits 1 until every inventoried assertion/driver passes.
cargo build --release -p olo-web --example style_bench
target/release/examples/style_bench 5
```

The `baseline-style-dense` feature disables sparse/default fast paths and
parent/adjacent override sharing, providing a same-semantics counterfactual
for the new stage. Measurements and their reproducible runner are in
`measurements/2026-11-05-style.json` and `2026-11-05-style-measure.py`. They
include first calls, maximum observed 128-node worker steps, lookup costs,
retained payload and separate process RSS; neither a hard real-time guarantee
nor Chrome/browser performance is established. No SIMD/GPU addition is
claimed for this scalar dependency/priority stage.

Normative rules consulted 2026-11-05: [CSS Cascade 5 defaulting](https://drafts.csswg.org/css-cascade-5/#defaulting),
[CSS Values 4 computation](https://drafts.csswg.org/css-values-4/#calc-computed-value),
[CSS Writing Modes 4](https://drafts.csswg.org/css-writing-modes-4/#propdef-writing-mode),
[CSS Display 3 transformations](https://drafts.csswg.org/css-display-3/#transformations),
[CSS Overflow 3](https://drafts.csswg.org/css-overflow-3/#overflow-properties),
[CSS Backgrounds 3 border width](https://drafts.csswg.org/css-backgrounds-3/#border-width),
[CSS Text 4 spacing](https://drafts.csswg.org/css-text-4/#letter-spacing-property) and
[CSS Text 3 alignment](https://drafts.csswg.org/css-text-3/#text-align-property).

## Font longhands and face requests

The supported registry now has **94 longhands** (93 reset by all), including
`font-family`, `font-style` and `font-width`. `Name::parse` canonicalizes the
legacy name `font-stretch` to FontWidth, sharing one declaration/cascade identity
and source order. This is an alias, not a second competing computed value.
The font shorthand, font-face/descriptors, variation/features/synthesis policy,
language/script fallback, actual font loading/matching and shaping remain open.

`properties::Families` owns one immutable shared array of ordered `Family` values:
Named decoded strings or GenericFamily enums. Quoted generic words stay Named;
unquoted identifier sequences join with single spaces, preserving decoded case
and characters for later matching. Full fallback lists and long multiword names
are not capped by quad-value scratch. CSS-wide/default and generic keywords
cannot occur inside unquoted names. Named system-font words such as caption are
ordinary family names in this property, not a system-font shorthand request.
Whitespace, comments, escaped names, quoted commas and implicit string closure
at EOF pass through the original Syntax tokenizer. Compilation bounds/cancel
checks apply before copying names; no Syntax/source document is retained.

Current Font 4 generic keywords and generic(fangsong/kai/khmer-mul/nastaliq)
are represented explicitly. These identify a requested generic; they do not
select an installed face or promise glyph coverage. Names are preserved, not
casefolded by the parser; actual CSS font-name matching still needs that stage.
Quoted and unquoted named forms can represent the same computed family.

Font style retains normal/italic/left/right/bare-oblique keywords or an explicit
angle. Literal angles convert units for the [-90,90] degree grammar check;
calculations clamp at computation using the existing numeric boundary, including
IEEE special handling. Explicit zero angle computes to normal; bare oblique
retains its computed keyword but requests a 14-degree slope. Font width keywords
map to 50/62.5/75/87.5/100/112.5/125/150/200 percent. Calculated percentages freeze
before face selection (negative calculations clamp to zero); the computed value
retains percentage type, not a layout percentage or a 0–1 opacity ratio.

Family/style/width prerequisites resolve before the subject font callback.
Font-relative numeric inputs in font-affecting properties use the parent's font
and line metrics, not the new subject's metrics. Other properties use the
subject's resulting metrics. Inherited computed angles/widths do not reevaluate
parent expressions. `FontRequest { size, weight, width, slant, families, axes }`
owns the shared family list, uses width=100 for normal and carries a FontSlant
selection value. `Host::font(&FontRequest)` receives this borrowed request;
unchanged full requests reuse the existing Font Arc. Equal used slants can
share a Font while retaining different computed syntax (bare vs explicit 14deg).
The request specifies face selection, never geometrically stretches a font.

Styles still have 80-byte headers; Font payload plus Arc controls is now 120 B
instead of 88 B (128 B with the fat-handle control). A thin family-list handle
uses triomphe's safe shared-ownership API; no third-party CSS or font engine is
embedded. Its strong count and stored length occupy the same two-word allocation
header counted for the standard fat Arc control. Lists/names share with the compiled declaration, Defaults,
fonts and computed arrays. `Memory::families_bytes` deduplicates them across all
these owners. The measurement excludes allocator/hash controls and borrowed
DOM/rules; it is distinct from process RSS. No new threads/I/O/polling/SIMD/GPU
path is introduced by these scalar grammars and dependency rules.

Independent native ports pin nine original WPT pages and two helpers in
`tests/font-fixtures`: **93/149 pass; 56 unsupported**, no unexpected
mismatch/error/panic. The 140 literal assertion inputs plus nine browser drivers
retain the denominator. Grammar ports check acceptance only. Ten family
computed assertions run the production initial document job with independent
canonical semantic references. All 47 original style/width computed assertions
require missing container-type context (style also links Ahem) and remain
unsupported, even for constant inputs. Tests do not supply real selected fonts
or execute CSSOM/JS/shaping/layout/rasterization. See the fixture README and
coverage artifact for exact scope.

Owned Wayland Chrome 157.0.0.0 probes record 23 local cases at 1×: all **17
selected supported observations match** native semantic values; **six explicit
reference-feature gaps are retained separately**. This reference ignores
font-width, so five such input cases cannot validate native current-spec width.
Its UI-Serif serialization preserves case and collapses quoted/unquoted output,
suggesting named-family handling; that differs from the current-spec generic
and is retained as a gap. It also reports no support for left/right style or the
generic() function. Legacy font-stretch observations validate width numeric
behavior independently. Those gaps are not passes and not website results.
Probe sources, observed values, binary identity and exclusions are recorded in
`measurements/2026-11-05-font-chrome.{json,html}`. No screenshot/visual review or
font selection claim follows from these structured CSSOM probes.

```sh
cargo test --release -p olo-web --test fonts
/usr/bin/python3 crates/web/tests/font-fixtures/extract.py
cargo run --release -p olo-web --example font_eval -- /var/tmp/font-report.json
# Exits 1 while any inventoried assertion/driver remains unsupported.
cargo run --release -p olo-web --example font_bench -- 10
```

The font benchmark uses fixed supplied metrics and independently generated
request/scalar expectations, not installed face metrics. Its new-feature costs
are not speedups over a prior browser. The common before/after style/raw probes,
retained payload, RSS, cold calls and maximum observed worker steps are in
`measurements/2026-11-05-font.json` with the replay runner. Adding enum variants
initially crossed the compiler's derived-clone inlining threshold; the raw
query regression and focused controls are retained in that evidence. Normal
builds retain derived value cloning, with direct known-type numeric/color copies
at the cascade boundary. `baseline-cascade-value-copy` selects the general outer
enum clone there; `baseline-font-family-fat` uses a fat standard Arc family
handle. All-feature tests cover both controls. Whole-clone forced inlining was
not retained: it traded numeric costs against slower colors. Raw owned-query
costs versus the parent remain profiling work; no settled performance acceptance
or universal speedup is claimed. These are ownership/code-generation choices,
not a new cascade semantic algorithm.

Primary rules: [CSS Fonts 4 families](https://drafts.csswg.org/css-fonts-4/#font-family-prop),
[style](https://drafts.csswg.org/css-fonts-4/#font-style-prop),
[width and alias](https://drafts.csswg.org/css-fonts-4/#font-width-prop) and
[CSS Values 4 font-relative contexts](https://drafts.csswg.org/css-values-4/#font-relative-lengths),
consulted 2026-11-05. Native loading/matching, UA styling, layout, painting,
networking, V8, native shell/agent control/Olo integrations and all website
journeys remain required by the full goal.

## Native font sources, matching and metric contexts

`olo_web::font` adds a worker-only native source catalog. `Builder::load_file`
reads at most 64 KiB between progress/cancellation checks; `add_owned` retains
its Vec without copying it into another byte allocation, and `add_shared`
leases caller-owned bytes. All faces in a TTC share one source. Failed additions
publish no partial faces; `finish` freezes metadata and a folded family-name
index. Scoped `FaceId`s reject other catalogs; a `Selection` keeps only its
source alive after the catalog goes away. No processes, OS discovery, file
watcher or font-service connection is hidden in these APIs.

The handwritten matcher narrows **width, style, weight**, in that order, using
CSS's directional searches and variable ranges. Localized legacy/typographic
family aliases participate; full/PostScript/subfamily names do not become
family names. Unicode 18.0.0 default full C/F folding uses the pinned normative
input, without normalization or Turkic tailoring. OS/2 oblique remains distinct
from labeled italic even when both have a nonzero post angle. Registered
wght/wdth/slnt/ital/opsz coordinates clamp to actual font ranges, with OpenType's
opposite slnt sign. CSS matching weight ranges clamp to 1–1000 while raw axes
remain available. Synthesis flags request future drawing; no geometric stretch,
bold or shear is rendered here. `font-style:left/right` explicitly reports
Unsupported. Automatic optical sizing currently maps CSS-pixel font size to
opsz, confirmed with an independently generated loaded variable font in Chrome.

`ttf-parser` **0.25.1**, a pure Rust OpenType table reader, supplies binary access;
it does not choose CSS faces or shape/render/layout text. The original module
remains `forbid(unsafe_code)`. SFNT/TTC directory checks enforce sorted unique
tags, table bounds/counts, file/retained payload limits, faces, names, axes and
metadata work; default caps include 64 MiB per file, 256 MiB retained referenced
payload, 16,384 faces, 256 collection faces, 512 tables and 63 axes (the decoder
cannot set variations with 64 axes). Names/aliases/axes are charged before
publishing. WOFF/WOFF2 is explicitly unsupported. These checks are **not a full
font sanitizer or adversarial execution sandbox**: opaque decoder operations
still need stronger whole-font CPU/cancellation bounds before web fonts.

`FontSet` binds the catalog, immutable explicit UA generic policy and matching
options. `scope(language)` normalizes a supplied language tag and creates an
identity for one complete font context. A cloned Scope keeps the identity but
starts with an empty worker-local cache. Bindings may specify language prefixes
and scalar ranges; complete generics use UA defaults when their mapped families
are unavailable, while incomplete generics allow the next CSS family. A present
face lacking a glyph advances to the next family. This is explicit embedding
policy, not automatic script/language inference or full cluster fallback.

`Scope::metrics` computes the first available font independently of cmap
fallback for `0` and `水`. A missing space glyph does not disqualify the first
font. ex/cap/normal line height use actual tables; ch/ic scale the selected
reference face's own units-per-em. Vertical mixed/upright/sideways policies pick
horizontal or vertical advances and expose synthesis/estimate provenance.
Fractional HVAR/VVAR and MVAR deltas survive until CSS-pixel scaling, with gvar
phantom points as a bounded fallback when HVAR/VVAR is absent. Optional missing
x/H-height inference only outlines simple TrueType glyphs with at most 4,096
points, bounded glyph+gvar bytes (256 KiB default). Composite/CFF outlines skip
this optional inference and use the documented CSS estimate; they cannot enter
recursive outline traversal just for metric estimation. Reference coverage is
cmap-only: GSUB, UVS, clusters, script features and shaping can change the final
metrics and remain work.

The FIFO cache retains family allocation identity to prevent address reuse,
and keys every numeric request, slant kind and writing/direction/orientation
axis. Defaults cap it at 4,096 entries and 4 MiB of conservative entry charges;
HashMap/deque capacities are additionally reported, so this is not a total-RSS
limit. Cancellation/errors do not publish cache entries. `Host::font_context`
is mandatory: parent/default Font reuse requires equal context as well as equal
request, and publication rejects context changes even when the stylesheet/font
revision stayed equal. Root Defaults must be made using the root's context.
`tests/support/native_font_host.rs` demonstrates the native Scope adapter.

Independent evidence: `tests/font-native-fixtures` contains 20 original
fontTools 4.60.1 fonts, deterministic generation, SHA-pinned Unicode input and
independent raw HVAR/MVAR references, including the real repository Noto font.
16 integration regression groups plus the full 1,606-record Unicode unit test
cover matching, shared leases, hostile prefixes/limits/cancel, metric fallback,
variable precision, cache bounds and style-job context coherence. The owned
Chrome probe records 32 loaded-font cases; the native test compares 24 auto
ch/ex/cap observations with an explicit one-font-unit plus 1/64 CSS-pixel
rounding tolerance. Chrome rounds some integer-font-unit metrics; this is not
exact CSSOM/rendering parity. No screenshot, scale/theme review or website
compatibility follows from a structured metric probe.

```sh
cargo test --release -p olo-web --test font_native
cargo run --release -p olo-web --example font_native_bench -- 10
```

The benchmark loads six pre-read shared repository Noto sources, freezes catalog
metadata, selects faces, compares identical metrics with the cache enabled/
disabled and builds static 1k/10k CSS jobs using real font metrics. Disk I/O,
shaping, rasterization, layout, JS, network and UI are excluded. The comparison
with parent 239cdb4b uses unchanged supplied-metric style/font/raw-cascade cases;
there was no native font implementation in that parent to benchmark. Raw runs,
source/binary hashes and replay commands live in
`measurements/2026-11-05-native-font{.json,-measure.py}`. The next section adds bounded Font object
interning; the prior raw width/margin query regression remains an explicit TODO.

Primary rules: [CSS Fonts matching](https://drafts.csswg.org/css-fonts-4/#font-matching-algorithm),
[font-relative units](https://drafts.csswg.org/css-values-4/#font-relative-lengths),
[OpenType variations](https://learn.microsoft.com/en-us/typography/opentype/spec/otvaroverview),
[loca](https://learn.microsoft.com/en-us/typography/opentype/spec/loca),
[glyf](https://learn.microsoft.com/en-us/typography/opentype/spec/glyf),
[ttf-parser 0.25.1](https://docs.rs/ttf-parser/0.25.1/ttf_parser/).

## Job-scoped selected Font sharing

Style jobs now intern selected `Font` objects across nonadjacent nodes, using
all numeric request bits, slant kind, writing/direction/orientation axes,
family **contents** and immutable `Host::font_context`. Separate equal family
allocations can share; different language/font contexts cannot. Exhaustive Rust
patterns require updating the key when FontRequest/Axes gains a field. Existing
parent/default Font reuse runs first, avoiding pool work on ordinary inherited
fonts. Direct `Style::compute` remains independent; no global cache or thread
is introduced.

`JobLimits::font_cache_entries` defaults to 4,096; zero disables the pool.
Saturation stops new insertions while existing entries stay usable. Failed
optional reservation also falls back to normal uncached selection. Numeric
values, failure/cancellation semantics and the complete style publication guards
remain in force. Pool hits do not bypass host revision/context/cancellation
checks. The pool is dropped at job completion; published styles own their Font
references and do not keep the pool alive. A failed job cannot publish a prefix.

`Job::font_cache_stats()` reports hits/misses/entries and HashMap slot payload.
The latter is worker scratch, charged to `JobLimits::retained_bytes`, and excludes
allocator/hash control overhead. Fonts and their referenced families are already
included once in the style Memory account; pool keys share the stored Font's
family allocation. These are payload counts, not hard process RSS limits. Entry
limits bound saturation; there is no polling or idle activity.

Nine integration tests cover nonadjacent/equal-source sharing, subject-context
separation, every numeric/slant/axis dimension, disabled/saturated pools,
cancel/revision failures, a context change during a hit, failed selection,
release lifetime and exact additional job-budget charge for observed pool
scratch. Very large family lists also skip optional caching: hashing is capped
at 64 KiB of family payload, with an early traversal exit. Literal supplied
metrics and pointer/callback assertions vary with the input dimensions to expose
incorrect reuse. These tests validate cache/style wiring; native matching,
shaping and website rendering remain separate evidence.

```sh
cargo test --release -p olo-web --test font_pool
cargo run --release -p olo-web --example font_pool_bench -- 10
cargo run --release -p olo-web --example font_pool_bench -- 10 reverse
```

The new benchmark compares disabled/default pools in the same binary for
repeated requests, separately parsed equal family lists and unique requests.
It records first calls, means, maximum 128-node slices, retained font objects,
style payload and pool scratch separately. Unique requests retain no fewer
Font objects and incur cache overhead; that cost remains in the results.
Parent facd8cd3 comparisons use the existing supplied-font/style/raw-cascade and
real-Noto benchmarks. The native metric cache control in `font_native_bench`
still controls only that cache; default style jobs now also use Font interning.
Raw observations, binary/source hashes, CPU/scheduler/RSS counters and replay
commands are `measurements/2026-11-05-font-pool{.json,-measure.py}`.

## Native text shaping and retained source runs

`text::shaping` connects native font selections to glyphs on a worker. Original
source/font/cache orchestration, Unicode grapheme segmentation and paragraph
itemization surround **HarfRust 0.15.0**, a pure Rust OpenType shaper using
read-fonts 0.46.0. The original engine crate still forbids unsafe code. The
older workspace RustyBuzz is archived; its maintainers recommend HarfRust.
This adds no browser engine, system process, font service, UI or rasterizer.

`Engine::shape_run` takes an unmodified native `Selection`, shared UTF-8 source,
byte range and explicit direction/script/language/features/context. It retains
absolute UTF-8 clusters and glyph IDs, advances, offsets and unsafe-to-break
flags in font units. `Run::advance(size)` scales font units to CSS pixels;
positive font y is upward. This is not a claim about final CSS layout rounding,
hinting, device metrics, spacing or font synthesis drawing. Features are explicit
OpenType inputs here; CSS feature/variant longhands are still incomplete.

The font adapter leases the existing Source through Arc/AsRef, without copying
its Vec or shared bytes. Parsed base fonts share across variation instances.
Worker caches key face/coordinates/synthesis and complete plan properties,
including language and feature ranges. Defaults cap base faces at 64, instances
and plans at 256 each, and conservative referenced source bytes at 32 MiB (TTC
faces may overcount). Saturation stops insertion; cache-disabled execution has
the same output. `clear` drops cache/source leases and the reusable buffer;
completed Runs keep their fonts alive. Source counters exclude opaque model/
shaper caches, allocator controls and process RSS.

Input defaults cap one shaping run at 16 KiB, output at 32,768 glyphs and features
at 64. Input ranges/context/variation finiteness and cancellation are checked;
opaque shaping is checked on return, and allocation/work failure discards its
output. The dependency has its own font-preprocessing and shaping allowances,
but these are **not** a hard total-memory or immediate-interruption boundary.
Stronger font sanitization/isolation and opaque cache accounting remain required
before web fonts. Only trusted explicit font sources are used by these tests.

`Analysis` owns paragraph embedding data from unicode-bidi's safe UAX #9
algorithm with our generated **Unicode 18.0.0** class/bracket properties. The
older bundled property table is disabled for this dependency. Unassigned
ranges follow normative `@missing` defaults; canonical bracket equivalence is
preserved. Paragraph levels precede L1; `visual_runs(line)` applies L1/L2 after
line breaking, copying only that line's data. Shaping handles marks/mirroring;
CSS markup embeddings, isolates and complete inline integration remain work.

The original constant-state `graphemes` iterator implements the current extended
rules with pinned grapheme, Indic-conjunct and extended-pictographic properties.
It handles Hangul arithmetically, RI parity, emoji ZWJ sequences and Unicode 18's
changed GB9c. The prior Unicode 17 segmentation crate missed ten current fixture
cases; no older segmentation runtime remains in this module. Printable ASCII
LTR/auto paragraphs, ASCII properties and scripts have focused fast paths;
`baseline-text-ascii` checks/measures equivalent full-Unicode execution.

`ParagraphJob` prepares one uniform-font-request, unwrapped paragraph. It resolves
bidi levels and graphemes, adopts common/inherited scripts within paragraph/level
segments, and chooses whole-grapheme cmap coverage. UVS selection retries
strictly then relaxes with explicit provenance; missing clusters and glyph-zero
counts remain visible. Primary coverage scans use a parsed charmap, and bounded
fallback memoization avoids reparsing for repeated graphemes. Font equivalence
uses full keys when Arc identity differs, so disabling caches cannot fragment
Arabic joining. This is not full Script_Extensions/paired-punctuation
itemization, GSUB suitability or multi-font mark fallback.

Default paragraph caps include 64 KiB text, 65,536 clusters, 4,096 bytes per
cluster, 4,096 runs, 131,072 glyphs and a 1,024-entry/1 MiB charged fallback
memo. `step(cluster_budget)` exposes progress/completed runs; failure poisons
publication. `for_style` checks the native Scope against the selected style's
font context. Empty paragraphs allocate no shaper/font cache. Finished glyph
data are Send+Sync for delivery to UI code; job construction/steps stay on the
worker. The constructor's bounded bidi/grapheme preparation is timed separately.

Whitespace/paragraph controls and vertical paragraph itemization are explicit
Unsupported inputs; the low-level run primitive can shape vertical text.
CSS whitespace preparation is now available separately (below); styled inline
shaping, spacing, line wrapping, safe boundary reshaping, hit testing, painting
and incremental invalidation remain.
A long same-font/script span that exceeds the run cap reports Limit; it is never
cut blindly where a ligature could cross graphemes. These restrictions are
prerequisites still to resolve in layout, not website compatibility passes.

Independent evidence is in `tests/shaping-fixtures`: four unmodified SHA-pinned
OFL fonts; native **HarfBuzz 15.0.0** C API vectors for 24 cases plus 15 longer
benchmark hashes; Unicode's entire **91,707-case BidiCharacterTest through L2**
and **853-case extended GraphemeBreakTest**. BidiCharacterTest excludes P1/L3/L4
by its own header. Older 14.2.1 vectors are retained separately: a Hebrew
mark-offset discrepancy disappeared with the matching native 15 oracle. The
Hebrew file also defaults to weight 100/legacy “Thin”; explicit CSS-normal
reference requests correctly use 400 and typographic aliases.

Fourteen regression groups cover raw/reference/paragraph glyphs, variations,
features, UTF-8 ranges, TTC sharing, mixed scripts, strict/relaxed/missing
fallback, line-level ordering, bounded caches and source lifetime, quota/cancel/
failure recovery, style context, empty text and worker delivery. Tests and
benchmarks compare glyph hashes outside timings; no screenshots or website
rendering evidence is implied.

```sh
cargo test --release -p olo-web --test shaping
cargo test --release -p olo-web --test shaping --all-features
cargo run --release -p olo-web --example shaping_bench -- 50
cargo run --release -p olo-web --example shaping_bench --features baseline-text-ascii -- 50
```

Raw release measurements and replay live in
`measurements/2026-11-05-shaping{.json,-measure.py}`. They separate font-unit
shape/drop from full paragraph prepare/step/drop, cold calls, constructor cost,
maximum 128-grapheme slices, source/glyph payload and whole-process RSS. Parent
e89c5dea had no shaping implementation; common before/after comparisons use
its unchanged style/font/cascade workloads. No navigation, GPU, UI/Chrome speed
or website acceptance is measured here.

Primary references: [HarfRust](https://github.com/harfbuzz/harfrust),
[HarfBuzz 15](https://github.com/harfbuzz/harfbuzz/releases/tag/15.0.0),
[clusters](https://harfbuzz.github.io/working-with-harfbuzz-clusters.html),
[Unicode 18 UAX #9](https://www.unicode.org/reports/tr9/tr9-52.html),
[UAX #29](https://www.unicode.org/reports/tr29/tr29-49.html),
[CSS text shaping](https://drafts.csswg.org/css-text-3/#shaping).

## CSS whitespace preparation

`text::prepare` implements phase I over DOM text in a single inline formatting
context, before bidi/shaping. Typed inherited `white-space-collapse` and
`text-wrap-mode` longhands support the two-longhand `white-space` shorthand,
legacy aliases, arbitrary ordering, defaults, CSS-wide resets and pending
substitution. Discard and white-space-trim remain unsupported. The registry has
94 longhands and 17 shorthands; this does not imply used-value/layout support.

`WhiteSpace::for_style` binds the prepared text settings to frozen computed
styles. Hosts supply one Arc source and an exact partition of nonempty UTF-8
spans, retaining each span's DOM node/local offset and font/style identity.
Span boundaries are transparent to collapsing; atomic/replaced inlines must
split contexts instead of being dropped. LF is a segment break, DOM CR is a
space, and NBSP/other separators/controls are not accidentally collapsed.
Bidi_Control characters remain in output but are transparent to phase-I
adjacency. Preserve modes keep tabs/line feeds for downstream layout. The
initial UA segment-break policy converts collapsible breaks to spaces; Chrome's
zero-width-space removal policy is an explicit remaining compatibility task.

Worker `Job::step(byte_budget)` resumes two linear scans, reports byte visits
and prepared bytes, checks cancellation and poisons publication on failure.
Defaults cap input at 8 MiB, 4,096 spans, 262,144 mappings and 131,072 trim
ranges. No per-character array or unbounded whitespace lookahead is needed.
Bulk memchr reverse searches and portable wide printable-ASCII scans/copies
keep ordinary text cheap. A scalar control preserves all semantics. Constructor
validation, first-edit prefix copying and final immutable allocation have
bounded total input sizes, but are not subdivided into byte-budget steps;
run construction/finish on workers too. These are storage/work quotas, not
a sandbox, fallible allocator or total RSS guarantee.

The immutable `Prepared` owns original/prepared text, spans and an encoded
source map including zero-output intervals for removed text. Unchanged text
shares its original Arc. Each stored record is 12 bytes: input/output starts
plus a packed span/kind; ends follow from the next record or string lengths.
`mappings()` decodes value records without allocating; source/output byte
queries binary-search starts. A 24-byte storage control has identical facts.
Glyph UTF-8 clusters can map back through this stage to the host's DOM spans;
this is not yet caret/grapheme hit testing or the structured browser API.

Independent evidence: seven preparation regression groups, 3,925 separate
regex-phase reference strings at multiple budgets and scalar-node partitions,
52 literal pinned WPT declaration acceptance assertions, and prepared-text →
native shaping/cluster-map integration. Original resources and local generator/
outputs are hashed in `tests/whitespace-fixtures/SOURCE.json`. No upstream JS,
CSSOM serialization, line layout, pixels or website pass is inferred.

An owned Olo Chrome 157 screen recorded 65 local observations. The diagnostic
projection comparison has 45 identical strings, 12 different strings and eight
reference-unsupported preserve-spaces cases, retained in measurements rather
than hidden. Phase-I text and innerText are different pipeline stages: three
differences retain CR in Chrome's plaintext, four concern its ZWSP break policy,
five concern transparent bidi controls. Native width/raster evidence is still
needed; the diagnostic executable exits 1 for differences/unsupported cases.

Phase II trimming, tab stops/hanging/wrapping, styled shaping and safe line
boundary reshaping remain next, followed by actual inline/block layout and
painting. The shaper still rejects preserved tabs/breaks until layout handles
them. No usable browser or website acceptance follows from this milestone.

References: [CSS Text 3 whitespace processing](https://www.w3.org/TR/css-text-3/#white-space-processing),
[CSS Text 4 shorthand](https://drafts.csswg.org/css-text-4/#white-space-property).

## Original Unicode line-break opportunities

`text::linebreak` implements the default Unicode 18 UAX #14 revision 57 rules
in original safe Rust. Generated raw Line_Break/General_Category/EastAsianWidth/
Extended_Pictographic facts remain distinct from the runtime algorithm. LB1
defaults resolve AI/SG/XX to AL, CJ to NS and SA marks to CM/other SA to AL;
raw classes remain available for future CSS tailoring. Quote categories,
dotted-circle identity, unassigned potential emoji and raw SA provenance are
retained. No new runtime dependency or external engine supplies decisions.

Worker `Job::step(byte_budget)` builds combining units incrementally and
applies ordered rules with two-unit right context and four fixed backward
unit values. It never allocates a character array or rescans unbounded numeric,
space or combining sequences. Normal lookahead retains up to three complete
units plus one pending unit; repeated combining marks do not grow scratch.
Progress, immutable completed frontiers, cancellation and failure-poisoned
publication are explicit. Defaults cap input at 8 MiB and 262,144 logical
opportunities, including EOT. A scalar can exceed the remaining byte budget
by up to three UTF-8 bytes; all construction/processing/finish belongs on workers.

Internal opportunities store four-byte offset/kind values. EOT is implicit,
derived from the shared source length, so an unbroken word needs no offset
allocation. An allocation-free exact-size, double-ended iterator includes that
required marker, even at zero for empty input; it does not itself create a
blank line. Input must be shorter than 2 GiB because one packed bit stores
mandatory kind. The measured x86_64 Job is 200 B. These are storage/work quotas,
not process isolation, fallible allocation or whole-browser memory bounds.

Focused ASCII property/pair shortcuts and portable SIMD letter/digit-run
transitions preserve the scalar context/lookahead frontier. The worker selects
a bulk/no-bulk specialization once per slice, avoiding speculative run checks
inside the multilingual scalar loop. Cancellation is checked before scanning
slice eligibility as well as during processing. Direct packed
class IDs avoid a redundant checked class decode. Full-rule, no-bulk and
class-table controls use identical facts/opportunities; all controls remain
measurement paths, not feature substitutes. `Breaks::source` shares the
prepared text Arc, and byte opportunities map through preparation to original
source spans. Choosing actual widths and caret hit testing remain separate.

`tests/linebreak-fixtures` pins unchanged Unicode files and source/license
reuse. All **19,346 default test strings** compare every boundary and mandatory/
allowed kind at one-byte and 4,096-byte job budgets. Five regression groups
also cover streaming/poisoning/limits/cancellation, UTF-8, hard/empty terminal
semantics, 100,000 combining marks, mixed iterator traversal, source sharing
and ASCII-run context invariance against scalar steps. The property generator
does not use test expectations or Olo decisions. Raw resources, generator,
generated facts and fixture documentation are hash checked.

**This is default UAX14, not browser CSS line layout.** `complex_context_scalars`
keeps raw SA usage visible; language dictionaries/complex-context segmentation
are still needed for scripts such as Thai, Lao, Khmer and Myanmar. CSS line-
break/word-break/overflow-wrap/nowrap/preserve-space policy, emergency grapheme
boundaries, safe width fitting/line reshaping, phase-II whitespace, actual
inline/block geometry and rasterization remain next. No CSS WPT, pixel or
website pass is inferred from the Unicode conformance result.

Release measurements and replay are in
`measurements/2026-11-05-linebreak{.json,-measure.py}`. Initial and guarded-loop
measurements stay in separate JSON files, with replay patches against the final
source; they motivated dispatch outside the character loop. Three alternating
CPU0 processes per variant separate full-rule, no-bulk and checked-class-table
controls, cold calls/constructor/max 4 KiB slices and exact offset payload from
whole-process RSS/CPU. The full browser/line fitter is not measured by these
worker-library cases. Residual mixed-workload variation remains explicit.

Primary references: [UAX14-57](https://www.unicode.org/reports/tr14/tr14-57.html),
[Unicode 18 corpus](https://www.unicode.org/Public/18.0.0/ucd/auxiliary/LineBreakTest.txt).

## Initial retained horizontal text geometry

`text::layout` connects prepared source, Unicode break opportunities and shaped
paragraphs. This is a worker library, with structured source/glyph/line data
for later painting and agent inspection; it does not yet lay out a DOM or draw
a page. `ParagraphJob::for_layout` accepts LF as a hard paragraph separator,
keeps it in bidi/source analysis, and excludes it from shaping context and
glyph/fallback counts. The older `new` entry still rejects paragraph controls.
LF-only input uses no shaping font cache. Paragraphs retain their original
font request, scope context and language for boundary reshaping/style binding.

Build `Arc<Text>` with `TextJob` from the **same source Arc** in Prepared,
Paragraph and Breaks. Width-independent geometry stores logical cluster edges
and integer advance prefixes (16 B each), break points (32 B each), per-run
indices and unsafe-edge shape results. Monotonicity checks measure adjacent
candidate endpoints rather than repeatedly summing the whole paragraph.
`Job` fits this retained text at a width and yields completed lines in bounded
candidate slices. Resize jobs share base glyph arrays; certified safe cuts use
slices, while unsafe edges reshape the smallest enclosing safe windows with
original font/script/language/paragraph context. No graphics context, timer or
idle task is created. Constructor, step, finish and font work all belong on
workers. Step budgets count candidates, not elapsed time; quotas/cancellation
are cooperative, not a hard CPU sandbox or fallible-allocation guarantee.

The initial scope is horizontal, one font request/size, uniform whitespace,
with font fallback and script/bidi runs. It handles greedy fitting, oversized
unbreakable words, SP edge trimming, preserved-space hanging, conditional
hanging at hard/end breaks, break-spaces opportunities, LF/blank lines and
start/end/left/right/center plus last-line alignment. Fragment order uses
line-specific bidi L1/L2 results. Integer font-unit sums convert to CSS pixels
once per original run, avoiding subtracting large floating-point prefixes.
Fonts with ligatures across spaces and context-deleted glyphs exercise unsafe
cuts; a narrower later candidate is still considered when advances are
nonmonotonic. Start-edge unsafe windows also delay the early-overflow exit.

`Options::for_style`, `ParagraphJob::for_layout_style` and `Job::for_style`
bind the computed font/context, CSS direction, line height and alignment to
this stage. They reject vertical text, nonzero letter/word spacing,
justification and non-normal word-break/overflow-wrap. `TextJob` rejects mixed
whitespace settings, tabs and unsupported Cc handling. Default limits cap
source at 64 KiB, cluster/break indices and fragments at 131,072 each, lines at
65,536, cached boundary runs at 512/32,768 glyphs, reshape input/output at
4 MiB bytes/4,194,304 glyphs, and candidate work at 4,194,304. Errors poison
publication, cancellation is explicit, and immutable outputs are Send + Sync.

**Remaining CSS behavior is substantial.** General inline boxes and mixed
fonts/sizes/styles, fallback-derived line-box heights/vertical alignment,
visual-edge trimming in complex mixed bidi, other-Zs hanging outside
break-spaces, tab stops, soft-hyphen realization, dictionaries, CSS break
classes/tailoring/emergency wrapping, justification, text transforms,
decoration, selection/caret/hit testing, fragmentation, block layout and
rasterization remain. Font-unit geometry is not Chromium's device/subpixel
shaping/raster scale. The lower-level Options API is not a complete CSS
conformance API; unsupported/missing behavior is not a website pass.

`tests/inline-fixtures` contains three reproducible original adversarial fonts
and **216 independent native HarfBuzz 15 layouts**. A brute-force external
fitter reshapes every candidate, including ones past overflow. Tests check
ranges/advances/glyph integers at one- and 128-candidate slices. Nine groups
also check literal whitespace/LF/bidi/style behavior, quotas, cancellation,
provenance and poisoned results. There is no independent mixed-bidi layout
or Chrome pixel oracle in this set.

`examples/inline_bench` measures build/drop, retained resize/drop, constructor
and worker-slice maxima, geometry payload and glyph/geometry hashes. The
isolated `baseline-inline-reshape` control reshapes every visual fragment;
all nine hashes agree with the retained-slice path. Raw parent/common and
control measurements, including the initial variant and its replay patch,
are in `measurements/2026-11-06-inline*`. Payload excludes shared source/fonts/
base glyphs, collection capacity/headers and hash/allocator overhead; process
RSS is separately recorded. No page, idle/frame, GPU or Chrome speed claim.

References: [CSS Text phase II](https://www.w3.org/TR/css-text-3/#white-space-phase-2),
[shaping across breaks](https://www.w3.org/TR/css-text-3/#shaping-line-breaking),
[CSS Inline line layout](https://drafts.csswg.org/css-inline-3/#line-layout).

## DOM boxes and initial normal flow

`layout::BuildJob` traverses a frozen connected light DOM and StyleTree on a
worker. It records compact parent/child/sibling box links, skips display:none,
flattens display:contents and groups text across inline elements between block
children. A block inside an inline splits those text groups without discarding
the continuing inline's source/style identity. `BoxKind::InlineContent` is an
internal formatting group; it is not a DOM element. TextSource spans preserve
DOM text nodes, style nodes and raw UTF-8 ranges. Prepared text mappings project
back through those spans. A Tree owns the style snapshot and its document
identity/revision; validate it against the current document before installing
results, and also check the caller's desired style generation. It does not
perform live DOM/style invalidation itself.

`PrepareJob` joins those groups to original text preparation, shaping, Unicode
breaks and retained inline geometry. Identical typography/source/span schemas
can share the whole immutable prepared paragraph. The temporary registry is
bounded to 4,096 entries by default and dropped at completion. Font object
identity scopes keys; source string equality, span ranges, whitespace and base
direction are checked before reuse. DOM/style mappings remain in each current
TextSource, so differently colored or separately located DOM nodes keep their
identity. A changed span schema cannot borrow another group's source map.
`cached_texts=0` disables both lookup and insertion. Cache glyph/index payload
counts shared paragraphs once, excluding shared fonts and non-glyph overhead.

`layout::Job` reuses that Cache at a width to compute original normal-flow box
geometry and retained text line/fragment layouts. It solves auto widths,
percentage dimensions/padding/margins, min/max constraints, content-/border-box
sizing, automatic inline margins, direction-dependent overconstraints,
definite/indefinite percentage heights, parent/sibling/empty-block positive and
negative margin collapse, root/flow-root/hidden-overflow barriers and relative
block offsets. It retains parent-relative rectangles; a painter should traverse
once carrying the content origin rather than independently walk ancestors for
every glyph. `bounds` is an inspection convenience with O(depth) work. Geometry
is not yet incremental across changed DOM/style generations or unchanged-size
subtrees.

Used lengths are quantized on a 1/64-device-pixel layout grid. Automatic margin
remainders preserve the direction's final unit. Borders already snap in
computed style and are not snapped again here. Styles expose their frozen
device scale; a layout viewport with another scale is stale and requires a
coherent new style snapshot. Typography remains at the previous unscaled
font-unit shaping scope: arbitrary device/subpixel glyph positioning and line
metrics are still open. The rectangle oracle uses a fixed-metric font and
explicit display/reset rules, not HTML UA defaults or arbitrary fonts.

Build defaults cap visited DOM nodes at 2,000,000, boxes at 1,000,000, depth at
1,024, visited text at 16 MiB, each raw text group at 64 KiB and source spans at
262,144. Preparation/flow also cap boxes/depth/retained source and work, and
inherit text-worker quotas. Build steps count traversal actions, prepare steps
count whole bounded text groups, and flow steps count traversal actions plus
nested text candidate work. A single prepare/fit action runs a bounded text
worker to completion, checking cancellation between its internal slices;
these are cooperative work budgets, not strict elapsed-time slices. Everything
including constructors/finish is worker-only. Immutable Tree/Cache/Layout are
Send + Sync; failures poison publication. Allocation is not fallible, quotas
are not a sandbox, and there is no whole-process memory guarantee.

The stage explicitly rejects flex/grid/table/ruby/list formatting, replaced
objects/controls/br/wbr, floating/absolute/fixed/clearance and vertical layout,
scrollbar-dependent overflow, intrinsic/fit-content sizes, nonzero inline
edges/borders and positioned/vertically aligned inline boxes. Mixed inline
font requests, sizes, directions, line heights or whitespace are rejected at
preparation. Colors/source identity survive grouping and feed the initial
paint/clip/scroll layer described below. Inline backgrounds/decorations,
caret/selection, Web IDL and complete CSS Inline/Block acceptance are still
required. Full HTML UA defaults, ordinary mixed markup and website integration
remain open.

`tests/flow-fixtures` pins 139 authored inputs and independent Chrome DOM
rectangles at 1×/1.5×/2×: 417 observations, each checked with one- and
128-action native budgets. The fixture README explains the measured iframe
viewport and float32 physical-rectangle/reciprocal-zoom export projection.
Seven regression groups also check literal flow/text behavior, source mapping
under paragraph sharing, stale generations/scales, cancellation, quotas and
poisoning. No pixel/theme/site acceptance follows. The native engine never
invokes Chrome; the local fixture server is development-only.

References: [normal block widths](https://www.w3.org/TR/CSS22/visudet.html#blockwidth),
[collapsing margins](https://www.w3.org/TR/CSS22/box.html#collapsing-margins),
[CSS Display](https://www.w3.org/TR/css-display-3/).

### Initial static page paint and native shell

`page::load` runs the current HTML→styles→boxes→prepared text→flow→paint
pipeline on a worker, collecting embedded author styles, an initial UA
stylesheet, title, inert-script count and stylesheet diagnostics. It uses
host Canvas as the opaque backing surface, leaving the root background
transparent so body canvas propagation works. This is not the full HTML
rendering stylesheet or runtime; external stylesheets/media contexts are
currently explicit errors.

`paint::BuildJob` retains the flow layout and source/style identity. Box
origins are accumulated once in preorder; original paint commands share the
existing shaped glyph arrays. Backgrounds, solid borders, simple normal and
relative paint order, foreground span colours/visibility and rectangular
ancestor clips form the first paint subset. Text commands split at colour
changes or 64 glyphs to bound tile work on long lines. An eight-way bounds
hierarchy queries only commands intersecting a damaged/visible region and
restores their paint order. Scene construction has incremental worker steps,
quotas and cancellation checks; sorting/font parsing is not a hard realtime
or security sandbox.

The worker outline cache retains actual variable-font instances and source
leases; pointer keys cannot be reused while a cache entry is alive. It is
bounded by font count/source bytes, glyph count/path payload and path segment
work. The pure-Rust `tiny-skia` raster library provides path coverage and its
SIMD kernels; no SIMD speedup is claimed without a measured counterfactual.
Only bounded tiles allocate raster surfaces. Global f64 coordinates subtract
the tile origin before conversion to f32, preserving fractional details in
far-scrolled documents. Solid borders use disjoint device-snapped strips and premultiplied corner
colour interpolation before one source-over. No per-border temporary surface
is allocated. Sharp background/clip rectangles snap to the device grid, and
painted text baselines are retained at device-pixel positions so the index
and agent bounds agree. Font masks, asymmetric/GPU corner coverage and
retained colour precision still have exact independent Chrome failures. Colour/SVG/bitmap font paint, hinting parity, full CSS backgrounds,
radii, decoration, opacity/compositing and GPU web raster are still required.

`apps/browser` composites those device-resolution tiles using the shared
kit's nearest texture path, one texel per physical pixel. Worker page keys
include source revision, viewport, scale and palette; resizes can retain
prepared text, scrolls retain the scene, and an LRU reuses raster tiles.
Dropping recycled visible images and evicting worker caches occur off the UI
thread. The current tile cache cap is 64 MiB payload, not an inclusive browser
memory budget. The shell has structured controls, page/visible text and status,
menus, cancel, local-file forwarding and kit reconnect/re-exec state. This
initial readable paint surface is not a semantic DOM action protocol. Full
interaction, isolation, network/JS and desktop/credential integration remain.

Native paint reference correction: DevTools emulated DPR was not equivalent
to actual Wayland scale for border snapping. Historical captures are retained
under tests/paint-fixtures/emulated; the authoritative native matrix now has
18 inputs × five actual output scales, 90 observations. 54 match exactly and
36 remain failures. Reference dimensions, input coverage and immutable source/
image hashes are verified. This is not website acceptance or complete font/
colour/GPU fidelity.


## Initial native network navigation (2026-11-06)

`apps/browser/src/network.rs` owns one pooled reqwest client and a current-thread
Tokio runtime on the page worker. TLS uses native trust roots; pending network
I/O races a wakeable cancellation token. No network runtime runs on the UI.
HTTP/1.1 and HTTP/2, gzip/Brotli/deflate/zstd decoding, bounded redirect chains,
final URL identity and file URL decoding are connected to the native address,
Go/Enter, Reload and Stop/Escape commands. Launch-relative files resolve against
the sender's cwd on the worker. Response HTML error bodies remain renderable;
plain text is escaped into an inert bounded preformatted document.

Responses are buffered before parsing, capped at 16 MiB for documents and
4 MiB per CSS response after transport decompression; document UTF-8 output
and generated plain-text HTML are capped at 16 MiB. The page caps aggregate
stylesheet UTF-8 source at 4 MiB and external requests at 128. These development
quotas are explicit errors, not website passes. HTTP pages cannot load local
files; HTTPS sheets cannot downgrade to HTTP, including through redirects.
Embedded URL credentials are rejected. Cookies, Fetch/web APIs, caches, full
origin/referrer/mixed-content policy and process sandboxing remain required.

`html::bytes::encoding` selects BOM, supported transport charset, bounded
1024-byte meta/XML prescan, then windows-1252. This is initial top-level input,
not the complete browser encoding/restart/locale algorithm. `text::Decoder`
performs bounded cancellable decoding; encoded document storage is released
before DOM/layout work. Linked CSS reuses `Syntax::parse_bytes` with transport
and document-environment encoding. Strict CSS MIME failures become resource
issues; HTML style/link order and the first base href are preserved.

`page::load_resources` delegates linked CSS to a worker resolver and retains
compiled rules. `Page::restyle` reuses DOM/rules on viewport-dependent resize,
scale and palette changes, without network requests. A host records whether
viewport units were used; the layout-only resize path remains for independent
styles. Desktop small/large/dynamic viewport dimensions currently coincide.
Unsupported media/import/conditional/SRI semantics remain visible issues or
explicit failures; this is not a complete stylesheet loader.

Run release regressions with `cargo test --release -p olo-web -p olo-browser`.
For native fixture checks, start a release browser on an owned Agent Screen
at 1280×800 and initial 2× (so physical output fits every tested scale), eval
its environment, then run `/usr/bin/python3 tools/browser/network-smoke.py
NAME /var/tmp/OWNED/output`. It checks redirects, final-URL reload, UTF-8 meta,
linked CSS pixels/order, no resize/theme/scale refetch, Stop recovery and plain
text, plus 15 live light/dark/night and 1/1.25/1.5/1.75/2× reviews. Night is dark
appearance plus forced 4500K night light. The fixture server is test-only Python;
production resource loading is Rust. It records no website compatibility pass.

`page_bench` measures the complete static pipeline, with feature flags, source
and geometry hashes. The network measurement script alternates frozen binaries
on CPU 0; the parent variant uses 6251094a's page.rs against the same new harness.
This isolates pipeline/resource-retention overhead, not HTTP throughput.
