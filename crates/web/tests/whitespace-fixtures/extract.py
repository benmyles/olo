#!/usr/bin/python3
"""Literal WPT acceptance ports and an independent CSS phase-I regex oracle.

No Olo module/binary is imported or executed. Oracle rewrites a bidi-control-
free projection in separate normative phases, then merges untouched controls.
Tests compare results under different resumable-job budgets/partitions.
"""
import hashlib,itertools,json,random,re
from pathlib import Path
ROOT=Path(__file__).parent
manifest=json.loads((ROOT/'SOURCE.json').read_text())
for name,info in manifest['files'].items():
 assert hashlib.sha256((ROOT/name).read_bytes()).hexdigest()==info['sha256'],name
parsing=[]
for path in sorted((ROOT/'wpt').glob('*.html')):
 text=path.read_text()
 calls=re.findall(r'test_(valid|invalid)_value\("([^"]+)", "([^"]+)"\);',text)
 assert len(calls)==text.count('test_valid_value(')+text.count('test_invalid_value(')
 parsing += [dict(file=path.name,property=prop,input=value,valid=kind=='valid')for kind,prop,value in calls]
controls=set('\u061c\u200e\u200f\u202a\u202b\u202c\u202d\u202e\u2066\u2067\u2068\u2069')
def oracle(text,mode):
 controls_at={i:c for i,c in enumerate(text)if c in controls}
 projected=[(i,c)for i,c in enumerate(text)if c not in controls]
 def remove(pattern):
  nonlocal projected
  delete={i for m in re.finditer(pattern,''.join(c for _,c in projected))for i in range(m.start(),m.end())}
  projected=[pair for i,pair in enumerate(projected)if i not in delete]
 if mode in ('collapse','preserve-breaks'):
  remove(r'[ \t\r]+(?=\n)|(?<=\n)[ \t\r]+')
  if mode=='collapse':remove(r'(?<=\n)\n+')
  projected=[(i,' ' if c in ('\t','\r')or(c=='\n'and mode=='collapse')else c)for i,c in projected]
  remove(r'(?<= ) +')
 elif mode=='preserve-spaces':projected=[(i,' 'if c in '\t\r\n'else c)for i,c in projected]
 else:projected=[(i,' 'if c=='\r'else c)for i,c in projected]
 return ''.join(c for _,c in sorted([*projected,*controls_at.items()]))
alphabet=['a',' ','\t','\n','\r','\u202e','\u202c','é']
texts=[''.join(t)for n in range(4)for t in itertools.product(alphabet,repeat=n)]
rng=random.Random(720042)
texts += [''.join(rng.choices(alphabet+['中','\u2067','\u2069','\u00a0','\u200b','\u2003'],k=rng.randrange(4,90)))for _ in range(200)]
cases=[dict(input=text,mode=mode,expected=oracle(text,mode))for text in texts for mode in ['collapse','preserve','preserve-breaks','preserve-spaces','break-spaces']]
(ROOT/'derived.json').write_text(json.dumps(dict(parsing=parsing,phase_one=cases),ensure_ascii=False,indent=2)+'\n')
print(json.dumps(dict(parsing=len(parsing),phase_one=len(cases))))
