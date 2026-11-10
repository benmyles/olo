#!/usr/bin/python3
"""Verify retained evidence, not website acceptance."""
import json,hashlib
from pathlib import Path
root=Path(__file__).resolve().parent;repo=root.parents[3]
for name,want in json.loads((root/'source-hashes.json').read_text()).items():assert hashlib.sha256((repo/name).read_bytes()).hexdigest()==want,name
f=repo/'crates/js/tests/scroll-into-view-fixtures'
for name,want in json.loads((f/'capture-manifest.json').read_text())['sources'].items():assert hashlib.sha256((f/name).read_bytes()).hexdigest()==want,name
for variant in ['before','after']:
 rows=json.loads((root/variant/'results.json').read_text());assert len(rows)==15
 assert {(r['mode'],r['scale'])for r in rows}=={(m,s)for m in ['light','dark','night']for s in [1,1.25,1.5,1.75,2]}
 for r in rows:
  n,c=r['native'],r['chrome'];assert n['dpr']==c['dpr']==r['scale'];assert n['experiments']==c['experiments'];assert n['viewport']==c['viewport']==[1000,578];assert n['client']==c['client']==[985,578]
  assert max(abs(x-y)for x,y in zip(n['scroll'],c['scroll']))<.001
  assert n['active']=='Show testimonial from '+('Jobber'if variant=='after'else'Mindbody')
  assert c['active']=='Show testimonial from Jobber'
  if variant=='after':assert abs(n['offset']-c['offset'])<.02
  else:assert n['offset']==0
  assert all(not x['findings']for x in r['review'])
  for e in ['native','chrome']:
   assert r[e+'_action']['matched']['name']=='Show testimonial from Jobber'
   assert (root/variant/f"{r['mode']}-{r['scale']}-{e}.png").is_file()
for name in ['build','test-workspace','measure']:assert json.loads((root/f'{name}-exit.json').read_text())['exit_code']==0
rows=json.loads((root/'costs.json').read_text());assert len(rows)==8
for r in rows:
 if r['variant']=='after':assert all(group['valid']for group in r['bench_ms']['destinations'])
latency=json.loads((root/'latency.json').read_text());assert len(latency)==8
assert all('error'not in s and s['first_move_ms']>0 for r in latency for s in r['scrolling'])
print('30 live records, 60 screenshots, clean native reviews, source/oracle identities, release checks and 8 cost runs verified. Stripe remains 0/30.')
