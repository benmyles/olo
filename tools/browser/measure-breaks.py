#!/usr/bin/python3
"""Frozen release worker pipeline measurements. No GUI actions during this run.
Args: /var/tmp/OWNED_DIR OUTPUT.json. Needs page-before/page-after/break-bench.
Common cases must retain source/geometry identities and have no baseline flags.
New semantic-break workload has no passing before counterpart. Excludes raster,
GPU and presentation. Stop own builds and active GUI tests before measuring.
"""
import hashlib
import json
from pathlib import Path
import statistics
import subprocess
import sys

root, destination = map(Path, sys.argv[1:])
assert root.is_absolute() and root.is_relative_to('/var/tmp')


def measure(binary, label, sample):
    rss = root / f'{label}-{sample}.rss'
    data = json.loads(subprocess.check_output(['/usr/bin/time', '-f', '%M', '-o',
        str(rss), str(binary), '60'], text=True))
    assert data['features'] == []
    return {'sample': sample, 'rss_kib': int(rss.read_text()), **data}


common = []
for sample in range(7):
    for variant in (['before', 'after'] if sample % 2 == 0 else ['after', 'before']):
        common.append({'variant': variant, **measure(root / f'page-{variant}', variant, sample)})
for observation in common:
    assert observation['command_bytes'] == 80
    counterpart = next(r for r in common if r['variant'] != observation['variant'])
    for row in observation['rows']:
        other = next(r for r in counterpart['rows'] if r['elements'] == row['elements'])
        for key in ['commands', 'source_sha256', 'geometry_sha256']:
            assert row[key] == other[key]
new = [measure(root / 'break-bench', 'breaks', sample) for sample in range(7)]
result = {'scope': __doc__, 'baseline': 'f1e462cc76f8b6bc0dc0bec36b7bf05b48def46a',
    'binary_sha256': {name: hashlib.file_digest((root / name).open('rb'), 'sha256').hexdigest()
                      for name in ['page-before', 'page-after', 'break-bench']},
    'common': common, 'breaks': new}
Path(destination).write_text(json.dumps(result, indent=2) + '\n')
for variant in ['before', 'after']:
    group = [r for r in common if r['variant'] == variant]
    print(variant, [(n, statistics.median(x['pipeline_us'] for r in group for x in r['rows']
        if x['elements'] == n)) for n in [100, 1000]])
print('breaks', [(n, statistics.median(x['pipeline_us'] for r in new for x in r['rows']
    if x['elements'] == n)) for n in [100, 1000]])
