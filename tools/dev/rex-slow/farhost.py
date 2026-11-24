#!/usr/bin/python3
"""A far host that is this computer: an SSH server of the rig's own on 127.0.0.1, for what Rex only does over SSH
(Olo's helper put on a host, what a host is asked, the status bar's marks) without a real host.

    from farhost import FarHost
    with FarHost() as far:          # with a scratch Rex server (rig.Rig().host() started it, or start=True)
        far.address                 # ssh://YOU@127.0.0.1:PORT/…/rex.sock, for a saved host (the scratch server's socket)
        far.agent                   # SSH_AUTH_SOCK for the app: an agent of the rig's own, holding the one key
        far.home                    # the "host's" home: what is installed there is found here
        far.bytes()                 # with FarHost(delay=MS): what has passed over its link, {"up", "down", "connections"}

FarHost(delay=MS) puts the rig's slow link in front of the SSH server (slowlink.py, over TCP: each way MS late, in
order) and counts the bytes that pass it: the address is the link's, so everything Rex does with the host goes
through it. Without, the address is the SSH server's own.

sshd runs as you (it can log nobody else in), with keys of its own under REX_SLOW_DIR/far, lets in only the rig's
own key, and gives every session a scratch HOME (SetEnv): nothing of yours is read or written (not ~/.ssh, not your Rex server, not your agent). It listens on 127.0.0.1 alone and ends with the
`with`. The shell it runs is your login shell, with the scratch HOME (so none of your profile)."""
import getpass, json, os, signal, socket, subprocess, sys, time
D = os.environ.get("REX_SLOW_DIR", "/var/tmp/olo-rex-slow")

def started(proc):
    """A process the rig started, noted with when it began (`REX_SLOW_DIR/started`): a run that is killed (a
    timeout, a session that ends) never reaches its own tidying, and its server, links and agents were found
    running hours later (three agents, seven listeners: 2026-11-23). `swept` ends them at the next run."""
    try:
        begun = open(f"/proc/{proc.pid}/stat").read().rsplit(")", 1)[1].split()[19]
        with open(f"{D}/started", "a") as f: f.write(f"{proc.pid} {begun}\n")
    except (OSError, IndexError): pass
    return proc

def swept():
    """What an earlier run started and left: ended, each only if it is still the process noted (the same pid begun
    at the same moment)."""
    try: lines = open(f"{D}/started").read().split("\n")
    except OSError: return
    for line in lines:
        try:
            pid, begun = line.split()
            if open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()[19] == begun:
                os.killpg(int(pid), signal.SIGTERM)
        except (OSError, ValueError, IndexError): pass
    try: os.remove(f"{D}/started")
    except OSError: pass

class FarHost:
    def __init__(self, delay=None, rate=None):
        # `rate`: the far link carries no more than so many bytes a second each way (slowlink.py's
        # REX_SLOW_RATE: a narrow link, on which a flood holds up what comes behind it).
        self.delay, self.rate = delay, rate
        # (Its home is the scratch Rex server's sessions' own: what a script typed into a pane puts there is what
        # a command run over SSH finds.)
        self.dir = f"{D}/far"; self.home = f"{D}/home"; self.agent = f"{self.dir}/agent.sock"
        self.procs = []
    def __enter__(self):
        try:
            return self.begin()
        except BaseException:
            self.__exit__()
            raise
    def begin(self):
        quiet = dict(stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        # (Its own folder made too: it was there only where a run before had left it, and a rig of a folder of
        # its own, at_once.py's, failed for want of it.)
        os.makedirs(self.home, exist_ok=True); os.makedirs(self.dir, exist_ok=True); os.chmod(self.dir, 0o700)
        for key in ("host_key", "client_key"):
            if not os.path.exists(f"{self.dir}/{key}"):
                subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "olo-rex-rig", "-f", f"{self.dir}/{key}"], check=True, **quiet)
        with open(f"{self.dir}/authorized_keys", "w") as f: f.write(open(f"{self.dir}/client_key.pub").read())
        os.chmod(f"{self.dir}/authorized_keys", 0o600)
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0)); self.port = s.getsockname()[1]
        env = {"HOME": self.home, "XDG_RUNTIME_DIR": f"{D}/runtime", "XDG_STATE_HOME": f"{D}/state", "XDG_CONFIG_HOME": f"{self.home}/.config", "XDG_CACHE_HOME": f"{self.home}/.cache",
               "REX_SERVER_SOCKET": f"{D}/rex/rex.sock", "TMPDIR": f"{D}/tmp",
               # (A Token Pool put there by a test: on ports of the rig's own, the person's pool being on this
               # computer too, and asking a provider that isn't there.)
               "OLO_POOL_PORT": "27891", "OLO_POOL_PROXY_PORT": "27892", "OLO_POOL_UPSTREAM": os.environ.get("FAR_POOL_UPSTREAM", "http://127.0.0.1:9"),
               # (The system's programs, and none of yours: with your own folders on its path, the "host" had your
               # agents installed before any test put one there, found by name.)
               "PATH": ":".join(p for p in os.environ.get("PATH", "/usr/bin:/bin").split(":") if p and not (p + "/").startswith(os.path.expanduser("~") + "/")) or "/usr/bin:/bin"}
        with open(f"{self.dir}/sshd_config", "w") as f:
            f.write("\n".join([f"Port {self.port}", "ListenAddress 127.0.0.1", f"HostKey {self.dir}/host_key", f"PidFile {self.dir}/sshd.pid", f"AuthorizedKeysFile {self.dir}/authorized_keys",
                               "PasswordAuthentication no", "KbdInteractiveAuthentication no", "UsePAM no", "StrictModes no", "PermitUserEnvironment no", "AllowAgentForwarding yes",
                               "AllowTcpForwarding yes", "X11Forwarding no", *([f"MaxSessions {os.environ['FAR_MAX_SESSIONS']}"] if os.environ.get("FAR_MAX_SESSIONS") else []), "PrintMotd no", "LogLevel ERROR", "SetEnv " + " ".join(f'{k}="{v}"' for k, v in env.items()), ""]))
        self.procs.append(started(subprocess.Popen(["/usr/sbin/sshd", "-D", "-e", "-f", f"{self.dir}/sshd_config"], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=open(f"{self.dir}/sshd.log", "w"), start_new_session=True)))
        if os.path.exists(self.agent): os.remove(self.agent)
        # (In the foreground, so it is this script's to end.)
        self.procs.append(started(subprocess.Popen(["ssh-agent", "-D", "-a", self.agent], start_new_session=True, **quiet)))
        end = time.monotonic() + 8
        while time.monotonic() < end:
            listening = False
            with socket.socket() as s:
                listening = s.connect_ex(("127.0.0.1", self.port)) == 0
            if listening and os.path.exists(self.agent): break
            time.sleep(0.05)
        else:
            raise SystemExit(f"the rig's sshd didn't start: {open(f'{self.dir}/sshd.log').read()[-300:]}")
        subprocess.run(["ssh-add", "-q", f"{self.dir}/client_key"], env=dict(os.environ, SSH_AUTH_SOCK=self.agent), check=True, **quiet)
        self.link = self.port
        if self.delay is not None:
            with socket.socket() as s:
                s.bind(("127.0.0.1", 0)); self.link = s.getsockname()[1]
            self.count = f"{self.dir}/bytes.json"
            if os.path.exists(self.count): os.remove(self.count)
            here = os.path.dirname(os.path.abspath(__file__))
            self.procs.append(started(subprocess.Popen([sys.executable, f"{here}/slowlink.py", f"tcp:{self.link}", f"tcp:{self.port}", str(self.delay)], env=dict(os.environ, REX_SLOW_COUNT=self.count, **({"REX_SLOW_RATE": str(self.rate)} if self.rate else {})),
                                               start_new_session=True, **quiet)))
            end = time.monotonic() + 8
            while time.monotonic() < end:
                with socket.socket() as s:
                    if s.connect_ex(("127.0.0.1", self.link)) == 0: break
                time.sleep(0.05)
            else:
                raise SystemExit("the far host's slow link didn't start")
        # (With the scratch server's socket in it: the host's `rex` would find, or start, a server of its own.)
        self.address = f"ssh://{getpass.getuser()}@127.0.0.1:{self.link}{D}/rex/rex.sock"
        return self
    def bytes(self):
        """What has passed over the far host's link so far (FarHost(delay=…)): up is what the app sent."""
        try: return json.load(open(self.count))
        except (OSError, ValueError): return {"up": 0, "down": 0, "connections": 0}
    def __exit__(self, *exc):
        for p in self.procs:
            try: os.killpg(p.pid, signal.SIGTERM)
            except OSError: pass
        # A Rex server of the far host's own: a `rex` there that was told no socket, or found none at the one it
        # was told, starts one at its default place (under the rig's runtime folder), which outlives the run (one
        # was found hours later with two test sessions in it, 2026-11-23). Told to stop by its own socket; never
        # started to be stopped, and never another's (the environment is the far host's, with no socket named).
        if os.path.exists(f"{D}/runtime/rex/server.sock"):
            env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": self.home, "XDG_RUNTIME_DIR": f"{D}/runtime", "XDG_STATE_HOME": f"{D}/state", "XDG_CONFIG_HOME": f"{self.home}/.config",
                   "XDG_CACHE_HOME": f"{self.home}/.cache"}
            try: subprocess.run(["rex", "--autostart=false", "--timeout", "5s", "server", "stop"], env=env, capture_output=True, timeout=15)
            except (OSError, subprocess.TimeoutExpired): pass
    def ran(self, command, timeout=30):
        """A command on the "far host", as Rex would run it (for a test to look at what is there)."""
        out = subprocess.run(["ssh", "-p", str(self.port), "-i", f"{self.dir}/client_key", "-o", "IdentitiesOnly=yes", "-o", "IdentityAgent=none", "-o", f"UserKnownHostsFile={self.dir}/known_hosts",
                              "-o", "StrictHostKeyChecking=accept-new", "-o", "BatchMode=yes", "-F", "/dev/null", f"{getpass.getuser()}@127.0.0.1", command], capture_output=True, text=True, timeout=timeout)
        return out.stdout + out.stderr

if __name__ == "__main__":
    import sys
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    from rig import Rig
    r = Rig("shell")
    try:
        r.host()
        with FarHost() as far:
            print("the far host says:", repr(far.ran("echo home=$HOME; rex --timeout 5s ls 2>&1 | head -3")[:300]))
    finally:
        r.__exit__()
