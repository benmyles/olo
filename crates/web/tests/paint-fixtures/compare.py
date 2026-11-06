#!/usr/bin/python3
"""Exact independent pixel comparison; all differences remain failures.
Usage: /usr/bin/python3 compare.py NATIVE_PNG_DIRECTORY [REPORT.json]
"""
from pathlib import Path
from PIL import Image,ImageChops
import json,sys
here=Path(__file__).resolve().parent;native=Path(sys.argv[1]);rows=[]
for case in json.loads((here/'inputs.json').read_text()):
 for scale in [1,1.5,2]:
  name=f'{case["id"]}-{scale}.png';a=Image.open(here/name).convert('RGBA');b=Image.open(native/name).convert('RGBA');assert a.size==b.size
  diff=ImageChops.difference(a,b);data=list(diff.get_flattened_data());n=sum(any(v)for v in data)
  rows.append(dict(file=name,differing_pixels=n,pixels=a.width*a.height,max_channel_error=max(max(v)for v in data),passed=n==0))
report=dict(scope='Exact full-surface RGBA comparison; no masks, tolerance or discarded pixels. Authored static fixed-font fixtures, not websites.',passed=sum(r['passed']for r in rows),observations=len(rows),rows=rows)
if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps(report,indent=2));sys.exit(0 if report['passed']==report['observations']else 1)
