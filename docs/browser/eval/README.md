# Browser compatibility evaluations

The evaluation has three separate results: standards, website journeys,
and cost. None implies another. HTTP success, a DOM title, a screenshot
file existing, or matching blank screens cannot count as a website pass.

## Implemented now

`sites.json` contains 100 distinct domains in eight categories, ranked
against Tranco list JZPGY (2026-11-04). This is a curated coverage set,
not the numeric top 100 or a measured complexity ranking. Selection
and journey recipes live in `tools/browser/build-corpus.py`; the full
ranking download's SHA-256 is pinned there and in the manifest. The
selected ranks are also in `selected-ranks.csv`. Rebuild by downloading
the pinned CSV to /var/tmp and passing its path to that Python script.
No website journey has been executed yet. Category recipes still need
site-specific runnable steps, stable test items and independent captures.

The native Rust comparator is runnable now:

```sh
cargo run --release -p olo-web-eval -- docs/browser/eval/sites.json \
    /var/tmp/olo-browser-eval/chrome /var/tmp/olo-browser-eval/olo \
    /var/tmp/olo-browser-eval/report.json
```

Create the report's parent directory first. Exit 0 requires every case
to pass; 1 means incomplete/failed/blocked evidence, 2 invalid input or
an output failure. Empty capture directories produce **0/3,000 cases,
0/100 sites**, with all missing cases retained in the denominator.
This command compares local artifacts; it does not launch a browser,
capture pages or execute the journeys yet.

Every run root needs `run.json` (`RunMetadata` in
`crates/web-eval/src/lib.rs`): browser/build and source identity, SHA-256
of the exact suite bytes, binary, replay resources and fonts, machine/OS,
locale/timezone, capture time and release-build status. Engines must be
`olo-chrome` and `olo-browser`, with different binary hashes. Environment
and replay-resource identities must match. These declarations must come
from the future capture driver, not be hand-authored to manufacture passes.

Each configuration has this layout:

```text
RUN/site-id/desktop/light/100/observation.json
RUN/site-id/desktop/light/100/loaded.png
RUN/site-id/desktop/light/100/interaction.png
RUN/site-id/desktop/light/100/scrolled.png
RUN/site-id/desktop/light/100/history.png
```

Scale is an integer percent, viewport IDs are `desktop`/`narrow`.
`Observation` requires the configuration identity, status (`completed`
or `blocked`, with reason), every assertion in the site's manifest,
all four `Checkpoint`s and `Cost` measurements. Each checkpoint records
meaningful visible text, document URL/generation, scroll position and
at least three named visible landmark rectangles in CSS pixels.
Snapshots exclude secrets. Navigation changes the document, scrolling
changes its position and captured pixels, and history restores the
original content/position. Matching failed assertions never pass.

The comparator checks text, URLs and geometry (0.5 CSS pixel tolerance)
and streams PNG scanline pairs, with bounded decoder allocations.
Captures must be opaque, static PNGs at the exact device size, with
at least 64 non-background pixels. No masks: a channel difference >8
marks a changed pixel; at most 0.01% of pixels may change, and mean
RGB delta must be <=0.05. Matching blank screenshots fail. The
thresholds are an initial strict policy; passing still requires visual
review and cannot prove every page or state renders perfectly.

Candidate costs require input/frame time <=8.3 ms, navigation <=5 s
and <=Chrome's time ×1.1 +1 ms, peak RSS <=Chrome's, steady RSS growth
<=1 MiB, and a measured idle second with zero frames and <=0.1% of a
core. These are gates to measure, not achieved performance claims.
The report records all outcomes and reference/candidate provenance.

## Still required before acceptance

The corpus contains exactly 100 distinct sites, stratified
across popular destinations and complex web applications. Record selection
provenance and date. Popularity and complexity are different measures;
do not describe a curated coverage set as an independently ranked top 100.
Refresh with a reproducible CrUX/ranking snapshot before acceptance.

Every site needs public and, where applicable, dedicated-account journeys:
navigation, text input, scrolling, content change, history and its key
workflows. Consent, CAPTCHA, network refusal and login walls are explicit
blocked evidence, never a pass or silently removed from the denominator.
Never submit purchases, send messages or mutate real accounts in an eval.

Each run records source revision, engine/reference versions, hardware,
viewport, scale, theme, locale, fonts, resource hashes and journey steps.
Compare matching captures at deterministic checkpoints. Disable nondeterministic
animation where appropriate; record and bound any masks for ads, clocks
and personalized content. Do not mask layout failures.

Acceptance includes every expected checkpoint, matching meaningful content,
behavior assertions, accessibility/agent actions, geometry, pixel differences,
and release performance budgets. Both engines must draw useful content:
matching empty output fails. Reference failures are reported separately and
cannot become engine passes. No missing artifact, unevaluated step, unsupported
feature or skipped site can be a pass. Report numerator and denominator.

The full matrix is light/dark/night × 1/1.25/1.5/1.75/2×, at 1280×800
and a narrow viewport, plus live scale changes. Olo's UI chrome is reviewed
with tsession at every required scale and theme. All captures and scratch
profiles go under a task directory in /var/tmp. Run Chrome references only
through Olo's authorized launcher on an isolated headless screen/profile.

WPT and independent parser/DOM differentials provide reproducible, local
coverage below website-level symptoms. Network recordings need response
headers, redirects, encodings, subresources and timing policy; copying the
homepage HTML alone does not reproduce a modern site.
