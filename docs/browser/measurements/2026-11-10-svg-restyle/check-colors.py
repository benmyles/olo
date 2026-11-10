from PIL import Image
from collections import Counter
from pathlib import Path
import json,sys
r=Path('/var/tmp/codex-browser-svg-refresh');rows=[]
for variant in ['before','after-svg-only','after']:
 if not (r/variant).exists(): continue
 for mode in ['light','dark','night']:
  for scale in [1,1.25,1.5,1.75,2]:
   colors={}
   for engine,shift in [('native',0),('chrome',12)]:
    p=r/variant/f'{mode}-{scale}-{engine}.png'
    if not p.exists():continue
    im=Image.open(p).convert('RGB');assert abs(im.width-1010*scale)<2,(p,im.size)
    crop=im.crop(tuple(round(n*scale) for n in [315,515+shift,460,585+shift]));c=Counter(crop.getdata())
    colors[engine]={'jobber_active_pixels':c[(1,41,57)],'inactive_pixels':c[(149,164,186)]}
   rows.append({'variant':variant,'mode':mode,'scale':scale,'colors':colors,'active':colors.get('native',{}).get('jobber_active_pixels',0)>20*scale*scale})
(r/'selection-colors.json').write_text(json.dumps(rows,indent=2))
for v in dict.fromkeys(x['variant']for x in rows):
 sub=[x for x in rows if x['variant']==v];print(v,sum(x['active']for x in sub),'/',len(sub),'active; nonpasses',[(x['mode'],x['scale'])for x in sub if not x['active']])
if '--require-final' in sys.argv:assert len([x for x in rows if x['variant']=='after'and x['active']])==15
