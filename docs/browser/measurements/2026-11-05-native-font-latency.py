#!/usr/bin/python3
"""Additional frozen-binary runs after unexpectedly large wall-clock slices.

Does not replace the primary runs. Records whole-process CPU time and scheduler
context switches to help assess the outliers; cannot assign a cause to each
individual slice. No source changes/builds/tests/GUI while measuring.
"""
import hashlib,json,re,subprocess,sys
from pathlib import Path
scratch=Path(sys.argv[1]).resolve();assert scratch.is_relative_to('/var/tmp')
result={'scope':'Follow-up on large observed wall-clock worker slices, same frozen default release binaries as primary measurement. Whole-process CPU/scheduler counters do not explain individual slices. All runs retained.','runs':[]}
for repeat in range(5):
 for scene in ['uniform','varying']:
  for variant in (['before','after'] if repeat%2==0 else ['after','before']):
   binary=scratch/(variant+'-style_bench');timing=scratch/'latency-time.log'
   data=json.loads(subprocess.check_output(['/usr/bin/time','-v','-o',str(timing),'taskset','-c','0',str(binary),'20','10000',scene],text=True));log=timing.read_text()
   def field(name):return re.search(r'^\s*'+re.escape(name)+r': (.+)$',log,re.M)[1]
   result['runs'].append(dict(repeat=repeat,variant=variant,scene=scene,sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),user_seconds=float(field('User time (seconds)')),system_seconds=float(field('System time (seconds)')),voluntary_switches=int(field('Voluntary context switches')),involuntary_switches=int(field('Involuntary context switches')),result=data))
 (scratch/'native-latency.json').write_text(json.dumps(result,indent=2)+'\n')
 print('Completed follow-up',repeat+1,flush=True)
