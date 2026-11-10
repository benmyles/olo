import hashlib,json,os,subprocess
from pathlib import Path
root=Path('/var/tmp/codex-browser-compile-queue');rows=[]
for keep in [0,4,64]:
 for variant in ['baseline','candidate','candidate','baseline']:
  binary=root/('burst-before' if variant=='baseline' else 'burst-after')
  ps=lambda:subprocess.run(['ps','-C','rustc','-o','pid=,etimes=,pcpu='],capture_output=True,text=True).stdout
  before=ps();load=os.getloadavg()
  text=subprocess.check_output(['/usr/bin/time','-f','{"user_s":%U,"system_s":%S,"wall_s":%e,"max_rss_kib":%M}','-o',str(root/'burst-time.json'),str(binary),'64','10000',str(keep)],text=True)
  row=dict(variant=variant,keep=keep,load_before=load,compilers_before=before,compilers_after=ps(),binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),usage=json.loads((root/'burst-time.json').read_text()),probe=json.loads(text))
  rows.append(row);(root/'burst.json').write_text(json.dumps(rows,indent=2)+'\n');print(variant,keep,row['probe'],row['usage'],flush=True)
