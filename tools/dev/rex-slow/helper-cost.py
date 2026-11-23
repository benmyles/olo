#!/usr/bin/python3
"""What Olo's helper on a host (olo-rex-link) costs by itself: the instructions of its own threads (perf stat
--no-scale) for 1,000 letters typed one after another, each answered before the next; for 50 lines ended (an Enter,
then 400 ms, as someone reading: the helper looks at the terminal then and says how it reads); and at rest for 6 s
after. Against the stand-in server of the helper's own tests (cargo test -p olo-rex-link builds it), three runs.

    helper-cost.py [HELPER] [LABEL]     HELPER: a release build of it (default: this tree's,
                                        cargo build --release -p olo-rex-link); to compare, build the one before
                                        (git stash, build, copy the file aside) and run both."""
import glob, json, os, struct, subprocess, sys, time
R3 = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
HELPER = sys.argv[1] if len(sys.argv) > 1 else f"{R3}/target/release/olo-rex-link"
LABEL = sys.argv[2] if len(sys.argv) > 2 else "this tree's"
standin = max((p for p in glob.glob(f"{R3}/target/debug/build/olo-rex-link/*/out/through-*") if not p.endswith(".d")), key=os.path.getmtime)
env = {k: v for k, v in os.environ.items() if k not in ("WAYLAND_DISPLAY", "OLO_SOCKET")} | {"OLO_REX_LINK_STANDIN": "--reads --slow-ms 0"}
def frame(text):
    b = text.encode(); head = bytes([0x81, 0x80 | len(b)]) if len(b) < 126 else bytes([0x81, 0x80 | 126]) + struct.pack(">H", len(b))
    key = b"\x11\x22\x33\x44"
    return head + key + bytes(c ^ key[i % 4] for i, c in enumerate(b))
def read_exact(f, n):
    out = b""
    while len(out) < n:
        got = f.read(n - len(out))
        if not got: raise EOFError
        out += got
    return out
def next_text(f):
    h = read_exact(f, 2); n = h[1] & 0x7f
    if n == 126: n = struct.unpack(">H", read_exact(f, 2))[0]
    elif n == 127: n = struct.unpack(">Q", read_exact(f, 8))[0]
    return json.loads(read_exact(f, n))
def counted(pid, work, secs):
    p = subprocess.Popen(["perf", "stat", "--no-scale", "--per-thread", "-x,", "-e", "instructions:u", "-p", str(pid), "--timeout", str(int(secs * 1000))], stderr=subprocess.PIPE, stdout=subprocess.DEVNULL, text=True)
    time.sleep(0.5); work(); err = p.communicate()[1]
    return sum(int(f[1]) for f in (l.split(",") for l in err.splitlines()) if len(f) > 3 and f[1].isdigit())
for turn in range(3):
    h = subprocess.Popen([HELPER, "--", standin], stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env, bufsize=0)
    h.stdin.write(b"GET /api/control HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\n\r\n")
    head = b""
    while not head.endswith(b"\r\n\r\n"): head += read_exact(h.stdout, 1)
    said = [l for l in head.decode().split("\r\n") if l.startswith("X-Olo")]
    ids = iter(range(1, 10**9)); words = [0]
    def write(data):
        i = next(ids)
        h.stdin.write(frame(json.dumps({"type": "request", "id": str(i), "method": "com.superlogical.terminal.write", "payload": {"session_id": "s", "block_id": "b", "args": {"data": data}}})))
        while True:
            v = next_text(h.stdout)
            if v.get("type") == "response" and v.get("id") == str(i): return
            if "input_mode_changed" in json.dumps(v): words[0] += 1
    def letters():
        for _ in range(1000): write("k")
    def lines():
        for _ in range(50):
            write("ls\r"); time.sleep(0.4)
    a = counted(h.pid, letters, 6)
    b = counted(h.pid, lines, 24)
    c = counted(h.pid, lambda: None, 6)
    print(f"{LABEL} ({said}), turn {turn}: 1,000 letters {a:,} instructions ({a / 1000:,.0f} each); 50 lines ended {b:,} ({b / 50:,.0f} each; {words[0]} words said so far); 6 s at rest {c:,}", flush=True)
    h.stdin.close(); h.wait(timeout=10)
