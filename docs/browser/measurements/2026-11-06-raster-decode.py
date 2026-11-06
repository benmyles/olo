#!/usr/bin/python3
"""Usage: /var/tmp/OWNED, containing frozen raster-bench. Stop own builds/GUI first."""
from pathlib import Path
import os,json,hashlib,subprocess,statistics,sys
root=Path(sys.argv[1]).resolve();assert root.is_relative_to('/var/tmp');binary=root/'raster-bench';allowed=sorted(os.sched_getaffinity(0))
def frequency(c):
 p=Path(f'/sys/devices/system/cpu/cpu{c}/cpufreq/cpuinfo_max_freq');return int(p.read_text()) if p.exists() else 0
cpu=max(allowed,key=frequency);os.sched_setaffinity(0,{cpu});observations=[]
for repeat in range(7):
 for mode in (['direct','bounded'] if repeat%2==0 else ['bounded','direct']):
  rss=root/'rss';data=json.loads(subprocess.check_output(['/usr/bin/time','-f','%M','-o',str(rss),str(binary),'10',mode],text=True))
  observations.append(dict(repeat=repeat,rss_kib=int(rss.read_text()),**data));print(repeat,mode,flush=True)
for o in observations:
 for r,ref in zip(o['rows'],observations[0]['rows']):
  for field in ['width','height','format','encoded_sha256','pixels_sha256']:assert r[field]==ref[field],field
result=dict(scope='Same frozen default release binary, seven alternating direct/bounded control processes on one allowed performance CPU. Encoders run outside timing; peak RSS includes fixture encoding. No file/network I/O, browser/UI, animation or process isolation coverage.',cpu=cpu,binary_sha256=hashlib.file_digest(binary.open('rb'),'sha256').hexdigest(),observations=observations)
(root/'decode-cost.json').write_text(json.dumps(result,indent=2)+'\n')
for size in [256,1920]:
 for fmt in ['Png','Jpeg','WebP','Gif']:
  print(size,fmt,{m:statistics.median(r['decode_ms'] for o in observations if o['mode']==m for r in o['rows'] if r['width']==size and r['format']==fmt) for m in ['direct','bounded']})
