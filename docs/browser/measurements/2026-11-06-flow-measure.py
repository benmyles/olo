#!/usr/bin/python3
"""Frozen parent/current release binaries on CPU0, three alternating processes.

Parent af8cbd5e includes origin/main merge, before DOM flow changes. Parent
web source was restored from HEAD for build and current work restored in a
finally block before any measurements. after-* default only. Flow variants:
retained; reprepare text on every resize; no-intern (cached_texts=0); both.
All variants must produce identical geometry/glyph hashes. No concurrent owned
builds/tests/GUI or source mutation. Raw wall/CPU/RSS/scheduler maxima retained;
no browser, GPU, pixels, frame/scroll or website acceptance claim.
"""
from pathlib import Path
import hashlib,json,platform,re,subprocess,sys,os,select,time
S=Path(sys.argv[1]).resolve();assert S.is_relative_to('/var/tmp')
common=['style_bench','shaping_bench','inline_bench'];names=[v+'-'+n for v in ['before','after']for n in common]+['after-flow_bench']
paths=[*Path('crates/web/src').rglob('*.rs'),Path('crates/web/Cargo.toml'),Path('Cargo.toml'),Path('Cargo.lock'),Path(__file__),Path('crates/web/tests/flow.rs'),Path('data/fonts/NotoSans[wght].ttf')]
paths += [p for p in Path('crates/web/tests/flow-fixtures').glob('*')if p.is_file()]+list(Path('crates/web/tests/support').glob('*.rs'))
paths += [Path('crates/web/examples/'+n+'.rs')for n in common+['flow_bench']]
result=dict(parent=json.loads((S/'parent-build.json').read_text()),platform=platform.platform(),cpu=Path('/proc/cpuinfo').read_text().split('model name')[1].split('\n')[0].strip(),rustc=subprocess.check_output(['rustc','--version','--verbose'],text=True),affinity=0,method=__doc__,features={'before':[],'after':[]},source_hashes={str(p):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths},binary_hashes={n:hashlib.sha256((S/n).read_bytes()).hexdigest()for n in names},binary_bytes={n:(S/n).stat().st_size for n in names},build_waits=[],runs=[])
def builds():
 found=[]
 for name in os.listdir('/proc'):
  if not name.isdigit():continue
  try:
   kind=Path('/proc/'+name+'/comm').read_text().strip()
   if kind not in ['cargo','rustc']:continue
   if kind=='cargo':
    args=Path('/proc/'+name+'/cmdline').read_bytes().split(b'\0')[1:5]
    if not any(a in [b'build',b'test',b'check',b'clippy',b'rustc',b'bench',b'run',b'doc']for a in args):continue
   found.append(dict(pid=int(name),kind=kind,cwd=os.readlink('/proc/'+name+'/cwd')))
  except OSError:pass
 return found
def wait_builds():
 while active:=builds():
  result['build_waits'].append(dict(at=time.time(),processes=active))
  print('Waiting for existing build processes',*[p['pid']for p in active],flush=True)
  fds=[]
  try:
   for p in active:
    try:fds.append(os.pidfd_open(p['pid']))
    except ProcessLookupError:pass
   if fds:select.select(fds,[],[],20)
  finally:
   for fd in fds:os.close(fd)
def run(variant,benchmark,args,repeat,**tags):
 wait_builds()
 name=('after'if benchmark=='flow'else variant)+'-'+benchmark+'_bench';log=S/'time.log'
 data=json.loads(subprocess.check_output(['/usr/bin/time','-v','-o',str(log),'taskset','-c','0',str(S/name),*args],text=True));raw=log.read_text()
 def field(s):return re.search(r'^\s*'+re.escape(s)+r': (.+)$',raw,re.M)[1]
 result['runs'].append(dict(builds_observed_after=builds(),binary=name,variant=variant,benchmark=benchmark,args=args,repeat=repeat,result=data,maximum_rss_kib=int(field('Maximum resident set size (kbytes)')),user_seconds=float(field('User time (seconds)')),system_seconds=float(field('System time (seconds)')),voluntary_switches=int(field('Voluntary context switches')),involuntary_switches=int(field('Involuntary context switches')),**tags));(S/'measurement.json').write_text(json.dumps(result,indent=2)+'\n')
for repeat in range(3):
 variants=['before','after']if repeat%2==0 else ['after','before']
 for scene in ['bare','uniform','varying']:
  for variant in variants:run(variant,'style',['10','10000',scene],repeat,scene=scene)
 for variant in variants:run(variant,'shaping',['50','reverse']if repeat%2 else ['50'],repeat)
 for variant in variants:run(variant,'inline',['100'],repeat)
 controls=[('retained',['10']),('reprepare',['10','reprepare']),('no-intern',['10','retained','no-intern']),('both',['10','reprepare','no-intern'])]
 for variant,args in (controls if repeat%2==0 else controls[::-1]):run(variant,'flow',args,repeat)
for repeat in range(3):
 rows=[r for r in result['runs']if r['benchmark']=='flow'and r['repeat']==repeat];default=next(r for r in rows if r['variant']=='retained')
 for r in rows:
  for a,b in zip(default['result']['cases'],r['result']['cases']):
   for key in ['scene','count','width','boxes','text_paragraphs','tree_payload_bytes','layout_geometry_payload_bytes','digest']:assert a[key]==b[key],(key,r['variant'],a,b)
for p,h in result['source_hashes'].items():assert hashlib.sha256(Path(p).read_bytes()).hexdigest()==h,p
assert all(not r['builds_observed_after']for r in result['runs']),'Build activity detected; retain this data as contaminated and repeat after quiescence.'
print(json.dumps({'processes':len(result['runs']),'output':str(S/'measurement.json'),'all_control_geometry_glyph_hashes_equal':True,'source_hashes_unchanged':True}))
