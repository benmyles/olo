# List numbering

`chrome.json` comes from Chrome 157.0.8103.38 on the owned `codex-browser`
Agent Screen, using a private profile in `/var/tmp/codex-browser-resume`.
It records the revision, source SHA-256, accessibility marker names and
reflected DOM values. Expectations were captured independently of Olo.
After navigating to `lists.html` and waiting for the matching URL and
`document.readyState === 'complete'`, enable CDP Accessibility and call
`Accessibility.getFullAXTree`. For each `listitem`, use `DOM.describeNode`
to identify its element and record its `ListMarker` children's names.
The capture script is retained in the measurement artifact.

`measure.rs` compares the marker numbers, ignoring trailing whitespace
(Chrome's AX space versus Olo's NBSP) and atomic-inline object placeholders.
Cases cover reversed/default/explicit starts, value overrides, signed HTML
integer prefixes and overflow, saturation, hidden markers versus hidden
items, nested lists, wrappers, display:contents, inline and atomic inline
items, and list-item counter increments/sets. Mutation tests independently
exercise both style reuse paths and insertion/removal. This is numbering
coverage, not pixel equivalence or complete CSS counter semantics.

`crates/js/tests/list-reflection.js` was evaluated in the same Chrome.
Its JSON result is beside it; it checks default/invalid values, prefixes,
signed long conversion and setter attribute mutations. In particular,
`ol.start` defaults to 1 even when reversed numbering begins elsewhere.

References: [HTML integer reflection](https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#reflecting-content-attributes-in-idl-attributes),
[HTML lists](https://html.spec.whatwg.org/multipage/grouping-content.html#the-ol-element),
[CSS Lists](https://drafts.csswg.org/css-lists-3/).
Full CSS counter scoping/reset/presentational-hint precedence, generated
marker content/images, style containment, writing modes and marker spacing
remain separate work.
