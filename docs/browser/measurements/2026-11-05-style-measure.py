#!/usr/bin/python3
"""Measure frozen release binaries from an owned /var/tmp directory.

Build style-dense with only baseline-style-dense; style-final with default
features. Both use this commit's sources. The dense counterfactual recomputes
and stores every canonical property without default or adjacent-value sharing.
Build common examples against acfd0d8e (before logical projection), 4da00b03
(parent) and this commit, named REVISION-EXAMPLE and after-EXAMPLE. Use this
commit's identical example sources for all builds, including the raw query
example introduced after acfd0d8e; copy binaries with cp --reflink=always.
Run from repository root, without concurrent builds/tests. No source mutation.
No browser, font selection, layout, rendering or Chrome performance evidence.
"""
import hashlib
import json
import platform
import re
import statistics
import subprocess
import sys
from pathlib import Path

scratch = Path(sys.argv[1]).resolve()
assert scratch.is_relative_to('/var/tmp')
examples = dict(cascade_property_bench='30000000', cascade_bench='100',
                math_bench='20', properties_bench='20000', variables_bench='20',
                color_bench='20', selector_match_bench='200000')
names = ['style-dense', 'style-final'] + [f'{v}-{e}' for v in ['acfd0d8e', '4da00b03', 'after'] for e in examples]
result = dict(platform=platform.platform(), cpu=Path('/proc/cpuinfo').read_text().split('model name')[1].split('\n')[0].strip(),
              rustc=subprocess.check_output(['rustc', '--version', '--verbose'], text=True), affinity=0,
              parents=[subprocess.check_output(['git', 'rev-parse', r], text=True).strip() for r in ['acfd0d8e', '4da00b03']],
              method='Three alternating process runs per comparison on CPU0, release; per-process means, first calls separate. No concurrent builds/tests. Dense is a new-stage counterfactual, not a prior browser. RSS includes fixture DOM/rules and process/runtime/temporary accounting, not just retained styles.',
              hashes={n: hashlib.sha256((scratch/n).read_bytes()).hexdigest() for n in names}, source_hashes={}, runs=[])
paths = list(Path('crates/web/src/css/style').glob('*.rs')) + [Path(p) for p in [
    'crates/web/src/css/math/compute.rs', 'crates/web/src/css/color/compute.rs',
    'crates/web/src/css/properties/numeric.rs', 'crates/web/src/css/properties/registry.rs',
    'crates/web/src/css/properties/parse.rs', 'crates/web/src/css/cascade/mod.rs',
    'crates/web/examples/style_bench.rs', 'crates/web/tests/support/style_host.rs',
    'docs/browser/measurements/2026-11-05-style-measure.py']]
paths += [Path(f'crates/web/examples/{e}.rs') for e in examples]
result['source_hashes'] = {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in paths}

def run(name, args, **tags):
    log = scratch / 'rss.log'
    cmd = ['/usr/bin/time', '-v', '-o', str(log), 'taskset', '-c', '0', str(scratch/name), *args]
    data = json.loads(subprocess.check_output(cmd, text=True))
    rss = int(re.search(r'Maximum resident set size \(kbytes\): (\d+)', log.read_text())[1])
    result['runs'].append(dict(binary=name, args=args, maximum_rss_kib=rss, result=data, **tags))
    (scratch/'style-measurement.json').write_text(json.dumps(result, indent=2)+'\n')
    print(name, ' '.join(args), flush=True)

for repeat in range(3):
    for size in [1000, 10000]:
        for scene in ['bare', 'uniform', 'varying']:
            for variant in (['dense', 'final'] if repeat % 2 == 0 else ['final', 'dense']):
                run('style-'+variant, ['5', str(size), scene], repeat=repeat, benchmark='style', variant=variant)
    for example, rounds in examples.items():
        for variant in (['acfd0d8e', '4da00b03', 'after'] if repeat % 2 == 0 else ['after', '4da00b03', 'acfd0d8e']):
            run(variant+'-'+example, [rounds], repeat=repeat, benchmark=example, variant=variant)

summary = []
for size in [1000, 10000]:
    for scene in ['bare', 'uniform', 'varying']:
        row = dict(elements=size, scene=scene)
        for variant in ['dense', 'final']:
            runs = [r for r in result['runs'] if r['benchmark'] == 'style' and r['variant'] == variant and r['args'][1:] == [str(size), scene]]
            cases = [r['result']['cases'][0] for r in runs]
            row[variant] = {key: statistics.median(c[key] for c in cases) for key in cases[0] if key not in ['scene', 'elements']}
            row[variant]['maximum_rss_kib'] = statistics.median(r['maximum_rss_kib'] for r in runs)
        summary.append(row)
result['style_medians'] = summary
(scratch/'style-measurement.json').write_text(json.dumps(result, indent=2)+'\n')
print('Saved', scratch/'style-measurement.json')
