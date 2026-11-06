#!/usr/bin/python3
"""Native navigation on an OWN Agent Screen; authored fixtures accept no sites.
Start release olo-browser there, eval olo-screen env, then run NAME OUTPUT_DIR.
Checks canonical redirects, in-document identity/no I/O, history, keyboard focus
and scroll restoration at all native modes/scales. HTTP server is test-only.
"""
import copy
import http.server
import json
import os
from pathlib import Path
import socket
import signal
import subprocess
import sys
import threading
import time

screen, destination = sys.argv[1:]
assert os.environ.get('OLO_SCREEN') == screen
out = Path(destination)
out.mkdir(parents=True, exist_ok=True)
requests = []


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        requests.append(self.path)
        if self.path == '/redirect':
            self.send_response(302)
            self.send_header('Location', '/nav')
            self.send_header('Content-Length', '0')
            self.end_headers()
            return
        if self.path == '/other':
            body = '<title>Other destination</title><h1>Other destination</h1>'
        else:
            body = '''<title>Navigation fixture</title>
<style>body{margin:20px}a{color:blue}a:focus-visible{color:purple}
p:target{color:red}h1{font-size:28px}</style>
<h1>Navigation fixture</h1><a href='#section'>Jump section</a>
<div style='height:1500px'></div>
<p id=section>Section target</p><a href='#top'>Back to top</a>
<a href='/other'>Other page</a><div style='height:1000px'></div>'''
        data = ('<!doctype html><meta charset=utf-8>' + body).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'text/html; charset=utf-8')
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
base = f'http://127.0.0.1:{server.server_port}'


def run(*args):
    return subprocess.check_output(['olo-screen', *map(str, args)], text=True)


def ipc(request):
    run('wait', screen, '--free')
    with socket.socket(socket.AF_UNIX) as s:
        s.connect(os.environ['OLO_SOCKET'])
        s.sendall((json.dumps(request) + '\n').encode())
        return json.loads(s.makefile().readline())


def tree():
    return next(w for w in json.loads(run('tree', screen, '--json')) if w['app_id'] == 'olo.browser')


def node(role, name):
    return next(n for n in tree()['nodes'] if n['role'] == role and n.get('name') == name)


def document():
    return json.loads(node('Document', 'Page')['value'])


def settled(text):
    run('wait', screen, '--text', text, '--timeout', 10000)
    run('wait', screen, '--idle')


def invoke(action, text):
    run('invoke', screen, action)
    settled(text)


original = ipc({'type': 'get_settings'})['settings']
rows = []
try:
    for theme in ['light', 'dark', 'night']:
        for scale in [1, 1.25, 1.5, 1.75, 2]:
            settings = copy.deepcopy(original)
            settings['appearance']['theme'] = 'dark' if theme == 'night' else theme
            settings['appearance']['night_light']['mode'] = 'on' if theme == 'night' else 'off'
            settings['appearance']['night_light']['temperature'] = 4500
            settings['display']['scale'] = scale
            for output in settings['display'].get('outputs', {}).values():
                output.pop('scale', None)
            assert ipc({'type': 'set_settings', 'settings': settings})['type'] == 'ok'
            run('set', screen, '--name', 'Address', base + '/redirect')
            run('click', screen, '--role', 'Button', '--name', 'Go')
            settled('Navigation fixture')
            initial = document()
            assert initial['url'] == base + '/nav', initial
            before = len(requests)
            # Native keys and documented agent menu commands share control.
            run('key', screen, 'Tab')
            run('wait', screen, '--idle')
            assert node('Link', 'Jump section').get('focused')
            run('key', screen, 'Cmd+L')
            run('wait', screen, '--idle')
            assert not node('Link', 'Jump section').get('focused')
            invoke('next-link', 'Navigation fixture')
            assert node('Link', 'Jump section').get('focused')
            run('key', screen, 'Enter')
            settled('Section target')
            fragment = document()
            assert fragment['url'] == base + '/nav#section', fragment
            assert fragment['document'] == initial['document']
            assert len(requests) == before, requests
            # Snapshot labels describe glyph ink; the anchor aligns its box.
            assert 0 <= node('Label', 'Section target')['y'] - node('Document', 'Page')['y'] < 12
            review = json.loads(run('review', screen, '--window', tree()['window'], '--json'))
            assert all(not r['findings'] for r in review), review
            run('look', screen, out / f'{theme}-{scale}.png', '--window', tree()['window'], '--no-pointer')
            invoke('back', 'Jump section')
            assert document()['url'] == base + '/nav'
            assert document()['document'] == initial['document']
            invoke('forward', 'Section target')
            assert document()['document'] == initial['document']
            assert len(requests) == before
            section_y = node('Label', 'Section target')['y']
            run('click', screen, '--role', 'Link', '--name', 'Other page')
            settled('Other destination')
            assert document()['url'] == base + '/other'
            assert requests[before:] == ['/other']
            invoke('back', 'Section target')
            restored = document()
            assert restored['url'] == base + '/nav#section'
            assert restored['document'] != initial['document']  # Refetch, no BFCache yet.
            assert abs(node('Label', 'Section target')['y'] - section_y) <= 1 / scale
            assert requests[before:] == ['/other', '/nav']
            before = len(requests)
            run('click', screen, '--role', 'Link', '--name', 'Back to top')
            settled('Jump section')
            assert document()['url'] == base + '/nav#top'
            assert document()['document'] == restored['document']
            assert len(requests) == before
            assert not node('Button', 'Forward')['enabled']  # New branch discarded it.
            run('key', screen, 'Alt+Left')
            settled('Section target')
            assert document()['url'] == base + '/nav#section'
            assert len(requests) == before
            rows.append({'theme': theme, 'scale': scale, 'initial': initial,
                         'fragment': fragment, 'restored': restored, 'review': review})
    # Restart only this owned screen's session; exercise the native re-exec
    # state path with a live HTTP fixture and forward history still present.
    previous = document()
    previous_y = node('Label', 'Section target')['y']
    meta = json.loads((Path(os.environ['XDG_RUNTIME_DIR']) / 'olo-screens' / (screen + '.json')).read_text())
    session_pid = meta['pid']
    assert meta['name'] == screen and meta['olo_socket'] == os.environ['OLO_SOCKET'] and meta['wayland_display'] == os.environ['WAYLAND_DISPLAY']
    # A workspace rebuild can replace this owned screen's executable while
    # it runs. Linux appends this suffix to /proc's link for that valid case.
    executable = os.readlink(f'/proc/{session_pid}/exe').removesuffix(' (deleted)')
    assert Path(executable).name == 'olo-session'
    assert b'--headless' in Path(f'/proc/{session_pid}/cmdline').read_bytes().split(b'\0')
    run('wait', screen, '--free')
    reload_started = time.monotonic()
    os.kill(session_pid, signal.SIGHUP)
    deadline = time.monotonic() + 25
    while True:
        try:
            current = document()
            if current['navigation'] < previous['navigation'] and current['url'] == previous['url']:
                break
        except (subprocess.CalledProcessError, StopIteration, KeyError, json.JSONDecodeError):
            pass
        assert time.monotonic() < deadline, 'Owned-screen re-exec did not restore navigation'
        time.sleep(.1)
    settled('Section target')
    assert abs(node('Label', 'Section target')['y'] - previous_y) <= .5
    before = len(requests)
    invoke('forward', 'Jump section')
    assert document()['url'] == base + '/nav#top'
    assert len(requests) == before
    invoke('back', 'Section target')
    assert document()['url'] == previous['url']
    assert len(requests) == before
    (out / 'report.json').write_text(json.dumps({'scope': __doc__, 'rows': rows,
        'requests': requests, 'reload': {'before': previous, 'restored': current,
        'section_y': previous_y, 'forward_then_back_without_requests': True,
        'reload_and_history_checks_seconds': time.monotonic() - reload_started},
        'accepted_websites': 0}, indent=2) + '\n')
    print(json.dumps({'reviews': len(rows), 'requests': len(requests), 'report': str(out / 'report.json')}))
finally:
    ipc({'type': 'set_settings', 'settings': original})
    server.shutdown()
    server.server_close()
