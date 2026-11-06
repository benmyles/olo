#!/usr/bin/python3
"""Frozen before/after release scroll and idle samples on an OWN Agent Screen.
Usage: SCREEN /var/tmp/OWNED OUTPUT.json; eval that screen's environment first.
Expects browser-before/browser-after. Only authored local text is loaded.
Client submission marks do not measure compositor presentation or optical latency.
"""
import hashlib
import importlib.machinery
import importlib.util
import json
import os
from pathlib import Path
import select
import subprocess
import sys

screen, directory, destination = sys.argv[1:]
assert os.environ['OLO_SCREEN'] == screen
root = Path(directory).resolve()
assert root.is_relative_to('/var/tmp')
loader = importlib.machinery.SourceFileLoader('scroll_check', 'tools/dev/scroll-check')
scroll = importlib.util.module_from_spec(importlib.util.spec_from_loader(loader.name, loader))
loader.exec_module(scroll)
fixture = root / 'scroll.html'
fixture.write_text('<!doctype html><title>Atomic layout cost fixture</title>'
    '<style>body{margin:20px}p{line-height:28px}</style><h1>Atomic layout cost fixture</h1>' +
    '<p>office alpha beta gamma<wbr> delta<br>After forced break.</p>' * 1000)


def run(*args):
    return subprocess.check_output(['olo-screen', *map(str, args)], text=True)


class Reports(scroll.Check):
    """Retain the idle offset and report timestamp missing from legacy analysis."""
    def synth(self, events):
        for event in events:
            if self.first_report is None and (event['type'] == 'wheel' or event['type'] == 'scroll' and event.get('dy')):
                self.first_report = event.get('at_us', scroll.now_us())
        super().synth(events)

    def marks(self):
        marks = super().marks()
        if self.before:
            if marks:
                self.last_y = marks[-1]['scrolls'][0][1]
            self.baseline = self.last_y
            self.before = False
        else:
            self.moving = next((m for m in marks if abs(m['scrolls'][0][1] - self.baseline) > 1e-3), None)
            if marks:
                self.last_y = marks[-1]['scrolls'][0][1]
        return marks

    def run(self, kind, rate, speed):
        self.before, self.first_report, self.moving = True, None, None
        result = super().run(kind, rate, speed)
        assert self.first_report is not None and self.moving is not None, result
        result['legacy_first_move_ms'] = result.pop('first_move_ms')
        result['report_to_first_moving_client_ms'] = (self.moving['at_us'] - self.first_report) / 1000
        result['first_report_at_us'] = self.first_report
        result['baseline_y'] = self.baseline
        result['first_moving_mark'] = self.moving
        return result


rows = []
assert not any(w['app_id'] == 'olo.browser' for w in json.loads(run('windows', screen))), 'Start with no Browser on this owned screen; a surviving instance can take a frozen launch'
for variant in ['before', 'after']:
    binary = root / f'browser-{variant}'
    pid = int(run('run', screen, binary, fixture))
    try:
        run('wait', screen, '--text', 'After forced break.', '--timeout', 10000)
        run('resize', screen, '1000x700')
        run('wait', screen, '--idle')
        windows = json.loads(run('windows', screen))
        owned = [w for w in windows if w['pid'] == pid]
        assert len(owned) == 1, 'Frozen launch was forwarded or failed to map; no measurement accepted'
        window = owned[0]
        frame = window.get('content') or window['frame']
        check = object.__new__(Reports)
        check.last_y = 0.
        check.sock, check.runtime = os.environ['OLO_SOCKET'], os.environ['XDG_RUNTIME_DIR']
        check.pid, check.sign = pid, 1
        check.at = (frame['x'] + frame['width'] / 2, frame['y'] + frame['height'] / 2)
        bursts = []
        for repeat in range(3):
            for kind in ['drag', 'flick', 'wheel']:
                run('wait', screen, '--free')
                result = check.run(kind, 90, 900)
                assert 'error' not in result, result
                bursts.append(dict(repeat=repeat, **result))
        run('wait', screen, '--idle')
        idle = json.loads(subprocess.check_output(['/usr/bin/python3', 'tools/dev/cost', str(pid), '3', '--json'], text=True))
        status = dict(line.split(':', 1) for line in Path(f'/proc/{pid}/status').read_text().splitlines() if ':' in line)
        rows.append(dict(variant=variant, binary_sha256=hashlib.file_digest(binary.open('rb'), 'sha256').hexdigest(),
            rss_kib=int(status['VmRSS'].split()[0]), hwm_kib=int(status['VmHWM'].split()[0]), idle=idle, bursts=bursts,
            compositor=scroll.req(check.sock, {'type': 'timings'})))
        Path(destination).write_text(json.dumps(dict(scope=__doc__, fixture_sha256=hashlib.file_digest(fixture.open('rb'), 'sha256').hexdigest(), rows=rows), indent=2) + '\n')
        print(variant, 'RSS', rows[-1]['rss_kib'], 'idle', idle['total'], flush=True)
    finally:
        for w in json.loads(run('windows', screen)):
            if w['pid'] == pid:
                # Quit the measured process, then wait for it to exit. A
                # close request alone can race the next single-instance launch.
                subprocess.run(['olo-screen', 'invoke', screen, 'quit', '--window', str(w['id'])], stdout=subprocess.DEVNULL, check=False)
        try:
            fd = os.pidfd_open(pid)
        except ProcessLookupError:
            pass
        else:
            try:
                assert select.select([fd], [], [], 10)[0], 'Measured Browser did not exit; do not launch another variant'
            finally:
                os.close(fd)
