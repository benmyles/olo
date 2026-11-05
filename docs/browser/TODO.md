# Browser TODO

Keep this file and JOURNAL.md current with each milestone. A checked
item means implemented and verified; a partial implementation stays open.

- [x] Inspect Olo's kit, browser history, agent protocol, keychain and rules.
- [x] Record architecture, acceptance criteria and honest compatibility gates.
- [ ] Build a reproducible 100-site corpus with explicit provenance,
  journeys, independent baselines and fail-closed reporting. Curated
  100-domain selection/ranks and native comparator implemented; site-specific
  executable journeys, capture/replay driver and baselines remain.
- [ ] Complete HTML tokenizer conformance (initial token implementation
  matches 7,042 html5lib cases; four UTF-16 cases unsupported; parse-error
  diagnostics and streaming byte-input interface remain).
- [x] Compact mutable document storage, source-backed text, stable scoped
  handles, bounded arenas, atomic tree validation, iterative reclamation
  and release storage measurements.
- [ ] Implement complete HTML tree construction and web-facing DOM semantics
  (Web IDL, arbitrary attribute namespaces and namespaced setters,
  mutation observers, ranges,
  event dispatch, shadow DOM and GC rooting). Amortized adjacent-text
  appends and source-range merging are implemented and measured.
- [x] Connect tokenizer to original static document/head/body/list/form/raw-text/
  formatting/frameset tree construction; independent WPT tree runner and
  per-case regression baseline. 3,898/3,904 tree cases match; this is partial.
- [x] Static table insertion modes, foster parenting, bounded/source-backed
  pending table text; release scaling and common-path comparisons.
- [x] Static SVG/MathML document dispatch, integration points, CDATA,
  spelling adjustments and compact XLink/XML/XMLNS attribute identity.
- [x] Static template document modes and compact owned content fragments;
  cloning, reclamation, cycle/depth checks and EOF unwinding.
- [x] Standalone inert context fragments with table/foreign/template/raw
  contexts, immutable metadata snapshots, form/quirks inheritance and
  cancellation. 3,898/3,904 tree variants match; six need script execution.
- [ ] Cross-document import/adoption, live fragment insertion hooks and
  Web IDL innerHTML/setHTML APIs.
- [ ] Declarative shadow roots, template content patching, inert owner-document/
  realm semantics and web-facing template APIs.
- [ ] Add network/encoding/cache/cookies with security and memory bounds.
- [x] Original CSS tokenizer, compact component arena and generic rule/
  declaration/block construction; independent syntax corpus, reviewed
  current-spec updates, native fail-closed evaluator and release comparisons.
- [ ] CSS byte decoding and descriptor-aware unicode-range handling;
  28 imported byte fixtures remain explicitly unsupported.
- [ ] Implement CSS selector/property grammars, cascade, media queries,
  custom properties and style invalidation.
- [ ] Implement block/inline, text shaping/bidi, flex/grid/table layout,
  positioning, fragmentation and incremental layout.
- [ ] Retained GPU painting, clipping, compositing, scrolling, images,
  SVG, canvas, fonts, video and audio; measure each on release builds.
- [ ] Native browser shell, tabs/navigation/find/history/bookmarks/downloads.
- [ ] Structured DOM snapshots and versioned agent actions; input/focus/IME.
- [ ] V8 integration, Web IDL bindings, GC rooting, event loop, modules,
  timers, promises, Fetch, workers, storage and required web APIs.
- [ ] Dark/night page adaptation and author color-scheme support.
- [ ] Tracker/file panels/printing/clipboard/notifications/permissions.
- [ ] Passwords autofill/save and passkeys with origin-bound mediation.
- [ ] Isolated/sandboxed renderer processes; cancellation and crash recovery.
- [ ] Restore windows/tabs on app_server restart and in-place upgrade.
- [ ] WPT integration and hostile-input/fuzz coverage; independent DOM,
  layout and visual differentials.
- [ ] Capture pinned Chrome baselines and execute every corpus journey;
  all scales/themes and scale changes; no missing/blocked case counted passed.
- [ ] Profile ownership preflight costs, the template-milestone fostered-row
  regression and fragment-milestone foreign dispatch costs (raw release
  comparisons in JOURNAL.md); performance acceptance is still open.
- [ ] Release performance comparisons, memory/idle/latency/scroll/GPU;
  numbers and raw evidence in journal and commit messages.
- [ ] Full workspace release build; layout lint; headless reviews and visual
  journal; reload testing; commit and push each working milestone.
- [ ] Demonstrate >=99% compatibility on a defined independent sample,
  with sample size, uncertainty, scope and exclusions reported.

Next: CSS byte decoding, selector/property/style compilation and byte/stream
input handling; complete tokenizer diagnostics, arbitrary namespaces, Web IDL/live DOM semantics
and the six tree cases requiring script execution.
Evaluation capture/replay and executable site journeys remain open.
