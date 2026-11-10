import json,statistics
from pathlib import Path
r=Path('/var/tmp/codex-browser-svg-refresh');a=json.loads((r/'costs.json').read_text());out=[]
for scale in [1.25,2]:
 row={'scale':scale}
 for variant in ['before','after']:
  xs=[x for x in a if x['scale']==scale and x['variant']==variant];assert len(xs)==2
  d={k:statistics.median(v for x in xs for v in x['bench_ms'][k][1:])for k in ['cached','changed','unrelated','frameTimes']}
  d['cpu_per_resize']=statistics.median((x['after']['cpu_ms']-x['before']['cpu_ms'])/len(x['resize_ms'])for x in xs)
  d['rss_kib']=statistics.median(x['after']['rss_kib']for x in xs)
  for kind in ['drag','wheel']:d[kind+'_first_ms']=statistics.median(v['first_move_ms']for x in xs for v in x['scrolling']if v['kind']==kind)
  d['hitches']=sum(v['hitches']for x in xs for v in x['scrolling']);d['stationary_frames']=sum(v['still']for x in xs for v in x['scrolling']);d['max_draw_ms']=max(v['slowest_draw_ms']for x in xs for v in x['scrolling'])
  d['idle_cpu']=max(x['idle']['total']['cpu']for x in xs);d['idle_frames']=sum(w['frames']for x in xs for w in x['idle']['windows']);row[variant]=d
 out.append(row)
(r/'cost-summary.json').write_text(json.dumps(out,indent=2));print(json.dumps(out,indent=2))
