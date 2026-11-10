import json,subprocess,hashlib
from pathlib import Path
root=Path('/var/tmp/codex-browser-grid');old=Path('docs/browser/measurements/2026-11-09-font-diagnostics');source=old/'review.html';refs=json.loads((old/'chrome-live-geometry.json').read_text());rows=[]
for row in refs:
 scale=row['requested_scale'];p=root/f'counter-{scale}.json'
 with (root/f'counter-{scale}.log').open('w') as log:subprocess.run(['target/release/examples/page_dump',str(source),'1000','--scale',str(scale),'--fonts',str(p)],stdout=log,stderr=subprocess.STDOUT,check=True)
 native=json.loads(p.read_text());byid={p['id']:p['element_bounds'] for p in native['paragraphs'] if p['id']};comp=[]
 for r in row['rects']:
  if r['id'] not in byid:continue
  got=byid[r['id']];comp.append({'id':r['id'],'chrome':r['rect'],'native':got,'max_delta':max(abs(a-b) for a,b in zip(got,r['rect']))})
 rows.append({'scale':scale,'matching':sum(c['max_delta']<.0001 for c in comp),'total':len(comp),'rectangles':comp})
print([(r['scale'],r['matching'],r['total']) for r in rows])
(root/'counter-geometry.json').write_text(json.dumps({'source_sha256':hashlib.sha256(source.read_bytes()).hexdigest(),'binary_sha256':hashlib.sha256(Path('target/release/examples/page_dump').read_bytes()).hexdigest(),'reference':'../2026-11-09-font-diagnostics/chrome-live-geometry.json','rows':rows},indent=2))
