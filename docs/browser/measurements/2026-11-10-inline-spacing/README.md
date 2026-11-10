# Inline spacing and matching Stripe experiments

Inline child styles now contribute their own resolved letter/word spacing to
shaping and the shared text-cache identity. Previously every run received the
paragraph's spacing, even when the child overrode it. Spacing also survives
line-boundary reshaping. Safe DOM text-item boundaries are retained after
shaping for independent device-grid advance enclosure, while ligatures and
joining sequences crossing an unsafe boundary stay intact.

On the actual Stripe page, “Flexible solutions for every business model.”
changes from about622.02 to593.23 CSS pixels wide. Chrome is593.22–593.23;
the maximum final width difference is0.011 CSS pixels across15 live
light/dark/night ×1/1.25/1.5/1.75/2 configurations. This is a component and
specific-heading correction, not site acceptance. Corpus remains **0/100**.

## Comparison correction

The previous checkpoint's438px Sessions-card offset was caused by different
server experiment assignments. The native pre-script DOM contained a product
recommendations section; Chrome's original HTTP HTML did not. Normalized
experiment metadata identified `wpp_curator_h2.treatment` versus `.control`.
Only the owned screen's anonymous Stripe `cid` and `__Secure-sid` assignments
were aligned in the owned Chrome profile, then the actual HTTPS page reloaded.
Both browsers received the treatment section. The Sessions section then
matched within0.055px, with no vertical-flow engine change.

`experiment-evidence.json` retains normalized labels, section presence and
source hashes. Native source was a before-scripts DOM serialization; Chrome
sources were original HTTP response bodies via CDP. Cookie values and source
bodies are not published. These were public anonymous test profiles, not a
personal browser or account. Lazy content was also scrolled into view before
whole-page diagnostic measurements.

## Validation

- `crates/web/tests/span-spacing-fixtures`:80 independent actual-Chrome
  case/scale observations, eight rectangles each. Sequential style changes,
  shared text with different styles, fresh versus reused measurement, nested
  sizes, negative spacing, wrapping, RTL, transparent ligatures and forced
  line-boundary reshaping. Absolute coordinate allowance remains0.02 CSS px.
- Existing inline-box component observations now pass225/225, resolving the
  six previously pinned fractional text-item rounding errors. Chrome reports
  and the existing comparison allowance are unchanged.
- Release workspace:2,209 passed,0 failed,136 existing ignored,253 suites.
  Release workspace build succeeds. Browser live layout lints pass at1280×800
  and1440×900. Logs and exit statuses accompany the evidence.
- `before` and `after`:30 native captures/reviews and30 Chrome captures,
  with the windows at odd x49. Every native review is clean. Final drivers
  assert DPR, full/client viewport, scroll500 and normalized experiment
  agreement. Chrome is focused and given two animation frames before each
  capture so its previously occluded surface cannot supply stale pixels.
  Visual heading crops were inspected at every theme/scale; complete window
  captures remain available. Screenshots are also in the visual journal.

Capture development exposed setup errors: the first component text used
characters absent from the tiny font; a later setup installed the same font
as both local and web, masking web-font metrics. The final test uses a Noto
fallback and loads the original fixture font as a web font in both engines.
The final strict regression initially retained16 width errors of0.025px at
1.25×; retaining text-item boundaries fixed those. Diagnostic logs preserve
these distinctions rather than treating setup errors as engine verdicts.
An initial live matrix reused a one-shot scroll flag after a full-page scan;
a later version read geometry correctly but captured stale occluded Chrome
pixels. The final matrices explicitly position and repaint both windows.

## Remaining work

Stripe's main-content height still differs by about420px in the matched,
scrolled diagnostic. The first uniquely matched section-height difference is
business-sizes, about22px short; subsequent sections have additional errors.
`remaining-sections.json` only pairs unique identities and records unmatched
nodes separately: duplicate class names cannot identify a section reliably.
Do not infer layout defects solely from unmatched server/script content.

Full pixels, animated state, navigation, pricing, public interactions and
all30 acceptance configurations remain open. Optional-ligature suppression
with letter spacing, arbitrary Unicode spacing, and reflection of
`HTMLMetaElement.content` are not implemented by this checkpoint. The probe
uses `getAttribute('content')`. Prior scrolling and kit performance gaps
remain open. No general Chrome parity, speedup or production-readiness claim.

## Reproducing the live checks

Use an owned `codex-browser` Agent Screen. The recorded scripts expect disk
scratch `/var/tmp/codex-browser-vertical` and an owned Chrome debugging port9338.
Freeze release binaries as `browser-before` (b73f19a7) and `browser-after` using
reflink copies. Launch Browser with `OLO_BROWSER_EVAL_FILE` and
`OLO_BROWSER_EVAL_LIVE` pointing at scratch `probe.js`, containing
`matrix-probe.js`. Launch Olo Chrome only through `olo-screen run`; never use
a personal profile. Ensure experiments match before `matrix.py before|after`.
`measure.py` expects the screen to have no windows and opens only the local
`perf.html` fixture. It closes each test window and restores display settings.
No personal desktop was installed or reloaded; no delegation was used.
The owned screen was stopped after validation. Fetched origin/main is
contained in this branch (443 ahead,0 behind before this checkpoint).
Run `/usr/bin/python3 docs/browser/measurements/2026-11-10-inline-spacing/verify.py`
from the repository root to audit the retained evidence.

## Release cost and latency

`perf.html` contains24 fixed-height paragraphs with nested styled spans.
Seven rounds each measure9,000 cached JS geometry reads and20 alternating
letter-spacing restyles followed by rectangle reads. The baseline incorrectly
keeps the same width; the candidate alternates widths correctly. This extra
layout work is part of the corrected behavior. The first round is excluded
from timing medians. Each run also measures eight resizes, three seconds idle,
compositor timings and drag/wheel first-movement latency with `scroll-check`.

The initial ABBA run covers1.25× and2×. One1.25× candidate RSS sample was
173816KiB versus the other155236KiB. A second1.25× ABBA produced155548/155876KiB;
all samples, including the outlier, remain in the combined12-run data.
The table uses all samples. No builds, tests or journal generation ran during
these measurements. Tool resize elapsed includes about400ms of quiet waiting.

| Median | 1.25× before → after | 2× before → after |
|---|---:|---:|
| 9,000 cached reads, ms | 6.287 → 5.708 | 5.834 → 5.860 |
| 20 forced spacing restyles/layouts, ms | 2.172 → 2.472 | 2.094 → 2.700 |
| CPU per resize, ms | 10.000 → 10.625 | 14.375 → 16.250 |
| RSS, KiB | 154686 → 155712 | 163774 → 163926 |
| Drag first movement, ms | 36.65 → 36.75 | 36.75 → 36.35 |
| Wheel first movement, ms | 17.25 → 17.35 | 9.90 → 9.10 |

Every idle sample has zero measured CPU and page frames, with existing
script-watchdog wakeups. Across all scroll samples, hitches are7→2 and maximum
draw is8.47→9.61ms; each variant has eight stationary wheel frames. These
remain open performance findings, including frames above8.3ms. The samples
do not establish general speed superiority or fully smooth scrolling.

The first benchmark logger truncated its long floating-point geometry array,
so the driver could not parse completion. Geometry values are now rounded to
0.001px for this performance log only; timings and the independent geometry
regressions are unchanged. The failed driver log is retained.
