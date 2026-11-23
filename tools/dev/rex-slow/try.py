#!/usr/bin/python3
"""The rig's own tries (tools/dev/rex-slow/rig.py), each printing the pictures it took (REX_SLOW_DIR/shots):

    try.py typed PROGRAM [DELAY_MS] [--off]   a letter typed and let come back, six more typed, pictured before the
                                              host can have echoed them, and once settled (--off: without typing
                                              shown at once, to compare)
    try.py stall PROGRAM                      the link frozen with letters typed into it: what the pane and the
                                              status bar show meanwhile, and after
    try.py connect [DELAY_MS...]              how long a host's terminal takes to show, by the link's delay
    try.py order [DELAY_MS] [--helper]        sixty letters typed quickly (several on their way at once): the host's
                                              line is what was typed, in order; --helper: sent through Olo's helper
                                              behind the link, each as it is typed
    try.py links                              addresses in a terminal (one on a row, one of a scheme of its
                                              own, one the terminal wrapped over rows, one a program named
                                              for its text, of a scheme nothing opens): each pictured with
                                              the pointer on it and the command key held (its underline,
                                              whole), and the last clicked (the status bar says it couldn't
                                              be opened: the click arrived, with the address)
    try.py copy                               a program in the terminal copies by OSC 52, a few words and then
                                              300 kB: what the session's clipboard then holds (olo-pbpaste,
                                              built in dev: cargo build -p olo-pb)
    try.py timed [DELAY_MS] [--helper]        letters typed a tenth of a second apart into the far side by itself
                                              (no app: the client crate's own measurement, tests/host.rs), each
                                              letter's echo timed, one write at a time and each sent as typed;
                                              --helper: with Olo's helper behind the link, as on a host that has it

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
elif what == "order":
    delay = int(next((a for a in args if a.isdigit()), "150"))
    with Rig("shell", delay=delay, helper="--helper" in args) as r:
        r.type("e"); time.sleep(4 * delay / 1000 + 0.6)
        text = "".join("asdfghjkl"[i % 9] for i in range(60))
        r.type(text, apart=0.02)
        time.sleep(0.15)
        print(r.shot(f"order-{'helper' if r.helper else 'plain'}-1-typed"))
        time.sleep(8 * delay / 1000 + 1.5)
        line = (r.rex("capture", "-s", "typing", "--trim").splitlines() or [""])[-1]
        print("the host's line is what was typed, in order:", line.endswith("e" + text), repr(line[-70:]))
        print(r.shot(f"order-{'helper' if r.helper else 'plain'}-2-settled"))
elif what == "links":
    import json
    # (Raw: the escapes are printf's to read, not Python's.)
    show = (r"printf 'plain https://example.com/one. and vscode://file/tmp/x.rs:3 now\n\n'; "
            r"printf 'long https://example.com/'; i=0; while [ $i -lt 30 ]; do printf 'segment%d/' $i; i=$((i+1)); done; printf 'end tail\n\n'; "
            r"printf 'named \033]8;;nothing-opens-this://named/target\033\\this text is a link\033]8;;\033\\ and this is not\n'; exec cat")
    with Rig(show, delay=0) as r:
        def walk(v, out):
            if isinstance(v, dict):
                if "role" in v: out.append(v)
                for x in v.values(): walk(x, out)
            elif isinstance(v, list):
                for x in v: walk(x, out)
            return out
        nodes = walk(r.tree(), [])
        term = next(n for n in nodes if n.get("role") == "Terminal")
        b = term.get("bounds") or term
        size = next((n.get("name") or n.get("value") or "" for n in nodes if "×" in str(n.get("name") or n.get("value") or "")), "")
        cols = int(size.split("×")[0].split()[-1]); rows = int(size.split("×")[1].split()[0])
        c = r.win["content"]
        cw, ch = b["width"] / cols, b["height"] / rows
        at = lambda col, row: (str(int(c["x"] + b["x"] + (col + 0.5) * cw)), str(int(c["y"] + b["y"] + (row + 0.5) * ch)))
        key = lambda pressed: r.ts("ipc", r.name, json.dumps({"type": "synthetic", "events": [{"type": "key", "key": 125, "pressed": pressed}]}))
        key(True); time.sleep(0.2)
        try:
            for name, col, row in (("plain", 12, 0), ("scheme", 45, 0), ("wrapped-first-row", 30, 2), ("wrapped-second-row", 10, 3), ("named", 12, 6), ("not-a-link", 40, 6)):
                r.ts("move", r.name, *at(col, row)); time.sleep(0.5)
                print(r.shot(f"links-{name}"))
            r.ts("click", r.name, *at(12, 6)); time.sleep(1.2)
        finally:
            key(False)
        said = [str(n.get("name") or n.get("value") or "") for n in walk(r.tree(), []) if "open" in str(n.get("name") or n.get("value") or "").lower() and "link" in str(n.get("name") or n.get("value") or "").lower()]
        print("after a click on the address nothing opens, the window says:", said[:3])
        print(r.shot("links-clicked"))
elif what == "copy":
    import subprocess
    # (At each line typed: a few words, then 300 kB of "b".)
    show = r'''read x; printf '\033]52;c;%s\a' "$(printf 'hello from far' | base64)"; read x; printf '\033]52;c;%s\a' "$(head -c 300000 /dev/zero | tr '\0' b | base64 -w0)"; exec cat'''
    with Rig(show, delay=0) as r:
        env = dict(os.environ)
        for line in r.ts("env", r.name).splitlines():
            line = line.removeprefix("export ").strip()
            if "=" in line:
                k, v = line.split("=", 1); env[k] = v.strip("'\"")
        def pasted():
            ran = subprocess.run([f"{R3}/target/debug/olo-pbpaste"], env=env, capture_output=True, text=True, timeout=20)
            if ran.returncode != 0: print("olo-pbpaste:", ran.returncode, ran.stderr.strip()[:200])
            return ran.stdout
        # (Each at a line typed: what is copied with nobody connected is nobody's to take.)
        r.type("\n"); time.sleep(1.5)
        print("after a few words copied there, the clipboard here holds:", repr(pasted()[:60]))
        r.type("\n"); time.sleep(2.5)
        got = pasted()
        print("after 300 kB copied there:", len(got), "characters, all b:", set(got) == {"b"})
elif what == "timed":
    import subprocess
    delay = int(next((a for a in args if a.isdigit()), "150"))
    r = Rig("shell", delay=delay, helper="--helper" in args)
    try:
        sock = r.host()
        env = {k: v for k, v in os.environ.items() if k not in ("WAYLAND_DISPLAY", "OLO_SOCKET")}
        env.update(OLO_REX_TEST_HOST=f"unix://{sock}")
        out = subprocess.run(["cargo", "test", "-q", "-p", "olo-rex-client", "--test", "host", "typing_over_the_link_is_timed", "--", "--ignored", "--nocapture"], cwd=R3, env=env, capture_output=True, text=True, timeout=900)
        print("\n".join(l for l in (out.stdout + out.stderr).splitlines() if l and not l.startswith(("warning", "package:", "workspace:", "running", "test "))))
    finally:
        r.__exit__()
elif what == "connect":
    for delay in [int(a) for a in args if a.isdigit()] or [0, 150, 300, 600]:
        with Rig("shell", delay=delay) as r:
            print(f"{delay} ms each way: the terminal shown {r.ready_after:.1f} s after the window opened", flush=True)
else:
    raise SystemExit(__doc__)
print("crash reports:", len(glob.glob(f"{D}/state/olo/crashes/*.json")))
