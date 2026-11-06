#!/usr/bin/python3
"""Owned-screen scroll replay with first-report and submission timestamps.

Load tests/paint-fixtures/scroll.html, then eval the OWN_SCREEN environment:
  /usr/bin/python3 THIS_SCRIPT OWN_SCREEN /var/tmp/OWNED/report.json

Keeps legacy scroll-check fields, whose first_move_ms starts before the
first timed report (one period early for drag/flick). Additional fields time
the first moving client-frame mark from the first input report/submission.
The legacy analyser can also miss the first moving mark without a baseline;
this replay retains the before-input pose. These are client marks, not measured
compositor presentation or optical latency.
"""
import hashlib,json,os,runpy,subprocess,sys,time
from pathlib import Path
screen=sys.argv[1];assert os.environ.get('OLO_SCREEN')==screen
wins=json.loads(subprocess.check_output(['olo-screen','windows',screen]));w=next(w for w in wins if w['app_id']=='olo.browser');p=w['content'];m=runpy.run_path('tools/dev/scroll-check');c=m['Check'].__new__(m['Check']);c.sock=os.environ['OLO_SOCKET'];c.runtime=os.environ.get('XDG_RUNTIME_DIR','/run/user/1000');c.pid=w['pid'];c.at=(p['x']+p['width']/2,p['y']+p['height']/2);c.sign=1
context={};baseline={};last_pose=None;original_marks=c.marks;original_synth=c.synth;original_analyse=c.run.__globals__['analyse']
def now():return int(time.clock_gettime(time.CLOCK_MONOTONIC)*1e6)
def synth(events):
 for e in events:
  if not context and e['type']in ['wheel','scroll']and e.get('dy',0)!=0:
   context['submitted_at_us']=now();context['report_at_us']=e.get('at_us',context['submitted_at_us'])
 original_synth(events)
def marks():
 global last_pose
 values=original_marks()
 if not context:
  baseline.clear();baseline['scroll_pose']=values[-1]['scrolls'][0]if values else last_pose
  baseline['marks_before_input']=values
 if values:last_pose=values[-1]['scrolls'][0]
 return values
def analyse(kind,marks,start,speed,**kwargs):
 r=original_analyse(kind,marks,start,speed,**kwargs);r['run_started_at_us']=start;r.update(context);r['baseline']=dict(baseline);r['client_marks']=marks
 pose=baseline.get('scroll_pose')
 moved=next((mark for mark in marks if pose is not None and abs(mark['scrolls'][0][1]-pose[1])>1e-3),None)
 if moved and context:
  at=moved['at_us'];r['first_moving_client_frame_at_us']=at
  r['first_client_move_from_report_ms']=round((at-context['report_at_us'])/1000,3)
  r['first_client_move_from_submission_ms']=round((at-context['submitted_at_us'])/1000,3)
  r['first_report_delay_from_run_start_ms']=round((context['report_at_us']-start)/1000,3)
 return r
c.marks=marks;c.synth=synth;c.run.__globals__['analyse']=analyse
rows=[]
for kind in ['drag','flick','wheel']:
 context.clear();rows.append(c.run(kind,90,900))
result=dict(scope=__doc__,window=w,legacy_scroll_check_sha256=hashlib.sha256(Path('tools/dev/scroll-check').read_bytes()).hexdigest(),rows=rows)
Path(sys.argv[2]).write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(rows,indent=2))
