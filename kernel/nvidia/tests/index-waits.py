#!/usr/bin/python3
"""Lexical inventory on the exact tag. Includes host collections and macro definitions."""
from pathlib import Path
import bisect,json,re,subprocess
base=Path(__file__).resolve().parent.parent
tree=base/'baseline'
roots=['src/nvidia-modeset','kernel-open/nvidia-modeset','kernel-open/nvidia-drm','src/common/unix/nvidia-push','src/common/displayport']
blot=re.compile(r'/\*.*?\*/|//[^\n]*|"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'',re.S)
loops=re.compile(r'\b(?:for|while)\s*\(|\b(?:wait_for_completion\w*|wait_event\w*|dma_fence_\w*wait\w*|drm_\w*wait\w*|(?:cancel|flush)_\w*work\w*|nv_kthread_q_(?:stop|flush)|nvkms_(?:yield|usleep)|drm_atomic_helper_shutdown)\s*\(')
files=[p.relative_to(tree).as_posix() for p in sorted(tree.glob('**/*')) if p.is_file() and p.suffix in {'.c','.cpp','.h','.hpp'} and any(p.relative_to(tree).as_posix().startswith(r+'/') for r in roots) and '/_out/' not in p.relative_to(tree).as_posix() and '/conftest/' not in p.relative_to(tree).as_posix()]
tags={}
for i in range(0,len(files),120):
 data=subprocess.check_output(['ctags','--output-format=json','--fields=+ne','--kinds-C=f','--kinds-C++=f','-f','-',*files[i:i+120]],cwd=tree,text=True)
 for line in data.splitlines():
  t=json.loads(line)
  if t.get('kind')=='function': tags.setdefault(t['path'],[]).append((t.get('line',0),t.get('end',0),t['name']))
with (base/'tests/loop-index.tsv').open('w') as out:
 print('upstream file\tline\tfunction (when available)\tloop or wait',file=out)
 n=0
 for f in files:
  if f.endswith('.mod.c'): continue
  raw=(tree/f).read_text(errors='replace')
  masked=blot.sub(lambda m:re.sub(r'[^\n]',' ',m[0]),raw)
  starts=[0]+[m.end() for m in re.finditer(r'\n',masked)]
  for match in loops.finditer(masked):
   pos=match.start(); lineno=bisect.bisect_right(starts,pos)
   func=next((fn for a,z,fn in tags.get(f,[]) if a<=lineno<=z),'-')
   snippet=' '.join(raw[pos:].splitlines()[0].strip().split())[:160]
   print(f,lineno,func,snippet,sep='\t',file=out); n+=1
print(f'{len(files)} source/header files; {n} loop and wait sites')
