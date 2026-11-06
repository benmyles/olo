#!/usr/bin/python3
"""Run from the repository root; pass the owned scratch directory of binaries.

Before binaries use parent acfd0d8e, with this commit's unchanged
cascade_property_bench.rs added. After binaries use this commit. Build each
with cargo build --release -p olo-web --example NAME; copy with reflinks into
/var/tmp, never /tmp. Do not run builds/tests concurrently with measurement.
The before/after raw query benchmark source is identical. Projection is a new
capability and has no claimed before speedup. No git/source mutation here.
"""
import hashlib
import json
import platform
import subprocess
import sys
from pathlib import Path

scratch = Path(sys.argv[1])
result = dict(
    parent="acfd0d8ec5233feef860c06b998db26d81921919",
    platform=platform.platform(),
    cpu=Path("/proc/cpuinfo").read_text().split("model name")[1].split("\n")[0].strip(),
    affinity=0,
    rustc=subprocess.check_output(["rustc", "--version", "--verbose"], text=True),
    method="Release builds; three alternating before/after process runs on CPU0. Per-process means; first calls separate; no concurrent builds/tests. No page/Chrome evidence.",
    hashes={}, source_hashes={}, runs=[],
)
for name in ["cascade-before", "cascade-after", "property-before", "property-after", "projected-after"]:
    result["hashes"][name] = hashlib.sha256((scratch / name).read_bytes()).hexdigest()
for path in [
    "crates/web/src/css/cascade/mod.rs", "crates/web/src/css/cascade/axes.rs",
    "crates/web/src/css/properties/registry.rs", "crates/web/examples/cascade_bench.rs",
    "crates/web/examples/cascade_property_bench.rs", "crates/web/examples/logical_bench.rs",
    "docs/browser/measurements/2026-11-05-logical-measure.py",
]:
    result["source_hashes"][path] = hashlib.sha256(Path(path).read_bytes()).hexdigest()
for repeat in range(3):
    for benchmark, rounds in [("property", "30000000"), ("cascade", "300"), ("projected", "30000000")]:
        for variant in (["before", "after"] if benchmark != "projected" else ["after"]):
            output = subprocess.check_output(["taskset", "-c", "0", str(scratch / (benchmark + "-" + variant)), rounds], text=True)
            result["runs"].append(dict(repeat=repeat, benchmark=benchmark, variant=variant, result=json.loads(output)))
if (scratch / "short-measurement.json").exists():
    result["exploratory_short_runs"] = json.loads((scratch / "short-measurement.json").read_text())
(scratch / "measurement.json").write_text(json.dumps(result, indent=2) + "\n")
print("Saved", scratch / "measurement.json")
