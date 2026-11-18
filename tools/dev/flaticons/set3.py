from forms import *  # noqa: F401,F403
ICONS = {}
def icon(name, title):
    def deco(fn):
        ICONS[name] = (title, fn)
        return fn
    return deco
RIMMED = ' stroke="%s" stroke-width="2"' % RIM

@icon("application", "Application")
def _(): return [R(5, 8, 54, 48, 7, PAPER, extra=RIMMED), R(5, 8, 54, 13, 7, BLUE), R(5, 15, 54, 6, 0, BLUE), C(12.5, 14.5, 2.2, WHITE, "from24"), C(19, 14.5, 2.2, BLUE_L, "from24"), R(12, 28, 18, 21, 2, BLUE_L), L(35, 31, 52, 31, G400, 3, "from24"), L(35, 38.5, 52, 38.5, G400, 3, "from24"), L(35, 46, 46, 46, G400, 3, "from24")]
@icon("windows", "Windows")
def _(): return [R(4, 6, 40, 32, 5, G300), R(4, 6, 40, 9, 5, G400), R(4, 11, 40, 4, 0, G400), R(18, 22, 42, 36, 5, PAPER, extra=RIMMED), R(18, 22, 42, 10, 5, YELLOW), R(18, 28, 42, 4, 0, YELLOW)] + lines(25, 40, 26, 2, 7, G400, 3)
@icon("workspaces", "Workspaces")
def _(): return [R(5, 7, 25, 23, 4, BLUE), R(34, 7, 25, 23, 4, GREEN), R(5, 34, 25, 23, 4, AMBER), R(34, 34, 25, 23, 4, PURPLE), R(9, 11, 9, 7, 1.5, WHITE, "from32"), R(38, 11, 9, 7, 1.5, WHITE, "from32"), R(9, 38, 9, 7, 1.5, WHITE, "from32"), R(38, 38, 9, 7, 1.5, WHITE, "from32")]
@icon("default-apps", "Default apps")
def _(): return [R(5, 5, 25, 25, 6, BLUE), R(34, 5, 25, 25, 6, ORANGE), R(5, 34, 25, 25, 6, PURPLE), R(34, 34, 25, 25, 6, GREEN), check(46.5, 47, 6.5, WHITE, 4)]
@icon("other-applications", "Other applications")
def _(): return tile(G200) + [C(22, 22, 8, RED), R(35, 14, 15, 15, 3, BLUE), poly([(22, 35), (31, 50), (13, 50)], GREEN), C(42.5, 42.5, 3, G500), C(42.5, 42.5, 8, "none", extra=' stroke="%s" stroke-width="3"' % G500)]
@icon("appearance", "Appearance")
def _(): return [P("M32 6Q58 6 58 28Q58 40 46 40H42Q37 40 37 45Q37 47 38.5 49Q41 53 37 56Q35 58 31 58Q6 56 6 31Q6 6 32 6Z", CREAM, extra=' stroke="%s" stroke-width="2"' % BROWN_L), C(19, 30, 5, RED), C(26, 17, 5, YELLOW), C(41, 15, 5, GREEN), C(49, 27, 5, BLUE), C(22, 44, 5, PURPLE)]
@icon("themes", "Themes")
def _():
    out = []
    for i, col in enumerate((RED, AMBER, GREEN, BLUE)):
        out.append('<rect x="24" y="6" width="16" height="46" rx="4" fill="%s" transform="rotate(%s 32 54)"/>' % (col, n(-42 + i * 28)))
    return out + [C(32, 53, 3, WHITE)]
@icon("backgrounds", "Backgrounds")
def _(): return [R(12, 6, 46, 36, 5, G300), R(5, 15, 48, 40, 5, PAPER, extra=RIMMED), R(10, 20, 38, 30, 2, SKY), C(19, 28, 4.4, YELLOW), poly([(10, 50), (24, 34), (34, 45), (40, 39), (48, 49), (48, 50)], GREEN_D)]
@icon("datetime", "Date and time")
def _(): return [C(30, 30, 25, G300), C(30, 30, 21, PAPER), L(30, 30, 30, 16, G800, 3.6), L(30, 30, 40, 35, G800, 3.6), C(30, 30, 2.6, G800)] + [R(38, 38, 22, 22, 4, PAPER, extra=RIMMED), R(38, 38, 22, 8, 4, RED), R(38, 43, 22, 3, 0, RED), R(43, 49.5, 5, 5, 1, G400, "from32"), R(50.5, 49.5, 5, 5, 1, G400, "from32")]
@icon("keyboard", "Keyboard")
def _():
    out = [R(3, 17, 58, 30, 5, G600)]
    for r in range(3):
        for c in range(9 if r < 2 else 0):
            out.append(R(7.5 + c * 5.6 + (r % 2) * 1.2, 21 + r * 7, 4.2, 5, 1, G200, "from24"))
    return out + [R(8, 35, 5.5, 5, 1, G200, "from24"), R(50.5, 35, 5.5, 5, 1, G200, "from24"), R(16, 35, 32, 5, 1, G200), R(8, 21, 48, 5, 1, G200, "upto16"), R(8, 28, 48, 5, 1, G200, "upto16")]
@icon("keyboard-aluminium", "Aluminium keyboard")
def _():
    out = [R(3, 17, 58, 30, 5, G300)]
    for r in range(2):
        for c in range(9):
            out.append(R(7.5 + c * 5.6 + (r % 2) * 1.2, 21 + r * 7, 4.2, 5, 1, WHITE, "from24"))
    return out + [R(8, 35, 5.5, 5, 1, WHITE, "from24"), R(50.5, 35, 5.5, 5, 1, WHITE, "from24"), R(16, 35, 32, 5, 1, WHITE), R(8, 21, 48, 5, 1, WHITE, "upto16"), R(8, 28, 48, 5, 1, WHITE, "upto16")]
@icon("touchpad", "Touchpad")
def _(): return [R(6, 9, 52, 46, 7, G300), R(6, 9, 52, 34, 7, G200), R(6, 36, 52, 7, 0, G200), L(32, 44, 32, 54, G400, 2, "from24"), C(26, 25, 7, BLUE_L), C(26, 25, 3.4, BLUE)]
@icon("usb", "USB stick")
def _(): return [R(22, 4, 20, 16, 2, G300), R(26, 9, 4.5, 5, 0.8, G600, "from24"), R(33.5, 9, 4.5, 5, 0.8, G600, "from24"), R(16, 18, 32, 42, 6, BLUE), R(27, 46, 10, 6, 3, BLUE_L, "from24")]
@icon("cpu", "Processor")
def _():
    out = [R(x, 4, 4, 56, 1.5, G400) for x in (18, 26, 34, 42)] + [R(4, y, 56, 4, 1.5, G400) for y in (18, 26, 34, 42)]
    return out + [R(10, 10, 44, 44, 5, GREEN_D), R(20, 20, 24, 24, 3, G700), R(25, 25, 14, 14, 2, G400, "from24")]
@icon("gpu", "Graphics card")
def _(): return [R(4, 15, 56, 30, 4, G700), R(8, 45, 26, 6, 1, YELLOW), R(4, 12, 4, 40, 1.5, G400), C(24, 30, 10, G500), C(24, 30, 3, G300), C(46, 30, 10, G500), C(46, 30, 3, G300)] + [L(24, 30, 24 + 7 * math.sin(math.radians(a)), 30 - 7 * math.cos(math.radians(a)), G300, 2, "from32") for a in (0, 120, 240)] + [L(46, 30, 46 + 7 * math.sin(math.radians(a)), 30 - 7 * math.cos(math.radians(a)), G300, 2, "from32") for a in (60, 180, 300)]
@icon("memory", "Memory")
def _(): return [R(3, 16, 58, 26, 3, GREEN_D)] + [R(8 + i * 12.6, 21, 9.5, 13, 1.5, G800) for i in range(4)] + [R(6 + i * 4.4, 42, 2.6, 6, 0.6, YELLOW, "from24") for i in range(12) if i != 6]
@icon("system", "System")
def _(): return [R(13, 4, 38, 56, 5, G700), R(18, 10, 28, 5, 1.5, G500), R(18, 18, 28, 5, 1.5, G500), R(18, 26, 28, 5, 1.5, G500, "from24"), C(32, 46, 5.5, G500), C(32, 46, 2.4, GREEN_L)]
@icon("database", "Database")
def _():
    out = []
    for i, col in enumerate((BLUE_D, BLUE, SKY)):
        y = 39 - i * 14.5
        out += [R(9, y, 46, 14, 0, col), E(32, y + 14, 23, 6, col), E(32, y, 23, 6, BLUE_L if i == 2 else SKY if i == 1 else BLUE)]
    return out
@icon("drive-setup", "Drive setup")
def _(): return [R(4, 10, 50, 34, 6, G400), R(4, 30, 50, 14, 6, G500), R(4, 30, 50, 6, 0, G500), L(10, 37, 26, 37, G300, 3, "from24"), C(21, 20, 6.5, G200, "from24")] + gear(45, 44, 9, BLUE, 8, PAPER)
@icon("diskbackup", "DiskBackup")
def _(): return [R(4, 10, 50, 34, 6, G400), R(4, 30, 50, 14, 6, G500), R(4, 30, 50, 6, 0, G500), L(10, 37, 26, 37, G300, 3, "from24"), C(21, 20, 6.5, G200, "from24")] + badge(45, 44, 12, GREEN) + [arc(45, 44, 6.5, 50, 330, WHITE, 3), poly([(45, 33.5), (51, 38.5), (44, 41)], WHITE)]
@icon("backup", "Backup")
def _(): return disc(GREEN) + [arc(32, 32, 14, 50, 330, WHITE, 5), poly([(32, 8.5), (43.5, 18.5), (30, 23.5)], WHITE), L(32, 24, 32, 33, WHITE, 4), L(32, 33, 39, 37, WHITE, 4)]
@icon("disk-usage", "Disk usage")
def _(): return [C(32, 32, 26, BLUE), P("M32 32V6A26 26 0 0 1 56.4 41Z", AMBER), P("M32 32L56.4 41A26 26 0 0 1 40 56.7Z", GREEN), C(32, 32, 10, "#f1f3f6", "from24"), C(32, 32, 10, WHITE)]
@icon("installer", "Installer")
def _(): return [P("M6 26L32 16L58 26V50L32 60L6 50Z", BROWN), P("M32 36L58 26V50L32 60Z", BROWN_D), P("M6 26L32 36L58 26L32 16Z", BROWN_L)] + arrow(32, 3, 32, 25, GREEN_D, 6.5, 8.5)
@icon("archive", "Archive")
def _(): return [R(4, 9, 56, 14, 3, BROWN_D), R(8, 23, 48, 33, 3, BROWN), R(24, 30, 16, 6, 3, CREAM)]
@icon("software-update", "Software update")
def _(): return [R(10, 22, 44, 34, 3, BROWN), R(6, 14, 52, 11, 3, BROWN_D), R(28, 14, 8, 20, 0, CREAM, "from24")] + badge(46, 44, 12, GREEN) + [arc(46, 44, 6.5, 50, 330, WHITE, 3), poly([(46, 33.5), (52, 38.5), (45, 41)], WHITE)]
@icon("kache", "Kache")
def _(): return [R(10, 22, 44, 34, 3, BROWN), R(6, 14, 52, 11, 3, BROWN_D)] + bolt(32, 39, 1.6, YELLOW)
@icon("suggestions", "Suggestions")
def _(): return [R(8, 24, 48, 32, 4, BLUE), R(18, 22, 28, 5, 2.5, G800), poly([(24, 24), (26, 4), (44, 7), (40, 24)], PAPER), poly([(32, 34), (34.6, 40), (41, 40.5), (36, 44.6), (37.7, 51), (32, 47.4), (26.3, 51), (28, 44.6), (23, 40.5), (29.4, 40)], YELLOW)]
@icon("integrations", "Integrations")
def _(): return [P("M6 10H22A6 6 0 1 1 34 10H50V26A6 6 0 1 0 50 38V54H34A6 6 0 1 0 22 54H6V38A6 6 0 1 1 6 26Z".replace("M6 10H22A6 6 0 1 1 34 10H50V26A6 6 0 1 0 50 38V54H34A6 6 0 1 0 22 54H6V38A6 6 0 1 1 6 26Z", "M5 14Q5 10 9 10H20A7 7 0 1 1 34 10H45Q49 10 49 14V25A7 7 0 1 1 49 39V50Q49 54 45 54H9Q5 54 5 50Z"), BLUE)]
@icon("development", "Development")
def _(): return [R(5, 8, 54, 48, 7, G800), S("M24 22L13 32L24 42", SKY, 4.6), S("M40 22L51 32L40 42", SKY, 4.6), L(35, 18, 29, 46, GREEN_L, 4.2)]
@icon("process", "Process")
def _(): return gear(24, 26, 14, BLUE, 8, "#f1f3f6") + gear(44, 46, 9, AMBER, 8, "#f1f3f6")
@icon("performance", "Performance")
def _(): return [C(32, 34, 26, G700), arc(32, 34, 19, -110, -40, GREEN, 5, cap="butt"), arc(32, 34, 19, -35, 35, YELLOW, 5, cap="butt"), arc(32, 34, 19, 40, 110, RED, 5, cap="butt"), L(32, 34, 44, 22, WHITE, 3.6), C(32, 34, 4, WHITE)]
@icon("speed-test", "Speed Test")
def _(): return [R(27, 3, 10, 7, 2, G500), C(32, 35, 24, G300), C(32, 35, 19.5, PAPER), L(32, 35, 32, 21, RED, 3.6), C(32, 35, 3.2, G800), L(48, 14, 52, 18, G500, 4, "from24")]
@icon("activity", "Activity Monitor")
def _(): return [R(5, 8, 54, 48, 7, G800), S("M10 34H20L25 20L32 46L38 28L42 34H54", GREEN, 4)]
@icon("sensors", "Sensors")
def _(): return [P("M26 8A6 6 0 0 1 38 8V34A13 13 0 1 1 26 34Z", PAPER, extra=RIMMED), C(32, 45, 8.5, RED), R(29.5, 20, 5, 24, 2.5, RED), L(43, 12, 49, 12, G400, 2.6, "from24"), L(43, 20, 47, 20, G400, 2.6, "from24"), L(43, 28, 49, 28, G400, 2.6, "from24")]
@icon("accessories", "Accessories")
def _(): return [G([R(8, 23, 48, 18, 9, RED), R(27, 26.5, 10, 11, 2, WHITE), R(30, 24.5, 4, 15, 1, RED), R(26, 30.5, 12, 3.4, 1, RED)], "rotate(-25 32 32)"), G([P("M30 24L34 2Q39 6 37 24Z", G300)], "rotate(20 32 30)")]
@icon("token-pool", "Token Pool")
def _():
    out = []
    for i, y in enumerate((47, 37, 27)):
        out += [R(8, y, 34, 9, 0, AMBER), E(25, y + 9, 17, 4.5, AMBER), E(25, y, 17, 4.5, YELLOW)]
    return out + [C(46, 24, 13, YELLOW), C(46, 24, 9, AMBER), R(44, 18, 4, 12, 1.5, YELLOW_L)]
@icon("screenshot", "Screenshot")
def _(): return [S("M6 20V12Q6 8 10 8H18M46 8H54Q58 8 58 12V20M58 44V52Q58 56 54 56H46M18 56H10Q6 56 6 52V44", G600, 4.6), C(32, 32, 13, G700), C(32, 32, 7.5, SKY), C(32, 32, 3, G800, "from24")]
@icon("agent-screens", "Agent Screens")
def _(): return monitor(G800, G700) + [poly([(20, 17), (20, 39), (26, 33), (30.5, 42), (35, 40), (30.5, 31), (38, 31)], WHITE), poly([(46, 15), (47.6, 19.4), (52, 21), (47.6, 22.6), (46, 27), (44.4, 22.6), (40, 21), (44.4, 19.4)], YELLOW)]
@icon("ai", "AI")
def _(): return tile(PURPLE) + [poly([(28, 12), (32.4, 25.6), (46, 30), (32.4, 34.4), (28, 48), (23.6, 34.4), (10, 30), (23.6, 25.6)], WHITE), poly([(45, 37), (47.2, 42.8), (53, 45), (47.2, 47.2), (45, 53), (42.8, 47.2), (37, 45), (42.8, 42.8)], YELLOW_L)]
@icon("airplay", "AirPlay")
def _(): return [R(5, 9, 54, 37, 5, G700), R(9, 13, 46, 29, 2, SKY), poly([(32, 36), (48, 58), (16, 58)], BLUE_D), poly([(32, 36), (48, 58), (16, 58)], "none")]
