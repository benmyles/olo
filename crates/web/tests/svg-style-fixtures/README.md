# SVG presentation attributes and computed paint styles

Original authored cases; independent Chrome capture via an owned Olo Agent
Screen, at live native DPR 1, 1.25, 1.5, 1.75 and 2. `capture.py` verifies
Chrome PID/profile/window ownership, fixture readiness and actual DPR, clears
emulation, restores screen settings and closes/waits for Chrome. Run with the
owned screen's environment and a profile/output under `/var/tmp`.

28 cases / 140 observations cover solid/none paint, inheritance/currentColor,
opacity percentages/clamping, fill/clip rules, stops/flood colors, zero-specificity
presentation attributes, stylesheet/layer/inline precedence and rollback,
variables, malformed important values, animation fill exclusion and SVG versus
HTML namespace applicability. Reference capture followed the initial property
implementation and preceded native comparison. It does not establish SVG sizing,
vector raster fidelity, paint-server support or website acceptance.

Reference semantics: https://www.w3.org/TR/SVG2/styling.html#PresentationAttributes
and https://www.w3.org/TR/SVG2/painting.html . Unsupported paint-server/context
paint is separately checked as an explicit diagnostic, not a matched case.
