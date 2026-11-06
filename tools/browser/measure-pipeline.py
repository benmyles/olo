#!/usr/bin/python3
"""Seven alternating frozen default release pipeline samples on one performance CPU.
Usage: /var/tmp/OWNED OUTPUT.json; expects page-before/page-after.
Stop own builds and GUI checks first. No network, JS, GPU or presentation coverage."""
import os,json,hashlib,subprocess,statistics,sys
from pathlib import Path
root=Path(sys.argv[1]).resolve()
assert root.is_relative_to('/var/tmp')
destination=Path(sys.argv[2])
allowed=sorted(os.sched_getaffinity(0))
def frequency(c):
 p=Path(f'/sys/devices/system/cpu/cpu{c}/cpufreq/cpuinfo_max_freq');return int(p.read_text()) if p.exists() else 0
cpu=max(allowed,key=frequency);os.sched_setaffinity(0,{cpu});rows=[]
for repeat in range(7):
 for variant in (['before','after'] if repeat%2==0 else ['after','before']):
  rss=root/f'{variant}.rss';binary=root/f'page-{variant}'
  data=json.loads(subprocess.check_output(['/usr/bin/time','-f','%M','-o',str(rss),str(binary),'60'],text=True));assert data['features']==[]
  rows.append(dict(variant=variant,repeat=repeat,rss_kib=int(rss.read_text()),**data))
  print(repeat,variant,[round(r['pipeline_us'],2) for r in data['rows']],flush=True)
for observation in rows:
 for row,reference in zip(observation['rows'],rows[0]['rows']):
  for key in ['elements','commands','source_sha256','geometry_sha256']:assert row[key]==reference[key],(key,row,reference)
result=dict(scope=__doc__,before='dac55f4e',cpu=cpu,allowed_cpus=allowed,binaries={v:hashlib.file_digest((root/f'page-{v}').open('rb'),'sha256').hexdigest() for v in ['before','after']},observations=rows)
destination.write_text(json.dumps(result,indent=2)+'\n')
for n in [100,1000]:
 print(n,{v:statistics.median(c['pipeline_us'] for r in rows if r['variant']==v for c in r['rows'] if c['elements']==n) for v in ['before','after']})
