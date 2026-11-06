#!/usr/bin/python3
"""Before/after CSS font-grid visuals on an OWN Agent Screen.
Usage: SCREEN /var/tmp/OWNED OUTPUT_DIR; eval the screen environment first.
Expects frozen browser-before/browser-after. Authored geometry only; no website acceptance.
"""
import copy
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import sys

screen, directory, destination = sys.argv[1:]
assert os.environ['OLO_SCREEN'] == screen
root, out = Path(directory).resolve(), Path(destination).resolve()
assert root.is_relative_to('/var/tmp') and out.is_relative_to('/var/tmp')
out.mkdir(parents=True, exist_ok=True)
fixture = root / 'font-grid.html'
fixture.write_text('''<!doctype html><title>Font grid fixture</title><style>
body{margin:20px}h1{font-size:24px;line-height:30px}
p{font-size:20px;line-height:24.3px;width:183px;margin:0 0 12px}
.narrow{width:75px}.small{font-size:13.5px;line-height:18px;width:240px}
</style><h1>Font grid fixture</h1><p>Atomic text wraps here at a fractional line height.</p>
<p class=narrow>Optional<wbr>break<br>After break</p>
<p class=small>Small fractional text and glyph advances across several lines.</p>''')


def run(*args):
    return subprocess.check_output(['olo-screen', *map(str, args)], text=True)


def ipc(body):
    run('wait', screen, '--free')
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(5)
        s.connect(os.environ['OLO_SOCKET'])
        s.sendall((json.dumps(body) + '\n').encode())
        return json.loads(s.makefile().readline())


original = ipc({'type': 'get_settings'})['settings']
rows = []
try:
    for variant in ['before', 'after']:
        binary = root / f'browser-{variant}'
        pid = int(run('run', screen, binary, fixture))
        try:
            run('wait', screen, '--text', 'Font grid fixture', '--timeout', 10000)
            for mode in ['light', 'dark', 'night']:
                for scale in [1, 1.25, 1.5, 1.75, 2]:
                    settings = copy.deepcopy(original)
                    settings['appearance']['theme'] = 'dark' if mode == 'night' else mode
                    settings['appearance']['night_light']['mode'] = 'on' if mode == 'night' else 'off'
                    settings['appearance']['night_light']['temperature'] = 4500
                    settings['display']['scale'] = scale
                    for o in settings['display'].get('outputs', {}).values():
                        o.pop('scale', None)
                    assert ipc({'type': 'set_settings', 'settings': settings})['type'] == 'ok'
                    run('resize', screen, '1000x700')
                    run('wait', screen, '--text', 'Small fractional text', '--timeout', 10000)
                    run('wait', screen, '--idle')
                    window = next(w for w in json.loads(run('tree', screen, '--json')) if w['app_id'] == 'olo.browser')
                    review = json.loads(run('review', screen, '--window', window['window'], '--json'))
                    assert all(not r['findings'] for r in review), review
                    path = out / f'{variant}-{mode}-{scale}.png'
                    run('look', screen, path, '--window', window['window'], '--no-pointer')
                    rows.append(dict(variant=variant, mode=mode, scale=scale, review=review, tree=window,
                        binary_sha256=hashlib.file_digest(binary.open('rb'), 'sha256').hexdigest(),
                        image_sha256=hashlib.file_digest(path.open('rb'), 'sha256').hexdigest()))
        finally:
            for w in json.loads(run('windows', screen)):
                if w['pid'] == pid:
                    run('close', screen, '--window', w['id'])
finally:
    ipc({'type': 'set_settings', 'settings': original})
(out / 'report.json').write_text(json.dumps(dict(scope=__doc__, fixture_sha256=hashlib.file_digest(fixture.open('rb'), 'sha256').hexdigest(), rows=rows), indent=2) + '\n')
print('Passed', len(rows), 'native reviews')
