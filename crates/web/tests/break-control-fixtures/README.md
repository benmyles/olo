# Semantic inline break measurements

Generate authored inputs with `generate.py`, run `serve.py` with an owned output
folder, and open its localhost URL in an owned Olo Agent Screen via `olo-chrome`.
Capture at live scales 1, 1.25, 1.5, 1.75 and 2. The probe measures DOM Range
letter positions and block rectangles after its local font is ready. The
reference does not import engine output. SOURCE.json pins all inputs, font,
base CSS and independent reports.

415 case/scale combinations match within one device layout unit of horizontal
DOM Range enclosure. Earlier four nonpasses were resolved by linear advance/grid
measurement and consuming adjacent zero-width controls on an overwide line.
This remains component geometry coverage, not website or full pixel acceptance.
