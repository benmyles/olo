"""Check retained evidence; this does not accept a site or rerun live tests."""
import hashlib
import json
import re
import struct
from collections import Counter
from pathlib import Path

from PIL import Image

root = Path(__file__).resolve().parent
repo = root.parents[3]


def read(name):
    return json.loads((root / name).read_text())


for name, digest in read("source-hashes.json").items():
    assert hashlib.sha256((repo / name).read_bytes()).hexdigest() == digest, name
for name, digest in read("artifact-hashes.json").items():
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == digest, name

expected = {(mode, scale) for mode in ["light", "dark", "night"] for scale in [1, 1.25, 1.5, 1.75, 2]}
for variant in ["before", "after-svg-only", "after"]:
    rows = read(f"{variant}/results.json")
    assert len(rows) == 15
    assert {(r["mode"], r["scale"]) for r in rows} == expected
    for row in rows:
        a, b = row["native"], row["chrome"]
        for key in ["experiments", "viewport", "client", "active", "dpr"]:
            assert a[key] == b[key], (variant, key, row["mode"], row["scale"])
        # Chrome exposes these fractional offsets with float32 precision.
        assert struct.pack("f", a["offset"]) == struct.pack("f", b["offset"])
        assert row["review"] and all(not w["findings"] for w in row["review"])

colors = read("selection-colors.json")
assert len(colors) == 45
for row in colors:
    scale = row["scale"]
    for engine, shift in [("native", 0), ("chrome", 12)]:
        name = f'{row["variant"]}/{row["mode"]}-{scale}-{engine}.png'
        im = Image.open(root / name).convert("RGB")
        assert abs(im.width - 1010 * scale) < 2
        crop = im.crop(tuple(round(n * scale) for n in [315, 515 + shift, 460, 585 + shift]))
        counts = Counter(crop.getdata())
        assert row["colors"][engine] == {"jobber_active_pixels": counts[(1, 41, 57)], "inactive_pixels": counts[(149, 164, 186)]}
    assert row["active"] == (row["colors"]["native"]["jobber_active_pixels"] > 20 * scale * scale)
assert sum(r["active"] for r in colors if r["variant"] == "after") == 15

journey = read("carousel-final.json")
assert {r["company"] for r in journey} == {"Mindbody", "Jobber", "Substack", "Lightspeed"}
assert all(r["native"] == r["chrome"] for r in journey)

costs = read("costs.json")
assert len(costs) == 8
for scale in [1.25, 2]:
    assert [r["variant"] for r in costs if r["scale"] == scale] == ["before", "after", "after", "before"]
for row in costs:
    assert row["sha256"] == read("binary-identities.json")["sha256"][row["variant"]]
    assert row["bench_ms"]["done"]
    assert len(row["resize_ms"]) == 8
    assert row["idle"]["total"]["cpu"] == 0
    assert all(w["frames"] == 0 for w in row["idle"]["windows"])
retention = read("retention.json")
assert [r["variant"] for r in retention] == ["before", "after"]
for row in retention:
    assert [s["frame"] for s in row["samples"]] == list(range(100, 1201, 100))
    assert row["idle"]["total"]["cpu"] == 0
    assert all(w["frames"] == 0 for w in row["idle"]["windows"])

validation = read("validation.json")
assert validation["build"]["exit"] == validation["test"]["exit"] == 0
log = (root / "test-workspace.log").read_text()
results = re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;", log)
assert len(results) == validation["test"]["suites"] == 262
assert [sum(int(r[i]) for r in results) for i in range(3)] == [2273, 0, 140]
assert "test result: FAILED" not in log
print("Evidence verified: 15 final selections, 4 controls, 8 ABBA samples, 2 retention runs; workspace 2273/0/140. Stripe remains 0/30.")
