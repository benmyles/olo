import json,statistics
from pathlib import Path
root=Path(__file__).resolve().parent
rows=json.loads((root/'costs.json').read_text());out={}
for scale in [1.25,2]:
 out[str(scale)]={}
 for variant in ['before','after']:
  r=[r for r in rows if r['scale']==scale and r['variant']==variant]
  latency=json.loads((root/'latency.json').read_text());lr=[a for a in latency if a['scale']==scale and a['variant']==variant]
  med=statistics.median
  out[str(scale)][variant]={'cached_ms':med(x for a in r for x in a['bench_ms']['cached'][1:]),'changed_ms':med(x for a in r for x in a['bench_ms']['changed'][1:]),'cpu_per_resize_ms':med((a['after']['cpu_ms']-a['before']['cpu_ms'])/len(a['resize_ms']) for a in r),'rss_kib':med(a['after']['rss_kib'] for a in r),'resize_tool_ms':med(x for a in r for x in a['resize_ms'])}
  for kind in ['drag','wheel']:
   out[str(scale)][variant][kind+'_first_ms']=med(s['first_move_ms'] for a in lr for s in a['scrolling'] if s['kind']==kind)
   out[str(scale)][variant][kind+'_still']=sum(s['still'] for a in lr for s in a['scrolling'] if s['kind']==kind)
(root/'cost-summary.json').write_text(json.dumps(out,indent=2));print(json.dumps(out,indent=2))
