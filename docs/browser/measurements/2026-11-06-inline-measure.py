#!/usr/bin/python3
"""Replay frozen parent/current/control release binaries from owned /var/tmp.

Build and freeze before running. No concurrent builds/tests/owned GUI/source
edits. before-* are a88b8775; after-* are default; control-inline_bench has
only baseline-inline-reshape. Parent has no layout implementation: the
control comparison measures sharing vs reshaping safe visual fragments in
the same fitter, not improvement over a preexisting browser.
"""
from pathlib import Path
import hashlib,json,platform,re,subprocess,sys
S=Path(sys.argv[1]).resolve();assert S.is_relative_to('/var/tmp')
names=[v+'-'+n for v in ['before','after']for n in ['style_bench','shaping_bench','linebreak_bench']]+['after-inline_bench','control-inline_bench']
paths=[*Path('crates/web/src').rglob('*.rs'),Path('crates/web/Cargo.toml'),Path('Cargo.toml'),Path('Cargo.lock'),Path(__file__),Path('crates/web/tests/inline.rs')]
paths += list(Path('crates/web/tests/inline-fixtures').glob('*'))
paths += [Path('crates/web/examples/'+n+'.rs')for n in ['inline_bench','shaping_bench','style_bench','linebreak_bench']]
paths=[p for p in paths if p.is_file()]
result=dict(parent=subprocess.check_output(['git','rev-parse','a88b8775'],text=True).strip(),platform=platform.platform(),cpu=Path('/proc/cpuinfo').read_text().split('model name')[1].split('\n')[0].strip(),rustc=subprocess.check_output(['rustc','--version','--verbose'],text=True),affinity=0,
 method=__doc__,features={'before':[],'after':[],'control':['baseline-inline-reshape']},source_hashes={str(p):hashlib.sha256(p.read_bytes()).hexdigest()for p in paths},binary_hashes={n:hashlib.sha256((S/n).read_bytes()).hexdigest()for n in names},binary_bytes={n:(S/n).stat().st_size for n in names},runs=[])
def run(variant,benchmark,args,repeat,**tags):
 name=variant+'-'+benchmark+'_bench';log=S/'time.log'
 data=json.loads(subprocess.check_output(['/usr/bin/time','-v','-o',str(log),'taskset','-c','0',str(S/name),*args],text=True));raw=log.read_text()
 def field(s):return re.search(r'^\s*'+re.escape(s)+r': (.+)$',raw,re.M)[1]
 result['runs'].append(dict(binary=name,variant=variant,benchmark=benchmark,args=args,repeat=repeat,result=data,maximum_rss_kib=int(field('Maximum resident set size (kbytes)')),user_seconds=float(field('User time (seconds)')),system_seconds=float(field('System time (seconds)')),voluntary_switches=int(field('Voluntary context switches')),involuntary_switches=int(field('Involuntary context switches')),**tags))
 (S/'measurement.json').write_text(json.dumps(result,indent=2)+'\n')
for repeat in range(3):
 variants=['before','after']if repeat%2==0 else ['after','before']
 for scene in ['bare','uniform','varying']:
  for variant in variants:run(variant,'style',['10','10000',scene],repeat,scene=scene)
 for variant in variants:run(variant,'shaping',['50','reverse']if repeat%2 else ['50'],repeat)
 for variant in variants:run(variant,'linebreak',['100'],repeat)
 for variant in (['after','control']if repeat%2==0 else ['control','after']):run(variant,'inline',['100'],repeat)
# Control must reproduce output geometry and glyph integers for every scene.
for repeat in range(3):
 rows={r['variant']:r['result']['cases']for r in result['runs']if r['benchmark']=='inline'and r['repeat']==repeat}
 for a,b in zip(rows['after'],rows['control']):
  for k in ['scene','width','source_bytes','lines','fragments','digest']:assert a[k]==b[k],(k,a,b)
for p,h in result['source_hashes'].items():assert hashlib.sha256(Path(p).read_bytes()).hexdigest()==h,p
print(json.dumps({'processes':len(result['runs']),'output':str(S/'measurement.json'),'control_geometry_equal':True,'source_hashes_unchanged':True}))
