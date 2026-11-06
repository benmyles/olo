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
- [x] Bounded/cancellable streaming text and CSS byte decoding, encoding
  provenance/precedence and source-buffer retention; all 28 imported byte
  fixtures match, 165 direct plus 12 reviewed current-spec syntax cases.
- [ ] Descriptor-aware unicode-range retokenization and context grammars;
  byte decoding does not make CSS tokenization incremental.
- [x] Original bounded selector compilation/matching against native DOM and
  borrowed trees; compact arenas, specificity, namespaces, logical/structural
  selectors, filtered sibling indices, public history masking and host-state
  hooks. 432/463 independent cases match; two modern grammar expectations,
  29 explicit non-passes; 128 independent An+B cases checked.
- [ ] Complete selector integration: live state, nesting, shadow/functional
  pseudo-elements, remaining fixture contexts, full standards behavior and
  document/style invalidation. Core rule indexing and stylesheet-local
  namespace contexts are implemented below; full context behavior remains open.
- [ ] Implement CSS property grammars, cascade, media queries and real
  element/style invalidation.
- [x] Core typed declaration compilation: 89 longhands and 16 shorthands (including
  all over implemented resettable properties),
  dimensional/range rules, structured display values, shared pending shorthand
  substitution and invalid-at-computed-value unset; independent native ports
  821/1,109 pass, 288 explicit non-passes. No actual style/layout execution.
- [x] Core frozen stylesheet sets/cascade over typed declarations: shared
  selector arenas and rule index, stylesheet-local namespaces, nested/anonymous/
  ordered layers, origin/importance/inline/specificity/proximity/source sorting,
  defaulting/revert-family traversal, guarded custom rollback and shared pending
  expansion. Native ports 128/136 pass; eight rendering drivers remain unported.
- [ ] Complete cascade/style integration: media/supports/container/import/scope
  conditions, actual flattened
  inheritance, UA defaults/presentational hints, computed font/color/size hosts,
  CSSOM, animations/transitions/registrations, live state and generation-aware
  invalidation/style sharing. Frozen candidate snapshots are not computed styles.
- [x] Project the 40 implemented physical/logical box/overflow longhands into
  paired cascades after computed axes; preserve inheritance source across axes,
  shared pending substitution, physical precedence for all and paired rollback.
  Native logical ports: 5,726/5,757 pass; 31 CSSOM drivers remain unsupported.
- [ ] Complete logical border/corner properties and remaining property groups;
  coupled overflow transforms, actual computed-value retention and layout hosts.
- [ ] Complete property registry/grammars (fonts/families, backgrounds/images,
  border-image/radius, grid/alignment, transforms, animations/transitions,
  aliases/descriptors), CSSOM, computed/used-value contexts and style hosts.
- [x] Core unregistered custom-property token compilation/substitution:
  compact batch arenas, dynamic names/argument wrappers, demand-driven cycles,
  revision-aware memoization, taint/keyword hooks and bounded expansion;
  119/151 native independent cases pass, 32 explicit non-passes.
- [ ] Complete custom-property integration: registered properties, actual
  cascade/inheritance/animation hosts, early invocation/spread, remaining
  arbitrary substitution functions, attributes/URL taint and CSSOM. Token
  substitution alone does not implement property validation or style.
- [x] Core typed numeric compilation/resolution: dimension algebra, flat
  programs, bounded work/stack/depth, host-unit/tree dependencies, constant
  simplification and independent numeric ports (1,572/1,586; 14 non-passes).
- [x] Integrate typed static/contextual math with color channels, shared
  budgets, live host unit/tree hooks and conservative invalidation metadata.
- [ ] Complete remaining math contexts, substitution, CSSOM and actual
  property/style/layout hosts; dependency metadata does not implement them.
- [x] Original typed absolute CSS colors, deferred current/system/light-dark
  context, missing components, explicit colorimetric conversion, bounded
  allocation-free SIMD worker and measured compact storage. 12,703 native
  cases: 9,921 direct plus 2,760 reviewed current-spec passes; 22 non-passes.
- [ ] Complete color substitution, relative colors/mixing/interpolation,
  gamut mapping/profiles, CSSOM, author color-scheme and Olo palette/style/
  rendering integration. Typed color support is not all-site dark mode.
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
- [ ] Profile compound/common subject-key selectivity beyond the selective
  cascade benchmark; the necessary-key index is conservative, not universally
  optimal.
- [ ] Isolate raw-margin query timing variation after logical projection: median
  8.256 → 8.670 ns (+5.0%) with overlapping three-run ranges; no uniform
  speedup claim. Retain short/long raw runs and compare in future style workloads.
- [ ] Profile ownership preflight costs, the template-milestone fostered-row
  regression and fragment-milestone foreign dispatch costs (raw release
  comparisons in JOURNAL.md); performance acceptance is still open.
- [ ] Release performance comparisons, memory/idle/latency/scroll/GPU;
  numbers and raw evidence in journal and commit messages.
- [ ] Full workspace release build; layout lint; headless reviews and visual
  journal; reload testing; commit and push each working milestone.
- [ ] Demonstrate >=99% compatibility on a defined independent sample,
  with sample size, uncertainty, scope and exclusions reported.

Next: computed/inherited style and UA defaults, logical borders/corners;
conditional stylesheet contexts and actual live element/style invalidation;
stylesheet contexts, property coverage and computed-value hosts,
remaining selector integrations, descriptor-aware parsing and
HTML encoding sniffing/byte/stream input handling; complete tokenizer diagnostics, arbitrary namespaces, Web IDL/live DOM semantics
and the six tree cases requiring script execution.
Evaluation capture/replay and executable site journeys remain open.
