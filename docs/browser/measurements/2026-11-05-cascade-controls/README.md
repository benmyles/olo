# Cascade measurement controls

Apply these patches to the final source containing this directory, in a
clean developer checkout. They are **measurement-only** configurations;
restore with `git apply -R` before ordinary development. Source/binary hashes
and raw observations are in the neighboring cascade measurement JSON.

- `unpooled.patch`: initial per-rule selector collections, Vec declaration
  capacity and dense property ranges. It uses the final common property/
  variable code and corrected rule-rollback priority traversal. The same
  native fixture adapter reports 128 passes/8 unsupported. Its initial
  resource/cache bookkeeping is not claimed identical outside these valid
  measured workloads; do not run the final resource-field-specific tests
  against this prototype API. This is not a previous production revision.
- `pooled-dense.patch`: final pooled selector/declaration code, with only
  dense property ranges and unconditional merge-buffer cloning restored.
  This isolates the sparse-range/single-candidate changes.

For each storage control:

```sh
git apply docs/browser/measurements/2026-11-05-cascade-controls/CONTROL.patch
cargo build --release -p olo-web --example cascade_bench --example cascade_eval
/usr/bin/time -v taskset -c 0 target/release/examples/cascade_bench 3
cargo run --release -p olo-web --example cascade_eval -- /var/tmp/cascade-control.json
git apply -R docs/browser/measurements/2026-11-05-cascade-controls/CONTROL.patch
```

The production `baseline-cascade-index` feature is a separate full-rule-scan
control, with the same matching/cascade semantics and independent outcomes.
It removes retained index data, so its lower storage is part of the reported
CPU/memory tradeoff. Default builds use the index.

The benchmark independently checks every generated element's expected width
outside timing. Matching buffers and node/rule data are warm during the bulk
loop; first-match/compile samples are separate. Times are synthetic worker
measurements, not page paint, UI, Chrome or browser memory. The payload sum
counts rule/declaration/selector/index capacities, excludes hash controls,
allocator metadata, RuleSet header, borrowed Syntax/DOM and runtime host state.
The benchmark's typed values are scalar widths with no heap programs.
