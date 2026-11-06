#!/usr/bin/python3
"""Owned /var/tmp frozen release binaries, parent 858b2818 vs current.

before-/after-{style_bench,shaping_bench,whitespace_bench,cascade_property_bench}
after-linebreak_bench: default; scalar-linebreak_bench: baseline-linebreak-ascii
only; no-runs-linebreak_bench: baseline-linebreak-runs only; class-table-
linebreak_bench: baseline-linebreak-class-table only. Build/copy all
binaries before replay. No concurrent builds/tests/owned GUI/source edits.
"""
import hashlib,json,platform,re,subprocess,sys
from pathlib import Path
scratch=Path(sys.argv[1]).resolve();assert scratch.is_relative_to('/var/tmp')
common=['style_bench','shaping_bench','whitespace_bench','cascade_property_bench']
names=[v+'-'+e for v in ['before','after']for e in common]+[v+'-linebreak_bench'for v in ['after','scalar','no-runs','class-table']]
paths=[*Path('crates/web/src').rglob('*.rs'),Path('crates/web/Cargo.toml'),Path('Cargo.toml'),Path('Cargo.lock'),Path(__file__),Path('tools/browser/generate-linebreak-unicode.py'),Path('crates/web/tests/linebreak.rs'),Path('crates/web/tests/linebreak-fixtures/SOURCE.json')]
paths +=[Path('crates/web/examples/'+n+'.rs')for n in common+['linebreak_bench']]
result=dict(parent=subprocess.check_output(['git','rev-parse','858b2818'],text=True).strip(),platform=platform.platform(),cpu=Path('/proc/cpuinfo').read_text().split('model name')[1].split('\n')[0].strip(),rustc=subprocess.check_output(['rustc','--version','--verbose'],text=True),affinity=0,
 features={'before':[],'after':[],'scalar':['baseline-linebreak-ascii'],'no-runs':['baseline-linebreak-runs'],'class-table':['baseline-linebreak-class-table']},method='Three alternating CPU0 release processes. Independent vectors checked outside timing. Default/full-rule/no-bulk/table-decode controls preserve Unicode facts and opportunities. Means, cold calls, max constructors/4096-byte steps, payload, whole-process RSS/CPU/scheduler counters retained. No concurrent builds/tests/owned GUI or source mutation. No CSS/layout/browser speed claim.',
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
 for benchmark,args in [('shaping_bench',['50','reverse']if repeat%2 else['50']),('whitespace_bench',['100']),('cascade_property_bench',['30000000'])]:
  for variant in variants:run(variant+'-'+benchmark,args,repeat=repeat,variant=variant,benchmark=benchmark)
 for variant in (['after','scalar','no-runs','class-table']if repeat%2==0 else['class-table','no-runs','scalar','after']):
  run(variant+'-linebreak_bench',['100'],repeat=repeat,variant=variant,benchmark='linebreak')
print(json.dumps({'runs':len(result['runs']),'output':str(scratch/'measurement.json')}))
