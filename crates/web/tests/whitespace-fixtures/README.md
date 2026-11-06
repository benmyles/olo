# CSS whitespace preparation references

Six unchanged WPT HTML files and their parsing helper/license are pinned in
SOURCE.json at d7bb5bb848fcd304c10669288a211dbafd106d90. The 52 literal ports
test native declaration acceptance, including CSS-wide values. They do not
execute upstream JavaScript or test CSSOM serialization, rendering or layout.
The original WPT files cover fewer values than the current draft: local
tests separately cover the two-longhand shorthand combinations. Discard and
white-space-trim are unsupported and rejected; they are not silently ignored.

extract.py also produces 3,925 independent uniform-mode phase-I references.
It rewrites a bidi-control-free projection in separate regex phases, then
merges controls back in source order. No Olo code, shaper or browser generates
these expected strings. Tests check every case with different job budgets,
then split every scalar into a separate text span, checking all source/output
coverage and lookups. Adversarial partitions, cancellation, quotas and SIMD
boundaries are separate tests. This is phase I, not all CSS Text conformance.

The production source map stores 12-byte records: input/output start offsets
and a packed span/kind. End offsets follow from the next record or string
lengths. Public Mapping values are decoded on demand, with binary search for
byte queries. Unchanged prepared text shares the original Arc allocation.
baseline-text-map-wide controls storage width (24 bytes) with identical facts;
baseline-text-prepare disables bulk scans/copies with identical semantics.

chrome-probe.html and chrome-inputs.json capture separate observations from an
owned Olo Agent Screen. Serve this directory on loopback with POST /result
saving the report, then run Olo's Chrome through olo-screen. The captured
65-row report and the native whitespace_probe comparison are in
docs/browser/measurements/2026-11-05-whitespace-{chrome,projections}.json.
Chrome 157 supports 57 cases; eight preserve-spaces cases are unsupported by
that build. Native phase I and Chrome innerText match in 45 cases and differ
in 12. **These are different pipeline stages, not a 45/65 compatibility score.**
CR plaintext retention accounts for three differences, the current always-
space UA policy versus Chrome's zero-width-space rule for four, and bidi
control handling for five. Native preparation follows CSS Text's transparent
bidi-control rule. Browser width observations are retained, but no native
layout/raster comparison exists yet. Do not count differences/unsupported as
passes; whitespace_probe exits 1 while they remain.

The current worker supports LF segment breaks in DOM text and treats DOM CR
as a space; the HTML tokenizer owns source CRLF normalization. It retains
tabs/newlines for preserve modes. Phase II trimming, tab stops, hanging and
soft-wrap opportunities, styled shaping, full language policy and visible
control glyphs remain downstream work. No website acceptance follows.

```sh
/usr/bin/python3 crates/web/tests/whitespace-fixtures/extract.py
cargo test --release -p olo-web --test whitespace --test style --test shaping
cargo run --release -p olo-web --example whitespace_bench -- 100
cargo run --release -p olo-web --example whitespace_probe -- \
  docs/browser/measurements/2026-11-05-whitespace-chrome.json \
  /var/tmp/owned-browser-projections.json
```
