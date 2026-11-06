# Original CSS environment fixtures

`cases.json` is authored test input. `chrome.json` contains unmodified independent
computed-style observations from Chrome157.0.8103.38, Chromium
a733b5afbb3ba618844ff8876b89ad1a774b2f18, on an owned Olo Wayland Agent Screen.
The input hash is recorded. Native live scales1,1.25,1.5,1.75,2 were verified
with devicePixelRatio; device emulation was cleared. The capture script records
padding after an initial7px declaration so invalid syntax and invalid computed
values can be distinguished.115 observations cover23 cases, not website pixels.

Run `capture.py` with `/usr/bin/python3` and its documented arguments after
launching private Chrome with `olo-screen run NAME olo-chrome
--user-data-dir=/var/tmp/OWNED/profile --remote-debugging-port=0`. First eval
`olo-screen env NAME`. The driver verifies its window PID/profile, waits for
agent control, and restores screen settings. Chrome UserAgent's legacy X11
spelling does not describe the native Wayland backend.

Specification: https://drafts.csswg.org/css-env-1/ (2026-11-03 draft, read
2026-11-06). Rectangular content viewports expose eight zero safe-area/current
maximum inset scalars. Hardware segment variables are undefined without at
least two hardware segments. Function spelling is insensitive to ASCII case;
environment names are case sensitive. Unknown names/indices select fallback.
Environment changes require a new host generation. This is initial env support,
not all future UA-defined values, CSSOM, foldable hardware or website acceptance.
