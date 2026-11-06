#!/usr/bin/python3
"""Frozen release ordinary text cost around CSS env support.

ROOT OUTPUT.json; ROOT is owned /var/tmp scratch with page/scene-before/after.
Stop own builds and GUI checks first. Seven rotated samples,
one performance CPU. Wait for rustc; record concurrent Cargo/test processes
separately (these samples do not establish complete host isolation).
Chrome environment fixtures provide independent semantic evidence separately.
No network, JS, form interaction, GPU, presentation or website speed coverage.
"""
import hashlib
import json
import os
from pathlib import Path
import select
import statistics
import subprocess
import sys

root = Path(sys.argv[1]).resolve()
assert root.is_relative_to('/var/tmp')
destination = Path(sys.argv[2])
allowed = sorted(os.sched_getaffinity(0))


def frequency(cpu):
    path = Path(f'/sys/devices/system/cpu/cpu{cpu}/cpufreq/cpuinfo_max_freq')
    return int(path.read_text()) if path.exists() else 0


def context():
    processes = {}
    for proc in Path('/proc').iterdir():
        if proc.name.isdigit():
            try:
                name = (proc / 'comm').read_text().strip()
                if name in ('cargo', 'rustc'):
                    processes[proc.name] = name
            except OSError:
                pass
    return dict(loadavg=os.getloadavg(), build_processes=processes)


cpu = max(allowed, key=frequency)
os.sched_setaffinity(0, {cpu})
rows, discarded, waits, hashes = [], [], [], {}
for mode, variants in [('page', ['before', 'after']), ('scene', ['before', 'after'])]:
    for repeat in range(7):
        for variant in variants if repeat % 2 == 0 else variants[::-1]:
            binary = root / f'{mode}-{variant}'
            with binary.open('rb') as stream:
                hashes[binary.name] = hashlib.file_digest(stream, 'sha256').hexdigest()
            while True:
                before = context()
                compilers = [pid for pid, name in before['build_processes'].items() if name == 'rustc']
                if compilers:
                    waits.append(before)
                    fds = []
                    try:
                        for pid in compilers:
                            try:
                                fds.append(os.pidfd_open(int(pid)))
                            except ProcessLookupError:
                                pass
                        if fds:
                            select.select(fds, [], [], 20)
                    finally:
                        for fd in fds:
                            os.close(fd)
                    continue
                rss = root / 'environment-cost.rss'
                data = json.loads(subprocess.check_output(['/usr/bin/time', '-f', '%M', '-o', str(rss), str(binary), '60'], text=True))
                assert data['features'] == [], binary.name
                observation = dict(mode=mode, variant=variant, repeat=repeat, rss_kib=int(rss.read_text()), before_context=before, after_context=context(), **data)
                if 'rustc' in observation['after_context']['build_processes'].values():
                    discarded.append(observation)
                    continue
                rows.append(observation)
                print(mode, repeat, variant, flush=True)
                break
    group = [r for r in rows if r['mode'] == mode]
    references = {r['elements']: r for r in group[0]['rows']}
    keys = ['source_sha256', 'geometry_sha256', 'commands'] + (['pixels_sha256'] if mode == 'scene' else [])
    for observation in group:
        for row in observation['rows']:
            ref = references[row['elements']]
            for key in keys:
                assert row[key] == ref[key], (mode, key)
    for identity in references:
        metrics = ['pipeline_us' if mode == 'page' else 'scene_us']
        print(mode, identity, {v: {k: statistics.median(c[k] for r in group if r['variant'] == v for c in r['rows'] if c['elements'] == identity) for k in metrics} for v in variants}, flush=True)
destination.write_text(json.dumps(dict(scope=__doc__, before='3c9bc54a (web core unchanged since 85200762; frozen default-feature examples)', cpu=cpu, allowed_cpus=allowed, binaries=hashes, observations=rows, discarded=discarded, build_waits=waits), indent=2) + '\n')
