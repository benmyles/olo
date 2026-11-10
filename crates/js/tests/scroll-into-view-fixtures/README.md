# CSSOM scrolling reference

Independent observations from Olo Chrome 157 on an owned Wayland Agent Screen.
`capture-manifest.json` records the browser revision and SHA-256 inputs/outputs.
`capture.py` retains the capture procedure: update its scratch path, owned screen,
profile and CDP port before replay. It validates ownership before connecting,
changes the actual display scale, and restores settings. No emulated DPR.

`journey.js` contains 28 cases at each of five scales: nested ancestors,
start/center/end/nearest, oversized targets, hidden/clip overflow, fixed and
hidden targets, physical/logical margin/padding, RTL, vertical writing, translated
targets and fractional bounds. Tests compare both offsets and target rectangles
within 0.02 CSS px, from fresh and reused realms. 135 cases pass; the five
`vertical-rl-start` cases remain explicit ignored nonpasses because vertical
block layout places a wide child incorrectly. They are not accepted coverage.

`smooth.js` checks intermediate motion, endpoint, event order, promise completion
and interruption by an instant scroll. `idl.js` checks dictionary getter order,
method lengths, omitted/nonfinite coordinates and asynchronous type/brand errors.
These two asynchronous oracles were captured with the Chrome window focused at
2× after the geometry run. They do not establish identical animation timing.

References: [CSSOM View](https://drafts.csswg.org/cssom-view/#dom-element-scrollintoview)
and [Chromium scroll curves](https://chromium.googlesource.com/chromium/src/+/HEAD/cc/animation/scroll_offset_animation_curve.cc).
Transformed ancestors, nested classic gutters, iframe traversal, snap positions,
full vertical flow and user interruption/event timing still need wider coverage.
