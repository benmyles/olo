#!/usr/bin/python3
"""Authored normal-flow inputs; no Olo output is used to generate cases."""
from pathlib import Path
import json
H=Path(__file__).parent;cases=[]
def add(name,body,css,width=400):cases.append(dict(name=name,body=body,css=css,width=width,height=800))
for direction in ['ltr','rtl']:
 for width in ['auto','100px','500px','50%']:
  for margins in ['0','auto','10px auto','auto 10px','10px 20px']:
   add(f'width-{direction}-{width}-{margins}','<div id=a></div>',f'body{{direction:{direction}}}#a{{height:20px;width:{width};margin:{margins};padding:10px;border-width:2px;border-style:solid}}')
for sizing in ['content-box','border-box']:
 for sizes in ['width:100px','width:100px;max-width:50px','width:10px;min-width:50px','width:auto;max-width:100px','width:100px;max-width:20px;min-width:50px','height:30px;min-height:50px;max-height:20px']:
  add(f'sizing-{sizing}-{sizes}','<div id=a></div>',f'#a{{box-sizing:{sizing};height:20px;{sizes};padding:10px;border-width:2px;border-style:solid;margin-left:auto;margin-right:auto}}')
for a in [-30,-10,0,10,30]:
 for b in [-20,0,20,40]:
  add(f'siblings-{a}-{b}','<div id=a></div><div id=b></div>',f'#a{{height:20px;margin-bottom:{a}px}}#b{{height:20px;margin-top:{b}px}}')
  add(f'empty-sibling-{a}-{b}','<div id=a></div><div id=e></div><div id=b></div>',f'#a{{height:20px;margin-bottom:{a}px}}#e{{margin-top:20px;margin-bottom:-10px}}#b{{height:20px;margin-top:{b}px}}')
for barrier in ['', 'display:flow-root','padding-top:1px','padding-bottom:1px','border-top-width:1px;border-top-style:solid','height:100px','min-height:100px','height:0px','overflow:hidden']:
 for empty in [False,True]:
  add(f'parent-{barrier}-{empty}','<div id=p><div id=a></div><div id=b></div></div>',f'#p{{{barrier};margin-top:10px;margin-bottom:20px}}#a{{height:{0 if empty else 20}px;margin-top:30px;margin-bottom:40px}}#b{{height:{0 if empty else 20}px;margin-top:-10px;margin-bottom:15px}}')
for mode in ['','height:100px','min-height:100px','height:50%;min-height:40px']:
 add('percent-height-'+mode,'<div id=p><div id=a></div><div id=b></div></div>',f'#p{{{mode}}}#a{{height:50%;min-height:10px}}#b{{height:calc(50% + 10px)}}')
for relative in ['left:10px;top:15px','left:10%;right:20px;top:10%;bottom:20px','right:20px;bottom:10px']:
 add('relative-'+relative,'<div id=p><div id=a></div></div><div id=b></div>',f'#p{{position:relative;{relative}}}#a{{height:20px}}#b{{height:20px}}')
add('percent-padding','<div id=a></div>','#a{width:50%;height:20px;padding:5%;margin:10% 5%}')
add('hidden-and-contents','<div id=p>A<span style="display:none"><img></span><span style="display:contents"> B</span> C</div>','',50)
add('inline-splits','<div id=p>A<span>B<div id=b>C</div>D</span>E</div>','',50)
for width in [20,50,100]:add('wrapping-'+str(width),'<div id=p>A <span>B</span> C</div>','',width)
for size in ['0.1px','0.3px','10.123px','33.333%','calc(33.333% + 0.2px)']:
 for direction in ['ltr','rtl']:
  add(f'fraction-{size}-{direction}','<div id=a></div><div id=b></div>',f'body{{direction:{direction}}}#a{{width:{size};height:10.123px;padding:0.3px;margin:0.1px auto}}#b{{height:0.3px;margin-top:-0.3px;margin-left:-0.3px}}',401.25)
for border in ['0.1px','0.3px','0.8px','1.3px','1.8px','2.3px']:
 add('fraction-border-'+border,'<div id=a></div>',f'#a{{width:100px;height:20px;border-width:{border};border-style:solid}}')
(H/'inputs.json').write_text(json.dumps(cases,indent=2)+'\n');print(len(cases),'inputs')
