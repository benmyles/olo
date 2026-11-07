# Referenced SVG rectangle clips

37 original HTML/CSS cases at five native Wayland DPRs:185 crops. Bounding
rectangles match;137 crops are pixel-exact. REFERENCE-GAPS.json records48
fractional-edge/rounded-corner nonpasses, not accepted tolerances. The default
regression preserves every exact case and rejects worsened gaps. Set
OLO_SVG_REQUIRE_COMPLETE=1 for the strict gate, which still fails. Older SVG
pixel nonpasses remain unchanged. No website acceptance is claimed.

Covers local/encoded IDs, duplicate-ID tree order including wrong-type targets,
missing/empty/zero/auto-size clips, CSS geometry and radius overrides, all reset,
variables, percentage lengths, overlapping/disjoint rectangle union, ancestor
intersection, sibling restoration, direct path clipping, ignored paint colors
and opacity, display/visibility and multiple definition/reference viewports.
The31-case capture was superseded by the37-case capture, not added to its count.

Chrome behavior matters: display:none on the definition or an ancestor makes
the resource unavailable (no clip), unlike CSS Masking1 section6.1. These cases
pin the intentional compatibility behavior. Percentages resolve in the SVG
containing the definition, before the referencing SVG's user-space transform.
The two-viewport case caught the initial incorrect reference-viewport basis.

The owned Olo Chrome capture checks process/profile/window, readiness, native
DPR and source hash; changes Wayland scale live, uses no emulation/page zoom,
restores settings and closes/awaits Chrome. Driver arguments: --screen NAME
--profile /var/tmp/OWNED/profile --out /var/tmp/OWNED/chrome.json. PNG hashes and
browser revision are pinned in chrome.json. These are authored component pages,
not extracted Stripe assets, journeys or page captures.

Remaining: analytic raster fidelity, root clipping of decorations, object
bounding-box units, non-rect clip geometry, recursive clip definitions and
transforms, basic-shape clipping, full SVG hit testing/group compositing. Current
unsupported forms stop explicitly; SVG root clip-path:none overrides and invalid
root references do work. Only rectangle clip definitions are compiled here;
ordinary visible rect elements are not yet implemented.

Specifications consulted: https://www.w3.org/TR/css-masking-1/#the-clip-path,
https://www.w3.org/TR/SVG2/linking.html#processingURL-validity,
https://www.w3.org/TR/SVG2/shapes.html#RectElement.
