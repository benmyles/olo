# Browser TODO

Keep this file and JOURNAL.md current with each milestone. A checked
item means implemented and verified; a partial implementation stays open.

While working, fetch and merge `origin/main` at regular working checkpoints,
revalidate the merged workspace and push the integration with browser work.

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
- [ ] Complete network/encoding/cache/cookies with security and memory bounds.
  Initial in-process HTTP/HTTPS, bounded decoded bodies, redirects, native TLS
  roots, wakeable cancellation, HTML encoding prescan and linked CSS now work.
  Document base/order, CSS MIME/encoding, local-file and mixed-content boundaries
  have regressions; compiled sheets survive resize/theme/scale changes.
  Streaming HTML parse, HTTP caches/cookies/referrer/origin policy, parallel
  prioritised resources, imports/media/SRI, downloads and renderer isolation remain.
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
- [x] Initial native font-unit glyph shaping, current Unicode 18 bidi properties
  and original extended-grapheme segmentation, uniform-request paragraph
  itemization/cmap fallback, retained source clusters and worker delivery.
  Match 24 native HarfBuzz 15 vectors plus 15 benchmark hashes; all 91,707
  BidiCharacterTest cases through L2 and 853 GraphemeBreakTest cases pass.
- [x] Initial CSS phase-I whitespace preparation over exact source/style-span
  partitions; inherited collapse/wrap longhands and two-longhand shorthand;
  cancellable worker scans, unchanged text sharing, 12-byte encoded maps and
  binary byte lookups. 3,925 independent regex references and 52 pinned WPT
  acceptance ports pass; no CSSOM/layout/website acceptance implied.
- [x] Initial retained horizontal text geometry for uniform font request/size
  and whitespace: safe shared glyph slices, unsafe-edge reshaping, width fitting,
  SP trimming/hanging, LF/blank lines, bidi fragments and alignment/style binding.
  216 independent HarfBuzz layouts at two slice budgets; nine regression groups;
  measured release resize and whole-fragment-reshape control. This is a library
  stage, not complete CSS Inline or page rendering.
- [ ] Profile the remaining common Latin paragraph variation after layout
  metadata/LF work: final cached ~4 KiB parent/current medians 246.383→273.308
  µs; reverse-order pair 242.303/242.151 µs and whole-process CPU equal.
  Fast control validation was restored; these data do not establish a fix or
  identify the cause. Preserve both measurement sets and scheduler counters.
- [ ] Complete text integration: phase-II whitespace trimming/tab stops/hanging,
  soft-wrap opportunities, styled shaping/spacing, markup
  bidi contexts, script-extension/paired-punctuation itemization, script/GSUB
  suitability and multi-font mark fallback, vertical paragraph policy,
  line breaking, safe boundary reshaping, hit testing and rasterization. Initial
  shaping rejects unprocessed controls/oversized uniform spans. Font/shaper
  opaque cache accounting, adversarial CPU bounds and cancellation/isolation
  remain required before web fonts; source budgets are not total RSS limits.
- [x] Original Unicode 18 default line-break opportunities: all 19,346
  LineBreakTest strings, every boundary/kind at scalar and 4 KiB budgets;
  constant-size lookahead/context, packed internal offsets, implicit EOT,
  streaming/cancellation/quotas and preparation source sharing. This is UAX14
  default behavior, not CSS/language tailoring, line fitting or page layout.
- [ ] CSS break/nowrap/preserve-space policy, grapheme-safe emergency breaks,
  language/strictness tailoring and complex-context segmentation/dictionaries.
  Raw SA counts remain visible; default AL treatment is not Thai/Lao/Khmer/
  Myanmar browser segmentation. Width selection, safe reshaping and painting
  are still required before text can render in the original engine.
- [ ] Profile residual line-break dispatch/code-layout costs and outliers:
  final default/no-bulk 36 KiB Latin 315.953/305.090 µs, 53 KiB Arabic
  404.457/392.465, 530 KiB combining 2,404.206/2,211.133; combining max-step
  median 661 µs. Bulk wins the 74 KiB word (197.457 vs 565.826 µs), with
  mixed other cases. Keep initial/guard/final raw release evidence rather
  than treating a control improvement as universal browser acceptance.
- [ ] Resolve whitespace reference differences with actual native line/raster
  evidence: Chrome 157 local projections have 45 identical, 12 different and
  eight reference-unsupported cases. Four differences need Chrome-compatible
  ZWSP segment-break removal; five concern transparent bidi controls, three
  CR plaintext retention. Phase-I strings and innerText are different stages;
  no combined compatibility percentage or hidden passing exclusion.
- [ ] Profile common-operation costs after whitespace registry expansion:
  parent/current 10k uniform/varying style build 21.243→22.701 / 29.160→31.290
  ms; max slice medians 612→7,671 / 774→10,589 µs. Raw width/margin-left
  10.344→11.127 / 10.608→11.288 ns. Shaping CPU is unchanged in each paired
  process (.44/.46/.44 s), with more involuntary switches after; this does not
  establish the cause of individual wall-clock regressions. Preserve raw
  evidence in measurements/2026-11-05-whitespace.json; no universal speedup.
- [x] Initial DOM-to-flow boxes and block/inline-content geometry: source/style
  identity, display:none/contents, block-in-inline text splits, width/min/max/
  percentage/box-sizing/auto margins, margin collapse and relative blocks;
  139 independent Chrome DOM-rectangle fixtures at three scales (417 observations),
  source-safe shared paragraphs and cancellation/quotas. This is partial normal
  flow, with explicit unsupported formatting; those geometry checks provide
  no pixel or website acceptance.
- [ ] Profile compact geometry and registry costs: current box/geometry records
  are 56/128 B. Source-safe paragraph sharing reduces repeated glyph/index
  payload by 1000× in repeated-text fixtures, but unique tree+prepare median
  5,186.677 vs 5,057.163 µs without the registry. Common varying-style wall
  build 28.314→29.642 ms and max-slice median 863→9,428 µs remain open with
  similar process CPU. See 2026-11-06-flow raw measurements; no universal win.
- [ ] Implement complete block/inline, flex/grid/table layout,
  positioning, fragmentation and incremental layout.
- [x] Initial horizontal inline margins/padding, percentage and auto contexts,
  nested/empty spans, slice ends across wrapping/block interruptions, and
  shaping isolation at actual used inline edges. Transparent vertical padding
  leaves glyph placement and line height unchanged, including empty spans.
- [ ] Complete inline fragment backgrounds/borders/hit geometry and bidi edge
  distribution. Empty/nested inline backgrounds must fail explicitly until
  painted. Independent spacing probes: 198/225 case/scale observations match
  the documented DOM Range allowance; 27 fractional-font X non-passes remain.
- [x] Initial retained static page scenes, outline cache, indexed tile queries,
  bounded pure-Rust CPU raster and native GPU tile composition. Ninety actual
  native-scale Chrome pixel observations are captured; 54 exact matches and
  36 remaining font/corner/colour failures. Earlier emulated-DPR captures are
  preserved separately, with the native/emulated border-snap mismatch explained.
- [x] Initial native single-page shell: local HTML/about page, worker progress,
  Stop/Escape, address shortcuts, retained scrolling, readable agent text/status,
  forwarded files and kit restart restoration. Fifteen live scale/theme reviews
  and 1280×800 native layout lint pass. This does not finish browser interaction.
- [ ] Profile native tile upload/cache size and browser memory (initial 400-row
  window RSS ~90,900 KiB); reduce touchpad first-movement latency (~29 ms),
  the observed drag hitch and half-pixel wheel discontinuities. Release
  observations are in 2026-11-06-paint-ui.json, not universal acceptance.
- [x] Device-snapped sharp rectangles/clips, device text baselines and
  disjoint solid border strips with premultiplied corner mixing; tiled/whole
  equality including empty clips and translucent asymmetric corners.
- [ ] Match native font masks, GPU/asymmetric corner coverage and retained
  colour precision (36/90 exact native observations still fail); arbitrary-font
  hinting/raster parity and inline backgrounds/ligature
  colour clipping. Complete CSS paint/stacking and semantic DOM/hit actions.
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

Next: resolve the remaining native font/corner/colour pixel failures and initial shell input/links;
complete mixed
inline typography, fractional device shaping/line metrics, visual-edge
whitespace/other-Zs hanging, tab stops, CSS tailoring, UA defaults, clipping,
scrolling and incremental box reuse. Initial native font/shaping
work is verified at glyph/Unicode/library scope; no website acceptance follows.
Web fonts/sanitization, complete CSS/live invalidation, networking/V8, the native
complete shell, universal dark mode and agent/Olo/password/passkey integration remain.

Latest-main integration (4a680075): Browser's 15 reviews and live lint pass,
but the full 1280×800 desktop lint reports ten findings outside Browser:
Disk Usage crowding (six), Passwords setup overflow (three), Integrations
footer/scroll-control grouping (one). Keep these visible for native integration
acceptance; raw reports are in 2026-11-06-paint-main-integration.json.

Latency method correction: the legacy ~29 ms touchpad figure starts before
the first report and can miss the first moving frame without a baseline.
The timestamped replay retains both and reports input-to-client-frame marks
separately. Full compositor presentation/optical input latency is still open;
this is a harness correction, not a browser speedup.

- [ ] Avoid outline extraction for fully invisible/transparent glyph runs
  after validating shared-cluster paint boundaries; omit fully transparent
  border paint commands while retaining box geometry. Measure the hidden-text
  path as well as the normal visible path.

- [ ] Public navigation smoke failures: Example Domain is visibly incomplete;
  Hacker News gets past inline spacing/padding and now fails on replaced/
  control/forced-break inline objects. Complete those objects, inline
  decorations/typography and table layout; preserve explicit failures in evals.
- [x] Distinguish authored document records from native widget layout review.
  `agent::Node.document_content` keeps snapshots/actions while excluding native
  spacing/inset rules; the shared regression still catches native crowding.
- [ ] Investigate the small-page pipeline sample regression: 100 elements
  948.099→1016.547 µs median; 1k 9183.913→9217.438 µs. Retaining compiled rules
  enables I/O-free restyle; samples do not establish a speedup or full cost acceptance.

- [ ] Complete document input/hit testing: initial visible text links and
  hover/active/focus ancestry now work through native/agent pointer input.
  Box/advance/whitespace/empty-anchor/image targets, pointer capture, cursor
  styling, complete keyboard/focus semantics, selection/editing and default-action
  event dispatch remain required. Cross-node shared ligatures deliberately
  have no invented click target until caret ownership is available.
- [ ] Incremental selector/style/layout/paint invalidation. Dynamic state masks
  avoid restyle when unused, but actual state changes still restyle the full
  retained document and rebuild visible tiles. This is not the final cost model.
- [x] Initial bounded Back/Forward history, canonical redirects, same-document
  fragments/:target and CSS-pixel scroll restoration, including native re-exec.
- [x] Initial Tab/Shift+Tab/Enter text-link control through the kit's focus
  actions and documented agent menu commands; worker-built ordered index.
- [ ] Complete navigation history/tabs/downloads/new windows and allowed URL
  handlers, frame/script histories, BFCache, focus starting points and box/image
  anchor targets. Unsupported link actions report a reason without requests.
- [x] Share URL/text bytes in per-frame records; immediately repaint pointer
  focus metadata even without focus-dependent CSS and clear it on native blur.
- [ ] Bound structured metadata work for pathological overlapping content and
  bound history URL bytes as well as entry count.
- [ ] Investigate the two candidate drag hitches from the first link-input
  sample; paired confirmation is clear. Preserve both runs and measure actual
  hover-to-presentation latency before performance acceptance.
- [ ] Reduce the new 1,001-link keyboard case's 17.711 ms median to the
  frame target; full document restyle/layout/raster is still conservative.
  Keep the merged-desktop sample's drag/flick hitches visible (baseline 2/1,
  candidate 1/0) and control external build load before attributing them.
