#!/usr/bin/python3
"""A tab whose size another client holds is sized to Rex's view once the person chooses it.

In Rex's protocol a pane has an owner, whose size is the one that holds: whoever makes a pane while attached to
its session owns it. Olo's agents service made its children's tabs so (from its second child in a session on),
at the size of the pane each was asked from, and Rex never took a pane from anybody: such a tab stood at that
size for as long as it ran, and in a shorter view its last rows, where an agent's prompt is, were cut off (Ben,
2026-11-27: "some antigravity agy terminals that are launched as subagents ... it doesn't show the full tui
stuff like the typing line"; a 108 x 64 tab in a view of 56 rows). Rex takes a tab's panes when the person
chooses it (`RexHub::select`); the service gives its children's up (`Service::never_its_size`, tried in
crates/rex-link/tests/agents.rs).

Here: Rex on the rig's scratch Rex server; another client, attached to the session, makes a tab of 108 x 64 (the
server makes it that client's own); the tab is chosen in Rex (Window > Next); the server is asked who owns it and
how large it is: Rex, and Rex's view of it.

    own.py [BIN]      BIN: another build of olo-rex to try (the build before the fix leaves it 108 x 64, the
                      other client's); else this tree's dev build
"""
import base64, json, os, socket, struct, sys, time

os.environ.setdefault("REX_SLOW_DIR", "/var/tmp/olo-rex-own")
os.environ.setdefault("REX_SLOW_NAME", "rex-own")
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from rig import *

ok = True


def check(what, passed, shown):
    global ok
    ok = ok and bool(passed)
    print(("ok   " if passed else "FAIL ") + what + ":", repr(shown)[:300], flush=True)


class Client:
    """Another client of the Rex server: its control API over a WebSocket on the server's unix socket (the second
    investigator's, 2026-11-27)."""

    def __init__(self, sock, name):
        self.s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.s.settimeout(5.0)
        self.s.connect(sock)
        key = base64.b64encode(os.urandom(16)).decode()
        self.s.sendall(("GET /api/control HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: rex.control.v1\r\n\r\n").encode())
        self.buf = b""
        while b"\r\n\r\n" not in self.buf:
            more = self.s.recv(4096)
            if not more:
                raise RuntimeError("the server closed the connection as it was made")
            self.buf += more
        head, self.buf = self.buf.split(b"\r\n\r\n", 1)
        if b"101 " not in head.splitlines()[0]:
            raise RuntimeError(f"no WebSocket: {head!r}")
        self.sent = 0
        self.send({"type": "hello", "api_revision": 19, "api_min_revision": 1, "client": {"kind": "service", "platform": "linux", "name": name, "version": "0.1.0"}})
        self.id = (self.frame().get("client") or {}).get("id")

    def read(self, n):
        while len(self.buf) < n:
            more = self.s.recv(65536)
            if not more:
                raise EOFError("the server closed the connection")
            self.buf += more
        out, self.buf = self.buf[:n], self.buf[n:]
        return out

    def write(self, opcode, payload):
        mask = os.urandom(4)
        head = bytearray([0x80 | opcode])
        n = len(payload)
        if n < 126:
            head.append(0x80 | n)
        elif n < 65536:
            head.append(0x80 | 126)
            head.extend(struct.pack("!H", n))
        else:
            head.append(0x80 | 127)
            head.extend(struct.pack("!Q", n))
        self.s.sendall(bytes(head) + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(payload)))

    def send(self, message):
        self.write(0x1, json.dumps(message).encode())

    def frame(self):
        """The next text frame, read as JSON (pings answered, the rest passed over)."""
        while True:
            b1, b2 = self.read(2)
            n = b2 & 0x7F
            if n == 126:
                n = struct.unpack("!H", self.read(2))[0]
            elif n == 127:
                n = struct.unpack("!Q", self.read(8))[0]
            mask = self.read(4) if b2 & 0x80 else None
            data = self.read(n)
            if mask:
                data = bytes(b ^ mask[i % 4] for i, b in enumerate(data))
            if b1 & 0x0F == 0x9:
                self.write(0xA, data)
            elif b1 & 0x0F == 0x1:
                return json.loads(data)

    def call(self, method, payload=None):
        self.sent += 1
        self.send({"type": "request", "id": str(self.sent), "method": method, **({"payload": payload} if payload is not None else {})})
        while True:
            said = self.frame()
            if said.get("type") == "response" and said.get("id") == str(self.sent):
                return said.get("payload") or {}

    def close(self):
        try:
            self.write(0x8, b"")
        except OSError:
            pass
        self.s.close()


def found(value, key, out):
    if isinstance(value, dict):
        if isinstance(value.get(key), str):
            out.append(value[key])
        for inner in value.values():
            found(inner, key, out)
    elif isinstance(value, list):
        for inner in value:
            found(inner, key, out)
    return out


with Rig("shell", delay=0, size="1280x800", scale=1, bin=(sys.argv[1] if len(sys.argv) > 1 else None)) as r:
    other = Client(f"{D}/rex/rex.sock", "Another client")
    try:
        sessions = found(other.call("session.bootstrap", {"event_scope": "all"}), "session_id", [])
        sid = sessions[0] if sessions else None
        check("the rig's session, seen by another client", sid and other.id, {"session": sid, "client": other.id})
        other.call("session.attach", {"session_id": sid})
        made = other.call("session.new_window", {"session_id": sid, "window_label": "theirs", "focus": False, "layout": {"block": {"flavor": "com.superlogical.terminal.shell", "options": {"command": ["/bin/sh"], "interactive": True, "size": {"columns": 108, "rows": 64}}}}})
        block = made["block_ids"][0]

        def size():
            said = other.call("com.superlogical.terminal.size", {"session_id": sid, "block_id": block, "args": {}})
            return said.get("columns"), said.get("rows"), said.get("owner")

        before = size()
        check("a tab made by an attached client is that client's, at the size it gave", before == (108, 64, other.id), before)
        time.sleep(1.0)
        check("shown nowhere yet, it is as it was", size() == before, size())
        # The person chooses the tab (Window > Next: the second of two). Taken first, then sized at Rex's next paint.
        said = r.invoke("window.next")
        r.wait(lambda: (lambda s: s if s[2] != other.id and (s[0], s[1]) != (108, 64) else None)(size()), 8, every=0.1)
        now = size()
        check("chosen in Rex: the tab is Rex's own, and as large as Rex's view of it", now[2] not in (None, other.id) and (now[0], now[1]) != (108, 64) and now[0] > 20 and now[1] > 5, {"asked": said, "size": now[:2], "still the other client's": now[2] == other.id})
    finally:
        other.close()
print("ALL OK" if ok else "SOMETHING FAILED")
sys.exit(0 if ok else 1)
