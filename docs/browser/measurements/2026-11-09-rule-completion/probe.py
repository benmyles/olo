import hashlib,json,os,subprocess,time
from pathlib import Path
root=Path('/var/tmp/codex-browser-rules'); rows=[]
for other in [0,8]:
 for variant in ['baseline','candidate','candidate','baseline']:
  binary=root/'refresh-before' if variant=='baseline' else Path.cwd()/'target/release/examples/refresh_bench'
  ps=lambda:subprocess.run(['ps','-C','rustc','-o','pid=,etimes=,pcpu='],capture_output=True,text=True).stdout
  before=ps(); load=os.getloadavg()
  out=subprocess.check_output(['/usr/bin/time','-f','{"user_s":%U,"system_s":%S,"wall_s":%e,"max_rss_kib":%M}','-o',str(root/'time.json'),str(binary),'200','1000',str(other)],text=True)
  row=dict(variant=variant,other_pages=other,load_before=load,compilers_before=before,compilers_after=ps(),binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),usage=json.loads((root/'time.json').read_text()),probe=json.loads(out))
  rows.append(row);(root/'probe.json').write_text(json.dumps(rows,indent=2)+'\n')
  print(variant,other,row['usage'],flush=True)
