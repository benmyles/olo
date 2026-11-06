#!/usr/bin/python3
"""Release keyboard-to-moving-client-frame sample on an OWN Agent Screen.
Args: SCREEN /var/tmp/OWNED OUTPUT.json. Expects frozen browser-after there.
No comparison is possible for the newly added keyboard behavior. This is
client submission evidence, not compositor presentation or website acceptance.
"""
import hashlib
import json
import os
from pathlib import Path
import socket
import statistics
import subprocess
import sys
import time

screen, directory, output = sys.argv[1:]
assert os.environ['OLO_SCREEN'] == screen
root = Path(directory)
assert str(root).startswith('/var/tmp/')
binary = root / 'browser-after'


def run(*args):
    return subprocess.check_output(['olo-screen', *map(str, args)], text=True)


def request(path, body):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(5)
        s.connect(path)
        s.sendall((json.dumps(body) + '\n').encode())
        return json.loads(s.makefile().readline())


def now():
    return time.clock_gettime_ns(time.CLOCK_MONOTONIC) // 1000


fixture = root / 'keyboard-cost.html'
fixture.write_text('<!doctype html><title>Keyboard cost fixture</title>'
    '<style>body{margin:0}a:focus-visible{color:purple}</style>' +
    ''.join(f'<div><a href="https://example.test/{i}">Link {i}</a></div>' for i in range(1000)) +
    '<div><a tabindex=1 href="https://example.test/end">First tab target</a></div>' +
    '<div style="height:600px"></div>')
pid = int(run('run', screen, binary, fixture))
try:
    run('wait', screen, '--text', 'Keyboard cost fixture', '--timeout', 10000)
    frame_socket = str(Path(os.environ['XDG_RUNTIME_DIR']) / f'olo-menus-{pid}.sock')
    rows = []
    for repeat in range(10):
        # Reload preserves scroll; explicitly seek the top before each sample.
        run('set', screen, '--name', 'Address', fixture.as_uri() + '#top')
        run('click', screen, '--role', 'Button', '--name', 'Go')
        run('wait', screen, '--text', 'Link 0', '--timeout', 10000)
        run('invoke', screen, 'reload')
        run('wait', screen, '--text', 'Link 0', '--timeout', 10000)
        run('wait', screen, '--idle')
        request(frame_socket, {'frames': True})
        run('wait', screen, '--free')
        submitted = now()
        request(os.environ['OLO_SOCKET'], {'type': 'synthetic', 'events': [
            {'type': 'key', 'key': 15, 'pressed': True},
            {'type': 'key', 'key': 15, 'pressed': False}]})
        run('wait', screen, '--text', 'First tab target', '--timeout', 10000)
        run('wait', screen, '--idle')
        frames = request(frame_socket, {'frames': True})
        marks = [m for w in frames for m in w.get('marks', []) if m['at_us'] >= submitted]
        moving = next(m for m in marks if m['scrolls'] and m['scrolls'][0][1] > 10000)
        focused = [n for w in json.loads(run('tree', screen, '--json')) if w['app_id'] == 'olo.browser'
                   for n in w['nodes'] if n.get('name') == 'First tab target' and n.get('focused')]
        assert focused
        rows.append({'submitted_at_us': submitted, 'first_moving_client_frame_ms':
            (moving['at_us'] - submitted) / 1000, 'frames': frames})
    idle = json.loads(subprocess.check_output(['/usr/bin/python3', 'tools/dev/cost', str(pid), '3', '--json'], text=True))
    status = dict(line.split(':', 1) for line in Path(f'/proc/{pid}/status').read_text().splitlines() if ':' in line)
    result = {'scope': __doc__, 'binary_sha256': hashlib.file_digest(binary.open('rb'), 'sha256').hexdigest(),
        'links': 1001, 'rows': rows, 'rss_kib': int(status['VmRSS'].split()[0]), 'idle': idle,
        'median_first_moving_client_frame_ms': statistics.median(r['first_moving_client_frame_ms'] for r in rows)}
    Path(output).write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({k: v for k, v in result.items() if k not in ['scope', 'rows', 'idle']}))
finally:
    for window in json.loads(run('windows', screen)):
        if window['pid'] == pid:
            run('close', screen, '--window', window['id'])
