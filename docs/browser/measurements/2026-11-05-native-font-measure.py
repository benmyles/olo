#!/usr/bin/python3
"""Replay from repository root; argument is an owned /var/tmp binary directory.

Freeze before-* examples from 239cdb4b, default features. Freeze after-* examples
from this commit, default features. cargo build --release -p olo-web --example
NAME; cp --reflink=always target/release/examples/NAME OWN-DIR/VARIANT-NAME.
Common names: style_bench,font_bench,cascade_property_bench,cascade_bench.
New after-only example: font_native_bench. No mutation/build/tests/GUI while
this runner measures. Native cache on/off control is in the same new binary.
"""
import hashlib
import json
import platform
import re
import statistics
import subprocess
import sys
from pathlib import Path

scratch=Path(sys.argv[1]).resolve()
assert scratch.is_relative_to('/var/tmp')
names=[f'{v}-{e}' for v in ['before','after'] for e in ['style_bench','font_bench','cascade_property_bench','cascade_bench']]+['after-font_native_bench']
paths=[*Path('crates/web/src/font').glob('*.rs'),*Path('crates/web/src/css/style').glob('*.rs')]
paths += [Path(f'crates/web/examples/{n}.rs') for n in ['style_bench','font_bench','font_native_bench','cascade_property_bench','cascade_bench']]
paths += [*Path('crates/web/tests/support').glob('*host.rs'),Path('crates/web/tests/font-native-fixtures/SOURCE.json'),Path(__file__),Path('Cargo.toml'),Path('Cargo.lock'),Path('crates/web/Cargo.toml')]
result=dict(parent=subprocess.check_output(['git','rev-parse','239cdb4b'],text=True).strip(),features=[],platform=platform.platform(),cpu=Path('/proc/cpuinfo').read_text().split('model name')[1].split('\n')[0].strip(),rustc=subprocess.check_output(['rustc','--version','--verbose'],text=True),affinity=0,method='Three alternating release processes for common operations on CPU0, means per process, medians over processes. Native metrics use cache_entries=0 vs enabled in the same binary; first calls/max worker slices recorded separately. No concurrent builds/tests or GUI. Synthetic library costs, no browser/Chrome performance claim.',hashes={n:hashlib.sha256((scratch/n).read_bytes()).hexdigest() for n in names},source_hashes={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in paths},runs=[])

def run(binary,args,**tags):
    timing=scratch/'native-time.log'
    data=json.loads(subprocess.check_output(['/usr/bin/time','-v','-o',str(timing),'taskset','-c','0',str(scratch/binary),*args],text=True))
    rss=int(re.search(r'Maximum resident set size \(kbytes\): (\d+)',timing.read_text())[1])
    result['runs'].append(dict(binary=binary,args=args,maximum_rss_kib=rss,result=data,**tags))
    (scratch/'native-measurement.json').write_text(json.dumps(result,indent=2)+'\n')
for repeat in range(3):
    variants=['before','after'] if repeat%2==0 else ['after','before']
    for size in [1000,10000]:
        for scene in ['bare','uniform','varying']:
            for variant in variants:run(variant+'-style_bench',['10',str(size),scene],repeat=repeat,benchmark='style',variant=variant)
    for name,args in [('font_bench',['10']),('cascade_property_bench',['30000000']),('cascade_bench',['100'])]:
        for variant in variants:run(variant+'-'+name,args,repeat=repeat,benchmark=name,variant=variant)
    run('after-font_native_bench',['10'],repeat=repeat,benchmark='native',variant='after')
    print('Completed release sample',repeat+1,flush=True)

summary=[]
for size in [1000,10000]:
    for scene in ['bare','uniform','varying']:
        row=dict(elements=size,scene=scene)
        for variant in ['before','after']:
            runs=[r for r in result['runs'] if r['benchmark']=='style' and r['variant']==variant and r['args'][1:]==[str(size),scene]]
            cases=[r['result']['cases'][0] for r in runs]
            row[variant]={k:statistics.median(c[k] for c in cases) for k in cases[0] if k not in ['scene','elements']}
            row[variant]['maximum_rss_kib']=statistics.median(r['maximum_rss_kib'] for r in runs)
        summary.append(row)
result['style_medians']=summary
for p,h in result['source_hashes'].items():
    assert hashlib.sha256(Path(p).read_bytes()).hexdigest()==h,p
for n,h in result['hashes'].items():
    assert hashlib.sha256((scratch/n).read_bytes()).hexdigest()==h,n
(scratch/'native-measurement.json').write_text(json.dumps(result,indent=2)+'\n')
print('Saved',scratch/'native-measurement.json')
