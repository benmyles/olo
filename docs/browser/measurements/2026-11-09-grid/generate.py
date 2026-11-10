import json
from pathlib import Path
root=Path('/var/tmp/codex-browser-grid');cases=[]
for axis in ['columns','rows']:
 for name,tracks,n,extra in [('thirds','repeat(3,1fr)',3,''),('sevenths','repeat(7,1fr)',7,''),('weighted','1fr 2fr 3fr',3,''),('fixed-middle','1fr 13.3px 1fr',3,''),('subunit','.1fr .2fr .3fr',3,''),('zero','0fr 1fr 2fr',3,''),('minmax','minmax(31px,1fr) minmax(0,2fr) minmax(0,1fr)',3,''),('frozen','minmax(70px,1fr) 1fr 1fr',3,''),('auto','auto auto auto',3,''),('fixed','33.333px 33.333px 33.333px',3,''),('large-factor','1e100fr',1,''),('small-factor','1e-100fr',1,'')]:
  for width,gap in [(100,0),(101.3,1.2),(960,16)]:
   ident=f'{axis}-{name}-{width}';size=f'width:{width}px;height:12px' if axis=='columns' else f'height:{width}px;width:12px'
   cross='grid-template-rows:12px' if axis=='columns' else 'grid-template-columns:12px'
   cells=''.join(f'<i id="{ident}-{i}" style="background:{["#d44","#4a6","#48c"][i%3]}"></i>' for i in range(n))
   cases.append({'id':ident,'axis':axis,'tracks':tracks,'count':n,'size':width,'gap':gap,'html':f'<div id="{ident}" class=grid style="display:grid;{size};{cross};grid-template-{axis}:{tracks};gap:{gap}px;{extra}">{cells}</div>'})
html='<!doctype html><meta charset=utf-8><title>Grid track distribution</title><style>body{margin:0}.grid{position:absolute;left:0;top:0}i{min-width:0;min-height:0}</style>'+''.join(c['html'] for c in cases)
(root/'grid.html').write_text(html);(root/'cases.json').write_text(json.dumps(cases,indent=2));print(len(cases),'cases')
