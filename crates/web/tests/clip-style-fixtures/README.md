# Local clip-source cascade comparisons

29 original cases × five native Wayland DPRs:145 computed clip-path values match
owned Olo Chrome. Covers SVG presentation attributes, ordinary/inline/important
CSS, layers and rollback, inheritance/defaulting, all reset, variables and
invalid-at-computed-value behavior, CSS escapes, case, percent-encoded IDs,
empty fragments and EOF-closed URL syntax. HTML attributes do not become SVG
presentation hints; ordinary HTML CSS clip-path is computed normally.

The driver checks actual DPR, fixture readiness and process/profile/window
ownership, changes real Wayland scale live without emulation, restores settings
and closes/awaits Chrome. Run it with --screen, --profile and --out in owned
/var/tmp scratch after starting Olo Chrome on an Agent Screen. chrome.json pins
Chrome and case SHA-256. The earlier28-case capture is superseded by this
29-case capture; only145 final observations count.

These are computed style comparisons, not URL resolution, clipping, geometry,
pixels, CSSOM completeness or site acceptance. Identifiers in the reference
cases need no CSS string serialization escapes. Non-local URLs and shape/box
forms remain explicitly unsupported. Rendering known local CSS clips still
fails until geometry/paint is connected; existing SVG attribute guards remain.
Native Stripe now exposes a shape/box failure during computed style, before
layout. Its retained CSS includes variable-dependent rounded inset() shapes.

Specification: https://www.w3.org/TR/css-masking-1/#the-clip-path.
