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
