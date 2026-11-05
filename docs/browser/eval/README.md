# Browser compatibility evaluations

The evaluation has three separate results: standards, website journeys,
and cost. None implies another. HTTP success, a DOM title, a screenshot
file existing, or matching blank screens cannot count as a website pass.

The initial corpus will contain exactly 100 distinct sites, stratified
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
