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

## 2026-11-05 — Original selector compiler and bounded native tree queries

Implemented original Rust selector compilation/matching, bridging the
existing CSS component arena to the native DOM. Compiled lists, complexes,
steps, tests and nth metadata live in flat arenas. Tests are **24 bytes**
on this x86-64 build; shared symbols use integer references. The compiler's
symbol lookup map is released at completion. Invalid forgiving branches
rewind all arenas/symbols, and unusually sparse arenas are trimmed.
Selector Debug omits source, IDs/classes and attribute values.

The grammar covers namespaces/types/universals, IDs/classes, attribute
operators/ASCII flags, four combinators, structural/typed/filtered An+B,
logical functions and relative `:has`, specificity, inherited language
ranges, host direction/state hooks and common named pseudo targets.
Unknown invalid branches are forgiven only inside `:is`/`:where`; recognized
unsupported features and limits remain explicit errors. Nested `:has` and
pseudo-elements in real-selector arguments are invalid. Default namespace
exceptions, HTML folding, foreign case, attribute-value rules and quirks
ID/class behavior are separate. Coefficients beyond i64 fail explicitly;
`:empty` retains the established nonempty-text behavior, with the newer
whitespace exception still open. Full Selectors conformance is not claimed.

The matcher borrows a stable light tree. `DomTree` reads actual scoped DOM
handles, without flattening/copying snapshots or walking template content
as light children. Queries stream document-order IDs and specificity;
individual matches return the maximum matching branch's specificity.
Wrong-document/stale/scoping handles fail explicitly. A small shared
backtracking stack handles long complex chains iteratively; bounded
logical/filter calls have a hard nesting ceiling of 128. Work, result,
backtracking and index budgets, cancellation and sink failures are checked.
Virtual fragment scopes use compiled featureless eligibility: logical
`:scope` combinations and guarded `:has` work, while `*`, `:not(.class)`
and ineligible compounds cannot accidentally select the virtual root.
An implicit default namespace does not block an eligible featureless root.

Live interaction/control/media/direction state comes from `Tree`/
`StateSource`. Missing host data is `StateUnavailable`, not invented
attribute-derived state. Origin/pseudo states are distinguished. Public
queries mask visited history; a future private paint path must enforce
visited-style restrictions separately. No live state model, shadow tree,
Web IDL, CSSOM or renderer has been connected by this milestone.

Independent fixtures: unmodified cssselect2 test vectors/documents at
`fc22accd5fdba764d95263fa8e6c97ce24f75f3e`, original licenses, deterministic
literal/static-XML extraction, all hashes checked. Extraction never runs
a selector engine or creates expected matches from Olo. The native runner
accounts for **463 cases: 432 direct matches**, **two exact modern grammar
expectations**, **29 explicit unsupported cases**. The two adjustments
reject `of` arguments on nth-of-type/nth-last-of-type, whose current
grammars do not accept them. Non-passes include live state, nesting,
shadow/functional pseudo-elements, an HTML fixture context and an upstream
matches-only scope. The evaluator still exits **1**, and those cases stay
in its denominator. Native regression tests reject every unexpected
mismatch/error/panic and protect exact counts. XML models test the generic
tree interface; they do not establish XML loading or HTML parsing behavior.

Also checked **128 independent CC0 An+B cases**, an independent arithmetic
model, actual DOM queries/mutations, stale handles, inert templates,
virtual scopes, namespace/specificity rules, privacy/state hooks, Unicode,
resource/sink/cancellation behavior, nested index reservations,
**10,001-compound chains**, **5,000 nested functions**, **10,000 malformed
inputs**, oversized unknown pseudo names and linear adversarial substring
work. Case-sensitive substring searches use memchr; ASCII-insensitive
searches use compiled prefix tables with linear recovery. No per-match
lowercase copies. Huge unknown pseudo names are charged before any case
conversion and rejected without making huge lowercase copies.

Initial release measurements found quadratic repeated sibling counting.
Queries now lazily cache ordinary, typed and selector-filtered positions/
totals. Cache entries and pending nested-filter entries share an explicit
quota, including rechecking after nested filters allocate their indices.
Only queries needing nth indices build a table. Tables live for one query
against an immutably borrowed tree, then drop; later changes cannot reuse
stale indices. Individual-element matches use bounded scans. The equivalent
`baseline-selector-nth` feature retains repeated scans as a control.

Release measurements: Core Ultra 7 356H, CPU 0, rustc 1.99.1, three
alternating runs; reference three iterations per case, default ten.
Clocks/thermals/load uncontrolled. DOM construction and selector compilation
are outside query timing; the sink black-boxes IDs. Benchmark work budget
is explicitly raised to usize::MAX to observe the reference quadratic
cases. Normal default budgets remain in production APIs. Medians in ms:

| Selector workload | 1k scan → index | 2k scan → index | 4k scan → index |
|---|---:|---:|---:|
| `li.item > a` | 0.237 → 0.233 | 0.460 → 0.465 | 0.925 → 0.929 |
| `li:nth-child(2n+1)` | 13.702 → 0.295 | 57.615 → 0.570 | 218.778 → 1.136 |
| `li:nth-of-type(2n+1)` | 19.636 → 0.338 | 78.512 → 0.668 | 326.167 → 1.347 |
| `li:nth-last-child(2n of .selected)` | 32.700 → 0.332 | 134.979 → 0.644 | 529.928 → 1.288 |
| `li:has(> a)` | 0.259 → 0.256 | 0.514 → 0.513 | 1.002 → 1.020 |
| `[data-x*='aBABc' i]` | 0.214 → 0.226 | 0.444 → 0.442 | 0.875 → 0.878 |

The indexed cases scale linearly at these sizes; no universal speedup is
claimed. Default first 4k queries: child **0.925 ms**, nth-child **1.307**,
nth-of-type **1.343**, filtered nth **1.314**, has **1.028**, substring
**0.875**. Warm compilation of the benchmark's three-branch logical/
attribute/has/nth selector list: scan-control **1.402 µs**, default
**1.372 µs**. Identical compilation code is exercised in both modes;
the small difference is not attributed to indexing.

At 4k siblings: unfiltered work count **16,072,033 → 84,033**; filtered
work **224,124,033 → 188,033**. Indexed tables hold **4,000 entries**,
**7,168 usable hash-table slots**, peak **4,000 pending indices**. Common
child/has/substring cases have **zero** index entries/capacity. Compiled
selectors in these workloads account for **451–554 bytes** of retained
vector capacities/symbol data/headers, plus their root struct and allocator
bookkeeping. Index slot capacity is reported as slots, not misrepresented
as exact allocator bytes. Whole-process peak RSS: scan **4,096–4,132 KiB**,
default **4,548–4,716**. The temporary index memory buys the measured
scaling improvement; no total-RSS reduction is claimed.

Reproduce `cargo build --release -p olo-web --example selector_bench`,
then `taskset -c 0 /usr/bin/time -v target/release/examples/selector_bench 10`;
use `--features baseline-selector-nth` for the scan control. Raw samples,
intermediate observations, compiler/CPU identity and source/binary hashes:
`measurements/2026-11-05-selectors.json`. Coverage:
`measurements/2026-11-05-selector-coverage.json`.

Validation: all engine release tests; 13 selector groups in default and
scan-control modes; strict release clippy default/all-features; full
workspace release build; deterministic fixture extraction reproduced every
pinned hash. Existing GPUI/future-incompatibility warnings remain unchanged.
No GUI, visible UI changes, personal session actions, upgrades or installs.
Next: property/style compilation and stylesheet contexts/rule indexing,
remaining selector integrations and the rest of the browser architecture.
There is still no usable browser or demonstrated website compatibility:
site evidence remains 0/3,000 configurations. The complete goal stays active.

## 2026-11-05 — typed CSS colors and bounded SIMD conversion

Added an original color-value compiler over the existing CSS component
arena: named/hex colors, legacy/modern RGB/HSL, HWB, Lab/LCH, OKLab/OKLCH,
predefined RGB/XYZ spaces, current/system colors and nested `light-dark()`.
Values retain color-space/missing-component identity without source strings.
Absolute/current/system values need no heap allocation beyond Syntax;
conditional expressions own one shared flat arena. Both branches validate,
only the selected branch resolves, and the host must provide actual
scheme/current color/system palette. No Olo palette, dark adaptation,
style computation or painting is connected yet. Compilation is bounded,
cancellable and hard-limited to 128 nested calls; resolution/drop are iterative.

Colorimetric conversion is original Rust with f64 arithmetic, standard
transfer functions and normative matrix facts. It preserves out-of-gamut
coordinates; explicit absolute conversion materializes missing input as
zero. The separate byte endpoint clips explicitly. Gamut mapping,
interpolation/analogous missing propagation, math/substitution, relative
colors/mixing, profiles, CSSOM and complete Color conformance remain open.
The fact-only generator reproduces both named/matrix tables byte-for-byte
from the hash-pinned CSS Color 4 HTML after repository rustfmt formatting.
It executes no JS and reads no test expectations/engine output.

Independent native evidence: **12,703 cases**, comprising **9,728 direct
passes, 2,760 separately reviewed current-spec passes and 215 non-passes**.
The generic CC0 corpus accounts for all 8,061 pairs (5,284 direct, 2,760
Lab-family lightness/chroma/hue normalization updates, 17 non-passes).
Literal WPT ports account for 4,598 assertions (4,401 passes, 197 non-passes)
plus one unported expectation. Forty-three literal standard-space Color.js
conversion vectors pass at their independent tolerances; 212 other objects
are inventoried outside that specific absolute CSS contract. Imported
resources/licenses/revisions/byte hashes are retained. Both derived JSON
files reproduce exactly. Restricted reference decoding never uses the
engine to create expected values. No upstream JS/pages, CSSOM text, layout
or rendering are executed. `color_eval` exits **1**, preserving all gaps:
195 math expressions, two substitutions, six custom-profile values,
eleven device colors and one unported array-of-serializations expectation.

Performance work uses a bounded, allocation-free conversion worker that
returns counts after at most 4,096 colors per step. Cancellation is checked
at least every four; completed work survives cancellation. Homogeneous
linear RGB/XYZ batches use safe `wide` f64 lanes, with target-specific SIMD
or scalar fallback. All 225 space pairs, masks, mixed batches and tails
compare exactly to scalar conversion in native tests, including f64 storage.
No unsafe engine code, threads, timers, UI activity or GPU dispatch added.

The first SIMD attempt regressed nonlinear throughput (OKLab→sRGB median
9.984 ms scalar versus 14.660 ms attempted SIMD). Profiling/disassembly
showed an extra input Color copy in one build where the scalar control
elided it. Borrowing directly inside the worker removes that dependence.
Profiling also exposed repeated vector-row helper calls; explicit inlined
rows remove those calls. Earlier observations are retained with their
scope; intermediate source snapshots are not presented as rebuildable revisions.

Final release measurements: Core Ultra 7 356H, CPU 0, three sequential
alternating runs per mode, 20 warm conversions of 262,144 values per
workload, with no concurrent checks launched by this session. Clocks,
thermals and other machine load are uncontrolled. Median-of-run medians
for scalar control → default:

- Same-space sRGB materialization: **0.690 → 0.124 ms**.
- Linear Display P3 → linear sRGB: **3.257 → 0.896 ms**.
- XYZ D50 → linear Display P3: **3.258 → 0.895 ms**.
- Nonlinear OKLab → sRGB: **11.279 → 8.295 ms**;
  Display P3 → sRGB: **17.244 → 16.151 ms**. These scalar-fallback
  differences include compiler/code-layout effects, not SIMD arithmetic.
- Observed default first-step medians: **2.565–249.907 µs**; largest
  sampled default step **262.549 µs**. These are synthetic CPU worker
  latencies, not worst-case guarantees or input/UI latency.
- Color storage **40 → 20 bytes**, Value **40 → 24 bytes**, 10,000
  absolute values **400,000 → 240,000 reserved bytes**, compared with
  the f64 storage control. The sample conditional arena uses 208 versus
  368 bytes, excluding its Value/allocator bookkeeping. f64 is slightly
  faster for the two linear conversions (0.750/0.746 ms), so the retained
  f32 choice is an explicit memory/throughput tradeoff at CSS precision.
- Entire synthetic benchmark peak RSS: default **18,448–18,652 KiB**,
  f64 **33,676–33,964 KiB**. Includes generated/reset vectors and allocator
  retention; it is not webpage or browser memory. Typed parsing from
  prebuilt Syntax is separately measured; lexical/source costs are excluded.

Raw measurements, source/binary hashes, controls and commands:
`measurements/2026-11-05-colors.json`; per-file coverage/non-passes:
`measurements/2026-11-05-color-coverage.json`. Native release tests pass
all **66 engine tests**, including eight color groups, 256 byte/alpha
endpoints, changing/lazy context, source lifetime, extreme finite output,
limits, cancellation/resumption, 10,000-deep hostile syntax and 10,000
malformed values. Color tests also pass separately with f64 storage and
scalar conversion controls. Default/all-feature release clippy and
format/diff checks pass. Full workspace release build passes (2.62 s;
existing GPUI warnings and proc-macro-error2 future-compatibility notice).

No GUI launched or personal session changed. No visible change requires
a screenshot journal yet. Website evidence remains **0/3,000 cases,
0/100 sites**; there is no usable browser. The full goal stays active.
Next: property/style compilation and color integration, substitution/math,
stylesheet contexts/rule indexing and remaining selector/HTML integrations;
layout, painting, V8, native shell, Olo integration and actual site journeys
remain substantial required work.

## 2026-11-05 — relative-color detection guard

Final review found that the early `from` check treated any function name
as an unsupported relative color. Restrict it to the actual relative-color
function families. `imaginary(from red r g b)` and
`light-dark(from, blue)` now return Invalid; supported-family relative
syntax remains explicitly Unsupported. Regression tests exercise both.
All eight native color groups pass; independent counts remain 9,728 direct,
2,760 reviewed current-spec and 215 unsupported. This is a classification
fix, with no new style/rendering or website capability.

Three alternating release CPU-0 runs, 20,000 parses per prebuilt value,
before → after: hex **15.308 → 15.401 ns**, keyword **104.117 → 106.329**,
RGB **71.710 → 71.038**, OKLCH **68.963 → 69.783**, color()
**67.328 → 68.234**, nested light-dark **175.594 → 172.282**. No throughput
improvement claimed; load/clocks/thermals uncontrolled. Full benchmark
output and exact before/after identities are retained in
`measurements/2026-11-05-color-relative-guard.json`; the initial color
milestone remains independently rebuildable at `4f0cf03b`. Default and
all-feature strict release clippy pass. Full goal remains active.

## 2026-11-05 — original typed CSS numeric math

Added `css::math`, a reusable original numeric compiler and bounded CPU
resolver. Seven dimension exponents and a percentage hint occupy eight
bytes; compiled values occupy 16. Absolute units canonicalize, compatible
types combine, and relative fonts/viewport/container/percentage/tree counts
come from explicit host snapshots. No production environment is guessed.
Numeric percentages cannot silently combine with numbers; dimensions are
checked before folding. Raw NaN, infinity and signed zero remain inside
calculations until the property boundary materializes them.

The grammar covers calc, comparisons/clamp, rounding/mod/rem, trig/inverse,
power/log/exp/hypot, abs/sign, constants, progress/no-clamp and scoped
host-provided sibling counts. Compilation and resolution check work,
instruction, stack, nesting and cancellation limits. Programs own a flat
postfix arena; reused Scratch avoids repeated stack allocation. Sources
are not retained and Debug does not expose them. Static subexpressions,
zero physical-unit coefficients and canonical constant sum terms fold
after type validation. Percentage zero remains contextual. Host physical
factors reject negative values, including negative zero, so folding does
not depend on an unenforced signed-zero assumption. Percentage factors
can remain negative, including signed zero.

Pinned 23 WPT resources and their helper before evaluation; extraction
ports literal assertions, loop templates and recorded context changes.
The native adapter independently decodes canonical numeric references and
uses explicit representative font/viewport contexts, not layout. It is
not a CSSOM, JS or property/cascade test. Extraction reproduces exact bytes
and every manifest hash matches. **1,586 inventoried cases: 1,572 pass,
14 non-passes**, with no mismatch/error/panic. Remaining cases are six
nonnumeric wrappers, three serialization assertions and five property-limit
helper forms. The evaluator deliberately exits 1 until all are supported.
The same supported counts pass with both optimization controls. Full
property syntax/ranges, substitution, media/container progress, calc-size,
line-width rounding and style/layout hosts remain open. Math has not yet
been connected to colors; the earlier color corpus is unchanged.

Measured release builds on Core Ultra 7 356H, CPU 0: three alternating
sequential runs per mode, 2,000 compiles and 20,000 reused-scratch resolves
per prebuilt value per run. Figures below are medians of three run means;
clocks, thermals and other load are uncontrolled. Controls use the same
sources with folding disabled or sums left in operator order; they are
counterfactual controls, not an earlier production implementation.

- Static `calc(1 + 2 * 3)`, no-fold control → default: compilation
  **0.127 → 0.089 µs**, resolution **0.0183 → 0.0025 µs**, extra retained
  arena **144 → 0 bytes**. A pool of 10,000 separately compiled static
  values uses **1,600,000 → 160,000 accounted reserved bytes**.
- 256-argument constant min: compile **7.526 → 8.125 µs** (a cost), resolve
  **1.460 → 0.0024 µs**, extra arena **8,256 → 0 bytes**.
- 1,000-term mixed sum, uncombined-sum control → default: compile
  **27.697 → 25.441 µs**, resolve **5.6134 → 0.0106 µs**, instructions
  **2,001 → 3**, extra arena **32,832 → 128 bytes**. Canonical sum
  reassociation follows CSS simplification, with no general promise of
  bitwise equality to ordered extreme IEEE arithmetic.
- Short unresolved expressions have mixed results: percentage/font sum
  **0.0181 → 0.0187 µs**, clamp **0.0257 → 0.0237 µs**, sign expression
  **0.0258 → 0.0255 µs** versus the sum control. No uniform speedup claim.
- Default first-compile medians across cases **0.137–26.711 µs**; largest
  sampled first compile **34.801 µs**. First-resolve medians **0.015–0.258 µs**;
  largest sampled **0.266 µs**. Synthetic worker latencies only.
- Whole benchmark peak RSS: default **3,212–3,284 KiB**, no-fold control
  **4,688–4,932 KiB**, uncombined-sum control **3,244–3,300 KiB**.
  Includes allocator retention; not browser/page memory. Storage accounting
  excludes allocator metadata, compilation spill and reusable Scratch.

Raw samples, first-call latency, storage, commands and exact source/binary
identities: `measurements/2026-11-05-math.json`; per-file results and all
remaining cases: `measurements/2026-11-05-math-coverage.json`. This module
introduces no SIMD/GPU, timers, idle work or GUI; its optimization removes
repeated arithmetic and retained instructions. No website/per-frame
performance claims follow from it.

All **73 engine release tests** pass, including seven numeric groups:
independent cases, type/unit algebra, live environment/dependencies, raw
IEEE behavior, scoped host hooks, hostile depth/flat limits/cancellation
and 10,000 malformed values. Both controls pass all seven math groups.
Default/all-feature strict release clippy and package formatting pass.
A workspace-wide formatting check also encounters existing vendored
missing modules/format differences; those files are untouched. Full
`cargo build --release --workspace` passes (2.09 s; existing GPUI warnings
and proc-macro-error2 future-compatibility notice). No user session changes
or GUI launches; no visible change to screenshot-journal yet.

Full browser goal remains active. Website evidence is still **0/3,000
cases, 0/100 sites**; no usable browser exists. Next: connect math to color
channels with explicit live context and retained-memory measurements,
then property/style compilation. Layout, painting, networking, V8, native
shell, Olo integrations and real website eval journeys remain required.

## 2026-11-05 — connect typed math to color channels

Connected the original numeric compiler to color channels. Static math
folds to ordinary absolute colors with no retained heap. Dynamic math
retains four checked typed channels, their units/space and flat shared
numeric programs. Root values use Arc; conditional nodes refer to an exact
boxed leaf slice, preserving 20-byte nodes and the default 24-byte Value.
The compiler's four pending channel slots and first Functional stay inline.
There is no recursively owned expression tree or retained source syntax.

Shared normalization preserves grammar-specific percentages, missing
components, alpha/lightness/chroma clamps and hue normalization. Incompatible
numeric dimensions remain invalid even if multiplication might erase them;
percentage/number sums remain invalid. Hue contexts reject percentage
leaves even if an expression would cancel their dimensions. Legacy comma
and homogeneous-unit restrictions still apply. NaN/infinity/signed zero
remain inside math until the color boundary; infinite hue normalizes to zero.

Color Context now delegates unit factors and scoped tree counts to the
numeric snapshot contract, without production defaults. Only the chosen
conditional branch queries its context. One resolution stack is reused
across channels; a shared work budget covers conditional visits and all
selected numeric instructions. Actual numeric compiler work is charged to
the outer parse budget, with aggregate node/instruction and nesting limits.
`Value::dependencies()` exposes conservative current/system/scheme/math
metadata, including unselected branches; conditional dependencies are
cached while compiling. This supports future host invalidation and does
not implement a cascade, live style host, tree scopes or dark-page adaptation.

The same 12,703-case color denominator now has **9,921 direct passes,
2,760 reviewed current-spec passes and 22 explicit non-passes**, with no
unexpected mismatch/error/panic. **193 additional direct passes** comprise
155 computed-coordinate assertions and 38 validity-only assertions. The
fixture extractor records literal font sizes and preceding container-width
assignments, never derives context from expected alpha/color, and supplies
an explicit representative 16px font when unspecified. This is a native
numeric environment, not font/layout measurement. Extraction reproduces
exact bytes; all manifest hashes match; imported resources remain unchanged.
The evaluator still exits 1. Remaining cases: eleven device colors, six
custom profiles, two substitutions, one unported serialization-array driver
and two infinite-chroma computed serialization references. The latter inputs
parse, but the finite-coordinate API/reference adapter does not verify their
serialization; those cases are explicitly unsupported, not rewritten to pass.

Measured release builds on Core Ultra 7 356H, CPU 0, three sequential
alternating before/after runs, reversed in the middle. The before binary
comes from `8cf0a2e1`; both use the same ordinary color benchmark. Parsing
and resolution each run 20,000 iterations per prebuilt value; figures are
medians of three run means. Clocks/thermals/other load are uncontrolled.
No concurrent checks were launched by this session during samples.
Ordinary parsing before → after, in ns:

- Hex **15.564 → 17.642**; keyword **101.888 → 109.387**.
- RGB **68.353 → 80.105**; OKLCH **69.260 → 85.878**.
- Predefined color() **70.056 → 75.687**; nested light-dark
  **166.337 → 190.708**. The added grammar/metadata has a cost; no general
  parsing speedup is claimed. Initial separate-validation and lazy-boxed-
  scratch experiments were checked; final code combines static validation
  with normalization and retains inline scratch. The lazy allocation
  experiment was slower and was reverted.
- Ordinary resolution medians span **13.174–14.001 → 13.735–14.428 ns**.
- Color/Value storage remains **20/24 bytes**; 10,000 absolute values remain
  **240,000 bytes**. The sample five-node conditional arena grows
  **208 → 232 bytes** for its compact function-table pointer/dependency cache.
- Existing conversion throughput (ms, median of run medians) is
  **0.1224 → 0.1228**, **0.8947 → 0.8796**, **0.8883 → 0.8870**,
  **8.2138 → 8.2174**, **16.0366 → 16.0559** for the unchanged five
  workloads. No new SIMD arithmetic or conversion speedup is claimed.
- Whole ordinary benchmark peak RSS **18,436–18,468 → 18,644–18,788 KiB**,
  including generated/reset vectors, synthetic pool and allocator retention.

A separate contextual benchmark runs three times, with 2,000 compiles and
20,000 resolves per prebuilt value per run. No before throughput exists:
the old parser rejects these expressions. Representative parse/resolve µs:
static RGB math **0.1343/0.0094**, static polar math **0.2218/0.0087**, font
math **0.3599/0.0433**, container math **0.3216/0.0367**, tree-count math
**0.2600/0.0279**, conditional font math **0.4478/0.0507**. Extra retained
arenas respectively **0, 0, 280, 248, 216, 416 bytes**. A 1,000-term mixed
sum inside RGB takes **25.6041/0.0339 µs** and retains **232 extra bytes**.
First-parse medians span **0.647–30.127 µs**, largest sample **31.569 µs**;
first-resolve medians **0.034–0.817 µs**, largest sample **1.485 µs**. These
are synthetic CPU worker latencies, not worst-case or UI guarantees.

Pools of 10,000 separately compiled static values retain **240,000 bytes**;
the font-math pool **3,040,000 bytes** (including each independent arena).
Clones share those arenas. Contextual benchmark peak RSS **6,096–6,324 KiB**.
All storage accounting excludes allocator metadata, source Syntax and
transient compiler/resolver scratch. No browser/page memory claim follows.
Exact commands, source/binary identities, cold latencies and raw observations:
`measurements/2026-11-05-color-math.json`; all per-file counts/non-passes:
`measurements/2026-11-05-color-math-coverage.json`.

All **76 engine release tests** pass, including eleven color groups. Three
new groups cover static/type/channel rules, live host context/dependencies,
source lifetime/private Debug, lazy branches, cancellation/resumption,
aggregate channel/conditional quotas, 10,000-deep numeric syntax and mixed
color/math nesting beyond the shared hard cap, even with huge caller limits. All
eleven color groups pass with all measurement features together and with
f64 storage separately. Default/all-feature strict release clippy, package
formatting and diff checks pass. Full workspace release build passes
(2.04 s; existing GPUI warnings and proc-macro-error2 future-compatibility
notice). No GUI, timers, polling, idle work or personal session changes.
There is no visible change to screenshot-journal yet.

Full browser goal remains active. Website evidence is still **0/3,000 cases,
0/100 sites**; no usable browser exists. Next: compile property/style values,
then cascade/rule indexing with actual contexts and invalidation. Networking,
layout, GPU painting, V8, browser shell, Olo integrations and real website
journeys remain required, along with the explicit CSS/HTML gaps.

## 2026-11-05 — token-preserving custom properties

Added original `css::variables` token compilation and unregistered-property
resolution against explicit winning element snapshots. Values occupy **16
bytes** and batch compilation shares a compact source/token arena. Input
stylesheet Syntax/source is released; static values/aliases share retained
arenas. A view retains its whole batch, so callers must choose sensible
batch lifetimes. No recursively owned expression tree or third-party CSS
engine is introduced. The existing Syntax node remains 24 bytes.

A bounded assembler copies token structures, decoded names/units and raw
number spellings directly. It rebuilds function/block ranges and makes
implicit EOF closures explicit, without pasting variable strings or
retokenizing the output. Boundary comments prevent token fusion. Native
checks caught and fixed CDC formation from `--` followed by `>`; batch
spelling excludes separators belonging between values. Size checks avoid
scanning individual spellings when their whole shared arena fits the cap.
The spelling accessor is not CSSOM/author serialization.

var() handles current argument grammar, {} wrappers, case-sensitive names,
dynamic names from nested var(), empty fallbacks and comma-containing
fallbacks. Unused fallback dependencies are not evaluated. Empty values and
guaranteed-invalid remain distinct. Direct/substituted CSS-wide keywords
apply at custom-property boundaries; inherit/unset use supplied computed
parent values, and revert-family keywords require a real cascade callback.
Supplied animation taint follows dependency reads; non-animatable consumers
can exclude tainted references and use fallbacks. There is no animation or
property-registration engine behind these hooks yet.

The resolver memoizes per-snapshot properties, checks revisions on requests,
rejects mid-operation changes and discards partial caches on operational
failure. No timers, polling, threads, idle work or I/O are added. Work,
combined nesting, property count and cached arena reservation are bounded;
hard nesting is 256. Output byte/node overflow produces guaranteed-invalid
so enclosing var() fallbacks can recover. Remaining references are still
visited after invalid/over-limit output to detect overlapping cycles.
Cancellation/work/cache failures remain distinct errors. Context callbacks
must provide cheap stable data; a future style worker owns progress/cancel.

Review also found unmetered whitespace scans in cached substituted names
and CSS-wide keyword checks. These now charge each visited component.
Argument scans charge source spans, and spelling scans under smaller output
limits charge the value node range. A native regression group verifies
that repeated cached whitespace/name/size checks exhaust the work budget.

Pinned 25 WPT resources before evaluation and ported literal grammar/name
assertions and complete literal cycle groups, with independent expected
numeric/ident decoding. **151 inventoried cases: 119 pass, 32 explicit
non-passes**, no unexpected mismatch/error/panic. Four need other substitution
functions, two registered-property hosts, and 26 JS/CSSOM/style/layout or
reftest driver forms remain unported. A given native block honors duplicate
order/importance; this does not test selectors or origin/layer cascade.
The evaluator exits 1. Extraction reproduces exact bytes and every manifest
hash matches; imported resources remain unchanged.

Twelve native groups check typed color/math consumers, source lifetime,
shared storage/private Debug, boundaries/escaping/EOF, current argument and
fallback rules, CSS-wide/inheritance hooks, taint, revisions, cancellation,
limits, expansion recovery and hostile input. **1,000 generated eight-node
graphs** compare cyclic membership with an independent transitive-closure
reference. This found a case outside the imported groups: cached merged
cycle roots must be followed transitively while guards unwind. The fix and
all generated graphs pass. Guards also retain invalid cyclic participants
while allowing noncyclic callers to recover with fallbacks. There are
10,000 malformed inputs and 10,000 nested inputs in the native checks.

Measured release builds on Core Ultra 7 356H, CPU 0. Three sequential
alternating cold/warm runs (reversed in the middle), 2,000 compiles per
prebuilt value and 20,000 resolutions for ordinary cases, 40 for the two
large cases. Figures are medians of three run means. The cold API control
clears results before each request but retains worker capacities; both
modes memoize within a request. Other load/clocks/thermals, hash seeds and
allocator/code layout are uncontrolled. No concurrent checks were launched
by this session during samples. This was a new API: no old-production
variable resolver exists to compare with.

- 1,000 separately compiled values → one shared batch: compilation/drop
  **164.090 → 36.953 µs**, accounted reservation **408,000 → 69,488 bytes**.
  Handles are 16 bytes in both modes; each unique arena is counted once.
- Cold → warm resolution: simple reference **0.0872 → 0.0553 µs**, alias
  chain **0.2394 → 0.0542 µs**, substituted name **0.1928 → 0.1219 µs**, fallback
  RGB **0.3464 → 0.3028 µs**, numeric RGB **0.5192 → 0.4231 µs**, diamond
  **0.9357 → 0.0603 µs**, cyclic fallback **0.4023 → 0.1334 µs**.
- A 1,000-reference expansion: **85.2828 → 0.0669 µs**; the memoized output
  arena is **69,872 bytes**. These savings require retaining the resolver;
  the future style host should retain typed consumer results too.
- A separate paired corrected suffix-scan control → final repeated-cycle
  workload: **179.413 → 162.178 µs**. The final mark derives the earliest
  root from its first guard and skips an already-marked suffix, rather than
  scanning/writing the active suffix repeatedly. Same benchmark, same
  limit/taint/cycle semantics; this is a control of the corrected module,
  not a previous production revision. No uniform speedup claim.
- Across the cold/warm dataset, first-compile medians **0.203–1.219 µs**,
  largest sampled **2.187 µs**; resolver-construction medians **0.014–6.267 µs**,
  largest sampled **6.758 µs**; first-resolve medians **0.720–170.822 µs**,
  largest sampled **179.123 µs**. Synthetic CPU worker
  latencies, not worst-case or UI guarantees.
- Whole variables benchmark peak RSS: cold **3,832–4,064 KiB**, warm
  **3,864–3,948 KiB**. Includes frozen inputs, retained first return,
  synthetic pools and allocator retention. Shared arenas, including input
  arenas, are reported once; hash/control/guard metadata and allocator
  overhead are excluded from the arena accounting and separately bounded
  by counts. This is not page/browser memory.

Captured parent CSS/color binaries from exact `fe72fd58` module exports,
with new unreferenced modules excluded, then restored working sources.
Other reachable production and benchmark sources match that parent. Three
sequential alternating parent → final runs, CPU 0: CSS stylesheet
**8.0543 → 8.0836** ms, large declaration list **7.7194 → 7.7632** ms,
mixed 8,000 rules **2.2409 → 2.2390** ms. Color parsing ns: hex
**18.389 → 17.778**, keyword **107.907 → 106.766**, RGB **77.669 → 81.544**,
OKLCH **78.694 → 81.689**, predefined color() **71.712 → 75.112**, nested
conditional **187.458 → 189.295**. These unchanged paths have no intended
algorithm change; code-layout/load differences are not an optimization
claim. All raw common workloads, storage and cold latencies are retained.

Exact commands, control/source/binary identities and raw samples:
`measurements/2026-11-05-variables.json`; full per-file/native non-passes:
`measurements/2026-11-05-variable-coverage.json`. No new SIMD/GPU here;
source copies use native Rust memory primitives and the original token
machinery. Node layout and existing conversion algorithms stay unchanged.

All **88 engine release tests** pass; all twelve variable groups pass with
all measurement features together. Default/all-feature strict release
clippy, package formatting and diff checks pass. Full workspace release
build passes (0.41 s; existing GPUI warnings and proc-macro-error2 future-
compatibility notice). No GUI launched or personal session changed. There
is no visible change to screenshot-journal yet.

Full browser goal remains active: **0/3,000 website cases, 0/100 sites**,
no usable browser. Next: typed property compilation and actual cascade/rule
indexing/inheritance/invalidation hosts. Remaining variable work includes
registrations, early invocation/spread, other arbitrary functions, attribute/
URL taint, animation integration and CSSOM. Native taint/keyword hooks and
provided snapshots do not prove these hosts. Networking, layout, GPU paint,
V8, native browser shell, Olo integrations and executable site journeys
remain substantial required work. Avoiding output materialization once a
property guard is known cyclic is also a remaining profiling opportunity.


## 2026-11-05 — typed property declarations

Added original `css::properties` compilation for **88 longhands and 15
shorthands**, connecting generic declarations to typed math/color/token
values. Registry names use case-insensitive binary lookup; inherited flags
and initial spellings are explicit metadata. Logical properties retain
logical identities until an actual style/writing-mode host maps them.
The decoded-name API is not CSSOM setProperty or a string-name parser.

Box sizing, margins/padding/insets, border colors/styles/widths, gaps, flex
basis/direction/wrap/numbers/order, font size/weight/line height, opacity,
positioning/overflow and other core keywords have typed values. Structured
display values retain outer/inner/list-item and internal table/ruby roles,
including legacy aliases and order-independent modern forms. This compiles
grammars only; it creates no box tree, fonts, cascade or rendered content.

Lengths accept bare zero; dimensions and range restrictions are checked.
Negative nonnegative literals are rejected, while numeric functions clamp
at computation. Opacity normalizes percentages; integer functions round
ties toward positive infinity. Number line-heights remain multipliers.
IEEE specials are censored only at the property's boundary. Review fixed
unbounded finite-token overflow (e.g. `1e999px`): it remains valid and clamps
to finite f64 representation when computed. Font/percent/viewport/tree
snapshots are explicit; no layout basis or font metrics are guessed.

Quad/pair shorthands expand atomically, with flex-flow's unordered keywords
and omitted defaults. Importance applies to all expanded assignments.
CSS-wide keywords expand over every member. The existing variable scanner
validates generic/substitution syntax without allocating token copies for
ordinary typed declarations. Pending shorthands share one Arc and one
compact token arena across every longhand; the future cascade host must
resolve that shared value once after selecting winners and retain the typed
result. Invalid computed results become unset, preserving the winning
assignment rather than revealing an older declaration. Unsupported features,
cancellation/resource failures remain explicit. Custom declarations retain
case-sensitive decoded names, importance and valid empty token values.

Preflight bounds flat node ranges/source spans, and every nested scanner/
math/color/token compiler has work/depth/output limits. These are bounded
per-invocation passes (at most four shorthand members), not one exact global
instruction counter or a stylesheet memory cap. Defaults: 1 MiB input,
65,536 nodes, 1,000,000 work and nesting 64. Declared dependency metadata
includes numeric/color and pending-substitution inputs; property-specific
font, cascade, logical-axis and layout dependencies remain host work.
No thread, timer, polling, I/O or idle work is introduced.

Imported **126 unmodified pinned WPT resources**, with independent literal
extraction/expected decoding: **1,109 inventoried native cases, 821 passes,
288 explicit non-passes**, no unexpected mismatch/error/panic. Parsing ports
check acceptance, not CSSOM serialization from the original helper. Actual
shorthand member identities and literal references are compared. Numeric
references use independent canonical decimal/px parsing; simple keyword
and absolute rgb references are checked independently. Expected strings
are never passed through the engine's property/color/math parser. Missing
font/percent/intrinsic/used-size, coupled overflow/positioning, display CSSOM,
compound references and live element contexts stay unsupported; so do
nonliteral JS drivers. The evaluator exits 1. Literal extraction reproduces
exact bytes and every source manifest hash matches. No website pass follows.

Eight native groups check registry defaults, dimensions/zero/percent/ranges,
integer rounding/IEEE boundaries, expansion/logical identities, structured
display, typed colors, source release, shared pending substitution and
invalid computed values, cancellation/quotas and 2,000 generated hostile
inputs. All groups pass in both storage controls and with all measurement
features enabled. These are native token/typed-value checks, not CSSOM,
actual cascade, style/layout/painting or JS execution.

Release measurements: Core Ultra 7 356H, CPU 0, rustc 1.99.1, three sequential
alternating four-slot/one-slot runs, middle reversed; 20,000 operations per
case. Figures are medians of three run means. Original lexical/source and
statement parsing costs are excluded from the property benchmark. Numeric
scratch and the variable resolver are retained; pending resolution includes
typed reparsing and assignment allocation. Display/color resolve samples
are reference-access controls, not color conversion or box computation.
Other load, clocks/thermals, allocator/code layout are uncontrolled; no
concurrent checks were launched by this session during samples.

The initial four-inline-assignment representation made **every declaration
184 bytes**. One inline assignment reduces that to **64 bytes**, with Value
**32 bytes**, Assignment **40 bytes**, and Assignments **176 → 56 bytes**.
This is an initial implementation control, not a previous production API.
Both controls pass the same checks. Retained batches of 1,000 declarations:

- Scalar width: **184,000 → 64,000 bytes**, no assignment heap allocations.
- Four-value padding: **184,000 → 224,000 bytes**. One slot spills **160,000
  bytes** of assignments; this is a real tradeoff, not a universal reduction.
- One pending width each: **576,000 → 456,000 bytes**, including distinct
  pending/token arenas. Each shared allocation is counted once.

Compile/drop: scalar width **0.1170 → 0.1051 µs**, contextual numeric width
**0.2135 → 0.2135 µs**, four-value margin **0.3547 → 0.3412 µs**, border colors **0.5032 → 0.4634 µs**,
display **0.0889 → 0.0749 µs**, font weight **0.1122 → 0.1006 µs**, pending padding **0.2643 → 0.2795 µs**,
pending width **0.3049 → 0.2933 µs**. Pending padding resolution
**0.2209 → 0.2303 µs**; it must be cached by the future style host after
its first substitution. These numbers do not establish a uniform speedup.

First-compile final medians **0.595–5.043 µs**, largest sampled
**7.119 µs**. First-resolve final medians **0.042–2.857 µs**, largest sampled
**3.328 µs**. Synthetic worker latencies, not UI or worst-case guarantees.
Peak whole-benchmark RSS: four-slot **3,548–3,988 KiB**,
one-slot **3,724–3,960 KiB**. Retained capacity accounting excludes
allocator metadata, original Syntax and runtime host state; RSS includes
transient buffers, libraries and allocator retention. This is not page or
browser memory, and the lower declaration size does not guarantee lower RSS.

Captured exact `baa75e86` parent exports/variable files with new modules
excluded, restored working source in finally, and compared unchanged CSS,
color and warm variable benchmarks. Three alternating parent → final runs:
CSS stylesheet **8.0822 → 8.0610** ms, declaration list
**8.2205 → 7.7566** ms, 8,000 mixed rules
**2.2077 → 2.2205** ms. Color RGB parsing
**81.222 → 80.258** ns, OKLCH **85.691 → 84.480** ns.
Warm variable aliases **0.0530 → 0.0568** µs, 1,000-reference
cached expansion **0.0655 → 0.0650** µs. No intended
algorithm change to these paths.

Named-color medians initially **92.654 → 104.123 ns** (+12.4%), with individual
samples spread across **90.100–111.426 ns**. A focused three-pair follow-up
using the same binaries gave **111.446 → 104.450 ns**, reversing the direction;
individual samples **90.370–120.942 ns**. Both full datasets are retained.
The variation does not establish a stable regression/speedup or its cause;
no improvement is attributed to the unchanged parser. Performance acceptance
for a real browser remains unproven.

Raw data, exact commands, parent/control/source/binary identities and checks:
`measurements/2026-11-05-properties.json`; per-file coverage and all remaining
cases: `measurements/2026-11-05-property-coverage.json`. No new SIMD/GPU code;
existing native memory/token/math/color machinery is reused. New compilation
will run on workers and retained style results must avoid per-frame parsing.

All **96 engine release tests** pass, all eight property groups pass with
all features, default/all-feature strict release clippy and package format
checks pass. Full workspace release build passes (**2.31 s**, existing GPUI
warnings and proc-macro-error2 future-compatibility notice). No GUI launched,
no personal session changed and no visible UI to screenshot-journal yet.

Full browser goal remains active: **0/3,000 website cases, 0/100 sites**, no
usable browser. Next: actual cascade and element/style snapshots, stylesheet
contexts/rule indexing/inheritance/invalidation and computed-value hosts.
Remaining property grammars include fonts/families, backgrounds/images,
border-image/radius, transforms, grid/alignment, animation/transition and
aliases/descriptors. HTML/encoding/Web IDL, networking, layout, GPU painting,
V8, native shell/Olo integrations and executable site journeys remain required.


## 2026-11-05 — original frozen cascade and stylesheet contexts

Connected original typed declarations/selectors into frozen stylesheet sets
and per-element cascade/defaulting snapshots. Named/nested/anonymous layers
share ordering across sheets of each origin; namespaces reset per stylesheet.
Layer names validate atomically before adding order. Resource failures poison
builders so partial state cannot be finished/published. Invalid syntax remains
recorded/ignored; unimplemented properties/selectors/at-rules/nesting remain
observable issues. No imports, media/supports/container/scope or keyframes yet.

All rule roots now share one flat selector collection and interned symbols.
Rejected roots rewind their unpublished tail. Borrowed namespace/cancellation
inputs exist only during compilation calls, so retained compiler state does
not self-reference a stylesheet. Frozen declaration slices avoid small Vec
capacity waste. Original CSS Syntax/source is released after compilation.
There is no author-text/CSSOM rule mutation host behind this representation.

A necessary subject ID/class/type index admits candidates, including universal
fallbacks for logical-only branches. Actual matching still decides every
branch and its highest matching specificity. ASCII folding can admit false
positives across HTML/quirks/XML but cannot replace the matcher. Reusable
worker stamps deduplicate rule branches/candidate keys. The separate
baseline-cascade-index feature scans all rules without retaining index data;
default builds index them. Private visited painting is explicitly unsupported.

Priority uses origin/importance, supplied encapsulation, inline attachment,
layer, matched specificity, supplied proximity, source/declaration order and
rule identity. Presentational/animation/transition ordering is represented,
not their actual execution or source generation. Namespace/inline/layer cases
run through the native DOM. Scope/shadow/encapsulation hosts remain incomplete.

The cascade keeps lower candidates for defaulting and revert-family traversal.
Dynamic custom rollback remains inside the variable resolver's active guards,
so a lower declaration cannot hide self/dependency cycles. A rollback-only
cycle defaults to unset, distinct from cyclic var()'s guaranteed-invalid result.
Independent upstream source inspection found an initial rule-chain bug:
revert-rule must descend to a lower priority outside the current rule. That
fix handles chains; layer/rule combinations can re-enter a candidate and need
an explicit cycle guard. The tentative upstream cycle case stays identified.

Pending shorthand expansions are shared per identity and animation-taint
policy, with conservative retained typed-memory limits. Invalid computed
output becomes unset, without revealing an older declaration. Results remain
Specified/Initial/Inherit markers; a style host must copy inherited computed
values instead of evaluating a parent's specified font expression in its child.
Only inherited custom tokens have a concrete already-computed parent callback.
Added all over the 87 implemented resettable properties (16 shorthands total),
excluding direction/custom properties. This does not complete the registry.

Limits bound candidates/phase work/rollback depth, compiled rules/declarations/
layers/issues, typed pending retention and nested compilers. Default cascade:
65,536 candidates, 4,000,000 work, depth 128 (hard 256), typed pending 16 MiB
plus original variable bounds. A private bottom-up merge checks cancellation/
work between comparisons; matching sums actual selected-rule work. These are
bounded phases, not one exact whole-style-job instruction/memory counter.
Typed accounting may count shared programs repeatedly; controls/allocator
metadata are count-bounded separately. No thread, clock, I/O, polling or idle
work is introduced. Long work/drops/progress belong to future worker jobs.

Independent evidence: **19 unchanged pinned WPT resources, 136 inventoried
native cases, 128 passes and eight explicit non-passes**. All eight are
unexecuted geometry/rasterization drivers. The adapter parses inert original
HTML, compiles literal sheets/inline declarations, selects actual target nodes,
runs cascade/defaulting and supplied inherited custom/color contexts, and
independently decodes RGB/decimal/identifier references. One rem reference
uses an explicit 16px root factor; the native palette is fixed Light, not the
person's session or all-theme evidence. Reftest background components are
checked separately from geometry; a green color is not a rendered green square.
JS, CSSOM, animations, layout/screenshots and websites remain unexecuted.
Evaluator exit stays 1. Extraction reproduces exact derived bytes and manifest
hashes match; the unpooled control has the same independent 128/8 outcomes.

Ten native groups cover ranking/defaulting, real DOM/inline/nested sheets,
namespaces, atomic invalid layers, rollback chains/cycles, all exclusions,
shared typed expansion, taint-policy isolation and work/retained/cancellation
bounds. Default/full-scan/all-feature modes pass. These are supported-token/
selector/cascade ports, not complete style or web-platform execution.

Release measurements: Core Ultra 7 356H, CPU 0, rustc 1.99.1. Three sequential
unpooled/pooled-dense/final storage runs (middle reversed), then three
alternating full-scan/indexed pairs. Each sample has three compile/drop or
whole-document match/default/numeric-width rounds; figures are medians of
three run means. Source/HTML parsing is excluded. Every generated node width
is independently verified in an untimed pass; bulk buffers/node/rule data are
warm and first calls separate. No concurrent checks launched by this session;
other load, clocks/thermals, allocator/hash/code layout remain uncontrolled.
These compare initial prototypes/new-API modes, not old production cascade.

| Rules/elements | Unpooled rule payload | Pooled rule payload | Compile/drop, unpooled → final | Match/default/resolve, unpooled → final |
| --- | --- | --- | --- | --- |
| 1,000 | 1,080,098 B | 358,909 B | 0.8812 → 0.6021 ms | 0.2848 → 0.2723 ms |
| 2,000 | 2,161,306 B | 718,909 B | 1.6688 → 1.1653 ms | 0.5720 → 0.5517 ms |
| 4,000 | 4,323,722 B | 1,438,909 B | 3.4987 → 2.2327 ms | 1.1306 → 1.0965 ms |

Pooling changes rule/declaration/selector capacities and shared symbols; the
index still costs memory. The isolated final pooled-dense control restores
only dense property ranges and unconditional merge-buffer cloning. Sparse
ranges stay inline for up to eight declared properties and do not allocate
all 88 ranges for an element with one candidate. One-candidate payload
**1,728 → 416 bytes** (candidate/range capacities, including inline ranges;
not the entire snapshot/allocator). A one-candidate sort avoids an unnecessary
merge allocation. Pooled-dense → final match/resolve at 1k/2k/4k:
**0.2921 → 0.2723 ms**, **0.6145 → 0.5517 ms**, **1.1679 → 1.0965 ms**.
No universal latency claim; some storage-only cases vary in the other direction.

| Rules/elements | Full rule scan | Indexed | Candidate visits, scan → index | Extra retained index payload |
| --- | --- | --- | --- | --- |
| 1,000 | 55.4663 ms | 0.2711 ms | 1,000,000 → 1,000 | 87,680 B |
| 2,000 | 222.5767 ms | 0.5606 ms | 4,000,000 → 2,000 | 175,360 B |
| 4,000 | 911.5985 ms | 1.1147 ms | 16,000,000 → 4,000 | 350,720 B |

These are deliberately selective descendant-class rules; universal/common-key
styles may have many candidates. This proves this workload's scaling, not all
site selector performance. Index building adds compilation/storage cost.
The full-scan/default native outcomes and generated expected widths match.
Payload sums include rule/declaration/selector/index capacities; exclude the
RuleSet header, hash controls, allocator metadata, borrowed Syntax/DOM and
host state. Benchmark values are scalar widths, with no typed heap programs.
First-compile final medians **0.633–1.981 ms**, largest sampled **1.982 ms**.
First-match/default/resolve final medians **1.943–4.622 µs**, largest sampled **5.156 µs**.
Synthetic worker observations, not UI/worst-case guarantees.
Whole storage benchmark peak RSS, unpooled: **14,680–14,780 KiB**.
Whole storage benchmark peak RSS, pooled-dense: **8,144–8,476 KiB**.
Whole storage benchmark peak RSS, indexed: **8,244–8,512 KiB**.
RSS includes original buffers/DOM, compiler temporaries, libraries and
allocator retention. The scan control's lower RSS includes its missing index;
none of these numbers establishes page/browser/Chrome memory.

A common-path check found an initial **4–13% public selector-match slowdown**
when accounting results shared a return path. Returning only work and a generic
output specialization partly recovered it, but extra cost remained. Restored
the original public setup/body/return path exactly and kept a private accounted
root setup; matching algorithms remain shared in Machine. The public method's
source hash is byte-identical to the parent. The final paired direct-match
check no longer reproduces the regression; failed and intermediate raw data
remain in the artifact instead of being discarded.

Captured exact `9cf442d2` touched production/package files for parent builds,
then restored working bytes in finally. Other reachable production and common
benchmark sources match that parent; the identical new direct-matches benchmark
uses only existing public APIs. Three alternating parent → final common runs:
CSS stylesheet **8.1068 → 8.1935** ms, declarations
**7.7761 → 7.8464** ms, 8,000 mixed rules
**2.2304 → 2.2562** ms. Color parse, RGB
**78.481 → 80.997** ns, keyword **105.441 → 108.369** ns,
OKLCH **84.395 → 83.409** ns. Warm variable alias
**0.0538 → 0.0549** µs, cached 1,000-reference expansion
**0.0686 → 0.0663** µs. Direct matches: ID
**42.595 → 42.229** ns, class
**50.137 → 49.124** ns, descendant
**100.899 → 101.003** ns, has/ancestor
**177.786 → 178.359** ns. Small mixed variations do not
establish uniform improvement or regression guarantees.

Auditable storage control patches, source/binary/check identities and all raw
samples: `measurements/2026-11-05-cascade.json` and
`measurements/2026-11-05-cascade-controls/`. Full per-file outcomes/non-passes:
`measurements/2026-11-05-cascade-coverage.json`. No new SIMD/GPU code; original
native token/math/color machinery is reused. No per-frame parsing or polling
is introduced; future style hosts must retain computed results by generation.

All **106 engine release tests** pass; ten cascade groups pass in default,
full-scan and all-feature builds. Default/all-feature strict release clippy,
package formatting and diff checks pass. Full workspace release build passes
(**2.36 s**, existing GPUI warnings and proc-macro-error2 future-compatibility
notice). No GUI launched, no personal session changed, no visible UI change
to screenshot-journal.

Full browser goal remains active: **0/3,000 website cases, 0/100 sites**, no
usable browser. Required next work includes logical/physical projection and
paired rollback, computed/inherited fonts/colors/used values, UA defaults and
hints, media/supports/import/scope/container contexts, live state/invalidation/
style sharing and actual lifecycle jobs. Profile compound/common subject-key
selectivity beyond this deliberately selective index workload. Full property/
selector/HTML/Web IDL, networking, layout/GPU painting, V8, native shell/Olo
integrations and executable site journeys remain acceptance work.


## 2026-11-05 — logical-property cascade projection

Progress; full browser goal remains active. Added explicit computed-axis
projection over the existing frozen candidate storage. This milestone
implements no browser window, networking, V8, layout or painting. Website
evidence remains **0/3,000 cases, 0/100 sites**.

`Resolver::axes` resolves writing-mode/direction/text-orientation before
paired queries, using variables, defaulting and rollback. Missing required
computed parent axes are an explicit Unsupported result. Computed RTL remains
available to descendants when upright vertical text forces used LTR;
sideways modes keep horizontal typographic conventions. The registry now has
89 longhands/16 shorthands, with all covering 88 resettable longhands.

`Resolver::projected` combines 20 physical/logical pairs (40 longhands) for
preferred/min/max size, margin, padding, inset and overflow. Declaration order,
specificity, importance, origins and layers all compare through Priority.
The result retains the winning declared source for inheritance across changed
parent/child axes. all: inherit prefers physical longhands on ties. Revert
traverses both aliases; invalid computed substitution defaults the winner.
Pending logical shorthands select the declared member and share the existing
expansion/cache, including taint policy and retained limits. Raw declared
queries keep a separate compile-time result type around the same algorithm.
No persistent alias index, whole computed-style map, timer or frame work added.

Consulted CSS Logical Properties 1 §4, CSS Writing Modes 4 §5.1/6.4 and CSS
Overflow 3. A detailed follow-up reading caught that upright has no effect in
sideways typographic modes; corrected before acceptance and tested across all
ten mode/direction combinations. A drafted inheritance test initially stored
literal inherit in a custom property, which correctly inherits that custom
property rather than retaining an ordinary token. Corrected the test to use
an ordinary property's var() fallback; production behavior was already right.

Independent evidence: nine unchanged pinned WPT documents/helpers at
`d7bb5bb848fcd304c10669288a211dbafd106d90`, license and SHA-256 manifest in
`crates/web/tests/logical-fixtures`. The extractor reads upstream literal
mapping/value tables and ports assertion loops, never engine output. It
preserves source-order and selector-specificity checks for all ten combinations,
logical/physical shorthands, deferred/inherited writing mode, variables and
layer rollback. Explicit inert target nodes reproduce helper-created nodes;
each mutation is an independent frozen input. **5,757 native assertions/drivers:
5,726 pass; 31 CSSOM drivers unsupported; no unexpected mismatch/error/panic.**
These are assertion ports, not 5,757 complete browser tests. Evaluator exits 1.
The older cascade corpus remains 128 pass/eight unsupported.

Six focused native groups cover every mapped group, paired identity/order,
inheritance source/all, used vs computed direction, missing-parent context,
pending/cache reuse, invalid substitution, important/origin/layer/rule rollback,
rollback cycles, taint, cancellation and work/depth/memory bounds. Shared native
fixture adapter now computes coherent parent axes and follows inheritance
source. It remains a supplied-context numeric/color adapter, not a style engine.

Release measurements: Intel Core Ultra 7 356H, Linux 7.2.15, CPU0 affinity;
three alternating before/after runs against parent acfd0d8e. The same new raw
query example was compiled against each revision. Extended repeats to 30M
queries and 300 cascade rounds after short-run variation; **both short and long
raw runs are retained**, with binary/source/toolchain identities and script in
`measurements/2026-11-05-logical.json` and `2026-11-05-logical-measure.py`.
These are synthetic worker-library costs, not page/Chrome performance.

Long-run medians (individual process means):

| Operation | Before | After |
| --- | ---: | ---: |
| Raw width | 8.688 ns | 8.170 ns |
| Raw margin-left | 8.256 ns | 8.670 ns |
| Raw color | 7.506 ns | 7.544 ns |
| Raw missing height | 2.728 ns | 2.701 ns |
| 1,000-element indexed match/resolve | 0.2761 ms | 0.2743 ms |
| 2,000-element indexed match/resolve | 0.5627 ms | 0.5632 ms |
| 4,000-element indexed match/resolve | 1.1823 ms | 1.1336 ms |

Raw margin median rose 5.0%; its before range is 8.238–8.734 ns, after
8.166–8.933 ns. Mixed changes and overlapping ranges do not demonstrate a
uniform speedup or settle all performance acceptance. Keep this variation on
the TODO for future coherent style workloads. Raw snapshot payload remains
**416 bytes before/after**, excluding allocator/maps/borrowed typed programs.

New paired-query medians: one physical 9.439 ns, both aliases 15.490 ns,
sideways logical margin 13.668 ns, cached pending shorthand 32.580 ns,
paired rule rollback 30.128 ns. First query latency ranges respectively
0.862–2.808 µs, 0.255–0.438 µs, 0.095–0.200 µs, 3.660–3.827 µs,
0.256–0.589 µs. First axes resolution 0.076–1.011 µs across scenarios.
Axes occupies three bytes. Existing snapshot storage is reused (256 or
416 payload bytes in these examples); there is no separate alias allocation.
No direct before speedup is claimed for the new projection capability.
No SIMD/GPU addition: this is bounded scalar priority work, with existing
SIMD token/math/color facilities unchanged.

All **113 normal release tests** and **112 all-feature tests** pass (the
owned-decoder move-only test is deliberately disabled by its copy control).
Strict default/all-feature release clippy, package formatting, fixture
reproduction/hash checks and diff checks pass. Full workspace release build
passes: cargo 6.54 s, wall 6.64 s including possible build-lock waits; existing
GPUI warnings and proc-macro-error2 future-compatibility notice remain.
No GUI launched, no personal session changed, no visible UI change to journal
with tools/journal.py.

Next: computed/inherited style and UA defaults, logical border/corner groups,
font/color/used-value hosts, conditional stylesheet contexts and actual live
invalidation/style sharing. CSSOM, shaping, bidi, layout, painting, networking,
V8, native shell/agent control/Olo integrations and all website journeys remain
required. Paired defaulting alone supplies none of that evidence.


## 2026-11-05 — frozen computed styles and initial document jobs

Implemented original Rust computed-style processing over the current 89
longhands/16 shorthands (69 canonical physical/unpaired values). Styles share
compiled contextual initial defaults, unchanged fonts/custom dictionaries and
exact equal parent/adjacent override arrays, with sparse storage. They retain no
parent, cascade or source DOM. Numeric partial evaluation freezes subject font,
viewport/container/tree contexts and preserves layout percentages and inner
IEEE behavior. Inheritance copies computed values; unitless line height stays a
multiplier, currentcolor stays subject-relative outside color itself, custom
bindings never rebind against descendant declarations. Actual font selection,
UA stylesheet, presentational hints and author color-scheme remain unimplemented.

Added worker-only bounded initial document jobs: connected light-tree preorder,
real selector/state snapshots, inline parsing, cascade and computed inheritance,
progress/completed prefixes, cancellation, failure poisoning, aggregate retained
payload limits and document/revision/style-epoch guards. Nearest box-parent
context skips display:contents independently of inheritance, carrying axes as
well as display. Coupled computation handles overflow, border device snapping,
absolute/fixed float, root/flex/grid/position blockification and writing-mode
flow-root. This is a full initial pass; no box tree, layout or live invalidation.
No threads, timers, polling or idle work added. Temporary per-element vectors,
nonadjacent/global sharing and incremental work remain profiling/acceptance tasks.

Used the [olo-screens skill](/home/ben/.agents/skills/olo-screens/SKILL.md) on
owned browser-style-check, launching only olo-chrome with a fresh /var/tmp
profile and localhost probes. Chrome 157.0.0.0 supplied 22 selected CSSOM
observations in 15 local cases; native tests match every selected value.
Current Overflow/Backgrounds rules and Chrome corrected outdated draft rules:
clip stays clip beside scrolling; none/hidden zeros used border width, preserving
computed width for inheritance. Match-parent uses computed parent direction,
including upright text; this Chrome exposes the prefixed alias only. Native
also tests the standard alias without claiming reference support. Effective
schemes are supplied by our host, not computed from author color-scheme.
Sources, observations and binary hashes retained in style*-chrome artifacts.
The Agent Screen was stopped, servers stopped and owned Chrome profile removed.
No personal display/session driven. Structured comparisons at scale 1, no
screenshots or visual checks; native border arithmetic also tests 1/1.25/1.5/1.75/2.
No visible product UI change to journal with tools/journal.py.

Nine pinned unchanged WPT resources/helpers plus literal-only extraction produce
316 native assertions and 25 unported browser drivers: **243/341 pass, 98
unsupported, no unexpected mismatch/error/panic**. Original inert HTML/styles
run through the production document job; independent display tuples/scalars
supply references. All 73 native weight/line-height assertions requiring missing
container-type context stay unsupported, even constant examples. Remaining
non-passes include CSSOM/JS/UA font-policy/presentational-hint drivers. This is
selected semantic coverage, not complete upstream browser tests. Evaluator exits
1 while any case is unsupported. Coverage summary and report hash recorded;
fixture regeneration is byte-identical and all manifest hashes verified.
Earlier logical/cascade corpus counts retain their original supplied-context
adapters, not newly claimed document-stage coverage.

Release measurements on Intel Core Ultra 7 356H, Linux 7.2.15, rustc 1.99.1,
CPU0 affinity: three alternating process runs, 5 full build/drop rounds each,
retained three-property lookups, cold calls and maximum 128-node worker steps.
Same-semantics baseline-style-dense recomputes/stores all canonical values and
disables sparse/default/adjacent sharing. It is a counterfactual for this new
stage, not an existing browser baseline. Prebuilt DOM/rules/defaults; supplied
fixed metrics, no actual font selection, layout, UI, network, JS or Chrome timing.
All generated scalar expectations checked outside timings. Raw observations,
source/binary hashes, toolchain, commands and runner retained in
measurements/2026-11-05-style.json and 2026-11-05-style-measure.py.

Medians for 10,000 generated item elements (plus four document elements):

| Scene | Dense build/drop | Sparse build/drop | Dense counted style/tree bytes | Sparse counted style/tree bytes | Dense / sparse process RSS KiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| Bare | 39.419 ms | 13.889 ms | 29,546,496 | 1,775,464 | 61,860 / 7,640 |
| Uniform | 46.838 ms | 19.988 ms | 29,546,584 | 1,775,904 | 61,732 / 8,084 |
| Varying inline widths | 49.621 ms | 26.732 ms | 29,546,584 | 4,735,608 | 62,088 / 14,068 |

Counted payload deduplicates shared allocations and includes Arc controls/tree
capacities, excluding allocator/hash controls and borrowed DOM/rules; RSS is
separate whole-process high water, including fixture/runtime/temporary memory.
Style header is 80 B. Three-property lookup medians dense→sparse are
120.51→9.65 ns, 197.61→16.69 ns and 132.25→19.02 ns. Cold initial build medians
are 35.57→14.09 ms, 51.28→20.56 ms, 45.58→26.65 ms. Defaults compilation ranges
37.89–56.10 µs dense and 39.15–61.99 µs sparse. Largest observed 128-node step
across all runs/sizes is 12.94 ms dense, 6.54 ms sparse (outliers retained).
This is no hard latency guarantee: adversarial matching/individual elements
remain bounded by work quotas and run only on workers.

Common-operation regression probes compare exact pre-logical acfd0d8e, parent
4da00b03 and current sources using identical existing example sources. Three
runs of 30M raw queries and 100 cascade rounds; math/properties/variables/color/
selector probes also retained. Raw margin medians: **8.589 / 8.605 / 8.978 ns**,
ranges 8.583–8.615 / 8.493–8.913 / 8.095–10.013. Previous +5% variation is not
resolved. Parent→current indexed match/resolve medians for 1k/2k/4k elements:
0.2818→0.2875 / 0.5797→0.6284 / 1.1419→1.1658 ms. 2k rises 8.4%, with overlapping
ranges (0.575–0.665 vs 0.547–0.644). Unchanged #target matcher median rises
44.23→53.15 ns (+20.2%), overlapping ranges 39.30–50.06 vs 40.01–59.51.
Other math/property/variable probes are mixed as well. No universal speedup or
settled performance acceptance claimed; preserve these costs on TODO. Raw
cascade snapshot payload remains 416 B. No new SIMD/GPU path for this scalar
stage; existing SIMD scans/color conversion unchanged.

All **134 normal release tests**, **133 all-feature release tests** and all
**19 dense-only style groups** pass. Strict default/all-feature all-target clippy,
package formatting and diff checks pass. Full workspace release build passes:
cargo 5.91 s, wall 6.16 s; existing GPUI warnings/proc-macro-error2 notice remain.

Full browser goal remains active. No browser shell, networking, V8, shaping,
layout/painting, native dark-page adaptation, agent/desktop/password/passkey
integration or website journey is implemented by this milestone. Website
evidence remains **0/3,000 combinations, 0/100 sites**. Next: UA/presentational
styles, actual font selection, missing contexts/properties and live style
invalidation, then layout/rendering and the remaining full-browser acceptance.

## 2026-11-05 — font longhands and coherent face requests

Implemented original font-family/style/width longhands, bringing the registry
to **92 longhands, 91 all-resettable, 16 shorthands, 72 canonical computed
entries**. Font-stretch is a legacy name alias for the same declaration/property,
not a separate cascade candidate. Family lists preserve decoded named/generic
identity, case, fallback ordering, escaped/quoted names, multiword joining and
current script-specific generic() requests. Reserved unquoted names, malformed
lists and font angle/width ranges fail explicitly; lists are bounded by the
existing scanner/work limits, not quad-value scratch.

Compute font-affecting numeric expressions using parent metrics before selecting
the subject font. Inherited family/angle/width values are frozen. Font width
retains percentage type, with keyword mapping and computed clamping, not a layout
basis or geometric stretching. Explicit angle zero becomes normal; bare oblique
retains computed syntax but requests 14 degrees. Host::font borrows the full
FontRequest { size, weight, width, slant, families, axes }. Parent/default full
requests reuse Font Arcs; matching bare/explicit used slopes can share a font
without coalescing computed values. No font files are loaded by this stage.

Family lists use an immutable thin shared handle through triomphe's safe Rust
API (already a workspace dependency), not another CSS/font engine. Named strings
share their own Arcs. The handle is 8 B vs 16 B for the standard-Arc control;
strong count + stored length occupy a two-word allocation header. Font payload
plus Arc controls is **120 B**, vs **128 B** with a fat family handle and **88 B**
before the new request inputs. Style headers stay 80 B; typed Value stays 32 B,
Assignment 40 B and Declaration 64 B. Memory accounting deduplicates families
across defaults, requests and computed arrays, including allocation headers.
No threads, I/O, timers, polling, UI work or new SIMD/GPU path added.

Independent native ports pin nine unchanged WPT font pages and two helpers,
with original paths, BSD notice and all-byte manifest: **93/149 pass, 56
unsupported, no unexpected mismatch/error/panic**. There are 140 literal inputs
plus nine unported browser drivers. Grammar inputs check acceptance only;
ten family computed inputs run the production document job. All 47 style/width
computed assertions requiring missing container-type context remain unsupported,
including constants; style also imports Ahem. No CSSOM serializer/JS, actual
font selection/shaping/layout/rasterization or website evidence. Existing
style 243/341, logical 5,726/5,757, cascade 128/136 and property 821/1,109 counts
remain unchanged; each evaluator exits 1 while non-passes remain.

Used the olo-screens skill on the owned browser-style-check Wayland screen,
with only olo-chrome, a fresh /var/tmp profile and localhost diagnostic page.
Captured 23 local font CSSOM cases at scale 1. All **17 selected supported
observations match** native semantic values; **six reference-feature gaps**
remain separately inventoried. This Chrome 157.0.0.0 supports font-stretch but
ignores font-width, and lacks left/right style and generic(). Its UI-Serif
serialization suggests named-family handling rather than the current-spec
generic. These gaps are not passes. The mixed alias-order row compares only
the final legacy value; its unsupported empty font-width result is retained,
not evidence for Chrome alias identity. Sources, reference binary hash and
observations retained in font-chrome artifacts. Screen stopped, server stopped,
owned Chrome profile removed; no personal session/display driven. Structured
CSSOM comparisons only, no screenshots/visual checks or all-scale rendering
claim. No visible product UI change to journal with tools/journal.py.

Release measurements: Intel Core Ultra 7 356H, Linux 7.2.15, rustc 1.99.1,
CPU0, three alternating process runs vs f1513b47; no concurrent builds/tests or
GUI. Ten build/drop rounds per style process, retained three-property lookups,
first calls and maximum observed 128-node steps. Fixed supplied metrics, no
actual face matching/selection, layout, JS/network/UI or Chrome timing. Generated
scalar/request expectations verified outside timing. Payload includes shared
allocation headers/tree capacities, excluding allocator/hash controls and
borrowed DOM/rules; process RSS is reported separately. Exact raw runs,
source/binary/toolchain identities, isolated controls and runner are in
measurements/2026-11-05-font.json and 2026-11-05-font-measure.py.

10,000 generated item elements plus four document elements, medians:

| Scene | Parent build/drop | New build/drop | Parent / new counted style-tree bytes | Parent / new RSS KiB |
| --- | ---: | ---: | ---: | ---: |
| Bare | 15.184 ms | 15.608 ms | 1,775,464 / 1,775,744 | 7,744 / 7,748 |
| Uniform | 21.787 ms | 22.046 ms | 1,775,904 / 1,776,216 | 8,044 / 8,112 |
| Varying inline widths | 29.992 ms | 29.936 ms | 4,735,608 / 4,735,920 | 13,920 / 13,904 |

Build/drop changes are +2.8%, +1.2%, -0.2%; no universal speedup. Cold build
medians 17.845→15.481 / 21.522→22.848 / 27.953→29.697 ms. Retained three-query
medians 10.136→11.211 / 16.675→17.151 / 20.224→19.200 ns. Largest observed common
style step 11.443 ms parent, 3.265 ms new; outliers retained, no hard latency
or UI-frame guarantee. Jobs run only on workers.

New-feature generated varying font requests: build/drop medians 3.289 ms for
1k and 35.092 ms for 10k. Counted style/tree payload 451,578 / 5,122,458 B;
family allocation payload 106 B shared across the whole job. Fat-handle control
retains 459,586 / 5,202,466 B, a saving of **8,008 / 80,008 B** with the thin
handle; build medians 3.338 / 35.607 ms. Whole font-process RSS median
15,144 vs 15,332 KiB. Largest observed font step 8.708 ms native / 10.651 ms
fat control. New feature has no claimed before speedup. The current 10k case
retains 10,001 Font objects for repeating requests; nonadjacent interning and
language/script/subject matching context remain work, not hidden by the count.
Six new declaration compile/drop medians range 0.055–0.681 µs; raw cold calls
are retained separately. No actual selected-font performance claim.

The first draft's expanded enum caused general typed cloning to move out of
line. Forced whole-clone inlining traded numeric costs against slower colors,
so was not retained; neither a redundant specified oblique numeric wrapper nor
hand-written Clone was retained. Normal code clones known numeric/color inner
types directly at the cascade boundary, with same-semantics general-copy and
fat-family controls. Final raw width/margin/color/missing medians parent→new:
**8.964→11.153 / 8.450→11.168 / 7.922→7.601 / 3.038→3.067 ns**. General-copy
control is 15.347 / 15.152 / 9.025 / 3.032 ns, showing the focused optimization's
benefit within the new stage. Width/margin still regress 24.4%/32.2% vs parent,
with nonoverlapping ranges (8.534–9.019 vs 10.699–11.928; 8.098–9.412 vs
11.031–11.796). Keep profiling on TODO; no settled acceptance. Snapshot payload
remains 416 B. Indexed 1k/2k/4k match/resolve medians 0.2854→0.2972 /
0.5861→0.5980 / 1.1252→1.1994 ms. Other common-operation results are mixed,
including selector timings, and remain in raw evidence. Exploratory unpublished
draft runs/hashes are retained explicitly as non-checkoutable investigation;
final controls are reproducible with documented Cargo features. Hardware
sampling was unavailable for these processes; no system settings changed.

All **140 default release tests and 139 all-feature release tests** pass,
including six new font groups: identity/grammar, parent vs subject contexts,
bounds/alias order/used slope sharing, ownership/quotas/cancel, independent WPT
and selected Chrome observations/gaps. Strict default/all-feature release
all-target clippy, formatting, fixture re-extraction/hash and diff checks pass.
Full workspace release build passes: cargo **36.53 s**, wall **36.77 s**;
existing GPUI warnings/proc-macro-error2 future-compatibility notice remain.

Full browser goal remains active, **0/3,000 website combinations, 0/100 sites**.
Next: actual native font loading/matching and coherent subject/font context,
UA/presentational defaults, remaining properties/conditions and live invalidation,
then shaping/layout/rendering and the rest of the browser. No shell, V8,
networking, all-site dark mode, agent control or Olo/password/passkey integration
is supplied by this prerequisite milestone.

## 2026-11-05 — native font sources, matching and scoped metrics

Added a worker-only shared-source OpenType catalog, native bounded file I/O with
progress/cancel, original CSS width → style → weight matching and immutable
language/range generic policy. ttf-parser 0.25.1 supplies binary table access in
Rust, not CSS face selection. TTC faces share source bytes; selected leases
survive catalog eviction and IDs reject other catalogs. Family aliases use
Unicode 18.0.0 full C/F folding without normalization/tailoring. The original
input/license, generated facts and hashes are pinned. No system discovery,
process, watcher, web-font decoder, shaper or rasterizer is introduced.

First-available metrics are independent of cmap fallback for zero/water.
Different units-per-em, vertical orientations, missing metrics and synthesized
vertical advances have explicit provenance. Fractional HVAR/VVAR/MVAR deltas
survive scaling; bounded gvar phantom points cover absent advance tables.
Missing-height inference only enters simple TrueType outlines with ≤4,096
points and bounded glyph/gvar bytes; composite/CFF inference uses CSS estimates.
Directory/name/axis/payload/work checks are not a whole-font sanitizer or
complete adversarial CPU/cancellation boundary. Stronger isolation remains
required before web fonts. Native font-style left/right remains Unsupported.

FontSet/Scope identities prevent cross-language/subject metric reuse. Host's
new required font_context distinguishes identical requests under different
policies/languages; parent/default reuse and final publication verify it, and
root Defaults must use the root's context. Bounded worker-local FIFO caches
retain family identity to prevent pointer reuse, with numeric/slant/all-axis
keys. Their budget counts conservative entry charges, with additional actual
container capacity reported; it is not a hard total-RSS budget. A cloned Scope
keeps context and starts with an empty cache.

20 original small fonts and independent numeric references are generated with
fontTools 4.60.1 in an isolated /var/tmp venv; fixed timestamps make regeneration
byte-identical. Includes static matching candidates, localized/Unicode names,
TTC sharing, missing/composite heights, optical HVAR/MVAR and no-HVAR gvar,
and a metadata-only registered-axis font. Real Noto data is SHA-checked. Tests
caught the table reader's is_italic helper classifying a nonzero-angle oblique
face as italic; OS/2 oblique is now kept distinct. They also disproved an assumed
Noto 100–900 range: this repository file reports 400–700, tested with a clamped
900 request. All 1,606 normative full-fold records are checked independently.

An owned Olo Agent Screen loaded the generated optical font and actual Noto
file in Chrome 157. 32 structured CSSOM cases record auto/none/explicit 12/16
opsz at four sizes. Auto uses CSS pixels here, correcting an initial px-to-point
assumption. Native checks compare 24 auto ch/ex/cap observations with explicit
one-font-unit + 1/64 CSS-pixel rounding tolerance. Chrome rounds some font-unit
metrics; native raw deltas are more precise, not exact Chrome parity. No visual
or scale/theme check is claimed; the owned screen and HTTP server are stopped.

Release evidence is in measurements/2026-11-05-native-font{.json,-measure.py},
with binary/source hashes, means, cold calls, worker slices and separate RSS.
Six pre-read shared Noto sources contain **3,923,244 B**; frozen catalog payload
is **3,925,359 B** (2,115 B additional, excluding allocator controls/RSS).
Three-process medians: catalog build/drop **5.679 µs**, first build **41.156 µs**;
family match **44.18 ns**; first native metrics **7.24 µs**. Same-binary metrics
cache-off → on **1,034.77 → 25.35 ns**. Real-metric CSS job build/drop 1k
**3.635 → 2.366 ms**, 10k **37.378 → 24.929 ms**; 10k maximum 128-node slice
medians **1,035 → 747 µs**. Job style payload is 4,241,169 B, excluding borrowed
catalog/cache; cache-on ends with 605 entries / 698,771 B conservative payload.
Whole native benchmark RSS is 19,224–19,324 KiB. No disk, shaping, layout, UI,
network, V8 or browser performance is measured.

Compared unchanged supplied-metric examples with parent 239cdb4b, alternating
release processes on CPU0. 10k bare/uniform/varying build medians **14.258 →
14.419 / 20.843 → 21.598 / 28.025 → 30.251 ms**. Supplied-font 10k **31.104 →
32.533 ms**, accounted Font allocation payload **120 → 128 B** from the new
context (8 B each; shared default/header behavior unchanged). Raw width/margin/
color/missing **10.706 → 10.657 / 10.587 → 10.678 / 7.161 → 7.144 / 2.748 →
2.690 ns**. Indexed 1k/2k/4k cascade **0.2671 → 0.2743 / 0.5407 → 0.5650 /
1.0770 → 1.2224 ms**. Keep earlier width/margin regression and Font interning
on TODO; no general speedup claim.

Unexpected current-build 8–11 ms wall-clock worker slices triggered five extra
alternating frozen-binary runs, preserved in native-font-latency.{json,py}.
Follow-up 10k uniform build medians **20.760 → 21.533 ms**, varying **28.774 →
28.460 ms**. Both binaries show large wall-clock outliers (parent up to 10.586 ms,
current up to 12.625 ms); process CPU time is similar (uniform mostly .76 s,
varying .93–.95 s), with many involuntary context switches. These whole-process
counters do not establish the cause of individual slices. Preserve primary
results and variability; there is no settled UI-latency or browser-performance
acceptance here. This worker stage has no UI thread or polling/idle loop.

All **157 default / 156 all-feature release tests** pass, including 16 new
integration groups and the normative folding unit test. Strict release
all-target default/all-feature clippy, formatting, diff/hash checks and fixture
regeneration pass. Full workspace release build: cargo **2.97 s**, wall
**3.07 s**; existing GPUI warnings/proc-macro-error2 notice remain. Detailed
commands/counts are in native-font-checks.json. No personal session change.

Full goal stays active: **0/3,000 website combinations, 0/100 sites**. Next:
context-scoped selected-request interning, shaping/bidi and original layout,
while tracking remaining font APIs/conditions/UA defaults/live invalidation.
Browser shell, networking, V8, all-site dark mode, structured browser actions,
passwords/passkeys and end-to-end Olo integrations remain unimplemented.

## 2026-11-05 — bounded selected Font request sharing

Previous native-font milestone facd8cd3 is pushed. Style jobs now pool selected
Font objects by complete numeric/slant/axis request, family contents and immutable
font context. Equal separately parsed family lists share; different contexts do
not. Parent/default fast paths run first. Defaults allow 4,096 entries; zero
disables the pool, saturation stops new insertion and reservation failure keeps
normal uncached selection. Hashing/traversal skips very large family payloads
(64 KiB cap) rather than spending unbounded time on optional memoization.
Exhaustive FontRequest/Axes patterns require updating keys when inputs grow.

Pool slots count toward the job's retained payload budget and are reported by
font_cache_stats. Fonts/families are already counted once by style Memory.
The pool drops at finish; completed styles own their shared Fonts. No global
cache, thread, timer, UI or new browser rendering capability is introduced.
Revision/context/cancel guards run on hits; failure poisons publication.

Nine integration groups cover nonadjacent/equal-source sharing, numeric/style/
axis/family/context separation, zero/full pools, failed selection, cancellation,
revision and context change during a hit, release lifetime, exact budget charge
for observed scratch and huge valid family lists. All **166 default / 165
all-feature release tests** pass; strict release all-target default/all-feature
clippy, formatting and diff checks pass. Full workspace build: cargo **2.89 s**,
wall **2.99 s**, existing GPUI warnings/proc-macro-error2 notice remain.

Final frozen CPU0 evidence: measurements/2026-11-05-font-pool{.json,-measure.py}.
Three alternating parent/current processes and same-binary reversed pool
controls include means, cold calls, max slices, source/binary hashes and
process CPU/scheduler/RSS counters. An earlier unpublished pre-hash-cap draft
is retained separately as exploratory raw evidence, not an exact checkoutable
baseline. Final source hashes are checked after measurement.

The existing supplied-font 10k case retains **301 Fonts / 38,528 B** instead of
**10,001 / 1,280,128 B**, a **97.0% Font payload reduction**. Whole-process RSS
medians **15,160 → 12,616 KiB**. Real Noto job Font payload **1,275,904 → 38,656 B**
(302 Fonts); process RSS **19,072 → 17,160 KiB**. Real-metric job 10k build/drop,
metric cache off **41.892 → 25.383 ms**, on **27.773 → 26.378 ms**. Process user
CPU across that full native benchmark goes .79/.79/.80 → .63/.62/.61 s. Those
are library workloads with pre-read font bytes, not navigation/UI measurements.

Same-binary 10k repeated-size control, pool off → on: **10,000 → 100 subject
Fonts**, total Font payload **1,280,128 → 12,928 B**, retained style-tree payload
**4,415,820 → 3,148,620 B**, and **7,168 B** pool slot scratch for 100 entries.
Separate equal inline-family lists also reduce Font objects to 100; independent
computed family values still retain **680,032 B**. Their style-tree payload
**5,095,752 → 3,828,552 B**. No Font reduction for all-unique requests; at 10k the
pool saturates at 4,096 entries / **458,752 B** slots and then stops insertion.
This unique-request overhead remains a tuning/profile TODO before real pages.

Timing tradeoffs stay visible. Same-binary pool off/on build medians: 1k repeated
**2.603 / 2.814 ms**, inline-family **3.971 / 3.193 ms**, unique **2.551 / 2.782 ms**;
10k repeated **27.106 / 25.711 ms**, inline-family **34.077 / 33.019 ms**, unique
**28.086 / 29.993 ms**. Unique overhead is ~9.1%/6.8%, with no memory saving.
Supplied-font parent/current 10k **34.719 → 36.111 ms**, while process CPU is
similar (.41/.41/.41 → .41/.42/.40 s) and wall-clock slice maxima vary widely.
Do not claim a universal speedup or settled latency acceptance.

Common parent/current 10k bare/uniform/varying style job build medians
**15.163 → 17.454 / 23.261 → 22.200 / 29.946 → 31.987 ms**. Median max slices
**389 → 3,515 / 3,120 → 2,181 / 815 → 5,119 µs**. These inherited-font scenes
allocate no pool slots. Preserve variation/outliers and process counters; they
do not establish a cause for individual wall-clock stalls. Raw width/margin/
color/missing medians **12.293 → 11.947 / 11.190 → 11.113 / 8.038 → 7.527 /
3.054 → 3.837 ns**; indexed cascade 1k/2k/4k **0.3540 → 0.3285 / 0.6503 →
0.5924 / 1.2005 → 1.3698 ms**. Earlier raw-query regression and mixed-load
worker latency remain open. No UI or owned GUI was started; no personal
session reload/install. Scratch is removed after the pushed checkpoint.

Full browser goal remains active: **0/3,000 website combinations, 0/100 sites**.
Next: shaping/bidi and original inline/block layout, including real text/font
selection and independently checked output. Font sanitization/web fonts,
automatic generic policy, complete CSS/live invalidation, browser shell,
networking, V8, universal dark mode and Olo/agent/password/passkey integration
remain substantial open work.

## 2026-11-05 — native glyph runs and Unicode paragraph preparation

Previous request-sharing milestone e89c5dea is pushed. Added original source/
font/cache orchestration around HarfRust 0.15.0, using its maintained pure-Rust
OpenType implementation (read-fonts 0.46.0). Primary sources showed RustyBuzz
archived and recommending HarfRust. No browser engine/system font process is
embedded. The engine still forbids unsafe code. Native sources are leased into
read-fonts through Arc/AsRef without a second byte allocation; parsed bases
share across variation instances and retained glyph runs survive cache/catalog
release. Defaults bound cached base faces/instances/plans at 64/256/256 and
conservative source bytes at 32 MiB; opaque model caches/RSS are separate.

Run primitives preserve glyph IDs, font-unit positions, unsafe-break flags and
absolute UTF-8 clusters, with explicit script/direction/language/features/context.
Original uniform-request paragraph jobs add bidi/grapheme/script itemization,
whole-grapheme cmap fallback, UVS retry/provenance and progress/cancellation.
Primary coverage uses a retained parsed charmap; bounded fallback memoization
avoids repeated parsing. Font-key equality, not only Arc identity, joins runs
when caches are disabled/full. Tests caught that integration error before
publication. Failure poisons completed-run access/finish; finished data are
Send+Sync for the future worker→UI path. Empty paragraphs allocate no shaper cache.

Pinned Unicode 18 class/bracket/ignorable/grapheme/Indic/emoji facts replace the
bidi dependency's older bundled data. Original constant-state forward grapheme
segmentation passes all **853 official cases**. The prior Unicode 17 library
missed ten cases: Unicode 18 changed GB9c to Linker Extend* × Consonant.
Bidi properties include unassigned @missing defaults and canonical bracket
normalization. All **91,707 BidiCharacterTest cases** pass through L2. One
initial adapter discrepancy was comparing paragraph levels before L1 with
reference line levels after L1; comparison now uses the correct line phase.
The corpus's own header excludes P1/L3/L4; no wider conformance follows.

Four unmodified OFL Noto script fonts are pinned at google/fonts commit
24ddc1f324cac5d9f5aedca126d298f56e3f1640, plus existing Latin/optical/TTC data.
The independent generator never imports/runs Olo. Native HarfBuzz **15.0.0**
C API matches all **24 authored glyph vectors and 15 longer packed hashes**,
including positions/flags/variations/features/UTF-8 subranges/TTC indices.
Uharfbuzz 0.55.0 bundles older **14.2.1**; those vectors remain separate. A
one-unit Hebrew anchor difference disappeared with the matching native 15
reference. The Hebrew file defaults to weight 100 and a legacy “Thin” name;
explicit CSS-normal references use 400, and alias checks use all family names.
These were reference-input/adapter corrections, not copied engine expectations.
Font bytes and all references/properties regenerate/hash exactly.

Defaults cap runs at 16 KiB input/32,768 glyphs, paragraph text at 64 KiB and
fallback memo at 1,024 entries/1 MiB charges. Unprocessed whitespace/paragraph
controls, vertical paragraph policy and oversized uniform spans are explicit
non-passes. Never cut blindly through a ligature at a size cap. CSS whitespace,
styled inline spans, script-extension/paired-punctuation itemization, full
script/GSUB eligibility, multi-font marks, spacing, line wrapping/reshaping and
rasterization remain. Source/payload bounds are not hard total-memory limits;
opaque font/shaper preprocessing/allocation/cancellation need isolation and
sanitization before web fonts. No UI, polling, network or V8 is added.

Release evidence: measurements/2026-11-05-shaping{.json,-measure.py}. Three
alternating CPU0 processes preserve all means, cold calls, max constructors/
128-grapheme slices, source/binary hashes and CPU/scheduler/RSS counters.
Same-binary source/font/plan cache-off → on for ~4 KiB:
Latin raw shape/drop **133.020 → 114.752 µs**, paragraph **260.677 → 243.952 µs**;
Arabic **200.920 → 187.990 / 328.401 → 310.048 µs**;
Hebrew **175.406 → 166.902 / 325.855 → 314.037 µs**;
Devanagari **673.726 → 190.985 / 741.107 → 258.125 µs**;
Thai **71.021 → 62.492 / 155.291 → 144.657 µs**.
Cached first paragraph medians **321/349/341/753/160 µs**, max constructor
medians **54/98/106/50/56 µs**, max worker slice medians **163/202/185/207/67 µs**
in that script order. Those are normal trusted-font library workloads, not
adversarial latency or browser/UI frame acceptance.

Same-facts full-Unicode control (baseline-text-ascii) → focused ASCII fast
paths: 1,800-byte ASCII analysis **5.555 → 1.667 µs**, grapheme scan **27.899 →
7.479 µs**. Cached 4 KiB Latin paragraph **308.830 → 243.952 µs**. Mixed scan
**47.558 → 37.287 µs**, emoji scan **24.610 → 16.996 µs**; other text operations
are mixed and remain raw. Retained dense glyph records are **28 B**: 4 KiB
Latin/Arabic/Hebrew/Devanagari/Thai payload **92,820 / 62,468 / 62,496 /
30,464 / 41,580 B**. Whole shaping benchmark RSS **8,772–9,008 KiB** with .43 s
process user CPU per run; control **8,648–8,744 KiB**, .44/.44/.45 s. No memory
or speed claim about a full page, rasterizer, GPU or Chrome.

Parent common-workload 10k bare/uniform/varying style build medians **14.044 →
14.331 / 20.263 → 22.782 / 27.446 → 29.381 ms**; max slice medians **366 →
411 / 605 → 7,555 / 732 → 10,229 µs**. Supplied-font 10k **31.200 → 31.609 ms**.
Native Noto job 10k metric cache off **23.356 → 23.287 ms**, on **22.799 →
25.925 ms**; full process CPU remains close (.58/.58/.56 → .57/.57/.58 s),
with different involuntary-switch counts. These counters do not establish the
cause of individual wall-clock outliers. Preserve variability and profiling
TODOs, not a general latency/speedup claim. Raw width/margin/color/missing
**10.658 → 10.244 / 11.121 → 10.164 / 7.188 → 7.506 / 2.791 → 2.714 ns**;
indexed 1k/2k/4k cascade **.2725 → .2778 / .5420 → .5542 / 1.0906 → 1.0983 ms**.

The native-font benchmark file grows **11,640,784 → 22,492,920 B** with debug
information. ELF evidence in shaping-elf.txt separates this: .text **1,123,296 →
1,118,960 B**, .rodata **260,376 → 260,248 B**, .debug_info **3,355,568 →
7,345,261 B**. Whole-process native-font RSS medians **17,236 → 17,272 KiB**.
The actual shaping example .text/.rodata are **1,681,232 / 513,148 B**. Debug
file size is not mapped runtime payload; no stripped-browser size claim.

All **179 default / 178 all-feature release tests** pass, with 13 new shaping
regression groups, plus strict default/all-feature all-target release clippy,
formatting, source/reference hash/regeneration and diff checks. Workspace build:
cargo **10.96 s**, wall **11.06 s**; existing GPUI warnings/proc-macro-error2
notice remain. No owned GUI or personal session reload/install. Scratch is
removed after pushing this checkpoint.

Full goal stays active, **0/3,000 website combinations, 0/100 sites**. Next:
CSS whitespace, styled inline source mapping/line breaking and original layout,
then native drawing. Networking/V8, complete CSS/live invalidation, native shell,
universal dark mode, structured browser control and Olo/password/passkey
integration remain substantial open work.

## 2026-11-05 — CSS whitespace preparation and compact source maps

Read HACKING.md and the Be Book's BTextView/CanEndLine contract: retain explicit
text offsets and separate wrapping decisions from text storage; its ASCII-era
break rules are not a web line breaker. Consulted current primary CSS Text 3/4
whitespace/DOM-CR/bidi-control/shorthand rules. Implemented original Rust
`text::prepare`, with inherited collapse/wrap longhands and the interoperable
white-space shorthand (legacy aliases, unordered combinations, defaults,
CSS-wide resets, inheritance and pending substitution). Registry: **94
longhands / 17 shorthands**, 93 reset by all. Discard/white-space-trim remain
unsupported and rejected; no silent partial reset.

The worker takes one shared source and an exact UTF-8 span partition. It keeps
original source and span identity even for a wholly removed text node, collapses
across inline span boundaries, treats DOM CR as a space, preserves non-CSS
spaces/controls and makes bidi controls transparent to phase-I adjacency.
Preserve modes retain tabs/breaks for downstream layout. Two resumable linear
scans avoid unbounded lookahead/per-character arrays; quotas, cancellation,
progress, fail-poisoned publication and worker-deliverable immutable output
are explicit. Constructor validation, first-edit prefix copying and final Arc
allocation are bounded by total limits but not subdivided into byte-budget
steps; they also belong on the worker. Default caps: 8 MiB input, 4,096 spans,
262,144 maps, 131,072 backward trim ranges. These are not total RSS/isolation
or fallible-allocation guarantees.

Unchanged output shares the original Arc. A stored map record is **12 B**:
input/output starts plus packed span/kind; ends derive from the next record or
string lengths. Queries binary-search byte coordinates and decode public values
without allocation. Removed intervals stay inspectable for future selection,
agent DOM mapping and wrapping policy; this is not caret hit testing. Portable
wide printable-ASCII scans/copies and memchr reverse scans bypass scalar work
when safe; scalar/24-byte map controls preserve the same facts. Prepared normal
text now feeds the native shaper in an integration test, with glyph clusters
mapped back to source spans. Preserved breaks/tabs still require layout.

Seven new preparation groups cover modes, DOM CR, all Bidi_Control characters,
non-CSS separators/control retention, mixed spans, empty/invalid partitions,
quotas, cancellation, poisoned/incomplete publication, all map lookups and SIMD
boundaries. **3,925 independent regex-phase references** run under two budgets
and per-scalar text-node partitions. The generator never invokes Olo; expected
strings come from separate normative-phase rewrites. **52 literal pinned WPT
acceptance ports** pass, with unchanged HTML/helper/license and every local
fixture/generator SHA checked. They are native declaration acceptance ports,
not JS/CSSOM/layout/raster WPT results. Existing style tests also verify
inheritance/reset/substitution and binding WhiteSpace to computed styles.

Applied the olo-screens skill for an owned browser-style-check headless screen.
Olo Chrome **157.0.0.0** recorded **65 local observations**: 57 supported and
eight preserve-spaces cases unsupported by that build. Diagnostic comparison
of native phase I with Chrome innerText gives **45 identical / 12 different /
8 reference-unsupported**. These are different pipeline stages, not a website
compatibility percentage. Three differences preserve CR in Chrome plaintext;
four concern its ZWSP segment-break removal versus our explicit always-space
UA policy; five concern transparent bidi controls. Kept raw innerText, computed
settings and browser widths. No native width/raster comparison exists yet.
`whitespace_probe` deliberately exits **1**, leaving differences/unsupported
visible. The owned screen and loopback probe server are stopped; no personal
session/browser/install/reload changed. No product UI drawing changed, so no
native visual/reload acceptance is inferred.

Release evidence: **measurements/2026-11-05-whitespace{.json,-measure.py}**,
with checks/ELF evidence and separate chrome/projections reports. **57 frozen,
CPU0, alternating processes**, three samples, no concurrent builds/tests/owned
GUI or source mutations; all 78 source hashes match after measurement. Parent
f111348a common examples were frozen before source edits. New same-semantics
controls disable bulk processing or double encoded-map storage width.

For ~36–58 KiB text, scalar → default prepare/drop medians (µs):
unchanged **268.763 → 54.484**; collapsed **336.399 → 114.165**;
multilingual **279.715 → 232.269**; pre **207.064 → 54.080**;
1,000 spans **391.677 → 142.500**; long ASCII word **188.660 → 1.798**.
Default cold calls in that order: **53.338 / 141.083 / 262.922 / 55.847 /
205.545 / 2.286 µs**. Medians of process max 4,096-byte steps: **13.201 /
105.681 / 79.325 / 16.579 / 69.905 / 3.465 µs**. Maxima include scheduler and
allocation variation; they are not adversarial/UI frame acceptance.

24-byte → 12-byte map control halves retained payload exactly: collapsed
**192,024 → 96,012 B**, multilingual **168,024 → 84,012 B**, 1,000 spans
**216,000 → 108,000 B**, unchanged single spans **24 → 12 B**, with unchanged
source/text Arc sharing. Wide → compact prepare/drop for the three edited
scenes: **164.029 → 114.165 / 275.463 → 232.269 / 250.655 → 142.500 µs**.
Whole preparation process RSS: default **2,676–2,880 KiB**, scalar
**2,560–2,812 KiB**, wide **3,060–3,096 KiB**; user CPU default **.31/.31/.30 s**,
scalar **.87/.87/.87**, wide **.34/.31/.33**. Allocator/RSS variation does not
replace the exact payload measurements or establish a full-browser limit.

Parent/current common 10k style medians (ms): bare **14.776 → 14.891**, uniform
**21.243 → 22.701**, varying **29.160 → 31.290**. Max worker slice medians (µs):
**399 → 388 / 612 → 7,671 / 774 → 10,589**. Shared defaults **6,008 → 6,152 B**,
with no per-element payload increase in these cases (tree totals +144 B).
Raw width/margin-left/color/missing-height (ns): **10.344 → 11.127 / 10.608 →
11.288 / 7.836 → 7.901 / 2.745 → 2.799**. These regressions/outliers remain
profiling TODOs; no general speedup/performance acceptance claim.

Cached ~4 KiB shaping paragraph parent/current medians (µs): Latin **250.840 →
253.688**, Arabic **320.934 → 350.931**, Hebrew **323.655 → 320.192**, Devanagari
**261.822 → 267.018**, Thai **148.949 → 148.796**. Whole shaping process user
CPU is identical in paired runs **.44/.46/.44 s**, with involuntary switches
**15/22/27 → 98/122/104**; this does not identify the cause of wall-clock
outliers (Arabic max-slice median **216 → 1,224 µs**). Preserve raw evidence.
ELF .text/.rodata for common shaping **1,681,232/513,148 → 1,681,520/513,276 B**;
new prep **290,723/42,732 B**. Debug/file bytes remain separate from runtime
payload. No network, JS, GPU, UI, website or Chrome speed claim.

**188 default / 187 all-feature release tests pass**. Strict default/all-feature
all-target release clippy, formatting, byte-identical fixture regeneration,
source/reference hashes and diff checks pass. Full workspace release build:
cargo **3.42 s**, wall **3.52 s**; existing GPUI warnings/proc-macro-error2
future-compatibility notice remain. Owned scratch is removed after pushing.

Goal remains active: **0/3,000 website combinations, 0/100 sites**. Next:
phase-II whitespace/tab/wrap behavior, styled shaping/safe line breaking and
original inline/block layout, then native painting. Networking/V8, full CSS,
isolation, native shell, universal dark/night adaptation and structured
browser/Olo/password/passkey integrations remain substantial work.

## 2026-11-05 — original Unicode 18 line-break opportunities

Previous goal turn was progress: 858b2818 is pushed, and the worktree started
clean. Read HACKING.md and BTextView/CanEndLine before changing code. Retain the
Be Book's explicit offsets/context hooks; use current Unicode rules rather
than its ASCII-era break choices. Consulted primary UAX14 revision 57 and
Unicode 18 data. The workspace unicode-linebreak 0.1.5 uses Unicode 15; it
cannot establish current behavior. ICU4X 2.4 offers current data/dictionaries,
but the initial default boundary engine here is original safe Rust, with no
new runtime dependency or borrowed engine.

`text::linebreak` applies all default UAX14 rules over generated factual raw
Line_Break/General_Category/EastAsianWidth/Extended_Pictographic data. Keep raw
classes for future CSS tailoring. Runtime LB1 resolves AI/SG/XX→AL, CJ→NS and
SA marks→CM/other SA→AL, preserving SA provenance. Ordered quote, numeric,
space, Hebrew-hyphen, Korean/Brahmic, RI and emoji context rules retain fixed
backward state and bounded right lookahead. Combining continuations are folded
incrementally, without a character array or unbounded scan/backtracking.

The worker is **200 B** on measured x86_64. It holds up to three complete
lookahead units plus one pending unit and four fixed backward unit values.
`step(byte_budget)` reports progress/completed frontiers and checks cancellation;
failure poisons publication. Defaults cap source at 8 MiB and logical
opportunities at 262,144, including EOT. A scalar may exceed the remaining
budget by up to three UTF-8 bytes. All construction/steps/finish belong on the
worker. These are bounded-work/storage policies, not process isolation,
fallible allocation or total RSS limits.

Each internal opportunity stores a packed **four-byte offset/kind**. EOT is an
implicit required marker derived from shared source length; a word without
internal breaks allocates no offset array. Exact-size, double-ended/fused
iteration includes EOT (zero for empty input), without itself creating a blank
line. One packed bit limits source to <2 GiB. Source Arc sharing with CSS
preparation and mappings back to original source spans are verified. Actual
widths, caret hit testing and line geometry are not implemented by this module.

**All 19,346 original Unicode 18 LineBreakTest strings pass**, comparing every
boundary and allowed/mandatory kind at one-byte and 4 KiB budgets. Five groups
also test progress frontiers, quotas/cancellation/poisoning, UTF-8/empty/hard
breaks, mixed iterator traversal, 100,000 combining marks, source sharing and
bulk-run context against scalar steps. Raw resources/license reuse, generator,
generated facts and fixture docs are SHA pinned. The generator does not use
Olo or test expectations to decide boundaries; regeneration is byte-identical.
Every isolated measurement control also passes all five groups on final code.

Raw SA counts remain explicit: default AL treatment is permitted by UAX14 but
is not Thai/Lao/Khmer/Myanmar browser segmentation. CSS strict/loose, word-break,
overflow-wrap, nowrap/preserve-space policy, emergency grapheme boundaries,
phase-II whitespace, width fitting, safe boundary reshaping, actual inline/
block layout and painting remain. No CSS WPT, native pixels or website pass
is inferred from Unicode corpus conformance.

Measured before optimizing and retained the negative evidence. Initial bulk
checks penalized multilingual scalar processing; an ASCII eligibility guard
reduced work but left overhead. Final code selects bulk/no-bulk specialization
once per worker slice, with cancellation checked before eligibility scanning.
Portable SIMD letter/digit runs preserve the exact scalar lookahead/context
frontier. Focused property/pair paths and direct packed class IDs avoid
unnecessary full-rule/table work. Full-rule, no-bulk and checked-class-table
controls preserve identical facts and output. The direct ID invariant follows
from private generated IDs (<49), LB1 resolution and two fixed bulk classes.

Evidence/replay: **measurements/2026-11-05-linebreak{.json,-measure.py}**, plus
checks/ELF evidence. Initial and guard JSON files retain **132 earlier frozen
processes**; replay patches against final runtime source reproduce their source
snapshots/hashes and pass git apply --check. Final **66 CPU0 processes**, three
alternating samples, run without concurrent builds/tests/owned GUI/source
mutations. All 79 measured source hashes match afterward. Parent 858b2818's
four before-example hashes exactly match the preceding checkpoint's after
hashes, independently confirming the parent binaries.

Final large-case medians (µs; build/drop includes worker orchestration):

| Scene / source bytes | Full-rule control | Default | No-bulk control | Class-table control | Stored offsets B |
| --- | ---: | ---: | ---: | ---: | ---: |
| Latin / 36,864 | 642.834 | 315.953 | 305.090 | 358.239 | 24,572 |
| CJK / 45,056 | 307.133 | 309.100 | 303.071 | 332.041 | 49,148 |
| Arabic / 53,248 | 506.545 | 404.457 | 392.465 | 438.873 | 16,380 |
| Devanagari / 77,824 | 469.576 | 374.115 | 373.984 | 409.566 | 16,380 |
| Thai default / 77,824 | 550.871 | 423.905 | 415.944 | 462.083 | 16,380 |
| Emoji / 61,440 | 330.033 | 310.842 | 306.059 | 334.663 | 24,572 |
| Joiners / 43,008 | 589.334 | 323.829 | 315.773 | 342.425 | 16,380 |
| Hard breaks / 18,432 | 182.287 | 131.821 | 125.316 | 137.029 | 24,572 |
| Alphanumeric word / 73,728 | 1,282.248 | 197.457 | 565.826 | 194.066 | 0 |
| Combining / 530,432 | 2,185.149 | 2,404.206 | 2,211.133 | 2,472.438 | 8,188 |

Default cold calls in that order: **307.090 / 320.441 / 404.106 / 373.346 /
436.966 / 337.312 / 317.691 / 124.221 / 169.679 / 2,259.389 µs**. Medians of
process max 4 KiB steps: **51.934 / 39.234 / 45.170 / 24.412 / 28.218 / 31.904 /
37.949 / 39.551 / 16.654 / 660.542 µs**. Constructor and every raw/max observation
remain in JSON. These are trusted synthetic library cases, not adversarial or
UI frame acceptance. Residual no-bulk costs and combining outliers remain
profiling work; no universal SIMD/latency speedup claim.

Whole line-break process RSS: default **3,420–3,540 KiB**, full-rule
**3,344–3,632**, no-bulk **3,528–3,568**, class-table **3,468–3,604**. User CPU:
default **.58/.53/.53 s**, full-rule **.77/.74/.74**, no-bulk **.59/.56/.56**,
class-table **.59/.57/.57**. Initial and guarded default CPU were
**.58/.58/.58** and **.55/.55/.55**; wall maxima/context switches vary. No
individual timing difference's cause is established solely by these counters.

Common parent/current 10k style build medians (ms): bare **15.064→14.566**,
uniform **21.860→21.360**, varying **29.673→29.048**; max slice medians (µs)
**392→383 / 639→601 / 8,047→768**. Style-tree payload is unchanged. Raw width/
margin-left/color/missing-height (ns): **11.631→10.672 / 11.271→10.580 /
7.689→7.740 / 2.868→2.898**. Previously observed style regressions/outliers
remain open; unrelated module/link changes and sampling are not a causal fix.
Cached ~4 KiB shaping paragraphs (µs), Latin/Arabic/Hebrew/Devanagari/Thai:
**250.377→250.946 / 328.723→324.289 / 329.903→322.481 / 267.351→265.332 /
148.110→147.301**. Parent/current shaping process CPU **.46/.45/.44→.46/.45/.43 s**.
Preparation unchanged/collapsed/multilingual/pre/1,000 spans/long-word (µs):
**56.602→56.251 / 114.749→118.897 / 233.843→233.753 / 54.632→57.816 /
140.908→145.078 / 1.729→1.781**. Keep mixed results and raw scheduler counters.

New line-break example ELF .text/.rodata **293,427 / 75,012 B**; parent/current
common shaping **1,681,520/513,276→1,672,416/512,636 B**. Debug/file bytes are
separate; these include benchmark scaffolding and are not a stripped browser
size or full-engine RSS claim. No network, JS, GPU, page or Chrome speed claim.

**193 default / 192 all-feature release tests pass** after the class
representation change. All five line-break groups are rerun on final dispatch/
cancellation code in default, all-feature and each isolated control config.
Strict default/all-feature all-target release clippy, formatting, source/hash/
regeneration/replay-patch/diff checks pass. Workspace release build: cargo
**7.97 s**, wall **8.17 s**, with existing GPUI/proc-macro-error2 notices. No GUI
was started, so no native visual/reload acceptance is implied; no personal
session install/upgrade/reload. Owned scratch is removed after pushing.

Full goal remains active: **0/3,000 website combinations, 0/100 websites**.
Next: CSS/phase-II whitespace policy and actual line fitting/styled shaping,
then original inline/block layout and native drawing. Networking/V8, full CSS,
isolation, native shell, universal dark/night behavior and structured browser/
Olo/password/passkey integrations remain substantial open work.

## 2026-11-06 — retained horizontal text geometry

GitHub access is restored: origin/browser was verified at a88b8775 before this
milestone. Implemented original `text::layout`, joining prepared source,
Unicode opportunities and retained native glyph runs. Width-independent
integer cluster/edge indices and safe glyph slices avoid copying or reshaping
whole lines on resize. Unsafe edge windows reshape with font/script/language
and paragraph context. Nonmonotonic ligature advances cannot hide a later
fitting candidate. LF has bidi/source identity without a visible glyph; blank
lines and LF-only paragraphs work. Initial SP trimming/hanging, break-spaces,
no-wrap, visual bidi fragments, alignment/last alignment, CSS line height and
font-scope/direction/whitespace binding are covered. Worker progress,
cancellation, quotas and poisoned publication remain explicit. Job sizes on
this x86_64 build: TextJob **400 B**, layout Job **432 B**.

Three **original** tiny fonts reproduce cross-space ligatures, a narrowing
longer candidate and contextual prefix deletion. FontTools 4.60.1 lives only
in an owned /var/tmp venv; font timestamps are fixed. Font and oracle
regeneration is byte exact. Independent native HarfBuzz **15.0.0** brute-force
fitting supplies **216 layouts**, checked at one/128-candidate budgets for
line ranges, advances (1e-9 px arithmetic tolerance) and exact glyph IDs,
clusters and positions. Nine regression groups additionally cover SP hanging,
LF/empty/no-wrap, RTL/alignment, computed styles, limits, source provenance,
cancellation and error poisoning. This is not a Chrome/Skia/pixel reference.

Final release measurements: **36 alternating CPU0 processes**, 100 inline
rounds each, no concurrent builds/tests/owned GUI/source edits; all nine
control geometry/glyph hashes agree. Parent a88b8775 common binaries were
frozen before source changes. `baseline-inline-reshape` is an isolated
counterfactual in the same fitter, not an earlier browser. At widths
120/400/1,000 CSS px, median resize/drop µs (reshape control → retained):

| Scene / source | 120 | 400 | 1,000 |
| --- | ---: | ---: | ---: |
| Latin / 4,096 B | 352.262 → 85.076 | 213.174 → 31.651 | 165.852 → 20.839 |
| Arabic / 6,400 B | 466.890 → 60.493 | 324.772 → 31.977 | 283.047 → 22.075 |
| Mixed Latin/Hebrew/Arabic / 4,864 B | 505.033 → 94.538 | 462.389 → 70.160 | 454.266 → 67.831 |

Retained text geometry payload is **77,848 / 71,704 / 92,184 B** for those
scenes, excluding shared source/fonts/base glyphs, headers/capacity/hash and
allocator overhead. At width 400 the layout payload is **10,272 / 6,144 /
25,336 B**, versus control **107,628 / 119,040 / 130,448 B**. All nine default
resize cases reshape **zero** glyphs; unsafe cuts are exercised separately by
the adversarial references. The payload is not total retained memory/RSS.
Inline process RSS: default **7,076–7,228 KiB**, control **7,188–7,464 KiB**;
process CPU **.20/.21/.20 vs .48/.48/.48 s**. These include fonts/harness/all
scenes and are not a browser footprint.

Whole prepare/shape/break/index build/drop medians are **332.068 / 580.430 /
652.520 µs**. Maximum observed index constructors **16.105 / 16.566 / 17.345
µs**; index 128-point slices **26.580 / 335.884 / 9.159 µs**. Worst observed
64-candidate resize slices across widths **18.633 / 323.380 / 21.022 µs**.
Keep outliers and scheduler counters; these are observed worker slices, not
hard latency guarantees or UI/frame/scroll tests.

Common parent/current 10k style build medians (ms): bare **14.159→14.285**,
uniform **21.000→20.979**, varying **27.954→28.250**; style payload unchanged.
Cached ~4 KiB paragraph medians (µs): Latin **246.383→273.308**, Arabic
**318.917→311.532**, Hebrew **316.550→311.187**, Thai **145.853→147.307**.
Latin remains an open variation/regression: the reverse-order pair is
**242.303/242.151 µs**, while original-order samples are slower in current.
Shaping process CPU is equal **.44/.43/.44 s** in both variants. The initial
measurement also showed Latin variation; it is preserved with its one-change
replay patch. Restoring the control-free byte-class scan avoids unnecessary
UTF-8 decoding, but does **not** establish a fix for this timing difference.
Default line-break large Latin/Arabic/long-word (µs): **309.309→309.408 /
401.432→404.027 / 164.503→169.845**; process CPU parent **.53/.53/.53**, current
**.54/.53/.53 s**. No universal performance acceptance follows.

**202 default / 201 all-feature release tests pass**. Strict default/all-feature
all-target clippy, package formatting, hash/regeneration and diff checks pass;
full workspace release build passes (see exact wall time/check commands in
`measurements/2026-11-06-inline-checks.json`). Raw initial/final results,
binary/source hashes and replay script are kept beside it. No GUI was started;
no pixel, theme/scale, reload or page acceptance is implied. No personal
session installation/upgrade/reload. Owned scratch is removed after pushing.

Remaining text work includes mixed inline styles/boxes and fallback line-box
heights, tab stops, other-Zs hanging outside break-spaces, complex visual-edge
trimming, soft hyphens, dictionaries, CSS tailoring/spacing/justification,
caret/hit testing and device/subpixel raster geometry. Next: original inline/
block box layout and native drawing. Networking/V8, full CSS, isolation, shell,
universal dark/night and agent/Olo/password/passkey integrations remain open.
The full goal remains active: **0/100 websites, 0/3,000 website combinations**.

## 2026-11-06 — DOM boxes and initial normal flow; integrate main

Merged `origin/main` through a59422b1 in **af8cbd5e**, preserving the unfinished
flow work and restoring it cleanly. The person's main checkout/session were
not changed. The browser TODO now records the requested ongoing practice:
fetch/merge main at regular working checkpoints, revalidate and push.

Implemented original `layout::{BuildJob,PrepareJob,Job}` from static DOM and
computed styles to normal-flow rectangles and retained text layouts. Compact
box links preserve DOM/style identity; display:none is skipped, contents is
flattened, and block-in-inline content splits into separate text groups.
Source spans map prepared glyph/line positions back to the current DOM nodes.
The used-value solver covers auto/percentage/min/max/content/border sizing,
inline auto margins and direction-dependent overconstraints; flow covers
positive/negative sibling, parent and empty-box margin collapse, barriers,
percentage-height definiteness and relative block offsets. Geometry remains
parent-relative; inspect `bounds` walks ancestors, while painting must carry
origins through one traversal. Constructors/step/finish are worker-only.

Independent Olo Chrome **157** observations on the owned `browser-flow-1`
Agent Screen now pin **139 inputs × 3 scales = 417 DOM-rectangle observations**
at 1×/1.5×/2×. Every observation matches at one/128-action native budgets.
The screen and loopback fixture server are stopped. No personal browser,
profile, display, accounts or session install/upgrade/reload were used.
Fractional fixtures exposed rounding gaps: used values now follow a physical
1/64-pixel grid and direction-aware automatic margin remainders. Computed
borders already snap and must not snap again. Frozen styles retain their
scale, and stale-scale layout is rejected. The CSSOM comparison models
float32 physical rectangles multiplied by float32 reciprocal zoom, then uses
1e-9 tolerance. A separate fixed-position ruler measures the iframe's actual
initial containing block; it is independent of tested in-flow boxes.
This is fixed-metric DOM geometry, not a pixel/theme, arbitrary-font, UA-sheet
or website oracle. All raw observations, probe, server, inputs and provenance
are in `tests/flow-fixtures`; no native output generated expectations.

A bounded temporary paragraph registry shares immutable preparation/shaping/
break/index data for identical font object, text and span schemas. It verifies
source string/ranges/whitespace/base direction before reuse; each current DOM
keeps separate node/style spans, including different colors. Zero entries
disables lookup and insertion. Tests reject incompatible mixed typography and
span schemas. Geometry payload counts shared native paragraphs once. Current
sizes: box record **56 B**, geometry record **128 B**, BuildJob **248 B**,
PrepareJob **288 B**, flow Job **424 B**. Compact geometry storage, complete
incremental invalidation and selective subtree reuse remain optimization work.

Release evidence: **42 alternating CPU0 processes**, three repeats, default
features only. Parent **af8cbd5e** (including main) was rebuilt from its exact
web source before restoring current edits and freezing binaries with reflinks.
No concurrent owned builds/tests/GUI/source edits; no cargo/rustc activity was
detected in the before/after process samples. Sources (104 hashes) stayed
unchanged. All **24 geometry/glyph hashes** agree across retained, reprepare,
no-paragraph-intern and combined controls. These are counterfactual library
paths, not another browser. Raw JSON and replay/check/parent-build evidence:
`measurements/2026-11-06-flow*`.

For 1,000 generated groups, width 400 CSS px, median resize/drop µs:

| Scene | Retained | Reprepare text | No interning | Both controls |
| --- | ---: | ---: | ---: | ---: |
| Empty blocks | 169.058 | 171.336 | 173.772 | 171.520 |
| Repeated text | 656.310 | 820.054 | 629.471 | 5,037.058 |
| Unique text | 644.258 | 5,679.965 | 649.970 | 5,622.248 |
| Repeated nested text | 1,328.949 | 1,668.470 | 1,396.995 | 9,002.114 |

Tree+prepare build/drop medians, sharing enabled/disabled (µs): empty
**47.390/46.734**, repeated **307.779/4,626.925**, unique **5,186.677/5,057.163**,
nested **650.603/8,064.160**. Unique text pays registry overhead; there is no
universal speedup. Glyph/index payload repeated **1,416 vs 1,416,000 B**,
nested **1,944 vs 1,944,000 B**, unique **1,619,160 B** in both. Tree payload
empty/repeated/unique/nested **56,168/182,168/186,058/404,168 B**; width-400
layout payload **128,384/352,384/352,384/832,384 B**. These payloads exclude
shared DOM/styles/fonts, preparation maps, headers/capacity/hash and allocator
overhead. Whole-process RSS retained **12,504–12,680 KiB**, reprepare
**12,528–12,704**, no-intern **16,980–17,204**, both **17,032–17,288**. Process
CPU respectively **.21/.21/.21 / .41/.41/.41 / .35/.36/.35 / .96/.98/.96 s**.
These include harness/fonts/all scenes, not a browser footprint.

Worst observed default constructor **124.750 µs**, 128-action tree slice
**34.886 µs**, one-paragraph prepare slice **123.410 µs**, 64-action flow slice
**615.362 µs** across all sizes/widths/scenes. Observed worker latency is not a
hard bound, UI/frame/scroll result or GPU evidence. There is no painting yet.

Common parent/current 10k style build medians (ms): **14.201→14.440 /
20.898→20.336 / 28.314→29.642** (bare/uniform/varying). Accounted style-tree
bytes increase **8 B total** for the shared scale snapshot. Max-slice medians
(µs): **398.538→435.631 / 623.823→596.891 / 862.766→9,428.005**. Keep the
varying-style wall outliers open; process CPU samples are similar and do not
identify a cause. Cached ~4 KiB Latin/Arabic/Hebrew/Thai paragraph medians
(µs): **268.408→269.436 / 310.095→319.349 / 314.298→313.679 /
144.389→145.475**. Common width-400 inline resize Latin/Arabic/mixed (µs):
**33.187→32.319 / 28.003→28.160 / 69.679→70.361**. No universal performance
acceptance or causal regression fix is inferred.

**209 default / 208 all-feature release tests pass**, including seven flow
groups. Strict default/all-feature all-target clippy, formatting and diff
checks pass. Full merged workspace release build passes: **174.861 s wall**
(cargo reports 2m54s), with the existing proc-macro-error2 future notice.
Owned scratch is removed after the working milestone is pushed.

The stage explicitly refuses unsupported formatting/objects, mixed inline
fonts/sizes/directions/line heights/whitespace, nonzero inline edges and
positioned/vertically aligned inline boxes. Complete CSS, fractional device
shaping/line metrics, arbitrary-font box references, ordinary mixed markup,
full UA defaults, painting/clipping/scrolling, flex/grid/table, live
invalidation and input/hit testing remain open. Next: native page painting and
the remaining inline/box integration, followed by the native shell and runtime.
Networking/V8, isolation, universal dark/night and structured browser/Olo/
password/passkey integrations remain substantial required work. Goal remains
active: **0/100 websites and 0/3,000 website combinations accepted**.

Checkpoint follow-up: flow is committed as **318237bb**. Fetched main again
before pushing and merged the five newer commits through **c61acc5d** in
**15ad99bf**. No web/Cargo runtime source changed in that merge; the 104
measurement source hashes still match. All 209 default web release tests pass
again (**4.391 s wall**), and the full workspace release build passes again
(**41.504 s wall**, cargo 41.24 s). Commands/times are preserved in
`measurements/2026-11-06-flow-latest-main-checks.json`. Main integration is part
of the regular checkpoint workflow, not a one-off catch-up.

## 2026-11-06 — retained page paint and the first native Browser window

Added original retained paint scenes over the DOM flow boxes: backgrounds,
solid borders, foreground span colours/visibility, simple relative paint
order and rectangular ancestor clips. An eight-way bounds hierarchy avoids
walking all paint commands for each tile. Text commands reference retained
shaped glyphs and split after at most 64 glyphs; native variable-font outlines
are cached with font leases and explicit payload/count limits. `tiny-skia`
supplies pure-Rust path coverage/SIMD kernels. Tile-local conversion preserves
fractional geometry far down a document (including the regression at
16,777,216.25 CSS pixels). This is CPU raster plus native GPU texture
composition, not complete GPU web painting or Chromium font fidelity.

`page::load` connects the original parser, embedded styles, initial UA rules,
font host, normal flow and paint on a worker. Independent Chrome capture
caught an actual body-canvas propagation mistake in the initial UA defaults;
Canvas now supplies the backing surface without overriding a transparent
root. Thirty of **36 exact Chrome pixel observations** pass at 1×/1.5×/2×.
Six fractional/multicolour-border observations remain **failures**, with raw
PNGs, browser revision, exact comparator and an explicitly ignored exact
test. No tolerance or masks hide them. Fixed original rectangular glyphs
are not arbitrary-font raster or website evidence.

`olo-browser` is now a native single-document development app. It opens
about/local UTF-8 HTML, streams worker progress/tiles, exposes native address,
Go/Stop, page text, status and menus to agents, supports address/reload/escape
keys and launch forwarding, and retains scene/tiles across scrolls. Palette
and device scale invalidate the page key; viewport resizing can reuse
prepared text. File reads, parsing/font/layout/paint, image preparation and
reclamation run off the UI thread. A 256-entry tile LRU caps cached tile
payload at **64 MiB**. Signed scrolling leaves room for the viewport and
aligned tile end even on extreme CSS document heights. Network navigation
is an explicit error; scripts remain inert. Agent labels are visible paint
runs, not semantic DOM snapshots or a complete input/action API.

Fifteen live light/dark/night reviews at **1×/1.25×/1.5×/1.75×/2×** pass.
The initial status inset error was fixed at every scale. The live Browser
layout lint also passes at 1280×800, 1×/1.5×/2× light/dark. Its existing test
session paths were redirected in-memory into owned `/var/tmp` wrappers,
without changing personal-session files or the repository's general tools.
Added Browser to the regular lint roster. The visual journal records the
native page; journal regeneration first required fetching existing LFS
screenshots. A restarted owned app_server caused kit re-execution and
restored the file and CSS scroll position. Address selection, forwarded opens,
structured network-error status and Escape cancellation were checked.
No personal session was installed, upgraded, reloaded or driven.

Release measurements: **30 frozen CPU0 processes**, three alternating runs;
raw results, source/binary hashes and replay scripts in
`measurements/2026-11-06-paint*`. Parent flow/style executables exactly match
those committed in the preceding flow evidence. Two existing main-worktree
builds were waited for, never stopped. All common flow hashes and cached/
uncached raster hashes agree. An initial harness scale mismatch was rejected
as Stale; the corrected harness was rebuilt and the full measurement rerun.

At 1,000 paragraphs, 1×/1.5×/2× medians (µs): cached scene build
**4,562/4,545/4,550**, uncached **25,410/25,141/25,367**; indexed query
**.167/.150/.131**, linear scan **2.061/2.038/2.073**; cached 256² tile raster
**679/469/348**, uncached **885/603/453**. Retained command/index payload is
**224,360 B**, with **15 glyphs / 5,626 B outline payload** and a conservative
**712,460 B font-source lease** count. Cached 32-box worker-step maximum was
**179.513 µs**. These exclude shared DOM/style/layout/text and allocator/GPU
costs. The 100-paragraph 1× raster median was **927 vs 891 µs**, so caching
was not faster in every wall-time observation. CLI RSS spans **9,588–9,824
KiB cached**, **9,656–9,732 uncached**, not a browser footprint.

Common pre/post unique-1k width-400 flow resize median **678.990→680.568 µs**;
RSS **12,480–12,616→12,504–12,792 KiB**. Common 10k style build medians (ms)
**15.113→15.146 / 21.549→21.516 / 29.070→28.898** (bare/uniform/varying).
Those payloads are unchanged; Layout retains one additional scale value.
No universal speedup or broad performance acceptance follows.

The native release app on a 400-row fixture measured **0% CPU, zero wakeups,
zero frames** over a three-second idle sample. An earlier sample's RSS was
**90,900 KiB** including native window/font/GPU-related mappings, not an
inclusive GPU-memory budget. Final 1.5× scroll drag/flick/wheel median frame
intervals **8.33/8.35/8.34 ms**, first movement **28.7/28.6/3.9 ms**, slowest
client draws **3.32/.71/2.67 ms**. There was **one drag hitch** and two
half-pixel wheel acceleration discontinuities. Keep touchpad latency and
scroll smoothness open; these are not perfect results. An intermediate sample
overlapping our own build was discarded and repeated after builds stopped.

**216 default / 215 all-feature web release tests pass; one explicitly ignored
pixel-gap test**. The browser extreme-scroll release test, targeted strict
clippy (`--no-deps`, default/all-feature web), formatting/diff checks and full
workspace release builds pass. Dependency clippy without `--no-deps` hit the
existing GPUI build-script borrow lint; that unrelated dependency was not
edited. Fetched main repeatedly; it remains at already-merged **c61acc5d**.

Full web rendering/interaction, flex/grid/table, arbitrary mixed typography,
networking/V8, isolation, images/media, semantic agent actions, native icon/
application registration, universal dark/night and Olo/password/passkey
integration remain required. **Goal active: 0/100 websites and 0/3,000
website combinations accepted.** Next: fix the independent pixel failures
and build actual document interaction, then the network/runtime path.

Latest-main follow-up: the native milestone is **08f9c085** and was pushed.
The next fetch found **4a680075**, StyledEdit's Markdown/source syntax work,
including changes in the shared kit. Merged it, retaining all seven new
journal records from both branches and regenerating the HTML/README. Removed
one trailing blank line in the upstream Dockerfile query to keep the diff
check clean. No web runtime source changed; the preceding library measurements
remain scoped to 08f9c085, with the workspace manifest/lock updated by main.

The merged workspace release builds pass (initial **2m20s**, after the query
whitespace cleanup **2m10s**). All **216 default** web release tests still
pass, with the known ignored pixel-gap test. The shared kit/Preferences layout
suite passes **12 tests**, and the Browser passes all **15 live scale/theme
reviews** and its 1280×800 live lint again. A broader full-desktop live lint
was repeated after the build finished: it reports **10 findings outside
Browser** (Disk Usage six crowding, Passwords three overflowing setup controls,
Integrations one footer/scroll-control grouping). Those app sources were not
changed by this browser milestone; this is not a passing full-desktop lint.
The raw findings are retained in `2026-11-06-paint-main-integration.json`.

A fresh merged native 400-row sample still records **0% CPU, zero wakeups and
zero frames** over three idle seconds, RSS **81,152 KiB**. This has different
cache/lifecycle history from the earlier 90,900-KiB sample, so it is not a
controlled memory improvement. Drag/flick/wheel median intervals
**8.34/8.32/8.33 ms**, first movement **28.6/28.8/9.0 ms**, slowest client
draws **.62/.70/.44 ms**, with no hitches in this run. Keep the earlier hitch
and the persistent touchpad latency open. Full compatibility remains **0/100
sites / 0/3,000 combinations**; the goal is active.

Merged follow-up validation also passes all **215 all-feature** web release
tests (one known pixel-gap test ignored), the browser release regression test
and strict targeted browser clippy. The full-desktop lint's ten other-app
findings remain recorded and unresolved; no passing result is implied.

## 2026-11-06 — native scale reference correction and solid paint edges

Revalidated the failing comparisons against actual Wayland output changes.
The previous pixel method used DevTools emulated DPR. At DPR 1.5 it retained
a 3-CSS-pixel border, while actual native scale 1.5 computes that border to
2.66667 CSS pixels (four device pixels). The production computed-style snap
rule was already correct. The old 36 captures, inputs, method and browser
metadata are preserved under `paint-fixtures/emulated`; their comparison was
not a valid native-scale acceptance reference. This corrects the earlier
interpretation, rather than removing unfavourable observations.

The new capture method verifies that the DevTools browser PID belongs to the
owned Agent Screen, changes real output scale, checks native/frame DPR and
font readiness, measures a 100×100 iframe, respects Take Control and restores
settings. It records GPU/browser, computed border widths/DOM rectangles and
source/image hashes. All 90 native PNG hashes match a repeat capture; the
original 60 also match the first native capture. Added 1.25×/1.75× and six
asymmetric, clipped, collapsed, transparent and translucent border inputs.
No personal screen/profile was touched.

Changed sharp rectangle and clip painting to device-snapped bounds, and kept
painted text baselines on device pixels in both commands and their index/agent
bounds. Solid borders now use disjoint strips, with analytical corner mixing
in premultiplied colour before one source-over. This avoids double alpha and
per-border temporary surfaces. Existing SIMD rectangle fills handle strips;
corner work checks cancellation and has tile-bounded storage. A new tiled/
whole comparison caught an empty clipped edge painting a stray strip; explicit
empty-intersection checks fixed it. Distant-coordinate parity still passes.

Against the **same 90 native references**, exact matches rise **36→54**,
differing pixels **4,043→1,848**, worst channel difference **238→11**. No
masks/tolerance are applied. **36 exact failures remain**: font masks at
fractional scales, GPU/asymmetric corner coverage, transparent joins and
retained colour/premultiplication precision. Some equal-width joins differ
by one byte level; asymmetric coverage differs by up to eleven. Do not fit
individual pixels or claim full font/GPU fidelity. All observations remain
in the failing full comparator; the companion exact test for the known set
is explicitly ignored. Uniform half-alpha joins, collapsed inner boxes and
clipped coloured borders match independently at all five scales.

Release measurements in `2026-11-06-edge*`: **42 frozen CPU0 processes**, three
alternating runs, solid/coloured/half-alpha borders and cached/uncached
outlines, plus common flow. All flow/geometry hashes agree between revisions;
cached/uncached pixel hashes agree within each revision. Pixel hashes between
revisions can change because placement/blending changed. No build activity
was observed in the accepted run. The parent runtime was temporarily restored
from **db9910ba** for the expanded harness build and current source restored
in `finally`. Explicit feature vectors are now emitted by both benchmarks.
An initial copied flow baseline had unverified compile-time features; its
comparison was discarded and the complete 42-process experiment rerun with
all feature vectors checked empty. The discarded rows are retained separately.

For 1,000 paragraphs, cached 256² raster parent/current medians (µs), at
1×/1.25×/1.5×/1.75×/2×:

| Border | Parent | Current |
| --- | --- | --- |
| Solid | 646.274 / 535.788 / 450.998 / 421.305 / 334.113 | 626.016 / 525.901 / 444.900 / 401.109 / 314.973 |
| Coloured | 573.291 / 521.429 / 357.536 / 332.330 / 321.370 | 561.542 / 517.862 / 355.463 / 322.621 / 320.333 |
| Half-alpha | 587.887 / 537.939 / 379.094 / 355.975 / 340.424 | 576.704 / 532.997 / 371.945 / 335.694 / 330.155 |

Cached paint-process RSS **9,380–9,784→9,508–9,748 KiB**, process CPU samples
mostly **.31–.33 s** in both. Common unique-1k width-400 flow resize median
**653.610→647.410 µs**; RSS **12,404–13,080→12,440–12,692 KiB**. These are
harness measurements, not browser footprint or a universal speedup.

Fifteen live light/dark/night reviews at 1×/1.25×/1.5×/1.75×/2× pass, with
the running window at odd x/y and no native-control findings. The visual
journal shows the mixed/half-alpha/asymmetric samples. Native idle remains
**0% CPU, zero wakeups, zero frames** over three seconds; a warmed lifecycle
sample RSS was **82,384 KiB**, high-water **85,056 KiB**. Legacy scroll replay
had median intervals **8.34/8.33/8.35 ms**, slowest draws **.73/.73/.52 ms**,
no hitches. It reports first movement **28.4/28.4/9.0 ms** using its old origin.

Latency-method correction: that legacy origin precedes the first timed
report by one 90-Hz period for drag/flick, and its pairwise comparison can
miss the first moving frame if it discarded the before-input pose. The new
replay preserves the pose, reports/submission times and raw client marks.
In a fresh run, first report→moving client-frame marks were
**10.080/8.942/.276 ms**, submission→marks **9.452/8.924/.276 ms**, while the
legacy fields read **29.1/28.5/9.1 ms**. This corrects measurement, not runtime
performance. These timestamps do **not** measure compositor presentation or
optical latency; full input-to-display acceptance remains open.

**218 default / 217 all-feature release web tests pass**, one exact known-gap
test ignored; strict default/all-feature clippy, workspace release build and
formatting/diff checks pass. The manifest also verifies coverage/dimensions
and immutable capture bytes. Next: native font masks and colour/GPU coverage,
actual document input/navigation and the network/runtime path. Full CSS,
interaction, networking/V8, universal dark mode, sandboxing and native
credential/desktop integrations remain required. **Goal active, 0/100 websites
and 0/3,000 site combinations accepted.**

Latest-main follow-up: renderer milestone **c2a34fad** is pushed. Merged main
through **7bf605ea**, including **5288fa00**'s grouped-window first-configure
size fix and StyledEdit's link/undo changes. Web runtime source is unchanged.
Merged validation passes **218 web release tests** (one known pixel-gap test
ignored), the browser release regression, the **12** shared kit/Preferences
layout tests, full workspace release build (**10.15 s**) and 1280×800 live
Browser/StyledEdit lint. All fifteen mode/scale reviews pass again. For this
integration screen, put `target/release` first on PATH and verify `/proc`
executable paths: both session and app_server are the new worktree binaries.
Restarting that new owned app_server restores Browser's document and scroll.
Evidence is in `2026-11-06-edge-main-integration.json`. No personal desktop
was installed, upgraded or reloaded. Historical full-desktop findings are
not a new passing global result; site acceptance remains **0/100**.

## 2026-11-06 — initial HTTP/HTTPS navigation and linked CSS

The native shell now drives in-process HTTP/HTTPS on its page worker, with
pooled connections, native TLS trust, HTTP/2 and transport decompression.
Cancellation wakes stalled network futures; Stop/Escape and newer requests
share the same generation checks. Redirects preserve final URL identity and
fragment inheritance; Reload and upgrade state use that final URL. Relative
file launches resolve against the sender's cwd on the worker, and file URLs
are percent-decoded. Encoded HTML buffers are released before DOM/layout work.

Document decoding now selects BOM/transport/meta/XML metadata, with a bounded
1024-byte prescan and windows-1252 fallback. Plain text stays literal and its
escaped representation is bounded. Linked CSS uses the existing CSS byte
decoder, strict MIME checking, the first document base and document cascade
order. Failed sheets count as issues. Body/redirect/request/source quotas,
web-to-file and HTTPS-to-HTTP stylesheet boundaries have explicit checks.
This is buffered top-level loading, not streaming Fetch, cookies/caches or
complete browser security; scripts remain inert. Imports, conditional sheets,
SRI and many CSS/layout behaviors remain required.

Public smoke exposed real gaps: Example Domain initially failed on viewport
units. Added desktop viewport factors and retained compiled rules; resize,
scale and palette changes now restyle without network I/O, retaining the
layout-only path for independent styles. Example Domain now renders partial
content, Hacker News fails selector state, and CERN's original page renders
text. None is accepted. CERN's valid 8px body margin also exposes a kit inset
lint false positive; don't change website spacing to meet desktop metrics.

The repeatable native fixture suite passes redirects, canonical Reload,
UTF-8 meta, external CSS order/green pixels, literal plain text, pending Stop
recovery and no resize/theme/scale refetch. All **15 live mode/scale reviews**
pass at odd coordinates (light/dark/night × 1/1.25/1.5/1.75/2). Night uses dark
appearance plus forced 4500K night light. Its entire sequence makes 17 requests;
Go-to-stall, Stop and subsequent successful navigation together take **1.340 s**
including CLI input/waits; that is not a transport-cancellation latency figure.

Frozen CPU-0 three-pair static pipeline samples, against 6251094a's page.rs
under the same new harness: 100 elements **948.099→1016.547 µs** median;
1,000 **9183.913→9217.438 µs**. Geometry/source hashes agree, feature vectors
are empty. Process peak RSS ranges **8884–9200→8880–9168 KiB**. The small-page
sample regression remains follow-up work; no speedup or full cost acceptance
is claimed. The compiled-rule retention buys I/O-free restyle.

Frozen native local-scroll measurements: warmed RSS **82612→85060 KiB**.
Both idle three-second samples have **0% CPU, zero wakeups and zero frames**.
Median first-report→moving client-frame marks, drag/flick/wheel, are
**8.830/8.897/.367→8.948/8.910/.315 ms** over three runs each. No hitches;
these are client marks, not compositor presentation or optical latency.

**220 default / 219 all-feature web release tests and seven browser tests**
pass; strict default/all-feature clippy, release workspace, twelve kit/Preferences
layout tests and 1280×800 live Browser lint pass. Owned-session restart restores
the final redirected URL and visible scrolled rows. A first harness attempt
incorrectly signalled app_server directly, ending only the owned session and
causing its waiting browser to time out; the corrected test signals olo-session,
as `tsession reload` does. No personal desktop was installed or reloaded.

Raw evidence: `2026-11-06-network-{pipeline,ui-cost,public-smoke,native}.json`,
`page_bench`, the measurement runner and `tools/browser/network-smoke.py`.
The visual journal records light 2× and night 1.5×. **Goal active: 0/100
websites and 0/3,000 site combinations accepted.** Next: initial/live selector
state, real document input/links, conditional CSS/imports and table layout,
then JavaScript/runtime and isolated resource loading.

Latest-main follow-up: network milestone **798945a5** is pushed. Merged main
through **e3e9499a** (including 7815f2e6 and 80c57ce9): auto-fuzz diagnostics,
fractional-scale fault notes, focus tracing and StyledEdit's early folder
rejection. Browser runtime source is unchanged. Merged validation passes
**220 web + seven browser release tests**, twelve kit/Preferences layout tests,
workspace release build (**11.36 s**) and live Browser/StyledEdit lint at
1280×800. Evidence: `2026-11-06-network-main-integration.json`. Prior native
15-mode/scale and frozen performance observations retain their stated scope;
no website acceptance follows from this merge.

## 2026-11-06 — native text links and pointer selector state

The original engine now resolves initial HTML link state and live pointer
hover/active/focus ancestry. Compiled nested selector state masks avoid full
restyle for irrelevant input changes. Relevant changes currently restyle the
retained document and rebuild visible tiles, with no HTTP or HTML reparse;
full incremental invalidation and complete document input remain required.

Text paint segments preserve source boundaries even when adjacent links have
the same colour. Visible ownership is derived from retained shaping mappings,
not duplicated in commands. A first implementation grew commands 80→88 bytes;
removed that storage, and final commands remain **80 bytes**. Shared clusters
crossing DOM nodes stay readable but have no invented click target until glyph
caret ownership exists. Paint clips constrain structured hit bounds.

The native canvas uses the same primary press/release handler for people and
agent `Link` clicks. Snapshots carry visible text, resolved URL and painted
focus state. Links resolve against the first base and final document URL;
press and release must identify the same anchor. Unsupported schemes,
downloads/new windows and web-to-file links report reasons without requests;
embedded credentials are removed from link snapshots. Also closed the HTTP
navigation redirect-to-file boundary with a loopback regression. New navigation
cancels/invalidates old replies immediately. Replaced/stale image buffers go
to the page worker through non-rendering recycle messages, avoiding UI releases
and render feedback loops. Failed/cancelled restyles retain the old scene/state.

The native fixture passes **15 live light/dark/night × 1/1.25/1.5/1.75/2×**
reviews at odd coordinates: adjacent URLs, hover green/held blue, relative
navigation/return, hidden/clipped links and blocked actions. The complete run
makes 36 expected requests. This is authored fixture evidence, not website
acceptance. Hacker News gets past selector state and exposes the next explicit
failure, **inline box edges**; its earlier public smoke is retained with scope.

The kit now distinguishes authored document records (`document_content`) from
native widget spacing review. This fixes legitimate inline-link/native-spacing
conflicts without changing web geometry. Structured nodes remain present; the
shared regression still detects native control crowding. Native toolbars and
overlays never opt out. Browser fidelity needs separate document comparisons.

Frozen CPU-0 three-pair common pipeline samples, control page/paint runtime
from **76cf0880** under the same harness: 100 generated divs
**923.457→998.048 µs** median; 1k **9295.359→9277.460 µs**. Source/geometry hashes
agree and feature vectors are empty. RSS ranges **8792–9040→8724–9060 KiB**.
These descriptive samples do not establish a speedup; small-page variance/
regression remains open. New full-restyle cost is **881.424 / 9132.821 µs**
for **100 / 1,000 link rows**, each a div plus anchor (not total DOM element
count); RSS **8976–9160 KiB**. This excludes visible tile/GPU/presentation work.

Native local-scroll first sample RSS **84780→82196 KiB**, confirmation
**84936→84884 KiB**; no footprint improvement inferred. All four idle samples
show **0% CPU, zero wakes and zero frames/3 s**. First-report→moving-client-frame
medians, drag/flick/wheel, first **8.997/9.066/.617→9.109/11.133/.344 ms**;
confirmation **9.037/8.949/.452→9.103/9.140/.239 ms**. The first candidate run has
two drag hitches; confirmation has none in either build. Keep both, without
claiming a fix or completed latency acceptance. Marks are not presentation.

**222 default / 221 all-feature web release tests, nine browser tests and
thirteen shared-kit/Preferences layout tests pass**, as do strict web/browser
clippy and workspace release builds. The broader strict UI clippy attempt
reports 31 existing diagnostics in unchanged code, retained in raw evidence.
Full 1280×800 desktop live lint reports the known ten findings outside Browser:
Disk Usage six, Passwords three, Integrations one. Browser passes; the global
result is not a pass. Raw data and reproducible runners are in
`2026-11-06-links-{pipeline-cost,ui-cost,ui-cost-confirm,native}.json` and
`tools/browser/links-smoke.py`. Visual journal: light 2× and night 1.5×.

Next: complete box/advance/empty/image hit testing, keyboard/focus metadata,
fragments/history/tabs, selection and DOM default actions; incremental state
invalidation, inline edges/typography/table layout, complete form state and
runtime/JavaScript isolation. **Goal active: 0/100 sites, 0/3,000 site cases
accepted.**

Latest-main follow-up: pushed **d5089d10**, then merged main through
**73f9c492**, including **994d262a**'s StyledEdit printing/export/code-colour
and Markdown table alignment, plus the auto-fuzz sweep notes. Resolved journal
conflicts by a three-way entry union and regeneration: **588 entries**, both
browser captures and the new StyledEdit capture retained; LFS HTML staged as a
pointer. Browser/web runtime files are unchanged by this merge. Merged checks
pass **222 web + nine browser release tests**, thirteen kit/Preferences tests,
workspace release build (**1m42s**) and 1280×800 live Browser/StyledEdit lint.
Evidence: `2026-11-06-links-main-integration.json`. Earlier full-desktop lint
and performance observations retain their stated scope and limitations.
Website acceptance remains **0/100**; no personal session install/reload.

## 2026-11-06 — document history, fragments and keyboard text links

Added native Back/Forward controls, bounded 256-entry history, final redirect
locations, forward-branch truncation and CSS-pixel scroll restoration. Fragment
entries retain the live DOM and issue no request; cross-document traversal
currently refetches. The complete navigation metadata survives the kit's
upgrade/re-exec path. The Document snapshot caches URL, navigation serial and
DOM identity for structured verification.

Raw then percent-decoded fragment lookup prefers IDs over legacy anchor names;
empty/top cases, duplicate IDs and cancellation have regressions. Target state
reaches `:target` and scrolling uses existing box/text geometry. Pending anchor
scroll survives request cancellation until the UI accepts Content. Keyboard
focus uses a worker-built text-link index, positive tabindex/tree order and
inert/hidden filtering. Inert inheritance is carried by a depth-bounded walk.
Native Tab/Shift+Tab use the kit's FocusNext/FocusPrev actions (the first GUI
test caught that ordinary key listeners never received Tab). Enter and the
documented agent menu commands share activation. Native blur clears document
focus. URL/text bytes are now shared across paint records; pointer focus
metadata updates immediately even without focus-dependent CSS.

Validation: **12 browser release tests**, plus the explicit ignored release
index measurement; **222 web release tests**, strict browser/web all-target
clippy, full workspace release build, thirteen kit/Preferences lint tests and
live Browser lint at 1280×800 pass. Navigation: **15** mode/scale reviews and
**61** HTTP requests including owned-session re-exec, retained-document/no-I/O
fragment traversal, refetched document/scroll restoration, branch truncation,
keyboard focus/activation and Back/Forward. Existing links: **15** reviews,
**36** requests, including the no-CSS focus-metadata regression. Visual journal
records light 2× and night 1.5×; all 1/1.25/1.5/1.75/2× modes were captured.

Frozen common scroll comparison, three runs per build: RSS **84736→85148 KiB**;
both variants **0 CPU / 0 wakeups / 0 frames** over three idle seconds. Median
first-report-to-moving-client-frame drag/flick/wheel:
**8.956/9.131/0.244→8.931/9.072/0.306 ms**; no hitches in either variant.
Median slowest client draw **0.57/0.62/0.58→0.58/0.66/0.72 ms**. These small
samples establish no speedup. Client marks are not compositor presentation.

New keyboard case, 1,001 links, ten cold-document repeats: median first moving
client frame **17.711 ms**, RSS **88336 KiB**, zero idle CPU/wakeups/frames.
This is **above the 8.3 ms target**, with full selector restyle/layout/raster
still on the worker. Incremental invalidation and actual presentation latency
remain required. Cold link ancestry/focus index alone, 30 release samples after
one warm-up: **87.0535 µs / 659.9745 µs** for 100 / 1,000 links. No baseline
counterpart exists for the new feature; this excludes parse/layout/tiles/GPU.
Other main-worktree builds were observed immediately after native samples;
possible external load was not controlled. Raw data preserves sample scope.

Evidence: `2026-11-06-navigation-{checks,native,links,index-cost,ui-cost,keyboard-cost}.json`;
runners `tools/browser/navigation-{smoke,cost}.py`. The pre-commit lint is run
manually with an owned `/var/tmp` root because the repository hook's tools
hardcode RAM-backed `/tmp/olo-dev`; the hook bypass is limited to that rerun.

Still open: box/image/empty-anchor hit regions, complete focus/default actions,
fragment focus starting points, frame/script history, tabs/downloads/BFCache,
URL-byte history bounds, incremental invalidation, V8/security/Olo credential
integration, complete rendering and independent website journeys. Authored-page
pixels still require an independent Chrome comparison; a clean native review
does not establish their typography or spacing fidelity.
**0/100 websites / 0/3000 combinations accepted; the goal remains active.**

Latest-main integration: pushed **6e19c735**, then merged through **b3cc22f0**:
shared search headers/filter-field changes, exact auto-tile edges/minima/frame
anchoring, and opt-in display-mode retention across compositor restarts.
Browser/web runtime files are unchanged by this merge. Workspace release build
passes (**2m12s**), **234 browser/web tests**, strict browser/web clippy,
thirteen kit/Preferences lint tests and both **15-case** native suites pass
again, including the owned-session re-exec. The full 1280×800 desktop lint
retains the same **ten findings outside Browser**: Disk Usage six, Passwords
setup three, Integrations one. Browser is clean. Journal retains all 590 entries.

Merged-desktop frozen paired sample, three replays per build: RSS
**84400→83896 KiB**; both zero idle CPU/wakeups/frames over three seconds.
Median first-report-to-moving-client-frame drag/flick/wheel
**9.870/9.047/0.222→9.168/8.990/0.496 ms**; aggregate hitches
**2/1/0→1/0/0**. Median slowest draw **0.51/0.50/1.41→0.62/0.57/0.61 ms**.
Keep these hitches visible alongside the earlier clear sample; this is not a
speedup or presentation claim. Main-worktree rustc activity was observed at
the beginning of this integration sample, none at its end; scheduling/load
was not controlled. The keyboard latency/full-restyle concern remains open.

Evidence: `2026-11-06-navigation-main-integration.json` and
`2026-11-06-navigation-merged-ui-cost.json`. The first milestone's push hook
also reran and passed its standard Browser lint, inadvertently using its
hardcoded `/tmp` root; that tool removes its session folders on completion.
This integration uses the manually rerun `/var/tmp` lint paths and skips that
duplicate hook. Its full-desktop failure is recorded, not silently called a
pass. No personal session reload/install. Latest fetched main is included;
website acceptance remains **0/100**, and the goal stays active.

## 2026-11-06 — retained inline box spacing

Non-replaced inline boxes now retain original source ranges and actual
first/last edges through anonymous text runs and block interruptions. Sparse
edge prefixes resolve horizontal margins/padding (including percentages and
auto margins) and participate in fitting, alignment and glyph placement.
Empty/collapsed boxes retain their edges; an edge-only box creates a line,
while transparent vertical padding does not move glyphs or enlarge a line.
Fragment storage has its own quota, including repeated block interruptions.

Shaping is isolated at nonzero used inline edges without adding fictional
characters or altering paragraph bidi analysis. Later reshaping respects the
same context cuts. Width-dependent edges that become zero restore joining
from shared prepared text, preserving the width-independent cache. Empty
shaping-boundary sets use an optional thin pointer. Transient edge prefixes
are discarded after placement; retained source ranges support later reflow.
Additional source range/glyph payload is included in geometry accounting.

Independent Chrome capture: **45 authored inputs × five live scales = 225
observations**, each tested at one- and 128-action worker budgets. **198 match**
the documented DOM Range export allowance; **27 remain non-passes**, with 32
pinned X coordinates caused by fractional device-size font metrics at 1.25×
and 1.75×. All block heights, wrapping and letter Y coordinates pass. The
non-pass ledger pins wrong native and independent values; it is a regression
guard, not an acceptance oracle. Inputs/font/probe/reports are SHA-256 pinned.
The first capture assumed requested iframe width; the corrected probe measures
its actual viewport independently. Empty-box break attachment and negative
margin greedy fitting were corrected from the Chrome evidence.

Native link snapshots/actions survive inline spacing and wrapped spans. The
fixture now explicitly resizes to 1000×700 points after every live scale
change and asserts that its wrapped text is visible, avoiding an earlier
pilot with an undersized viewport. The final inline suite and original link
suite each pass **15 mode/scale reviews and 36 requests**; an additional inline
confirmation passes. The live Hacker News result now gets past inline spacing
and padding but stops at **replaced/control/forced-break inline objects**.
An early commentary incorrectly stated that next failure before reading the
saved result; the actual intermediate vertical-padding guard and subsequent
spec/Chrome verification were preserved during investigation.

Inline backgrounds are still unimplemented. Their guard now checks retained
boxes once, including empty padded and nested boxes, so a visible background
cannot silently disappear just because no text glyph belongs to its owner.
Bidi inline-edge distribution, borders/background painting, complete fragment
and hit geometry, mixed typography, device-size glyph metrics and all website
acceptance remain open. **0/100 sites / 0/3000 combinations accepted.**

Release validation: **231 default / 230 all-feature web tests + twelve
browser tests**, strict browser/web all-target clippy, workspace build,
thirteen kit/Preferences lint tests and standalone Browser live lint pass.
The component suite retains its explicit 27 non-passes; no website pass is
inferred from a green regression ledger. Native confirmation completed
normally after earlier long harness chains ended 143 after writing results.
Visual journal includes the same fixture before/after at light 2× and the
candidate at night 1.5×; all fifteen native mode/scale cases were captured.

Frozen worker comparison, seven alternating samples, 60 rounds each, default
features **[]**, identical common source/geometry hashes and unchanged
**80-byte paint commands**: 100/1,000-div median pipeline
**1035.713/8679.268→1043.494/8572.474 µs**; peak RSS ranges
**8620–9072→8720–8976 KiB**. New inline-box path (different authored workload,
no passing before counterpart): **1035.347/8506.456 µs**, RSS
**10244–10656 KiB**. These exclude raster/GPU/presentation and establish no
universal speedup. An earlier investigation sample is not the final comparison.

Native common scroll, three replays per frozen build: RSS **85072→85460 KiB**;
both **0 CPU / 0 wakeups / 0 frames** over three idle seconds. Median first
report→moving client frame drag/flick/wheel
**8.997/8.910/0.408→9.117/8.910/0.509 ms**, no hitches in either variant.
Median slowest client draw **0.47/0.57/0.58→0.61/0.50/0.59 ms**. The unchanged
1,001-link keyboard case measures **17.237 ms**, RSS **88376 KiB**, zero idle
CPU/wakeups/frames; full restyle and the 8.3 ms target remain open. These client
marks are not compositor presentation or optical latency. Own builds and active
GUI tests were stopped before measurements; external desktop load is not fully
controlled.

Evidence: `2026-11-06-inline-box-{checks,native,links,pipeline-cost,ui-cost,keyboard-cost}.json`;
`tools/browser/measure-inline.py`, the frozen native scroll runner and the
`--inline` link fixture. Earlier `2026-11-06-inline-checks.json` is retained
unchanged; this milestone uses the distinct `inline-box` prefix.

Latest-main follow-up: pushed **2008e6a9**, then merged main through
**a83981aa**, including the native GPU-frame recording path and its format/
fallback handling. Merged workspace release build passes (**56.62s**), as do
**243 browser/web release tests**, strict browser/web clippy, thirteen
kit/Preferences lint tests, Browser live lint and another **15 native inline
mode/scale reviews / 36 requests**. Browser/web runtime files and the release
browser's SHA-256 are unchanged by this merge, so the earlier frozen browser
measurements retain their scope. The journal retains **593** unique entries;
all images exist. Full desktop lint was not repeated for this recording-only
merge; its previous ten findings outside Browser remain open. Evidence:
`2026-11-06-inline-box-main-integration.json`. No personal session install or
reload. Website acceptance stays **0/100**, and the full goal remains active.


### 2026-11-06 — semantic forced and optional inline breaks

Normal-flow `br`/`wbr` now retain real element ownership and sparse semantic
metadata without modifying DOM text. Forced breaks survive collapsing;
optional points participate even under nowrap/pre. Prepared metadata is packed
into four-byte entries behind an optional shared pointer. Validation bounds
positions, overlap, count and cancellation; cache sharing distinguishes an
actual optional-break element from literal DOM ZWSP with identical source bytes.
Paint omits marker-only glyphs and structured text filters only semantic
carriers. Shared real clusters stay readable but have no invented click target.
Styled/positioned control cases and unsupported backgrounds still fail explicitly.

Independent Chrome measurements: **83 cases × five scales = 415 combinations**,
**411 matching / four explicit nonpasses**, at worker budgets 1/128. Two
fractional-font exact-fit cases differ in wrapping/height; two RTL cases differ
in horizontal position. The nonpass ledger pins actual and reference values;
it does not accept these failures. Literal Unicode, mappings, malformed input,
quotas/cancellation, cache identity, pixels and text ownership have regression
checks. Website acceptance remains **0/100**, **0/3000** combinations.

Default release tests: **239 web + 12 browser**, with one ignored browser
measurement; all-feature web **238**. Strict web/browser clippy passes (existing
dependency warnings retained), workspace release build passes (**7.10s** for
final lookup/hash hoisting), thirteen kit/Preferences lint tests pass and
Browser live lint reports zero errors. Native component link checks: **15
mode/scale reviews, 36 requests**; navigation: **15 reviews, 61 requests**,
including history/scroll restoration across owned-session re-exec. Only owned
Agent Screens were used. Before/after journal images use the same authored
local fixture. No personal session install/reload.

The first controlled worker sample measured 100/1,000 elements at
**1055.300/8587.087→1079.078/9001.613 µs**. Hoisted control lookup, skipped empty
control hashing and reused element-name lookup. Final alternating seven-sample,
60-round frozen default builds retain equal common source/geometry hashes and
**80-byte paint commands**: **1048.580/8616.924→1058.642/8936.758 µs**.
The remaining ~1.0%/~3.7% regressions are open, not performance acceptance.
New control workload (no passing before counterpart):
**1495.480/13429.661 µs**. Raster/GPU/presentation are excluded. First sample
and final raw results are both retained; external desktop load is not fully
controlled. Own builds and active GUI tests were stopped during final samples.

Evidence: `2026-11-06-break-control-{checks,links,navigation,pipeline-first,
pipeline-cost,ui-first,ui-cost,keyboard-cost}.json`,
`tests/break-control-fixtures`, `tools/browser/measure-breaks.py` and
`links-smoke.py --inline --breaks`. Images/replaced objects, complete CSS,
JavaScript, passkeys, universal dark mode and the complete browser goal remain
unfinished.

Final common pipeline peak RSS ranges: **8536–9112→8724–9212 KiB**;
new-control workload **12072–12264 KiB**. Native common scroll (three replays):
RSS **84892→85788 KiB**; both variants idle at **zero CPU/wakeups/frames**
over three seconds. Median first report→moving client frame drag/flick/wheel
**9.107/9.046/0.381→9.064/8.948/0.522 ms**, zero hitches in final samples.
Median slowest draw **0.62/0.60/0.58→0.64/0.52/0.60 ms**. The first candidate
sample had one drag hitch; it is retained, not discarded. The unchanged
1,001-link keyboard case measures **17.338 ms**, RSS **89008 KiB**, zero idle
CPU/wakeups/frames; the 8.3 ms frame target remains open. These client marks
are not compositor presentation or optical latency. Journal: **595** unique
entries, all PNGs present. Manual lint used owned `/var/tmp` scratch because
the hook hardcodes RAM `/tmp/olo-dev`; only that duplicate hook lint is skipped.


Latest-main integration: pushed **0b412305**, then merged main through
**bdc8036a**, including opt-in native Programs' Secrets and window restoration/
settling fixes. Merged workspace release build passes (**115s**), as do **251
browser/web release tests**, **238 all-feature web tests**, strict browser/web
clippy and thirteen kit/Preferences lint tests. Browser and the newly changed
Preferences Security panel each pass live lint at 1280×800. Full desktop lint
was not repeated; its ten earlier findings outside Browser remain open.

The prior component/native samples ran the development browser on the installed
Agent Screen desktop. For merged integration, started a fresh screen with this
worktree's absolute `target/release/olo-screen`; verified both `olo-session` and
`olo-app-server` execute from this worktree. Repeated **15 link/break reviews /
36 requests** and **15 navigation reviews / 61 requests**, including history and
scroll restoration after re-exec. Browser/web runtime source is unchanged by
main, but shared protocol/system changes alter the linked browser's SHA-256 to
`affb7455e90e9185b7c94e3c2864ec405d47494288ca14854ef034f385664398`.

Repeated frozen before/merged-after native costs on the same newly built
compositor (three replays each): RSS **84628→85776 KiB**; both idle at **zero
CPU/wakeups/frames** for three seconds. Median first report→moving client frame
D/F/W **9.157/9.053/0.416→9.086/8.907/0.224 ms**, no hitches. Median slowest
draw **0.61/0.60/0.59→0.50/0.39/0.40 ms**. Keyboard 1,001 links:
**17.276 ms**, RSS **88724 KiB**, zero idle CPU/wakeups/frames. These compare
the feature plus imported shared libraries; they do not isolate the feature's
cost or measure compositor presentation. The pure engine pipeline regression
and frame target remain open. Build/active GUI checks stopped during samples;
external desktop load remains uncontrolled.

Evidence: `2026-11-06-break-control-main-{integration,ui-cost,keyboard-cost}.json`.
Journal stays **595** unique entries, all images present. No personal session
install/reload or secret-service takeover. Website acceptance **0/100**, goal
still active. Next: profile the measured ordinary-page regression, then continue
images/replaced controls, inline decorations and table layout.


### 2026-11-06 — reuse span colors during retained text paint

Paint construction now resolves/converts the current source span's color and
visibility once for consecutive glyphs. It skips comparing that span's own
color with itself when checking a cluster. Other contributing spans still get
checked; differently painted real text inside one shared glyph remains an
explicit unsupported case. State is local to one immutable text-context build,
with no retained allocation or cross-generation invalidation. Added regression
coverage for color changes across breaks/hidden text and a differently colored
real span inside a shared ligature. Output remains unchanged on tested inputs.

A phase callback diagnostic pointed to paint: 1,000 elements
**3783.627→3944.395 µs** pre-controls→before. Instrumentation changes code
generation, so this was a lead, not exact attribution. Perf task-clock counting
works here; software/hardware sampling is denied by machine policy. No policy
change. A const-specialized ordinary/control dispatch trial had no material
1,000-element scene gain and added about 5 KiB of text; removed it. Its patch
and measurements remain recorded.

Final frozen release measurements, CPU0 (highest advertised maximum 4.7 GHz,
not a frequency lock), seven rotated samples, 200 scene / 60 pipeline rounds.
Baseline is pre-controls **f1e462cc**; before is **ec3fd83c**. Archived engine
revisions use separate standalone build targets and matching release profiles/
dependency versions, with identical diagnostic examples. Every binary reports
**features=[]**; source, geometry and command counts agree; scenes additionally
have matching untimed 256×256 raster hashes. Default command storage stays
**80 bytes**. Own builds/GUI tests stopped during samples; external desktop load
and CPU frequency remain uncontrolled.

100/1,000-element scene medians: before **314.227/3709.820→179.616/2414.210 µs**
(about **43%/35%** faster). Pre-control baseline **318.432/3644.453 µs**.
Control-bearing scenes **350.925/4323.552→220.792/2890.279 µs** (about **37%/33%**
faster). Whole pipeline before **905.738/8897.912→776.512/7441.896 µs**
(about **14%/16%** faster), below the matched pre-control
**895.940/8868.916 µs**. This resolves the recorded regression on this authored
case; it does not establish universal cost acceptance. Ordinary scene peak RSS
**7908–8120→7724–8068 KiB**, control scenes **9832–9900→9696–9864 KiB**;
pipeline **8632–9100→8664–9020 KiB** (overlapping ranges).

**252 browser/web release tests**, **239 all-feature web tests**, strict clippy,
workspace release (**7.16s**), thirteen kit/Preferences lint tests and Browser
live lint pass. Native links/breaks: **15 reviews / 36 requests**; navigation:
**15 reviews / 61 requests**, including re-exec/history/scroll restoration.
Looked at the light/dark/night five-scale screenshot matrix; tested source
geometry/raster hashes and existing independent Chrome component cases stay
unchanged. Four break-control combinations remain rendering nonpasses.

Native common scroll, three replays each: RSS **84880→85200 KiB**, both **zero
idle CPU/wakeups/frames** over three seconds. First report→moving client frame
D/F/W **7.740/9.036/0.489→5.435/5.822/0.473 ms**, zero hitches. Slowest-draw
medians **0.61/0.59/0.60→0.64/0.59/2.41 ms**. Candidate confirmation (original
sample retained): **9.031/9.039/0.478 ms**, draws **0.58/0.44/0.59 ms**, zero
hitches; RSS **85672 KiB**, idle still zero. No causal native-latency speedup
claim: scene construction is not performed by every common scroll frame.
Frozen 1,001-link keyboard: **17.3225→17.3495 ms**, RSS **88360→88264 KiB**,
idle zero. The frame-latency target remains open. Client marks do not measure
compositor presentation or optical latency.

Evidence: `2026-11-06-paint-color-cache-{checks,phase-profile,dispatch-experiment,
scenes,ui-cost,ui-confirmation,keyboard-before,keyboard-after}.json` and the
rejected dispatch patch; `page_profile`, `scene_bench` and matching Python
runners. No new visible design or personal session install/reload. Website
acceptance remains **0/100**, **0/3000** combinations; corpus still needs actual
rank/complexity selection and runnable independent website journeys. Next:
images/replaced controls, inline decorations and tables, alongside the frame
latency and broader performance work.


Latest-main integration: pushed **3a9ceb5c**, merged main through **92364e1b**
(per-thread compositor font systems and better burst timing reports). Workspace
release build passes (**1.34s**), as do **252 release tests**, strict clippy,
Browser live lint and a fresh worktree-desktop **15 link/break reviews / 36
requests + 15 navigation reviews / 61 requests**, including re-exec restoration.
Browser/web source and browser SHA-256 are unchanged by the merge
(`3a2245d6b1e7e489214858dfa96e2ef54e86aba4c0fed4c5338d7e638f982194`), so
frozen engine/feature measurements retain their scope. Native cost samples above
used the earlier bdc8036a desktop; merged-native reviews/fuzz use the new server.

Required performance fuzz, browser-specialized APPS, private two-display
headless tsession/fake HOME, disk scratch, unchanged budgets: seed **4103**,
**36 steps**, passes. Worst loop delay **13.2 ms**, ping **15.4 ms**, compositor
frame **5.9 ms**, app frame **3.2 ms**; fd **112→73**, threads **12→13** with
named lazy workers, RSS **122→128 MiB**. A focused run started before its browser
mapped, skipped every resize/scroll action and wrongly printed "ok". It is
explicitly **discarded as coverage**. Fixed the shared harness to fail when it
measures no bursts; the regression fails against old main and passes with the
fix (no GUI required). After waiting for a real mapped browser, seed **4104**,
**eight resize/scroll steps**, measures both and passes: worst loop delay
**4.6 ms**, ping **7.9 ms**, compositor frame **5.3 ms**, app frame **6.0 ms**,
fd **94→86**, threads **13→13**, RSS **128→128 MiB**. Initial setup can release
older kept buffers/windows, so decreasing fd counts are not a memory speedup.

Evidence: `2026-11-06-paint-color-cache-main-integration.json` (raw fuzzer logs,
discarded run, mapped-window evidence and regression outputs included);
`tools/dev/test_perf_fuzz.py`, HACKING's coverage rule. Agent Screens remain the
native interaction/visual test path; the stock fuzzer's separate two-display
headless harness tests bursts and resources. Full desktop lint was not repeated;
the prior ten findings outside Browser remain open. No personal session install/
reload, secrets access or policy change. The browser journal stays at **595**
unique images (all present); this performance change has no new intended visual
output. Website acceptance stays **0/100**, goal active; next work is images and
replaced inline layout, then further compatibility and latency work.

### 2026-11-06 — Atomic inline geometry and CSS font grid

Added semantic atomic objects to the bounded text worker: isolated glyph-free
shaping, mandatory complete used metrics, bidi order, wrapping/negative margins,
and sparse per-line baseline/height expansion. Ordinary text allocates no object
table. Literal DOM U+FFFC remains font text. CSS flow now passes device scale
through font advance measurement and strut half-leading. Overwide words consume
adjacent trailing zero-width controls together. This is the geometry foundation:
**HTML images still fail explicitly**; decoding, replaced sizing, tree/paint and
agent metadata integration are next.

New independent Chrome set: **76 cases × five live scales = 380/380** matching
object/letter/paragraph geometry observations at both worker budgets. Parent
styles/font requests use real CSS factories; object used metrics are supplied.
Hashes pin the authored inputs, font, probe/server and captures. No native output
generated reference data. Existing unchanged break captures now match **415/415**
(previously 411); inline spacing **219/225** (previously 198), with six remaining
transparent-inline text-item X differences pinned. Normal line-height selection,
arbitrary font metrics/hinting, mixed struts and complete inline alignment remain
open. These component observations accept no websites.

**258 unique release browser/web tests** pass (257 in the full run plus the added
literal-carrier regression); **245 all-feature web**, strict clippy, full workspace
release (**3.52s**), thirteen kit/Preferences lints and Browser live lint pass.
Native links/breaks: **15 reviews / 36 requests**; navigation: **15 / 61**, including
re-exec. Frozen before/after font-grid fixture: **30 reviews**, light/dark/night
at 1/1.25/1.5/1.75/2, live scale changes and odd window X. Looked at the matrix
and enlarged detail; journal has paired light/dark captures, **599 unique images**
(all present). Existing exact native pixel nonpasses remain open.

Frozen default pipeline on one allowed performance CPU, seven alternating runs:
100/1k blocks **774.603/7687.826→777.241/7514.918 µs**. Runner confirmation,
all samples retained: **774.632/7645.572→773.069/7473.493 µs**. Source/command
geometry hashes agree. Initial peak RSS **8852–9152→8940–9100 KiB**; overlapping
ranges, no memory improvement claim. These exclude network, JS, raster/GPU and
presentation. Native common-scroll precise samples (three replays each), RSS
**107452→108236 KiB**, both **zero idle CPU/wakeups/frames**. Report→moving
client D/F/W **9.078/9.144/0.505→8.975/9.102/0.301 ms**; slowest-draw medians
**0.60/0.61/0.62→0.61/0.56/0.61 ms**, no hitches. No causal scroll speedup claim;
this path reuses retained layout. The 120Hz latency target remains open. Earlier
legacy samples are retained separately: their pre-report first_move_ms is not
the precise latency result. `tools/browser/measure-native.py` now retains the
idle offset and actual input-report timestamp for repeatable client measurements.

Private two-display performance fuzz, mapped browser readiness, own fake HOME,
default budgets: **24 steps, seed 4105, pass**. Worst loop delay **7.3 ms**, ping
**10.4 ms**, compositor frame **1.9 ms**, app frame **4.9 ms**; fd **122→86**,
threads **14→13**, RSS **128→131 MiB**. Shell windows are included in this stock
fuzzer, so zero app-frame rows do not establish browser scroll coverage; the
Agent Screen replay above does. Full desktop lint was not repeated; earlier
findings outside Browser remain open.

Evidence: `2026-11-06-atomic-inline-{checks,pipeline,pipeline-initial,ui-cost,
ui-cost-legacy}.json`, fixture SOURCE files and the new native/font-grid/pipeline
runners. Fetched origin/main twice; its current **92364e1b** is already merged.
Both own Agent Screens and the private test session are stopped. No personal
install/reload or secrets access. Website acceptance stays **0/100, 0/3000**;
corpus ranking/journeys and the complete browser goal remain open.

### 2026-11-06 — Byte-backed native still decoder

Added shared `olo_images::raster::decode` for PNG/JPEG/WebP/GIF on resource
workers, with no temporary files. It borrows caller input, enforces encoded,
dimension/pixel and individual surface quotas before pixel decode, caps profiles
before conversion, applies EXIF orientation and the existing native ICC policy,
and returns straight-alpha sRGB RGBA8. JPEG's required encoded-data copy reserves
its known length once; reads/copies check cancellation every 32 KiB. Literal
unsupported formats fail without falling back to the file preview's larger limit.
The existing file API and render paths are unchanged. **Browser images are still
unsupported** until asset/tree/flow/paint/agent integration is connected.

**17 image tests** pass, including file-free lossless formats, malformed and
unsupported bytes, exact/over-limit budgets, valid oversized PNG headers rejected
before pixel-body decoding, 16-bit storage, EXIF/ICC behavior and a live cancelled
reader. **258 browser/web release tests**, browser/web strict clippy and workspace
release build (**1m51s**) pass. Image clippy has **seven pre-existing findings**
in unchanged lib/gain code; its strict command fails on those, with no findings
in the new API/tests/example. No global lint suppression or unrelated fixes.

One frozen release binary, seven alternating direct/bounded controls on CPU0,
identical encoded/pixel hashes: 256² PNG/JPEG/WebP/GIF medians **0.1374/0.3639/
0.0553/0.1165→0.1375/0.3636/0.0557/0.1159 ms**. At 1920×1080 **2.0105/11.3010/
1.7310/3.9777→1.9801/11.3044/1.7734/4.0012 ms**. Worst individual calls remain
recorded (direct/bounded JPEG **14.76/22.85 ms**, GIF **18.68/5.97 ms**); no
maximum-latency or speedup claim. Peak RSS **43660–43968→43908–44132 KiB** includes
fixture encoding, so it is not an isolated decoder working-set measurement.
These authored stills exclude network, rendering, animation and presentation.

Codec allocation limits are upstream **best effort**, not OS process limits;
metadata extraction and colour/orientation conversion can hold other buffers.
Cancellation is cooperative at reads/stages, not inside every codec CPU loop or
colour transform. Hard isolation, aggregate page budgets, animation, SVG and
bounded other formats stay open. Evidence: `2026-11-06-raster-decode-{cost,checks}
.json`, retained Python runner and `raster_bench`. No visible change/new journal
image or personal session install/reload. Fetched main, still already merged
through **92364e1b**. Website acceptance remains **0/100, 0/3000**, goal active.

### 2026-11-06 — Native page still images

Connected worker-decoded PNG/JPEG/WebP/GIF stills to immutable shared assets,
typed page resources, inline/block replaced boxes, intrinsic/percentage/min-max
sizing, HTML dimension cascade hints, retained tiled paint and structured
image/link records. Repeated canonical URLs share one fetch/decode per navigation;
resize/theme restyles retain the same assets. A document has at most 128 image
bindings, rejected during the first walk before external resource callbacks;
encoded images are capped at 16 MiB each and decoded payload at 64 MiB per page,
separate from the existing 64 MiB raster tile cache. Codec allocation hints are
still best effort, with no hard process sandbox or fully interruptible codec CPU.

Full page image geometry matches **380/380** existing independent Chrome object
observations. The new intrinsic/constraints/display/attribute suite matches
**285/300**; **15 top/middle/bottom alignment cases remain unsupported**. New
native Wayland pixel captures match **49/60 exactly**. Nine minification failures
(up to 64 channel levels) and two fractional opaque one-byte differences are
pinned separately as failures, not rendering passes. The observed Intel/Mesa/
ANGLE filter policy is not a universal GPU rounding guarantee. Inputs, independent
captures, browser/GPU metadata, hashes and explicit nonpasses are retained in the
image fixture directories. No screenshots are generated from our engine as oracles.

Initial tiny-skia tile-local inverse transforms introduced 11 byte differences
across tile boundaries at 1.25x. Replaced those with global device coordinates:
whole/tiled output now agrees at all five scales, including borders/padding and
alpha. Compact premultiplied RGBA8 is retained once; normalized texels use a 256
entry lookup. The first two-pixel u16x8 ingestion and explicit f64x4 interpolation
were measured slower and rejected. The final ingestion packs four RGBA pixels
into u32x4 lanes; interpolation uses the faster compiler-selected arithmetic.
A scalar ingestion control produces identical bytes. Minification caches/GPU
image composition remain open.

**266 default browser/web release tests**, **252 all-feature web tests**, strict
browser/web clippy, **13 kit/Preferences lint tests**, workspace release build and
Browser live lint pass. One optional public-network test remains ignored. The
initial merged workspace build took **5m01s**, final rebuild **8.81s**. Native
image checks pass **15 mode/scale reviews**, with one HTTP image fetch for three
fragment variants, no resize/theme refetch and linked-image navigation. Existing
link/hover/break checks pass another **15 reviews**; navigation/history/focus and
owned-session re-exec pass **15 reviews**. The image matrix ran against the
frozen pre-optimization default workspace binary; the packed revision passes
the same independent pixels and is exercised again after main integration. Global desktop lint was not repeated;
previous findings outside Browser remain open.

Private no-shell two-display performance fuzz with one mapped Browser and 40
images, default budgets, seed 4106: initial run fails at step21, with a **66 ms
app frame and 30 ms compositor frame** during shortcuts. A fresh same-seed
confirmation completes all **24 steps**: worst loop delay **7.5 ms**, ping
**10.4 ms**, compositor frame **5.0 ms**, app frame **20.8 ms**; fd46→46,
threads12→12, RSS123→126 MiB. Both runs are retained. The 8.3 ms target and the
initial hitch remain unresolved; passing the stock 50 ms app-frame threshold
is not performance acceptance.

Paired native ordinary-text scroll sample: RSS **107436→113704 KiB**;
**0 CPU and 0 idle wakeups** for both. Report-to-first-moving-client medians
(drag/flick/wheel) **8.934/10.383/0.336→8.934/9.041/0.299 ms**; hitches2→0.
A separate main-worktree build overlapped part of this sequential sample, so
these are recorded observations, not a causal latency or memory claim. The
native/fuzz sample precedes the packed optimization; source/binary identities
and this scope are explicit in the evidence. Main integration gets a fresh check.

Seven alternating frozen processes per comparison on CPU0, zero compiler
activity in all 42 final samples, identical source/geometry/pixel hashes:
1080p ingestion including fresh buffer clone, scalar→packed SIMD median
**1.1565→0.9245 ms opaque, 2.4250→1.2415 ms mixed alpha**. Retained 256² tile
raster is **0.9482–0.9550 ms** at scales1/1.25/2; scalar-control arithmetic is
shared, so tiny tile timing differences are not a SIMD interpolation speedup.
Image harness peak RSS **21272–21636→21048–21544 KiB**, not browser memory.
The first u16x8/f64x4 implementation measured slower; both its full observations
and the intermediate packed run remain retained. A benchmark host initially
supplied scale1 metrics for fractional viewports; corrected to the same scaled
native-font host used by independent pixel tests before successful measurements.

Common 100/1,000-element full pipeline before3035b3ff→after medians
**818.319/8277.681→835.393/8068.671 µs**; isolated scenes
**190.037/2388.630→186.410/2495.630 µs**. The remaining **4.5% 1,000-element
scene regression** stays open. Specializing text-only scene building removes
image bookkeeping but does not establish full cost acceptance. Earlier runs
(including external compiler context) remain visible. These worker samples
exclude network, codecs, JavaScript, GPU and presentation and do not establish
maximum-call latency. Source/binary identities and runners accompany results.

Broken/alt fallback, responsive sources, animation/SVG/other formats, complete
alignment/writing modes/stacking/object-fit and resource/security behavior stay
open. Journal before/after photos preserve the explicit old rejection and the
new light/dark native image page; all modes/scales were checked. Main is merged
through **6f58acf4** with all 601 earlier journal images preserved, plus three
new entries (604 unique images verified). Another fetch found main146c4375;
that integration follows this measured checkpoint. No personal install/reload
or secrets access. Website acceptance remains **0/100, 0/3000**, goal active.

### 2026-11-06 — Revalidate images with main 350f22c6

Pushed **42f94d98**, including main 6f58acf4, image integration, scalar/SIMD cost
controls, independent references and journal/TODO. Explicitly checked/pushed its
six LFS objects. Fetched again and merged **350f22c6**, including the five newer
Passwords, capture, separate-launch, window grouping and tray-menu commits.
Merge is clean; no browser engine source changes were needed.

Merged workspace release build **2m49s**, final rebuild **0.40s**; Browser
**13 tests** (one optional public-network probe ignored), strict browser/web
clippy, **13 kit/Preferences tests** and live Browser lint all pass. Current
image, link and navigation/history/focus checks each pass **15 mode/scale
reviews**, including live scale changes and owned-session re-exec. Image pixel
counts now include only opaque pixels: invisible RGB outside a shaped window
tab must not supply false colour evidence. All 15 image checks were repeated
with that stronger assertion. The engine's earlier 266/252 release test counts
and 49/60 independent pixel result remain unchanged; web sources match 42f94d98.

Discarded the first merged fuzz attempt: a launch was forwarded to a surviving
copy after display-name reuse. Its launcher-PID readiness assertion failed,
but the shell continued and the fuzzer passed; that log is **not accepted
coverage**. A fresh repeat verified the actual window PID, drawn image tree
and exact optimized binary SHA before fuzz. Stock default budgets, one image
Browser in a private no-shell two-display session, seed4106, **24 steps pass**:
worst loop **11.9 ms**, ping **14.8 ms**, compositor frame **2.2 ms**, app frame
**14.4 ms**; fd 46→46, threads 12→12, RSS 125→127 MiB. This still misses the 8.3 ms
app-frame target. Earlier pre-optimization hitch and scene regression stay open.

The native sampler also rejected a forwarded launch before accepting any data.
It now requires an empty owned screen, asserts the fresh PID owns exactly one
window, invokes Quit and awaits process exit with pidfd before the next frozen
variant. Fresh paired native sample, common merged server/default wheel speed1,
no builds: RSS **106972→112708 KiB**, idle **0 CPU/0 wakeups both**. Median
report-to-first-moving-client drag/flick/wheel **9.040/8.965/0.326→9.355/8.855/
0.520 ms**; hitches2→0. These are client submission observations on authored
text, not presentation latency, image loading cost, maximum-call latency or a
general memory/speedup claim. The 14.4 ms fuzz frame and 9 ms motion response leave
the 120 Hz performance goal open.

Evidence: `2026-11-06-image-main-{integration,ui-cost}.json`, updated sampler,
existing image smoke/fixture controls. No new visible UI difference/new global
journal image; all 604 existing journal entries are preserved. Only owned test
sessions were used; no personal install/reload or secrets access. Website
acceptance remains **0/100, 0/3000**, goal active.

### 2026-11-06 — Follow main's session upgrade changes

Main advanced during validation; merged again through **b3064ab8**, including
Agent Screens following upgrades, Passwords field editing/search and Doctor's
rtkit follow-up. Clean merge, workspace release build **11.40s**, **nine session
upgrade tests** pass. Temporary test paths used disk scratch. No personal
upgrade, install, keyring access or Doctor fix was performed.

Browser and app_server release binaries have **identical SHA256 hashes** before
and after this sync, so the earlier rendering and cost evidence remains scoped
to those same binaries. The changed session lifecycle received a fresh owned
Agent Screen navigation/history/focus test: **15 mode/scale reviews and native
session re-exec pass**, including forward/back and CSS-pixel scroll restoration.
No extra image/performance reruns without changed inputs or code. Evidence:
`2026-11-06-image-session-followup.json`. All earlier known compatibility,
pixel and performance failures remain open. Website acceptance **0/100,
0/3000**; goal active. Test windows/screens and owned scratch are cleaned up.

### 2026-11-06 — Bounded image minification

GitHub authentication was restored; verified with `gh api user` and repository
access, then pushed pending f220336a using gh's Git credential helper. No token
was printed or saved in the repository.

The worker now lazily generates source-pixel mip levels with cooperative
cancellation, packed SIMD reduction and an independently exercised scalar
control. The LRU has separate 32 MiB pixel / 128-source bounds, keeps weak source
references and releases levels on new navigation, including failed loads.
Restyle/resize reuse levels; no idle timer or eager full pyramid. Budget
exhaustion falls back to an available level. These are allocation budgets,
not a process memory limit or codec isolation.

Pinned Chrome157/Skia ca84ce0a6f1659748188469a89361ccea71e3a64 sources inform
box/odd-axis triangle reduction and half-level sharpening bias. Actual renderer
comparisons improve **49/60→53/60 exact** against unchanged independent captures.
Five minification captures still differ by at most two channel levels (formerly
up to64); two opaque fractional cases retain a one-byte difference. The original
classification is archived. No GPU-specific LOD approximation was tuned to
force these remaining cases to pass. Broader odd/anisotropic independent browser
comparisons and GPU composition remain open.

Release default browser/web **270 tests**, all-feature web **256**, Browser13,
kit13 and strict browser/web clippy pass. Workspace release build **19.70s**;
existing dependency warnings remain. New tests cover odd/even normalized
convolution, premultiplied alpha, cache limits/LRU/weak lifetimes/cancellation,
whole-page versus 8×8 tiles at five scales, restyle reuse and reclamation.
All68 fixture hashes verify. No reference capture or authored input was changed.

The initial generic two-level draw measured ordinary image sampling about40%
slower, with substantial compiler overlap retained in that initial dataset.
Separate const draw paths and removal of redundant snapped-edge coverage recover
most of it. Seven rotated frozen release processes per variant, CPU0, retain
**49 accepted observations** and four compiler-overlap attempts separately.
Final ordinary 256² upscale tiles remain **~0.94→0.98ms (~4% slower)**; this stays
open, rather than being hidden by the new path's SIMD improvement. A 2048×1536
image minified into a 256² tile takes **2.94/3.01ms cold SIMD versus7.32/7.36ms
scalar** (opaque/alpha), **~0.70ms warm**. Three cached levels retain4,128,768bytes
and warm runs generate no additional mip pixels. All common old/new bytes and
new scalar/SIMD bytes agree. Matched new-process image RSS median25764/25816KiB
(default/scalar); old/new process RSS is not comparable because workloads differ.
Text pipeline100/1000 elements **846.48→844.54 / 8371.00→8113.74µs**; scene
**185.12→190.71 / 2535.98→2464.44µs**. These observations do not erase the earlier
isolated scene regression or establish full-host isolation.

Owned native minification demo: **15 mode/scale reviews pass**, source reuse
across resize/theme, structured image/link bounds and image-link navigation.
Source SHA and actual fresh window PID/executable SHA are verified. The full
contact sheet and original-resolution before/after details were inspected;
all light/dark/night modes and1/1.25/1.5/1.75/2× were exercised live. This tests
native palette behavior, not universal author-colour darkening.

Live Browser layout/review lint at1280×800 and all five scales reports zero
findings. The paired ordinary-text native sample reports RSS108008→110756KiB,
idle0 CPU/0wakeups both; first-moving-client drag/flick/wheel medians
9.329/9.112/.435→9.629/9.243/.976ms. Compilation overlapped both native attempts;
their opposite hitch counts (54→0 initially, 0→57 on repeat) and latency figures
are **inconclusive**, not evidence of a speedup or an established regression.
Both datasets and compiler context are retained. These are client submissions,
not compositor presentation or optical latency.

A fresh two-display private session, real PID/binary/drawn-image identity and
seed4107 **fails at step14/24**: arrange x12 produces a **78.8ms app frame**
(23.4ms average over four), above the stock50ms budget. Worst loop27.9ms,
ping11.7ms, compositor8.2ms, answer179.3ms. app_server FD48→46,
threads15→15, RSS126→128MiB. Heavy foreign workspace compilation was active; that alone does
not establish the cause. This is retained as an actual nonpass and needs
profiling/control comparison. Two earlier preflight failures never ran the
fuzzer: a frozen executable outside the tree, in a fresh fake home, lacked
`OLO_DATA` and correctly exposed “Browser font is unavailable”. The last tree
is retained; supplying the repository data directory (same font SHA as the
installed copy) fixed the fixture. No empty/error-page run counts as coverage.

Evidence: `2026-11-06-minification-{checks,cost,ui-cost}.json`, initial cost and
native samples, pinned fixture provenance, benchmark/smoke tools. Added two
before/after global screenshot entries; all604 earlier entries are preserved.
Only owned screens/test sessions were used; no personal upgrade or install.
Website acceptance remains **0/100, 0/3000**. Full JS, layout, security, universal
dark mode and desktop integrations remain substantial unfinished work.

### 2026-11-06 — Main sync and real-page priority

Minification checkpoint0106c55c is pushed. Merged main through574d8ddb in
5b2f9103, including Secret Service/gh repair, card autofill and recording
fallback work. Browser/web release270 tests and strict clippy pass; workspace
release build4m22s. Browser SHA is unchanged; app_server SHA changes from
cdb588bb… to91b8aa46…. Existing native cost evidence retains its original
server scope. Combined native evaluation follows with real-page testing. No
personal upgrade/install or secrets access was performed.

The user asked about reaching real pages. HTTP/HTTPS already works, so initial
live-page diagnosis starts now rather than waiting for every component to be
polished. First public probes: Wikipedia article, GitHub public repository and
Hacker News; initial observations/captures are diagnostics, not complete corpus
journeys or acceptance. A frame diagnostic recorder was drafted in disk scratch
but not committed or counted as verified; investigating the retained79ms frame
remains queued while real-page testing takes priority. Website acceptance
remains0/100 and0/3000. Evidence:2026-11-06-minification-main-sync.json.

### 2026-11-06 — Follow real pages; Stripe first

The user asked when real-page testing could start. HTTP/HTTPS was already
implemented; isolated component refinement had delayed useful end-to-end probes.
Ran live Hacker News, Wikipedia and public rust-lang/rust GitHub startup probes.
All fail: missing decoded image (HN), CSS StateUnavailable (Wikipedia/GitHub).
These are native startup diagnostics, not corpus journeys or visual passes.
Evidence:2026-11-06-live-startup-probes.json, including probe source/identity.

The user then selected **stripe.com first**, finishing each website before the
next. Its site-specific queue is in eval/stripe/README.md. Private Chrome loaded
the actual public homepage; captured initial DOM/typography/resource inventory
and hashes for original HTML/five stylesheet bodies in owned disk scratch.
The first resource-tree response omitted four stylesheets; the actual DOM sheet
list corrected the inventory. This is not a complete replay or accepted visual
reference. A web-search-service Indian regional redirect is not used as baseline.
No account, payment, form submission or personal profile was used.

A precise selector diagnostic identifies the first native Stripe blocker as
**Disabled**. Added original Rust connected-tree CSS enabled/disabled state,
lazily on the page worker. A bounded traversal carries fieldset/first-legend and
select/optgroup contexts, then stores sorted sparse entries: **8bytes/control**,
no DOM clone or repeated ancestry scans. Node/work/control/depth limits,
cancellation and document/revision/handle checks are explicit. Inapplicable
elements return neither state without building the index. No timers or threads
are added. The model is initial inert CSS state, not form activation/submission,
widgets, Web IDL or shadow/custom-element upgrade behavior.

Independent original Chrome157 fixtures: **176 element observations /352 CSS
boolean assertions**, including legend movement and attribute changes, match.
Pinned Chromium sources confirm the important distinction: option/group CSS
state includes the owning select (OptionDisablednessCheckAncestors), unlike
intrinsic option disabledness. Fieldset selector state uses inherited actual
disabledness. Two native regression groups cover resource bounds/stale handles
and full-page CSS selection/rebuild after a native DOM attribute change.

Actual freshly launched workspace Browser (PID/executable SHA verified) now
gets past Disabled and stops later at **Variables(Unsupported(arbitrary
substitution function))**. Retained Stripe CSS uses
`calc(env(safe-area-inset-bottom,0px) + var(...))`; env/host-context support is
next. Captured115 independent env observations at five native DPRs for that
next task in disk scratch; this is not implemented env acceptance. No selectors
were silently forced false and the real site was not replaced with a fixture.

Release default browser/web **273**, all-feature web **259**, strict clippy and
workspace build **17.68s** pass. Existing dependency warnings remain. Cost
controls: CPU0/seven rotated frozen processes/variant, **35 accepted samples**,
three compiler-overlap attempts retained, nine samples with Cargo/test context.
Sampler paused before measurements while an independent Chrome fixture was
captured, then restarted after owned GUI apps were quit. Native server/build
identities are separate in the evidence; no source equivalence is assumed.

Index cold construction100/1000 inputs: **5.17/50.96µs** with first legend,
**8.23/88.83µs** with late legend. Warm whole-set queries **1.78/19.60µs** and
**1.55/22.92µs** respectively; direct input-only ancestor-walk controls
**15.87/141.70µs**, **67.63/5183.46µs**. Payload **808/8008bytes** includes the
fieldset. Walk/index bytes agree; this is not a general website speedup.
Ordinary pipeline100/1000 **1257.02→1245.31 /11889.36→12320.26µs**; scene
**282.24→328.15 /3594.45→4042.43µs**. Source/geometry/pixels match. Host load is
high and scene building never queries the new index; the observed regressions
and their cause remain open, rather than being declared improvements. Earlier
image sampling, scene and79ms fuzz findings also remain open.

Evidence:2026-11-06-stripe-controls-{checks,cost}.json, original control fixtures,
benchmarks, bounded CDP transport and site queue. There is still no useful native
Stripe content or new site visual to accept. No global screenshot journal image
was added for a CSS-state/diagnostic-only milestone. **Stripe0/30, corpus0/100
and0/3000**; goal active, next task env substitution on the same real website.

### 2026-11-06 — Follow main; preserve headless identity across self-upgrade

Stripe state milestone85200762 is pushed. Merged main throughc753797c in
c41a889b (Token Pool drain continuity, session upgrade handling, audio/Doctor
and shared kit changes). Browser13 tests, kit-base restore2 and session upgrade9
pass. An initial olo-ui upgrade filter matched0 tests and is not coverage.
Workspace release build2m39s; complete session suite20 pass/1 ignored.

The native restart check exposed a real session integration bug. Rebuilding
replaced the test session executable, so Linux's /proc link had the legitimate
“(deleted)” suffix; the old test rejected it before signaling. The guard now
checks the owned screen name/socket/display, executable basename and headless
argv explicitly. Then a real reload self-upgraded the session **without its
backend arguments**. Agent Screens' liveness check lost --headless and removed
the record although the private session/Browser were still running. No reload
coverage was accepted from that failure.

Session exec now carries its backend argv as well as Resume state. Only this
rare exec path changes; no idle/frame work or timer is added. Cleaned the known
private orphan with socket-peer/window-PID/executable/control ownership checks;
no personal process was signaled. A proposed cross-filesystem staging rename
failed before signaling and is not coverage. The accepted test starts a fresh
owned screen, touches session source and **rebuilds normally**, verifies changed
inode/deleted running executable, then reloads. The headless flag and registry
record survive; **15 mode/scale navigation reviews, scroll/history restoration
and re-exec pass**. End-to-end reload plus following history checks1.368s;
this includes CLI observation and is not isolated restoration/presentation latency.
Before the fix the flow failed because the registry disappeared, so no comparable
success latency exists. Evidence:2026-11-06-stripe-main-session-sync.json.

This is required integration upkeep, not another site milestone. Stripe remains
active and0/30; corpus0/100,0/3000. Return to CSS env substitution next. Existing
rendering/performance nonpasses remain open; no personal install or reload.

### 2026-11-06 — Stripe env substitution and the next real failure

Implemented bounded CSS env() substitution over an immutable document-wide host
snapshot. Names are case-sensitive decoded identifiers (escaped references
included); nonnegative integer index tuples distinguish bindings. Literal
invalid indices reject declarations; missing names/tuples select only the needed
fallback. Defined empty values stay defined. Dynamic arguments, recursive/empty
fallbacks, custom cycles, taint, cancellation and output/work bounds use the
existing variable resolver. Host publication rejects pending values/duplicates
and caps payload. Native rectangular content viewports share eight zero current/
maximum safe-area insets (<2KiB accounted payload). Defaults freezes the host
snapshot once per generation; a changed revision rebuilds consumers. Shared
snapshot storage is charged once; arenas also exported as custom values are
conservatively charged in both categories, not an exact RSS measurement.

Independent original Chrome fixtures:23 cases ×5 native scales =115 computed
observations. All match through HTML/style/layout/paint, including Stripe's
actual calc(env(safe-area-inset-bottom,0px) + var(...)) expression and lazy cycle
fallbacks. The checked-in owned-screen capture driver was rerun and reproduced
all115 rows, input hash and pinned Chrome version. This is computed-style
coverage, not site pixels. A final review corrected decoded host-name handling;
escaped identifier and publication-bound checks pass.

The actual native HTTPS Stripe retry advances from Unsupported(arbitrary
substitution function) to **CSS style ContextUnavailable**. Keeping original
font-loader error detail leaves that diagnostic unchanged. Retained CSS contains
container-relative units and actual homepage carousel elements match those rule
classes; this is a candidate, not demonstrated cause. No errors were suppressed,
no viewport substitute was invented and no site HTML was replaced. There is
still no useful native Stripe content. Both owned screens were quit with PID/
window/executable checks and stopped afterward. Five-second idle in the native
error view:0 CPU/0 wakeups; this is not useful-page idle acceptance.

Full release suites276 default browser/web and262 all-feature web pass before
the final decoded-host-name-only correction. Final focused environment/variable
suites15 default +15 all-feature pass, strict all-target clippy and workspace
release build8.25s pass. An earlier clippy attempt raced a diagnostic source edit
and used inconsistent artifacts; that failure is not accepted coverage. Final
checks ran against stable source. Format, Python AST and diff checks pass.
Fetched origin/main:still c753797c, already merged. No personal install/reload.

Final ordinary cost:CPU0, seven rotated processes/variant,28 samples, no compiler
waits/discards or Cargo/test context, host load6.47–7.19.100/1000-element pipeline
**819.16→814.03 /8213.89→8835.89µs**; retained scene
**208.63→190.04 /2711.48→3395.01µs**. Source/geometry/pixels match. The larger
pipeline (~7.6%) and scene (~25.2%) regressions remain performance nonpasses;
scene work does not query env bindings, and no cause is claimed. Initial run
before the decoded-name correction is retained separately:28 accepted,4 compiler
overlap discards,11 waits,12 samples with Cargo/test context. Earlier image/
scene/fuzz nonpasses also remain open. Do not accept performance from small-case
improvements. Evidence:2026-11-06-stripe-environment-{checks,cost,cost-initial}.json.

**Stripe0/30; corpus0/100,0/3000.** Goal remains active. Next:identify the missing
CSS context precisely, implement the required semantics, then retry Stripe.

### 2026-11-06 — Name Stripe's missing query-container unit

Environment milestone08bea308 is pushed. A structured numeric-unit diagnostic
on the native host confirms the next live failure:
**CSS style Numeric(UnitContextUnavailable(Cqi))**. Only the unavailable-unit
error construction changes; successful unit calculations and idle/frame paths
are unchanged. Browser13 pass/1 ignored, math/property/style37 pass, strict
all-target clippy and workspace release build pass. The first compile correctly
required the new error to be handled by color's existing generic context-error
mapping; that failed build is not accepted coverage. Final source/checks are
stable. The owned live window's PID/executable hash and full diagnostic are
recorded, then it was quit and its screen stopped. Stripe is still0/30, no
useful content. The following fetch completed with new main5e9df2d2; its
integration is recorded below.

CSS Conditional5 (2026-11-02 draft) requires nearest eligible ancestor selection
per relevant axis, with small-viewport fallback only when none is eligible:
https://drafts.csswg.org/css-conditional-5/#container-lengths . The actual Stripe
HTML has carousel elements and retained CSS uses cqi on their widths. The current
engine computes all styles before layout; next implement container-dependent
box-length resolution with query-container geometry. Earlier font-dependent
contexts remain explicit errors until their computation path is implemented.
This diagnostic is not container-unit support, pixels or performance acceptance.
Evidence:2026-11-06-stripe-container-diagnostic.json. Prior performance nonpasses
remain open; goal active and corpus0/100,0/3000.

### 2026-11-06 — Merge the new Terminal title service

Merged origin/main5e9df2d2 as5b07a3df after the precise Stripe Cqi diagnostic.
The fetch completed while the preceding journal entry was being written; main
had advanced fromc753797c. The merge carries the loopback HTTP title service,
Terminal context/title changes, shared restore/session plumbing and documentation.
Read the changed HACKING instructions; no personal install or reload was run.
Workspace release build2m09s, kit-base restore2, session upgrade9 and Terminal
loopback title-service integration1 pass. The latter exercises concurrent calls
through repeated exec upgrades. Main sync is pushed with this journal checkpoint;
Stripe's active next task remains query-container box-length resolution. Frozen
parent benchmark examples use this merged web core before container changes.
No site/configuration was accepted and prior performance failures remain open.

### 2026-11-06 — Stripe reaches flex; initial horizontal container box lengths

Implemented container-type normal/inline-size/size, including all:initial reset,
and selected deferred relative units in the existing typed numeric evaluator.
Supported box dimensions, margins, padding and insets can wait for horizontal
layout; font/viewport/tree inputs keep their original computed context. Active
layout frames carry16 bytes of per-axis nearest eligible ancestor content-size
bases. A container's own dimensions use its ancestors; nested eligible axes can
differ and missing axes use the small viewport. Size containment freezes an
empty auto block size; query containers establish their formatting context.
No per-length ancestor walk, second document index, polling or frame timer.
Prepared styles/text survive resize while query geometry is recalculated.

This is partial support, with explicit errors for early font/color/border
contexts and still-pending inherited query lengths rather than rebinding them
to a child. Vertical layout, named/conditional queries, combined types/container
shorthand, replaced-element containment and complete containment remain open.
Stripe's retained CSS actually uses inline-size containers. The live HTTPS
startup now gets through the initial style stage and stops at **flex box
formatting**. Its own deferred query lengths have not yet been resolved: the
formatting tree stops before geometry. No useful native Stripe content and no
site/configuration is accepted.

Independent original Chrome:8 ancestor contexts ×6 units ×5 verified native
DPRs =240 widths, all match the full native page pipeline within0.0001 CSS px
(float32 exposure tolerance, below a layout subpixel). Captured fractional small
viewport dimensions because innerWidth/Height round them. The first authored
fixture used unsupported border shorthand; replaced it with equivalent longhands
and recaptured Chrome with the new hash. All240 probe and host observations stay
identical. No actual Stripe input or Chrome expected value was edited. Additional
checks cover changed containment, all reset, cancellation, cache reuse on resize
and early/inherited context rejection. Broad border shorthand remains required.

Recognizing the original fixture container-type contexts enables69 more style
ports:312/341 pass,29 unsupported. Every original243 passing identity is guarded;
all remain passing. The font subset improves93→137/149,12 unsupported. Pinned
upstream/derived inputs/references stay unchanged; only local fixture README
hashes update with documentation. Default browser/web280, all-feature web266,
final focused23 and post-documentation style/font26 pass. Strict all-target
clippy, format/AST/diff and workspace release build26.33s pass. Earlier old-count
assertions failed and were corrected only after validating the independent
outcomes. Final production source is frozen in the measured release binaries.

Native navigation with actual Cqi padding:15 mode/scale reviews, history/scroll
and owned re-exec pass. One tree request resets during re-exec and the expected
reconnect loop retries it; restored URL/navigation generation, section position
and forward/back without network requests pass. Reload+following history checks
1.536s, not isolated presentation latency. A visible authored nested-container
component also passes15 reviews, with all15 screenshots inspected. Five-second
useful-component idle0 CPU/0 wakeups. Both owned screens quit/stopped. Global
journal preserves606 entries and adds two component shots (608), with original
JSON formatting; no forced-dark site/night-output or Chrome pixel acceptance.

Ordinary cost against frozen merged-parent5b07a3df:CPU0, seven rotated processes
per variant,28 accepted samples,2 compiler-overlap discards/4 waits/3 samples with
Cargo/test context, load43.72–46.01.100/1000-element pipeline
1271.93→1149.55 /11127.51→11899.97µs; retained scene
272.26→236.98 /4264.27→4328.84µs. Source/geometry/pixels agree. Larger pipeline
~6.9% and scene~1.5% regressions stay nonpasses; host isolation/causality is not
established. Prior image/environment/scene/fuzz performance failures remain open.
Evidence:2026-11-06-stripe-containers-{checks,cost}.json, original fixtures and
native navigation/screenshot journal. No general website speedup is claimed.

Origin/main advanced to9f23236a (recording controls); merge next after committing
this working milestone. **Stripe0/30, corpus0/100,0/3000; goal active.** Next
required site feature is the original flex formatting/layout implementation.

### 2026-11-06 — Follow recording main; inspect actual Stripe flex requirements

Cqi milestoneea49514a is pushed. Merged main9f23236a ascc3ed9ea, bringing
recording controls/status protocol, the shared recording-light drawing helper
and capture-only layer exclusion. Read the changed source/HACKING; native
screenshots still include visible controls while video/share captures omit them.
Workspace4m41s, protocol35 and Browser13 pass/1 ignored. A fresh owned session
and freshly rebuilt Browser pass15 Cqi navigation mode/scale reviews, history,
scroll and re-exec checks. No personal install/reload; private windows quit and
screens stopped. Evidence:2026-11-06-stripe-main-recording-sync.json. This is
integration upkeep, not website acceptance.

A read-only Chrome inventory now records20 actual visible flex contexts from
https://stripe.com/ at native DPR2, readyState complete and fonts loaded. The
header is a nowrap row, with auto-sized navigation items, gaps and center
alignment; nested items also use flex basis/grow/shrink. Stored technical style/
geometry metadata only, no marketing text or account/payment interaction.
Rejected preflights are explicit: scratch import path, about:blank with zero
contexts before navigation committed, and a stale debugging-port startup read.
None became site evidence. The corrected driver verifies screen/PID/profile,
waits bounded startup and actual URL/load, rejects empty data, then closes its
Chrome and waits for exit. The reference screen is stopped.

Retained inventory/driver provenance in eval/stripe/flex-inventory.json is not
complete capture/replay or pixel acceptance. Next: intrinsic text min/max content
widths from existing shaped prefixes/break data, then original flex sizing,
wrapping/alignment and anonymous item construction. The current text API retains
those inputs but exposes no intrinsic-width operation yet. No line width will
be guessed and no existing browser engine substituted. **Stripe0/30, corpus0/100,
0/3000; goal active.** Prior performance nonpasses stay open.

### 2026-11-06 — Retained intrinsic text widths for the Stripe flex dependency

Implemented original `text::layout::IntrinsicJob`, currently outside the page
formatter. It measures retained glyph/cluster prefixes and break points without
line/fragment output or new shaping. Its248-byte job bounds numeric prefixes,
steps/cancels on the worker, poisons errors and publishes a16-byte min/max pair
for the caller's layout snapshot. A width-only change need not rerun it. Initial
scope is homogeneous horizontal LTR text with uniform whitespace and one shaping
item per hard line. Bidi/adjacent script/font runs, inline objects/decorations,
discretionary hyphens and complete item/intrinsic/flex CSS remain unsupported.
General GPOS/variable-font metric parity is unestablished. Normal line fitting
and actual page geometry do not call this operation yet.

Independent original Chrome fixtures embed the same three adversarial TTF files:
60 cases ×5 verified live DPRs =300 min/max pairs,600 widths, all within0.0001
CSS px. Every original140 row remains unchanged in the expanded capture. The
first native attempt failed24/140 (unsafe ligature cuts and rounding); an
intermediate prefix attempt failed32, original retained boundaries reduced that
to13, and the first new retained pass matched140. Expanded cases then exposed6
further failures, all corrected without changing expected widths or tolerance:
font effective-size buckets require float (not double) multiplication before
flooring, and preserved leading spaces split only with wrapping. Final default
and all-feature suites cover the full300 pairs. Primary Chrome/Skia source was
read to trace these mechanisms; no existing layout engine is substituted.
Historical failures are retained in stripe-intrinsic-checks.json.

Rejected scratch diagnostic attempts: malformed JS quoting and10s --idle
timeout; no reference data accepted, owned Chrome quit. A later metric-only
diagnostic used frame acknowledgements plus actual DPR; its original140 values
match the first capture. The independent expanded capture itself passed the
original idle/DPR guards. All owned Chrome processes exited and screens stopped;
no personal session interaction. No site or screenshot acceptance is implied.

Release Browser/web284 pass,2 ignored; all-feature web270 pass,1 ignored.
Strict all-target clippy, format/AST/diff checks and release workspace8.83s pass.
`inline_bench ROUNDS intrinsic|materialized` validates equivalent ordinary Latin
widths before timing. The initial1000-unit untimed setup exceeded the existing
16KiB shaping-run limit; reduced the benchmark to100/500 units (500/2500 words),
without widening production quotas. CPU0, seven rotated100-round processes per
variant,14 accepted/no discards/no compiler contexts, load13.48: materialized
119.07/626.31µs versus intrinsic33.51/190.24µs. Numeric temporary payload
22,416/112,016B versus line/glyph payload48,096/240,096B; excludes capacity,
shared source/fonts, headers/allocator overhead and control shaping caches.
Worst observed constructor/128-step3.54/309.67µs for intrinsic; control's worst
step824.93µs. Both variants warm both paths, so process RSS8,300/8,364KiB is not
an isolated allocation comparison. This is a worker-control comparison, not a
website or frame-speed claim.

Ordinary before/after uses frozen default-feature webea49514a (unchanged in
parent4016837d).28 accepted samples,0 discards,60 compiler waits and1 accepted
Cargo context; load14.92–16.81, CPU0, geometry/pixel/source hashes agree.
100/1000-element pipeline809.94→814.49 /9000.35→8886.89µs; retained scene
186.14→190.19 /3265.09→3496.42µs. Scene~2.2%/~7.1% and small pipeline~0.6%
regressions remain nonpasses; the new operation is not invoked on these paths
and causality/full isolation is not established. Prior performance/image/fuzz
nonpasses remain open. Evidence:stripe-intrinsic-{checks,ordinary-cost,worker-cost}
JSON and original text fixtures. No overall performance improvement is claimed.

Fetched origin/main12069396 using gh-backed credentials; merge after this
working checkpoint. **Stripe0/30, corpus0/100 and0/3000; goal active.** Next:
original flex item construction/sizing/alignment consuming measured content,
then retry the same actual Stripe URL. No other site is selected.
