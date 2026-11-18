from forms import *  # noqa: F401,F403
ICONS = {}
def icon(name, title):
    def deco(fn):
        ICONS[name] = (title, fn)
        return fn
    return deco
RIMMED = ' stroke="%s" stroke-width="2"' % RIM

@icon("bluetooth", "Bluetooth")
def _(): return [R(13, 4, 38, 56, 19, BLUE), S("M22 22L42 42L32 51V13L42 22L22 42", WHITE, 4.2)]
@icon("browser", "Browser")
def _(): return disc(BLUE) + [ring(32, 32, 26, 0, BLUE), E(32, 32, 11, 26, "none"), S("M32 6V58M6 32H58M10 19Q32 27 54 19M10 45Q32 37 54 45", BLUE_L, 3), '<ellipse cx="32" cy="32" rx="12" ry="26" fill="none" stroke="%s" stroke-width="3"/>' % BLUE_L]
@icon("internet", "Internet")
def _(): return [C(30, 30, 24, BLUE), P("M14 18Q22 14 26 20Q30 24 24 28Q18 30 18 36Q14 34 10 30Q9 23 14 18Z", GREEN), P("M34 12Q44 12 48 20Q44 24 40 22Q34 20 34 12Z", GREEN), P("M32 36Q40 32 46 38Q46 46 38 50Q32 46 32 36Z", GREEN)] + [poly([(40, 36), (40, 60), (46.5, 54), (51, 62), (56, 59.5), (51.5, 51.5), (60, 51.5)], WHITE), poly([(42.4, 41), (42.4, 55), (47, 51), (52, 59), (53.4, 58.3), (48.5, 49.6), (55, 49.6)], G800)]
@icon("network", "Network")
def _(): return [L(32, 22, 32, 34, G400, 3.4), L(14, 34, 50, 34, G400, 3.4), L(14, 34, 14, 42, G400, 3.4), L(50, 34, 50, 42, G400, 3.4), L(32, 34, 32, 42, G400, 3.4), R(21, 4, 22, 18, 4, BLUE), R(5, 42, 18, 16, 4, GREEN), R(23, 42, 18, 16, 4, AMBER), R(41, 42, 18, 16, 4, RED)]
@icon("network-prefs", "Network settings")
def _(): return [arc(32, 40, 26, -52, 52, BLUE, 5.5), arc(32, 40, 17, -52, 52, BLUE, 5.5), arc(32, 40, 8, -52, 52, BLUE, 5.5), C(32, 41, 3.6, BLUE), R(8, 46, 48, 12, 4, G600), C(16, 52, 2.2, GREEN_L), C(23, 52, 2.2, GREEN_L, "from24")]
@icon("sound", "Sound")
def _(): return [P("M6 24Q6 22 8 22H18L32 9Q34 7.5 34 10V54Q34 56.5 32 55L18 42H8Q6 42 6 40Z", G600), arc(34, 32, 9, 40, 140, BLUE, 4.6), arc(34, 32, 18, 40, 140, BLUE, 4.6)]
@icon("audio", "Audio")
def _(): return tile(PINK) + [C(22, 43, 7.5, WHITE), C(42, 39, 7.5, WHITE), S("M26.5 43V18L46.5 14V39", WHITE, 5), L(26.5, 20, 46.5, 16, WHITE, 7, cap="butt")]
@icon("music", "Music")
def _(): return [S("M10 40V32A22 22 0 0 1 54 32V40", G700, 5.5), R(5, 34, 14, 22, 6, G700), R(45, 34, 14, 22, 6, G700), R(9, 38, 6, 14, 3, BLUE, "from24"), R(49, 38, 6, 14, 3, BLUE, "from24")]
@icon("microphone", "Microphone")
def _(): return [R(22, 4, 20, 34, 10, G500), L(26, 14, 38, 14, G300, 2.4, "from24"), L(26, 20, 38, 20, G300, 2.4, "from24"), L(26, 26, 38, 26, G300, 2.4, "from24"), S("M14 28V30Q14 46 32 46Q50 46 50 30V28", G700, 4.6), L(32, 46, 32, 55, G700, 4.6), L(21, 57, 43, 57, G700, 4.6)]
@icon("video", "Video")
def _(): return [R(6, 20, 52, 36, 4, G800), G([R(6, 8, 52, 11, 2, G700)] + [poly([(10 + i * 12, 8), (17 + i * 12, 8), (12 + i * 12, 19), (5 + i * 12, 19)], PAPER) for i in range(1, 5)], "rotate(-9 6 20)"), poly([(27, 30), (27, 48), (42, 39)], WHITE)]
@icon("multimedia", "Multimedia")
def _(): return [R(4, 9, 42, 34, 5, G800), poly([(19, 17), (19, 35), (34, 26)], WHITE), R(4, 45, 42, 4, 2, G400, "from24"), C(42, 50, 6.5, PINK), C(55, 46, 6, PINK), S("M46 50V30L59 27V46", PINK, 4.4)]
@icon("games", "Games")
def _(): return [P("M18 16H46Q58 16 60 30L62 44Q62 52 55 52Q50 52 46 46L44 43H20L18 46Q14 52 9 52Q2 52 2 44L4 30Q6 16 18 16Z", G600), R(13.5, 27, 13, 4.6, 1.5, G200), R(17.7, 22.8, 4.6, 13, 1.5, G200), C(42, 33, 3.2, BLUE), C(49, 27, 3.2, RED), C(49, 39, 3.2, GREEN_L, "from24"), C(56, 33, 3.2, YELLOW, "from24")]
@icon("office", "Office")
def _(): return [R(6, 22, 52, 34, 5, BROWN), R(6, 22, 52, 14, 5, BROWN_D), R(6, 30, 52, 6, 0, BROWN_D), S("M23 22V15Q23 11 27 11H37Q41 11 41 15V22", BROWN_D, 4.4), R(27, 31, 10, 9, 2, YELLOW)]
@icon("people", "People")
def _(): return [C(44, 22, 9, ORANGE), P("M28 56Q28 36 44 36Q60 36 60 56Z", ORANGE), C(23, 20, 11, BLUE), P("M3 58Q3 35 23 35Q43 35 43 58Z", BLUE)]
def bud(flip):
    body = [P("M20 8Q34 6 38 18Q41 28 33 32V54Q33 59 28.5 59Q24 59 24 54V32Q14 30 13 20Q12 10 20 8Z", PAPER, extra=RIMMED), E(22, 20, 4, 7, G700), R(26.5, 50, 4, 5, 2, G400, "from24")]
    return disc(BLUE_P) + [G(body, "translate(%s 0) scale(%s 1) translate(6 -1)" % ((64, -1) if flip else (0, 1)))]
@icon("airpod-left", "Left AirPod")
def _(): return bud(False)
@icon("airpod-right", "Right AirPod")
def _(): return bud(True)
@icon("airpods", "AirPods")
def _():
    one = lambda dx, flip: G([P("M20 8Q34 6 38 18Q41 28 33 32V54Q33 59 28.5 59Q24 59 24 54V32Q14 30 13 20Q12 10 20 8Z", PAPER, extra=RIMMED), E(22, 20, 4, 7, G700)], "translate(%s 4) scale(%s 0.86)" % ((dx + 52 * 0.86, -0.86) if flip else (dx, 0.86)))
    return disc(BLUE_P) + [one(4, False), one(16, True)]
@icon("airpods-case", "AirPods case")
def _(): return disc(BLUE_P) + [R(11, 14, 42, 38, 12, PAPER, extra=RIMMED), L(12, 27, 52, 27, RIM, 2), C(32, 34, 2.4, GREEN)]
def phone(body=G700, lens=None, flash=None, shade=None, r=7):
    "A phone from behind: its body, a camera plate with lenses, its flash."
    light = sum(int(body[i:i + 2], 16) for i in (1, 3, 5)) > 660
    out = [R(15, 4, 34, 56, r, body, extra=(' stroke="%s" stroke-width="2"' % RIM) if light else "")]
    return out + (lens or [])
def lenses(points, r=3.6, ring_c=G800, glass=G500):
    out = []
    for x, y in points:
        out += [C(x, y, r, ring_c), C(x, y, r * 0.45, glass, "from24")]
    return out
@icon("phone", "Phone")
def _(): return [R(15, 4, 34, 56, 7, G800), R(18, 8, 28, 46, 3, SKY), R(27, 5.6, 10, 1.8, 0.9, G600, "from24"), poly([(18, 54), (30, 36), (38, 46), (42, 42), (46, 48), (46, 54)], BLUE, "from24")]
PHONES = {
    "phone-iphone-15": ("iPhone 15", "#f9c9d6", [R(19, 8, 15, 15, 4.5, "#f3b6c6")] + lenses([(23.5, 12.5), (29.5, 18.5)], 3.4) + [C(29.5, 12, 1.3, WHITE, "from32")]),
    "phone-iphone-16": ("iPhone 16", "#6f9be8", [R(19, 8, 9, 17, 4.5, "#5a86d6")] + lenses([(23.5, 12.5), (23.5, 20.5)], 3.4) + [C(31, 12, 1.3, WHITE, "from32")]),
    "phone-iphone-16e": ("iPhone 16e", "#f1f3f6", lenses([(24, 13)], 4.2, G700) + [C(31, 13, 1.3, G400, "from32")]),
    "phone-iphone-17e": ("iPhone 17e", "#2f3640", lenses([(24, 13)], 4.2, G500, G800) + [C(31, 13, 1.3, G300, "from32")]),
    "phone-iphone-16-pro": ("iPhone 16 Pro", "#c9b79c", [R(19, 8, 17, 17, 5, "#b8a488")] + lenses([(24, 13), (24, 20), (31, 16.5)], 3.3)),
    "phone-iphone-16-pro-max": ("iPhone 16 Pro Max", "#3b3f47", [R(19, 8, 17, 17, 5, "#2a2d33")] + lenses([(24, 13), (24, 20), (31, 16.5)], 3.3, G500, G800)),
    "phone-iphone-17": ("iPhone 17", "#b9d7a8", [R(19, 8, 9, 17, 4.5, "#a5c793")] + lenses([(23.5, 12.5), (23.5, 20.5)], 3.4) + [C(31, 12, 1.3, WHITE, "from32")]),
    "phone-iphone-17-pro": ("iPhone 17 Pro", "#f08a3c", [R(15, 4, 34, 20, 7, "#de7527"), R(15, 16, 34, 8, 0, "#de7527")] + lenses([(23, 10.5), (23, 18), (30.5, 14.2)], 3.3) + [C(43, 11, 1.4, WHITE, "from32")]),
    "phone-iphone-17-pro-max": ("iPhone 17 Pro Max", "#34466e", [R(15, 4, 34, 20, 7, "#263556"), R(15, 16, 34, 8, 0, "#263556")] + lenses([(23, 10.5), (23, 18), (30.5, 14.2)], 3.3, G400, G800) + [C(43, 11, 1.4, WHITE, "from32")]),
    "phone-iphone-air": ("iPhone Air", "#dfe9f3", [R(19, 8, 26, 10, 5, "#c8d6e4")] + lenses([(24.5, 13)], 3.6) + [C(40, 13, 1.3, WHITE, "from32")]),
    "phone-pixel-10": ("Pixel 10", "#6e7fd6", [R(19, 10, 26, 10, 5, G800)] + lenses([(25, 15), (33, 15)], 3, G600, SKY) + [C(40.5, 15, 1.3, YELLOW_L, "from32")]),
    "phone-pixel-10-pro": ("Pixel 10 Pro", "#d9d4c7", [R(19, 10, 26, 10, 5, G800)] + lenses([(24, 15), (30.5, 15), (37, 15)], 2.8, G600, SKY) + [C(42, 15, 1.1, YELLOW_L, "from32")]),
    "phone-galaxy-a07": ("Galaxy A07", "#9fd3c0", [R(19, 8, 8.5, 17, 4.2, "#8cc3af")] + lenses([(23.2, 12.5), (23.2, 20.5)], 3.1)),
    "phone-galaxy-a17": ("Galaxy A17", "#aab7c8", [R(19, 8, 8.5, 24, 4.2, "#98a6b8")] + lenses([(23.2, 12.5), (23.2, 20), (23.2, 27.5)], 3.1)),
    "phone-galaxy-a56": ("Galaxy A56", "#f3c6d4", [R(19, 8, 8.5, 24, 4.2, G700)] + lenses([(23.2, 12.5), (23.2, 20), (23.2, 27.5)], 3.1, G500, G800)),
    "phone-galaxy-s26": ("Galaxy S26", "#cfe0f5", lenses([(23.5, 12.5), (23.5, 20.5), (23.5, 28.5)], 3.4) + [C(30.5, 12.5, 1.3, YELLOW_L, "from32")]),
    "phone-galaxy-s25-ultra": ("Galaxy S25 Ultra", "#8d939c", lenses([(23.5, 12.5), (23.5, 20.5), (23.5, 28.5)], 3.5, G800, G600) + lenses([(31.5, 16.5), (31.5, 24.5)], 2.6, G800, G600)),
    "phone-galaxy-s26-ultra": ("Galaxy S26 Ultra", "#2c3038", [R(19, 8, 9, 25, 4.5, "#3b4049")] + lenses([(23.5, 12.5), (23.5, 20.5), (23.5, 28.5)], 3.3, G500, G800) + lenses([(32, 16.5), (32, 24.5)], 2.4, G500, G800)),
    "phone-redmi-a5": ("Redmi A5", "#8fc7e8", [C(27, 16, 10, "#78b5d9")] + lenses([(27, 12), (23, 19)], 3) + [C(31.5, 19, 1.4, YELLOW_L, "from32")]),
    "phone-redmi-note-14": ("Redmi Note 14", "#b9a2e6", [R(18.5, 7.5, 19, 19, 6, G800)] + lenses([(24, 13), (32, 13), (24, 21)], 3, G600, SKY) + [C(32, 21, 1.6, YELLOW_L, "from32")]),
}
for name, (title, body, parts) in PHONES.items():
    r = 3.5 if "ultra" in name else 7
    icon(name, title)(lambda body=body, parts=parts, r=r: phone(body, parts, r=r))

# --- the sky -------------------------------------------------------------
SKYDARK = "#94a3b8"
def W(name, title, fn): icon(name, title)(fn)
W("weather", "Weather", lambda: sun(22, 22, 10) + cloud(36, 40, 1.15, PAPER) )
W("weather-clear-day", "Clear", lambda: sun(32, 32, 13))
W("weather-clear-night", "Clear night", lambda: moon(30, 32, 20) + [C(49, 15, 2.2, YELLOW_L, "from24"), C(54, 27, 1.6, YELLOW_L, "from24")])
W("weather-mostly-clear-day", "Mostly clear", lambda: sun(28, 27, 12) + cloud(42, 46, 0.7, PAPER))
W("weather-mostly-clear-night", "Mostly clear night", lambda: moon(26, 27, 17) + cloud(42, 46, 0.7, G300))
W("weather-partly-cloudy-day", "Partly cloudy", lambda: sun(22, 22, 10) + cloud(36, 40, 1.15, PAPER))
W("weather-partly-cloudy-night", "Partly cloudy night", lambda: moon(20, 22, 14) + cloud(36, 40, 1.15, G300))
W("weather-mostly-cloudy", "Mostly cloudy", lambda: sun(46, 18, 7) + cloud(38, 26, 0.85, G300) + cloud(30, 40, 1.2, PAPER))
W("weather-cloudy", "Cloudy", lambda: cloud(40, 24, 0.9, G400) + cloud(30, 38, 1.25, G200))
W("weather-fog", "Fog", lambda: cloud(32, 22, 1.2, G300) + [L(8, 42, 56, 42, G400, 4), L(14, 50, 50, 50, G400, 4), L(8, 58, 40, 58, G400, 4, "from24")])
W("weather-drizzle", "Drizzle", lambda: cloud(32, 24, 1.3, G300) + drops((22, 34, 46), 44, SKY, 6))
W("weather-rain", "Rain", lambda: cloud(32, 24, 1.3, G400) + drops((20, 30, 40, 50), 43, BLUE, 9))
W("weather-heavy-rain", "Heavy rain", lambda: cloud(32, 22, 1.3, G500) + drops((18, 27, 36, 45, 54), 41, BLUE, 8) + drops((22, 31, 40, 49), 52, BLUE, 7, "from24"))
W("weather-freezing-rain", "Freezing rain", lambda: cloud(32, 24, 1.3, G400) + drops((22, 44), 43, BLUE, 9) + flakes((33,), 50, SKY))
W("weather-sleet", "Sleet", lambda: cloud(32, 24, 1.3, G400) + drops((20, 42), 43, BLUE, 9) + flakes((31, 52), 50, SKY))
W("weather-flurries", "Flurries", lambda: cloud(32, 24, 1.3, G300) + flakes((24, 42), 49, SKY))
W("weather-snow", "Snow", lambda: cloud(32, 24, 1.3, G300) + flakes((20, 32, 44), 48, SKY))
W("weather-heavy-snow", "Heavy snow", lambda: cloud(32, 22, 1.3, G400) + flakes((18, 30, 42, 54), 44, SKY) + flakes((24, 36, 48), 55, SKY, "from24"))
W("weather-thunderstorm", "Thunderstorm", lambda: cloud(32, 22, 1.3, G600) + bolt(32, 46, 1.5, YELLOW) + drops((16, 50), 44, BLUE, 8, "from24"))
