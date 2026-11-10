# CSS counter sequencing and scopes

The expectations are independent Chrome 157.0.8103.38 observations, recorded
on the owned `codex-browser` Agent Screen with a private profile. The JSON
contains Chrome's revision, the source SHA-256 and its full accessibility
tree. After navigation to a URL containing the source hash, the capture
waits for that URL and a completed document, then calls CDP
`Accessibility.getFullAXTree` and `DOM.describeNode`.

For each element with an ID, collect its generated StaticText nodes (those
without a backend DOM node), stopping at descendants with another ID.
ListMarker names are included directly. Ordinary DOM text is excluded.
`measure.rs` compares that text against Olo's source spans owned by each
originating element, removing atomic-object placeholders and trailing
whitespace. Native screenshots are separate from these text assertions.

Cases cover reset/increment/set order; duplicate declarations; nested and
sibling counter scopes; pseudo-element scopes; display:none, hidden and
contents; quotes; signed 32-bit boundaries; overflow in compound increments;
and the difference between general counter overflow and list ordinals.
Chrome saturates the compound increment, but leaves a general counter's old
value when adding it would overflow. List ordinals saturate instead. These
are observations of this Chrome version, not a claim of complete standards
conformance. Mutation and active-scope limit tests supplement the fixture.

References: [CSS Lists](https://drafts.csswg.org/css-lists-3/) and
[CSS Generated Content](https://drafts.csswg.org/css-content-3/).
Implicit list-item counter unification, reversed CSS counters, style
containment, computed-value serialization and complete marker rendering
remain open. Reading a missing counter does not instantiate one in this
Chrome fixture; keep that observation distinct from evolving draft wording.
