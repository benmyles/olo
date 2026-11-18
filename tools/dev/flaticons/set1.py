from forms import *  # noqa: F401,F403
ICONS = {}
def icon(name, title):
    def deco(fn):
        ICONS[name] = (title, fn)
        return fn
    return deco

@icon("folder", "Folder")
def _(): return folder() + [R(6, 22, 52, 3, 0, YELLOW_L, "from32")]
@icon("folder-open", "Folder, open")
def _(): return [P("M6 14 Q6 10 10 10 H24 Q26 10 27.5 11.5 L30 14.5 Q31 16 33 16 H54 Q58 16 58 20 V50 Q58 54 54 54 H10 Q6 54 6 50Z", AMBER), R(12, 18, 40, 30, 2, PAPER), P("M2 30 Q1.4 26 5.4 26 H54.6 Q58.6 26 58 30 L55.4 50.4 Q55 54 51 54 H9 Q5 54 4.6 50.4Z" .replace("M2 30 Q1.4 26 5.4 26 H54.6 Q58.6 26 58 30", "M3 30 Q2.5 26 6.5 26 H57.5 Q61.5 26 61 30"), YELLOW)]
@icon("document", "Document")
def _(): return page() + lines(20, 24, 24, 4)
@icon("text", "Text")
def _(): return page() + [L(20, 18, 34, 18, BLUE, 3.4)] + lines(20, 27, 24, 4, 6.5)
@icon("new-document", "New document")
def _(): return page() + lines(20, 24, 24, 2) + badge(46, 46, 11, GREEN) + [plus(46, 46, 5.2)]
@icon("save-document", "Save document")
def _(): return page() + lines(20, 22, 24, 2) + badge(46, 46, 11, BLUE) + arrow(46, 39.5, 46, 51, WHITE, 3.2, 5)
@icon("copy", "Copy")
def _(): return page(8, 4, 32, 44, 9) + page(24, 16, 32, 44, 9) + lines(30, 32, 20, 3, 6.5)
@icon("paste", "Paste")
def _(): return [R(9, 9, 46, 50, 5, BROWN), R(15, 16, 34, 38, 2, PAPER), R(22, 4, 20, 11, 3, G500), R(27, 7, 10, 4, 2, G300, "from32")] + lines(21, 25, 22, 4, 6.5)
@icon("terminal", "Terminal")
def _(): return [R(5, 8, 54, 48, 7, G800), R(5, 8, 54, 10, 7, G700), R(5, 14, 54, 4, 0, G700), C(11.5, 13, 1.8, RED, "from32"), C(17, 13, 1.8, YELLOW, "from32"), C(22.5, 13, 1.8, GREEN, "from32"), S("M14 28L22 35L14 42", GREEN, 4), L(27, 43, 38, 43, G200, 4)]
@icon("display", "Display")
def _(): return monitor(SKY) + [poly([(8, 45), (24, 27), (36, 39), (44, 32), (56, 45)], BLUE, "from24")]
@icon("desktop", "Desktop")
def _(): return monitor(BLUE) + [R(11, 15, 5, 5, 1, YELLOW, "from24"), R(11, 23, 5, 5, 1, WHITE, "from24"), R(40, 15, 13, 14, 1, PAPER, "from24"), R(40, 15, 13, 3, 1, YELLOW, "from24")]
@icon("deskbar", "Deskbar")
def _(): return monitor(SKY) + [R(41, 12, 15, 22, 2, PAPER), R(41, 12, 15, 5, 2, YELLOW), L(44, 21.5, 53, 21.5, G400, 2, "from24"), L(44, 25.5, 53, 25.5, G400, 2, "from24"), L(44, 29.5, 50, 29.5, G400, 2, "from24")]
@icon("lock-screen", "Lock screen")
def _(): return monitor(G700, G800) + padlock(32, 31, 0.95)
@icon("screensaver", "Screen saver")
def _(): return monitor(G800, G700) + moon(27, 28, 9) + [C(42, 20, 1.6, WHITE, "from24"), C(47, 31, 1.3, WHITE, "from24"), C(39, 37, 1.1, WHITE, "from32")]
@icon("home", "Home")
def _(): return [R(42, 10, 6, 14, 1, BROWN_D), R(13, 28, 38, 28, 2, CREAM), poly([(32, 6), (59, 31), (5, 31)], RED), R(27, 39, 10, 17, 1.5, BROWN), R(17, 36, 7, 7, 1, SKY, "from24"), R(40, 36, 7, 7, 1, SKY, "from24")]
@icon("mail", "Mail")
def _(): return [R(5, 13, 54, 38, 5, PAPER, extra=' stroke="%s" stroke-width="2"' % RIM), P("M7 16L32 36L57 16", "none", extra=' stroke="%s" stroke-width="3.2" stroke-linecap="round" stroke-linejoin="round"' % BLUE), L(8, 48, 23, 34, G300, 2.4, "from32"), L(56, 48, 41, 34, G300, 2.4, "from32")]
@icon("trash-empty", "Trash, empty")
def _(): return [R(24, 5, 16, 6, 2, G500), R(9, 9, 46, 7, 3, G500), P("M13 18H51L48 54Q47.6 58 43.6 58H20.4Q16.4 58 16 54Z", G400), L(24, 25, 25, 50, G500, 2.6, "from24"), L(32, 25, 32, 50, G500, 2.6, "from24"), L(40, 25, 39, 50, G500, 2.6, "from24")]
@icon("trash-full", "Trash, full")
def _(): return [poly([(15, 14), (20, 3), (30, 8), (27, 16)], PAPER), poly([(28, 15), (36, 2), (47, 7), (44, 16)], G200), poly([(21, 14), (26, 6), (36, 9), (34, 16)], WHITE), R(9, 12, 46, 7, 3, G500), P("M13 21H51L48 54Q47.6 58 43.6 58H20.4Q16.4 58 16 54Z", G400), L(24, 27, 25, 50, G500, 2.6, "from24"), L(32, 27, 32, 50, G500, 2.6, "from24"), L(40, 27, 39, 50, G500, 2.6, "from24")]
@icon("calculator", "Calculator")
def _():
    out = [R(11, 4, 42, 56, 6, G600), R(16, 9, 32, 12, 2, GREEN_L)]
    for r in range(4):
        for c in range(4):
            col = ORANGE if c == 3 else (G300 if r else G400)
            out.append(R(16 + c * 8.4, 25 + r * 7.8, 6.6, 6, 1.5, col, "from24"))
    return out
@icon("image", "Picture")
def _(): return [R(5, 9, 54, 46, 5, PAPER, extra=' stroke="%s" stroke-width="2"' % RIM), R(10, 14, 44, 36, 2, SKY), C(20, 24, 5, YELLOW), poly([(10, 50), (26, 30), (37, 43), (44, 36), (54, 48), (54, 50)], GREEN_D), poly([(26, 30), (37, 43), (31, 50), (10, 50)], GREEN, "from24")]
@icon("tracker", "Tracker")
def _(): return folder() + magnifier(38, 36, 11)
@icon("preferences", "Preferences")
def _():
    out = tile(G600)
    for i, (x, col) in enumerate(((24, SKY), (40, GREEN_L), (30, YELLOW))):
        y = 20 + i * 12
        out += [L(15, y, 49, y, G400, 3.2), C(x, y, 5, col)]
    return out
@icon("about", "About")
def _(): return disc(BLUE) + [C(32, 19, 4, WHITE), R(28.5, 27, 7, 21, 3.5, WHITE)]
@icon("info", "Information")
def _(): return tile(SKY) + [C(32, 20, 4, WHITE), R(28.5, 28, 7, 20, 3.5, WHITE)]
@icon("warning", "Warning")
def _(): return [P("M28.5 8Q32 2 35.5 8L60 50Q63 56 56.5 56H7.5Q1 56 4 50Z".replace("M28.5 8Q32 2 35.5 8L60 50Q63 56 56.5 56H7.5Q1 56 4 50Z", "M28.2 9.5Q32 3 35.8 9.5L58.6 48.5Q62 55 55 55H9Q2 55 5.4 48.5Z"), YELLOW), R(29, 21, 6, 18, 3, G800), C(32, 46, 3.6, G800)]
@icon("stop", "Stop")
def _(): return [poly([(21, 5), (43, 5), (59, 21), (59, 43), (43, 59), (21, 59), (5, 43), (5, 21)], RED), R(29, 16, 6, 20, 3, WHITE), C(32, 44, 3.8, WHITE)]
@icon("disk", "Disk")
def _(): return [R(6, 14, 52, 36, 6, G400), R(6, 36, 52, 14, 6, G500), R(6, 36, 52, 6, 0, G500), C(47, 43, 3, GREEN_L), L(13, 43, 30, 43, G300, 3, "from24"), C(24, 25, 7.5, G200, "from24"), C(24, 25, 2.2, G500, "from32")]
@icon("printer", "Printer")
def _(): return [R(16, 5, 32, 16, 2, PAPER, extra=' stroke="%s" stroke-width="2"' % RIM), R(5, 19, 54, 26, 5, G500), R(14, 36, 36, 22, 2, PAPER, extra=' stroke="%s" stroke-width="2"' % RIM), C(49, 26, 2.4, GREEN_L)] + lines(20, 43, 24, 2, 6, G400, 2.6)
@icon("camera", "Camera")
def _(): return [R(22, 9, 20, 8, 3, G600), R(5, 14, 54, 40, 7, G700), C(32, 34, 13, G300), C(32, 34, 9, G800), C(32, 34, 4.5, SKY), C(49, 22, 2.5, RED_L, "from24")]
@icon("key", "Key")
def _(): return [G([C(20, 32, 13, AMBER), C(17, 32, 4.4, WHITE), R(30, 28.5, 30, 7, 2, AMBER), R(47, 34, 5, 9, 1.5, AMBER), R(55, 34, 5, 6, 1.5, AMBER)], "rotate(-35 32 32)")]
@icon("notifications", "Notifications")
def _(): return [P("M32 6Q46 6 46 22V34Q46 40 52 45Q54 48 50 48H14Q10 48 12 45Q18 40 18 34V22Q18 6 32 6Z", AMBER), R(28, 2.5, 8, 7, 3.5, AMBER), P("M25 51H39Q39 58.5 32 58.5Q25 58.5 25 51Z", ORANGE)]
