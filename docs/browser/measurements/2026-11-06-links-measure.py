#!/usr/bin/python3
"""Frozen release samples, CPU 0, no concurrent owned GUI clients/builds.
Args: /var/tmp/OWNED containing page-before, page-after, interaction; OUTPUT.json.
Parent control: page.rs/paint scene.rs/raster.rs from 76cf0880 under the same
new page_bench harness. Other source is common. State restyle is a new capability
with no working parent control; measure it separately, without I/O/HTML parsing.
"""
import hashlib,json,platform,subprocess,sys,time
from pathlib import Path
root=Path(sys.argv[1]);rows=[]
for repeat in range(3):
 for variant in (['page-before','page-after']if repeat%2==0 else['page-after','page-before'])+['interaction']:
  binary=root/variant
  run=subprocess.run(['/usr/bin/time','-f','{"rss_kib":%M,"user_seconds":%U,"system_seconds":%S,"elapsed_seconds":%e}','taskset','-c','0',str(binary),'100'],capture_output=True,text=True,check=True)
  result=json.loads(run.stdout);assert result['features']==[],result['features']
  rows.append({'repeat':repeat,'variant':variant,'binary_sha256':hashlib.file_digest(binary.open('rb'),'sha256').hexdigest(),'result':result,'cost':json.loads(run.stderr)})
reference=next(r['result']['rows']for r in rows if r['variant']=='page-before')
for r in rows:
 if r['variant']=='interaction':continue
 for actual,expected in zip(r['result']['rows'],reference):
  for key in ['elements','commands','geometry_sha256','source_sha256']:assert actual[key]==expected[key],(key,actual,expected)
Path(sys.argv[2]).write_text(json.dumps({'scope':__doc__,'machine':platform.platform(),'captured_at_unix':time.time(),'rows':rows},indent=2)+'\n')
print(json.dumps(rows,indent=2))
