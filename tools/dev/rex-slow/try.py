#!/usr/bin/python3
"""The rig's own tries (tools/dev/rex-slow/rig.py), each printing the pictures it took (REX_SLOW_DIR/shots):

    try.py typed PROGRAM [DELAY_MS] [--off]   a letter typed and let come back, six more typed, pictured before the
                                              host can have echoed them, and once settled (--off: without typing
                                              shown at once, to compare)
    try.py stall PROGRAM                      the link frozen with letters typed into it: what the pane and the
                                              status bar show meanwhile, and after
    try.py connect [DELAY_MS...] [--trace] [--bin=APP]
                                              how long a host's terminal takes to show, by the link's delay
                                              (--bin: another build of the app, to compare):
                                              from the app's first byte to the terminal's first screen, read
                                              from the link's own notes of what passed, and in round trips
                                              (nine once, two now); --trace: and those notes, in order
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
    try.py prompt [DELAY_MS] [--runs SECS] [--helper[=FILE]]
                                              the first letter typed at a shell's prompt soon after a command:
                                              a command is run (one that takes SECS, with --runs), and a letter
                                              typed half a second after its prompt can be back, well before
                                              the app could have asked the host itself; how much of the letter
                                              is still to come in a picture taken at once (0: it was drawn at
                                              once; a picture that took as long as the echo is taken again). --helper: Olo's helper behind the link (this tree's build,
                                              or FILE: an older one, to compare), which says the prompt unasked
    try.py secrets [DELAY_MS] [--helper[=FILE]]
                                              secrets the shell itself reads (`read -s -p`, `read -es -p`,
                                              `read -es`), typed soon after the Enter and after a pause: how
                                              much more ink there is with four letters typed than once the host
                                              has them (0: none was drawn)
    try.py bytes [DELAY_MS]                   what a far host costs on the wire, with SSH's own compression and
                                              without (OLO_REX_SSH_COMPRESSION: the app packs by itself only
                                              where a host's connection takes a while to open, and the rig's
                                              opens at once): the bytes each way over the far host's link
                                              (farhost.py's, which counts) for connecting with the helper put
                                              there, 132 letters typed at a shell, a flood (seq 20000) and 60
                                              letters typed into a program that redraws its box at each key
    try.py flood [LINES] [--release]          what the packing costs where the link is fast: seq LINES from a
                                              far host with no delay, without and with: the bytes, the time
                                              until the link is quiet, the app's own threads' instructions
    try.py behind RATE [LINES] [--bin=APP]    a flood on a narrow link, and when the pane is at the present again:
                                              the session's own program writes LINES lines (seq: over in a second
                                              on the host; 3,000,000 unless said) and is then a shell at its
                                              prompt; the far host is behind a link of RATE bytes a second, taken
                                              for far by the app. The time from the flood's beginning until the
                                              far link is quiet, the bytes that came, what the app said of
                                              catching up, and a picture. (Not typed: a key held a moment too
                                              long on a busy computer repeats.)
    try.py agent                              Passwords' SSH agent on a host: the far host is given the SSH keys
                                              (as in Preferences ▸ Rex), with a stand-in for Passwords that knows
                                              one made-up key; `ssh-add -L` there then lists it, the stand-in
                                              was asked for that host by Rex, and with the permission taken back
                                              nothing answers there

PROGRAM: shell, box, box-nosync, line (tui.py's stand-ins), or a command line."""
import contextlib, glob, os, shutil, signal, subprocess, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rig
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
elif what in ("prompt", "secrets"):
    # (--runs' number isn't the delay.)
    runs = float(args.pop(args.index("--runs") + 1)) if "--runs" in args else 0.0
    delay = int(next((a for a in args if a.isdigit()), "1000"))
    helper = next((a.partition("=")[2] or True for a in args if a.startswith("--helper")), False)
    def warmed(r):
        # (Letters echoed as typed: the link is seen to be slow, and the prompt to echo.)
        r.type("e"); time.sleep(3.2 * delay / 1000 + 0.6)
        r.type("cho hi", apart=0.05); time.sleep(4 * delay / 1000 + 0.6)
    if what == "prompt":
        # (The letter is typed after the helper's word can be there, one way and a moment after the prompt, and
        # long before the app's own question could be answered, a pause and a round trip more. A picture that
        # took as long as the letter's own echo says nothing, on a busy computer: such a turn is taken again.)
        told, turn = 0, 0
        while told < 3 and turn < 8:
            turn += 1
            with Rig("shell", delay=delay, helper=helper) as r:
                warmed(r)
                if runs:
                    r.type("\n", apart=0); time.sleep(8 * delay / 1000 + 2.0)
                    r.type(f"sleep {runs:g}", apart=0.05); time.sleep(4 * delay / 1000 + 0.6)
                # (No picture between the Enter and the letter: taking one takes as long as the wait this is about.)
                t0 = time.monotonic(); r.type("\n", apart=0)
                time.sleep(max(0, t0 + runs + (2 * delay + (600 if runs else 500)) / 1000 - time.monotonic()))
                t = time.monotonic(); r.type("x", apart=0)
                soon = r.inked(r.shot(f"prompt-{turn}-soon")); took = time.monotonic() - t
                time.sleep(4 * delay / 1000 + 0.8)
                settled = r.inked(r.shot(f"prompt-{turn}-settled"))
                if took * 1000 > 2 * delay - 300:
                    print(f"(a picture that took {took * 1000:.0f} ms, too near the round trip of {2 * delay} ms to tell: again)", flush=True)
                    continue
                told += 1
                print(f"{delay} ms each way, the letter {(t - t0) * 1000:.0f} ms after the Enter: {settled - soon} pixels of it still to come {took * 1000:.0f} ms after it was typed (a round trip is {2 * delay} ms)", flush=True)
        if told < 3:
            print(f"only {told} of 3 turns could be told: the computer is too busy for this at {delay} ms; give a longer DELAY_MS")
    else:
        for command in ("read -s -p pw x", "read -es -p pw x", "read -es x"):
            for wait, name in (((2 * delay + 250) / 1000, "soon"), (8 * delay / 1000 + 2.5, "after a pause")):
                with Rig("shell", delay=delay, helper=helper) as r:
                    warmed(r)
                    r.type("\n"); time.sleep(8 * delay / 1000 + 2.5)
                    r.type(command, apart=0.05); time.sleep(4 * delay / 1000 + 0.6)
                    r.type("\n", apart=0); time.sleep(wait)
                    r.type("q", apart=0); time.sleep(0.12); r.type("wer", apart=0.08)
                    typed = r.inked(r.shot("secret-typed"))
                    time.sleep(4 * delay / 1000 + 1.0)
                    print(f"`{command}`, typed {name}: {typed - r.inked(r.shot('secret-settled'))} pixels more with four letters typed than once the host has them", flush=True)
                    r.type("\n"); time.sleep(1.0)
elif what in ("bytes", "flood"):
    import shutil, subprocess
    def settled(r, quiet=1.2, within=120):
        """The far link's count once nothing has passed for `quiet` seconds, and when the last of it did."""
        last, since, end = r.far.bytes(), time.monotonic(), time.monotonic() + within
        while time.monotonic() < end:
            time.sleep(0.1); now = r.far.bytes()
            if now != last: last, since = now, time.monotonic()
            elif time.monotonic() - since >= quiet: break
        return last, since
    def more(a, b): return f"{b['up'] - a['up']:,} up, {b['down'] - a['down']:,} down"
    if what == "bytes":
        delay = int(next((a for a in args if a.isdigit()), "40"))
        for packed in ("no", "yes"):
            for program in ("shell", "box"):
                # (The helper is put there at each connection: the host's home has none to begin with.)
                shutil.rmtree(f"{D}/home/.local/share/olo/rex-link", ignore_errors=True)
                with Rig(program, delay=0, far=True, far_delay=delay, env=[f"OLO_REX_SSH_COMPRESSION={packed}"]) as r:
                    b0, _ = settled(r)
                    print(f"compression {packed}, {program}: connected, the helper put there: {b0['up']:,} up, {b0['down']:,} down", flush=True)
                    r.type(("the quick brown fox jumps over the lazy dog " * 3) if program == "shell" else "the quick brown fox jumps over the lazy dog and more words here"[:60], apart=0.03)
                    b1, _ = settled(r)
                    print(f"compression {packed}, {program}: {132 if program == 'shell' else 60} letters typed: {more(b0, b1)}", flush=True)
                    if program == "shell":
                        r.type("\b" * 140, apart=0.005); time.sleep(1.0); b1, _ = settled(r)
                        r.type("seq 20000\n", apart=0.02); b2, _ = settled(r)
                        print(f"compression {packed}, shell: seq 20000 (108,894 bytes of digits): {more(b1, b2)}", flush=True)
    else:
        lines = next((a for a in args if a.isdigit()), "2000000")
        release = "--release" in args
        if release: rig.ENV["TSESSION_PROFILE"] = "release"
        for turn in range(2):
            for packed in ("no", "yes"):
                with Rig("shell", delay=0, far=True, far_delay=0, release=release, env=[f"OLO_REX_SSH_COMPRESSION={packed}"]) as r:
                    b0, _ = settled(r, quiet=1.0)
                    p = subprocess.Popen(["perf", "stat", "--no-scale", "--per-thread", "-x,", "-e", "instructions:u", "-p", str(r.win["pid"]), "--timeout", "110000"], stderr=subprocess.PIPE, stdout=subprocess.DEVNULL, text=True)
                    time.sleep(0.5); t = time.monotonic(); r.type(f"seq {lines}\n", apart=0.02)
                    b1, at = settled(r, quiet=1.0); p.send_signal(signal.SIGINT); err = p.communicate()[1]
                    own = sum(int(f[1]) for f in (l.split(",") for l in err.splitlines()) if len(f) > 3 and f[1].isdigit() and not f[0].startswith("llvmpipe"))
                    print(f"compression {packed}: seq {lines}: {b1['down'] - b0['down']:,} bytes down, the last of them {at - t:.1f} s after the command; the app's own threads {own / 1e6:,.0f} million instructions", flush=True)
elif what == "agents":
    # Keep It Going and Auto Approve in a pane, for each of the three coding agents at once (`agents-one AGENT`,
    # each in a rig of its own: at_once.py).
    sys.exit(subprocess.run(["/usr/bin/python3", f"{HERE}/at_once.py", os.path.abspath(__file__), "claude", "codex", "agy", "--", "agents-one"]).returncode)
elif what in ("claude", "codex", "agy") and args[:1] == ["agents-one"]:
    # One coding agent's stand-in (tools/dev/stand-ins: it works, asks leave to run a command with "1. Yes" chosen,
    # waits at its own prompt, works again and waits again, writing what it is answered and told), as at_once.py runs
    # it (`try.py AGENT agents-one`). Keep It Going and Auto Approve turned on by the app's commands, as an agent
    # would ask: Auto Approve then asks the person, who presses Turn On. The question is answered with Return once
    # its time has come (shortened: OLO_REX_AUTO_APPROVE_SECS), the countdown showing meanwhile; the agent at its
    # prompt, quiet long enough (OLO_REX_KEEP_GOING_SECS), is told to keep going; and not again within the hour.
    agent = what
    ok = True
    def check(what, passed, shown):
        global ok
        ok = ok and bool(passed); print(("ok   " if passed else "FAIL ") + what + ":", repr(shown)[:220], flush=True)
    said = "Please keep going until everything is complete and verified."
    record = f"{D}/agent-{agent}.txt"
    with contextlib.suppress(FileNotFoundError): os.remove(record)
    read = lambda: open(record).read() if os.path.exists(record) else ""
    # (Under a copy of bash named for the agent: Rex's server names a script's program by its interpreter,
    # "bash", where an agent runs under its own name.)
    named = f"{D}/agents/{agent}"
    os.makedirs(os.path.dirname(named), exist_ok=True)
    shutil.copy("/usr/bin/bash", named)
    with Rig(f"exec {named} {R3}/tools/dev/stand-ins/{agent} {record}", delay=0, env=["OLO_REX_AUTO_APPROVE_SECS=4", "OLO_REX_KEEP_GOING_SECS=3"]) as r:
        time.sleep(1.0)
        r.invoke("agent.keep_going_toggle")
        r.invoke("agent.auto_approve_toggle")
        # (Asked for through the commands socket: the person is asked, and says Turn On.)
        button = r.wait(lambda: next((n for n in r.nodes() if n.get("role") == "Button" and n.get("name") == "Turn On"), None), 10)
        alert = next((w for w in r.windows() if w.get("title") == "Rex" and w.get("id") != r.win.get("id")), None)
        check(f"{agent}: Auto Approve asked of the person first", button and alert, {"alert": bool(alert), "button": bool(button)})
        if button and alert:
            c = alert["content"]
            r.ts("click", r.name, str(int(c["x"] + button["x"] + button["width"] / 2)), str(int(c["y"] + button["y"] + button["height"] / 2)))
        counting = r.wait(lambda: next((n for n in r.nodes() if n.get("role") == "Status" and "Yes in" in str(n.get("name") or "")), None), 12)
        check(f"{agent}: the question counts down", counting, (counting or {}).get("name"))
        answered = r.wait(lambda: "answer:[]" in read(), 15)
        check(f"{agent}: answered Yes (Return) once its time came", answered, read())
        told = r.wait(lambda: f"said:[{said}]" in read(), 20)
        check(f"{agent}: told to keep going at its prompt, once quiet", told, read())
        time.sleep(9)
        check(f"{agent}: not told again within the hour", told and "said again" not in read(), read())
    if not ok:
        sys.exit(1)
elif what == "agent":
    import getpass
    with Rig("shell", delay=0, far=True) as r:
        asked = r.keychain()
        sock = f"{D}/home/.local/state/olo-rex/agent.sock"
        listed = lambda: r.far.ran(f"SSH_AUTH_SOCK={sock} ssh-add -L 2>&1").strip()
        print("before the host is given the keys, ssh-add there says:", repr(listed()[:80]))
        r.grant("far", ssh_keys=True)
        got = r.wait(lambda: (lambda out: out if "olo-test-key" in out else None)(listed()), 20, every=0.5)
        print("given them, ssh-add there lists:", repr((got or listed())[:120]))
        said = open(asked).read().replace(getpass.getuser(), "USER") if os.path.exists(asked) else ""
        print("the stand-in for Passwords was asked:", said.strip().splitlines()[:3])
        r.grant("far", ssh_keys=False)
        gone = r.wait(lambda: (lambda out: out if "olo-test-key" not in out else None)(listed()), 20, every=0.5)
        print("taken back, ssh-add there says:", repr((gone or listed())[:80]))
        print("PASS" if got and gone is not None and "for far at" in said and "asked by olo-rex" in said else "FAIL")
elif what == "bridge":
    # The Olo desktop through Rex, from a program on the far host: `rex do olo.screen …` there, answered by the
    # app here as the host's permissions allow, with what the command wrote; a picture fetched; a long command
    # waited for. (A stand-in for olo-screen: the rig starts no real Agent Screen.) And the Host window's Bridge
    # line following the permissions while connected.
    import base64, shlex
    fake = f"{D}/bridge-bin"; os.makedirs(fake, exist_ok=True)
    picture = bytes((i * 7) % 251 for i in range(700_000))
    open(f"{fake}/look.png", "wb").write(picture)
    open(f"{fake}/olo-screen", "w").write(f"#!/bin/sh\ncase \"$1\" in\n  look) printf '{{\"picture\":\"{fake}/look.png\"}}\\n' ;;\n  slow) sleep 12; echo slow done ;;\n  *) echo \"screen: $*\" ;;\nesac\n")
    os.chmod(f"{fake}/olo-screen", 0o755)
    # (And for `op`: one that says what it was given, reads the files its flags name and writes its -o.)
    open(f"{fake}/olo-op", "w").write(f"""#!/bin/sh
case "$1" in
  whoami) echo '{{}}'; exit 0 ;;
  item) if [ "$2" = get ]; then printf '{{"value":"{fake}/look.png"}}\\n'; exit 0; fi ;;
esac
echo "op: $*"
for a in "$@"; do
  case "$a" in
    --template=*|--i=*|--in-file=*|--env-file=*) echo "read: $(cat "${{a#*=}}")" ;;
    --o=*|--out-file=*) printf 'written by op' > "${{a#*=}}" ;;
  esac
done
[ -p /dev/stdin ] && echo "stdin: $(cat)"
exit 0
""")
    os.chmod(f"{fake}/olo-op", 0o755)
    with Rig("shell", delay=0, far=True, env=[f"OLO_REX_BRIDGE_BIN={fake}"], bin=next((a.partition("=")[2] for a in args if a.startswith("--bin=")), None)) as r:
        # (The rig's far host is this computer, and its Rex server the rig's one: the app is connected to it
        # twice, as this computer and as "far". A program there names the client it means, as it would where
        # two Olo desktops show one host; the far one is the one that refuses what "far" isn't granted.)
        rex = f"rex -S unix://{D}/rex/rex.sock --autostart=false"
        first = lambda out: json.JSONDecoder().raw_decode(out.lstrip())[0]   # (ssh's own words come after)
        clients = [c["client_id"] for c in first(r.far.ran(f"{rex} client ls --json 2>/dev/null")).get("clients", []) if c.get("info", {}).get("kind") == "app"]
        client = [None]
        def do(action, a=None, pairs=""):
            out = r.far.ran(f"{rex} -C {client[0]} do {action} --json {pairs} " + (f"--args {shlex.quote(json.dumps(a))}" if a is not None else "") + " 2>/dev/null")
            try: return first(out)
            except ValueError: return {"said": out.strip()[:200]}
        def bridge():
            said = [str(n.get("name") or n.get("value") or "") for n in r.nodes("Host") if n.get("role") in ("Text", "Label", "StaticText", "Heading")]
            said = [w for w in said if w]
            if "Bridge" not in said: return None
            rest = said[said.index("Bridge") + 1:]
            return " · ".join(rest[:next((i for i, w in enumerate(rest) if w in ("Skill for agents", "Typing helper")), len(rest))])[:60]
        ok = True
        def check(what, passed, shown):
            global ok
            ok = ok and bool(passed); print(("ok   " if passed else "FAIL ") + what + ":", repr(shown)[:230], flush=True)
        r.invoke("host.manage")
        line = r.wait(lambda: bridge(), 20)
        check("the Host window, nothing granted", line and line.startswith("Off") and "nothing granted" in line, line)
        for c in clients:
            client[0] = c
            if do("olo.screen", {"args": ["tree", "agent"]}).get("ok") is False: break
        refused = do("olo.screen", {"args": ["tree", "agent"]})
        check("not granted, refused by name", refused.get("ok") is False and "Agent Screens" in str(refused.get("stderr")), refused)
        r.grant("far", agent_screens=True)
        line = r.wait(lambda: (lambda b: b if b and b.startswith("On") else None)(bridge()), 10)
        check("the Host window, once granted (no new connection)", line, line or bridge())
        tree = r.wait(lambda: (lambda t: t if t.get("ok") else None)(do("olo.screen", {"args": ["tree", "agent", "two words", "it's \"quoted\" $HOME"]})), 10)
        check("a command's answer, its arguments as given", tree and tree.get("stdout") == "screen: tree agent two words it's \"quoted\" $HOME\n", tree)
        pairs = do("olo.screen", None, pairs="""'args=["tree","agent"]'""")
        check("the same with KEY=VALUE", pairs.get("stdout") == "screen: tree agent\n", pairs)
        look = do("olo.screen", {"args": ["look", "agent"]})
        files = look.get("files") or [{}]
        got, offset, pieces = b"", 0, 0
        while files[0].get("id") and pieces < 9:
            piece = do("olo.file", None, pairs=f"id={files[0]['id']} offset={offset}")
            got += base64.b64decode(piece.get("base64") or ""); offset += piece.get("count") or 0; pieces += 1
            if piece.get("done") or not piece.get("count"): break
        check("a picture named and fetched whole", got == picture, {"files": files, "pieces": pieces, "bytes": len(got)})
        t0 = time.monotonic(); slow = do("olo.screen", {"args": ["slow"]}); took = time.monotonic() - t0
        waited = slow
        while waited.get("pending"): waited = do("olo.wait", None, pairs=f"id={waited['pending']}")
        check("a command of 12 s: pending, then its answer", slow.get("pending") and waited.get("stdout") == "slow done\n", {"first answer after": round(took, 1), "then": waited})
        listed = r.far.ran(f"{rex} -C {client[0]} actions --json 2>&1 | grep -o '\"olo\\.[a-z_.]*\"' | sort -u | tr '\\n' ' '")
        check("what `rex actions` lists of Olo's", all(w in listed for w in ("olo.open", "olo.op", "olo.screen", "olo.file", "olo.wait")), listed)
        # The skill's one command on the host (put there as the Host window puts it): the same, in one step
        # each: which client, an answer waited for, a picture saved there.
        ran = lambda c: "\n".join(l for l in r.far.ran(c).splitlines() if not l.startswith("Warning: Permanently"))
        r.invoke("host.put:far:skill")
        r.wait(lambda: "olo.lua" in ran("ls $HOME/.claude/skills/olo-rex-bridge 2>/dev/null"), 30, every=0.5)
        olo = f'OLO_BRIDGE_REX="{rex}" $HOME/.claude/skills/olo-rex-bridge/olo'
        said = ran(f"{olo} screen tree agent 'two words'")
        check("the skill's command: a command's answer", said == "screen: tree agent two words", said)
        looked = ran(f"{olo} screen look agent")
        try: here = json.loads(looked).get("picture", "")
        except ValueError: here = ""
        saved = open(here, "rb").read() if here.startswith(f"{D}/tmp/olo-rex-bridge/") and os.path.exists(here) else b""
        check("the skill's command: a picture saved on the host in one step", saved == picture, {"said": looked[:120], "bytes": len(saved)})
        t0 = time.monotonic(); slow = ran(f"{olo} screen slow")
        check("the skill's command: a command of 12 s waited for", slow == "slow done", {"said": slow[:80], "took": round(time.monotonic() - t0, 1)})
        # `op` through the bridge (the stand-in above, saying what it was given): nothing of this computer's
        # is the host's to name. `op run` refused; the host's template and standard input sent; a file it
        # writes written on the host, its owner's alone, and not over one with something in it.
        r.grant("far", passwords=True)
        r.wait(lambda: do("olo.op", {"args": ["whoami"]}).get("ok"), 10)
        refused = ran(f"{olo} op run -- id; echo status=$?")
        check("op run, refused", "isn't carried" in refused and "ran a command" not in refused, refused)
        open(f"{D}/far-template.json", "w").write('{"title":"From the host"}')
        sent = ran(f"cd {D} && {olo} op item create --template far-template.json --vault V username=x")
        given = next((l for l in sent.splitlines() if l.startswith("op: ")), "")
        check("a template: the host's file, sent", 'read: {"title":"From the host"}' in sent and "far-template.json" not in given and "--vault V username=x" in given, sent)
        piped = ran(f"printf 'from the pipe' | {olo} op item create -")
        check("standard input: the host's", "stdin: from the pipe" in piped, piped)
        out = f"{D}/far-out.txt"
        if os.path.exists(out): os.remove(out)
        ran(f"{olo} op read op://v/i/f -o {out}")
        mode = oct(os.stat(out).st_mode & 0o777) if os.path.exists(out) else None
        check("a file it writes: on the host, its owner's alone", os.path.exists(out) and open(out).read() == "written by op" and mode == "0o600", {"mode": mode})
        again = ran(f"{olo} op read op://v/i/f -o {out} 2>&1")
        check("not over a file with something in it", "already exists" in again, again)
        named = do("olo.op", {"args": ["item", "get", "pic"]})
        check("a picture an answer names: not carried", named.get("ok") and not named.get("files"), named)
        r.grant("far", passwords=False)
        r.grant("far", agent_screens=False)
        line = r.wait(lambda: (lambda b: b if b and b.startswith("Off") else None)(bridge()), 10)
        check("taken back", line and do("olo.screen", {"args": ["tree", "agent"]}).get("ok") is False, line)
        print("PASS" if ok else "FAIL")
elif what == "pool":
    # The Token Pool put on the far host over SSH by the desktop pool's own code, kept in step, borrowed from and
    # taken away (crates/pool/tests/hosts_live.rs: a made-up account, a provider of the test's own). The far
    # host's pool listens on ports of the rig's (farhost.py), the person's own pool being on this computer too.
    # Needs: cargo build -p olo-pool (the host's program) and the test built (done here).
    import getpass, shutil, subprocess
    os.environ["FAR_POOL_UPSTREAM"] = "http://127.0.0.1:27990"
    from farhost import FarHost
    built = subprocess.run(["cargo", "test", "-p", "olo-token-pool", "--test", "hosts_live", "--no-run", "--message-format=json"], capture_output=True, text=True, cwd=R3, env=ENV).stdout
    binary = next(json.loads(l)["executable"] for l in built.splitlines() if l.startswith("{") and json.loads(l).get("executable") and "hosts_live" in l)
    program = f"{R3}/target/pool-host/x86_64-unknown-linux-musl/release/olo-pool-host"
    assert os.path.exists(program), "cargo build -p olo-pool"
    r = Rig("shell")
    try:
        r.host()
        with FarHost() as far:
            home = f"{D}/far/poolhome"; shutil.rmtree(home, ignore_errors=True); os.makedirs(f"{home}/.ssh")
            key = open(f"{D}/far/host_key.pub").read().split()
            open(f"{home}/.ssh/known_hosts", "w").write(f"[127.0.0.1]:{far.port} {key[0]} {key[1]}\n")
            env = dict(ENV, HOME=home, XDG_CONFIG_HOME=f"{home}/.config", XDG_STATE_HOME=f"{home}/state", SSH_AUTH_SOCK=far.agent, OLO_REX_TEST_HOST=f"ssh://{getpass.getuser()}@127.0.0.1:{far.port}",
                       OLO_POOL_HOST_BIN=program, OLO_POOL_TEST_UPSTREAM_PORT="27990", OLO_REX_SSH_COMPRESSION="no")
            try:
                out = subprocess.run([binary, "--ignored", "--nocapture", "a_pool_is_put_on_a_host"], capture_output=True, text=True, timeout=170, env=env)
                print("\n".join(l for l in (out.stdout + out.stderr).splitlines() if any(w in l for w in ("the pool", "test result", "panicked", "left:", "right:", "said"))).replace(getpass.getuser(), "USER")[:3000])
                # (And from a home that has never met the far host: said plainly, nothing taken on trust.)
                home2 = f"{D}/far/poolhome-new"; shutil.rmtree(home2, ignore_errors=True); os.makedirs(home2)
                env2 = dict(env, HOME=home2, XDG_CONFIG_HOME=f"{home2}/.config", XDG_STATE_HOME=f"{home2}/state")
                out2 = subprocess.run([binary, "--ignored", "--nocapture", "a_host_never_met"], capture_output=True, text=True, timeout=60, env=env2)
                print("\n".join(l for l in (out2.stdout + out2.stderr).splitlines() if any(w in l for w in ("the pool", "test result", "panicked", "left:", "right:"))).replace(getpass.getuser(), "USER")[:1500])
                assert not os.path.exists(f"{home2}/.ssh/known_hosts"), "a key was learnt with nobody asked"
                print("PASS" if "test result: ok. 1 passed" in out.stdout and "test result: ok. 1 passed" in out2.stdout else "FAIL")
            finally:
                # (Whatever became of the test: no pool of its is left running on the far host, nor anything of it there.)
                far.ran('d="$HOME/.local/share/olo/pool"; [ -x "$d/olo-pool-host" ] && { echo \'{"do":"stop"}\' | "$d/olo-pool-host" ctl; "$d/olo-pool-host" put-back; "$d/olo-pool-host" forget; }; rm -rf "$d"')
    finally:
        r.__exit__()
elif what == "poolrex":
    # The Token Pool on the far host from Rex's Host window, through both apps: Rex asks this session's own Token
    # Pool (a test one, on ports and a key file of its own, no accounts), which puts the pool there over SSH; the
    # window's Token Pool line follows; and taken away again. Needs: cargo build -p olo-pool -p olo-rex.
    with Rig("shell", delay=0, far=True, env=["OLO_POOL_PORT=27991"]) as r:
        pool_env = ["OLO_POOL_PORT=27991", "OLO_POOL_PROXY_PORT=27992", f"OLO_POOL_KEYFILE={D}/pool-test.key", "OLO_POOL_UPSTREAM=http://127.0.0.1:9", f"SSH_AUTH_SOCK={r.far.agent}", "OLO_REX_SSH_COMPRESSION=no"]
        r.ts("run", r.name, *pool_env, f"{R3}/target/debug/olo-pool", "--service")
        def line():
            said = [str(n.get("name") or n.get("value") or "") for n in r.nodes("Host") if n.get("role") in ("Text", "Label", "StaticText", "Heading")]
            said = [w for w in said if w]
            return " · ".join(said[said.index("Token Pool") + 1:][:4])[:90] if "Token Pool" in said else None
        there = lambda: r.far.ran('echo \'{"do":"status"}\' | "$HOME/.local/share/olo/pool/olo-pool-host" ctl 2>&1 | head -c 300').strip()
        ok = True
        def check(what, passed, shown):
            global ok
            ok = ok and bool(passed); print(("ok   " if passed else "FAIL ") + what + ":", repr(shown)[:260], flush=True)
        try:
            # (The test's pool answers on its port before the window is opened.)
            import urllib.request
            end = time.monotonic() + 20
            while time.monotonic() < end:
                try:
                    if urllib.request.urlopen("http://127.0.0.1:27991/pool/hosts", timeout=1).status == 200: break
                except OSError: time.sleep(0.3)
            r.invoke("host.manage")
            first = r.wait(lambda: (lambda l: l if l and "Not installed" in l else None)(line()), 25)
            check("the Host window, no pool there", first, first or line())
            # From the Token Pool itself, as its own Hosts window does it: Rex's window, open and asked nothing,
            # follows (it holds one question open with the pool, answered when a host's standing changes).
            def pool_does(what):
                req = urllib.request.Request(f"http://127.0.0.1:27991/pool/hosts/far/{what}", data=b"", method="POST")
                return urllib.request.urlopen(req, timeout=5).status
            pool_does("install")
            got = r.wait(lambda: (lambda l: l if l and l.startswith("Installed") and "up to date" in l else None)(line()), 60, every=0.5)
            check("installed from the Token Pool, and Rex's window followed by itself", got, got or line())
            pool_does("remove")
            gone = r.wait(lambda: (lambda l: l if l and "Not installed" in l else None)(line()), 60, every=0.5)
            check("removed from the Token Pool, and Rex's window followed", gone, gone or line())
            r.invoke("host.put:far:pool")
            got = r.wait(lambda: (lambda l: l if l and l.startswith("Installed") else None)(line()), 60, every=0.5)
            check("installed from Rex's window", got and "up to date" in got, got or line())
            check("the pool on the far host says", '"hosted":true' in there(), there()[-120:])
            r.invoke("host.take:far:pool")
            gone = r.wait(lambda: (lambda l: l if l and "Not installed" in l else None)(line()), 60, every=0.5)
            check("removed from Rex's window", gone, gone or line())
            check("nothing of it on the far host", "No such file" in there() or "not found" in there(), there()[-120:])
        finally:
            r.far.ran('d="$HOME/.local/share/olo/pool"; [ -x "$d/olo-pool-host" ] && { echo \'{"do":"stop"}\' | "$d/olo-pool-host" ctl; "$d/olo-pool-host" put-back; "$d/olo-pool-host" forget; }; rm -rf "$d"')
        print("PASS" if ok else "FAIL")
elif what == "account":
    # A far host's account made ready from the Account window, by its commands: asked how it stands, an agent
    # installed with its installer's lines shown as it goes, one stopped half way (its script on the host ends,
    # and nothing of it comes later), then all that is left at once: the path, the three agents, git's identity.
    # The installers are stand-ins served from this computer (OLO_REX_READY_FROM), each a script that puts a
    # program of the agent's name in ~/.local/bin; Codex's takes six seconds and says where it is each second.
    import subprocess, shutil
    served, home, app_home = f"{D}/installers", f"{D}/home", f"{D}/home-app"
    os.makedirs(served, exist_ok=True); os.makedirs(app_home, exist_ok=True)
    stand_in = lambda name, version, slow: f"""#!/bin/sh
set -eu
mkdir -p "$HOME/.local/bin"
{f'for i in 1 2 3 4 5 6; do echo "Downloading {name}: part $i of 6"; sleep 1; done' if slow else f'echo "Downloading {name}"'}
printf '#!/bin/sh\\necho "{name} {version}"\\n' > "$HOME/.local/bin/{name}"
chmod +x "$HOME/.local/bin/{name}"
echo "Installed {name} {version}"
"""
    for file, name, version, slow in (("claude.sh", "claude", "9.9.1", False), ("codex.sh", "codex", "8.8.2", True), ("agy.sh", "agy", "7.7.3", False)):
        open(f"{served}/{file}", "w").write(stand_in(name, version, slow))
    def clean():
        for f in (".local/bin/claude", ".local/bin/codex", ".local/bin/agy", ".gitconfig"):
            with contextlib.suppress(FileNotFoundError): os.remove(f"{home}/{f}")
        shutil.rmtree(f"{home}/.claude/skills/olo-rex-bridge", ignore_errors=True)
        for f in ("CLAUDE.md", "CLAUDE.md.before-olo", "settings.json", "settings.json.before-olo", "commands/ship.md"):
            with contextlib.suppress(FileNotFoundError): os.remove(f"{home}/.claude/{f}")
        for f in (".profile", ".bash_profile", ".bashrc", ".zshrc", ".zprofile"):
            try: text = open(f"{home}/{f}").read()
            except OSError: continue
            open(f"{home}/{f}", "w").write("\n".join(l for l in text.split("\n") if "Olo: programs in ~/.local/bin" not in l))
    clean()
    open(f"{app_home}/.gitconfig", "w").write("[user]\n\tname = Rig Person\n\temail = rig@example.org\n")
    # (The app's home has a little of an agent's settings, and what is never carried; the host has instructions of its own.)
    os.makedirs(f"{app_home}/.claude/commands", exist_ok=True); os.makedirs(f"{home}/.claude", exist_ok=True)
    open(f"{app_home}/.claude/CLAUDE.md", "w").write("Be brief.\n"); open(f"{app_home}/.claude/commands/ship.md", "w").write("Ship it.\n")
    open(f"{app_home}/.claude/settings.json", "w").write('{"model":"opus","env":{"HTTPS_PROXY":"http://127.0.0.1:17892","NO_PROXY":"localhost","EDITOR":"vi","GITHUB_TOKEN":"made-up"}}')
    open(f"{app_home}/.claude/.credentials.json", "w").write('{"made":"up"}')
    open(f"{home}/.claude/CLAUDE.md", "w").write("The host's own.\n")
    # (And Claude Code's sessions, the app's: two used this week, one with its folder and one of 30 MB, and one used
    # forty days ago; the far host's home is another folder, so each is said as the host's there.)
    name = lambda path: "".join(c if c.isalnum() else "-" for c in path)
    project_here, project_there = f"{app_home}/.claude/projects/{name(app_home)}-src-app", f"{home}/.claude/projects/{name(home)}-src-app"
    shutil.rmtree(f"{app_home}/.claude/projects", ignore_errors=True); shutil.rmtree(f"{home}/.claude/projects", ignore_errors=True)
    os.makedirs(f"{project_here}/s1/tool-results")
    open(f"{project_here}/s1.jsonl", "w").write(json.dumps({"cwd": f"{app_home}/src/app", "sessionId": "s1"}) + "\n")
    open(f"{project_here}/s1/tool-results/t.txt", "w").write("out")
    open(f"{project_here}/s2.jsonl", "w").write("".join(json.dumps({"n": i, "text": "x" * 200}) + "\n" for i in range(140_000)))
    open(f"{project_here}/s3.jsonl", "w").write("{}\n")
    os.utime(f"{project_here}/s3.jsonl", (time.time() - 40 * 86400, time.time() - 40 * 86400))
    # (And a repository made here, for an address: one file in it.)
    origin = f"{D}/origin"; shutil.rmtree(origin, ignore_errors=True); shutil.rmtree(f"{home}/src/origin", ignore_errors=True); os.makedirs(origin)
    subprocess.run("git init -q . && echo hello > README && git add README && git -c user.name=T -c user.email=t@example.org commit -q -m first", shell=True, cwd=origin, check=True, env={"PATH": os.environ["PATH"], "HOME": D, "GIT_CONFIG_NOSYSTEM": "1"})
    server = subprocess.Popen(["/usr/bin/python3", "-m", "http.server", "27881", "--bind", "127.0.0.1", "--directory", served], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    ok = True
    def check(what, passed, shown):
        global ok
        ok = ok and bool(passed); print(("ok   " if passed else "FAIL ") + what + ":", repr(shown)[:260], flush=True)
    try:
        with Rig("shell", delay=0, far=True, env=["OLO_REX_READY_FROM=http://127.0.0.1:27881"]) as r:
            TITLES = ("Program folder", "Claude Code", "Codex", "Antigravity", "Git identity", "Agent settings", "Agent sessions", "Skill for agents", "Your repositories")
            def rows(under=False):
                """Each item's row as the window says it: {title: what is said after it}; `under`: the line under it."""
                said = [str(n.get("name") or "") for n in r.nodes("Account") if n.get("role") in ("Label", "Text", "StaticText")]
                out = {}
                for title in TITLES:
                    if title in said:
                        at = said.index(title)
                        if at + 2 < len(said) and said[at + 1] == "·": out[title] = said[at + 3] if under and at + 3 < len(said) else said[at + 2]
                return out
            there = lambda name: os.path.exists(f"{home}/.local/bin/{name}")
            skill = lambda: os.path.exists(f"{home}/.claude/skills/olo-rex-bridge/SKILL.md")
            r.invoke("host.account:far")
            first = r.wait(lambda: (lambda w: w if w.get("Claude Code") == "Not installed" else None)(rows()), 25)
            check("the window, asked of a new account", first and first.get("Codex") == "Not installed" and first.get("Git identity") == "Not set" and first.get("Program folder") == "Not on the path" and first.get("Agent settings") == "3 files to copy", first or rows())
            r.invoke("host.ready:far:claude")
            got = r.wait(lambda: (lambda w: w if w.get("Claude Code") == "9.9.1" else None)(rows()), 30)
            check("Claude Code installed by its installer, and its version said", got and there("claude"), (got or rows()).get("Claude Code"))
            r.invoke("host.ready:far:codex")
            line = r.wait(lambda: (lambda w: w.get("Codex") if str(w.get("Codex")).startswith("Downloading codex: part") else None)(rows(under=True)), 20)
            check("an installer's line shown under its row as it goes", line and rows().get("Codex") == "Installing…", {"row": rows().get("Codex"), "under": line})
            r.invoke("host.ready.stop")
            back = r.wait(lambda: (lambda w: w if w.get("Codex") == "Not installed" else None)(rows()), 15)
            time.sleep(7)
            check("stopped half way: nothing of it there, then or later", back and not there("codex"), {"row": rows().get("Codex"), "there": there("codex")})
            r.invoke("host.ready:far:all")
            want = {"Program folder": "On the path", "Claude Code": "9.9.1", "Codex": "8.8.2", "Antigravity": "7.7.3", "Git identity": "Rig Person <rig@example.org>", "Agent settings": "As on this computer", "Skill for agents": "Installed", "Your repositories": "None chosen"}
            # (The sessions are copied by their own Copy alone: not a part of all that is left.)
            done = r.wait(lambda: (lambda w: w if {k: v for k, v in w.items() if k != "Agent sessions"} == want else None)(rows()), 60, every=0.5)
            check("all that was left, one after another", done, done or rows())
            git = subprocess.run(["git", "config", "--file", f"{home}/.gitconfig", "user.email"], capture_output=True, text=True).stdout.strip()
            check("on the host: the three programs, git's identity, the skill", there("claude") and there("codex") and there("agy") and git == "rig@example.org" and skill(), {"git": git, "skill": skill(), "bin": sorted(os.listdir(f"{home}/.local/bin")) if os.path.isdir(f"{home}/.local/bin") else None})
            # The person's settings (a made-up home's): there as here, less what is never carried; what the host had kept beside.
            read = lambda f: open(f"{home}/{f}").read() if os.path.exists(f"{home}/{f}") else None
            settings = json.loads(read(".claude/settings.json") or "{}")
            check("the settings carried: instructions, a command, the settings without the key or this computer's pool", read(".claude/CLAUDE.md") == "Be brief.\n" and read(".claude/commands/ship.md") == "Ship it.\n" and settings.get("model") == "opus" and settings.get("env") == {"EDITOR": "vi"} and not os.path.exists(f"{home}/.claude/.credentials.json"),
                  {"CLAUDE.md": read(".claude/CLAUDE.md"), "settings": settings, "credentials there": os.path.exists(f"{home}/.claude/.credentials.json")})
            check("what the host had is kept beside ours", read(".claude/CLAUDE.md.before-olo") == "The host's own.\n", read(".claude/CLAUDE.md.before-olo"))
            # The agents' sessions: two used this week to copy, said by number and size, and left alone by all that
            # was left; copied by their own command (the archive made as it is sent); each where Claude Code looks for
            # the project's there, the paths in it said as the host's, its time with it; the one used forty days ago
            # not, until the window is all of them; one continued there is left as it is.
            row = rows().get("Agent sessions") or ""
            check("the sessions: two of this week to copy, by number and size, not copied by all that was left", row.startswith("2 sessions") and row.endswith("MB to copy") and not os.path.exists(project_there), row)
            r.invoke("host.ready:far:sessions")
            copied = r.wait(lambda: (lambda w: w if w.get("Agent sessions") == "As on this computer" else None)(rows()), 60, every=0.5)
            s1 = read(f".claude/projects/{name(home)}-src-app/s1.jsonl")
            same_time = os.path.exists(f"{project_there}/s2.jsonl") and int(os.stat(f"{project_there}/s2.jsonl").st_mtime) == int(os.stat(f"{project_here}/s2.jsonl").st_mtime)
            check("copied: in the host's project folder, said as the host's, each with its time", copied and s1 and json.loads(s1)["cwd"] == f"{home}/src/app" and read(f".claude/projects/{name(home)}-src-app/s1/tool-results/t.txt") == "out" and same_time and not os.path.exists(f"{project_there}/s3.jsonl"),
                  {"row": rows().get("Agent sessions"), "s1": (s1 or "")[:120], "same time": same_time, "old one there": os.path.exists(f"{project_there}/s3.jsonl")})
            r.invoke("host.sessions.days:0")
            older = r.wait(lambda: (lambda w: w if str(w.get("Agent sessions")).startswith("1 session") else None)(rows()), 30, every=0.5)
            check("all of them: the one used forty days ago to copy", older, rows().get("Agent sessions"))
            os.utime(f"{project_there}/s1.jsonl", (time.time() + 600, time.time() + 600))
            r.invoke("host.sessions.days:7")
            newer = r.wait(lambda: (lambda w: w if w.get("Agent sessions") == "As on this computer  ·  1 continued there" else None)(rows()), 30, every=0.5)
            check("one continued there: said, and left as it is", newer, rows().get("Agent sessions"))
            # A repository of the person's: typed into the form (its address; Return), then cloned on the host and set up.
            r.invoke("host.repos.add")
            form = r.wait(lambda: next((w for w in r.windows() if w.get("title") == "Add Repository"), None), 10)
            time.sleep(0.6)
            # (Its address, then past the folder to its set-up command, then Return: Tab is key 15, Return 28.)
            r.ts("type", r.name, f"file://{origin}"); r.ts("key", r.name, "15"); r.ts("key", r.name, "15")
            r.ts("type", r.name, "echo ran > set-up.txt"); r.ts("key", r.name, "28")
            listed = r.wait(lambda: (lambda w: w if w.get("Your repositories") == "0 of 1 cloned" else None)(rows()), 20)
            check("a repository added through its form, and the host asked of it", form and listed, rows().get("Your repositories"))
            r.invoke("host.ready:far:repos")
            cloned = r.wait(lambda: (lambda w: w if w.get("Your repositories") == "1 cloned" else None)(rows()), 40, every=0.5)
            check("cloned on the host and set up", cloned and read("src/origin/README") == "hello\n" and read("src/origin/set-up.txt") == "ran\n", {"row": rows().get("Your repositories"), "README": read("src/origin/README"), "set up": read("src/origin/set-up.txt")})
    finally:
        server.terminate(); clean()
        shutil.rmtree(served, ignore_errors=True)
        with contextlib.suppress(FileNotFoundError): os.remove(f"{app_home}/.gitconfig")
        shutil.rmtree(f"{app_home}/.claude", ignore_errors=True); shutil.rmtree(f"{home}/.claude/projects", ignore_errors=True)
        shutil.rmtree(origin, ignore_errors=True); shutil.rmtree(f"{home}/src/origin", ignore_errors=True)
    print("PASS" if ok else "FAIL")
elif what == "disconnect":
    # A host let go of and connected again (Hosts and Ports' Disconnect, by its command): its SSH connection is
    # closed and Rex is no client of its server any more, the host stays in the list, saying so, with Connect
    # in the button's place; Connect brings its session back, typed into as before.
    import subprocess
    with Rig("shell", delay=0, far=True) as r:
        ok = True
        def check(what, passed, shown):
            global ok
            ok = ok and bool(passed); print(("ok   " if passed else "FAIL ") + what + ":", repr(shown)[:240], flush=True)
        # (Connections to the far host's SSH server, which is the rig's own port: counted by that port.)
        ssh = lambda: len(subprocess.run(["ss", "-tnH", "state", "established", f"sport = :{r.far.port}"], capture_output=True, text=True).stdout.split("\n")) - 1
        def window():
            nodes = r.nodes("Hosts and Ports")
            row = next((n for n in nodes if n.get("role") == "Row" and str(n.get("name") or "").startswith("far ")), None)
            button = next((n for n in nodes if n.get("role") == "Button" and n.get("name") in ("Connect", "Disconnect")), None)
            return {"row": row, "says": row and str(row["name"]).split()[-1], "button": button and (button["name"], button.get("enabled"))}
        def choose():
            # (The far host's row chosen, as a person would: the button under the list is for the one chosen.)
            w = next((x for x in r.windows() if x.get("title", "").startswith("Hosts and Ports")), None); row = window()["row"]
            if w and row:
                c = w["content"]; r.ts("click", r.name, str(int(c["x"] + row["x"] + row["width"] / 2)), str(int(c["y"] + row["y"] + row["height"] / 2))); time.sleep(0.8)
        def typed(mark):
            r.type(f"touch {mark}\n")
            return r.wait(lambda: mark in r.far.ran(f"ls $HOME {D} {D}/home 2>/dev/null"), 15)
        mark = f"disc{int(time.time()) % 100000}"
        check("connected: typed into its shell", typed(mark + "a"), mark + "a")
        check("connected: SSH connections to it", ssh() >= 1, ssh())
        r.invoke("app.hosts")
        r.wait(lambda: window()["row"], 15); choose()
        w = window()
        check("Hosts and Ports, the host chosen: Connected, and the button is Disconnect", w["says"] == "Connected" and w["button"] == ("Disconnect", True), {k: w[k] for k in ("says", "button")})
        r.invoke("host.disconnect:far")
        gone = r.wait(lambda: ssh() == 0, 10)
        check("let go of: no SSH connection to it is left", gone, ssh())
        w = r.wait(lambda: (lambda w: w if w["says"] == "Disconnected" else None)(window()), 10) or window()
        check("the window says Disconnected, and the button is Connect", w["says"] == "Disconnected" and w["button"] == ("Connect", True), {k: w[k] for k in ("says", "button")})
        time.sleep(3)
        check("and it stays so (nothing connects by itself)", ssh() == 0 and window()["says"] == "Disconnected", ssh())
        # (Connect, by the button itself.)
        w0 = next((x for x in r.windows() if x.get("title", "").startswith("Hosts and Ports")), None)
        b = next((n for n in r.nodes("Hosts and Ports") if n.get("role") == "Button" and n.get("name") == "Connect"), None)
        if w0 and b:
            c = w0["content"]; r.ts("click", r.name, str(int(c["x"] + b["x"] + b["width"] / 2)), str(int(c["y"] + b["y"] + b["height"] / 2)))
        back = r.wait(lambda: ssh() >= 1 and window()["says"] == "Connected", 25)
        w = window()
        check("the Connect button brings it back", back and w["button"] == ("Disconnect", True), {"ssh": ssh(), "says": w["says"], "button": w["button"]})
        # (And Disconnect by the button, then the command to connect.)
        b = next((n for n in r.nodes("Hosts and Ports") if n.get("role") == "Button" and n.get("name") == "Disconnect"), None)
        if w0 and b:
            c = w0["content"]; r.ts("click", r.name, str(int(c["x"] + b["x"] + b["width"] / 2)), str(int(c["y"] + b["y"] + b["height"] / 2)))
        check("the Disconnect button lets go of it", r.wait(lambda: ssh() == 0 and window()["says"] == "Disconnected", 10), {"ssh": ssh(), "says": window()["says"]})
        r.invoke("host.connect:far")
        check("the command connects it again", r.wait(lambda: ssh() >= 1 and window()["says"] == "Connected", 25), {"ssh": ssh(), "says": window()["says"]})
        print("PASS" if ok else "FAIL")
elif what == "quic":
    # The helper's own link over UDP to the far host: the app says its streams go over it, what is typed reaches
    # the host's shell through it, and with it turned off (OLO_REX_QUIC=no) everything is as before. The far
    # host is this computer: the UDP goes straight to the helper (the slow link is TCP's).
    import re, subprocess
    def run(env, label):
        with Rig("shell", delay=0, far=True, env=["RUST_LOG=info", *env]) as r:
            log = f"/var/tmp/olo-dev/{r.name}/olo-rex.log"
            said = lambda: re.sub(r"\x1b\[[0-9;]*m", "", open(log, errors="replace").read())
            linked = r.wait(lambda: next((l for l in said().splitlines() if "link over UDP" in l or "own link (UDP)" in l), None), 25)
            mark = f"quic{int(time.time()) % 100000}"
            r.type(f"touch {mark}\n"); 
            there = r.wait(lambda: mark in r.far.ran(f"ls $HOME {D} {D}/home 2>/dev/null"), 15)
            udp = subprocess.run(["ss", "-lunH"], capture_output=True, text=True).stdout
            port = next((p for p in re.findall(r":(611\d\d)\s", udp)), None)
            import getpass
            linked = linked and linked.replace(getpass.getuser(), "USER")
            print(f"{label}: the app said: {(linked or '(nothing of a link)')[-110:].strip()!r}; typed, the file is there: {bool(there)}; the helper listens on UDP {port}", flush=True)
            return linked, there, port
    on = run(["OLO_REX_QUIC=yes"], "with the link (waited for)")
    by_itself = run([], "as the app does it (begun behind the first streams)")
    off = run(["OLO_REX_QUIC=no"], "turned off")
    print("PASS" if all(x[0] and "own link (UDP)" in x[0] and x[1] and x[2] for x in (on, by_itself)) and not off[0] and off[1] else "FAIL")
elif what == "behind":
    import re, subprocess
    numbers = [a for a in args if a.isdigit()]
    rate, lines = int(numbers[0]), (numbers[1] if len(numbers) > 1 else "3000000")
    if os.path.exists(f"{D}/tmp/t0"): os.remove(f"{D}/tmp/t0")
    flood = f"sleep 16; date +%s.%N > {D}/tmp/t0; seq {lines}; PS1='you@far:~/project$ ' exec bash --norc -i"
    with Rig(flood, delay=0, far=True, far_delay=50, far_rate=rate, bin=next((a.partition("=")[2] for a in args if a.startswith("--bin=")), None),
             env=["OLO_REX_SSH_OPENED_IN_MS=100", "RUST_LOG=info,olo_rex=debug"]) as r:
        began = time.monotonic()
        while not os.path.exists(f"{D}/tmp/t0") and time.monotonic() - began < 40: time.sleep(0.05)
        b0 = r.far.bytes(); t0 = time.monotonic(); last, since = b0, t0
        while time.monotonic() - t0 < 600:
            time.sleep(0.2); now = r.far.bytes()
            if now["down"] != last["down"]: last, since = now, time.monotonic()
            elif time.monotonic() - since >= 4.0: break
        shot = r.shot("behind")
        print(f"{rate:,} bytes a second, {lines} lines: the far link was quiet again {since - t0:.1f} s after the flood began ({last['down'] - b0['down']:,} bytes came down); pictured in {shot}", flush=True)
    said = subprocess.run(["grep", "-a", "-E", "caught up|catching", f"/var/tmp/olo-dev/{r.name}/olo-rex.log"], capture_output=True, text=True).stdout
    print(re.sub(r"(block|session):[0-9a-f-]+", r"\1:…", re.sub(r"\x1b\[[0-9;]*m", "", said))[-600:] or "(the app said nothing of catching up)")
elif what == "connect":
    import re
    for delay in [int(a) for a in args if a.isdigit()] or [0, 150, 300, 600]:
        os.environ["REX_SLOW_TRACE"] = f"{D}/connect.trace"
        with Rig("shell", delay=delay, bin=next((a.partition("=")[2] for a in args if a.startswith("--bin=")), None)) as r:
            shown = r.ready_after
        # From the link's own notes (each chunk as it is read, before its delay): the first thing the app sent,
        # and the first of the terminal's bytes on the stream that asked for them (what follows its upgrade's
        # answer and its welcome), which reaches the app a delay later. (The rig's own "shown after the window
        # opened" begins a second and more after the app does, and can't see a connection quicker than that.)
        lines = [l.split(None, 5) for l in open(f"{D}/connect.trace")]
        first = float(lines[0][0])
        data = next((l[2] for l in lines if l[3] == ">" and len(l) > 5 and l[5].startswith("GET /api/data")), None)
        down = [float(l[0]) for l in lines if l[2] == data and l[3] == "<"]
        if len(down) >= 3:
            took = down[2] + delay - first
            print(f"{delay} ms each way: the terminal's first screen {took / 1000:.1f} s after the app's first byte" + (f" ({took / (2 * delay):.1f} round trips)" if delay else "") + f"; shown {shown:.1f} s after the window opened", flush=True)
        else:
            print(f"{delay} ms each way: the terminal's stream wasn't seen in the trace; shown {shown:.1f} s after the window opened", flush=True)
        if "--trace" in args:
            # (What passed, in order: each step of twice the delay is a round trip. Names of this computer's
            # and ids are left out of what is printed.)
            for line in open(f"{D}/connect.trace"):
                line = re.sub(r"(session|block|window|layer|client):[0-9a-f-]+", r"\1:…", line.rstrip("\n"))
                line = re.sub(r'"(user|path|invoked_path|cwd|home|hostname|label|ticket|reconnect_token)":"[^"]*"', r'"\1":"…"', line)
                print(line[:150])
else:
    raise SystemExit(__doc__)
print("crash reports:", len(glob.glob(f"{D}/state/olo/crashes/*.json")))
