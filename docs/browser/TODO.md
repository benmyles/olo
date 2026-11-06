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
- [x] Core typed declaration compilation: 92 longhands and 16 shorthands (including
  all over implemented resettable properties),
  dimensional/range rules, structured display values, shared pending shorthand
  substitution and invalid-at-computed-value unset; independent native ports
  821/1,109 pass, 288 explicit non-passes. Separate initial style jobs below;
  no layout execution.
- [x] Core frozen stylesheet sets/cascade over typed declarations: shared
  selector arenas and rule index, stylesheet-local namespaces, nested/anonymous/
  ordered layers, origin/importance/inline/specificity/proximity/source sorting,
  defaulting/revert-family traversal, guarded custom rollback and shared pending
  expansion. Native ports 128/136 pass; eight rendering drivers remain unported.
- [ ] Complete cascade/style integration: media/supports/container/import/scope
  conditions, actual flattened
  inheritance, UA defaults/presentational hints, real font selection/size hosts,
  CSSOM, animations/transitions/registrations, live state and generation-aware
  invalidation/global style sharing. Frozen candidate snapshots are not computed
  styles; the separate bounded stage below now computes supported values.
- [x] Project the 40 implemented physical/logical box/overflow longhands into
  paired cascades after computed axes; preserve inheritance source across axes,
  shared pending substitution, physical precedence for all and paired rollback.
  Native logical ports: 5,726/5,757 pass; 31 CSSOM drivers remain unsupported.
- [x] Initial computed-style worker jobs over the supported 92 longhands:
  shared Defaults, parent-before-child connected light-tree traversal, frozen
  font/viewport/tree arithmetic with deferred layout percentages, inherited
  currentcolor/custom tokens, coupled overflow/border/display transforms,
  sparse and adjacent shared values, diagnostics, cancellation/quotas and
  document/style-epoch rejection. 243/341 pinned native assertions/drivers pass;
  98 unsupported. All 22 selected local Chrome CSSOM observations match.
- [x] Font-family/style/width longhand parsing and computed face requests:
  named/generic fallback identity, legacy font-stretch alias, bounded oblique
  angles and percentage widths before subject metrics. 93/149 pinned native
  assertions/drivers pass; 56 unsupported. Match 17 selected local Chrome
  observations; six reference-feature gaps remain explicit.
- [x] Initial native TTF/OTF/TTC source catalog, original width/style/weight
  matching, Unicode full family-name folding, variable-axis coordinates,
  synthesis requests, first/reference fallback metrics, bounded worker-local
  metric caches and explicit language/range generic policy. Native table/metric
  work is a prerequisite; it does not render text.
- [ ] Complete font selection: OS discovery/change watching, web fonts and
  WOFF/WOFF2 validation/sanitization/isolation, unicode-range, full cluster/script/
  variation-selector fallback, font-style left/right, optical policy/rounding
  parity, synthesized drawing, font shorthand/remaining descriptors, shaping/bidi
  and automatic language/script-sensitive generic policy. Composite/CFF missing
  height inference currently uses estimates; full-font hostile-input CPU bounds
  and cancellation inside third-party table operations remain.
- [ ] Complete logical border/corner properties and remaining property groups;
  UA styles/presentational hints, flattened/shadow inheritance, conditional
  contexts, font-family/selection and layout/used-value hosts.
- [ ] Profile temporary per-element style vectors, nonadjacent sharing and
  real mixed pages; initial full passes are not live incremental invalidation.
- [x] Scope selected metric reuse by explicit immutable subject/language/font
  context; reject context changes during publication and root Defaults mismatch.
- [x] Bounded per-job selected Font request interning with complete request/
  subject-context keys, equal separate family contents, saturation controls,
  scratch accounting and release measurements. Generated 10k repeating requests
  now retain 301 Font objects (38,528 B), vs 10,001 (1,280,128 B).
- [ ] Complete automatic script/cluster contexts before globally sharing
  selected fonts. Profile unique-request cache overhead (about 6–9% in current
  synthetic controls), slot storage and mixed-load worker latency. Supplied
  metrics are not real font-selection evidence; the separate Noto job is.
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
- [ ] Isolate raw-query/common-operation timing variation after logical/style
  work: original margin median 8.256 → 8.670 ns (+5.0%); current three-way
  repeat 8.589 / 8.605 / 8.978 ns (pre-logical / parent / style), overlapping
  ranges. Other mixed changes include 2k cascade +8.4% and #target selector
  +20.2% vs parent in this sample. Raw results retained; no universal speedup
  or settled performance acceptance. See 2026-11-05-style.json and journal.
- [ ] Profile ownership preflight costs, the template-milestone fostered-row
  regression and fragment-milestone foreign dispatch costs (raw release
  comparisons in JOURNAL.md); performance acceptance is still open.
- [ ] Profile owned cascade value copies after font property expansion. Final
  raw width 8.964 → 11.153 ns (+24.4%) and margin 8.450 → 11.168 ns (+32.2%)
  vs f1513b47, with nonoverlapping ranges. Known-type copies improve over the
  general-clone control (15.347 / 15.152 ns); full performance acceptance is
  open. Consider borrowed values for the actual style pipeline rather than
  repeatedly copying frozen typed values; see 2026-11-05-font.json.
- [ ] Release performance comparisons, memory/idle/latency/scroll/GPU;
  numbers and raw evidence in journal and commit messages.
- [ ] Full workspace release build; layout lint; headless reviews and visual
  journal; reload testing; commit and push each working milestone.
- [ ] Demonstrate >=99% compatibility on a defined independent sample,
  with sample size, uncertainty, scope and exclusions reported.

Next: UA defaults/presentational hints and actual font selection/metrics,
logical borders/corners;
conditional stylesheet contexts and actual live element/style invalidation;
stylesheet contexts, property coverage and computed-value hosts,
remaining selector integrations, descriptor-aware parsing and
HTML encoding sniffing/byte/stream input handling; complete tokenizer diagnostics, arbitrary namespaces, Web IDL/live DOM semantics
and the six tree cases requiring script execution.
Evaluation capture/replay and executable site journeys remain open.

Next: shaping/bidi and original inline/block layout. Initial native font
catalogs/matching/metrics and bounded job-scoped request sharing are validated
prerequisites. Whole-font sanitization, web fonts, automatic generic policy,
full font APIs and rendering remain open; website acceptance is still zero.
