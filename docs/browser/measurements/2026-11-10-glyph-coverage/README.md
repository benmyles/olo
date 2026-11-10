Baseline: ae3f9701. This checkpoint caches the existing glyph raster output.
It does not change the raster policy or advance website acceptance (0/100).

- `bench.py`, `bench-final.json`, `bench-summary.json`: release ABBA direct/cache
  controls in the same binary, 500 repeated tiles/case, cold cost, hashes and
  retained payload. 100/1,000 paragraphs at five scales. No compiler overlap.
- `measure.py`, `costs.json`, `cost-summary.json`: Browser ABBA, 16 owned-screen
  runs with seven reloads each, idle cost and drag/wheel latency. `ordinary.html`
  and `text.html` are the local workloads. One baseline compiler snapshot was
  nonempty; raw data retain it. All idle samples are zero; no scroll hitches.
- `capture.py`, `visual-before`, `visual-after`, `visual-final`: 45 clean review
  observations, three themes × five live scales for each build stage, odd x=49.
  `client-pixels.json` and `final-client-pixels.json` compare inset client regions.
  `visual-pixels.json` retains whole-image exterior decoration differences.
- `fixture-pixels.json`, `webfont-pixels.json`: full native page PNG comparisons
  at five scales. Installed reference PNGs and HTML are in the preceding
  `2026-11-10-text-baselines` directory; downloaded fixture is its
  `synthetic-final/baselines.html`. Use `page_dump FILE 1000 --scale DPR --png
  OUTPUT HEIGHT`, with the corresponding reference device height. Both policies
  preserve all compared pixels. `paint_bench N direct|cached` is the permanent
  same-build pixel/control path.
- `workspace-tests.log`: initial run, failed only on the sandbox probe's checkout
  parent assumption. `sandbox-recheck.log`, `workspace-tests-final.log`: corrected
  probe and full rerun (2,181 passed, 136 existing ignored). Build/lint logs are
  retained too. `tests-mask.log` records the focused four-test cache suite.
- `close-shortcut.json`: static Browser's Cmd+W exits the actual process. No
  Cmd+Q shortcut is declared. In-flight close paths remain open.
- `analytic-prototypes`: rejected glyph raster investigations; not shipped.

Harnesses target the owned `codex-browser` screen and scratch under
`/var/tmp/codex-browser-glyphs`. The measurement harness requires a frozen
baseline executable at `browser-before` there; the candidate is the repository's
release Browser. They validate the running screen, restore its settings and
check process exit between launches. Stop the owned screen after use. Do not run
these against a person's session. The cache budget is payload accounting, not an
RSS cap; allocator headers and container capacity are additional.
