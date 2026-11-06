#!/usr/bin/python3
"""Run from repository root with the owned /var/tmp directory of frozen binaries.

Build before-* at parent f1513b47, default features, cargo build --release -p
olo-web --example NAME. Build after-* from this commit's default sources.
Examples: style_bench and the seven common examples below; font_bench is new,
after only. Copy with cp --reflink=always; no source mutation in this runner.
Build control-general-{cascade_property_bench,style_bench} with only
baseline-cascade-value-copy; control-fat-{font_bench,style_bench} with only
baseline-font-family-fat. Both controls otherwise use final default sources.
Common examples are unchanged except style_bench's added families_bytes output
and its test host's borrowed richer request; supplied metric numbers unchanged.
No concurrent builds/tests or GUI sessions during measurement. No real font
loading/matching, layout, UI, website or Chrome performance evidence.
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
names = [f'{v}-{e}' for v in ['before', 'after'] for e in ['style_bench', *examples]] + ['after-font_bench']
names += ['control-general-cascade_property_bench', 'control-general-style_bench', 'control-fat-font_bench', 'control-fat-style_bench']
result = dict(platform=platform.platform(), cpu=Path('/proc/cpuinfo').read_text().split('model name')[1].split('\n')[0].strip(),
              rustc=subprocess.check_output(['rustc', '--version', '--verbose'], text=True), affinity=0,
              parent=subprocess.check_output(['git', 'rev-parse', 'f1513b47'], text=True).strip(),
              features={'before': [], 'after': [], 'control-general': ['baseline-cascade-value-copy'], 'control-fat': ['baseline-font-family-fat']},
              method='Three alternating release process runs per comparison on CPU0; process means, cold calls and maximum observed worker steps separate. No concurrent builds/tests or GUI. Fixed supplied metrics, no actual font selection. Payload differs from separate whole-process RSS. Timings are synthetic library costs, not Chrome/browser performance.',
              hashes={n: hashlib.sha256((scratch/n).read_bytes()).hexdigest() for n in names}, source_hashes={}, before_source_hashes={}, runs=[])
paths = [*Path('crates/web/src/css/style').glob('*.rs'), *Path('crates/web/src/css/properties').glob('*.rs')]
paths += [Path(f'crates/web/examples/{e}.rs') for e in ['style_bench', 'font_bench', *examples]]
paths += [Path('crates/web/tests/support/style_host.rs'), Path('docs/browser/measurements/2026-11-05-font-measure.py')]
paths += [Path('crates/web/src/css/cascade/mod.rs'), Path('Cargo.toml'), Path('Cargo.lock'), Path('crates/web/Cargo.toml')]
for p in paths:
    result['source_hashes'][str(p)] = hashlib.sha256(p.read_bytes()).hexdigest()
    previous = subprocess.run(['git', 'show', f'f1513b47:{p}'], capture_output=True)
    result['before_source_hashes'][str(p)] = hashlib.sha256(previous.stdout).hexdigest() if previous.returncode == 0 else None

def run(name, args, **tags):
    log = scratch/'rss.log'
    command = ['/usr/bin/time', '-v', '-o', str(log), 'taskset', '-c', '0', str(scratch/name), *args]
    data = json.loads(subprocess.check_output(command, text=True))
    rss = int(re.search(r'Maximum resident set size \(kbytes\): (\d+)', log.read_text())[1])
    result['runs'].append(dict(binary=name, args=args, maximum_rss_kib=rss, result=data, **tags))
    (scratch/'measurement.json').write_text(json.dumps(result, indent=2)+'\n')
    print(name, ' '.join(args), flush=True)

for repeat in range(3):
    variants = ['before', 'after'] if repeat % 2 == 0 else ['after', 'before']
    for size in [1000, 10000]:
        for scene in ['bare', 'uniform', 'varying']:
            for variant in variants:
                run(variant+'-style_bench', ['10', str(size), scene], repeat=repeat, benchmark='style', variant=variant)
    for example, rounds in examples.items():
        for variant in variants:
            run(variant+'-'+example, [rounds], repeat=repeat, benchmark=example, variant=variant)
    run('after-font_bench', ['10'], repeat=repeat, benchmark='font', variant='after')
    run('control-general-cascade_property_bench', ['30000000'], repeat=repeat, benchmark='cascade_property_bench', variant='control-general')
    for scene in ['bare', 'uniform', 'varying']:
        for variant in ['control-general', 'control-fat']:
            run(variant+'-style_bench', ['10', '10000', scene], repeat=repeat, benchmark='style', variant=variant)
    run('control-fat-font_bench', ['10'], repeat=repeat, benchmark='font', variant='control-fat')

summary = []
for size in [1000, 10000]:
    for scene in ['bare', 'uniform', 'varying']:
        row = dict(elements=size, scene=scene)
        for variant in ['before', 'after']:
            runs = [r for r in result['runs'] if r['benchmark']=='style' and r['variant']==variant and r['args'][1:]==[str(size), scene]]
            cases = [r['result']['cases'][0] for r in runs]
            row[variant] = {key: statistics.median(c[key] for c in cases) for key in cases[0] if key not in ['scene', 'elements']}
            row[variant]['maximum_rss_kib'] = statistics.median(r['maximum_rss_kib'] for r in runs)
        summary.append(row)
result['style_medians'] = summary
# Exploratory unpublished drafts are evidence of the investigation, not exact
# checkoutable baselines. Reproduce the final comparisons with the two features.
initial = scratch/'initial-derived-measurement.json'
if initial.exists():
    result['exploratory_unpublished_draft'] = dict(scope='Initial nine-variant property enum, fat Arc family lists, general derived candidate clones. Unpublished draft; source hashes/raw runs retained, not an exact checkoutable revision. Final controls above are reproducible.', result=json.loads(initial.read_text()))
experiments = {}
for label in ['inline', 'compact', 'scalar', 'direct', 'thin', 'range', 'general']:
    p = scratch/(label+'-experiment.json')
    if p.exists():
        experiments[label] = json.loads(p.read_text())
result['exploratory_clone_investigation'] = experiments
(scratch/'measurement.json').write_text(json.dumps(result, indent=2)+'\n')
print('Saved', scratch/'measurement.json')
