#!/usr/bin/python3
"""The slow-link rig: a scratch Rex server, a slow link in front of it, and the Rex app headless on the other side.
For trying and measuring what Rex does over a link that takes its time (docs/HACKING.md, Rex, "Typing shown at
once"; docs/AGENT-NOTES.md, "Rex over a slow link, tried here").

    import sys; sys.path.insert(0, "tools/dev/rex-slow"); from rig import *
    with Rig("shell", delay=300) as r:      # or "box", "box-nosync", "line" (tui.py), or a command line
        r.type("echo hi"); print(r.shot("typed"))

Its files are under REX_SLOW_DIR (/var/tmp/olo-rex-slow unless set), a scratch home, runtime and Rex server of its
own among them; the app is this tree's build (dev unless `release=True`), in a `tsession`. Nothing is on the
person's screen or session, and the only processes ended are the ones started here, by their pids. It needs the
`rex` command (the server it starts is a scratch one, at a socket of its own)."""
import json, os, signal, socket, subprocess, sys, time
HERE = os.path.dirname(os.path.abspath(__file__))
D = os.environ.get("REX_SLOW_DIR", "/var/tmp/olo-rex-slow"); R3 = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
ENV = {k: v for k, v in os.environ.items() if k not in ("WAYLAND_DISPLAY", "OLO_SOCKET", "REX_SESSION", "REX_BLOCK", "SSH_AUTH_SOCK")}
ENV.update(XDG_STATE_HOME=f"{D}/state")
CODES = dict(zip("qwertyuiop", range(16, 26))) | dict(zip("asdfghjkl", range(30, 39))) | dict(zip("zxcvbnm", range(44, 51))) | {" ": 57, "\n": 28, "\b": 14, "/": 53, "-": 12, ".": 52, "<": 105, ">": 106, "\t": 15}
PROGRAMS = {
    "shell": "PS1='\\[\\e[1;32m\\]you@far\\[\\e[0m\\]:\\[\\e[1;34m\\]~/project\\[\\e[0m\\]$ ' exec bash --norc -i",
    "box": f"exec /usr/bin/python3 {HERE}/tui.py box",
    "box-nosync": f"exec /usr/bin/python3 {HERE}/tui.py box --no-sync",
    "line": f"exec /usr/bin/python3 {HERE}/tui.py line",
}

class Rig:
    def __init__(self, program="shell", delay=300, jitter=0, stall=(0, 0), name="rexslow", size="2560x1600", scale=None, type_ahead=True, bin=None, look=None, release=False, helper=False):
        self.program, self.delay, self.jitter, self.stall, self.name, self.size, self.scale = program, delay, jitter, stall, name, size, scale
        self.type_ahead, self.look = type_ahead, look
        self.bin = bin or f"{R3}/target/{'release' if release else 'debug'}/olo-rex"
        # Olo's helper on the host (olo-rex-link), behind the link as it is behind SSH: True for this tree's
        # build of it, or a path.
        self.helper = f"{R3}/target/{'release' if release else 'debug'}/olo-rex-link" if helper is True else helper
        self.procs = []
    def ts(self, *a):
        return subprocess.run(["/usr/bin/python3", f"{R3}/tools/dev/tsession", *a], capture_output=True, text=True, env=ENV).stdout
    def scratch(self, *a):
        """A command of the scratch Rex (`rex …` or `server`), away from the person's: a home, a runtime folder
        and a server socket of its own."""
        env = {k: v for k, v in os.environ.items() if k not in ("REX_SESSION", "REX_BLOCK", "SSH_AUTH_SOCK", "OLO_SESSION", "OLO_REAL_SESSION", "DBUS_SESSION_BUS_ADDRESS", "PULSE_SERVER", "WAYLAND_DISPLAY", "OLO_SOCKET")}
        env.update(HOME=f"{D}/home", XDG_RUNTIME_DIR=f"{D}/runtime", XDG_STATE_HOME=f"{D}/state", XDG_CONFIG_HOME=f"{D}/home/.config", XDG_CACHE_HOME=f"{D}/home/.cache", REX_SERVER_SOCKET=f"{D}/rex/rex.sock", TMPDIR=f"{D}/tmp")
        for d in ("home", "runtime", "state", "tmp", "rex", "shots"): os.makedirs(f"{D}/{d}", exist_ok=True)
        os.chmod(f"{D}/runtime", 0o700)
        sock = f"unix://{D}/rex/rex.sock"
        if a[0] == "server":
            return ["rex", "-S", sock, "server", "run", "--listen", sock, "--state-dir", f"{D}/rex/state", "--restore=false"], env
        return ["rex", "-S", sock, "--autostart=false", *a[1:]], env
    def rex(self, *a):
        cmd, env = self.scratch("rex", *a)
        return subprocess.run(cmd, capture_output=True, text=True, timeout=30, env=env, cwd=D).stdout
    def windows(self):
        return json.loads(self.ts("ipc", self.name, '{"type":"list_windows"}') or "{}").get("windows", [])
    def __enter__(self):
        try:
            return self.begin()
        except BaseException:
            self.__exit__()
            raise
    def host(self):
        """The far side by itself: the scratch server and the slow link in front of it (at REX_SLOW_DIR/link.sock),
        with Olo's helper behind the link where the rig has one. For what talks to a host without the app."""
        for f in ("rex/rex.sock", "link.sock"):
            if os.path.exists(f"{D}/{f}"): os.remove(f"{D}/{f}")
        quiet = dict(stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        cmd, env = self.scratch("server")
        self.procs.append(subprocess.Popen(cmd, env=env, cwd=D, **quiet))
        end = time.monotonic() + 8
        while not os.path.exists(f"{D}/rex/rex.sock") and time.monotonic() < end: time.sleep(0.05)
        assert os.path.exists(f"{D}/rex/rex.sock"), "the scratch server didn't start"
        link = ["/usr/bin/python3", f"{HERE}/slowlink.py", f"{D}/link.sock", f"{D}/rex/rex.sock", str(self.delay), str(self.jitter), str(self.stall[0]), str(self.stall[1])]
        dial, env = self.scratch("rex", "__internal", "dial-stdio")
        if self.helper:
            assert os.path.exists(self.helper), f"no helper at {self.helper}: cargo build -p olo-rex-link"
            env["REX_SLOW_HELPER"] = json.dumps([self.helper, "--", *dial])
        self.procs.append(subprocess.Popen(link, env=env, cwd=D, **quiet))
        while not os.path.exists(f"{D}/link.sock") and time.monotonic() < end: time.sleep(0.05)
        return f"{D}/link.sock"
    def begin(self):
        self.host()
        home = f"{D}/home-app"
        os.makedirs(home, exist_ok=True)
        self.ts("start", self.name, self.size, "--home", home, "--owner", "none")
        # (The session's own config folder is where its apps read their settings.)
        config = f"/var/tmp/olo-dev/{self.name}/config/olo"
        os.makedirs(config, exist_ok=True)
        json.dump({"type_ahead": self.type_ahead}, open(f"{config}/rex.json", "w"))
        self.screen_w = int(self.size.split("x")[0]) / 2
        if self.scale: self.ts("scale", self.name, str(self.scale))
        if self.look: self.ts("look", self.name, self.look)
        time.sleep(1.2)
        self.rex("new", "typing", "--json", "--", "sh", "-c", PROGRAMS.get(self.program, self.program))
        self.ts("run", self.name, f"REX_SERVER_SOCKET={D}/link.sock", f"XDG_STATE_HOME={D}/state", "SSH_AUTH_SOCK=", self.bin)
        self.win = None
        for _ in range(80):
            time.sleep(0.25)
            w = self.windows()
            if w: self.win = w[0]; break
        assert self.win, "the app's window didn't open"
        # (Its place once it has settled: a first window is sized after it opens.)
        time.sleep(1.0)
        self.win = next((w for w in self.windows() if w["id"] == self.win["id"]), self.win)
        # The pane is there once the host's snapshot has come over the link: until then the window says it is
        # connecting, on a ground of another colour. (A point low in the window's content, watched for a change.)
        from PIL import Image
        began = time.monotonic(); self.ready_after = None; first = None
        c = self.win["content"]
        while time.monotonic() - began < 120:
            im = Image.open(self.shot("ready")).convert("RGB"); k = im.size[0] / self.screen_w
            px = im.getpixel((min(im.size[0] - 2, int((c["x"] + c["width"] * 0.75) * k)), min(im.size[1] - 2, int((c["y"] + c["height"] * 0.6) * k))))
            # (The ground the window says "Connecting…" on is its menu bar's.)
            first = im.getpixel((min(im.size[0] - 2, int((c["x"] + c["width"] - 20) * k)), int((c["y"] + 14) * k)))
            if max(abs(px[i] - first[i]) for i in range(3)) > 12:
                self.ready_after = time.monotonic() - began; break
            time.sleep(0.2)
        assert self.ready_after is not None, "the pane never showed the host's screen"
        time.sleep(0.5 + 2 * self.delay / 1000)
        c = self.win["content"]
        self.ts("click", self.name, str(c["x"] + c["width"] * 2 // 3), str(c["y"] + c["height"] // 2)); time.sleep(0.4 + 2 * self.delay / 1000)
        return self
    def __exit__(self, *exc):
        self.ts("stop", self.name)
        for p in self.procs:
            try: os.killpg(p.pid, signal.SIGTERM)
            except OSError: pass
    def tap(self, code, held=0.012):
        ev = lambda pressed: json.dumps({"type": "synthetic", "events": [{"type": "key", "key": code, "pressed": pressed}]})
        self.ts("ipc", self.name, ev(True)); time.sleep(held); self.ts("ipc", self.name, ev(False))
    def type(self, text, apart=0.05):
        for ch in text:
            self.tap(CODES[ch]); time.sleep(apart)
    def shot(self, name):
        path = f"{D}/shots/{name}.png"
        self.ts("shot", self.name, path); return path
    def inked(self, path, box=None):
        """How many pixels of the pane's text area (or `box`) aren't its ground."""
        from PIL import Image
        im = Image.open(path).convert("RGB")
        w, h = im.size
        c = self.win["content"]; k = w / self.screen_w
        box = box or (int((c["x"] + 8) * k), int((c["y"] + 60) * k), int((c["x"] + c["width"] - 8) * k), int((c["y"] + c["height"] - 30) * k))
        crop = im.crop(box); ground = crop.getpixel((crop.size[0] - 2, crop.size[1] - 2))
        return sum(1 for px in crop.getdata() if max(abs(px[i] - ground[i]) for i in range(3)) > 40)
    def felt(self):
        return json.loads(self.ts("ipc", self.name, '{"type":"timings","quiet":true}') or "{}").get("timings", {}).get("felt", [])
    def tree(self):
        s = socket.socket(socket.AF_UNIX); s.settimeout(4)
        try:
            s.connect(f"/run/user/{os.getuid()}/olo-menus-{self.win['pid']}.sock"); s.sendall(b'{"tree":true}\n')
            return json.loads(s.makefile().readline() or "[]")
        except (OSError, ValueError):
            return None
        finally:
            s.close()
