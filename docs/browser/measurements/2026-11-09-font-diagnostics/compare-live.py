from pathlib import Path
from PIL import Image,ImageChops
import json
r=Path('/var/tmp/codex-browser-fonts');rows=[]
for c in json.loads((r/'chrome-live-geometry.json').read_text()):
 s=c['requested_scale'];a=Image.open(r/f'chrome-live-{s}.png').convert('RGB').crop((0,0,int(1000*s),int(700*s)));b=Image.open(r/f'native-{s}.png').convert('RGB');diff=ImageChops.difference(a,b);values=list(diff.getdata())
 n=json.loads((r/f'native-{s}.json').read_text());byid={a['id']:a for a in n['paragraphs'] if a['id']};geometry=[]
 for a in c['rects']:
  if a['id'] not in byid:continue
  b=byid[a['id']]['element_bounds'];delta=max(abs(x-y) for x,y in zip(b,a['rect']));geometry.append({'id':a['id'],'chrome':a['rect'],'native':b,'max_delta':delta})
 row={'scale':s,'different_pixels':sum(any(v) for v in values),'pixels_over_10':sum(max(v)>10 for v in values),'bbox':diff.getbbox(),'mean_absolute_channel_delta':sum(sum(v) for v in values)/len(values)/3,'geometry_exact':sum(g['max_delta']==0 for g in geometry),'geometry_within_1e_minus_4':sum(g['max_delta']<.0001 for g in geometry),'geometry':geometry}
 diff.save(r/f'diff-live-{s}.png');rows.append(row)
(r/'comparison-live.json').write_text(json.dumps(rows,indent=2));print(json.dumps([{k:v for k,v in row.items() if k!='geometry'} for row in rows],indent=2))
out=Image.new('RGB',(1000,300),'#ccc')
for i,s in enumerate([1,1.25,1.5,1.75,2]):
 for j,k in enumerate(['chrome-live','native','diff-live']):
  im=Image.open(r/f'{k}-{s}.png').convert('RGB');clip=im.crop((int(18*s),int(24*s),int(110*s),int(65*s)));out.paste(clip,(i*200,j*100))
out.save(r/'inspect-live.png')
