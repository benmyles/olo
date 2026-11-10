#!/usr/bin/python3
"""Verify retained evidence without driving a screen or making native expectations."""
import hashlib,json
from pathlib import Path
root=Path(__file__).resolve().parent;repo=root.parents[3]
for name,want in json.loads((root/'source-hashes.json').read_text()).items():assert hashlib.sha256((repo/name).read_bytes()).hexdigest()==want,name
for variant in ['before','after']:
 rows=json.loads((root/variant/'results.json').read_text());assert len(rows)==15
 assert {(r['mode'],r['scale']) for r in rows}=={(m,s) for m in ['light','dark','night'] for s in [1,1.25,1.5,1.75,2]}
 for r in rows:
  n,c=r['native'],r['chrome'];assert n['dpr']==c['dpr']==r['scale'];assert n['experiments']==c['experiments'];assert n['viewport']==c['viewport']==[1000,578];assert n['client']==c['client']==[985,578]
  assert all(not x['findings'] for x in r['review'])
  for engine in ['native','chrome']:assert (root/variant/f"{r['mode']}-{r['scale']}-{engine}.png").is_file()
  cards=[b for b in n['boxes'] if b['selector'].endswith('.feature-detail__content')];refs=[b for b in c['boxes'] if b['selector'].endswith('.feature-detail__content')];assert len(cards)==len(refs)==3
  assert all(b['paddingRight']==('32px' if variant=='after' else '16px') for b in cards)
  assert all(b['paddingRight']=='32px' for b in refs)
  if variant=='after':assert abs(cards[0]['rect'][3]-refs[0]['rect'][3])<.047
assert json.loads((root/'build-exit.json').read_text())['exit_code']==0
assert json.loads((root/'test-workspace-exit.json').read_text())['exit_code']==0
assert len(json.loads((root/'costs.json').read_text()))==8
assert json.loads((root/'measure-exit.json').read_text())['exit_code']==0
print('Source hashes, 30 configuration records, 60 screenshots, reviews and release checks verified. Website acceptance remains zero.')
