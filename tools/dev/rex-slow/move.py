#!/usr/bin/python3
# An account moved from one host to another, for real (Rex's move:
# crates/rex/tests/moving.rs): an Ubuntu 26.04 host to a Fedora 44 host,
# the rig's containers on a network of their own (farhost.py), each set up
# for Rex by its real setup code (the host test's setup, personal); the
# source with stand-in agents (stand-ins/agent.sh: Claude Code, Codex and
# agy keeping sessions as the real ones do, two Claude Codes in one folder)
# in a session with a shell, an agent pane each and a work list pane
# (`tail -f`), data enough to be sent in parts, a dotfile linked into the
# home, a folder of its own in /var/tmp and a second sign-in key; then the
# move as the app drives it, and what came out checked on the destination:
# every pane, each agent in the session it was in with the move's message at
# its prompt, the work list showing, /var/tmp's folder, the dotfile made
# right, the source stopped, the key taken away.
#
#   tools/dev/rex-slow/move.py [--keep]
#
# Needs the two images (fedora-host/, ubuntu-host/: `sudo docker build -f
# Containerfile -t olo-rig-fedora44 .` and `olo-rig-ubuntu2604`) and the
# helper built with `move` (`jato build -p olo-rex-link --features move`).
# Scratch in $REX_SLOW_DIR (/var/tmp/olo-rex-slow by default); the
# containers and their network are removed with it (--keep: kept till
# Enter, to look at).
import base64, os, shlex, shutil, subprocess, sys
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from farhost import D, FedoraHost, UbuntuHost, rig_network

ROOT = os.path.abspath(os.path.join(HERE, "../../.."))
HELPER = f"{ROOT}/target/debug/olo-rex-link"


def test_binary(name):
    out = subprocess.run(["jato", "test", "-p", "olo-rex-client", "--test", name, "--no-run"], cwd=ROOT, capture_output=True, text=True)
    words = (out.stdout + out.stderr).replace("(", " ").replace(")", " ").split()
    return ROOT + "/" + [w for w in words if f"/{name}-" in w and w.startswith("target/")][-1]


def set_up(host, binary):
    """Rex set up on `host` by its real setup code (the host test's), as an account's own server."""
    home = f"{D}/test-home-{host.KIND}"
    shutil.rmtree(home, ignore_errors=True)
    os.makedirs(f"{home}/.ssh", mode=0o700)
    shutil.copy(f"{host.dir}/known_hosts", f"{home}/.ssh/known_hosts")
    run = f"{D}/run"
    os.makedirs(run, mode=0o700, exist_ok=True)
    env = {"PATH": os.environ["PATH"], "HOME": home, "SSH_AUTH_SOCK": host.agent, "OLO_REX_TEST_HOST": host.address, "OLO_REX_TEST_SETUP": "personal", "XDG_RUNTIME_DIR": run}
    r = subprocess.run([binary, "a_host_is_set_up_from_nothing", "--ignored", "--nocapture"], env=env, capture_output=True, text=True, timeout=900)
    return r.returncode == 0, (r.stdout + r.stderr)[-600:]


def scenario(src):
    """The source as a person's: stand-in agents (two Claude Codes in one folder), projects and data enough to be sent in parts, a dotfile linked into the home, a folder of its own in /var/tmp, a second sign-in key, a work list, and a Rex session with a pane of each."""
    agent = open(f"{HERE}/stand-ins/agent.sh").read()
    key = "ssh-ed25519 " + base64.b64encode(b"\x00\x00\x00\x0bssh-ed25519\x00\x00\x00\x20" + os.urandom(32)).decode() + " laptop"
    script = f"""set -e
mkdir -p ~/.local/bin ~/proj-a ~/proj-b ~/data/a ~/data/b ~/dotfiles
cat > ~/.local/bin/olo-stand-in-agent <<'AGENT'
{agent}
AGENT
chmod 755 ~/.local/bin/olo-stand-in-agent
for a in claude codex agy; do ln -sf olo-stand-in-agent ~/.local/bin/$a; done
printf 'first thing\\nsecond thing\\n' > ~/todo.txt
echo 'print(1)' > ~/proj-a/main.py; echo 'fn main() {{}}' > ~/proj-b/main.rs
head -c 3000000 /dev/urandom > ~/data/a/blob1; head -c 3000000 /dev/urandom > ~/data/b/blob2; head -c 2000000 /dev/urandom > ~/proj-b/big.bin
printf '[core]\\n\\texcludesFile = %s/.gitignore_global\\n' "$HOME" > ~/dotfiles/gitconfig; ln -sf "$HOME/dotfiles/gitconfig" ~/.gitconfig
echo 'gone after pass 1' > ~/ephemeral.tmp
mkdir -p /var/tmp/olo-rig-scratch && echo 'scratch notes' > /var/tmp/olo-rig-scratch/notes.txt && echo 'gone vt' > /var/tmp/olo-rig-scratch/ephemeral.tmp
echo 'loose scratch' > /var/tmp/olo-rig-loose.txt
for g in {{00..49}}; do mkdir -p /var/tmp/olo-bulk-$g-{{00..99}}; done
echo '{key}' >> ~/.ssh/authorized_keys
export PATH="$HOME/.local/bin:$PATH"
rex new work --cwd ~ --window main -- bash -l > /dev/null
sleep 1
rex split -s work --cwd ~/proj-a --label claude-pane -- bash -lc 'exec claude --dangerously-skip-permissions' > /dev/null
rex split -s work --cwd ~/proj-b --label codex-pane -- bash -lc 'exec codex --yolo' > /dev/null
rex split -s work --cwd ~/proj-a --label agy-pane -- bash -lc 'exec agy' > /dev/null
rex split -s work --cwd ~/proj-a --label claude-pane-2 -- bash -lc 'exec claude --model opus' > /dev/null
rex split -s work --cwd ~ --label todo -- tail -f ~/todo.txt > /dev/null
sleep 2
rex block ls -s work 2> /dev/null | awk 'NR > 1 {{print $2, $3, $4}}'
"""
    return src.ran(f"bash -lc {shlex.quote(script)}", timeout=120)


if __name__ == "__main__":
    setup_bin, move_bin = test_binary("host"), test_binary("moving")
    with rig_network() as net, UbuntuHost(network=net) as src, FedoraHost(network=net) as dst:
        for h in (src, dst):
            ok, said = set_up(h, setup_bin)
            print(f"{h.KIND} set up for Rex:", ok, "" if ok else said, flush=True)
            if not ok:
                sys.exit(1)
        print("the source's panes:", scenario(src).replace("\n", " | "), flush=True)
        dst.ran("echo 'stay put' > ~/dest-preexisting.txt", timeout=30)
        home = f"{D}/move-home"
        shutil.rmtree(home, ignore_errors=True)
        os.makedirs(f"{home}/.ssh", mode=0o700)
        with open(f"{home}/.ssh/known_hosts", "w") as k:
            for h in (src, dst):
                k.write(open(f"{h.dir}/known_hosts").read())
        agent = f"{D}/move-agent.sock"
        if os.path.exists(agent):
            os.remove(agent)
        keeper = subprocess.Popen(["ssh-agent", "-D", "-a", agent], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        try:
            for _ in range(50):
                if os.path.exists(agent):
                    break
                subprocess.run(["sleep", "0.1"])
            for h in (src, dst):
                subprocess.run(["ssh-add", "-q", f"{h.dir}/client_key"], env=dict(os.environ, SSH_AUTH_SOCK=agent), check=True)
            run = f"{D}/move-run"
            os.makedirs(run, mode=0o700, exist_ok=True)
            env = {
                "PATH": os.environ["PATH"], "HOME": home, "SSH_AUTH_SOCK": agent, "XDG_RUNTIME_DIR": run,
                "OLO_REX_TEST_HOST": src.address, "OLO_REX_TEST_DEST": dst.address,
                "OLO_REX_TEST_PEER": f"{dst.USER}@{dst.name}:22", "OLO_REX_TEST_HELPER_FILE": HELPER,
                # (A small home sent in parts all the same, a mebibyte's
                # share each.)
                "OLO_REX_TEST_MOVE_EACH": "1048576",
            }
            r = subprocess.run([move_bin, "an_account_moves_whole", "--ignored", "--nocapture"], env=env, capture_output=True, text=True, timeout=1500)
            print((r.stdout + r.stderr)[-8000:], flush=True)
            print("the move:", "passed" if r.returncode == 0 else f"failed ({r.returncode})", flush=True)
            if "--keep" in sys.argv:
                print({"source": src.address, "destination": dst.address}, flush=True)
                sys.stdin.read()
            sys.exit(r.returncode)
        finally:
            keeper.terminate()
