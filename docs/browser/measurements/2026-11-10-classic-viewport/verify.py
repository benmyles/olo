"""Check retained checkpoint evidence; does not accept a website."""
import hashlib,json
from pathlib import Path
from PIL import Image,ImageChops
root=Path(__file__).resolve().parent
read=lambda path:json.loads((root/path).read_text())
v=read('validation.json')
assert (v['exit'],v['passed'],v['failed'],v['ignored'],v['suites'])==(0,2203,0,136,252)
assert v['build_finished']
for variant in ['before','after']:
 rows=read(f'{variant}/results.json')
 assert len(rows)==15 and all(not w['findings'] for row in rows for w in row['review'])
for row in read('visual-diff.json'):
 a=Image.open(root/'before'/f"{row['case']}.png").convert('RGB')
 b=Image.open(root/'after'/f"{row['case']}.png").convert('RGB')
 assert a.size==b.size
 assert ImageChops.difference(a,b).crop(row['client_crop']).getbbox() is None
rows=read('costs.json')
assert len(rows)==8 and [r['variant'] for r in rows]==['before','after','after','before']*2
for row in rows:
 assert row['idle']['total']['cpu']==0
 assert all(w['frames']==0 for w in row['idle']['windows'])
 assert all(s['moving_frames']>0 and s['hitches']==0 for s in row['scrolling'])
 for sizes in row['bench_ms']['sizes']:
  assert len(sizes)==20 and len(set(sizes))==2
  assert len(set(sizes[::2]))==len(set(sizes[1::2]))==1
rows=read('engine-costs.json')
assert len(rows)==20
assert [r['classic'] for r in rows]==[False,True,True,False]*5
assert hashlib.sha256((root/'engine-cost.rs').read_bytes()).hexdigest()==read('engine-build.json')['source_sha256']
print('2,203 tests, 30 reviews, 15 identical client captures and 16 moving scroll probes; no site accepted.')
