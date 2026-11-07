# SVG object and group opacity

28 original cases at five native Wayland DPRs:140 independent Chrome crops.
All bounds match;65 crops are exact and75 fractional-edge/curve/clip pixel
nonpasses remain. REFERENCE-GAPS.json records these failures, not acceptance
tolerances. Default regressions preserve exact cases and reject worsened gaps;
OLO_SVG_REQUIRE_COMPLETE=1 requires every pixel and still fails.

Covers overlapping objects versus groups, nested/sibling/empty groups,
translucent children, fill opacity, zero/hidden/display-none groups, explicit
inheritance, CSS overrides/reset/variables/math/clamping, clipped opacity and
opacity inside clipped groups, fractional edges, curves and offscreen content.
Group alpha is quantized once to eight bits, matching observed reference solid
colors, including nested groups. Raster path/clip coverage remains incomplete.
Earlier SVG corpora retain38 path/overflow and48 rectangular-clip nonpasses.

Root SVG opacity and decoration stacking, non-path shapes, transforms, filters,
masks, blending and complete hit testing remain open. This does not accept
Stripe or any website. See capture.py for owned-screen capture/restoration and
process validation. Source and PNG hashes/browser version are in chrome.json.
