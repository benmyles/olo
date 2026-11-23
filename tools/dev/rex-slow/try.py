#!/usr/bin/python3
"""The rig's own tries (tools/dev/rex-slow/rig.py), each printing the pictures it took (REX_SLOW_DIR/shots):

    try.py typed PROGRAM [DELAY_MS] [--off]   a letter typed and let come back, six more typed, pictured before the
                                              host can have echoed them, and once settled (--off: without typing
                                              shown at once, to compare)
    try.py stall PROGRAM                      the link frozen with letters typed into it: what the pane and the
                                              status bar show meanwhile, and after
    try.py connect [DELAY_MS...]              how long a host's terminal takes to show, by the link's delay

PROGRAM: shell, box, box-nosync, line (tui.py's stand-ins), or a command line."""
import glob, os, signal, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from rig import *
what, args = (sys.argv[1] if len(sys.argv) > 1 else ""), sys.argv[2:]
program = next((a for a in args if not a.isdigit() and not a.startswith("--")), "shell")
if what == "typed":
    ahead = "--off" not in args
    delay = int(next((a for a in args if a.isdigit()), "600"))
    with Rig(program, delay=delay, type_ahead=ahead) as r:
        tag = f"{program}-{'on' if ahead else 'off'}"
        print(f"the terminal showed {r.ready_after:.1f} s after the window opened (the link: {delay} ms each way)")
        r.type("e"); time.sleep(3.2 * delay / 1000 + 0.6)
        print(r.shot(f"{tag}-1-proven"))
        t = time.monotonic()
        r.type("cho hi", apart=0.03)
        print(r.shot(f"{tag}-2-typed"), f"taken {time.monotonic() - t:.2f} s after the first of six letters (the link is {2 * delay / 1000:.1f} s there and back)")
        time.sleep(8 * delay / 1000 + 1.0)
        print(r.shot(f"{tag}-3-settled"))
elif what == "stall":
    with Rig(program, delay=150) as r:
        r.type("e"); time.sleep(1.5)
        link = r.procs[1]
        os.killpg(link.pid, signal.SIGSTOP)
        try:
            r.type("cho hi", apart=0.03)
            time.sleep(3.6)
            print(r.shot(f"{program}-stalled"))
        finally:
            os.killpg(link.pid, signal.SIGCONT)
        time.sleep(2.5)
        print(r.shot(f"{program}-stall-over"))
elif what == "connect":
    for delay in [int(a) for a in args if a.isdigit()] or [0, 150, 300, 600]:
        with Rig("shell", delay=delay) as r:
            print(f"{delay} ms each way: the terminal shown {r.ready_after:.1f} s after the window opened", flush=True)
else:
    raise SystemExit(__doc__)
print("crash reports:", len(glob.glob(f"{D}/state/olo/crashes/*.json")))
