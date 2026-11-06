#!/usr/bin/python3
"""Frozen release library benchmarks, CPU0, three alternating processes.

Before flow/style executables are byte-identical to after executables in the
committed 2026-11-06-flow.json evidence. No parent source replacement needed.
After executables use default features. Indexed and linear scene queries must
agree; cached/uncached outline painting must produce identical tile hashes.
No owned GUI/build/test/source mutation during this library measurement.
CLI process RSS includes harness/fonts/all cases. It is not browser memory.
"""
from pathlib import Path
import hashlib,json,platform,re,subprocess,sys,os,select,time
S=Path(sys.argv[1]).resolve();assert S.is_relative_to('/var/tmp')
paths=[*Path('crates/web/src').rglob('*.rs'),Path('crates/web/Cargo.toml'),Path('Cargo.toml'),Path('Cargo.lock'),Path(__file__),*Path('crates/web/examples').glob('*_bench.rs'),*Path('crates/web/tests/support').glob('*.rs'),Path('data/fonts/NotoSans[wght].ttf')]
prior=json.loads(Path('docs/browser/measurements/2026-11-06-flow.json').read_text());names=['before-flow_bench','after-flow_bench','before-style_bench','after-style_bench','after-paint_bench']
binary_hashes={n:hashlib.sha256((S/n).read_bytes()).hexdigest()for n in names}
for n in ['flow_bench','style_bench']:assert binary_hashes['before-'+n]==prior['binary_hashes']['after-'+n]
result=dict(method=__doc__,platform=platform.platform(),rustc=subprocess.check_output(['rustc','--version','--verbose'],text=True),affinity=0,features={'before':[],'after':[]},parent_evidence='2026-11-06-flow.json',source_hashes={str(p):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths},binary_hashes=binary_hashes,build_waits=[],runs=[])
def builds():
 found=[]
 for name in os.listdir('/proc'):
  if not name.isdigit():continue
  try:
   kind=Path('/proc/'+name+'/comm').read_text().strip()
   if kind not in ['cargo','rustc']:continue
   found.append(dict(pid=int(name),kind=kind,cwd=os.readlink('/proc/'+name+'/cwd')))
  except OSError:pass
 return found
def wait_builds():
 while active:=builds():
  result['build_waits'].append(dict(at=time.time(),processes=active));print('Waiting for builds',*[p['pid']for p in active],flush=True)
  fds=[]
  try:
   for p in active:
    try:fds.append(os.pidfd_open(p['pid']))
    except ProcessLookupError:pass
   if fds:select.select(fds,[],[],20)
  finally:
   for fd in fds:os.close(fd)
def run(binary,args,repeat,**tags):
 wait_builds();log=S/'time.log';at=time.monotonic()
 data=json.loads(subprocess.check_output(['/usr/bin/time','-v','-o',str(log),'taskset','-c','0',str(S/binary),*args],text=True));wall=time.monotonic()-at;raw=log.read_text()
 def field(s):return re.search(r'^\s*'+re.escape(s)+r': (.+)$',raw,re.M)[1]
 result['runs'].append(dict(binary=binary,args=args,repeat=repeat,result=data,wall_seconds=wall,maximum_rss_kib=int(field('Maximum resident set size (kbytes)')),user_seconds=float(field('User time (seconds)')),system_seconds=float(field('System time (seconds)')),involuntary_switches=int(field('Involuntary context switches')),builds_observed_after=builds(),**tags));(S/'measurement.json').write_text(json.dumps(result,indent=2)+'\n');print(binary,args,repeat,flush=True)
for repeat in range(3):
 order=['before','after']if repeat%2==0 else ['after','before']
 for variant in order:run(variant+'-flow_bench',['10'],repeat,benchmark='flow',variant=variant)
 for scene in ['bare','uniform','varying']:
  for variant in order:run(variant+'-style_bench',['10','10000',scene],repeat,benchmark='style',variant=variant,scene=scene)
 for variant in (['cached','uncached']if repeat%2==0 else ['uncached','cached']):run('after-paint_bench',['30']+(['uncached']if variant=='uncached'else[]),repeat,benchmark='paint',variant=variant)
for repeat in range(3):
 for benchmark in ['flow','paint']:
  rows=[r for r in result['runs']if r['benchmark']==benchmark and r['repeat']==repeat]
  for a,b in zip(rows[0]['result']['cases'],rows[1]['result']['cases']):assert a['digest'if benchmark=='flow'else'tile_sha256']==b['digest'if benchmark=='flow'else'tile_sha256']
for p,h in result['source_hashes'].items():assert hashlib.sha256(Path(p).read_bytes()).hexdigest()==h,p
assert all(not r['builds_observed_after']for r in result['runs']),'Build activity observed; retain and repeat uncontaminated.'
print(json.dumps(dict(processes=len(result['runs']),hashes_equal=True,source_hashes_unchanged=True)))
