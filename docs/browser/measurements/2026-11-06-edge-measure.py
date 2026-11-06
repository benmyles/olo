#!/usr/bin/python3
"""Frozen parent/current CPU0 release measurements, three alternating runs.
Parent paint runtime from db9910ba, current expanded benchmark inputs. Solid,
multicolour and translucent borders; cached and uncached outlines; common flow.
Input/geometry hashes must agree across revisions; cached/uncached output
must agree within a revision. Pixel hashes may change between revisions:
rectangle snapping, corner blending and painted baseline placement are fixes.
No owned GUI/build/test/source mutation during official measurement.
"""
from pathlib import Path
import hashlib,json,platform,re,subprocess,sys,os,select,time
S=Path(sys.argv[1]).resolve();assert S.is_relative_to('/var/tmp')
paths=[*Path('crates/web/src').rglob('*.rs'),Path('crates/web/Cargo.toml'),Path('Cargo.toml'),Path('Cargo.lock'),Path(__file__),Path('crates/web/examples/paint_bench.rs'),Path('crates/web/examples/flow_bench.rs'),Path('crates/web/examples/support/features.rs'),*Path('crates/web/tests/support').glob('*.rs'),Path('data/fonts/NotoSans[wght].ttf')]
names=['parent-paint_bench','after-paint_bench','before-flow_bench','after-flow_bench']
result=dict(method=__doc__,parent=json.loads((S/'parent-build.json').read_text()),platform=platform.platform(),rustc=subprocess.check_output(['rustc','--version','--verbose'],text=True),affinity=0,features=[],source_hashes={str(p):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths},binary_hashes={n:hashlib.sha256((S/n).read_bytes()).hexdigest()for n in names},build_waits=[],runs=[])
def builds():
 found=[]
 for name in os.listdir('/proc'):
  if not name.isdigit():continue
  try:
   kind=Path('/proc/'+name+'/comm').read_text().strip()
   if kind in ['cargo','rustc']:found.append(dict(pid=int(name),kind=kind,cwd=os.readlink('/proc/'+name+'/cwd')))
  except OSError:pass
 return found
def wait_builds():
 while active:=builds():
  result['build_waits'].append(dict(at=time.time(),processes=active));print('Waiting for builds',*[p['pid']for p in active],flush=True);fds=[]
  try:
   for p in active:
    try:fds.append(os.pidfd_open(p['pid']))
    except ProcessLookupError:pass
   if fds:select.select(fds,[],[],20)
  finally:
   for fd in fds:os.close(fd)
def run(binary,args,repeat,**tags):
 wait_builds();log=S/'time.log';at=time.monotonic();data=json.loads(subprocess.check_output(['/usr/bin/time','-v','-o',str(log),'taskset','-c','0',str(S/binary),*args],text=True));wall=time.monotonic()-at;raw=log.read_text();assert data['features']==[],data['features']
 def field(s):return re.search(r'^\s*'+re.escape(s)+r': (.+)$',raw,re.M)[1]
 result['runs'].append(dict(binary=binary,args=args,repeat=repeat,result=data,wall_seconds=wall,maximum_rss_kib=int(field('Maximum resident set size (kbytes)')),user_seconds=float(field('User time (seconds)')),system_seconds=float(field('System time (seconds)')),involuntary_switches=int(field('Involuntary context switches')),builds_observed_after=builds(),**tags));(S/'edge-measurement.json').write_text(json.dumps(result,indent=2)+'\n');print(binary,args,repeat,flush=True)
for repeat in range(3):
 order=['parent','after']if repeat%2==0 else ['after','parent']
 for mode in ['solid','color','alpha']:
  for cache in ['cached','uncached']:
   for variant in order:run(variant+'-paint_bench',['30',cache,mode],repeat,benchmark='paint',variant=variant,mode=mode,cache=cache)
 for variant in order:run(('before'if variant=='parent'else'after')+'-flow_bench',['10'],repeat,benchmark='flow',variant=variant)
for repeat in range(3):
 rows=[r for r in result['runs']if r['benchmark']=='flow'and r['repeat']==repeat]
 for a,b in zip(rows[0]['result']['cases'],rows[1]['result']['cases']):assert a['digest']==b['digest']
 for mode in ['solid','color','alpha']:
  rows=[r for r in result['runs']if r['benchmark']=='paint'and r['repeat']==repeat and r['mode']==mode]
  for row in rows[1:]:
   for a,b in zip(rows[0]['result']['cases'],row['result']['cases']):assert (a['paragraphs'],a['scale'],a['geometry_sha256'])==(b['paragraphs'],b['scale'],b['geometry_sha256'])
  for variant in ['parent','after']:
   pair=[r for r in rows if r['variant']==variant]
   for a,b in zip(pair[0]['result']['cases'],pair[1]['result']['cases']):assert a['tile_sha256']==b['tile_sha256']
for p,h in result['source_hashes'].items():assert hashlib.sha256(Path(p).read_bytes()).hexdigest()==h,p
assert all(not r['builds_observed_after']for r in result['runs']),'Build activity observed; preserve and repeat uncontaminated.'
print(json.dumps(dict(processes=len(result['runs']),geometry_and_control_hashes_equal=True,source_hashes_unchanged=True)))
