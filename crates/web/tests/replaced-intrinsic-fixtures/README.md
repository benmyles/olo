# Replaced intrinsic widths

48 cases × five live Chrome device scales (1, 1.25, 1.5, 1.75, 2).
Each case measures the grid, its picture, image and overlapping content.
The 2×1 red PNG is decoded before measuring loaded cases; source-less cases
exercise Chrome's different broken-image behavior. Width/height attributes are
2464×800, with CSS height 100px, reproducing Stripe's implicit grid track.

`capture.py CHROME_PID` runs from the repository root against owned
`codex-browser`, Chrome port 9338, profile
`/var/tmp/codex-browser-overflow/chrome-profile`. It verifies process/profile
ownership, waits for screen availability, and restores screen settings.
Run with `/usr/bin/python3` (PIL). Chrome's version and the exact input hash are
in `chrome.json`. Rust tests load identical red pixels in process.

Checks cover definite, min-content and max-content grid widths; preferred and
maximum percentages (including calc); fixed and cyclic minimums; and fixed
width controls. These are geometry tests, not page/pixel acceptance. The
original loaded-only test produced 140 geometry mismatches before the fix.

[CSS Sizing 3 cyclic percentage rules](https://drafts.csswg.org/css-sizing-3/#min-percentage-contribution)
explain why percentage constraints affect intrinsic contributions. Chrome 157
uses zero for the cyclic preferred/maximum contribution even with a constant
calc term; the term appears in the final image size. The current draft instead
describes retaining that term in the intrinsic minimum. These fixtures pin the
observed Chrome behavior, not blanket draft conformance. Cyclic min-width does
retain its constant term. Transferred aspect-ratio minimums, other replaced
controls, inline formatting and vertical writing need separate coverage.
