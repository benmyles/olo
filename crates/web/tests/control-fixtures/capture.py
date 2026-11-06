#!/usr/bin/python3
"""Capture original control-state fixtures in an owned Chrome profile/screen.

--screen NAME --profile /var/tmp/OWNED/profile --out /var/tmp/OWNED/control.json
Launch Chrome through olo-screen/olo-chrome with --remote-debugging-port=0.
The existing site tab is kept. A new local fixture tab is closed after capture.
No form is submitted. This is CSS state evidence, not visual/site acceptance.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import urllib.parse
import urllib.request

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[3] / 'tools' / 'browser'))
from cdp import Connection

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--screen', required=True)
parser.add_argument('--profile', type=Path, required=True)
parser.add_argument('--out', type=Path, required=True)
args = parser.parse_args()
assert os.environ.get('OLO_SCREEN') == args.screen
profile = args.profile.resolve()
assert profile.is_relative_to('/var/tmp') and args.out.resolve().is_relative_to('/var/tmp')
port = int((profile / 'DevToolsActivePort').read_text().splitlines()[0])
endpoint = f'http://127.0.0.1:{port}'
browser = Connection(json.load(urllib.request.urlopen(endpoint + '/json/version'))['webSocketDebuggerUrl'])
pid = next(p['id'] for p in browser.call('SystemInfo.getProcessInfo')['processInfo'] if p['type'] == 'browser')
windows = json.loads(subprocess.check_output(['olo-screen', 'windows', args.screen]))
assert any(w['pid'] == pid and w['app_id'] == 'google-chrome' for w in windows)
command = [a.decode() for a in Path(f'/proc/{pid}/cmdline').read_bytes().split(b'\0') if a]
if len(command) == 1:  # Chrome can replace argv with one process-title string.
    command = shlex.split(command[0])
assert f'--user-data-dir={profile}' in command
version = browser.call('Browser.getVersion')


def free():
    subprocess.run(['olo-screen', 'wait', args.screen, '--free'], check=True, stdout=subprocess.DEVNULL)


source = (HERE / 'input.html').read_text()
operations = [
    dict(id='initial', script=''),
    dict(id='move-first-legend', script='document.getElementById("disabled-fieldset").appendChild(document.getElementById("first-legend"))'),
    dict(id='enable-fieldset', script='document.getElementById("disabled-fieldset").removeAttribute("disabled")'),
    dict(id='disable-option', script='document.getElementById("inside-disabled-select").setAttribute("disabled", "")'),
]
free()
target_id = browser.call('Target.createTarget', url='data:text/html;charset=utf-8,' + urllib.parse.quote(source))['targetId']
page = None
try:
    target = next(t for t in json.load(urllib.request.urlopen(endpoint + '/json')) if t['id'] == target_id)
    page = Connection(target['webSocketDebuggerUrl'])
    free()
    ready = page.call('Runtime.evaluate', expression='new Promise(r=>document.readyState==="complete"?r():addEventListener("load",()=>r(),{once:true}))', awaitPromise=True)
    assert 'exceptionDetails' not in ready
    rows = []
    for operation in operations:
        free()
        expression = operation['script'] + ';JSON.stringify([...document.querySelectorAll("[id]")].map(e=>({id:e.id,tag:e.localName,namespace:e.namespaceURI,disabled:e.matches(":disabled"),enabled:e.matches(":enabled")})))'
        reply = page.call('Runtime.evaluate', expression=expression, returnByValue=True)
        assert 'exceptionDetails' not in reply, reply
        rows.append(dict(id=operation['id'], elements=json.loads(reply['result']['value'])))
    args.out.write_text(json.dumps(dict(scope=__doc__, version=version, input_sha256=hashlib.sha256(source.encode()).hexdigest(), rows=rows), indent=2) + '\n')
    print('Captured', sum(len(row['elements']) for row in rows), 'independent element/state pairs')
finally:
    if page:
        page.close()
    free()
    browser.call('Target.closeTarget', targetId=target_id)
    browser.close()
