#!/usr/bin/python3
"""Replay from repository root with owned /var/tmp frozen release binaries.

Before: e89c5dea default; after: this commit default. Common examples:
style_bench,font_bench,font_native_bench,cascade_property_bench,cascade_bench.
New after-shaping_bench: default; control-shaping_bench: baseline-text-ascii only.
Build cargo build --release -p olo-web --example NAME [--features ...], copy
with cp --reflink=always to OWN-DIR/VARIANT-NAME. No builds/tests/owned GUI or
source mutations while measuring. Same-binary cache on/off is in shaping_bench.
"""
import hashlib,json,platform,re,statistics,subprocess,sys
from pathlib import Path
scratch=Path(sys.argv[1]).resolve();assert scratch.is_relative_to('/var/tmp')
common=['style_bench','font_bench','font_native_bench','cascade_property_bench','cascade_bench'];names=[f'{v}-{e}'for v in ['before','after']for e in common]+['after-shaping_bench','control-shaping_bench']
paths=[Path('crates/web/src/text.rs'),*Path('crates/web/src/text').rglob('*.rs'),*Path('crates/web/src/font').glob('*.rs'),Path('crates/web/Cargo.toml'),Path('Cargo.lock'),Path('Cargo.toml'),Path('crates/web/tests/shaping.rs'),Path('crates/web/tests/shaping-fixtures/SOURCE.json'),Path(__file__)]
paths += [Path(f'crates/web/examples/{e}.rs')for e in [*common,'shaping_bench']]
result=dict(parent=subprocess.check_output(['git','rev-parse','e89c5dea'],text=True).strip(),features={'before':[],'after':[],'control':['baseline-text-ascii']},platform=platform.platform(),cpu=Path('/proc/cpuinfo').read_text().split('model name')[1].split('\n')[0].strip(),rustc=subprocess.check_output(['rustc','--version','--verbose'],text=True),affinity=0,method='Three alternating CPU0 release processes, means per process, cold calls/max constructor and worker slices separate. Same-binary cache order alternated; same-facts full Unicode control disables text ASCII fast paths. Process RSS/CPU/scheduler counters retained. No concurrent builds/tests or owned GUI. Synthetic library work, no UI/Chrome/website speed claim.',hashes={n:hashlib.sha256((scratch/n).read_bytes()).hexdigest()for n in names},binary_bytes={n:(scratch/n).stat().st_size for n in names},source_hashes={str(p):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths},runs=[])
def run(binary,args,**tags):
 logpath=scratch/'time.log';data=json.loads(subprocess.check_output(['/usr/bin/time','-v','-o',str(logpath),'taskset','-c','0',str(scratch/binary),*args],text=True));log=logpath.read_text()
 def field(name):return re.search(r'^\s*'+re.escape(name)+r': (.+)$',log,re.M)[1]
 result['runs'].append(dict(binary=binary,args=args,maximum_rss_kib=int(field('Maximum resident set size (kbytes)')),user_seconds=float(field('User time (seconds)')),system_seconds=float(field('System time (seconds)')),voluntary_switches=int(field('Voluntary context switches')),involuntary_switches=int(field('Involuntary context switches')),result=data,**tags));(scratch/'measurement.json').write_text(json.dumps(result,indent=2)+'\n')
for repeat in range(3):
 variants=['before','after']if repeat%2==0 else['after','before']
 for size in [1000,10000]:
  for scene in ['bare','uniform','varying']:
   for variant in variants:run(variant+'-style_bench',['10',str(size),scene],repeat=repeat,benchmark='style',variant=variant)
 for name,args in [('font_bench',['10']),('font_native_bench',['10']),('cascade_property_bench',['30000000']),('cascade_bench',['100'])]:
  for variant in variants:run(variant+'-'+name,args,repeat=repeat,benchmark=name,variant=variant)
 for variant in (['after','control']if repeat%2==0 else['control','after']):run(variant+'-shaping_bench',['50','reverse'if repeat%2 else'normal'],repeat=repeat,benchmark='shaping',variant=variant)
 print('Completed release sample',repeat+1,flush=True)
summary=[]
for size in [1000,10000]:
 for scene in ['bare','uniform','varying']:
  row=dict(elements=size,scene=scene)
  for variant in ['before','after']:
   runs=[r for r in result['runs']if r['benchmark']=='style'and r['variant']==variant and r['args'][1:]==[str(size),scene]];cases=[r['result']['cases'][0]for r in runs]
   row[variant]={k:statistics.median(c[k]for c in cases)for k in cases[0]if k not in ['scene','elements']};row[variant]['maximum_rss_kib']=statistics.median(r['maximum_rss_kib']for r in runs)
  summary.append(row)
result['style_medians']=summary
for p,h in result['source_hashes'].items():assert hashlib.sha256(Path(p).read_bytes()).hexdigest()==h,p
for n,h in result['hashes'].items():assert hashlib.sha256((scratch/n).read_bytes()).hexdigest()==h,n
(scratch/'measurement.json').write_text(json.dumps(result,indent=2)+'\n');print('Saved',scratch/'measurement.json')
