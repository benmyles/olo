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
-- -D warnings`; formatting. Full workspace release build also requested.
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
