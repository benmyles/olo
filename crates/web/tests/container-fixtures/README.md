# Original query-container box lengths

`input.html` is original passive test input. `chrome.json` contains240 independent
width observations:8 ancestor contexts ×6 units ×5 live native DPRs, from
Chrome157.0.8103.38, Chromium a733b5afbb3ba618844ff8876b89ad1a774b2f18.
Contexts cover absent, inline-size, size, border-box content sizes, nested axes,
self-versus-ancestor selection, display:contents and non-replaced inline boxes.
The native normal-flow test compares widths to within0.0001 CSS px because
Chrome exposes some geometry through float32; this is not pixel acceptance.

Run `capture.py` with `/usr/bin/python3`, its documented screen/profile/output
arguments and an owned Olo Chrome profile launched through `olo-screen`. It
checks the owning PID/profile, waits for agent control, changes live scales,
verifies DPR, closes its passive tab and restores screen settings. A temporary
invisible fixed box measures100svw/100svh and is immediately removed. Fractional
small-viewport dimensions are needed because innerWidth/innerHeight round them.
The native input remains the original file, without that measurement helper.

The first authored fixture used `border`, which the current property registry
does not support. It was replaced with equivalent border-width/style/color
longhands to isolate container content-box sizing. Chrome was recaptured with
the new input hash; all240 probes and host observations stayed identical. No
expected values were adjusted to native output, and the real Stripe source was
not edited. Broad border shorthand remains required browser work.

Specification: https://drafts.csswg.org/css-conditional-5/#container-lengths
(2026-11-02 draft, read2026-11-06), plus CSS Containment2. Nearest eligible
ancestors can differ per axis. Small-viewport fallback is used only when no
eligible query container exists. This fixture tests horizontal normal-flow box
lengths, not vertical layout, queries, script, fonts, inherited pending values,
all containment effects or website acceptance.
