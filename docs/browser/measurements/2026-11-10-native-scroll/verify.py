"""Check retained checkpoint evidence; does not accept a website."""
import json,re
from pathlib import Path
root=Path(__file__).resolve().parent
read=lambda p:json.loads((root/p).read_text())
v=read('validation.json')
assert (v['exit'],v['passed'],v['failed'],v['ignored'],v['suites'])==(0,2206,0,136,252)
assert v['build_finished']
for name,n in [('before',15),('after',60),('controls',5),('stripe',15)]:
 rows=read(name+'/results.json')
 assert len(rows)==n
 assert all(not w['findings'] for r in rows for w in r['review'])
assert sum(len(r['data']['steps']) for r in read('oracle.json'))==100
assert all(r['position']==[120,64] and r['error'] is None for r in read('oracle-scrollby.json'))
assert len(read('nested/results.json'))==5
assert all(130<r['distance']<150 for r in read('nested/results.json'))
for key,tree in read('reload.json').items():
 assert any('Position 640, 720' in n.get('name','') for w in tree for n in w['nodes']),key
for name,moved in [('stop-before-exact.json',True),('stop-after.json',False)]:
 r=read(name)
 offsets=[m['scrolls'][0][1] for w in r['marks'] for m in w.get('marks',[]) if m['scrolls']]
 assert bool(max(offsets))==moved
 assert 'Stopped' in r['status'] and r['stop_ms']<100
chrome=read('stripe-chrome-check.json')['geometry']
assert chrome['size']==[1000,578] and chrome['client']==[985,578]
assert chrome['canvases'][0]['rect']==[985,542.03125]
for r in read('stripe/results.json'):
 assert abs(r['data']['dpr']-r['scale'])<1e-6
 assert abs(r['data']['canvases'][0]['rect'][0]-r['data']['client'][0])<.1
rows=read('costs.json')
assert len(rows)==8 and [r['variant'] for r in rows]==['before','after','after','before']*2
for r in rows:
 assert r['idle']['total']['cpu']==0
 assert all(w['frames']==0 for w in r['idle']['windows'])
 assert all(s['moving_frames']>0 for s in r['scrolling'])
 for sizes in r['bench_ms']['sizes']:
  assert len(sizes)==20 and len(set(sizes))==2
  assert len(set(sizes[::2]))==len(set(sizes[1::2]))==1
print('2,206 tests; 95 clean reviews; scroll, cancellation and reload checks verified. No website accepted.')
