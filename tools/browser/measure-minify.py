#!/usr/bin/python3
"""Frozen release minification controls on one performance CPU.

Usage: /var/tmp/OWNED OUTPUT.json. Expects image-before/image-after/image-scalar
and page-before/after, scene-before/after. Before is the f220336a renderer.
Seven rotated processes; stop own builds/screens first. Common old/new image
operations must agree byte-for-byte; new SIMD/scalar mip operations must too.
Image process RSS differs in workload from the old executable (new mip cases);
only new default/scalar RSS is matched. No network/codec/GPU/presentation scope.
"""
import hashlib,json,os,platform,select,statistics,subprocess,sys,time
from pathlib import Path
root=Path(sys.argv[1]).resolve();assert root.is_relative_to('/var/tmp')
destination=Path(sys.argv[2]);allowed=sorted(os.sched_getaffinity(0))
def freq(c):
 p=Path(f'/sys/devices/system/cpu/cpu{c}/cpufreq/cpuinfo_max_freq');return int(p.read_text()) if p.exists() else 0
cpu=max(allowed,key=freq);os.sched_setaffinity(0,{cpu});observations=[];binaries={};discarded=[];waits=[]
def context():
 pids=[]
 for p in Path('/proc').iterdir():
  if p.name.isdigit():
   try:
    if (p/'comm').read_text().strip() in ['cargo','rustc']:pids.append(int(p.name))
   except OSError:pass
 return dict(loadavg=os.getloadavg(),compiler_processes=len(pids),compiler_pids=pids)
def wait_builds():
 while (ctx:=context())['compiler_processes']:
  waits.append(dict(at=time.time(),**ctx));print('Waiting for compilers',ctx['compiler_processes'],flush=True);fds=[]
  try:
   for pid in ctx['compiler_pids']:
    try:fds.append(os.pidfd_open(pid))
    except ProcessLookupError:pass
   if fds:select.select(fds,[],[],20)
  finally:
   for fd in fds:os.close(fd)
def identity(r):return tuple(r.get(k) for k in ['operation','opaque','scale','phase','elements'])
for mode,variants in [('image',['before','after','scalar']),('page',['before','after']),('scene',['before','after'])]:
 for repeat in range(7):
  order=variants[repeat%len(variants):]+variants[:repeat%len(variants)]
  if repeat%2:order=order[::-1]
  for variant in order:
   name=f'{mode}-{variant}';binary=root/name;rss=root/'rss.txt';binaries[name]=hashlib.file_digest(binary.open('rb'),'sha256').hexdigest()
   while True:
    wait_builds();before=context()
    data=json.loads(subprocess.check_output(['/usr/bin/time','-f','%M','-o',str(rss),str(binary),'60'],text=True))
    assert data['features']==(['scalar-image-processing']if variant=='scalar'else[]),name
    row=dict(mode=mode,variant=variant,repeat=repeat,rss_kib=int(rss.read_text()),before_context=before,after_context=context(),**data)
    if not row['before_context']['compiler_processes'] and not row['after_context']['compiler_processes']:break
    discarded.append(row);print('Retaining compiler-overlap attempt',mode,repeat,variant,flush=True)
   observations.append(row);print(mode,repeat,variant,flush=True)
 group=[r for r in observations if r['mode']==mode];reference={identity(c):c for c in next(r for r in group if r['variant']=='after')['rows']}
 for observation in group:
  for row in observation['rows']:
   ref=reference[identity(row)]
   for key in (['source_sha256','result_sha256']if mode=='image'else['source_sha256','geometry_sha256','commands']+(['pixels_sha256']if mode=='scene'else[])):assert row[key]==ref[key],(mode,key,row,ref)
 for key,ref in reference.items():
  timing='micros'if mode=='image'else'pipeline_us'if mode=='page'else'scene_us';medians={}
  for v in variants:
   values=[c[timing]for r in group if r['variant']==v for c in r['rows']if identity(c)==key]
   if values:medians[v]=statistics.median(values)
  print(mode,{k:ref[k]for k in ['operation','opaque','scale','phase','elements']if k in ref},medians,flush=True)
destination.write_text(json.dumps(dict(scope=__doc__,before='f220336a',cpu=cpu,allowed_cpus=allowed,platform=platform.platform(),binaries=binaries,observations=observations,discarded=discarded,build_waits=waits),indent=2)+'\n')
