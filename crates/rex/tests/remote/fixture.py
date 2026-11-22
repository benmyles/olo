"""Private pid, mount, and network namespace for one Rex integration test."""
import contextlib
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time

ROOT = Path("/fixture")
SOCKET = ROOT / "rex/rex.sock"
ADDR = "unix://" + str(SOCKET)


def outer(binary, case):
    programs = ["bwrap", "rex"]
    if case.startswith("ssh") or case.startswith("remote"):
        programs += ["sshd", "ssh-keygen"]
    if case == "ssh_agent":
        programs += ["ssh-agent", "ssh-add"]
    if case in ("remote_resources", "ssh_proxy_command"):
        programs += ["ncat"]
    if case in ("https_carrier", "ssh_password", "ssh_interactive"):
        programs += ["openssl"]
    if case == "remote_slow":
        programs += ["tc"]
    for program in programs:
        if not shutil.which(program):
            print(f"SKIP {case}: {program} missing", flush=True)
            return
    with tempfile.TemporaryDirectory(prefix="rex-remote-", dir=os.environ.get("OLO_REX_TEST_DIR", "/var/tmp")) as scratch:
        base = Path(scratch)
        for sub in ("home/.ssh", "run", "state", "tmp", "rex", "etc"):
            (base / sub).mkdir(parents=True, mode=0o700)
        uid, gid = os.getuid() or 1000, os.getgid() or 1000
        (base / "etc/passwd").write_text(f"scratch:x:{uid}:{gid}:Fixture:/fixture/home:/bin/sh\nsshd:x:74:74:Privilege separation:/var/empty:/bin/false\n")
        (base / "etc/group").write_text(f"scratch:x:{gid}:\nsshd:x:74:\n")
        (base / "etc/hosts").write_text("127.0.0.1 localhost\n::1 localhost\n")
        (base / "etc/nsswitch.conf").write_text("passwd: files\ngroup: files\nshadow: files\nhosts: files\n")
        args = ["bwrap", "--die-with-parent", "--unshare-user", "--unshare-net", "--unshare-pid", "--unshare-ipc", "--new-session",
                "--ro-bind", "/usr", "/usr", "--ro-bind", "/bin", "/bin", "--ro-bind", "/lib", "/lib",
                "--ro-bind-try", "/lib64", "/lib64", "--dev", "/dev", "--proc", "/proc",
                "--bind", scratch, str(ROOT), "--bind", str(base / "etc"), "/etc",
                "--ro-bind-try", "/etc/ld.so.cache", "/etc/ld.so.cache",
                "--ro-bind-try", "/etc/crypto-policies", "/etc/crypto-policies",
                "--ro-bind-try", "/etc/ssl", "/etc/ssl", "--dir", "/run/sshd", "--dir", "/var/empty/sshd",
                "--bind", str(base / "run"), f"/run/user/{uid}", "--bind", str(base / "tmp"), "/tmp",
                "--ro-bind", binary, "/suite", "--ro-bind", str(Path(__file__).resolve()), "/fixture.py",
                "--clearenv", "--setenv", "PATH", "/usr/sbin:/usr/bin:/bin", "--setenv", "HOME", "/fixture/home",
                "--setenv", "USER", "scratch", "--setenv", "SHELL", "/bin/sh", "--setenv", "XDG_RUNTIME_DIR", "/fixture/run",
                "--setenv", "XDG_STATE_HOME", "/fixture/state", "--setenv", "REX_SERVER_SOCKET", str(SOCKET),
                "--chdir", "/fixture/home", "--", "/usr/bin/python3", "-I", "/fixture.py", "inside", case]
        if case == "remote_slow":
            args[1:1] = ["--uid", "0", "--gid", "0", "--cap-add", "CAP_NET_ADMIN", "--cap-add", "CAP_SETFCAP"]
        elif os.getuid() == 0 or os.getgid() == 0:
            args[1:1] = ["--uid", str(uid), "--gid", str(gid)]
        subprocess.run(args, timeout=185, check=True)


@contextlib.contextmanager
def service(args, log):
    with open(ROOT / log, "a") as output:
        child = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=output, stderr=output)
        try:
            yield child
        finally:
            if child.poll() is None:
                child.kill()
            child.wait()


def wait_ready(child, address, log):
    deadline = time.monotonic() + 5
    while child.poll() is None and time.monotonic() < deadline:
        with socket.socket(socket.AF_INET if isinstance(address, tuple) else socket.AF_UNIX) as probe:
            probe.settimeout(.5)
            if probe.connect_ex(address) == 0:
                return
        time.sleep(.02)
    raise RuntimeError(f"{log} failed to start: {(ROOT / log).read_text()}")


def key(path, password=""):
    subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", password, "-f", str(path)], check=True)


def tls_proxy():
    import asyncio
    import ssl

    async def relay(client, writer):
        remote, upstream = await asyncio.open_connection("127.0.0.1", 7297)
        async def copy(reader, output):
            while data := await reader.read(65536):
                output.write(data)
                await output.drain()
        tasks = [asyncio.create_task(copy(client, upstream)), asyncio.create_task(copy(remote, writer))]
        try:
            await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
        finally:
            for task in tasks:
                task.cancel()
            await asyncio.gather(*tasks, return_exceptions=True)
            upstream.close()
            writer.close()

    async def listen():
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain("/fixture/leaf.pem", "/fixture/leaf.key")
        server = await asyncio.start_server(relay, "127.0.0.1", 9443, ssl=context)
        await server.serve_forever()

    asyncio.run(listen())


def certificates():
    def openssl(*args):
        subprocess.run(["openssl", *args], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    openssl("req", "-config", "/dev/null", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
            "-subj", "/CN=Rex Fixture CA", "-addext", "basicConstraints=critical,CA:TRUE",
            "-keyout", "/fixture/ca.key", "-out", "/fixture/ca.pem")
    openssl("req", "-config", "/dev/null", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=localhost",
            "-keyout", "/fixture/leaf.key", "-out", "/fixture/leaf.req")
    (ROOT / "extensions").write_text("basicConstraints=CA:FALSE\nsubjectAltName=DNS:localhost\nextendedKeyUsage=serverAuth\n")
    openssl("x509", "-req", "-in", "/fixture/leaf.req", "-CA", "/fixture/ca.pem", "-CAkey", "/fixture/ca.key",
            "-days", "1", "-extfile", "/fixture/extensions", "-out", "/fixture/leaf.pem")


def inside(case):
    if case == "remote_slow" and os.getuid() == 0:
        # tc configures only this test's network namespace. Drop the capability
        # and map the original unprivileged uid before starting Rex or sshd.
        user = (ROOT / "etc/passwd").read_text().splitlines()[0].split(":")
        subprocess.run(["tc", "qdisc", "add", "dev", "lo", "root", "netem", "delay", "100ms", "loss", "2%"], check=True)
        os.execvp("bwrap", ["bwrap", "--unshare-user", "--uid", user[2], "--gid", user[3], "--cap-drop", "ALL",
                           "--bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--", "/usr/bin/python3", "-I", "/fixture.py", "inside", case])
    with contextlib.ExitStack() as resources:
        server = resources.enter_context(service(["rex", "-S", ADDR, "server", "run", "--listen", ADDR, "--state-dir", "/fixture/rex/state",
                                                 "--restore=false", "--extra-listen", "http://127.0.0.1:7297", "--allow-anonymous"], "rex.log"))
        wait_ready(server, str(SOCKET), "rex.log")
        (ROOT / "rex.pid").write_text(str(server.pid))
        if case == "https_carrier":
            certificates()
            os.environ["SSL_CERT_FILE"] = "/fixture/ca.pem"
            tls = resources.enter_context(service(["/usr/bin/python3", "-I", "/fixture.py", "tls"], "tls.log"))
            wait_ready(tls, ("127.0.0.1", 9443), "tls.log")
        if case.startswith(("ssh", "remote")):
            ssh = ROOT / "home/.ssh"
            key(ROOT / "host")
            key(ROOT / "identity")
            keys = (ROOT / "identity.pub").read_bytes()
            if case == "ssh_protected_key":
                key(ROOT / "protected", "test-passphrase")
                keys += (ROOT / "protected.pub").read_bytes()
            (ROOT / "authorized_keys").write_bytes(keys)
            # Identity and authentication databases exist only inside this mount namespace.
            (ROOT / "etc/shadow").write_text("scratch:*:19000:0:99999:7:::\n")
            if case.startswith("ssh_password") or case.startswith("ssh_interactive"):
                # openssl reads the fixture password on stdin; neither account file is the host's.
                hashed = subprocess.check_output(["/usr/bin/openssl", "passwd", "-6", "-stdin"], input=b"fixture-password\n").strip().decode()
                (ROOT / "etc/shadow").write_text(f"scratch:{hashed}:19000:0:99999:7:::\n")
                (ROOT / "etc/pam.d").mkdir()
                (ROOT / "etc/pam.d/sshd").write_text("auth required pam_unix.so nodelay\naccount required pam_permit.so\nsession required pam_permit.so\n")
            wrapper = ROOT / "dial"
            wrapper.write_text("#!/bin/sh\necho \"$SSH_ORIGINAL_COMMAND\" >> /fixture/dial-calls\ncase \"$SSH_ORIGINAL_COMMAND\" in\n"
                               "\"rex __internal dial-stdio\"|\"rex -S unix://'/fixture/rex/rex.sock' __internal dial-stdio\") ;;\n*) exit 1;; esac\n"
                               "export HOME=/fixture/home XDG_RUNTIME_DIR=/fixture/run XDG_STATE_HOME=/fixture/state REX_SERVER_SOCKET=/fixture/rex/rex.sock\n"
                               "exec /usr/bin/rex -S unix:///fixture/rex/rex.sock --autostart=false __internal dial-stdio\n")
            wrapper.chmod(0o700)
            (ssh / "config").write_text("Host fixture proxied\n HostName 127.0.0.1\n Port 2222\n User scratch\n IdentityFile /fixture/identity\n"
                                        "Host protected\n HostName 127.0.0.1\n Port 2222\n User scratch\n IdentityFile /fixture/protected\n"
                                        "Host proxied\n ProxyCommand /usr/bin/ncat %h %p\nHost *\n ConnectTimeout 2\n")
            (ROOT / "sshd.conf").write_text("Port 2222\nListenAddress 127.0.0.1\nHostKey /fixture/host\nAuthorizedKeysFile /fixture/authorized_keys\n"
                                           "PidFile /fixture/sshd.pid\nUsePAM no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n"
                                           "StrictModes no\nForceCommand /fixture/dial\n")
            if case.startswith(("ssh_password", "ssh_interactive")):
                conf = ROOT / "sshd.conf"
                settings = conf.read_text().replace("PasswordAuthentication no", "PasswordAuthentication yes") if case.startswith("ssh_password") else conf.read_text().replace("KbdInteractiveAuthentication no", "KbdInteractiveAuthentication yes").replace("UsePAM no", "UsePAM yes")
                conf.write_text(settings + "MaxAuthTries 10\n")
            if case == "remote_slow":
                conf = ssh / "config"
                conf.write_text(conf.read_text().replace("ConnectTimeout 2", "ConnectTimeout 30"))
            if case == "ssh_missing_rex":
                wrapper.write_text("#!/bin/sh\nexport PATH=/fixture/no-rex\nexec rex -S unix:///fixture/rex/rex.sock __internal dial-stdio\n")
            if case == "ssh_closes_after_auth":
                wrapper.write_text("#!/bin/sh\nexit 42\n")
            sshd = resources.enter_context(service([shutil.which("sshd"), "-D", "-e", "-f", "/fixture/sshd.conf"], "sshd.log"))
            wait_ready(sshd, ("127.0.0.1", 2222), "sshd.log")
            if case == "ssh_agent":
                agent = resources.enter_context(service(["ssh-agent", "-D", "-a", "/fixture/agent.sock"], "agent.log"))
                wait_ready(agent, "/fixture/agent.sock", "agent.log")
                subprocess.run(["ssh-add", "/fixture/identity"], env={**os.environ, "SSH_AUTH_SOCK": "/fixture/agent.sock"}, check=True)
        try:
            subprocess.run(["/suite", "--exact", case, "--nocapture"], check=True, timeout=170)
        except BaseException:
            for logfile in ["sshd.log", "rex.log"]:
                if (ROOT / logfile).exists():
                    print((ROOT / logfile).read_text()[-1800:], file=sys.stderr)
            raise


if __name__ == "__main__":
    if sys.argv[1] == "tls":
        tls_proxy()
    elif sys.argv[1] == "inside":
        inside(sys.argv[2])
    else:
        outer(*sys.argv[1:])
