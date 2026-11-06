# Unicode 18 default line-breaking evidence

Unchanged LineBreakTest.txt, LineBreak.txt, EastAsianWidth.txt and
DerivedGeneralCategory.txt come from Unicode 18.0.0. SOURCE.json pins their
URLs/hashes and reuses the shaping fixture's emoji-data/Unicode License V3.
The runtime algorithm is original Rust, following UAX #14 revision 57. The
generator reads factual properties only and preserves raw classes for future
CSS tailoring. It never uses test expectations or runs Olo to derive expected boundaries.

The native conformance test runs **all 19,346 strings**, at one-byte and
4,096-byte job budgets, comparing every allowed/prohibited boundary and
mandatory/allowed kind. Expected positions come directly from the original
markers; kinds come from its original rule annotations. Nothing is filtered.
Separate tests cover streaming frontiers, cancellation/poisoning/quota bounds,
UTF-8, empty/EOT semantics, mixed iterator traversal, 100,000 combining marks,
source sharing with CSS preparation, and bulk ASCII-run context preservation.
One-byte stepping always uses scalar unit transitions, independently checking
the SIMD run shortcut at larger budgets. Full-rule and no-bulk measurement
controls preserve the same Unicode facts and are checked separately. A checked
class-table control measures the direct packed-class ID decode separately.

LB1 defaults map ambiguous/unknown/surrogate classes to AL, CJ to NS, and SA
marks to CM/other SA to AL. **SA counts remain visible**: Thai/Lao/Khmer/Myanmar
browser layout requires separate complex-context segmentation/dictionaries.
Passing default UAX tests does not supply that language behavior. CSS strict/
loose rules, word-break/overflow-wrap/nowrap/white-space tailoring, emergency
grapheme boundaries, choosing safe widths and shaping line boundaries remain
outside this module. No CSS WPT, pixels, page layout or website acceptance is
implied by this conformance result.

Lookahead and backward context have constant size (the initial Job is 200 B
on the measured x86_64 target). Internal opportunities store four-byte packed
offset/kind values. EOT is an implicit required marker derived from source
length: no offset allocation when there is no internal break. Source storage
is shared. Total input/opportunity quotas, byte-budget steps, progress and
AtomicBool cancellation are enforced on workers; these are not a process
sandbox, fallible allocation or a whole-browser RSS bound.

```sh
/usr/bin/python3 tools/browser/generate-linebreak-unicode.py
cargo test --release -p olo-web --test linebreak
cargo test --release -p olo-web --test linebreak --all-features
cargo run --release -p olo-web --example linebreak_bench -- 200
```
