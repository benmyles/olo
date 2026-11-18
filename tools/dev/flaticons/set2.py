from forms import *  # noqa: F401,F403
ICONS = {}
def icon(name, title):
    def deco(fn):
        ICONS[name] = (title, fn)
        return fn
    return deco
RIMMED = ' stroke="%s" stroke-width="2"' % RIM

# --- pages with a mark -----------------------------------------------------
def bars(align):
    out = page(11, 5, 42, 54, 0)
    widths = [30, 20, 30, 16, 26]
    for i, w in enumerate(widths):
        y = 14 + i * 8.6
        if align == "justify": x0, w = 17, 30
        elif align == "left": x0 = 17
        elif align == "right": x0 = 47 - w
        else: x0 = 32 - w / 2
        out.append(L(x0, y, x0 + w, y, BLUE if i == 0 else G500, 3.6))
    return out
for a in ("left", "center", "right", "justify"):
    icon("align-" + a, "Align " + a)(lambda a=a: bars(a))

@icon("indent-more", "Indent more")
def _(): return page(11, 5, 42, 54, 0) + [L(17, 14, 47, 14, G500, 3.6), L(30, 23, 47, 23, G500, 3.6), L(30, 32, 47, 32, G500, 3.6), L(30, 41, 47, 41, G500, 3.6), L(17, 50, 47, 50, G500, 3.6)] + arrow(15, 32, 24, 32, BLUE, 3.4, 5)
@icon("indent-less", "Indent less")
def _(): return page(11, 5, 42, 54, 0) + [L(17, 14, 47, 14, G500, 3.6), L(30, 23, 47, 23, G500, 3.6), L(30, 32, 47, 32, G500, 3.6), L(30, 41, 47, 41, G500, 3.6), L(17, 50, 47, 50, G500, 3.6)] + arrow(26, 32, 17, 32, BLUE, 3.4, 5)
@icon("list-bullets", "Bulleted list")
def _(): return page(11, 5, 42, 54, 0) + sum(([C(19.5, y, 2.8, BLUE), L(27, y, 46, y, G500, 3.6)] for y in (16, 27, 38, 49)), [])
@icon("list-numbers", "Numbered list")
def _(): return page(11, 5, 42, 54, 0) + sum(([R(17, y - 3.2, 2.6 + i * 1.6, 6.4, 1.2, BLUE), L(28, y, 46, y, G500, 3.6)] for i, y in enumerate((16, 27, 38, 49))), [])
@icon("wrap-lines", "Wrap lines")
def _(): return page(11, 5, 42, 54, 0) + [L(17, 15, 47, 15, G500, 3.6), S("M17 27H42Q47 27 47 32.5Q47 38 42 38H30", BLUE, 3.6), S("M34 33.5L29.5 38L34 42.5", BLUE, 3.6), L(17, 38, 22, 38, G500, 3.6), L(17, 49, 38, 49, G500, 3.6)]
@icon("header-footer", "Header and footer")
def _(): return page(11, 5, 42, 54, 0) + [R(11, 5, 42, 11, 3, BLUE_L), R(11, 48, 42, 11, 3, BLUE_L), L(17, 10.5, 34, 10.5, BLUE_D, 3), L(17, 53.5, 28, 53.5, BLUE_D, 3)] + lines(17, 24, 30, 3, 7.5, G400, 3)
@icon("page-view", "Page view")
def _(): return page(4, 10, 27, 44, 0) + page(33, 10, 27, 44, 0) + lines(9, 19, 17, 4, 7.5, G400, 3) + lines(38, 19, 17, 4, 7.5, G400, 3)
@icon("insert-table", "Insert table")
def _():
    out = [R(5, 9, 54, 46, 4, PAPER, extra=RIMMED), R(5, 9, 54, 12, 4, BLUE), R(5, 15, 54, 6, 0, BLUE)]
    out += [L(5, 32, 59, 32, RIM, 2), L(5, 43.5, 59, 43.5, RIM, 2), L(23, 21, 23, 55, RIM, 2), L(41, 21, 41, 55, RIM, 2)]
    return out + badge(48, 46, 10.5, GREEN) + [plus(48, 46, 5)]
@icon("spreadsheet", "Spreadsheet")
def _():
    out = page() + [R(19, 22, 26, 28, 1.5, GREEN_L), R(19, 22, 26, 7, 1.5, GREEN)]
    return out + [L(19, 36, 45, 36, WHITE, 1.8, "from24"), L(19, 43, 45, 43, WHITE, 1.8, "from24"), L(28, 22, 28, 50, WHITE, 1.8, "from24"), L(36.5, 22, 36.5, 50, WHITE, 1.8, "from24")]
@icon("word-document", "Word document")
def _(): return page() + [R(13, 5, 9, 54, 3, BLUE), R(18, 5, 4, 54, 0, BLUE)] + lines(27, 24, 19, 4, 7, G400, 3)
@icon("presentation", "Presentation")
def _(): return page(6, 11, 52, 42, 0) + [R(6, 11, 52, 9, 3, ORANGE), R(6, 16, 52, 4, 0, ORANGE), C(22, 36, 9, ORANGE), P("M22 36V27A9 9 0 0 1 31 36Z", YELLOW)] + lines(36, 30, 16, 3, 6.5, G400, 3)
@icon("pdf", "PDF")
def _(): return page() + [R(8, 30, 40, 18, 3, RED), S("M14 44V35H17.5Q21 35 21 38Q21 41 17.5 41H14", WHITE, 2.6), S("M25.5 44V35H28Q33 35 33 39.5Q33 44 28 44Z", WHITE, 2.6), S("M37.5 44V35H43.5M37.5 39.7H42", WHITE, 2.6)] + lines(20, 15, 14, 2, 6.5, G400, 3)
@icon("secure-note", "Secure note")
def _(): return [P("M9 9Q9 6 12 6H52Q55 6 55 9V45L43 57H12Q9 57 9 54Z", YELLOW_L, extra=' stroke="%s" stroke-width="2"' % AMBER), P("M55 45H46Q43 45 43 48V57Z", AMBER)] + lines(16, 16, 30, 2, 7, AMBER, 3) + padlock(30, 41, 0.8, G600, G500, YELLOW_L)
@icon("styled-edit", "StyledEdit")
def _(): return page(9, 5, 38, 54) + lines(16, 24, 22, 4, 7) + [G([R(-4, -24, 8, 30, 1, BLUE), R(-4, -24, 8, 6, 1, RED_L), poly([(-4, 6), (4, 6), (0, 14)], CREAM), poly([(-1.6, 10.8), (1.6, 10.8), (0, 14)], G800)], "translate(47 40) rotate(28)")]
@icon("spelling", "Spelling")
def _(): return page() + lines(20, 22, 24, 2, 7) + [S("M19 40q3 -4 6 0t6 0t6 0", RED, 2.6)] + badge(46, 46, 11, GREEN) + [check(46, 46.5, 5)]
@icon("query", "Find")
def _(): return page(9, 5, 38, 54) + lines(16, 22, 20, 3, 7) + magnifier(40, 40, 10)
@icon("show-image", "Show picture")
def _(): return [R(5, 9, 48, 40, 5, PAPER, extra=RIMMED), R(10, 14, 38, 30, 2, SKY), C(19, 23, 4.4, YELLOW), poly([(10, 44), (24, 28), (34, 40), (40, 34), (48, 43), (48, 44)], GREEN_D)] + magnifier(43, 43, 10)
@icon("executable", "Program")
def _(): return page() + gear(32, 36, 10, BLUE, 8, PAPER)
@icon("logs", "Logs")
def _():
    out = page(11, 5, 42, 54, 0)
    for i, col in enumerate((GREEN, G400, AMBER, G400, RED, G400)):
        y = 14 + i * 7.3
        out += [C(18.5, y, 2.4, col), L(25, y, 46 - (i % 3) * 5, y, G500, 3, "from24")]
    return out
@icon("font", "Font")
def _(): return tile(PAPER) + [R(6, 6, 52, 52, 13, "none", extra=RIMMED), S("M17 46L27 16H29L39 46M20.5 37H35.5", G800, 4.6, join="miter"), S("M43 46V32Q43 28 47.5 28Q51 28 51 32M43 36Q43 33 47 33Q51 33 51 37V46M51 41Q51 46 47 46Q43 46 43 42", BLUE, 3, "from32")]
def letter(d, extra=None, color=G800):
    return tile(PAPER) + [R(6, 6, 52, 52, 13, "none", extra=RIMMED), d] + (extra or [])
@icon("format-bold", "Bold")
def _(): return letter(S("M23 16V48M23 16H33Q41 16 41 23.5Q41 31 33 31H23M23 31H35Q43 31 43 39.5Q43 48 35 48H23", G800, 6.5, join="miter"))
@icon("format-italic", "Italic")
def _(): return letter(S("M37 16L27 48M29 16H44M20 48H35", G800, 4.6))
@icon("format-underline", "Underline")
def _(): return letter(S("M22 14V30Q22 40 32 40Q42 40 42 30V14", G800, 5), [L(20, 49, 44, 49, BLUE, 4)])
@icon("format-strikeout", "Strike out")
def _(): return letter(S("M42 21Q40 15 32 15Q23 15 23 22Q23 28 32 31Q42 34 42 41Q42 49 32 49Q23 49 21 42", G800, 5), [L(15, 32, 49, 32, RED, 4)])
@icon("ruler", "Ruler")
def _(): return [G([R(3, 21, 58, 22, 3, YELLOW)] + [L(9 + i * 6.6, 21, 9 + i * 6.6, 29 if i % 2 else 33, G800, 2.2, "from24") for i in range(8)], "rotate(-20 32 32)")]
@icon("undo", "Undo")
def _(): return [S("M22 27H38Q51 27 51 39Q51 51 38 51H20", BLUE, 8), poly([(26, 11), (6, 27), (26, 43)], BLUE)]
@icon("redo", "Redo")
def _(): return [S("M42 27H26Q13 27 13 39Q13 51 26 51H44", BLUE, 8), poly([(38, 11), (58, 27), (38, 43)], BLUE)]
@icon("cut", "Cut")
def _(): return [L(21, 8, 39, 40, G400, 5.5), L(43, 8, 25, 40, G300, 5.5), C(32, 29, 2.2, G600, "from24"), ring(20, 47, 7.5, 5.5, RED), ring(44, 47, 7.5, 5.5, RED)]
@icon("restart", "Restart")
def _(): return disc(BLUE) + [arc(32, 32, 13, 40, 320, WHITE, 5), poly([(32, 9.5), (42.5, 19), (29.5, 23.5)], WHITE)]
@icon("shut-down", "Shut down")
def _(): return disc(RED) + [arc(32, 33, 13, 35, 325, WHITE, 5), L(32, 14, 32, 30, WHITE, 5)]
@icon("log-out", "Log out")
def _(): return [R(8, 6, 30, 52, 4, BROWN), R(13, 11, 20, 42, 2, BROWN_L), C(29, 33, 2.6, YELLOW)] + arrow(36, 32, 57, 32, GREEN_D, 6.5, 8.5)
@icon("power", "Power")
def _(): return [R(25, 4, 14, 7, 2, G500), R(14, 9, 36, 51, 7, G600), R(18, 13, 28, 43, 4, G800), R(18, 27, 28, 29, 4, GREEN), R(18, 27, 28, 6, 0, GREEN)] + bolt(32, 37, 1.3, WHITE)
@icon("stay-awake", "Stay awake")
def _(): return [S("M23 5q-4 4 0 8M33 5q-4 4 0 8", G400, 3, "from24"), P("M9 20H47V38Q47 54 31 54H25Q9 54 9 38Z", PAPER, extra=RIMMED), R(12, 23, 32, 6, 2, BROWN), ring(49, 31, 7, 4.4, RIM), R(5, 55, 46, 4, 2, G400)]
@icon("idea", "Idea")
def _(): return [P("M32 5Q49 5 49 22Q49 31 42.5 36.5Q41 38 41 40V43H23V40Q23 38 21.5 36.5Q15 31 15 22Q15 5 32 5Z", YELLOW), R(23, 45, 18, 5, 2, G500), R(25, 51.5, 14, 4.5, 2, G500), S("M27 31L32 23L37 31", AMBER, 3, "from24")]
@icon("doctor", "Olo Doctor")
def _(): return [R(22, 7, 20, 10, 3, G600), R(26, 10, 12, 5, 1.5, "#f1f3f6", "from24"), R(5, 14, 54, 42, 7, RED), C(32, 35, 13, WHITE), R(28.5, 26, 7, 18, 1.5, RED), R(23, 31.5, 18, 7, 1.5, RED)]
@icon("security", "Security")
def _(): return [P("M32 4L56 12V30Q56 49 32 60Q8 49 8 30V12Z", BLUE), P("M32 4L56 12V30Q56 49 32 60Z", BLUE_D), check(32, 32, 10, WHITE, 5.5)]
@icon("authentication", "Authentication")
def _(): return padlock(32, 38, 1.9, AMBER, G400, G800)
@icon("passwords", "Passwords")
def _(): return [R(4, 17, 56, 30, 8, PAPER, extra=RIMMED)] + [C(x, 32, 4.2, G700) for x in (16, 28, 40)] + [R(49, 24, 3.4, 16, 1.5, BLUE)]
@icon("vault", "Vault")
def _(): return [R(6, 6, 52, 50, 6, G600), R(11, 11, 42, 40, 3, G500), R(13, 54, 9, 5, 1.5, G700), R(42, 54, 9, 5, 1.5, G700), C(29, 31, 11, G300), C(29, 31, 4, G700), L(29, 20, 29, 24, G700, 2.4, "from24"), L(29, 38, 29, 42, G700, 2.4, "from24"), L(18, 31, 22, 31, G700, 2.4, "from24"), L(36, 31, 40, 31, G700, 2.4, "from24"), R(45.5, 22, 4, 18, 2, G300)]
@icon("fingerprint", "Fingerprint")
def _(): return tile(G800) + [arc(32, 36, 17, -70, 70, SKY, 3.4), arc(32, 36, 11.5, -85, 85, SKY, 3.4), arc(32, 36, 6, -95, 95, SKY, 3.4), L(15.5, 41, 15.5, 47, SKY, 3.4), L(48.5, 41, 48.5, 47, SKY, 3.4), L(20.5, 37, 20.5, 49, SKY, 3.4, "from24"), L(43.5, 37, 43.5, 49, SKY, 3.4, "from24"), L(26, 37, 26, 50, SKY, 3.4), L(38, 37, 38, 50, SKY, 3.4), L(32, 36, 32, 50, SKY, 3.4)]
@icon("id-card", "ID card")
def _(): return [R(4, 12, 56, 40, 6, PAPER, extra=RIMMED), R(4, 12, 56, 9, 6, BLUE), R(4, 17, 56, 4, 0, BLUE), C(19, 31, 5.5, G400), P("M9 46Q9 38 19 38Q29 38 29 46Z", G400)] + lines(35, 29, 18, 3, 6.5, G400, 3)
@icon("credit-card", "Credit card")
def _(): return [R(4, 12, 56, 40, 6, BLUE), R(4, 20, 56, 8, 0, G800), R(10, 35, 12, 9, 2, YELLOW), L(30, 43, 52, 43, BLUE_L, 3, "from24")]
@icon("bank", "Bank")
def _(): return [poly([(32, 5), (59, 21), (5, 21)], G500), R(7, 21, 50, 5, 1, G400), R(7, 49, 50, 4, 1, G400), R(4, 53, 56, 6, 2, G500)] + [R(x, 28, 6, 20, 1.5, G300) for x in (11, 23, 35, 47)] + [C(32, 15.5, 3, G200, "from24")]
