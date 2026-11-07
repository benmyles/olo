# Rounded inset style comparisons

41 original cases at five native Wayland DPRs:205 computed-value observations.
Chrome also materializes the computed components through independent positioned
probes at percentage bases80,200,350.5px. The probes use CSS left, whose used
values are quantized to1/64px; compare with that tolerance plus .0005px CSSOM
serialization precision. Shape numeric computation itself stays unsnapped.
The first comparison exposed that probe quantization (35.05→35.0469); it was
not treated as a reason to round the engine's CSS shape values.

Covers1–4 offsets, negative/percentage/overlapping offsets,1–4 circular/elliptical
radii, calc/min/max/clamp, nonnegative radius computed clamping, font-relative
lengths and inherited computed context, all seven reference boxes, variables,
invalid-at-computed-value behavior and invalid declarations retaining earlier
values. Three authored expressions model the nav/card/toggle patterns observed
in Stripe's retained CSS. They are not a replay of Stripe's actual cascade.

The capture runs on an owned Agent Screen in isolated Olo Chrome, verifies
profile/process/window/readiness/DPR, changes actual Wayland scale without
emulation, restores settings, closes and awaits Chrome. Run capture.py with
--screen, --profile and --out under owned /var/tmp scratch. chrome.json pins the
browser revision and cases SHA-256; capture.js only reads computed shape values
and asks Chrome to resolve their numeric components through separate probes.

No clipping geometry, overlap reduction, radius fitting, reference box layout,
paint, hit testing, pixel equality or site acceptance is claimed here. The
renderer still explicitly rejects non-none clip-path values.

Sources: https://www.w3.org/TR/css-shapes-1/#funcdef-inset and
https://www.w3.org/TR/css-masking-1/#the-clip-path.
