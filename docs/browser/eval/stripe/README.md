# Stripe: active website

The user selected **stripe.com first**, then one website at a time until the
100-site corpus works. Do not move to another site before Stripe passes its
recorded journeys and all configurations. The earlier three live startup
probes are diagnostics; they are not accepted websites.

Stripe is already `stripe-com` in `../sites.json`. Keep its platform journey
and acceptance gates: a marketing screenshot alone cannot accept the site.
Start with the public homepage, header navigation, pricing and a read-only
public interactive example. Inspect the actual reference before specifying
that example's executable steps. Authenticated workflows remain blocked
without a dedicated eval environment; no personal account is part of this run.

## Latest checkpoint

2026-11-08 midday: the homepage renders as Chrome does at 1000 × 700,
1× (static sections within a pixel; JOURNAL 2026-11-07), with its
scripts run (V8), its WebGL scenes drawn on the GPU, its workers,
fonts, SVG (`<use>`, masks, text) and the live page's animations
(the hero wave, the bento scenes, the marquee, the counter) at
11.6% of a core against Chrome's 18.1% (`tools/dev/cost` and /proc
over the same 30 s at steady state; JOURNAL 2026-11-08 evening): per
2 s, scripts 53–63 ms, renders 54–84 ms, tiles ~28 ms. The pricing
page renders as Chrome (JOURNAL 2026-11-07 night). Hover opens the
navigation menus. Not yet compared: the animated
states at the same moment of the page's clock, the pricing page and a
navigation journey (click, scroll, back), the narrow viewport, dark
mode and the other scales. No configuration is accepted.

The checkpoint below is from before the layout and script work.

The native browser renders the homepage end to end (2026-11-06 evening):
hero heading, buttons and the hero image, with 1,054 recorded fallbacks
(`Page::fallbacks`; the list is in the agent Document node and in the
JOURNAL). Against Chrome at 1000 × 700, 1×: the header is missing, the
heading is drawn twice, the hero image paints over the text (z-index in
tree order), the buttons stretch (column flex as block), the web font is
not loaded, and backgrounds, gradients, radii, shadows and transforms are
absent. Earlier component results (SVG paths, clips, opacity, rectangles)
stand; they are component results, not site passes. No configuration is
accepted. Work follows the TODO's impact order.

## Current evidence

An isolated Olo Chrome profile loaded `https://stripe.com/` with
Chrome157.0.8103.38, Chromium a733b5afbb3ba618844ff8876b89ad1a774b2f18.
Its observed DOM contains44 images and86 scripts and uses Sohne/Source Code Pro
WOFF2 fonts. Five live stylesheet URLs and the original HTML were retained in
owned disk scratch with hashes. The initial resource-tree query omitted some
stylesheets; the document's actual stylesheet list supplied all five. This is
an initial live inventory, not a complete deterministic network recording.
The separate web-search service redirected to an Indian regional page; it is
not used as this run's visual reference.

The original native startup stops with `Selectors(StateUnavailable)`.
A precise diagnostic identifies **Disabled**. The first engine fix adds
bounded, lazy connected-tree CSS `:disabled`/`:enabled` states. Independent
Chrome captures cover176 element observations (352 selector booleans), including
legend moves, fieldset/option attributes, native/custom/foreign elements and
nested disabled ancestry. Form submission/activation disabledness is distinct
from these CSS states. Input widgets and JavaScript are not implemented by this
change.

After that fix, the actual native Stripe startup advances to
`Property(Variables(Unsupported("arbitrary substitution function")))`.
The retained stylesheets contain `env(safe-area-inset-bottom, 0px)` inside a
padding calculation. Environment substitution and its frozen host context are
now implemented. Independent original fixtures match115 Chrome computed-style
observations at five native DPRs, including invalid indices, lazy fallbacks,
custom-property cycles and a changed host generation. Re-running the checked-in
Chrome capture driver reproduces all115 observations. Rectangular native
content viewports expose eight zero safe-area/current-maximum insets; hardware
segment values remain undefined. This is not every possible UA environment.

The live page now advances to **CSS style ContextUnavailable**. Preserving font
loader error detail leaves that diagnostic unchanged; no font-loader failure
was observed in that retry. A further real retry with a named numeric-unit diagnostic confirms
**Numeric(UnitContextUnavailable(Cqi))**. Initial horizontal box lengths now
retain container-dependent parts until layout and select nearest eligible
content-size bases per axis. Eight original ancestor contexts ×six units ×five
native DPRs match240 Chrome widths; inheritance/early contexts still needing
geometry fail explicitly. The real HTTPS startup advances to
**flow layout Unsupported("flex box formatting")**. Stripe's own container
geometry is not observed yet because formatting-tree construction stops first.
There is still no useful native Stripe content.
Do not suppress selector/value failures or replace the site with authored HTML.

Recognizing the site's `flex` shorthand exposes carousel basis
`clamp(240px, var(--case-study-carousel-card-fluid-width, 30.38cqi),
var(--case-study-carousel-card-max-width, 384px))`. The next real retry stops at
Cqi during styling; retaining FlexBasis with the existing box-length context
then returns actual HTTPS startup to the flex formatting blocker. A normative
regression covers the exact expression at three query widths and rejects
inherited pending rebinding. Its geometry on Stripe is still not reached.
Independent shorthand observations match365/370; five draft fit-content()
syntax differences remain recorded nonpasses. This does not implement item
layout or native CSSOM.

## Work queue

- [x] Launch the real public site in isolated native and Chrome windows.
- [x] Identify the first native CSS state failure precisely.
- [x] Implement initial enabled/disabled state and independent Chrome controls.
- [x] Release state cost/full checks:273 default /259 all-feature web tests,
  strict clippy and workspace build pass. Cost regressions remain explicit.
- [x] Implement required initial CSS environment substitution; retry the real site.
- [x] Identify Cqi and implement initial horizontal box query-container lengths.
- [ ] Implement required original flex layout; retry the same real site.
  The initial live20-context inventory is retained in `flex-inventory.json`:
  complete Stripe URL, loaded fonts, native DPR2, no text/account interaction,
  not a frozen replay. Header rows use auto content sizes, gaps and alignment.
  Initial retained LTR intrinsic text widths now match300 independent Chrome
  min/max pairs at five native scales. They remain outside page sizing, with
  bidi/multi-run items, inline objects/decorations and soft hyphens explicit
  unsupported contexts. Definite single-line size distribution now matches320
  original Chrome lines (1,095 widths); it remains outside page layout. Next:
  item construction/intrinsic bases and formatting integration, then wrapping/
  alignment using measured content; no guessed text widths.
- [ ] Complete container semantics: early/inherited contexts, query conditions,
  vertical modes, replaced containment and additional type/name grammars.
- [ ] Pin a complete replay, locale/region/cohort, assets, fonts and dynamic clock
  policy; capture meaningful reference checkpoints. Live changes cannot be
  mistaken for renderer differences, and blank/error views never pass.
- [ ] Implement the next observed engine blockers. The reference already needs
  modern layout/positioning, custom web fonts, SVG, dynamic JS/DOM and interaction;
  a dependency list is not evidence that these work.
- [ ] Execute public navigation/menu/pricing/demo journeys with mouse, keyboard
  and the same structured agent APIs, scrolling and history restoration.
- [ ] Exact useful-page geometry/pixel comparisons and visual review in
  light/dark/night, five live scales and desktop/narrow viewports (30 configs).
- [ ] Chrome-comparative release memory, latency, loading and idle gates.
  Environment milestone ordinary1000-element cost still regresses: pipeline
  ~7.6%, retained scene ~25.2%. Source/geometry/pixels match; cause unestablished.
  These remain nonpasses along with earlier sampling/fuzz findings.
- [ ] Accept Stripe only when every required assertion/checkpoint passes;
  choose the next site afterward.

**Stripe accepted:0/30 configurations. Corpus accepted:0/100 sites,0/3000
configurations.** The browser's minimum window width and the full capture/replay
and journey driver also need work; no narrow-viewport coverage is claimed.
