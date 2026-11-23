#!/usr/bin/python3
"""How a program in a tmux pane draws its caret, without its text (an agent's session shows the account it is
signed in to, which is nobody's to copy): the terminal's cursor as tmux has it (shown or hidden, where, on which
screen), and for the rows about it and at the pane's foot, each cell by its class: '.' blank, 'x' text, 'R' in
reverse video, 'B' on a background. What typing ahead has to find (apps/rex/src/predict/pane.rs, `screen_of`).

    caret.py SOCKET PANE      e.g. caret.py olo-agents opus-look1 (tools/dev/agents' tmux server and a session)"""
import re, subprocess, sys
sock, target = sys.argv[1], sys.argv[2]
def tm(*a):
    return subprocess.run(["tmux", "-L", sock, *a], capture_output=True, text=True).stdout
info = tm("display-message", "-p", "-t", target, "#{cursor_flag} #{cursor_x} #{cursor_y} #{pane_width} #{pane_height} #{alternate_on} #{cursor_shape}").split()
print("cursor shown:", info[0], "at x,y:", info[1], info[2], "pane:", info[3], "x", info[4], "alt screen:", info[5], "shape:", info[6:] )
raw = tm("capture-pane", "-e", "-p", "-t", target)
rows = raw.split("\n")
cy = int(info[2])
for y, line in enumerate(rows[: int(info[4])]):
    if abs(y - cy) > 6 and y < int(info[4]) - 9:
        continue
    out, rev, bg = [], False, False
    i = 0
    while i < len(line):
        m = re.match(r"\x1b\[([0-9;:]*)m", line[i:])
        if m:
            ps = [p for p in m.group(1).split(";")] or ["0"]
            k = 0
            while k < len(ps):
                p = ps[k] or "0"
                if p == "0": rev = bg = False
                elif p == "7": rev = True
                elif p == "27": rev = False
                elif p == "49": bg = False
                elif p in ("48",):
                    bg = True
                    k += 2 if (k + 1 < len(ps) and ps[k + 1] == "5") else 4
                elif p.isdigit() and (40 <= int(p) <= 47 or 100 <= int(p) <= 107): bg = True
                elif p in ("38",):
                    k += 2 if (k + 1 < len(ps) and ps[k + 1] == "5") else 4
                k += 1
            i += m.end()
            continue
        if line[i] == "\x1b":
            i += 1
            continue
        c = line[i]
        out.append("R" if rev else "B" if bg else "." if c == " " else "x")
        i += 1
    mark = "<- cursor row" if y == cy else ""
    print(f"{y:3} {''.join(out)[:int(info[3])]} {mark}")
