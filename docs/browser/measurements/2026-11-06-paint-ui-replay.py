#!/usr/bin/python3
"""Replay release scroll checks on an OWNED, already running Agent Screen.

Load crates/web/tests/paint-fixtures/scroll.html (400 rows) first;
focus the page, then invoke:
  eval "$(olo-screen env OWN_SCREEN)"
  /usr/bin/python3 THIS_SCRIPT SCREEN OUTPUT.json
Uses the repository's scroll-check analyser, with explicit owned sockets;
no /tmp sessions, personal screen, process launch, polling or installs.
"""
import json,os,runpy,socket,sys,subprocess
from pathlib import Path
screen=sys.argv[1];out=Path(sys.argv[2]);assert os.environ.get('OLO_SCREEN')==screen
wins=json.loads(subprocess.check_output(['olo-screen','windows',screen]));w=next(w for w in wins if w['app_id']=='olo.browser');p=w['content'];m=runpy.run_path('tools/dev/scroll-check');c=m['Check'].__new__(m['Check']);c.sock=os.environ['OLO_SOCKET'];c.runtime=os.environ.get('XDG_RUNTIME_DIR','/run/user/1000');c.pid=w['pid'];c.at=(p['x']+p['width']/2,p['y']+p['height']/2);c.sign=1
rows=[c.run(k,90,900)for k in ['drag','flick','wheel']];out.write_text(json.dumps(rows,indent=2)+'\n');print(out)
