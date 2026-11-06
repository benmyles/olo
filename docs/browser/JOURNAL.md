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
