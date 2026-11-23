#!/usr/bin/python3
"""Stand-ins for the programs people type into, to try typing ahead of a slow host against. Not those programs:
only how each draws its input, which is what matters to a terminal.

    tui.py box     as Claude Code draws: the cursor hidden, a rounded box, the caret a cell in reverse video, a dim
                   hint where nothing is typed, the whole of it drawn again at each key inside a synchronized
                   update, a line of status under it, and a spinner with lines of "work" appearing above while it
                   "thinks" (after Enter), so the box moves
    tui.py line    as agy and Codex draw: the terminal's own cursor in a composer between two rules, a dim hint
                   right of it where nothing is typed, a status line under it
    tui.py box --no-sync   the same without synchronized updates, drawn in two writes (a piece, then the rest)

Keys: letters, Backspace, left and right, Enter (sends: the text goes above, the spinner runs a while), / at the
start opens a little menu under the box, Ctrl-D ends."""
import os, select, shutil, signal, sys, termios, time, tty

MODE = sys.argv[1] if len(sys.argv) > 1 else "box"
SYNC = "--no-sync" not in sys.argv
HINT = "Try \"fix the failing test\"" if MODE == "box" else "Type your message"
out = sys.stdout

def write(s):
    out.write(s); out.flush()

class App:
    def __init__(self):
        self.text = []; self.col = 0; self.drawn = 0; self.busy_until = 0.0; self.spin = 0; self.log = []; self.menu = False
    def width(self):
        return max(30, shutil.get_terminal_size((100, 30)).columns)
    def lines(self):
        w = self.width(); rows = []
        if time.monotonic() < self.busy_until:
            rows.append(f"\x1b[35m{'⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏'[self.spin % 10]}\x1b[0m Thinking… \x1b[2m(esc to interrupt)\x1b[0m")
            rows.append("")
        text = "".join(self.text)
        if MODE == "box":
            if text or self.col:
                before, at, after = text[:self.col], (text[self.col] if self.col < len(text) else " "), text[self.col + 1:]
                body = f"{before}\x1b[7m{at}\x1b[27m{after}"
                used = len(text) + (1 if self.col >= len(text) else 0)
            else:
                body = f"\x1b[7m{HINT[0]}\x1b[27m\x1b[2m{HINT[1:]}\x1b[22m"; used = len(HINT)
            rows.append("\x1b[2m╭" + "─" * (w - 2) + "╮\x1b[22m")
            rows.append("\x1b[2m│\x1b[22m > " + body + " " * max(0, w - 5 - used) + "\x1b[2m│\x1b[22m")
            rows.append("\x1b[2m╰" + "─" * (w - 2) + "╯\x1b[22m")
            if self.menu:
                rows += ["  \x1b[36m/help\x1b[0m    Show what can be asked", "  \x1b[2m/clear\x1b[0m   Start again", "  \x1b[2m/model\x1b[0m   Choose a model"]
            else:
                rows.append("  \x1b[2m? for shortcuts\x1b[0m" + " " * max(1, w - 40) + "\x1b[2mauto-accept edits on\x1b[0m")
        else:
            rows.append("\x1b[2m" + "─" * w + "\x1b[22m")
            rows.append("\x1b[1;36m>\x1b[0m " + (text if text else f"\x1b[2m{HINT}\x1b[22m"))
            rows.append("\x1b[2m" + "─" * w + "\x1b[22m")
            rows.append("\x1b[2m~/project  model: fast  ctx 12%\x1b[22m")
        return rows
    def draw(self, above=()):
        rows = self.lines()
        up = f"\x1b[{self.drawn - 1}A" if self.drawn > 1 else ""
        head = "\r" + up + "\x1b[J" + "".join(l + "\r\n" for l in above)
        body = "\r\n".join(rows)
        if MODE == "line":
            # the terminal's own cursor, put back in the composer
            row_of_input = 1 + (2 if time.monotonic() < self.busy_until else 0)
            back = len(rows) - 1 - row_of_input
            tail = (f"\x1b[{back}A" if back else "") + f"\r\x1b[{2 + self.col}C"
            self.drawn = row_of_input + 1
        else:
            tail = ""; self.drawn = len(rows)
        if SYNC:
            write("\x1b[?2026h" + head + body + tail + "\x1b[?2026l")
        else:
            cut = len(body) // 2
            write(head + body[:cut]); time.sleep(0.004); write(body[cut:] + tail)
    def key(self, data):
        above = []
        i = 0
        while i < len(data):
            c = data[i]
            if data[i:i + 3] in ("\x1b[D", "\x1bOD"):
                self.col = max(0, self.col - 1); i += 3; continue
            if data[i:i + 3] in ("\x1b[C", "\x1bOC"):
                self.col = min(len(self.text), self.col + 1); i += 3; continue
            if c == "\x1b":
                i += 1
                while i < len(data) and not data[i].isalpha() and data[i] != "~": i += 1
                i += 1; continue
            if c == "\x04": return None
            if c in "\r\n":
                if self.text:
                    above = ["\x1b[2m>\x1b[22m " + "".join(self.text), ""]
                    self.busy_until = time.monotonic() + 4.0
                self.text = []; self.col = 0; self.menu = False
            elif c in "\x7f\x08":
                if self.col: self.col -= 1; del self.text[self.col]
            elif c >= " ":
                self.text.insert(self.col, c); self.col += 1
            self.menu = MODE == "box" and self.text[:1] == ["/"]
            i += 1
        return above

def main():
    fd = sys.stdin.fileno(); old = termios.tcgetattr(fd)
    tty.setraw(fd)
    app = App()
    write("\x1b[?2004h" + ("\x1b[?25l" if MODE == "box" else ""))
    def whole(*_):
        # (All of it anew, as these programs do when their window's size changes.)
        write("\x1b[2J\x1b[H\x1b[1mA stand-in for a program you type into\x1b[0m (" + ("a box with a caret it draws" if MODE == "box" else "a composer with the terminal's cursor") + ")\r\n\r\n")
        app.drawn = 0
        app.draw()
    signal.signal(signal.SIGWINCH, whole)
    whole()
    try:
        while True:
            busy = time.monotonic() < app.busy_until
            try:
                ready, _, _ = select.select([fd], [], [], 0.1 if busy else None)
            except InterruptedError:
                continue
            if ready:
                data = os.read(fd, 4096).decode("utf-8", "replace")
                above = app.key(data)
                if above is None: break
                app.draw(above)
            elif busy:
                app.spin += 1
                extra = [f"\x1b[32m●\x1b[0m Read \x1b[1msrc/file_{app.spin // 7}.rs\x1b[0m \x1b[2m({40 + app.spin} lines)\x1b[0m"] if app.spin % 7 == 0 else []
                app.draw(extra)
            else:
                app.draw()
    finally:
        write("\x1b[?2004l\x1b[?25h\r\n")
        termios.tcsetattr(fd, termios.TCSADRAIN, old)

main()
