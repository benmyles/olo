# Original flex shorthand observations

`cases.json` is original authored input. `chrome.json` contains unmodified
observations from Chrome157.0.8103.38, Chromium
a733b5afbb3ba618844ff8876b89ad1a774b2f18.74 cases ×5 verified live Wayland DPRs
(1,1.25,1.5,1.75,2) give370 observations: CSS.supports, specified/computed
longhands and two isolated basis-width probes per case. The capture predates
the shorthand implementation. Native365 observations match;5 observations
remain explicit nonpasses in `REFERENCE-GAPS.json`. Do not call this370 passes.

Coverage includes none/auto/wide keywords, omitted defaults and explicit zero
units, basis before/after the contiguous grow/shrink group, invalid order and
negative literals, numeric calc/clamping, percentage/em/rem/math bases, pending
variables/fallbacks/cycles, a zero safe-area environment and invalid syntax.
Omitted basis retains interoperable0%, distinct from explicitly authored0px.
The native registry still has95 longhands; shorthand count grows17→18.

The reference models a CSSOM setter: start with7 8 13px, then assign the input.
Invalid syntax retains the prior value; empty assignment removes it. Parent
values3 4 17px exercise inheritance. The native test harness models these input
operations using typed declarations and the production cascade/style engine;
it does not implement or claim native CSSOM. Its `supports` result is typed
parse acceptance, compared with the reference's actual CSS.supports. Recognizing
an AST is not a claim that the browser implements that value's full layout.

Basis probes copy the reference's computed basis onto empty grow:0/shrink:0
children at200/1000px with a42px explicit auto width and no minimum/decorations.
Native numeric bases use production computed math resolution plus used-grid
snapping from authored contexts. Keywords are compared as typed identities;
the empty-element width interpretation in the harness is not a native item
formatter. These data do not accept intrinsic/auto layout, cross sizing,
wrapping/alignment, general CSSOM or website pixels. Numeric tolerance0.0001
CSS px covers float32 exposure below a layout quantum.

`fit-content(20px)` is accepted by the existing shared draft size grammar while
this Chrome rejects it. Its five records stay unchanged and marked nonpassing;
used-basis function geometry and compatible feature/profile policy remain open.
Other differences must fail the comparison, and original style-port passes
remain guarded by their existing baseline identities.

Capture with `/usr/bin/python3 capture.py --screen NAME --profile
/var/tmp/OWNED/profile --out /var/tmp/OWNED/chrome.json`, after evaluating
`olo-screen env NAME` and starting only `olo-screen run NAME olo-chrome
--user-data-dir=/var/tmp/OWNED/profile --remote-debugging-port=0 about:blank`.
The driver verifies owned window/PID/profile, loaded authored document and
actual native DPRs, clears emulation, restores settings, closes its Chrome and
waits for process exit. Stop the owned screen afterward.

Primary references (read2026-11-06):
- [Flex shorthand and basis](https://drafts.csswg.org/css-flexbox-1/#flex-property)
- [Draft size grammar](https://drafts.csswg.org/css-sizing-3/#preferred-size-properties)

A separate normative regression uses the exact retained Stripe carousel clamp
with30.38cqi. FlexBasis retains its container dependency until layout, resolves
at three query widths without restyling, and rejects inherited pending context
rebinding. This is distinct from the370 independent observations; the actual
Stripe retry reaches flex formatting before its carousel geometry executes.
