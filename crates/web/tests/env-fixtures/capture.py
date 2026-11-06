#!/usr/bin/python3
"""Capture original env fixtures in an owned Chrome Agent Screen.

--screen NAME --profile /var/tmp/OWNED/profile --out /var/tmp/OWNED/chrome.json
Start Chrome through olo-screen/olo-chrome with remote-debugging-port=0.
Native Wayland scales are changed live; no device emulation or site acceptance.
The new passive fixture tab is closed and screen settings restored afterward.
"""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import shlex
import socket
import subprocess
import sys
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


def run(*arguments):
    return subprocess.check_output(['olo-screen', *map(str, arguments)], text=True)


def free():
    run('wait', args.screen, '--free')


def ipc(body):
    if body['type'] != 'get_settings':
        free()
    with socket.socket(socket.AF_UNIX) as stream:
        stream.connect(os.environ['OLO_SOCKET'])
        stream.sendall((json.dumps(body) + '\n').encode())
        return json.loads(stream.makefile().readline())


port = int((profile / 'DevToolsActivePort').read_text().splitlines()[0])
endpoint = f'http://127.0.0.1:{port}'
browser = Connection(json.load(urllib.request.urlopen(endpoint + '/json/version'))['webSocketDebuggerUrl'])
pid = next(p['id'] for p in browser.call('SystemInfo.getProcessInfo')['processInfo'] if p['type'] == 'browser')
assert any(w['pid'] == pid and w['app_id'] == 'google-chrome' for w in json.loads(run('windows', args.screen)))
command = [a.decode() for a in Path(f'/proc/{pid}/cmdline').read_bytes().split(b'\0') if a]
if len(command) == 1:
    command = shlex.split(command[0])
assert f'--user-data-dir={profile}' in command
version = browser.call('Browser.getVersion')
original = ipc({'type': 'get_settings'})['settings']
cases = json.loads((HERE / 'cases.json').read_text())
rows, page, target_id = [], None, None
try:
    free()
    target_id = browser.call('Target.createTarget', url='data:text/html,<title>Olo original env fixtures</title><div id=probe></div>')['targetId']
    target = next(t for t in json.load(urllib.request.urlopen(endpoint + '/json')) if t['id'] == target_id)
    page = Connection(target['webSocketDebuggerUrl'])
    free()
    page.call('Emulation.clearDeviceMetricsOverride')
    ready = page.call('Runtime.evaluate', expression='new Promise(r=>document.readyState==="complete"?r():addEventListener("load",()=>r(),{once:true}))', awaitPromise=True)
    assert 'exceptionDetails' not in ready
    for scale in [1, 1.25, 1.5, 1.75, 2]:
        settings = copy.deepcopy(original)
        settings['display']['scale'] = scale
        for output in settings['display'].get('outputs', {}).values():
            output.pop('scale', None)
        assert ipc({'type': 'set_settings', 'settings': settings})['type'] == 'ok'
        run('wait', args.screen, '--idle')
        for case in cases:
            free()
            expression = '(()=>{const e=document.getElementById("probe");e.style.cssText=' + json.dumps('padding-bottom:7px;' + case['declaration']) + ';return JSON.stringify({dpr:devicePixelRatio,padding:getComputedStyle(e).paddingBottom,specified:e.style.getPropertyValue("padding-bottom"),custom:e.style.getPropertyValue("--a")})})()'
            reply = page.call('Runtime.evaluate', expression=expression, returnByValue=True)
            assert 'exceptionDetails' not in reply, reply
            result = json.loads(reply['result']['value'])
            assert result['dpr'] == scale, result
            rows.append(dict(scale=scale, id=case['id'], **result))
    args.out.write_text(json.dumps(dict(version=version, source_sha256=hashlib.sha256((HERE / 'cases.json').read_bytes()).hexdigest(), method=__doc__, rows=rows), indent=2) + '\n')
    print('Captured', len(rows), 'env observations')
finally:
    ipc({'type': 'set_settings', 'settings': original})
    if page:
        page.close()
    if target_id:
        free()
        browser.call('Target.closeTarget', targetId=target_id)
    browser.close()
