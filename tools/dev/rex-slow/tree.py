#!/usr/bin/python3
"""Rex's demo, driven as a person drives it, through every way to act on a child tab that asks the hub
something from the window's own thread: a child chosen from a tab's tree menu, the path above a child
clicked, and Keep It Going and Auto Approve chosen from a row's menu in Activity: a child's, and the row of
a tab's own agent above it (Ben, 2026-11-27: "i should be able to right click any agent in the activity list
to toggle keep going and auto approve"; until then only a child's row had them). After each, the window
must show what was asked for: the child, its parent again, the item ticked in a menu opened anew. (Each is
drawn by the window's own thread. That its tree can still be read says nothing: a window that has stopped
is read as it last was.)

    tree.py [BIN]      BIN: another build of olo-rex to try (to compare); else this tree's dev build
                       (REX_TREE_TARGET: where this tree's builds are, when not in its target/)

Each of these stopped the window for good until 2026-11-26: the hub was kept inside the app's locked
state, `app().hub.select_window(…)` held that lock for the whole statement, and the hub's notice (told at
once, on the asking thread) took it again. A dev build says so by itself (the lock's own check panics);
a release build just stops, which is what this reads: what was asked for never shows.
"""
import json, os, socket, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
NAME = os.environ.get("REX_TREE_NAME", "rex-tree")
SCRATCH = os.environ.get("REX_TREE_DIR", "/var/tmp/olo-rex-tree")
# (The session's own programs from the tree's builds, whatever target folder the asker has; or from
# REX_TREE_TARGET, where the tree's builds are kept elsewhere: a worktree's, by Jato.)
TARGET = os.environ.get("REX_TREE_TARGET", f"{ROOT}/target")
BIN = sys.argv[1] if len(sys.argv) > 1 else f"{TARGET}/debug/olo-rex"
ENV = {**os.environ, "CARGO_TARGET_DIR": TARGET}
ok = True


def ts(*a):
    return subprocess.run(["/usr/bin/python3", f"{ROOT}/tools/dev/tsession", *a], capture_output=True, text=True, env=ENV).stdout


def check(what, passed, shown):
    global ok
    ok = ok and bool(passed)
    print(("ok   " if passed else "FAIL ") + what + ":", repr(shown)[:260], flush=True)


def windows():
    try:
        return json.loads(ts("ipc", NAME, '{"type":"list_windows"}') or "{}").get("windows", [])
    except ValueError:
        return []


def nodes(pid, within=3.0):
    """What agents read of the app's windows; None if it doesn't answer in time (it has stopped)."""
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(within)
    try:
        s.connect(f"/run/user/{os.getuid()}/olo-menus-{pid}.sock")
        s.sendall(b'{"tree":true}\n')
        tree = json.loads(s.makefile().readline() or "[]")
    except (OSError, ValueError):
        return None
    finally:
        s.close()
    out = []

    def walk(v):
        if isinstance(v, dict):
            if "role" in v:
                out.append(v)
            for x in v.values():
                walk(x)
        elif isinstance(v, list):
            for x in v:
                walk(x)

    walk(tree)
    return out


def wait(what, within=8.0, every=0.1):
    until = time.monotonic() + within
    while time.monotonic() < until:
        got = what()
        if got:
            return got
        time.sleep(every)
    return None


def named(found, role, name):
    """The node of that name (and role; None: any)."""
    return next((n for n in found or [] if role in (None, n.get("role")) and str(n.get("name") or "") == name), None)


def crumb(found):
    """The path above a child (its parent's button), there only while a child is the tab shown."""
    return next((n for n in found or [] if n.get("role") == "Button" and str(n.get("name") or "").startswith("Switch to")), None)


def main():
    os.makedirs(f"{SCRATCH}/home", exist_ok=True)
    ts("stop", NAME)
    ts("start", NAME, "1280x800", "--light", "--scale", "1", "--home", f"{SCRATCH}/home", "--owner", "none")
    try:
        ts("run", NAME, "OLO_REX_DEMO=1", "RUST_BACKTRACE=1", BIN)
        # (Listed before it has a size: a click then is on nothing.)
        win = wait(lambda: next((w for w in windows() if w.get("title") == "Rex" and w["content"]["width"] > 0), None), 20)
        if not win:
            return check("Rex's window", False, [w.get("title") for w in windows()])
        pid = win["pid"]
        if not wait(lambda: named(nodes(pid), None, "Activity"), 10):
            return check("Rex's window is drawn", False, sorted({str(n.get("name")) for n in nodes(pid) or [] if n.get("name")})[:30])
        at = next(w for w in windows() if w["id"] == win["id"])["content"]

        def click(x, y, *more):
            ts("click", NAME, str(int(at["x"] + x)), str(int(at["y"] + y)), *more)

        def centre(n):
            return n["x"] + n["width"] / 2, n["y"] + n["height"] / 2

        def chosen(menu_at, item):
            """A menu's item clicked: its place is the menu's own, the menu opened at `menu_at`."""
            items = wait(lambda: (lambda f: f if named(f, "MenuItem", item) else None)(nodes(pid)), 5)
            it = named(items, "MenuItem", item)
            if not it:
                return [str(n.get("name")) for n in items or [] if n.get("role") == "MenuItem"]
            click(menu_at[0] + it["x"] + 20, menu_at[1] + it["y"] + it["height"] / 2)
            return None

        def row_menu_at(row, spot):
            """Where a row's menu in Activity opens, right-clicked at `spot`: under the row, at the
            pointer's x, 12 points in at least. (Clicked as if it opened at the pointer, each choice
            fell on the item below the one meant.)"""
            return max(spot[0], row["x"] + 12), row["y"] + row["height"] + 2

        # 1. A child chosen from its tab's tree menu.
        found = nodes(pid)
        tree_btn = next((n for n in found or [] if n.get("role") == "Button" and str(n.get("name") or "").startswith("Child tabs of")), None)
        if not tree_btn:
            return check("a tab with children has its tree button", False, [n.get("name") for n in found or [] if n.get("role") == "Button"][:20])
        click(*centre(tree_btn))
        # (The menu opens under the tab strip, at the button's left edge.)
        missing = chosen((tree_btn["x"], tree_btn["y"] + tree_btn["height"]), "API Audit")
        after = wait(lambda: (lambda f: f if crumb(f) else None)(nodes(pid)), 6)
        check("a child chosen from the tree menu: the window shows the child", after and not missing, {"path above it": (crumb(after) or {}).get("name"), "menu lacked": missing})
        if not after:
            return

        # 2. The path above the child: back to its parent.
        click(*centre(crumb(after)))
        back = wait(lambda: (lambda f: f if f is not None and not crumb(f) else None)(nodes(pid)), 6)
        check("the path above a child clicked: the window shows the parent", back is not None, {"path above it": (crumb(nodes(pid)) or {}).get("name")})
        if back is None:
            return

        # 3 to 6. Keep It Going and Auto Approve, from a row's own menu in Activity: a child's, then the row of
        # the tab's own agent (the demo's first tab).
        click(*centre(named(nodes(pid), None, "Activity")))
        if not wait(lambda: named(nodes(pid), "ListItem", "API Audit"), 6):
            return check("Activity lists the children", False, [n.get("name") for n in nodes(pid) or [] if n.get("role") == "ListItem"][:12])
        for who, name in (("a child", "API Audit"), ("a tab's own agent", "olo-rex")):
            row = named(nodes(pid), "ListItem", name)
            if not row:
                return check(f"Activity lists {who} ({name})", False, [n.get("name") for n in nodes(pid) or [] if n.get("role") == "ListItem"][:12])

            def ticked(item):
                """The row's menu opened again (the window's own thread draws it): whether `item` is ticked there."""
                spot = centre(row)
                click(*spot, "right")
                it = wait(lambda: named(nodes(pid), "MenuItem", item), 5)
                ts("key", NAME, "1")  # (Escape, by its key code)
                wait(lambda: (lambda f: f is not None and not named(f, "MenuItem", item))(nodes(pid)), 3)
                return it and bool(it.get("checked"))

            for item in ("Keep It Going", "Auto Approve"):
                was = ticked(item)
                spot = centre(row)
                click(*spot, "right")
                missing = chosen(row_menu_at(row, spot), item)
                time.sleep(0.4)
                now = ticked(item)
                check(f"{item} chosen for {who} in Activity: its menu, opened again, has it ticked", was is False and now is True and not missing, {"ticked before": was, "after": now, "menu lacked": missing})
                if now is None:
                    return
            # Chosen again: off again (each row's own, and the other row's untouched).
            spot = centre(row)
            click(*spot, "right")
            chosen(row_menu_at(row, spot), "Keep It Going")
            time.sleep(0.4)
            check(f"Keep It Going chosen again for {who}: off again", ticked("Keep It Going") is False, None)
        # (The child's Auto Approve is still on, the parent's too; the parent's Keep It Going did not turn the child's on.)
        # Each agent's menu in Activity, the tab's own agent's and a child's, in one order (2026-11-28):
        # the view shown; what to do with it, Open in Tracker (its folder) and what folds under it; its
        # two settings; what stops it. Open in Tracker offered: the demo's agents are this computer's.
        for who, expected in (
            ("olo-rex", ["Show Transcript", "Show Terminal", "Open in Tracker", "Collapse Subagents", "Keep It Going", "Auto Approve", "Stop 2 Subagents…"]),
            ("API Audit", ["Show Transcript", "Show Terminal", "Open in Tracker", "Collapse Subagents", "Keep It Going", "Auto Approve", "Stop Subagent", "Stop Subagent & Children", "Force Close Tab"]),
        ):
            row = named(nodes(pid), "ListItem", who)
            spot = centre(row)
            click(*spot, "right")
            items = wait(lambda: (lambda f: f if named(f, "MenuItem", "Keep It Going") else None)(nodes(pid)), 5)
            ts("key", NAME, "1")
            menu = [n for n in items or [] if n.get("role") == "MenuItem"]
            had = [str(n.get("name")) for n in menu]
            check(f"{who}'s menu in Activity, in one order: what shows, what to do with it, the two, what stops", had == expected, had)
            tracker = named(menu, "MenuItem", "Open in Tracker")
            check(f"{who}'s Open in Tracker offered", bool(tracker and tracker.get("enabled")), tracker and {k: tracker.get(k) for k in ("name", "enabled")})
    finally:
        log = f"/var/tmp/olo-dev/{NAME}/olo-rex.log"
        if not ok and os.path.exists(log):
            said = [l for l in open(log, errors="replace").read().splitlines() if "panicked" in l or "cycle" in l.lower() or "ERROR" in l]
            print("the app's log:", *said[-6:], sep="\n  ", flush=True)
        ts("stop", NAME)


main()
print("ALL OK" if ok else "SOMETHING FAILED")
sys.exit(0 if ok else 1)
