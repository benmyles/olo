#!/usr/bin/python3
"""A far host that is this computer: an SSH server of the rig's own on 127.0.0.1, for what Rex only does over SSH
(Olo's helper put on a host, what a host is asked, the status bar's marks) without a real host.

    from farhost import FarHost
    with FarHost() as far:          # with a scratch Rex server (rig.Rig().host() started it, or start=True)
        far.address                 # ssh://YOU@127.0.0.1:PORT/…/rex.sock, for a saved host (the scratch server's socket)
        far.agent                   # SSH_AUTH_SOCK for the app: an agent of the rig's own, holding the one key
        far.home                    # the "host's" home: what is installed there is found here

sshd runs as you (it can log nobody else in), with keys of its own under REX_SLOW_DIR/far, lets in only the rig's
own key, and gives every session a scratch HOME (SetEnv): nothing of yours is read or written (not ~/.ssh, not your Rex server, not your agent). It listens on 127.0.0.1 alone and ends with the
`with`. The shell it runs is your login shell, with the scratch HOME (so none of your profile)."""
import getpass, os, signal, socket, subprocess, time
D = os.environ.get("REX_SLOW_DIR", "/var/tmp/olo-rex-slow")

class FarHost:
    def __init__(self):
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
        os.makedirs(self.home, exist_ok=True); os.chmod(self.dir, 0o700)
        for key in ("host_key", "client_key"):
            if not os.path.exists(f"{self.dir}/{key}"):
                subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "olo-rex-rig", "-f", f"{self.dir}/{key}"], check=True, **quiet)
        with open(f"{self.dir}/authorized_keys", "w") as f: f.write(open(f"{self.dir}/client_key.pub").read())
        os.chmod(f"{self.dir}/authorized_keys", 0o600)
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0)); self.port = s.getsockname()[1]
        env = {"HOME": self.home, "XDG_RUNTIME_DIR": f"{D}/runtime", "XDG_STATE_HOME": f"{D}/state", "XDG_CONFIG_HOME": f"{self.home}/.config", "XDG_CACHE_HOME": f"{self.home}/.cache",
               "REX_SERVER_SOCKET": f"{D}/rex/rex.sock", "TMPDIR": f"{D}/tmp", "PATH": os.environ.get("PATH", "/usr/bin:/bin")}
        with open(f"{self.dir}/sshd_config", "w") as f:
            f.write("\n".join([f"Port {self.port}", "ListenAddress 127.0.0.1", f"HostKey {self.dir}/host_key", f"PidFile {self.dir}/sshd.pid", f"AuthorizedKeysFile {self.dir}/authorized_keys",
                               "PasswordAuthentication no", "KbdInteractiveAuthentication no", "UsePAM no", "StrictModes no", "PermitUserEnvironment no", "AllowAgentForwarding no",
                               "AllowTcpForwarding yes", "X11Forwarding no", "PrintMotd no", "LogLevel ERROR", "SetEnv " + " ".join(f'{k}="{v}"' for k, v in env.items()), ""]))
        self.procs.append(subprocess.Popen(["/usr/sbin/sshd", "-D", "-e", "-f", f"{self.dir}/sshd_config"], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=open(f"{self.dir}/sshd.log", "w"), start_new_session=True))
        if os.path.exists(self.agent): os.remove(self.agent)
        # (In the foreground, so it is this script's to end.)
        self.procs.append(subprocess.Popen(["ssh-agent", "-D", "-a", self.agent], start_new_session=True, **quiet))
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
        # (With the scratch server's socket in it: the host's `rex` would find, or start, a server of its own.)
        self.address = f"ssh://{getpass.getuser()}@127.0.0.1:{self.port}{D}/rex/rex.sock"
        return self
    def __exit__(self, *exc):
        for p in self.procs:
            try: os.killpg(p.pid, signal.SIGTERM)
            except OSError: pass
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
