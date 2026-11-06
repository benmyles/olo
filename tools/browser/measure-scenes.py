#!/usr/bin/python3
"""Frozen release scene/pipeline comparison on one allowed performance CPU.
Usage: OWNED_DIR OUTPUT.json [SCENE_ROUNDS=200]. Ordinary text: scene-baseline
(pre-control f1e462cc), scene-before (ec3fd83c), scene-after. Control text:
before/after only. Pipeline: page-baseline/page-current/page-after. All must
have default features and matching source/geometry (and scene raster) hashes.
Stop own builds and GUI checks before measuring. Excludes raster timing,
network, JavaScript, GPU and compositor presentation.
"""
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys

root, destination = map(Path, sys.argv[1:3])
assert root.is_absolute() and root.is_relative_to('/var/tmp')
rounds = int(sys.argv[3]) if len(sys.argv) > 3 else 200
assert rounds > 0
allowed = sorted(os.sched_getaffinity(0))


def frequency(cpu):
    p = Path(f'/sys/devices/system/cpu/cpu{cpu}/cpufreq/cpuinfo_max_freq')
    return int(p.read_text()) if p.exists() else 0


cpu = max(allowed, key=frequency)
os.sched_setaffinity(0, {cpu})
rows = []
names = set()
for mode, variants in [('plain', ['baseline', 'before', 'after']),
                       ('controls', ['before', 'after']),
                       ('pipeline', ['baseline', 'current', 'after'])]:
    for sample in range(7):
        order = variants[sample % len(variants):] + variants[:sample % len(variants)]
        if sample % 2:
            order = order[::-1]
        for variant in order:
            name = ('page-' if mode == 'pipeline' else 'scene-') + variant
            names.add(name)
            rss = root / f'{mode}-{variant}-{sample}.rss'
            args = [str(root / name), str(60 if mode == 'pipeline' else rounds)]
            if mode == 'controls':
                args.append('controls')
            data = json.loads(subprocess.check_output(['/usr/bin/time', '-f', '%M',
                '-o', str(rss), *args], text=True))
            assert data['features'] == []
            rows.append(dict(mode=mode, variant=variant, sample=sample,
                rss_kib=int(rss.read_text()), **data))
for mode in ['plain', 'controls', 'pipeline']:
    group = [r for r in rows if r['mode'] == mode]
    for observation in group:
        for row in observation['rows']:
            ref = next(r for r in group[0]['rows'] if r['elements'] == row['elements'])
            for key in ['source_sha256', 'geometry_sha256', 'commands'] + ([] if mode == 'pipeline' else ['pixels_sha256']):
                assert row[key] == ref[key], (mode, key)
    for variant in sorted({r['variant'] for r in group}):
        for elements in [100, 1000]:
            samples = [c['pipeline_us' if mode == 'pipeline' else 'scene_us']
                for r in group if r['variant'] == variant
                for c in r['rows'] if c['elements'] == elements]
            print(mode, variant, elements, statistics.median(samples), flush=True)
destination.write_text(json.dumps(dict(scope=__doc__, cpu=cpu, allowed_cpus=allowed,
    advertised_max_khz=frequency(cpu), baseline='f1e462cc', before='ec3fd83c',
    binaries={n: hashlib.file_digest((root / n).open('rb'), 'sha256').hexdigest()
        for n in sorted(names)}, observations=rows), indent=2) + '\n')
