#!/usr/bin/python3
"""Audit retained evidence; does not turn diagnostics into website acceptance."""
import hashlib, json, re
from pathlib import Path
root=Path(__file__).resolve().parent;repo=root.parents[3]
def read(name):return json.loads((root/name).read_text())
for variant in ['before','after']:
 rows=read(variant+'/results.json');assert len(rows)==15
 assert {(r['mode'],r['scale']) for r in rows}=={(m,s) for m in ['light','dark','night'] for s in [1,1.25,1.5,1.75,2]}
 for row in rows:
  n,c=row['native'],row['chrome'];assert n['dpr']==c['dpr']==row['scale'];assert n['scroll']==c['scroll']==500
  assert n['viewport']==c['viewport']==[1000,578];assert n['client']==c['client'];assert n['experiments']==c['experiments'] and n['experiments']
  assert row['review'] and all(not r['findings'] for r in row['review'])
  width=abs(n['boxes'][0]['rect'][2]-c['boxes'][0]['rect'][2]);assert width<.011 if variant=='after' else width>28
  for app in ['native','chrome']:assert (root/variant/f"{row['mode']}-{row['scale']}-{app}.png").is_file()
experiments=read('experiment-evidence.json');a=experiments['native-source.html'];b=experiments['chrome-source.html'];c=experiments['chrome-aligned-source.html']
assert a['personalize_section'] and not b['personalize_section'] and c['personalize_section']
assert a['normalized_experiments']!=b['normalized_experiments'] and a['normalized_experiments']==c['normalized_experiments']
for name in ['build','test-workspace']:assert read(name+'-exit.json')['returncode']==0
results=re.findall(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;', (root/'test-workspace.log').read_text())
assert [sum(int(r[i]) for r in results) for i in range(3)]==[2209,0,136] and len(results)==253
for size in ['1280','1440']:assert 'ui-lint: 0 errors' in (root/f'lint-{size}.log').read_text()
rows=read('costs.json');assert len(rows)==12
assert rows==read('costs-initial.json')+read('costs-repeat.json')
for row in rows:
 assert row['idle']['total']['cpu']==0 and all(w['frames']==0 for w in row['idle']['windows'])
 assert len(row['bench_ms']['cached'])==len(row['bench_ms']['changed'])==len(row['bench_ms']['sizes'])==7
 for widths in row['bench_ms']['sizes']:
  assert len(widths)==20
  assert len(set(widths))==(2 if row['variant']=='after' else 1)
 assert {s['kind'] for s in row['scrolling']}=={'drag','wheel'}
for variant in ['before','after']:assert len({r['sha256'] for r in rows if r['variant']==variant})==1
for name,expected in read('source-hashes.json').items():assert hashlib.sha256((repo/name).read_bytes()).hexdigest()==expected,name
print('Evidence verified: 80 new component observations, 225 existing inline-box observations, 30 native reviews, 30 Chrome captures, 2209 release tests and 12 cost runs. Website acceptance remains 0/100.')
