from forms import *  # noqa: F401,F403
ICONS = {}
def icon(name, title):
    def deco(fn):
        ICONS[name] = (title, fn)
        return fn
    return deco
RIMMED = ' stroke="%s" stroke-width="2"' % RIM
def letter_tile(): return tile(PAPER) + [R(6, 6, 52, 52, 13, "none", extra=RIMMED)]
@icon("font", "Font")
def _(): return letter_tile() + [S("M14 46L24 16H26L36 46M17.5 37H32.5", G800, 4.6, join="miter"), ring(45, 40, 5.2, 3.4, BLUE), L(51.2, 33, 51.2, 46, BLUE, 3.4)]
@icon("trash-full", "Trash, full")
def _(): return [P("M15 14L20 3L30 8L27 16Z", G200, extra=RIMMED), P("M28 15L36 2L47 7L44 16Z", G300, extra=RIMMED), P("M21 14L26 6L36 9L34 16Z", PAPER, extra=RIMMED), R(9, 12, 46, 7, 3, G500), P("M13 21H51L48 54Q47.6 58 43.6 58H20.4Q16.4 58 16 54Z", G400), L(24, 27, 25, 50, G500, 2.6, "from24"), L(32, 27, 32, 50, G500, 2.6, "from24"), L(40, 27, 39, 50, G500, 2.6, "from24")]
@icon("suggestions", "Suggestions")
def _(): return [P("M24 26L26 4L44 7L40 26Z", PAPER, extra=RIMMED), R(8, 24, 48, 32, 4, BLUE), R(18, 22, 28, 5, 2.5, G800), poly([(32, 33), (34.6, 39), (41, 39.5), (36, 43.6), (37.7, 50), (32, 46.4), (26.3, 50), (28, 43.6), (23, 39.5), (29.4, 39)], YELLOW)]
@icon("installer", "Installer")
def _(): return [R(8, 30, 48, 28, 3, BROWN), R(4, 24, 56, 9, 3, BROWN_D), R(28, 24, 8, 16, 0, CREAM, "from24")] + arrow(32, 3, 32, 19, GREEN_D, 6.5, 8.5)
@icon("integrations", "Integrations")
def _(): return [P("M5 18Q5 14 9 14H19A7 7 0 1 1 33 14H43Q47 14 47 18V28A7 7 0 1 1 47 42V52Q47 56 43 56H33A7 7 0 1 0 19 56H9Q5 56 5 52Z", BLUE)]

# --- kept on the canvas (2 to 62, the thing itself) ------------------------
@icon("authentication", "Authentication")
def _(): return padlock(32, 41, 1.75, AMBER, G400, G800)
@icon("installer", "Installer")
def _(): return [R(8, 32, 48, 26, 3, BROWN), R(4, 26, 56, 9, 3, BROWN_D), R(28, 26, 8, 16, 0, CREAM, "from24")] + arrow(32, 6.5, 32, 21, GREEN_D, 6, 8)
@icon("key", "Key")
def _(): return [G([C(20, 32, 15, AMBER), C(17, 32, 5, WHITE), R(31, 28, 28, 8, 2, AMBER), R(45, 34, 6, 10, 1.5, AMBER), R(53, 34, 6, 7, 1.5, AMBER)], "rotate(-35 32 32)")]
@icon("sensors", "Sensors")
def _(): return [P("M26 10A6 6 0 0 1 38 10V34A12.5 12.5 0 1 1 26 34Z", PAPER, extra=RIMMED), C(32, 44.5, 8, RED), R(29.5, 20, 5, 24, 2.5, RED), L(43, 13, 49, 13, G400, 2.6, "from24"), L(43, 21, 47, 21, G400, 2.6, "from24"), L(43, 29, 49, 29, G400, 2.6, "from24")]
@icon("themes", "Themes")
def _():
    out = []
    for i, col in enumerate((RED, AMBER, GREEN, BLUE)):
        out.append('<rect x="25" y="12" width="14" height="40" rx="4" fill="%s" transform="rotate(%s 32 53)"/>' % (col, n(-36 + i * 24)))
    return out + [C(32, 52, 2.8, WHITE)]
@icon("trash-full", "Trash, full")
def _(): return [P("M15 16L20 6L30 10L27 18Z", G200, extra=RIMMED), P("M28 17L36 5L46 9.5L43.5 18Z", G300, extra=RIMMED), P("M21 16L26 9L36 11.5L34 18Z", PAPER, extra=RIMMED), R(9, 14, 46, 7, 3, G500), P("M13 23H51L48 55Q47.6 59 43.6 59H20.4Q16.4 59 16 55Z", G400), L(24, 29, 25, 51, G500, 2.6, "from24"), L(32, 29, 32, 51, G500, 2.6, "from24"), L(40, 29, 39, 51, G500, 2.6, "from24")]
@icon("video", "Video")
def _(): return [R(6, 24, 52, 34, 4, G800), G([R(6, 12, 49, 11, 2, G700)] + [poly([(10 + i * 11.5, 12), (16.5 + i * 11.5, 12), (12 + i * 11.5, 23), (5.5 + i * 11.5, 23)], PAPER) for i in range(1, 5)], "rotate(-8 6 24)"), poly([(27, 33), (27, 50), (41, 41.5)], WHITE)]
@icon("calendar", "Calendar")
def _():
    out = [R(7, 10, 50, 48, 7, PAPER, extra=RIMMED), P("M7 25V17A7 7 0 0 1 14 10H50A7 7 0 0 1 57 17V25Z", RED)]
    out += [R(18, 5, 5, 11, 2.5, G600, "from24"), R(41, 5, 5, 11, 2.5, G600, "from24")]
    for r in range(3):
        for c in range(4):
            out.append(R(12.5 + c * 10, 30 + r * 8.5, 7, 6, 1.5, BLUE if (r, c) == (1, 2) else G300, "from32"))
    for r in range(2):
        for c in range(2):
            out.append(R(13 + c * 20, 30 + r * 13, 18, 10, 2, BLUE if (r, c) == (1, 1) else G300, "upto24"))
    return out
