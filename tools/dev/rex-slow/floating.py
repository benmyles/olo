#!/usr/bin/python3
"""A floating terminal pane in Rex can be used: it stands inside its tab's panes, is the pane typed into when it is
made and when it is clicked, moves and sizes by a drag, and closes.

It could do none of these (Ben, 2026-11-27: "i just opened a new floating terminal pane and it shows but i can't
select it or click on it or get rid of it or do anything with it now"), for five reasons the second investigator
proved on this rig, there since floating layers were first drawn:
  1. its place was a share of the whole window, set down from the panes' corner: with the sidebar open it ran past
     the window's right edge, its frame, its button and its corner with it;
  2. a press on it went on to the tiled pane under the pointer, which took the focus straight back;
  3. it was made without the focus (`session.new_layer` with no "focus");
  4. it had no close button, and Close Pane closed the pane under it;
  5. what a drag began with was kept in a cell made anew at each redraw, so a drag went one step and stopped.

Here, with a 290-point Activity sidebar and two tiled panes, a new floating terminal (`layer.new_floating`):
fits within the panes' area with its minimize and close buttons on the screen; has the focus as it is made; keeps
it (the server's `focused_block_id`) when its body or its bar is clicked, and gives it up when a tiled pane
beside it is; moves and sizes by the whole of a drag in several steps; and is closed by its button.

    floating.py [BIN]      BIN: another build of olo-rex to try (the build before the fix fails most of these);
                           else this tree's dev build
"""
import base64, json, os, socket, struct, sys, time

os.environ.setdefault("REX_SLOW_DIR", "/var/tmp/olo-rex-floating")
os.environ.setdefault("REX_SLOW_NAME", "rex-floating")
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from rig import Rig, D

ok = True


def check(what, passed, shown):
    global ok
    ok = ok and bool(passed)
    print(("ok   " if passed else "FAIL ") + what + ":", repr(shown)[:260], flush=True)


class Client:
    def __init__(self, sock, name="floating-test"):
        self.s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.s.settimeout(5.0)
        self.s.connect(sock)
        key = base64.b64encode(os.urandom(16)).decode()
        self.s.sendall(
            (
                "GET /api/control HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: rex.control.v1\r\n\r\n"
            ).encode()
        )
        self.buf = b""
        while b"\r\n\r\n" not in self.buf:
            more = self.s.recv(4096)
            if not more:
                raise RuntimeError("closed during handshake")
            self.buf += more
        head, self.buf = self.buf.split(b"\r\n\r\n", 1)
        assert b"101 " in head.splitlines()[0], f"no WS: {head!r}"
        self.sent = 0
        self.send({"type": "hello", "api_revision": 19, "api_min_revision": 1, "client": {"kind": "service", "platform": "linux", "name": name, "version": "0.1.0"}})
        self.id = (self.frame().get("client") or {}).get("id")

    def read(self, n):
        while len(self.buf) < n:
            more = self.s.recv(65536)
            if not more:
                raise EOFError("closed")
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

    def send(self, msg):
        self.write(0x1, json.dumps(msg).encode())

    def frame(self):
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


def find_key(v, key, out):
    if isinstance(v, dict):
        if isinstance(v.get(key), str):
            out.append(v[key])
        for x in v.values():
            find_key(x, key, out)
    elif isinstance(v, list):
        for x in v:
            find_key(x, key, out)
    return out


def wv(cli, sid):
    return cli.call("session.view", {"session_id": sid}).get("windows", [{}])[0]


def main():
    bin_path = sys.argv[1] if len(sys.argv) > 1 else None
    with Rig("shell", delay=0, size="2620x1860", scale=2, bin=bin_path, settings={"sidebar_width": 290.0, "sidebar_mode": "Activity"}) as r:
        cli = Client(f"{D}/rex/rex.sock")
        try:
            sid = find_key(cli.call("session.bootstrap", {"event_scope": "all"}), "session_id", [])[0]
            r.invoke("pane.split_right")
            time.sleep(0.7)
            r.invoke("layer.new_floating")
            time.sleep(0.9)

            win = next(w for w in r.windows() if w.get("title") == "Rex")
            c = win["content"]
            view = wv(cli, sid)
            fl = next(l for l in view["layers"] if l["kind"] != "tiled")
            fl_lid, fl_bid = fl["layer_id"], fl["blocks"][0]["block_id"]
            tiled_bids = [b["block_id"] for l in view["layers"] if l["kind"] == "tiled" for b in l["blocks"]]

            nodes = r.nodes("Rex")
            dialog = next((n for n in nodes if n.get("role") == "Dialog" and n.get("name") == "Floating Terminal"), None)
            terms = [n for n in nodes if n.get("role") == "Terminal"]
            # Panes area right edge is window content width; panes area bottom is tiled terminal bottom.
            panes_left = min(t["x"] for t in terms[:2])
            panes_right = c["width"]
            panes_w = panes_right - panes_left
            fl_size = cli.call("com.superlogical.terminal.size", {"session_id": sid, "block_id": fl_bid, "args": {}})
            fl_grid_w = fl_size["columns"] * (fl_size["cell_width_px"] / 2.0)

            check("floating pane fits within panes area width (not sized to whole window)",
                  dialog is not None and (dialog["x"] + dialog["width"] < panes_right - 4) and fl_grid_w < panes_w * 0.80,
                  {"dialog": dialog, "panes_w": panes_w, "fl_grid_w": fl_grid_w, "win_w": c["width"]})

            min_btn = next((n for n in nodes if n.get("role") == "Button" and n.get("name") in ("—", "Minimize Floating Terminal", "Minimize")), None)
            close_btn = next((n for n in nodes if n.get("role") == "Button" and n.get("name") in ("Close Floating Terminal", "Close Floating Pane")), None)
            check("minimize button is on-screen inside floating frame",
                  min_btn is not None and not min_btn.get("offscreen") and min_btn["x"] + min_btn["width"] <= panes_right,
                  min_btn)
            check("close button is present and on-screen inside floating frame",
                  close_btn is not None and not close_btn.get("offscreen") and close_btn["x"] + close_btn["width"] <= panes_right,
                  close_btn)

            check("new floating pane receives focus on creation",
                  view.get("focused_block_id") == fl_bid,
                  {"focused": view.get("focused_block_id"), "fl": fl_bid})

            # Click left tiled pane outside floating card to move focus away first.
            r.ts("click", r.name, str(int(c["x"] + panes_left + 30)), str(int(c["y"] + 110)))
            time.sleep(0.4)
            check("clicking tiled pane outside floating card focuses tiled pane",
                  wv(cli, sid).get("focused_block_id") == tiled_bids[0],
                  wv(cli, sid).get("focused_block_id"))

            # Click inside floating pane body OVER the right tiled pane -> must focus floating pane!
            click_x = int(c["x"] + dialog["x"] + min(dialog["width"] * 0.7, panes_right - dialog["x"] - 40))
            click_y = int(c["y"] + dialog["y"] + 120)
            r.ts("click", r.name, str(click_x), str(click_y))
            time.sleep(0.4)
            check("left-clicking floating body over tiled pane focuses floating pane",
                  wv(cli, sid).get("focused_block_id") == fl_bid,
                  {"focused": wv(cli, sid).get("focused_block_id"), "fl": fl_bid})

            # Move focus away again, then click floating titlebar -> must focus floating pane!
            r.ts("click", r.name, str(int(c["x"] + panes_left + 30)), str(int(c["y"] + 110)))
            time.sleep(0.4)
            r.ts("click", r.name, str(int(c["x"] + dialog["x"] + 30)), str(int(c["y"] + dialog["y"] + 12)))
            time.sleep(0.4)
            check("left-clicking floating titlebar focuses floating pane",
                  wv(cli, sid).get("focused_block_id") == fl_bid,
                  {"focused": wv(cli, sid).get("focused_block_id"), "fl": fl_bid})

            # Drag titlebar by (+60, +40) across multiple steps -> bounds must move ~60/panes_w (not 1/10th!).
            b0 = next(l for l in wv(cli, sid)["layers"] if l["layer_id"] == fl_lid)["bounds"]
            sx, sy = int(c["x"] + dialog["x"] + 30), int(c["y"] + dialog["y"] + 12)
            r.ts("drag", r.name, str(sx), str(sy), str(sx + 60), str(sy + 40))
            time.sleep(0.5)
            b1 = next(l for l in wv(cli, sid)["layers"] if l["layer_id"] == fl_lid)["bounds"]
            dx_pts = (b1[0] - b0[0]) * panes_w
            check("dragging titlebar (+60,+40) moves floating pane by full drag (>35 pt, not 1 step)",
                  dx_pts > 35.0,
                  {"before": b0, "after": b1, "dx_pts": round(dx_pts, 1)})

            # Drag resize handle by (-60, -40) when on-screen -> width must shrink >35 pt.
            nodes2 = r.nodes("Rex")
            d2 = next(n for n in nodes2 if n.get("role") == "Dialog" and n.get("name") == "Floating Terminal")
            rx, ry = int(c["x"] + d2["x"] + d2["width"] - 5), int(c["y"] + d2["y"] + d2["height"] - 5)
            r.ts("drag", r.name, str(rx), str(ry), str(rx - 60), str(ry - 40))
            time.sleep(0.5)
            b2 = next(l for l in wv(cli, sid)["layers"] if l["layer_id"] == fl_lid)["bounds"]
            dw_pts = (b1[2] - b2[2]) * panes_w
            check("dragging resize handle (-60,-40) resizes floating pane by full drag (>35 pt)",
                  dw_pts > 35.0,
                  {"before": b1, "after": b2, "dw_pts": round(dw_pts, 1)})

            # Put away by its Minimize button: its head alone at the panes' foot, a button named for it; pressed, the
            # pane is back where it was.
            def press(node):
                r.ts("click", r.name, str(int(c["x"] + node["x"] + node["width"] / 2)), str(int(c["y"] + node["y"] + node["height"] / 2)))
            button = lambda begins: next((n for n in r.nodes("Rex") if n.get("role") == "Button" and str(n.get("name") or "").startswith(begins)), None)
            state = lambda: next((l.get("state") for l in wv(cli, sid)["layers"] if l["layer_id"] == fl_lid), None)
            mb = button("Minimize Floating Terminal")
            if mb: press(mb)
            chip = r.wait(lambda: button("Restore "), 5)
            away = state()
            check("its Minimize button puts it away: no card, a button to bring it back, inside the panes",
                  mb and chip and not button("Close Floating Terminal") and chip["x"] + chip["width"] <= panes_right + 1,
                  {"layer": away, "chip": chip and {k: chip.get(k) for k in ("name", "x", "y", "width", "height")}})
            if chip: press(chip)
            back = r.wait(lambda: button("Close Floating Terminal"), 5)
            check("that button brings it back, as large as it was",
                  back and [round(v, 3) for v in next((l["bounds"] for l in wv(cli, sid)["layers"] if l["layer_id"] == fl_lid), [])] == [round(v, 3) for v in b2],
                  {"layer": state(), "bounds": next((l.get("bounds") for l in wv(cli, sid)["layers"] if l["layer_id"] == fl_lid), None)})

            # Close via close button if present, else Pane > Close Pane.
            nodes3 = r.nodes("Rex")
            cb = next((n for n in nodes3 if n.get("role") == "Button" and n.get("name") in ("Close Floating Terminal", "Close Floating Pane")), None)
            if cb and not cb.get("offscreen"):
                r.ts("click", r.name, str(int(c["x"] + cb["x"] + cb["width"] / 2)), str(int(c["y"] + cb["y"] + cb["height"] / 2)))
            else:
                r.invoke("pane.close")
            time.sleep(0.6)
            rem = [l["layer_id"] for l in wv(cli, sid)["layers"] if l["layer_id"] == fl_lid]
            check("closing floating pane removes the floating layer",
                  not rem,
                  {"remaining_layers": [l["layer_id"] for l in wv(cli, sid)["layers"]]})
        finally:
            cli.close()


main()
print("ALL OK" if ok else "SOMETHING FAILED")
sys.exit(0 if ok else 1)
