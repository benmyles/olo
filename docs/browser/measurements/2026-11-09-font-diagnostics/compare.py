from pathlib import Path
from PIL import Image,ImageChops
import json,hashlib
r=Path('/var/tmp/codex-browser-fonts');rows=[]
for s in [1,1.25,1.5,1.75,2]:
 a=Image.open(r/f'chrome-{s}.png').convert('RGB');b=Image.open(r/f'native-{s}.png').convert('RGB');assert a.size==b.size
 diff=ImageChops.difference(a,b);values=list(diff.getdata())
 row={'scale':s,'size':a.size,'different_pixels':sum(any(v) for v in values),'pixels_over_10':sum(max(v)>10 for v in values),'bbox':diff.getbbox(),'mean_absolute_channel_delta':sum(sum(v) for v in values)/len(values)/3}
 diff.save(r/f'diff-{s}.png');rows.append(row)
a=Image.open('docs/browser/measurements/2026-11-09-counters/after/light-1.png').convert('RGB').crop((5,100,1005,678));b=Image.open(r/'native-1.png').convert('RGB').crop((0,0,1000,578));diff=ImageChops.difference(a,b)
result={'chrome_vs_engine':rows,'engine_vs_live_browser_dpr1':{'different_pixels':sum(any(v) for v in diff.getdata()),'bbox':diff.getbbox()},'source_sha256':hashlib.sha256(Path('/var/tmp/codex-browser-counters/review.html').read_bytes()).hexdigest()}
(r/'comparison.json').write_text(json.dumps(result,indent=2));print(json.dumps(result,indent=2))
