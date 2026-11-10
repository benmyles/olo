#!/usr/bin/python3
"""Check retained component/live evidence; this does not accept the website."""
import hashlib,json
from pathlib import Path
root=Path(__file__).resolve().parent;repo=root.parents[3]
for name,want in json.loads((root/'source-hashes.json').read_text()).items():assert hashlib.sha256((repo/name).read_bytes()).hexdigest()==want,name
for variant in ['before','after']:
 rows=json.loads((root/variant/'results.json').read_text());assert len(rows)==15
 assert {(r['mode'],r['scale']) for r in rows}=={(m,s) for m in ['light','dark','night'] for s in [1,1.25,1.5,1.75,2]}
 for r in rows:
  n,c=r['native'],r['chrome'];assert n['dpr']==c['dpr']==r['scale'];assert n['experiments']==c['experiments'];assert n['viewport']==c['viewport']==[1000,578];assert n['client']==c['client'];assert abs(n['scroll']-c['scroll'])<.001
  assert all(not x['findings'] for x in r['review'])
  for engine in ['native','chrome']:assert (root/variant/f"{r['mode']}-{r['scale']}-{engine}.png").is_file()
  author=n['boxes'][3];assert author['selector'].endswith('.testimonial-card__author')
  assert abs(author['rect'][3]-(50.4 if variant=='before' and r['scale']==1.75 else 25.2))<1e-8
  if variant=='after':assert abs(n['boxes'][0]['rect'][3]-c['boxes'][0]['rect'][3])<.42
for name in ['build','test-workspace','measure']:assert json.loads((root/f'{name}-exit.json').read_text())['exit_code']==0
b=json.loads((root/'merge-binary.json').read_text());assert b['before_merge']==b['after_merge']
rows=json.loads((root/'costs.json').read_text());assert len(rows)==12
for r in rows:
 if r['variant']=='after':assert all(all(abs(h-(25.9 if i%2 else 16.8))<.001 for i,h in enumerate(group)) for group in r['bench_ms']['sizes'])
print('Source identities, 30 live configuration records, 60 screenshots, reviews, release checks and 12 cost runs verified. Website acceptance remains zero.')
