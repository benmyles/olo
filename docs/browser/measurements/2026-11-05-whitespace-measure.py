#!/usr/bin/python3
"""Replay CPU0 frozen release examples from an owned /var/tmp directory.

Parent f111348a: before-{style_bench,cascade_property_bench,shaping_bench}.
Current default: after-{those examples,whitespace_bench}.
scalar-whitespace_bench: baseline-text-prepare only.
wide-whitespace_bench: baseline-text-map-wide only.
Build/copy --reflink=always before running. No builds/tests/source mutations
or owned GUI run concurrently with this measurement. Synthetic worker/library
work; no UI latency, rendering, websites or Chrome performance claim.
"""
import hashlib,json,platform,re,subprocess,sys
from pathlib import Path
scratch=Path(sys.argv[1]).resolve();assert scratch.is_relative_to('/var/tmp')
common=['style_bench','cascade_property_bench','shaping_bench']
names=[v+'-'+e for v in ['before','after']for e in common]+[v+'-whitespace_bench'for v in ['after','scalar','wide']]
paths=[*Path('crates/web/src').rglob('*.rs'),Path('crates/web/Cargo.toml'),Path('Cargo.lock'),Path('Cargo.toml'),Path(__file__)]
paths += [Path('crates/web/tests/whitespace-fixtures/SOURCE.json'),Path('crates/web/tests/whitespace.rs'),Path('crates/web/tests/shaping.rs'),Path('crates/web/tests/style.rs')]
paths += [Path(f'crates/web/examples/{e}.rs')for e in common+['whitespace_bench','whitespace_probe']]
result=dict(parent=subprocess.check_output(['git','rev-parse','f111348a'],text=True).strip(),platform=platform.platform(),cpu=Path('/proc/cpuinfo').read_text().split('model name')[1].split('\n')[0].strip(),rustc=subprocess.check_output(['rustc','--version','--verbose'],text=True),affinity=0,
 features={'before':[],'after':[],'scalar':['baseline-text-prepare'],'wide':['baseline-text-map-wide']},
 method='Three alternating frozen CPU0 release processes; repeated build/drop means and cold/constructor/max 4096-byte worker slice observations separate. RSS/CPU/scheduler counters retained. Same-semantics scalar scan and wide map controls. No concurrent builds/tests/owned GUI or source mutations.',
 hashes={n:hashlib.sha256((scratch/n).read_bytes()).hexdigest()for n in names},binary_bytes={n:(scratch/n).stat().st_size for n in names},source_hashes={str(p):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths},runs=[])
def run(binary,args,**tags):
 logpath=scratch/'time.log';data=json.loads(subprocess.check_output(['/usr/bin/time','-v','-o',str(logpath),'taskset','-c','0',str(scratch/binary),*args],text=True));log=logpath.read_text()
 def field(name):return re.search(r'^\s*'+re.escape(name)+r': (.+)$',log,re.M)[1]
 result['runs'].append(dict(binary=binary,args=args,maximum_rss_kib=int(field('Maximum resident set size (kbytes)')),user_seconds=float(field('User time (seconds)')),system_seconds=float(field('System time (seconds)')),voluntary_switches=int(field('Voluntary context switches')),involuntary_switches=int(field('Involuntary context switches')),result=data,**tags))
 (scratch/'measurement.json').write_text(json.dumps(result,indent=2)+'\n')
for repeat in range(3):
 variants=['before','after']if repeat%2==0 else['after','before']
 for size in [1000,10000]:
  for scene in ['bare','uniform','varying']:
   for variant in variants:run(variant+'-style_bench',['10',str(size),scene],repeat=repeat,variant=variant,benchmark='style')
 for benchmark,args in [('cascade_property_bench',['30000000']),('shaping_bench',['50','reverse']if repeat%2 else['50'])]:
  for variant in variants:run(variant+'-'+benchmark,args,repeat=repeat,variant=variant,benchmark=benchmark)
 for variant in (['after','scalar','wide']if repeat%2==0 else['wide','scalar','after']):
  run(variant+'-whitespace_bench',['500'],repeat=repeat,variant=variant,benchmark='whitespace')
print(json.dumps({'runs':len(result['runs']),'output':str(scratch/'measurement.json')}))
