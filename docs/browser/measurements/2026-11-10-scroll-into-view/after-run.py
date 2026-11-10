import subprocess,time
time.sleep(2)
for script,args in [('setup-live.py',[]),('matrix.py',['after'])]:
 with open('/var/tmp/codex-browser-scroll/'+('setup-after-final.log'if script.startswith('setup')else'matrix-after.log'),'w')as f:
  subprocess.run(['/usr/bin/python3','/var/tmp/codex-browser-scroll/'+script,*args],stdout=f,stderr=subprocess.STDOUT,check=True)
