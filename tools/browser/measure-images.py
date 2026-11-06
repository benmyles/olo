#!/usr/bin/python3
"""Frozen release image SIMD/scalar and common text before/after measurements.

Usage: /var/tmp/OWNED OUTPUT.json. Expects image-simd/image-scalar,
page-before/page-after and scene-before/scene-after. Stop own builds/screens
first. Seven alternating processes on one allowed performance CPU; process
RSS includes fonts/harness. No network/codec/GPU/presentation coverage.
"""
import hashlib,json,os,platform,statistics,subprocess,sys
from pathlib import Path
root=Path(sys.argv[1]).resolve();assert root.is_relative_to('/var/tmp')
destination=Path(sys.argv[2]);allowed=sorted(os.sched_getaffinity(0))
def frequency(c):
 p=Path(f'/sys/devices/system/cpu/cpu{c}/cpufreq/cpuinfo_max_freq');return int(p.read_text()) if p.exists() else 0
cpu=max(allowed,key=frequency);os.sched_setaffinity(0,{cpu});observations=[];binaries={}
def context():
 compilers=0
 for p in Path('/proc').iterdir():
  if p.name.isdigit():
   try:compilers += (p/'comm').read_text().strip() in ['cargo','rustc']
   except OSError:pass
 return dict(loadavg=os.getloadavg(),compiler_processes=compilers)
for mode,variants in [('image',['scalar','simd']),('page',['before','after']),('scene',['before','after'])]:
 for repeat in range(7):
  for variant in variants if repeat%2==0 else variants[::-1]:
   name=f'{mode}-{variant}';binary=root/name;rss=root/'rss.txt'
   binaries[name]=hashlib.file_digest(binary.open('rb'),'sha256').hexdigest()
   before_context=context()
   data=json.loads(subprocess.check_output(['/usr/bin/time','-f','%M','-o',str(rss),str(binary),'60'],text=True))
   assert data['features']==(['scalar-image-processing'] if variant=='scalar' else []),name
   observations.append(dict(mode=mode,variant=variant,repeat=repeat,rss_kib=int(rss.read_text()),before_context=before_context,after_context=context(),**data))
   print(mode,repeat,variant,flush=True)
 group=[r for r in observations if r['mode']==mode]
 for observation in group:
  for row,ref in zip(observation['rows'],group[0]['rows'],strict=True):
   keys=['source_sha256','result_sha256'] if mode=='image' else ['source_sha256','geometry_sha256','commands']+(['pixels_sha256'] if mode=='scene' else [])
   for key in keys:assert row[key]==ref[key],(mode,key,row,ref)
 for i,row in enumerate(group[0]['rows']):
  key='micros' if mode=='image' else 'pipeline_us' if mode=='page' else 'scene_us'
  print(mode,{k:row[k] for k in ['operation','opaque','scale','elements'] if k in row},
   {v:statistics.median(r['rows'][i][key] for r in group if r['variant']==v) for v in variants},flush=True)
destination.write_text(json.dumps(dict(scope=__doc__,before='3035b3ff',cpu=cpu,allowed_cpus=allowed,platform=platform.platform(),rustc=subprocess.check_output(['rustc','--version'],text=True).strip(),binaries=binaries,observations=observations),indent=2)+'\n')
