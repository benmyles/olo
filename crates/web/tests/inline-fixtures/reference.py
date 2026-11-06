#!/usr/bin/python3
"""Brute-force independent line fitter over native HarfBuzz 15, no Olo calls.

Only literal space-separated, single-script, normal whitespace cases. Every
candidate is reshaped from scratch with paragraph context, including candidates
past an overflowing prefix. This deliberately does not use the retained index.
"""
from pathlib import Path
import hashlib, itertools, json, sys
ROOT=Path(__file__).resolve().parents[4]
sys.path.insert(0,str(Path(__file__).resolve().parent.parent/'shaping-fixtures'))
from native_harfbuzz import shape, version
HERE=Path(__file__).parent
fonts=[(name,'Olo Inline '+name,str((HERE/(name+'.ttf')).relative_to(ROOT)),'Latn','ltr')for name in ['plain','ligature','deletion']]
fonts += [('latin','Noto Sans','data/fonts/NotoSans[wght].ttf','Latn','ltr'),('hebrew','Noto Sans Hebrew','crates/web/tests/shaping-fixtures/fonts/notosanshebrew/NotoSansHebrew[wdth,wght].ttf','Hebr','rtl'),('arabic','Noto Sans Arabic','crates/web/tests/shaping-fixtures/fonts/notosansarabic/NotoSansArabic[wdth,wght].ttf','Arab','rtl')]
texts={'plain':['A B C','A A B C A','fi A fi'], 'ligature':['A B C','A B A B C','C A B C','fi A B fi'], 'deletion':['D E','D E D E','A D E B','D E A D E'], 'latin':['office affine cafe','alpha beta gamma delta','a\u0308 cafe office'], 'hebrew':['שלום עולם שלום','אבג דהו זחט'], 'arabic':['مرحبا بالعالم مرحبا','العربية سلام عالم']}
cases=[]
for name,family,path,script,direction in fonts:
 data=(ROOT/path).read_bytes()
 for text in texts[name]:
  raw=text.encode();cuts=[i+1 for i,b in enumerate(raw)if b==32]+[len(raw)]
  cache={}
  def measure(start,end):
   if (start,end)not in cache:
    upem,gs=shape(data,text,start,end,script,direction,'en',{'wght':400,'wdth':100},{})
    cache[start,end]=(sum(g['x_advance']for g in gs)*(20/upem),gs)
   return cache[start,end]
  for width in [0,5,10,15,20,25,30,40,50,70,100,1000]:
   start=0;lines=[]
   while start<len(raw):
    while start<len(raw)and raw[start]==32:start+=1
    if start==len(raw):break
    candidates=[]
    for cut in cuts:
     if cut<=start:continue
     end=cut
     while end>start and raw[end-1]==32:end-=1
     advance,gs=measure(start,end)
     candidates.append((cut,end,advance,gs))
    fitting=[c for c in candidates if c[2]<=width]
    cut,end,advance,gs=fitting[-1]if fitting else candidates[0]
    lines.append(dict(start=start,end=end,advance=advance,glyphs=gs))
    start=cut
   cases.append(dict(font=name,family=family,path=path,text=text,rtl=direction=='rtl',width=width,lines=lines))
result=dict(oracle='native HarfBuzz '+version,method=__doc__,fonts={path:hashlib.sha256((ROOT/path).read_bytes()).hexdigest()for _,_,path,_,_ in fonts},cases=cases)
(HERE/'harfbuzz.json').write_text(json.dumps(result,ensure_ascii=False,separators=(',',':'))+'\n')
print(len(cases),'independent layouts')
