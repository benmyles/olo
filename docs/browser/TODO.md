# Browser TODO

Keep this file and JOURNAL.md current with each milestone. A checked
item means implemented and verified; a partial implementation stays open.

While working, fetch and merge `origin/main` at regular working checkpoints,
revalidate the merged workspace and push the integration with browser work.

**Active websites: [Stripe](eval/stripe/README.md) (homepage and
pricing page as Chrome, live) and Wikipedia (the main page up to the
search box and `overflow-wrap`; the Stripe, Inc. article with its
scripts, as Chrome line for line, JOURNAL 2026-11-08); GitHub's
homepage (its hero as Chrome since JOURNAL 2026-11-07, late; open items
below).**
Finish its real pages/journeys and all 30 configurations before choosing
another site. The engine renders every page end to end and records what it
substituted in `Page::fallbacks` (see JOURNAL, 2026-11-06 evening); work is
chosen by what is visibly wrong against Chrome and by the fallback counts,
not by the first refusal. Stripe's homepage at 1000 × 700 lays out as
Chrome does in every static section (JOURNAL 2026-11-07); what differs is
script-driven. No configuration is accepted yet.

GitHub's homepage, open (seen against Chrome at 1000 × 700):

- [ ] Container queries (`@container`, `container:`): its sign-up form
  stays stacked (83 rules dropped).
- [x] Flex `order`, row and column (its header's menu button sat left).
- [ ] CSS animations and transitions: `animation`, `@keyframes` (207
  rules), `transition` are not parsed; also `outline`,
  `appearance`, `filter`, `backdrop-filter`
  (`text-transform` and `text-box` done).
- [ ] Load cost: its first 12 s cost Olo about 2.4 CPU s and Chrome about
  1.3-1.9 (scripts 1.2 s, the scripts' first full style 45 ms each time
  the sheets change, WebGL's EGL start 135 ms on the page thread).

Nothing blocks a page's event loop (JOURNAL 2026-11-08, night), open:

- [ ] GitHub against Chrome (each alone, 1000 × 700): load 9.01 vs 9.10
  CPU s, idle 27% vs 37%, scrolling 54% vs 61% (2026-11-08). Element
  insertions (its animated demo panel) still rebuild the whole box tree
  (2-4.5 ms); its animation's style patch is 0.3-0.7 ms a frame for 9
  elements, most of the page thread while idle.
- [x] MDN's load against Chrome: 0.95 vs 1.15 CPU s (2026-11-08: its
  pseudo-elements looked for from the rule index, its components' new
  styles restyling only their trees, ids and names from an index: the
  prelude was 475 ms of it). Left to make faster still: the prelude's
  16-23 ms each page (a V8 startup snapshot would make it nothing),
  matching 21-27 ms per restyle from nothing, layout 8-10 ms per pass.
- [ ] Reddit against Chrome, quiet, its feed working: load 3.18 vs 2.63
  CPU s, idle 0.9% vs 3.7%, scrolling 123% vs 60% (2026-11-08; since:
  SipHash, 14% of the page thread, gone; blurred backdrops 2.35 -> 0.67 ms
  a tile; its feed differs each load, ±25% between runs). While
  scrolling: its scripts ~1 s in 6 (parsing each 528 KB next page through
  a template, serializing and parsing it again), style 770 ms (each page's
  2,000 elements, 73 candidate rules each with its Tailwind classes; a
  document sheet changed restyles from nothing: shadow trees' changes
  keep styles, the document's don't yet), boxes 260 ms, layout 250 ms.
- [ ] A script-less page with CSS animations takes a realm for its clock
  (the prelude's 16-23 ms once): a clock of its own would be lighter. A V8
  startup snapshot of the prelude would make every page's 16-23 ms nothing.
- [ ] A translucent, transformed or filtered atomic inline paints in its
  line's place, not raised to its stacking context's later layers as
  CSS 2 Appendix E has it (the same unless something overlaps it); its
  z-index is ignored.
- [ ] Scaled pictures through the upright sampler: 0.28 ms a draw against
  tiny-skia's 0.16 (it blends two mip levels, as Chrome's results show it
  should); SIMD over the row, or one level when they are near, would
  close it.
- [ ] `getComputedStyle` answers no `animation-name` (a page probing which
  elements animate finds none).
- [ ] Decoded pictures: kept whole while bound (a page's budget 256 MB,
  the latest kept past it). Decode at the size shown, let pictures far
  out of view go and decode them again when they come back (Chrome's
  discardable decodes): Reddit's long feed sits at 1.0-1.5 GB.
- [x] Reddit: its loading indicator (between pages): its ring, an absolute
  child of a centring flex box, sat at the start (2026-11-08: placed as
  the sole flex item would be); its Snoo draws as Chrome's.
- [ ] Bot challenges keep whole sites out (2026-11-08 sweep): Stack
  Overflow stops at Cloudflare's "Performing security verification", The
  New York Times at DataDome's device check (in an iframe, whose scripts
  don't run yet: a blank page), Amazon at its AWS WAF. Chrome is let
  through. Needs iframes' scripts, and requests less unlike Chrome's
  (TLS and header order) to be flagged less often.
- [x] YouTube: its home as Chrome's, icons within a second (2026-11-08:
  idle callbacks took their timeout as a delay; Cache Storage kept nothing).
- [ ] Selectors are compiled again with every compile of a page's sheets
  (11 ms of GitHub's 17): keep each sheet's compiled selectors with it,
  as its declarations are, relocated into the rule set's arenas.
- [x] A page's tiles were painted one after another on its thread: up to
  16 at a time are painted side by side on the CPU pool now (each pool
  thread a painter of its own), shown in order.
- [x] A page's first module graph was fetched with a thread per module
  waiting on the network (`modules::fetch_all`): they start as tasks on
  the network runtime (the page waits for the graph either way).
- [ ] `crypto.subtle.digest` and HMAC compute on the page's thread
  (their promises are settled at once): on the CPU pool for large inputs.

apple.com and bbc.com (JOURNAL 2026-11-07, late), open:

- [x] CSS animations (`@keyframes`, `animation`): apple.com's ribbon, the
  headings GitHub fades in. Values interpolate from their text (calc,
  sRGB, functions argument by argument); `var()` values switch at the
  midpoint. Transitions too (`transition`, events; JOURNAL 2026-11-07,
  late night). Open: transitions of shorthands' parts named by the
  shorthand alone, `transition-behavior: allow-discrete`, animations on pages
  without scripts (no live clock: they stay at their start),
  `getComputedStyle` of animated values, `animation-composition`.
- [x] Video (`<video>`): played by Olo's own player (`olo_preview::video`:
  H.264, HEVC, AV1, VP9 in MP4 or Matroska), each picture made the size
  it shows at and bound to its element like a drawn canvas; scripts get
  HTMLMediaElement (play, pause, currentTime, readyState and its
  constants, events). GitHub's hero film plays (HEVC); a video shown as a
  surface layer goes to the window as its decoded planes, made RGB,
  scaled and rounded by the GPU (JOURNAL 2026-11-08). Open, in order:
  animate opacity and transform without a style patch each display
  frame (0.3 ms a render on GitHub now; Chrome's compositor does it off
  its main thread); a video under a real mask or an opacity group is
  still painted with the page each picture; a page paint `Invalid` after a script's change on
  GitHub (now healed by a whole render, logged); pictures decoded on the GPU and shown as its planes
  (now the processor, the `video` thread 40-50% of a core for 2560 x
  1726); sound (muted only); `poster`; `<source>` media queries;
  streaming (the file is fetched whole, 256 MB at most).
- [x] Container queries (`@container [name] <condition>`, nested,
  `container-name`, range syntax, `inline-size`/`block-size`): GitHub's
  sign-up form on one line, as Chrome. Decided as elements match, against
  the nearest container of that name with the sizes the last layout gave;
  a pass whose containers come out another width restyles what they hold
  (incremental) and lays out again. Open: `style()` and `scroll-state()`
  queries, the scripts' measurements (they don't apply them), container
  units against the container (they read the viewport).
- [ ] Cost against Chrome (`compare-cost.py`, each browser alone; JOURNAL
  2026-11-08): bbc.com Olo better in all four (scroll 23% to 129%);
  github.com idle 29% to 36%, scroll level, memory level; apple.com
  scroll 34-38% to 25-37%, load 1.8-2.2 to 1.2 CPU s (open: its load;
  scrolling's scene rebuilds and tile painting on the processor, Chrome
  rasterises on the GPU). Inline elements' `opacity` paints now (glyphs,
  decorations and inline backgrounds at the product of their inline
  elements' opacities; where an element overlaps itself a group would
  differ). apple.com's Upgrade video plays (`load()` honoured). Open:
  apple.com's "Hello, hello." word swap (scripted
  transform and opacity on inline-blocks) ends with both words showing,
  shifted.
- [ ] GitHub's "More ways" cards stay pinned (sticky, stacked) while the
  page scrolls in Chrome; in Olo they scroll by.
- [x] `background-clip: text` (apple.com's gradient subhead), on block
  boxes. Open: on an inline box, and comma-separated `background-clip`
  lists.
- [x] Shadow DOM: the composed tree renders (shadow content in the host's
  place, slots taking their children), its sheets scoped (`<style>`,
  `<link>`, `adoptedStyleSheets`; `:host`, `:host()`, `::slotted()`; the
  outer tree wins for normal declarations), `::part()` (an outer tree's
  rules win over the shadow tree's own). Shoelace's docs as Chrome
  (JOURNAL 2026-11-08). Open: `:host-context()`, `::part()` reaching
  through `exportparts`, `revert` across trees (Unsupported). Declarative
  shadow roots (`<template shadowrootmode>`) parse, render and reach
  scripts.
- [x] GitHub's repository page: the file list's commits (IndexedDB
  requests now fail as events, so its query cache runs), its columns
  (`table-layout: fixed`), its row lines (cells fill their row),
  `text-overflow: ellipsis`, the README's dark logo.
- [x] apple.com scrolled through: lazy pictures load (images a script
  inserts, `<picture>` sources changed, `noscript.innerHTML` as text),
  no touch handlers (it served the tablet page), inline-level absolute
  boxes at their place in the line, SVGs with only a viewBox fill the
  width, a moved layer clipped on one axis shows. `backdrop-filter` on
  fixed and sticky boxes blurs on the GPU (JOURNAL 2026-11-08; open: on
  boxes painted with the page, filter functions other than blur and
  saturate). Open: a space collapsed across an absolute box in a line (2 px),
  its iPhone Duo subhead animation, inline backgrounds.
- [x] Lazy images' timer loop: web-vitals' capturing window `load`
  listener got every image's load event (load events don't reach the
  window now). Open: `loading=lazy` images still never load.
- [x] Scroll containers (`overflow: auto | scroll`; `hidden` by script):
  the wheel (Shift: sideways) scrolls the innermost one with room that
  way, `scrollTop`/`scrollLeft`/`scrollTo`/`scrollBy` and `scroll`
  events, their range as Chrome's (margin boxes and end padding; fixed
  boxes and those positioned outside count for nothing and stay put),
  hit tests follow, agents see each as a ScrollArea. Open: scrollbars
  (Chrome's take 15 px of the box's width; none drawn), `scrollIntoView`,
  `scroll-snap`, smooth scrolling, a scrolled box's `getBoundingClientRect`
  children (they read unscrolled), keyboard scrolling of a box.
- [x] GitHub's menu: opens, rows clickable, Products expands, its panel
  scrolls, as Chrome (JOURNAL 2026-11-07, night).
- [ ] Forms: a placeholder whose `::placeholder` opacity changes on focus
  (GitHub's shows on focus in Chrome); going back to a posted page asks
  nothing and loads it with GET (Chrome asks to send it again); a file
  input. Done: POST submission (urlencoded, multipart, text/plain; with
  the page's Origin and Referer); forms on pages without scripts (a realm
  is made for their controls); JSON and other text documents shown as
  text, as Chrome. Done earlier: agents see fields, buttons, checkboxes and selects
  (role, name from `aria-label`, `<label>`, placeholder; value; checked;
  focused as the page has it); the page's focus is what `:focus` and
  `:focus-within` match (GitHub's label floats); a focus change restyles
  the page once (56 ms on GitHub: every element matched again; narrow it
  to what the states reach).

Next, in impact order (each removes a fallback kind or a visible gap):

- [x] Stacking order: stacking contexts and z-index in CSS paint order.
- [x] `@media` and `@supports` (every site rendered its narrow layout before).
- [x] Grid formatting (track sizing, auto-placement, lines and spans, gaps,
  alignment; areas, dense packing and column flow remain), column flex,
  atomic inlines (inline-block, inline-flex, inline-grid) laid out by
  fit-content inside their paragraph.
- [x] Web fonts: `@font-face` descriptors, WOFF2/WOFF decoding in Rust,
  `unicode-range`, per-page font scopes; Stripe renders in Sohne.
  Remaining: `local()` sources, `font-display` timing (fonts load before
  first paint), system font discovery (the catalog holds Noto Sans only).
- [x] Styled inline runs: a paragraph shaped from several fonts/sizes;
  `font-feature-settings` per run; synthetic oblique.
- [x] Tables (auto layout, spans, border-spacing), the HTML UA stylesheet,
  presentational hints, legacy centring; letter-spacing; opacity groups.
- [x] Flex wrapping (lines, align-content), aspect-ratio, min-height
  containers aligning their tracks, Chrome's line-height rounding.
- [x] Floats and clear, list markers, system fonts with Chrome's family
  policy (Liberation Sans for sans-serif, …), SVG files as images,
  mask-image, text-decoration.
- [ ] vertical-align (23 images), table captions/column groups/rowspan,
  collapsed borders, a block formatting context beside a float narrowed.
- [x] Paint: gradients, border-radius, box-shadow, rounded overflow clips.
- [x] Paint: `url()` background images (size, position, repeat), CSS
  transforms, opacity groups.
- [ ] Paint: dashed/dotted
  borders, inline backgrounds' images and borders (their colour; their
  corners round since JOURNAL 2026-11-08, elliptical radii as circular)
  paints), rounded borders with differing sides,
  `filter` (colours, blur and one `drop-shadow()` drawn, in order;
  open: a second drop-shadow, url(); JOURNAL 2026-11-08), `mask-image`, `mix-blend-mode`, SVG `<text>`, `<use>`,
  `<mask>` (Stripe's Sign in button).
- [x] Text: letter-spacing/word-spacing, `text-wrap: balance`,
  `::before`/`::after` generated content (strings, attr()).
- [ ] Text: word-break/overflow-wrap (583), text-decoration (28),
  text-transform, counters and quotes in `content`.
- [ ] MDN: its columns, header and Baseline panel as Chrome's (JOURNAL
  2026-11-08 night); open: the sidebar toggle and three browser logos
  are missing, the contents' current entry
  is unmarked; old Reddit's login dialog (a `faceplate-partial` its app loads)
  never shows; Amazon's front page is its AWS WAF challenge, which fetches
  its inputs and then waits on something that never comes (no verify
  request; blob workers, Web Crypto and FormData bodies work).
- [x] Wikipedia's video player: poster and play button as Chrome's (2026-11-08).
- [ ] Line height: Chrome spaces DuckDuckGo's result snippets (an inline
  `<a>` with `line-height: 1.45` in a block with 1.6) by the inline's
  line height, its block's strut not counting, though every reduced case
  counts it (23.04 px); Olo counts it. Find what in that page does it.
- [ ] Form controls: checkboxes, radio buttons and selects are drawn as
  Chrome draws them (`appearance`, `accent-color`, disabled, dark), on the
  baseline as Chrome lines them up; their hover and pressed shades, range,
  colour, file, `<progress>` and `<meter>` are not drawn yet. A select
  opens its menu (the kit's) and takes the choice, and is as wide as its
  widest option (a group's indented), as Chrome, within 1 px; open:
  `<optgroup>` labels in the menu, arrow keys on a focused select. Canvas placeholders.
- [ ] Iframes: a live page's iframes render as pictures of their documents
  (stylesheets, images; JOURNAL 2026-11-08). Open: their scripts (ads,
  YouTube and map embeds need them), input into them, iframes of pages
  without scripts, `srcdoc`, `loading=lazy`.
- [x] JavaScript: V8 with a native DOM (crates/js): the Web API surface in
  a prelude, scripts that scripts insert, measurement from a layout of the
  page as it is (getBoundingClientRect, offset*/client*/scroll*,
  getComputedStyle), observers (MutationObserver records as Chrome's),
  custom elements, Web Animations (sampled by the engine each frame),
  MessageChannel, the 2D canvas (tiny-skia) and WebGL 1/2 (the
  GPU through OpenGL ES, a context per canvas, draws validated as WebGL
  does), images loaded for scripts (`img.src`, textures, drawImage,
  ImageBitmap) and fetch()/XMLHttpRequest, both fetched off the thread
  and delivered to the live page, Web Workers (a thread and isolate
  each, structured clones), blob URLs, ES modules (static graphs
  fetched level by level on threads, `import.meta.url`, dynamic
  `import()`), events from the person. Next: import maps and JSON
  modules, dynamic `import()` fetching off the thread, CSSOM
  (`insertRule`, `styleSheets`, CSS-in-JS in production builds),
  streaming request bodies, `document.write` while
  parsing, iframes, OffscreenCanvas, ResizeObserver with real sizes.
  IntersectionObserver follows the scroll and the layout (JOURNAL
  2026-11-08); createHTMLDocument makes a document (jQuery needs one).
- [x] Stripe's graphics: the hero wave, the agentic and issuing
  backgrounds, the globe and the data-viz canvases are WebGL (three.js:
  textures from the page's images, geometry from Web Workers, render
  targets) and draw through the engine's OpenGL ES contexts, one per
  canvas, read back when drawn (JOURNAL 2026-11-07 afternoon); the
  carousel's 2D canvas draws. Events from the person reach the scripts
  (hover opens the menus). The Sign in button (an SVG mask with
  `<text>`) renders. Remaining on Stripe: the animated states checked
  against Chrome at the same moment of the page's clock, and the live
  page's cost (below).
- [x] The page lives: after the static render (612 ms on stripe.com,
  prefetching subresources together) and the scripted one (3.6 s), the
  realm's timers and animation frames run on the page's clock, the
  document is re-rendered from a snapshot when it changes (incremental
  style through selector invalidation sets and kept matches, prepared
  texts kept, tiles kept by signature so only what changed repaints,
  WebGL canvases read back asynchronously, coalesced to 10 a second),
  and nothing runs while nothing is due. Measurements after inline
  style changes patch the previous styles in place (0.05–0.1 ms an
  element; JOURNAL 2026-11-07 evening); prepared texts are reused by
  index. Live renders patch styles from the change log too, and a
  change of `transform` alone patches the scene's matrices (0.4 ms a
  render instead of 4.9); an animating canvas is a surface layer (one
  image upload a frame, the tiles under it kept). stripe.com at steady
  state: Olo 13.9–14.9% of a core against Chrome's 15.6% (JOURNAL
  2026-11-08 midday). Layout is incremental: a block subtree holding no
  changed node, under the same inputs, is taken from the previous
  layout (`layout::Job::with_previous`; checked against a full layout
  by `OLO_LAYOUT_CHECK`; JOURNAL 2026-11-08 afternoon), so the counter's
  text change relays out 180 of 1983 boxes (layout 7.6 → 1.2 ms) and
  stripe.com runs at 11.6% of a core against Chrome's 18.1% over 30 s
  (JOURNAL 2026-11-08 evening: the natives profiled, observers gated on
  geometry, a spinning tick fixed). Live pages render at 60 frames a
  second (20.4% of a core on stripe.com at 54 renders a second; JOURNAL
  2026-11-08 night: moved layers, shared document strings, the GPU
  atlas leak). WebGL canvases show from shared textures, no copy, on the
  window's GPU, which is the compositor's (16.5% of a core on stripe.com
  against Chrome's 18%; JOURNAL 2026-11-08 late). Pages are capped:
  memory, CPU, GPU memory and frame cost (same entry). Next, in order:
  one process per page when tabs or more windows come (the caps are a
  process's: each page needs its own); semaphores instead of glFinish
  for shared frames (the page thread waits for the GPU; a heavy shader
  blocks it); a hidden or covered window's page drawn at 1 Hz; frames paced by the window's vsync (the
  worker renders on a timer now, 54 of 60 a second); the hero canvas
  shared with the window's GPU without a readback (one 2.5 MB copy and
  upload a frame); renders that keep the previous scene's tiles for
  more than transforms (opacity, colours); the rest of that
  full job (the box tree rebuilt from the whole document, 3.4 ms: copy
  the clean subtrees' boxes; the scene rebuilt, 3.2 ms; the style job,
  1.2 ms, where a text change needs no restyle unless a selector tests
  `:empty`); flex and grid items from the previous layout too (only
  blocks in normal flow are taken now); a V8 CPU profile of
  the live page (scripts are 85–130 ms per 2 s: the page's own work or
  our natives?); an animating transform as a moving layer placed by the
  window (the marquee repaints 4 tiles a frame); hover restyles patched
  (`restyle` is a full job: 15 ms on stripe.com, 30 on pricing); CSS
  gradients rasterised once and blitted (3 ms a tile on
  stripe.com/pricing); canvases read back as BGRA, then composited on
  the GPU without readback; dynamic `import()` fetching off the
  scripts' thread (21 graphs, 1.2 s of a load, in turn); numeric GL
  opcodes; a window resize reaching the live page's scripts
  (innerWidth/innerHeight, `resize`, measurements at the new size);
  a new image bound after load relays out the page (pricing: 55, 59,
  61 images bound over its first seconds) where a box whose size is
  known should only repaint.
- [ ] Performance, the first priority: styling Stripe is 34 ms (was 115:
  pseudo-element buckets, ancestor Bloom filter, hashed keys, root/
  attribute/:is() buckets, sparse computed values, chained custom
  properties). Next: incremental restyle and relayout for scripts'
  measurements are incremental for style (5 ms) and texts; boxes, layout
  and paint still redo everything (10 ms); a stylesheet added by a script
  recompiles every rule (133 ms on Stripe) where it should extend the set
  and invalidate only what the new rules can match; matching's
  per-candidate floor (17 ms for 76k candidates), computing declared
  values (8 ms), then parallel
  styling (elements are independent given their parent), SIMD text
  shaping and rasterisation, GPU compositing of the scene.
- [x] `position: fixed` and `position: sticky`: layers over the page's
  tiles (the scene indexes their commands apart; the worker rasterises
  each in tiles kept by signature), the fixed one unscrolled, each
  sticky one shifted as the scroll has it (`layout::Sticky::offset`,
  top and bottom insets, within its containing block); hit-testing and
  the agent labels follow. Not yet: left/right insets, sticky inside a
  scrolling container, scripts seeing the stuck position.
- [ ] Dark pages: `color-scheme` is honoured as Chrome does (system
  colours, `light-dark()`, the canvas, form controls follow an element's
  scheme; text in the default colour turns light). Not yet:
  `<meta name=color-scheme>`, an automatic dark rendering for pages that
  have none.
- [ ] CSS animations and transitions: at least the resting end state
  (`animation-fill-mode: forwards`, keyframes' last frame), which pages
  rely on for fade-ins that scripts would otherwise finish.

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
- [ ] Complete network/encoding/cache with security and memory bounds
  (cookies: a jar, JOURNAL 2026-11-08 morning; open: the Cookie Store API,
  partitioned cookies, clearing them from the UI).
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
- [x] Core typed declaration compilation: initial92 longhands/16 shorthands;
  current95/18 (including
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
- [x] Connect initial row flex item construction, scalar distribution, layout,
  initial alignment and cross-size/stretch to production pages; compare320
  definite lines and260 independent authored page layouts at five native DPRs.
  Cache intrinsic measurements per shared immutable Text across width jobs;
  explicit bases/minima avoid intrinsic measurement. This is partial Flexbox.
- [ ] Complete flex column/RTL/wrapping/order/baseline and compound alignment,
  replaced items, negative inner border-box bases, nested/contained intrinsic
  contributions and mixed/decorated text intrinsic contexts. Three initial
  alignment longhands and border width/style/color expansion are registered;
  complete alignment grammar, border-image reset and related painting remain.
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

- [x] Baseline atomic-inline worker geometry: explicit semantic objects, glyph-free
  shaping, complete bounded used metrics, bidi/wrap/margins and sparse line-height
  expansion; 380 independent Chrome component observations at five scales.
- [x] CSS device-grid font advances/struts and trailing zero-width control fitting:
  break-control geometry 415/415, inline spacing 219/225 (six pinned nonpasses).
- [x] Shared byte-backed PNG/JPEG/WebP/GIF still decode with explicit input,
  dimension/pixel/surface/profile limits, codec allocation hint, EXIF/ICC policy
  and read/stage cancellation; lossless bytes and direct-path cost controls.
- [ ] Complete resource decoder isolation and interruptible CPU/colour work,
  animation, SVG and bounded AVIF/JXL/ICO/other formats. Codec allocation hints
  are best effort, not a hard process-memory bound.
- [x] Initial still-image integration: typed resources, shared decoded assets,
  inline/block replaced sizing, dimension cascade hints, tiled paint and structured
  image/link metadata. Full-pipeline object geometry matches 380 Chrome observations;
  new sizing checks match 285/300 with 15 explicitly unsupported alignment cases.
- [ ] Complete image behavior: broken/alt fallback, responsive sources,
  top/middle/bottom alignment, object-fit/position, writing modes, stacking and
  resource/security policy. Native Chrome pixels currently match 53/60 captures;
  five minification differences (at most two channel levels) and two single-byte
  fractional interpolation differences remain pinned as nonpasses. Bounded
  worker mip caches and scalar/SIMD byte controls are implemented; independent
  odd-source/anisotropic controls, exact GPU filtering and GPU composition
  remain required. The original 49/60 classifications are preserved.
- [x] Bounded, cancellable source-pixel minification cache, weak lifetimes,
  navigation reclamation, tile seam controls and scalar/SIMD reduction controls.
  Four more pinned Chrome captures now match exactly (53/60 total).
- [ ] Recover the ordinary-image upscale cost after mip integration: seven
  rotated frozen release processes measure about 0.94→0.98 ms per 256² tile
  (roughly 4%). Initial generic blending was about 40% slower; const draw
  specialization and removal of redundant snapped-edge coverage recovered most
  of it. Preserve both datasets; remaining regression is not accepted.
- [ ] Resolve image-page performance fuzz latency: seed4106 first run has a
  66 ms Browser / 30 ms compositor shortcut hitch. Same-seed fresh confirmation
  passes stock budgets, but its worst app frame is 20.8 ms, above the 8.3 ms
  target. Preserve both observations and identify the cause before acceptance.
  Optimized build with main 350f22c6 passes a PID/binary/content-verified 24-step
  repeat; worst app frame 14.4 ms and native first-motion response about 9 ms still
  miss 120 Hz. Discarded forwarded-launch runs cannot establish coverage; native
  cost runner now asserts fresh PID ownership and awaits Quit/process exit.
  Minification fixture seed4107 fails step14/24, arrange x12:78.8ms app frame
  (23.4ms average of four), despite PID/binary/image verification. Concurrent
  compilation is recorded but not established as the cause. Keep this nonpass;
  compare a baseline and profile before claiming latency acceptance. Paired
  native text trials overlap compiler work and have opposite hitch counts;
  their latency/memory observations cannot establish a causal change.
- [ ] Profile control-state integration cost observations: seven rotated
  frozen release pairs show pipeline100/1000:1257.02→1245.31 /11889.36→12320.26µs;
  scene282.24→328.15 /3594.45→4042.43µs. All source/geometry/pixels match,
  but host load is high and nine accepted samples have concurrent Cargo/test
  context (no rustc observed at sample boundaries). Scene construction does not
  query the new state index; cause/attribution remains unresolved. Preserve the
  dataset and confirm/profile before performance acceptance.
- [ ] Profile integrated-flex checkpoint cost nonpasses: ordinary pipeline100
  811.81→813.75µs (~0.24%);1k8832.14→8796.69µs. Scene202.98→191.08 /
  3341.87→3246.78µs does not clear earlier regressions.28 accepted samples,
  2 compiler discards/71 waits/10 accepted Cargo contexts, load8.88–13.51;
  no causal attribution or full host isolation. Native2× trials each have1
  hitch; first-motion client median5.648→8.926ms, max9.159→9.211ms still misses
  8.3ms. Compiler overlap is recorded, not established as the cause. Idle0 CPU/
  0 wakeups and new worker/cache controls do not accept full performance.
- [ ] Profile flex-shorthand-checkpoint ordinary-path cost: pipeline100/1000
  813.35→819.94 /8938.51→8652.91µs; scene188.84→209.25 /3316.88→3531.09µs,
  identical geometry/pixels. Small pipeline~0.8% and scene~10.8%/~6.5%
  regressions remain nonpasses (28 accepted, no observed compiler/Cargo context,
  load9.69–10.55). Preserve the earlier dataset too. Retained scene construction
  does not call the new declaration/style code; cause remains unresolved.
  Separate near-identical instruction-counter diagnostics overlap compilation
  and do not clear these findings.
- [ ] Resolve the five flex shorthand fit-content() reference gaps (one case at
  five DPRs): current shared draft sizing grammar accepts fit-content(20px),
  Chrome157 rejects it; function used geometry remains unsupported.365/370
  shorthand observations match; no native CSSOM/item layout acceptance yet.
- [ ] Profile intrinsic-checkpoint ordinary-path cost: pipeline100/1000
  809.94→814.49 /9000.35→8886.89µs; scene186.14→190.19 /3265.09→3496.42µs
  with identical geometry/pixels. The new intrinsic operation is not called on
  these paths. Scene~2.2%/~7.1% and small pipeline~0.6% regressions remain
  nonpasses;60 compiler waits/1 accepted Cargo context are recorded, causality
  and full isolation unestablished. Worker-control speedups do not resolve them.
- [ ] Profile the remaining 1,000-element scene regression after image support:
  2388.630→2495.630 µs (4.5%), CPU0/seven alternating frozen processes with
  identical source/geometry/pixels and no compiler activity. Text-only scene
  specialization avoids image bookkeeping; total pipeline is 8277.681→8068.671
  µs on the same fixture, which does not resolve the isolated scene finding.
- [ ] Complete font metric policy: per-inline-item used advances, normal line-height,
  arbitrary fonts/hinting/variation and mixed struts; six independent inline
  coordinate nonpasses remain explicit.

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
  Box/advance/whitespace/empty-anchor targets, complete image target semantics,
  pointer capture, cursor
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
  anchor behavior. Initial decoded-image link and anchor targets now work.
  Unsupported link actions report a reason without requests.
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

- [x] Initial semantic `br`/`wbr` formatting sources, preserved forced breaks,
  optional breaks under nowrap/pre, carrier-free structured text, bounded
  validated sparse metadata and distinct literal-Unicode cache identity.
- [ ] Complete inline break behavior beyond the 415/415 matching independent
  Chrome combinations. Styled/positioned controls, bidi paragraph propagation,
  caret/anchor ownership of shared clusters and full inline decorations remain
  required. Initial decoded still images work; form/other media objects remain
  unsupported. No website passes.

- [x] Resolve the semantic-control milestone's ordinary-page worker cost
  regression on the authored matched case. Reusing span paint colors measures
  905.738/8897.912→776.512/7441.896 µs for 100/1,000 elements; matched
  pre-control baseline 895.940/8868.916 µs (CPU0, seven rotated samples).
  Original samples remain retained. This is not general website acceptance.


- [ ] Remove sequential image-loading startup delays. The resource loop currently
  completes image requests before box construction, with a60-second per-request
  timeout. Preserve cancellation, bounded memory, progress and deterministic
  resource/layout updates when implementing concurrency; don't count omitted
  or placeholder images as site acceptance.

- [x] Prevent long status/error text from resizing the document viewport and
  repeatedly reloading failed pages. Delayed HTTP repro stays at one request;
  real Stripe failure returns to idle. Full absolute layout remains open.

- [x] Initial absolute non-replaced axis equations, independently compared in
  225 Chrome observations at five native DPRs (numeric stage only).
- [x] Initial deferred absolute non-replaced blocks, containing-block selection,
  padding-box percentages, auto-height/nested placement, flex exclusion and
  painting. 125 independent pages/545 Chrome rectangles match at five DPRs.
- [ ] Complete absolute static-position discovery, shrink-to-fit, replaced
  positioning, inline interruption and modern alignment/stacking semantics.
- [x] Initial SVG visible/per-axis clipped overflow with expanded ink query
  bounds. 90 independent border rectangles match, 80/90 image crops exact.
- [ ] Close the 10 SVG overflow fractional/curve pixel nonpasses plus the older
  28 SVG page pixel nonpasses; strict pixel gate still fails.
- [x] Exclude non-rendered SVG defs subtrees from direct painting; preserve DOM.
- [x] Initial local clip-source parsing/cascade:145 independent Chrome computed
  values, thin shared references and unique-allocation accounting.
- [x] Typed rounded inset() values: compact shared numerics, reference boxes,
  variables/math, computed inheritance,205 Chrome observations across five DPRs.
  Numeric-probe precision limited to1/64px+.0005px; no shape geometry claim.
- [x] Initial referenced SVG rectangular clips: bounded indexed ID lookup,
  shared definitions, computed geometry/radii, unions and ancestor intersections.
  185 independent Chrome bounds match;137 exact crops,48 explicit pixel nonpasses.
- [ ] Close48 clipping pixel nonpasses; exact gate remains failing. Support root
  decoration clips, bbox units, non-rect shapes, recursive clips and transforms.
- [ ] Complete variable-dependent inset() geometry/painting and SVG hit testing.
- [x] Initial non-root SVG object/group opacity and isolated clipping: retained
  ranges/bounds, cropped bounded worker surfaces, nested compositing, byte alpha.
  140 Chrome bounds match;65 exact crops,75 explicit pixel nonpasses.
- [ ] Profile native ordinary scroll hitch observed at opacity checkpoint:
  before0/after1 hitch and still frame; slowest client draw4.89→9.27ms. Median
  client-submit latency improved, but does not establish smoothness acceptance.
- [ ] Optimize isolated clipped-layer raster:256×256 tile418/408→542/537us
  for100/1000 SVG workloads (~30% slower); full ink bounds are needed with the
  current scan converter. Investigate exact binary-clip fast paths and raster
  coverage together, without hiding blend/edge regressions. Ordinary SVG retained
  bytes improve8B/root; clipped vector bytes add40B/root, command bytes save8B/ref.
- [ ] Close opacity/clip curve and fractional-edge nonpasses; clipped benchmark
  first viewport differs57 pixels vs55 before, same max20 error atDPR1.
- [ ] Complete root SVG opacity/decorations, blending and CSS stacking.
- [ ] Visible SVG rectangles: latest fresh HTTPS startup blocker (slot317).
  Retry actual HTTPS after each supported component; keep failures explicit.
- [x] Clarify probe shutdown: Cmd+Q was not proof of process termination; explicit
  documented menu quit exits promptly. Harnesses must invoke quit, await actual
  window PID and verify fresh launch identity; forwarded launches aren't fresh.
- [ ] Verify Browser's human quit shortcut and last-window lifecycle separately:
  the Cmd+Q probe left a live single-instance process; explicit menu quit exits.
  Correct harness cleanup is not proof that every human quit path works.
