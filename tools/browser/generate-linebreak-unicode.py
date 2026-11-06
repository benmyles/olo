#!/usr/bin/python3
"""Generate factual Unicode 18 line-break properties, not boundary decisions.

Keep raw Line_Break for future CSS tailoring; runtime applies LB1 defaults.
Packed flags retain EastAsianWidth, quote category, unassigned pictographs,
SA marks and dotted-circle identity. No test expectations are inspected.
"""
from pathlib import Path
import hashlib,json,subprocess
ROOT=Path('crates/web/tests/linebreak-fixtures')
manifest=json.loads((ROOT/'SOURCE.json').read_text())
for name,info in manifest['files'].items():assert hashlib.sha256((ROOT/name).read_bytes()).hexdigest()==info['sha256'],name
classes='AI AK AL AP AS B2 BA BB BK CB CJ CL CM CP CR EB EM EX GL H2 H3 HH HL HY ID IN IS JL JT JV LF NL NS NU OP PO PR QU RI SA SG SP SY VF VI WJ XX ZW ZWJ'.split()
def span(code):
 s=code.strip().split('..');return int(s[0],16),int(s[-1],16)
def records(path,missing=False):
 for line in path.read_text().splitlines():
  if missing:
   if '# @missing:'not in line:continue
   record=line.split('# @missing:')[1].strip()
  else:record=line.split('#')[0].strip()
  if record:
   parts=list(map(str.strip,record.split(';')));yield (*span(parts[0]),parts[1])
raw=['XX']*0x110000;gc=['Cn']*0x110000;ea=bytearray(0x110000);ep=bytearray(0x110000)
for target,path in [(raw,'LineBreak.txt'),(gc,'DerivedGeneralCategory.txt')]:
 for missing in [True,False]:
  for a,b,value in records(ROOT/'ucd'/path,missing):target[a:b+1]=[value]*(b-a+1)
for missing in [True,False]:
 for a,b,value in records(ROOT/'ucd/EastAsianWidth.txt',missing):ea[a:b+1]=bytes([value in ('F','W','H')])*(b-a+1)
for a,b,value in records(ROOT/'../shaping-fixtures/ucd/emoji-data.txt'):
 if value=='Extended_Pictographic':ep[a:b+1]=b'\1'*(b-a+1)
packed=[]
for c,name in enumerate(raw):
 original=name
 flags=int(ea[c])<<6 | int(gc[c]=='Pi')<<7 | int(gc[c]=='Pf')<<8 | int(gc[c]=='Cn'and ep[c])<<9 | int(c==0x25cc)<<10 | int(original=='SA')<<11 | int(original=='SA'and gc[c]in('Mn','Mc'))<<12
 packed.append(classes.index(name)|flags)
ranges=[(c,value)for c,value in enumerate(packed)if c==0x10ffff or packed[c+1]!=value]
out='// Generated factual Unicode 18.0.0 properties; see tools/browser/generate-linebreak-unicode.py.\n// Unicode License V3; pinned sources in tests/linebreak-fixtures/SOURCE.json.\n'
out+='pub(super) mod class {\n    pub type Class = u8;\n'+''.join(f'    pub const {c}: Class = {i};\n'for i,c in enumerate(classes))+'}\n'
out+='pub(super) const CLASS_IDS: &[u8] = &['+','.join(str(i)for i in range(len(classes)))+'];\n'
out+='pub(super) const ASCII: &[u16;128] = &['+','.join(str(v)for v in packed[:128])+'];\n'
out+='pub(super) const RANGES: &[(u32,u16)] = &[\n'+''.join(f'    (0x{c:x},{v}),\n'for c,v in ranges)+'];\n'
Path('crates/web/src/text/linebreak_data.rs').write_text(out)
subprocess.run(['rustfmt','--config-path','rustfmt.toml','crates/web/src/text/linebreak_data.rs'],check=True)
print(json.dumps(dict(classes=len(classes),ranges=len(ranges))))
