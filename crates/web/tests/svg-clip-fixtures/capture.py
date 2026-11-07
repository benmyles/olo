#!/usr/bin/python3
"""Capture original SVG page geometry and pixels in an owned Chrome Agent Screen.

--screen NAME --profile /var/tmp/OWNED/profile --out /var/tmp/OWNED/chrome.json
Start Chrome through olo-screen/olo-chrome with remote-debugging-port=0.
Native Wayland scales are changed live; no device emulation or site acceptance.
The new passive fixture tab is closed and screen settings restored afterward.
"""
import argparse
import base64
import copy
import time
import select
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
sys.path.insert(0, str(Path.cwd() / 'tools' / 'browser'))
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


deadline = time.monotonic() + 15
browser = None
while True:
    try:
        port = int((profile / 'DevToolsActivePort').read_text().splitlines()[0])
        endpoint = f'http://127.0.0.1:{port}'
        browser = Connection(json.load(urllib.request.urlopen(endpoint + '/json/version', timeout=1))['webSocketDebuggerUrl'], timeout=3)
        pid = next(p['id'] for p in browser.call('SystemInfo.getProcessInfo')['processInfo'] if p['type'] == 'browser')
        if any(w['pid'] == pid and w['app_id'] == 'google-chrome' for w in json.loads(run('windows', args.screen))):
            break
    except (OSError, EOFError):
        pass
    if browser:
        browser.close()
        browser = None
    assert time.monotonic() < deadline, 'Owned Chrome startup was not verified'
    time.sleep(.1)
command = [a.decode() for a in Path(f'/proc/{pid}/cmdline').read_bytes().split(b'\0') if a]
if len(command) == 1:
    command = shlex.split(command[0])
assert f'--user-data-dir={profile}' in command
version = browser.call('Browser.getVersion')
original = ipc({'type': 'get_settings'})['settings']
cases = json.loads((HERE / 'cases.json').read_text())
base="html,body{margin:0;padding:0}svg{display:block}body{width:128px;font-size:0;line-height:0;font-family:\"Olo Inline plain\"}"
source = '<!doctype html><meta charset=utf-8><title>Olo SVG pixels</title><style id=sheet></style><main id=probe></main>'
rows, page, target_id = [], None, None
try:
    free()
    target_id = browser.call('Target.createTarget', url='about:blank')['targetId']
    target = next(t for t in json.load(urllib.request.urlopen(endpoint + '/json')) if t['id'] == target_id)
    page = Connection(target['webSocketDebuggerUrl'], timeout=40)
    page.call('Page.enable')
    free()
    page.call('Emulation.clearDeviceMetricsOverride')
    free()
    navigation = page.call('Page.navigate', url='data:text/html;charset=utf-8,' + urllib.parse.quote(source))
    assert not navigation.get('errorText'), navigation
    deadline = time.monotonic() + 20
    while True:
        try:
            state = page.call('Runtime.evaluate', expression='JSON.stringify({url:location.href,ready:document.readyState,probe:!!document.getElementById("probe")})', returnByValue=True)
            state = json.loads(state['result']['value'])
            if state['url'].startswith('data:text/html') and state['ready'] == 'complete' and state['probe']:
                break
        except RuntimeError as error:
            assert 'context' in str(error).lower(), error
        assert time.monotonic() < deadline, 'Original fixture did not load'
        time.sleep(.1)
    font=base64.b64encode((HERE.parent/'inline-fixtures/plain.ttf').read_bytes()).decode()
    free()
    reply=page.call('Runtime.evaluate',expression="(async()=>{const f=new FontFace('Olo Inline plain','url(data:font/ttf;base64,"+font+")');await f.load();document.fonts.add(f);return f.status})()",awaitPromise=True,returnByValue=True)
    assert reply.get('result',{}).get('value')=='loaded',reply
    for scale in [1, 1.25, 1.5, 1.75, 2]:
        settings=copy.deepcopy(original);settings['display']['scale']=scale
        for output in settings['display'].get('outputs',{}).values():output.pop('scale',None)
        assert ipc({'type':'set_settings','settings':settings})['type']=='ok'
        run('wait',args.screen,'--idle')
        for case in cases:
            free()
            expression = '(()=>{const c=CASE;document.getElementById("sheet").textContent=BASE+c.css;const main=document.querySelector("main");main.removeAttribute("id");main.innerHTML=c.html;const e=document.getElementById("probe");const b=e.getBoundingClientRect();return JSON.stringify({dpr:devicePixelRatio,bounds:{x:b.x,y:b.y,width:b.width,height:b.height}})})()'.replace("CASE",json.dumps(case)).replace("BASE",json.dumps(base))
            reply=page.call('Runtime.evaluate',expression=expression,returnByValue=True)
            assert 'exceptionDetails'not in reply,reply
            result=json.loads(reply['result']['value']);assert result['dpr']==scale,result
            free()
            shot=page.call('Page.captureScreenshot',format='png',clip=dict(x=0,y=0,width=128,height=80,scale=1),captureBeyondViewport=True)
            filename=f"{case['id']}-{scale}.png"
            data=base64.b64decode(shot['data']);(args.out.parent/filename).write_bytes(data)
            rows.append(dict(scale=scale,id=case['id'],png=filename,sha256=hashlib.sha256(data).hexdigest(),**result))
    args.out.write_text(json.dumps(dict(scope='Original SVG rectangular clip page bounding rectangles and 128x80 CSS-pixel crops at five native scales. No website acceptance.',version=version,cases_sha256=hashlib.sha256((HERE/'cases.json').read_bytes()).hexdigest(),rows=rows),indent=2)+'\n')
    print('Captured',len(rows),'independent SVG page rows')

finally:
    ipc({'type':'set_settings','settings':original})
    if page:
        page.close()
    if target_id:
        free()
        browser.call('Target.closeTarget', targetId=target_id)
    free()
    try:
        browser.call('Browser.close')
    except EOFError:
        pass
    browser.close()
    try:
        fd = os.pidfd_open(pid)
    except ProcessLookupError:
        pass
    else:
        assert select.select([fd], [], [], 10)[0], 'Owned Chrome did not exit'
        os.close(fd)
