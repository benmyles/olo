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
