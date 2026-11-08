# Replaced image sizing

Original two-pixel PNG inputs exercise intrinsic sizing, CSS constraints,
HTML dimension hints and the author cascade, box sizing, margins, direction
and replaced display behavior. Run `generate.py` to recreate inputs, serve
`probe.html` with `serve.py`, then collect each live native scale in an owned
Agent Screen. The five `chrome-*.json` files are independent observations.
`SOURCE.json` pins inputs, outputs, font and references.

The engine matches all 300 observations, the top/middle/bottom alignment
cases included. These are component checks, with no site
compatibility claim. The older object fixture suite independently supplies
another 380 full page box comparisons.
