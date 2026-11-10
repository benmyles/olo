import json,os,subprocess,time
from pathlib import Path
root=Path('/var/tmp/codex-browser-scroll');env=os.environ.copy();env['TMPDIR']=str(root)
for k in ['WAYLAND_DISPLAY','OLO_SOCKET','CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER']:env.pop(k,None)
for name,cmd in [('build',['cargo','build','--release','--workspace']),('test-workspace',['cargo','test','--release','--workspace'])]:
 t=time.monotonic()
 with (root/(name+'.log')).open('w') as f:r=subprocess.run(cmd,env=env,stdout=f,stderr=subprocess.STDOUT,start_new_session=True)
 (root/(name+'-exit.json')).write_text(json.dumps({'exit_code':r.returncode,'seconds':time.monotonic()-t}))
 print(name,r.returncode,flush=True)
 if r.returncode:break
