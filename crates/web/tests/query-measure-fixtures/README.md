# Container query measurement and paint geometry

`chrome.json` records 50 independent Chrome observations: ten sequential
states at five actual live device scales (1, 1.25, 1.5, 1.75, 2). Each has
four border-box coordinates and computed right padding for fourteen elements.
HTML/case hashes and the Chrome version accompany the observations.

The fixture covers two named container breakpoints, resizing in both
directions, renaming and removing containers, restoring them, padding/border
content-box sizing, size containment and display:none. A six-level named
query cascade requires more than three corrective layouts and must also
settle when its root shrinks. These are component checks, not website or
pixel acceptance, shadow-tree coverage or complete CSS Conditional conformance.

The release tests compare both fresh and reused script measurements with
Chrome (absolute tolerance 0.02 CSS px). The paint path follows the same
mutation sequence and checks all rectangles against the independent oracle.
The baseline fails 705 script assertions and stops the nested paint cascade
before its leaf reaches the expected height. The intermediate fix exposed
another ten failures from renaming a container without changing its width.

To recapture in Olo Chrome already running on your own Agent Screen:

```
/usr/bin/python3 capture.py --screen YOUR_SCREEN --port YOUR_PORT
```

The driver checks screen ownership indicators and the Chrome debugging port,
creates a background tab, changes actual display scale, then restores settings
and closes the tab. Native results never generate expected geometry.
