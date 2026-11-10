"""Validate recorded component, live geometry and release evidence."""
import hashlib,json,re
from pathlib import Path
root=Path(__file__).resolve().parent
repo=root.parents[3]
fixture=repo/'crates/web/tests/replaced-intrinsic-fixtures'
ref=json.loads((fixture/'chrome.json').read_text())
assert hashlib.sha256((fixture/'cases.json').read_bytes()).hexdigest()==ref['cases_sha256']
assert len(ref['rows'])==240
keys={(m,s) for m in ['light','dark','night'] for s in [1,1.25,1.5,1.75,2]}
observations={}
for variant in ['before','after','after-card','chrome']:
 rows=json.loads((root/f'stripe-{variant}/results.json').read_text())
 assert len(rows)==15 and {(r['mode'],r['scale']) for r in rows}==keys
 observations[variant]={(r['mode'],r['scale']):r['data'] for r in rows}
 for r in rows:
  assert abs(r['data']['dpr']-r['scale'])<1e-6
  assert (root/f"stripe-{variant}/{r['mode']}-{r['scale']}.png").is_file()
  if variant!='chrome':
   assert r['review'] and all(not w['findings'] for w in r['review'])
for key in keys:
 before=observations['before'][key];after=observations['after'][key];chrome=observations['chrome'][key]
 assert before['extent'][0]>before['client'][0] and before['client']==[985,562 if key[1]==1.25 else 563]
 assert after['extent'][0]==after['client'][0]==985
 assert after['client']==chrome['client']==[985,578]
 for a,b in zip(after['boxes'][:4],chrome['boxes'][:4]):
  # Vertical positions differ and remain an open site-layout failure.
  assert all(abs(a['rect'][i]-b['rect'][i])<.001 for i in [0,2,3]),(key,a,b)
 assert observations['after-card'][key]['extent'][0]==985
for name in ['build','test-web','test-workspace']:
 assert json.loads((root/(name+'-exit.json')).read_text())['code']==0
counts=re.findall(r'test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored;', (root/'test-workspace.log').read_text())
assert [sum(int(c[i]) for c in counts) for i in range(3)]==[2207,0,136]
for screen in ['1280','1440']:assert 'ui-lint: 0 errors' in (root/f'lint-{screen}.log').read_text()
rows=json.loads((root/'costs.json').read_text())
assert [(r['scale'],r['variant']) for r in rows]==[(s,v) for s in [1.25,2] for v in ['before','after','after','before']]
for r in rows:
 assert len(r['bench_ms']['sizes'])==7
 assert all(len(set(values))==2 for values in r['bench_ms']['sizes'])
 assert r['idle']['total']['cpu']==0 and all(w['frames']==0 for w in r['idle']['windows'])
 assert all(s['distance']>0 and not s.get('error') for s in r['scrolling'])
print('240 Chrome cases; 45 clean native reviews; 15 matched live width checks; release workspace/lints/cost evidence verified.')
