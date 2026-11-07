#!/usr/bin/python3
"""Authored replaced-image cases; never consumes native engine output."""
from pathlib import Path
import base64,io,json,html
from PIL import Image
here=Path(__file__).resolve().parent
image=Image.new('RGBA',(2,1));image.putdata([(255,0,0,255),(0,0,255,128)])
png=io.BytesIO();image.save(png,format='PNG');url='data:image/png;base64,'+base64.b64encode(png.getvalue()).decode()
rows=[]
def case(name,css='',attrs='',context='',unsupported=None):
 rows.append(dict(name=name,width=200,height=800,body=f'<div id=p><img id=image alt="Reference image" src="{url}" {attrs}></div>',css=f'#p{{font-size:0;line-height:0;{context}}}#image{{{css}}}',unsupported=unsupported))
for display in ['inline','block','inline-block','flex','inline-flex','grid','inline-grid','table','inline-table']:
 case('natural-'+display,'display:'+display)
for name,css,context in [('width','width:23.5px',''),('height','height:7px',''),('both','width:20px;height:7px',''),('percent-width','width:50%',''),('percent-height-definite','height:50%','height:80px'),('percent-height-auto','height:50%',''),('centred-block','display:block;margin-left:auto;margin-right:auto',''),('rtl-block','display:block;margin:2px 4px 3px 7px','direction:rtl'),('border-box','box-sizing:border-box;width:23.5px;height:17px;padding:2px;border-width:1px;border-style:solid',''),('content-box','width:23.5px;height:17px;padding:2px;border-width:1px;border-style:solid',''),('negative-margins','margin:-2px -3px -4px -5px','')]:
 case(name,css,context=context)
for name,css in [('max-width','max-width:1px'),('max-height','max-height:.5px'),('min-width','min-width:6px'),('min-height','min-height:3px'),('both-max-width-limits','max-width:1px;max-height:.75px'),('both-max-height-limits','max-width:1.5px;max-height:.5px'),('both-min-width-limits','min-width:8px;min-height:3px'),('both-min-height-limits','min-width:6px;min-height:5px'),('incompatible-width-min','min-width:10px;max-height:3px'),('incompatible-height-min','max-width:1px;min-height:3px'),('min-over-max','min-width:6px;max-width:1px;min-height:3px;max-height:.5px'),('specified-width-auto-height-limit','width:20px;max-height:3px'),('specified-height-auto-width-limit','height:10px;max-width:3px'),('specified-minimum','width:20px;height:10px;min-width:25px;max-height:3px')]:
 case(name,css)
for value in ['37','23.5','23.','+23','-23','.5','0','50%','50 %','50px',' 50.5%tail','23e2','23.5e2','\t23','\v23','\u00a023','bogus']:
 case('attribute-'+repr(value),attrs='width="'+html.escape(value,quote=True)+'"')
for name,attrs,css in [('height-attribute','height=7',''),('both-attributes','width=37 height=9',''),('author-auto','width=37','width:auto'),('inline-auto','width=37 style="width:auto"',''),('author-zero','width=37','width:0'),('author-revert','width=37','width:revert')]:
 case(name,css,attrs)
for align in ['top','middle','bottom']:
 case('alignment-'+align,'vertical-align:'+align,unsupported='vertical-align on image')
(here/'inputs.json').write_text(json.dumps(rows,indent=2)+'\n')
print(len(rows),'authored cases')
