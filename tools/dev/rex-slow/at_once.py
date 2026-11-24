#!/usr/bin/python3
"""Run a rig script for several jobs at once, each in a rig of its own (its own REX_SLOW_DIR and REX_SLOW_NAME, so
their scratch servers, homes and headless sessions never meet), and say how each ended.

    tools/dev/rex-slow/at_once.py SCRIPT JOB… [-- ARG…]

Runs `SCRIPT JOB ARG…` for every JOB, all at the same time; each one's output goes to
$REX_SLOW_DIR-JOBS/JOB.log (REX_SLOW_DIR: /var/tmp/olo-rex-slow unless set), printed after it ends. Exits 0 when all
did. The dev server has 32 processors: fourteen picture jobs took 13 s at once and seven minutes one after another
(2026-11-24; Ben: "now that you're running on a beefy server see if you can increase concurrency for stuff like taking
the pics"). Each job's session is `rx-JOB` (dots made underscores): keep JOBs short and apart."""
import os, subprocess, sys, time
if len(sys.argv) < 3:
    raise SystemExit(__doc__)
script, rest = sys.argv[1], sys.argv[2:]
jobs, extra = (rest[:rest.index("--")], rest[rest.index("--") + 1:]) if "--" in rest else (rest, [])
base = os.environ.get("REX_SLOW_DIR", "/var/tmp/olo-rex-slow")
logs = f"{base}-jobs"
os.makedirs(logs, exist_ok=True)
began, procs = time.time(), {}
for job in jobs:
    env = {**os.environ, "REX_SLOW_DIR": f"{base}-{job}", "REX_SLOW_NAME": f"rx-{job}".replace(".", "_")}
    procs[job] = subprocess.Popen(["/usr/bin/python3", script, job, *extra], env=env, stdout=open(f"{logs}/{job}.log", "w"), stderr=subprocess.STDOUT)
failed = 0
for job, p in procs.items():
    # (Each waited for with a deadline: a rig that hangs is ended by its pid, and said.)
    try:
        code = p.wait(timeout=max(1.0, 900 - (time.time() - began)))
    except subprocess.TimeoutExpired:
        p.kill(); p.wait(); code = "ended: it ran past 15 minutes"
    failed += code != 0
    print(f"=== {job}: {'ok' if code == 0 else code} ({time.time() - began:.0f} s)")
    print(open(f"{logs}/{job}.log").read().rstrip())
sys.exit(1 if failed else 0)
