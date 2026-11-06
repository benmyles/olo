#!/usr/bin/python3
"""Pair frozen phase-instrumented and uninstrumented worker executables.
Usage: OWNED_DIR OUTPUT.json [ROUNDS=200]. Requires profile-baseline,
profile-current, page-baseline and page-current, built with matching default
release profiles. Selects one allowed CPU with the highest advertised maximum
frequency; descendants inherit affinity. Stop own builds/GUI checks first.
Phase instrumentation changes code generation: confirm with the ordinary
page benchmark. No raster, GPU, network, JavaScript or presentation coverage.
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
    path = Path(f'/sys/devices/system/cpu/cpu{cpu}/cpufreq/cpuinfo_max_freq')
    return int(path.read_text()) if path.exists() else 0


cpu = max(allowed, key=frequency)
os.sched_setaffinity(0, {cpu})
observations = []
for mode, count in [('profile', rounds), ('page', 60)]:
    for sample in range(7):
        variants = ['baseline', 'current'] if sample % 2 == 0 else ['current', 'baseline']
        for variant in variants:
            binary = root / f'{mode}-{variant}'
            rss = root / f'{mode}-{variant}-{sample}.rss'
            data = json.loads(subprocess.check_output(['/usr/bin/time', '-f', '%M',
                '-o', str(rss), str(binary), str(count)], text=True))
            assert data['features'] == []
            observations.append(dict(mode=mode, variant=variant, sample=sample,
                rss_kib=int(rss.read_text()), **data))
for observation in observations:
    for row in observation['rows']:
        reference = next(r for r in observations[0]['rows'] if r['elements'] == row['elements'])
        for key in ['source_sha256', 'geometry_sha256', 'commands']:
            assert row[key] == reference[key], (observation['mode'], key)
result = dict(scope=__doc__, baseline='f1e462cc', current='ec3fd83c', cpu=cpu,
    allowed_cpus=allowed, advertised_max_khz=frequency(cpu),
    binaries={p.name: hashlib.file_digest(p.open('rb'), 'sha256').hexdigest()
        for p in [root / (m + '-' + v) for m in ['profile', 'page']
            for v in ['baseline', 'current']]}, observations=observations)
destination.write_text(json.dumps(result, indent=2) + '\n')
for count in [100, 1000]:
    for mode in ['profile', 'page']:
        selected = [r for r in observations if r['mode'] == mode]
        fields = ['html', 'styles', 'boxes', 'text', 'layout', 'paint', 'dispose'] if mode == 'profile' else ['pipeline_us']
        for field in fields:
            values = {}
            for variant in ['baseline', 'current']:
                rows = [c for r in selected if r['variant'] == variant
                    for c in r['rows'] if c['elements'] == count]
                values[variant] = statistics.median(c['phase_us'][field] if mode == 'profile' else c[field] for c in rows)
            print(count, mode, field, values, flush=True)
