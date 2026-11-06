#!/usr/bin/python3
"""Generate factual Unicode 18 bidi/bracket/default-ignorable properties.
No engine, segmentation implementation or test expectations are inspected.
"""
from pathlib import Path
import hashlib
ROOT=Path('crates/web/tests/shaping-fixtures/ucd')
classes=['L','R','AL','EN','ES','ET','AN','CS','NSM','BN','B','S','WS','LRE','LRO','RLE','RLO','PDF','LRI','RLI','FSI','PDI','ON']
aliases={'Left_To_Right':'L','Right_To_Left':'R','Arabic_Letter':'AL','European_Terminator':'ET'}
values=bytearray(0x110000)
def span(s):
 p=s.strip().split('..');return int(p[0],16),int(p[-1],16)
source=(ROOT/'DerivedBidiClass.txt').read_text()
for missing in [True,False]:
 for line in source.splitlines():
  if missing:
   if '# @missing:' not in line:continue
   record=line.split('# @missing:')[1].strip()
  else:
   record=line.split('#')[0].strip()
   if not record:continue
  code,name=map(str.strip,record.split(';')[:2]);name=aliases.get(name,name);a,b=span(code);values[a:b+1]=bytes([classes.index(name)])*(b-a+1)
ranges=[]
for c in range(0x110000):
 if c==0x10FFFF or values[c]!=values[c+1]:ranges.append((c,values[c]))
ignorable=[]
for line in (ROOT/'DerivedCoreProperties.txt').read_text().splitlines():
 record=line.split('#')[0].strip()
 if not record:continue
 parts=list(map(str.strip,record.split(';')))
 if parts[1]=='Default_Ignorable_Code_Point':ignorable.append(span(parts[0]))
brackets=[]
for line in (ROOT/'BidiBrackets.txt').read_text().splitlines():
 record=line.split('#')[0].strip()
 if not record:continue
 cp,pair,kind=map(str.strip,record.split(';')[:3]);cp=int(cp,16);pair=int(pair,16);opening=cp if kind=='o' else pair
 # UAX #9 BD16's canonical equivalence for angle brackets.
 if opening==0x2329:opening=0x3008
 brackets.append((cp,opening,kind=='o'))
ignorable.sort();brackets.sort()
gcb_names=['Other','Control','CR','LF','Extend','ZWJ','SpacingMark','Prepend','L','V','T','LV','LVT','Regional_Indicator']
gcb=[];incb=[];pictographic=[]
for filename,target,property_name in [('GraphemeBreakProperty.txt',gcb,None),('DerivedCoreProperties.txt',incb,'InCB'),('emoji-data.txt',pictographic,'Extended_Pictographic')]:
 for line in (ROOT/filename).read_text().splitlines():
  record=line.split('#')[0].strip()
  if not record:continue
  parts=list(map(str.strip,record.split(';')))
  if property_name is not None and parts[1]!=property_name:continue
  a,b=span(parts[0])
  value=gcb_names.index(parts[1]) if property_name is None else ['None','Consonant','Extend','Linker'].index(parts[2]) if property_name=='InCB' else 1
  target.append((a,b,value))
def compact(records):
 out=[]
 for a,b,value in sorted(records):
  assert not out or a>out[-1][1]
  if out and a==out[-1][1]+1 and value==out[-1][2]:out[-1]=(out[-1][0],b,value)
  else:out.append((a,b,value))
 return out
gcb=compact(gcb);incb=compact(incb);pictographic=compact(pictographic)
# Hangul LV/LVT are computed from syllable arithmetic in the iterator.
assert all(0xac00<=a<=b<=0xd7a3 for a,b,v in gcb if v in [11,12])
gcb=[r for r in gcb if r[2] not in [11,12]]
text='// Generated Unicode 18.0.0 facts; Unicode License V3 in tests/shaping-fixtures/ucd.\n'
for name in ['DerivedBidiClass.txt','DerivedCoreProperties.txt','BidiBrackets.txt','GraphemeBreakProperty.txt','emoji-data.txt']:text+='// '+name+' SHA256 '+hashlib.sha256((ROOT/name).read_bytes()).hexdigest()+'\n'
text+='pub(super) const CLASSES: &[unicode_bidi::BidiClass] = &[\n'+''.join('    unicode_bidi::BidiClass::'+name+',\n' for name in classes)+'];\n'
text+='pub(super) const ASCII: &[u8;128] = &[\n'+''.join(f'    {v},\n' for v in values[:128])+'];\n'
text+='pub(super) const BIDI: &[(u32,u8)] = &[\n'+''.join(f'    (0x{end:x},{value}),\n' for end,value in ranges)+'];\n'
text+='pub(super) const IGNORABLE: &[(u32,u32)] = &[\n'+''.join(f'    (0x{a:x},0x{b:x}),\n' for a,b in ignorable)+'];\n'
text+='pub(super) const BRACKETS: &[(u32,u32,bool)] = &[\n'+''.join(f'    (0x{cp:x},0x{o:x},{str(k).lower()}),\n' for cp,o,k in brackets)+'];\n'
for name,records in [('GCB',gcb),('INCB',incb),('PICTOGRAPHIC',pictographic)]:
 text+='pub(super) const '+name+': &[(u32,u32,u8)] = &[\n'+''.join(f'    (0x{a:x},0x{b:x},{v}),\n' for a,b,v in records)+'];\n'
Path('crates/web/src/text/unicode_data.rs').write_text(text)
print('Bidi ranges',len(ranges),'ignorables',len(ignorable),'brackets',len(brackets))
