#!/usr/bin/python3
"""Original raster inputs and PNGs. No renderer output is used as an oracle."""
from pathlib import Path
from PIL import Image
import json,io,base64
here=Path(__file__).resolve().parent;assets={}
def asset(name,w,h,rgba):
 im=Image.frombytes('RGBA',(w,h),bytes(rgba));out=io.BytesIO();im.save(out,format='PNG');url='data:image/png;base64,'+base64.b64encode(out.getvalue()).decode();assets[url]=dict(width=w,height=h,rgba=rgba,name=name);return url
alpha=asset('alpha',2,1,[255,0,0,255,0,0,255,128])
opaque=asset('opaque',2,2,[255,0,0,255,0,255,0,255,0,0,255,255,255,255,0,255])
checker=asset('checker',8,8,[v for y in range(8) for x in range(8) for v in ([255,255,255,255] if(x+y)%2 else [0,0,0,255])])
rows=[]
def case(name,css='',url=alpha,body=None):
 rows.append(dict(id=name,css='img{display:block}'+css,body=body or f'<img alt="Reference image" src="{url}">'))
case('natural')
case('upscale','img{width:32px;height:16px}')
case('fractional','img{width:33.5px;height:17.25px;margin-left:2.125px;margin-top:3.375px}')
case('fractional-dark','body{background-color:#202020}img{width:33.5px;height:17.25px;margin-left:2.125px;margin-top:3.375px}')
case('opaque','img{width:32px;height:24px}',opaque)
case('opaque-fractional','img{width:31.125px;height:24.875px;margin-left:.375px;margin-top:.625px}',opaque)
case('downscale','img{width:3px;height:3px}',checker)
case('downscale-fractional','img{width:3.5px;height:2.5px;margin-left:.25px}',checker)
case('clip','div{width:15px;height:11px;overflow:hidden}img{width:32px;height:16px;margin-left:-2.5px}',body=f'<div><img alt="Reference image" src="{alpha}"></div>')
case('border-padding','img{width:32px;height:16px;padding:2px;border-width:1px;border-style:solid;border-color:green}')
case('overlap','img{display:inline;width:20px;height:12px;margin-right:-8px}',body=f'<img alt="First" src="{alpha}"><img alt="Second" src="{opaque}">')
case('overlap-rtl','body{direction:rtl}img{display:inline;width:20px;height:12px;margin-left:-8px}',body=f'<img alt="First" src="{alpha}"><img alt="Second" src="{opaque}">')
(here/'base.css').write_text('html,body,div{display:block;margin:0;padding:0;border-width:0;border-style:none}head{display:none}html{font-family:"Olo Inline plain";font-size:0;line-height:0}body{padding-top:1px}\n')
(here/'inputs.json').write_text(json.dumps(rows,indent=2)+'\n');(here/'assets.json').write_text(json.dumps(assets,indent=2)+'\n');print(len(rows),'raster cases')
