# Browser development journal

## 2026-11-04 — project established

User requested a native Rust browser with an original engine, V8 allowed,
99% Chrome-level website compatibility, a 100-site complex/popular corpus,
native dark mode, Olo/agent integration and measured performance. An
active persistent goal tracks the full objective; it is not complete.

Starting revision: `33c6e1f7`, branch `browser`. Worktree initially clean.
Current browser is `apps/chrome`: launcher, installer, Passwords extension
and agent tree bridge. Previous NetPositive used Servo and was removed
(docs/TODO.md); restoring it would not satisfy the original-engine request.
Passkey support was removed with it and will need deliberate restoration.

Read HACKING.md's architecture, responsiveness, performance, app/agent,
headless testing, scale, journal, Chrome and keychain contracts; inspected
the Be Book's BMessage and BView. No apps launched on the person's screen,
no installation/default changes, no delegation.

Added architecture, explicit acceptance gates and this dedicated TODO and
journal. Research references are in README.md. Compatibility achieved:
**not measured; no claim**. No browser engine or shell exists yet.

## 2026-11-04 — original HTML tokenizer

Added `olo-web`, a toolkit-independent Rust engine crate. Its first
component is an original HTML tokenizer: borrowed text/name/value slices,
tag attributes (first duplicate wins), comments, doctypes, numeric and
all 2,231 named references, RCDATA, raw text, script escape handling,
plaintext and CDATA. Tree construction controls text modes. Current HTML
processing instructions follow the living standard. No existing browser
engine is embedded. There is no DOM, loader, layout, JavaScript or shell yet.

Pinned unmodified html5lib tokenizer fixtures at
`c5495c539ca8a79c3c5107f5f14010bcff9a35d5`, with their MIT license.
7,042 token/state comparisons pass; four non-scalar UTF-16 tests are
explicitly unsupported by the UTF-8 API. Parse-error codes/locations are
not implemented or counted as conforming. XML infoset-coercion tests
are outside the HTML tokenizer's scope. A deterministic 20,000-input,
six-mode adversarial smoke test checks progress and Unicode safety.
Limits and cancellation are tested; the library forbids unsafe Rust.

Validation: `cargo test --release -p olo-web` with both default and
`--features scalar-scans`; `cargo clippy --release -p olo-web --all-targets
-- -D warnings`; formatting. Full workspace release build passed (1m 56s;
existing GPUI warnings and a proc-macro-error2 future-compatibility notice).
No visible changes yet, so no screenshot journal entry applies.

Release measurements on Intel Core Ultra 7 356H, Rust 1.99.1:

- 4,096-attribute tag: linear duplicate scan **15,381.999 → 241.629 µs**,
  first token **12,773.031 → 235.937 µs**, after hash lookup for large
  tags. Small tags retain the vector path. Runs were not clock-controlled;
  this is a synthetic lexical workload, not UI latency.
- Three alternating runs pinned to CPU 0, 500 parses per workload:
  median SIMD/scalar parse time for 1,032,192 bytes of plain text
  **21.8 / 575.4 µs**; mixed article **2,038.9 / 1,757.9 µs**;
  references **4,127.4 / 4,179.1 µs**; attribute stress
  **245.6 / 226.6 µs**. SIMD's long-text win does not establish a
  mixed-workload win. Further optimization requires measurements.
- Entire benchmark process peak RSS **5,180 KiB**, including all four
  generated inputs. This is not the memory usage of a loaded webpage.

Commands and raw samples: `measurements/2026-11-04-tokenizer.json`;
`cargo build --release -p olo-web --example tokenize_bench`, then
`target/release/examples/tokenize_bench 500`; scalar build adds
`--features scalar-scans`. The example emits JSON and keeps only one
token at a time. Reproduce CPU affinity with `taskset -c 0`.

Website compatibility remains **unmeasured**. Next: the fail-closed
100-site evaluator, DOM/tree construction and standards coverage.

## 2026-11-04 — 100-site corpus and native comparator

Frozen curated coverage of 100 distinct domains across eight categories.
Every domain has a rank in Tranco snapshot JZPGY, generated 2026-11-04;
the full source CSV SHA-256 is
`218ab1bd9fb2074559928bdef831c33539651ce5904eff3b78f31798f5b59de7`.
The selection is explicitly not a numeric top-100 or independently
measured complexity ranking. `tools/browser/build-corpus.py` reproduces
the manifest and selected ranks from those pinned bytes.

Added `olo-web-eval`, a Rust artifact comparator: 100 sites × two
viewports × five scales × three themes = 3,000 cases, each requiring
four checkpoints (12,000 captures per engine). It checks paired run
provenance, assertion completeness, meaningful visible text, document
navigation/history and scroll evidence, landmark geometry, PNG pixels
and release cost budgets. PNG comparisons stream scanlines instead of
retaining whole images. Matching blanks, stale scroll pictures, missing
assertions/captures, invalid identities, differing environments and
blocked journeys cannot pass. No masks are supported yet.

The empty-evidence command exits 1 and reports **0/3,000 cases, 0/100
sites**, all 3,000 missing. Summary and resource observation are in
`measurements/2026-11-04-eval-empty.json`. This tests missing-evidence
handling, not website compatibility. No website or application journey
has been run. Category recipes still need site-specific executable
actions, stable fixtures/test accounts, a capture/replay driver and
pinned independent Chrome observations. Live scale-change checks remain.

Validation: five release integration tests, covering denominator
preservation, malformed manifests, a complete synthetic pair and its
behavior/geometry/pixel/metadata/cost failure modes, blank/transparent/
truncated PNGs and strict observation parsing. Synthetic tests are clearly
labelled and are not site successes. `cargo clippy --release -p
olo-web-eval --all-targets --no-deps -- -D warnings` passes. Without
`--no-deps`, an existing `unnecessary_sort_by` warning in `olo-crash`
is promoted to an error; that unrelated code is unchanged. Full workspace
release build and every-program crash-installation test pass.

Next: compact mutable DOM and original HTML tree construction, with
independent tree fixtures. Browser shell/layout/V8 and actual website
compatibility remain unimplemented/unmeasured. The full goal stays active.

## 2026-11-04 — compact document storage

Added `olo_web::dom`, separate from HTML tree construction and Web IDL.
Nodes occupy 40-byte arena slots, with u32 links and document-scoped
generational IDs. A removed/reused slot or a previous document's ID cannot
silently address a different live node. The stored types cover documents,
fragments, HTML/SVG/MathML elements, text, comments, processing instructions
and doctypes. Attributes retain insertion order and HTML case behavior.
This is storage, not a claim of a complete standards DOM implementation.

Unchanged character/attribute data references ranges in one shared source;
decoded owned strings transfer into the document without a second copy.
Names are interned. Arenas and owned text are reclaimed/reused explicitly;
detachment preserves nodes retained by a future JS wrapper, destruction is
only for unreachable subtrees. Iterative destruction avoids a recursive
drop stack. Limits cover source, nodes, attributes, names, owned text and
tree depth. Fragment insertion validates the whole operation before
relinking; document children, doctype ordering, cycles and parent/reference
ownership are checked. Identical value/attribute/reordering operations
leave the mutation revision unchanged. Debug output omits source and
character data so a diagnostic cannot dump session tokens/form secrets.

Eleven release tests cover ownership, stale handles, fragments, failed
mutation atomicity, doctypes, limits, source/owned text and insertion order,
decoded buffer transfer, secret-free Debug, iterative deep/wide deletion
and 20,000 reparentings against a separate parent-list model. The baseline
configuration has ten tests (buffer transfer is deliberately absent there).
These are storage tests; independent WPT/HTML tree comparisons remain.
Release engine tests and clippy pass. Full workspace release build passes.

Synthetic release storage comparison on the same CPU (CPU 0), three
alternating runs, 50,000 paragraphs, 100,002 live nodes, 50,000 attributes
and 4,250,000 source bytes. The `baseline-storage` feature reproduces the
initial second text copy and single-insertion heap allocation, with the
same tree semantics. Default builds eliminate both allocations:

| Case | Baseline median | Default median |
|---|---:|---:|
| Source-backed build | 6.589 ms | 6.393 ms |
| Owned decoded-text build | 8.177 ms | 7.729 ms |
| Reorder 1,000 paragraphs (source case) | 13.18 µs | 11.52 µs |
| Destroy 100,001 nodes (source case) | 0.607 ms | 0.672 ms |
| Identical value write (source case) | 4.48 ns | 4.46 ns |

Default source-backed run RSS: 14,496–14,532 KiB; owned-input runs:
17,840–18,024 KiB, including the original source and benchmark handle
vector. Node reservation: 5,242,880 bytes; attribute reservation:
1,048,576 bytes; source-backed owned text: zero. After destruction:
one document node, zero live attributes, zero owned text. Retained arena
capacity is reused, not a per-cycle growth. CPU clocks/load are not
controlled; the small timing differences are not broad performance claims.

Reproduce: `cargo build --release -p olo-web --example dom_bench` (add
`--features baseline-storage` for the baseline), then
`taskset -c 0 /usr/bin/time -v target/release/examples/dom_bench 50000`
(add `--owned` for decoded strings). Raw paired runs and commands are in
`measurements/2026-11-04-dom.json`. No network, CSS, layout, JavaScript,
frames or Chrome comparison is involved. Parsing/building will run on a
worker in the future shell. There are no visible changes to journal yet.

Next: HTML tree construction, adjacent text handling and independent tree
fixtures. No functional browser or website pass exists yet; goal active.

## 2026-11-04 — adjacent character data

Revalidated the worktree at `d1c7288e`; the preceding goal turn made
verified progress (four pushed commits, tests and release measurements).
The maintained tree fixtures now live in WPT, not html5lib-tests. Pinned
WPT revision `d7bb5bb848fcd304c10669288a211dbafd106d90` under /var/tmp
for the next tree-construction implementation.

Added bounded, amortized character-data appends. Adjacent ranges of the
same source merge with no copy. Noncontiguous/decoded text promotes to a
growing buffer; its reserved capacity is charged to the owned-text quota.
Unchanged/empty appends do not advance the revision. Failed appends preserve
the old value, revision and quota; replacing/destroying text releases its
buffer. Ordinary boxed text retains its compact representation. Node slots
remain 40 bytes. Invalid '<' text tokens now borrow their actual source
range, enabling source merging. Token semantics are unchanged.

Release tests: 13 DOM tests and the existing tokenizer suite pass;
clippy passes. The new tests include 100,000 Unicode appends, source-range
merging, quota failure atomicity and reclamation. No UI or idle work added.

Synthetic baseline reconstructs the full value for every append through
the existing read/set API. Default uses `append_value`. Three runs on CPU
0 (Core Ultra 7 356H), appending a four-byte character; median times:
10,000 appends **3.323 / 0.063 ms**, 20,000 **116.336 / 0.115 ms**,
40,000 **1,031.322 / 0.232 ms** (baseline/default). Growing-buffer capacity
is counted, not hidden. This is a character-data algorithm comparison,
not a page-load/UI/Chrome claim. Reproduce with `cargo build --release
-p olo-web --example append_bench` then `taskset -c 0
target/release/examples/append_bench`. Raw runs:
`measurements/2026-11-04-appends.json`. Next: original HTML tree builder.

## 2026-11-04 — original static HTML tree construction

Connected the tokenizer to the compact document store with an original
Rust tree builder. Implemented initial/before-html/before-head/head/
head-noscript/after-head/body/text/after-body/after-after-body and frameset
insertion modes, including implied nodes/end tags, lists, forms, buttons,
ruby, raw text/RCDATA/plaintext, scripting-flag noscript behavior, doctype
mode selection, active formatting reconstruction, Noah's Ark matching and
adoption agency handling outside table contexts. Scripts remain inert;
`scripting` currently controls parsing behavior, not execution.

The first independent run matched 2,476 static trees and found eight
mismatches, all missing `selectedcontent` initialization. Added initial
selection/disabledness handling and selected-option content cloning, using
an iterative, cancellable subtree clone with independent mutable buffers
and shared source ranges. Those eight cases now match. Live selectedness,
mutation hooks, events and the complete DOM/Web IDL APIs remain open.

Pinned 62 unmodified WPT fixture files at
`d7bb5bb848fcd304c10669288a211dbafd106d90`, with the upstream BSD license,
format README and per-file SHA-256 checks. Literal whitespace/control bytes
are preserved. All fixtures run: **2,484/3,904 static tree comparisons match**;
**1,420 unsupported** stay in the denominator: tables 438, foreign content
364, context fragments 412, templates 200, JavaScript execution 6.
Parse-error diagnostics are not checked. No website compatibility follows
from these counts. Summary: `measurements/2026-11-04-tree-coverage.json`.

The native `tree_eval` example always requires every static case and
currently exits **1**, as verified. Default unit tests reject any emitted
tree mismatch/error/panic and protect each previously matched case in
`tests/tree-coverage.json`; they are a regression gate, not proof that
unsupported cases work. `OLO_TREE_REQUIRE_COMPLETE=1` additionally requires
all 3,904 static cases. ENGINE.md documents this distinction and commands.

Validation: release engine tests (15 DOM tests, tokenizer suite, five
tree tests), clippy and workspace release build pass. Tree tests also
check cancellation, input/stack/formatting limits (markers included),
current doctype mode rules and 10,000 generated malformed documents.
Both default and counterfactual parser algorithms matched the independent
fixture outputs. Existing GPUI workspace warnings remain unchanged.

Release comparison on Core Ultra 7 356H, CPU 0, three alternating runs,
three parse/drop iterations per sample. `baseline-parser` reproduces
repeated attribute lookup and linear-pair formatting-attribute comparison;
default uses one-pass initial attributes and a hashed comparison for large
formatting attribute sets. Small tags keep the simple path:

| Case | Baseline median parse/drop | Default median parse/drop |
|---|---:|---:|
| Article, 65,544 nodes | 6.352 ms | 6.322 ms |
| 100,000 fragmented text tokens | 3.777 ms | 3.635 ms |
| One 4,096-attribute tag | 16.650 ms | 0.633 ms |
| Four formatting tags, 4,096 attributes each | 131.950 ms | 2.220 ms |

Median first complete parse: article **8.028 / 7.704 ms**, attribute case
**16.523 / 0.564 ms**, formatting stress **131.308 / 2.153 ms**
(baseline/default). All cases retain zero owned text bytes; source slices
are reused, including fragmented '<' runs. Peak benchmark RSS:
baseline **8,408–8,484 KiB**, default **8,324–8,572 KiB**. These are
synthetic complete-buffer parsing measurements, not network, first paint,
layout, JS, UI-frame or Chrome results. CPU clocks/load were not controlled;
the raw samples retain an article outlier rather than hiding it.

Reproduce: `cargo build --release -p olo-web --example parse_bench` (add
`--features baseline-parser` for the counterfactual), then
`taskset -c 0 /usr/bin/time -v target/release/examples/parse_bench 3`.
Raw samples: `measurements/2026-11-04-tree.json`. No visible UI changes.
Next: table insertion modes and foster parenting, then foreign content,
templates and context fragments. Browser/JS/layout/site acceptance and
the full goal remain unfinished and active.


## 2026-11-05 — table tree construction and measured common paths

Implemented the table, pending-table-text, caption, column-group,
table-body, row and cell insertion modes. Implicit `tbody`/`tr`/`colgroup`,
table-scope closing/reset, fostered insertion and adoption into the foster
location now repair malformed table markup. Scripts remain inert; this
is tree construction, not CSS table layout. Pending text merges contiguous
source ranges without allocation; decoded/discontiguous text grows one
quota-bounded buffer. Fostered text appends amortize into the preceding
text node. Cancellation continues through token/reprocessing loops.

All 62 pinned WPT files still run: **2,796/3,904** static trees match,
**312 additional matches**. **1,108 remain unsupported**: foreign content
440, context fragments 412, templates 250, JavaScript execution 6. Some
former table rejections now reach foreign/template rules, hence those
categories grow. The per-case baseline protects every current match.
No emitted-tree mismatch, error or panic remains. `tree_eval` still exits
**1**, verified; error diagnostics and browser/website behavior are not
part of these counts. Summary: `measurements/2026-11-05-table-coverage.json`.

Release measurements found an initial 14–16% common-text slowdown. Assembly
showed a larger insertion stack frame and a table-expanded dispatcher that
stopped inlining into the token loop. Kept rare foster insertion methods
and the table dispatcher outlined. Final alternating comparison with
committed `5f242dc8`, CPU 0, Core Ultra 7 356H, rustc 1.99.1, three runs,
20 parse/drop iterations per sample (CPU clocks/load uncontrolled):

| Static workload | Before tables, median | With tables, median |
|---|---:|---:|
| Article, 65,544 nodes | 6.331 ms | 6.276 ms |
| 100,000 fragmented text tokens | 3.693 ms | 3.756 ms |
| One 4,096-attribute tag | 0.639 ms | 0.634 ms |
| Four formatting tags with 4,096 attributes each | 2.234 ms | 2.218 ms |

Fragmented text is 1.7% slower in these samples; no zero-cost claim.
Median first article parse: **8.108 / 7.996 ms**. Benchmark peak RSS:
**8,264–8,488 KiB** across both builds. All common cases retain zero owned
text bytes. Raw samples include intermediate regressions and outliers:
`measurements/2026-11-05-table-common.json`, including source SHA-256.

Table stress, final build, same CPU, three runs of ten iterations:

| Workload | 10,000 rows/tokens | 20,000 | 40,000 |
|---|---:|---:|---:|
| Two-cell rows | 3.463 ms | 6.771 ms | 13.226 ms |
| Text fostered before each row | 3.201 ms | 6.432 ms | 13.184 ms |
| Contiguous fragmented table text | 0.155 ms | 0.310 ms | 0.638 ms |
| Decoded then fragmented table text | 0.158 ms | 0.318 ms | 0.663 ms |

These scale with input. Row trees have 50,007/100,007/200,007 nodes and no
owned text. Fostered-row text capacities: 16,384/32,768/65,536 bytes.
Contiguous text retains zero owned bytes; decoded text retains
10,001/20,001/40,001 bytes. Median first complete 40,000-row parse:
**17.247 ms**; worker-only API. Stress peak RSS **19,284–19,408 KiB**.
Raw observations: `measurements/2026-11-05-tables.json`.
Reproduce `cargo build --release -p olo-web --example table_bench`, then
`taskset -c 0 /usr/bin/time -v target/release/examples/table_bench 10`.
These are synthetic static parses; no network, JS, layout, UI or Chrome
performance is measured.

Validation: release engine tests (15 DOM, four tokenizer, six tree),
release clippy, counterfactual-parser tree suite and workspace release
build. The malformed-input test now includes table structures; dedicated
buffer tests enforce zero-owned source text and decoded-buffer quotas.
No visible UI changes or person's-session actions. Next: foreign content
and namespaced attributes, templates and context fragments. The full
browser goal remains active and substantially unfinished.


## 2026-11-05 — SVG/MathML document trees and compact attribute identity

Implemented foreign-content dispatch, MathML text and HTML integration
points, HTML breakouts, foreign end-tag recovery, self-closing elements,
SVG tag/attribute case adjustment and MathML `definitionURL`. Updated
scope, implied ends, list/heading handling, adoption, foster locations
and table reset to distinguish HTML elements from identically named
foreign elements. SVG scripts remain inert. CDATA declarations are
recognized in foreign namespaces and remain comments in HTML.

Attribute namespace identity now distinguishes the HTML parser's
XLink/XML/XMLNS attributes from ordinary colon-containing names.
Namespace bits share a 30-bit interned-name ID: attribute slots remain
**16 bytes**, nodes **40 bytes**, with no per-attribute side allocation.
Structured `attributes_ns` exposes qualified/local names, prefix, namespace
and value; `attribute_ns` reads expanded identity. Cloning, qualified-name
mutation/removal, source reuse and reclamation preserve this metadata.
Arbitrary XML namespace URIs and namespaced Web IDL setters remain open.
Namespaces declared by markup do not change HTML-parser element identity.

The unchanged upstream corpus now matches **3,236/3,904** static tree
variants, **440 additional matches**. **668 unsupported** remain: context
fragments 412, templates 250, script execution 6. All cases formerly
rejected as foreign content now match; that does not claim full SVG/MathML
or XML conformance. The protected baseline includes all 3,236 matches;
there are no emitted-tree mismatches/errors/panics. Full `tree_eval`
acceptance still exits **1**, verified. Diagnostics are unchecked.
Summary: `measurements/2026-11-05-foreign-coverage.json`.

Performance investigation caught an initial article regression of about
23%. Namespace-sensitive rules were repeating checked name/namespace
lookups and calling larger helpers. Added a single checked pair lookup,
kept the foreign rules separate from the ordinary dispatcher, and avoided
namespace checks when an element's name already rules out a match.
CDATA permission follows actual current-node changes. Text/ignored tokens
reuse that state, including HTML following an earlier SVG; the foreign
rules run once before HTML insertion-mode reprocessing.

Release comparison against `ba22847b`, Core Ultra 7 356H, CPU 0, rustc
1.99.1, three alternating runs of 20 parse/drop iterations:

| Static workload | Before foreign rules | Final foreign rules |
|---|---:|---:|
| Article, 65,544 nodes | 6.009 ms | 6.184 ms |
| 100,000 fragmented text tokens | 3.648 ms | 3.737 ms |
| One 4,096-attribute tag | 0.609 ms | 0.610 ms |
| Four formatting tags, 4,096 attributes each | 2.142 ms | 2.150 ms |

The final article/text samples remain **2.9%/2.4% slower**; that cost is
reported, not hidden by the added capability. Median first article parse
**7.569/7.603 ms**; common benchmark RSS **8,340–8,468 KiB** across builds.
All common cases retain zero owned text. A separate three-run comparison
at 40,000 table rows measured **13.790/13.956 ms**, fostered rows
**13.373/13.300 ms** (before/after). Table benchmark RSS
**19,212–19,376 KiB**. CPU clocks/load were not controlled. Raw data,
including intermediate variants and outliers, are in
`measurements/2026-11-05-foreign-common.json`, with source SHA-256.

New `foreign_bench`, three runs of ten iterations, median parse/drop:

| Static workload | 10,000 repetitions | 20,000 | 40,000 |
|---|---:|---:|---:|
| SVG icon/foreignObject groups | 7.679 ms | 15.139 ms | 30.238 ms |
| MathML/integrated HTML groups | 11.519 ms | 22.983 ms | 47.456 ms |
| Fragmented HTML text | 0.364 ms | 0.737 ms | 1.481 ms |
| Fragmented HTML text after SVG | 0.370 ms | 0.751 ms | 1.492 ms |
| One contiguous SVG CDATA run | 0.002 ms | 0.004 ms | 0.007 ms |

40,000 SVG groups retain **240,005 nodes/160,000 attributes**, MathML
**440,005 nodes/40,000 attributes**, both with zero owned text. First
complete parses: **33.501/55.825 ms**; worker-only API. Benchmark peak
RSS **45,128–45,172 KiB**. The initial mixed-text run measured
1.451 ms without SVG versus 2.525 ms after SVG; the final cache avoids
repeated namespace work. Those intermediate observations are single runs,
not an independently controlled speedup claim. Reproduce with
`cargo build --release -p olo-web --example foreign_bench`, then
`taskset -c 0 /usr/bin/time -v target/release/examples/foreign_bench 10`.
Raw data: `measurements/2026-11-05-foreign.json`. These are static tree
measurements, not SVG/MathML rendering, network, JS, layout, UI or Chrome
comparisons.

Validation: release engine tests (15 DOM, four tokenizer, seven tree),
counterfactual-parser tree tests, clippy and full workspace release build.
Malformed-document generation now mixes HTML/SVG/MathML tags. A dedicated
namespace-storage check covers expanded/qualified lookup, clone isolation,
no-op revisions, namespace-preserving updates/removal and reclamation.
No visible UI changes or person's-session actions. Next: templates and
context fragments. The browser goal remains active; shell, CSS/layout,
JS, integrations, website capture and compatibility acceptance remain.


## 2026-11-05 — template contents, document modes and measured ownership

Added a compact template node variant and a fragment host link. Templates
and their owned content roots still use **40-byte node slots**; no side
hash table or per-node pointer was added. Content roots have null DOM
parents, and actual template children remain separate. Both allocations
are preflighted together against node/revision bounds. Iterative cloning
and destruction include content roots and light-DOM children. Host-aware
cycle/depth checks reject cycles through template contents; owned roots
cannot be reclaimed independently. Node/text/attribute reclamation and
clone-failure cleanup are tested. Web-facing owner-document/realm
semantics remain open; storage ownership does not claim those APIs exist.

Implemented nested template insertion modes, adjusted insertion locations
(including fostered elements/text, comments, foreign content and adoption),
form-pointer isolation, HTML/body attribute suppression in template
contents, reset and EOF unwinding. A cached insertion flag keeps the
ordinary insertion check to one byte; it changes only at template-stack
and table-fallback transitions. The bounded reprocessing allowance now
includes stack depth, so 128 unclosed templates unwind at EOF. Independent
template cases exposed a missing frameset-ok reset when the parser creates
an implicit body after a head template; added the standard's reset.
Declarative shadow roots and template content patching are explicit
unsupported results, pending their actual implementation.

All **250** previously unsupported template document variants now match:
**3,486/3,904 static tree variants**. **418 unsupported** remain: context
fragments 412 and script execution 6. No tree mismatch/error/panic remains;
the baseline protects every current match. Native `tree_eval` still exits
**1**, verified; parse-error diagnostics are unchecked. Summary:
`measurements/2026-11-05-template-coverage.json`. These counts establish
neither website rendering nor JavaScript/browser compatibility.

Release comparison against `b0adb567`, Core Ultra 7 356H, CPU 0, rustc
1.99.1. Final common comparison: seven alternating pairs, reversed order
on alternating pairs, 50 parse/drop iterations per sample:

| Static workload | Before templates | Final templates |
|---|---:|---:|
| Article, 65,544 nodes | 5.815 ms | 5.993 ms |
| 100,000 fragmented text tokens | 3.567 ms | 3.578 ms |
| One 4,096-attribute tag | 0.593 ms | 0.594 ms |
| Four formatting tags with 4,096 attributes each | 2.077 ms | 2.069 ms |

Article **3.1% slower**, fragmented text **0.3% slower** in these samples;
no zero-cost claim. Median first article parse **7.291/7.302 ms**.
Common peak RSS **8,268–8,588 KiB** across builds. Intermediate runs
showed a large article discrepancy (7.110 versus 5.992 ms) that did not
recur in seven additional reversed-order pairs before the flag change
(5.970 versus 5.997 ms); no cause is claimed. Intermediate fragmented text
was 3.3% slower; combining template/foster checks into the cached flag
reduced the final measured difference. All observations are retained.
CPU clocks/load were not controlled; these samples do not establish
statistical significance or a Chrome performance comparison.

Three additional alternating pairs, ten iterations each, at 40,000 units:

| Existing workload | Before templates | Final templates |
|---|---:|---:|
| Two-cell table rows | 12.844 ms | 13.231 ms |
| Fostered table rows | 12.686 ms | 13.601 ms |
| SVG icon groups | 29.844 ms | 30.563 ms |
| MathML/HTML groups | 44.968 ms | 46.967 ms |
| Fragmented HTML text after SVG | 1.410 ms | 1.471 ms |

These retain the ownership-check costs, including the **7.2%** fostered
row regression, for further profiling; no performance target is declared
met. Table/foreign peak RSS across builds **19,240–19,424 /
45,064–45,280 KiB**. Full raw before/after/intermediate observations and
source hashes: `measurements/2026-11-05-template-common.json`.

New `template_bench`, three runs of ten iterations, median parse/drop:

| Workload | 10,000 templates | 20,000 | 40,000 |
|---|---:|---:|---:|
| Mixed HTML/SVG contents | 5.806 ms | 11.402 ms | 22.526 ms |
| Implicit table row contents | 5.576 ms | 11.119 ms | 22.025 ms |
| Fostered table text contents | 8.786 ms | 17.539 ms | 34.925 ms |

At 40,000 templates: **240,005/280,005/320,005 nodes**, respectively;
owned text **0/0/480,000 bytes**. Fostered discontiguous runs retain
12 bytes per template. First complete parses **25.544/29.309/36.445 ms**;
worker-only API. Peak benchmark RSS **29,588–29,756 KiB**. Unclosed nested
templates at depths 32/64/128/256 measured **0.0077/0.0175/0.0503/0.1663
ms**; ancestor preflight has quadratic cost in this bounded nesting
stress, unlike the linear sibling workloads. Default ownership-depth
bounds include content-root edges. Reproduce with
`cargo build --release -p olo-web --example template_bench`, then
`taskset -c 0 /usr/bin/time -v target/release/examples/template_bench 10`.
Raw samples: `measurements/2026-11-05-templates.json`. No network, scripts,
shadow DOM, content patching, rendering, UI or Chrome results are measured.

Validation: release engine tests (17 DOM, four tokenizer, eight tree),
counterfactual-parser tree tests, clippy and workspace release build.
Malformed generation now includes templates; dedicated checks exercise
owned content/light-DOM clones, cycles, quota atomicity, depth and failed
clone cleanup. No visible UI changes or person's-session actions.
Next: context fragments. The complete browser goal remains active.


Template milestone committed locally as `e7f1e33e`. Its push failed because
`gh auth status` reports an invalid CLI token. The connected GitHub app
authenticates but returns 404 for `benmyles/olo` and the known prior commit.
Requested restored repository access; local engine work continues, and
commits will be pushed once access is available.

Repository access was restored by the user, and `e7f1e33e` was successfully
pushed to `origin/browser`; local and tracked remote heads match.


## 2026-11-05 — context fragments with independent, cancellable arenas

Added `parse_fragment` and immutable `FragmentContext` metadata, either
an unattached name/namespace or a cancellable snapshot of a source element.
Snapshots carry namespaced attributes, quirks mode and form ancestry;
debug output omits attribute values. Parsing does not mutate that source
document. The result has its own scoped arena, detached fragment root and
quirks mode; temporary context/root/form nodes are reclaimed. Context
slots are local 32-bit links while public IDs keep document/generation
checks. This avoids a 24-byte optional public handle in builder state.

Document/fragment construction share the same token loop. Added adjusted
current-node dispatch and CDATA, fragment-aware mode reset, head/cell/
frameset handling and select-context rules. Initial RCDATA has no
appropriate end tag; raw/script contexts use HTML's permitted plaintext
equivalent while diagnostics are not collected. Literal end-tag markup
there remains text. Inert fragment parsing defaults declarative shadow
roots to disabled, so their markup remains ordinary template contents.
Document declarative roots, allow-shadow fragment APIs, content patching,
live insertion hooks, cross-document import/adoption, Web IDL and proper
owner-document/realm integration remain open.

The first complete corpus run found one fragment case failing under each
scripting flag: input in a select context. Added the standard's ignore rule
for input and nested select starts. **All 412 fragment variants now match**:
**3,898/3,904 total tree variants**, with **six JavaScript execution cases
unsupported**. Every file/flag still runs, and the baseline protects all
matches. Native `tree_eval` still exits **1**, verified against the final
release binary. Parse-error diagnostics are unchecked; this is not a
website, JS or rendering compatibility result. Summary:
`measurements/2026-11-05-fragment-coverage.json`.

Release comparisons against `e7f1e33e`, Core Ultra 7 356H, CPU 0, rustc
1.99.1; three alternating pairs, 20 iterations for common workloads,
ten for the remaining suites. Final median parse/drop:

| Existing static workload | Before fragments | Final fragments |
|---|---:|---:|
| Article, 65,544 nodes | 6.111 ms | 5.954 ms |
| 100,000 fragmented text tokens | 3.600 ms | 3.631 ms |
| One 4,096-attribute tag | 0.596 ms | 0.603 ms |
| Four formatting tags with 4,096 attributes each | 2.057 ms | 2.099 ms |
| 40,000 two-cell table rows | 13.648 ms | 13.557 ms |
| 40,000 fostered rows | 13.848 ms | 13.713 ms |
| 40,000 SVG icon groups | 31.115 ms | 32.990 ms |
| 40,000 MathML/HTML groups | 47.846 ms | 50.283 ms |
| 40,000 mixed template contents | 22.276 ms | 22.487 ms |
| 40,000 implicit template rows | 22.223 ms | 22.210 ms |
| 40,000 fostered template contents | 35.028 ms | 34.567 ms |

Median first article parse **7.530/7.352 ms**. SVG/MathML samples are
**6.0%/5.1% slower**; keep that for profiling, with no overall speedup or
performance-target claim. CPU clocks/load were uncontrolled. Earlier
common sets ranged from 1.5% to 6.4% slower article parsing before the
compact-context change; results vary, and all observations/source hashes
are retained. Peak RSS across builds: common **8,340–8,540 KiB**, table
**19,284–19,500**, foreign **45,156–45,232**, template **29,644–29,772**.
Raw measurements: `measurements/2026-11-05-fragment-common.json`.

New `fragment_bench`, three runs of ten iterations, median parse/drop:

| Fragment context/workload | 10,000 repetitions | 20,000 | 40,000 |
|---|---:|---:|---:|
| HTML sections/links | 5.390 ms | 10.454 ms | 20.737 ms |
| Table rows | 3.359 ms | 6.661 ms | 13.407 ms |
| SVG/HTML integration | 6.683 ms | 12.861 ms | 25.637 ms |
| MathML | 7.203 ms | 14.331 ms | 28.696 ms |
| Template rows | 4.375 ms | 9.094 ms | 17.712 ms |
| Decoded/literal RCDATA | 1.086 ms | 2.172 ms | 4.344 ms |

At 40,000 repetitions, live nodes are **240,002/200,003/200,002/
280,002/200,002/3**; the first five retain zero owned text and reuse source
spans. RCDATA retains **250,000/500,000/1,000,000** decoded text bytes
at the three sizes, in addition to its shared input. Node slots remain
40 bytes. First complete 40,000-unit parses: HTML **23.287 ms**, table
**13.344**, SVG **26.166**, MathML **33.658**, template **17.770**,
RCDATA **4.352**; worker-only APIs. Peak benchmark RSS **28,148–28,344
KiB**. Reproduce `cargo build --release -p olo-web --example fragment_bench`,
then `taskset -c 0 /usr/bin/time -v target/release/examples/fragment_bench 10`.
Raw samples: `measurements/2026-11-05-fragments.json`. These measure no
live-document adoption, Web IDL, scripts, network, layout, UI or Chrome.

Validation: release engine tests (17 DOM, four tokenizer, 12 tree), the
combined scalar-scan/reference-parser feature suite, clippy and workspace
release build. The 10,000 malformed-document samples also run as fragments
in HTML/table/template/SVG/MathML contexts. Additional checks cover form/
quirks inheritance, context encoding/integration, raw end-tag text,
select behavior, inert shadow markup, cancellation, quota errors, secret-
free debug output and source immutability. No visible UI/session changes.
Next: CSS syntax/style foundation and byte/stream input work. The full
native browser, JS, website evaluations and compatibility goal remain active.


## 2026-11-05 — original generic CSS syntax and measured linear parsing

Added hand-written CSS tokenization, source preprocessing/escapes,
strings/URLs, typed numeric values/units, hash flags, punctuation and the
special unicode-range descriptor mode. Default tokenization uses the
current 2026 identifier ranges and separate delimiter tokens. Long scans
check cancellation; input/token/decoded-text/depth/diagnostic/item bounds
are explicit. Diagnostics retain source-byte offsets. Their complete
standards conformance is not established by the syntax fixtures.

An immutable preorder arena stores components in **24-byte slots**.
Container traversal skips subtrees; blocks/functions have no recursive
Rust ownership, including error/drop paths. Unchanged values borrow the
shared source. Only decoded values own text; empty decoded values do not
allocate slots. Numeric values occupy the node; dimensions have sparse
12-byte metadata. Retained byte counters make `memory()` constant time.
Debug output omits CSS source and values. Read-only views borrow their
owning arena, keeping raw indices private.

Generic stylesheet/rule/declaration/single-item APIs retain source
spelling, important flags, mixed statement order, custom-property blocks,
modern whitespace and semicolon behavior. Declarations discard at-rules;
invalid markers remain inspectable. Raw rule blocks are parsed on demand
and the future style compiler must retain results. Selector/property and
at-rule context grammars, descriptor retokenization, CSSOM, cascade,
substitution, media queries, invalidation, encoding and streaming remain
unfinished. These records cannot yet be applied as style.

Pinned implementation-independent css-parsing-tests at
`203ce36bffd617db7f118c551e32794561fb273d`, including CC0 license/README and
per-file SHA-256. All **177 imported cases** run: **137 match** their old
expectations, **12 match exact reviewed current-spec updates**, and **28
byte-input cases remain unsupported**. No unexpected mismatch/error/panic
remains. The adapter folds adjacent literal delimiter pairs only for the
old representation, compares JSON numeric values without conflating their
separate type/spelling fields, and explicitly exercises unicode-range
mode for that old corpus. Default mode is checked independently.

The 12 reviewed cases cover the modern non-ASCII ranges, whitespace,
semicolon/end-of-input behavior, and discarded at-rules. Expectations are
specified from current algorithms, not captured from engine output. Native
ports additionally check identifier boundaries and nine whitespace/
important cases from two pinned WPT resources. These do not run their
JS/CSSOM/pages. The identifier source has a missing comma in its data
list; linewise extraction leaves the original file unchanged and makes
this limitation explicit. The native `css_eval` includes all cases and
currently exits **1**, verified; unsupported bytes stay in the denominator.
Summary: `measurements/2026-11-05-css-coverage.json`. No website or rendering
compatibility follows from this coverage.

Release measurements exposed two problems before accepting this milestone:

- Mixed blocks speculatively scanned the remaining list for every nested
  rule: quadratic behavior. Added header/block early exits, including the
  sole-block `!important` case, and single-pass declaration-value checks.
  `baseline-css-parser` retains the old lookahead as an equivalent-results
  measurement control. Independent fixtures run in both modes.
- The first SIMD string search scanned a full 64 KiB tail again at every
  escape. Limit the second search to the prefix before the first candidate.
  Also scan ASCII identifier/whitespace runs without repeated UTF-8 reads.
  The initial decoded-string parse/drop sample was **87.275 ms**, final
  median **2.626 ms** on the same input; initial stylesheet **23.596 ms**,
  final **8.083 ms**. These within-milestone comparisons combine changes,
  and the initial raw sample is retained, not a previous released CSS engine.

Final release comparisons: Core Ultra 7 356H, CPU 0, rustc 1.99.1;
three alternating runs, baseline parser three iterations per sample,
scalar/default five. CPU clocks/load uncontrolled. Median parse/drop:

| Synthetic workload | Quadratic lookahead | Scalar scans | Default |
|---|---:|---:|---:|
| 8,192 raw stylesheet rules | 8.264 ms | 8.163 ms | 8.083 ms |
| Long literals/comment, 2.13 MB | 0.052 ms | 0.838 ms | 0.059 ms |
| Escaped string, 500 KB | 2.664 ms | 2.557 ms | 2.626 ms |
| 100,000 dimension values | 7.860 ms | 7.829 ms | 7.737 ms |
| Mixed blocks, 2,000 rules | 41.000 ms | 0.564 ms | 0.561 ms |
| Mixed blocks, 4,000 rules | 170.623 ms | 1.138 ms | 1.126 ms |
| Mixed blocks, 8,000 rules | 688.525 ms | 2.242 ms | 2.240 ms |

The default mixed workload scales linearly with these sizes. Pure
literal/comment tokenization: scalar **0.833 ms**, default **0.057 ms**.
Escaped tokenization: scalar **2.564 ms**, default **2.636 ms**; SIMD is not
claimed faster on every short run. Default first complete parses:
stylesheet **9.394 ms**, literals **0.081 ms**, escaped **2.648 ms**,
long dimension values **8.426 ms**, 8,000 mixed rules **2.238 ms**.
All APIs are worker-only. The stylesheet workload parses components/root
rule boundaries; it does not recursively compile declarations or style.

The stylesheet has **262,144 nodes**, **6,291,456 reserved node bytes**,
**98,304 dimension bytes**, **zero owned text**, plus its 655,360-byte
shared source. Literal workload: **10 nodes**, **384 reserved bytes**,
zero owned text, shared source 2,128,598 bytes. Escaped workload:
**9 nodes**, **300,000 owned text bytes**, plus source. Dimension stress:
**200,005 nodes**, **6,291,456 node bytes**, **1,572,864 dimension bytes**.
Peak RSS across full benchmark processes: quadratic **13,300–13,424 KiB**,
scalar **13,276–13,400**, default **13,280–13,464**. String-slot capacity and
all other recorded pools are included in raw observations.

Reproduce `cargo build --release -p olo-web --example css_bench` (add
`--features scalar-scans` or `--features baseline-css-parser` for the
controls), then `taskset -c 0 /usr/bin/time -v target/release/examples/css_bench 5`.
Raw samples and source hashes: `measurements/2026-11-05-css.json`.
These are static syntax measurements, not selector/property validation,
style calculation, layout, network, JS, UI frames or Chrome comparisons.

Validation: release engine tests (six CSS, 17 DOM, four tokenizer, 12 tree),
combined scalar/reference CSS tests, release clippy and workspace release
build. CSS tests include 12,000 nested containers, long Unicode chunks,
resource/cancellation bounds, source borrowing, private Debug output and
10,000 malformed inputs through stylesheet/mixed/declaration entry points.
Existing workspace GPUI warnings remain unchanged. No visible UI changes,
GUI windows, session upgrades or installs. Next: byte decoding and CSS
selector/property/style compilation. The complete browser goal stays active.


## 2026-11-05 — Streaming text/CSS byte decoding and retained source buffers

Added reusable worker-only `text::Decoder` and original CSS encoding
selection/stream wrapper. UTF-8 validates borrowed runs with stable SIMD,
keeps at most three unfinished bytes, and uses standard replacement
semantics. Legacy encodings use Olo's existing pure Rust `encoding_rs`
library in process; no C library or external executable is added.
The decoder is **144 bytes** on this x86-64 build, with **no UTF-8 heap
buffer**. Legacy codecs use a fixed **16,384-byte staging buffer**.
The CSS wrapper is **1,256 bytes**, including its inline **1,024-byte**
sniffing prefix. It does no I/O, starts no threads and wakes no timers.

CSS selection follows BOM, valid protocol label, exact leading ASCII
`@charset "LABEL";` completed in the first 1,024 bytes, environment, UTF-8.
UTF-16 charset labels map to UTF-8. Selection reports provenance and BOM
length; BOM is removed, while generic syntax retains declarations for the
later style compiler to discard. Labels and legacy tables are supplied by
`encoding_rs`, not handwritten approximations of the standard aliases.
The common decoder intentionally has no HTML sniffing or BOM policy.

Input/output budgets are separate. Cancellation is checked within 64 KiB
spans and between sink callbacks; sinks must not block. Invalid data is
recorded in `had_errors`; resource/cancellation/sink errors and finalization
are terminal. Progress counts received input and bytes offered to the
sink, including an offer a sink rejects. Failed prefix emission preserves
that progress, rather than losing it in a temporary decoder.

`Syntax::parse_bytes` bounds raw and decoded source by the CSS input quota.
It allocates an initial destination once after preflight, trims spare
capacity, and retains the owned String buffer via shared ownership. This
removes the full String-to-Arc<str> source copy. `parse(Arc<str>, …)` keeps
its existing shared input; `parse_owned` rejects a retained capacity over
its input limit. `source()` now returns `&str`; `memory()` separately
reports source length and reserved buffer capacity. Component nodes remain
**24 bytes**. An immutable cached backing range avoids repeated source
ownership dispatch when storing source-backed tokens.

All **28 byte fixtures** now match their independent expected syntax and
canonical encoding. Total generic CSS corpus: **165 direct matches**,
**12 exact documented current-spec expectations**, **0 unsupported** out
of **177**. The fail-closed `css_eval` now exits 0 for this corpus. It
still checks the same pinned bytes/hashes and does not generate expectations
from the engine. This is generic syntax coverage, not CSS conformance,
selector/property validation, rendering or website compatibility.

Six new native tests cover every possible two-chunk split of those byte
fixtures, all **65,536 two-byte UTF-8 sequences**, malformed/overlong/
surrogate UTF-8, unfinished sequences, split astral characters, all **40
standard encodings**, stateful/large-output drains, every encoding
precedence and 1,024-byte boundary, limits, cancellation, terminal errors,
source-buffer retention and capacity reporting. Codec comparisons use
`encoding_rs` as an independent implementation of our UTF-8 path and as a
reference for our legacy wrapper/state/bounds. They do not independently
validate that library's legacy tables. HTML/CSS tokenization still needs
complete-input text; decoding alone does not make parsing incremental.

Measurements found and corrected two issues during this milestone:

- Repeated SIMD error localization was slower on dense malformed input.
  After the first malformed sequence, enter a separately compiled scalar
  loop for the rest of the bounded piece, returning to SIMD next piece.
  `baseline-utf8-errors` retains repeated SIMD localization as a control.
- The first fallback revision also revalidated a long *valid* prefix with
  the scalar validator when only the final character was split. Preserve
  SIMD validation for that prefix; an unfinished sequence is not itself a
  decoding error. Intermediate raw observations are retained. Source
  ownership initially added common-path lookup overhead; caching its
  immutable range restored the principal stylesheet workload to baseline.

Release measurements: Core Ultra 7 356H, CPU 0, rustc 1.99.1, three
alternating runs per variant, 15 iterations per byte case. Clocks,
thermals and other load are uncontrolled. Median milliseconds:

| Synthetic workload | Source-copy control | Repeated SIMD errors | Scalar scans | Default |
|---|---:|---:|---:|---:|
| Decode 2.10 MB ASCII, whole push | 0.026 | 0.025 | 0.053 | 0.026 |
| Decode 2.10 MB Unicode, whole push | 0.145 | 0.145 | 1.297 | 0.145 |
| Decode 0.90 MB malformed, whole push | 1.683 | 3.136 | 1.652 | 1.856 |
| Decode 1.05 MB Windows-1252 | 1.114 | 1.139 | 1.172 | 1.179 |
| Decode 1.80 MB UTF-16LE | 0.682 | 0.679 | 0.684 | 0.683 |
| Parse/drop 8,192 byte stylesheet rules | 8.678 | 8.803 | 8.742 | 9.039 |
| Parse/drop 2.13 MB literal/comment CSS | 0.329 | 0.190 | 1.687 | 0.193 |
| Parse/drop 1.05 MB Latin literal CSS | 1.348 | 1.250 | 2.185 | 1.268 |

The source-copy control differs only in final source-buffer ownership;
its decode results illustrate code-layout/load variation rather than a
semantic reason for different decode speed. Scalar recovery improves
dense-invalid throughput over repeated SIMD; scalar-only remains faster
on that synthetic invalid case. No universal SIMD or parsing speedup is
claimed. Default median first parses: stylesheet **11.233 ms**, literals
**0.316 ms**, Latin literals **1.265 ms**; these run on workers.

Retained source capacities equal decoded byte lengths for these cases:
stylesheet **655,360**, literals **2,128,598**, Latin literals **2,097,165**.
There are no additional owned token strings. The stylesheet still has
262,144 nodes/6,291,456 reserved node bytes; literal workloads have 10/5
nodes and 384/192 reserved node bytes. The copy control temporarily owns
another full source buffer; process-wide RSS does not isolate that peak.
RSS across complete benchmark processes: copy **16,244–16,356 KiB**,
repeated SIMD **16,468–16,544**, scalar **16,400–16,500**, default
**16,532–16,636**. No total-RSS improvement follows from these runs.

Common existing UTF-8-input syntax benchmark, three alternating 10-iteration
runs against the saved `1a5e78d9` executable, before → after milliseconds:
stylesheet **7.995 → 7.996**, literals **0.058 → 0.063**, escaped strings
**2.632 → 2.674**, long dimensions **7.748 → 7.875**, mixed 2k/4k/8k rules
**0.564/1.124/2.238 → 0.564/1.129/2.255**. Small regressions/variation are
reported; this milestone is not a blanket common-path acceleration.

Reproduce with `cargo build --release -p olo-web --example bytes_bench`
and `taskset -c 0 /usr/bin/time -v target/release/examples/bytes_bench 15`.
Use `--features baseline-css-byte-copy`, `baseline-utf8-errors`, or
`scalar-scans` for the controls. `css_bench 10` measures the existing text
API. Raw samples, intermediate measurements, CPU/compiler identity and
source/binary hashes: `measurements/2026-11-05-css-bytes.json`; independent
coverage: `measurements/2026-11-05-css-bytes-coverage.json`.

Validation: all release engine tests (six byte decoding, six CSS, 17 DOM,
four tokenizer, 12 tree); separate byte/CSS reference runs for copy plus
repeated-SIMD controls and scalar plus old generic lookahead; strict
release clippy in default/all-feature modes; full workspace release build.
Existing GPUI/future-incompatibility warnings remain unchanged. No GUI,
visible UI changes, session upgrades or installs. Next: CSS selector/
property/style compilation, descriptor grammars, and HTML encoding/stream
input. The complete browser goal stays active; site validation is still
0/3,000 configurations and no website is claimed working.
