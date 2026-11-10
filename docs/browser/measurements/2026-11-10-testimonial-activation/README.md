# Stripe testimonial activation diagnosis

Source: `8abbff7d`, release browser SHA256
`50898dae087bd9d45d367005124de156fcb78b87abf500e6111fa8fea712c27d`.
Only the owned `codex-browser` Agent Screen was used. Public anonymous
Stripe homepage, matching experiment labels and 1000×578 viewport at 1×.
This is a script/geometry diagnostic, not a new visual acceptance check.

The earlier pointer and HTMLElement.click controls both reached the right
button. This follow-up temporarily wrapped the button's React `onClick`
property in each live page to record invocation, return and thrown errors.
Both handlers (`()=>{A(a)}`) run and return without an exception.
`handler-trace.json` was collected while Chrome was occluded: its partially
advanced scroll is not a settled comparison. The later container record
shows its final 951px horizontal offset and Jobber at x17.

Wrapping Element.prototype.scrollIntoView/scrollTo/scrollBy then showed the
same handler calls `scrollIntoView({behavior:"smooth",block:"nearest"})` on
the Jobber card in both browsers. Native's method is an empty stub at
`crates/js/src/prelude.js:686`. This identifies the missing operation rather
than an event-dispatch or React hydration failure.

A native-only diagnostic control directly called the carousel container's
`scrollTo({left:951,top:0})`. Its offset changed 0→951, the card's rectangle
changed x968→17, and the page's active button changed Mindbody→Jobber after
its scroll event. The four card x positions became [-934,17,968,1919].
This proves existing nested scrolling and the page's active-state update
work for that control. It does not fix the real button and is not a journey
pass. No injected wrapper or site-specific replacement is shipped.

Next implement the general scrollIntoView operation, with independent
Chrome controls for alignment, nested containers, clipping, viewport and
smooth-scroll cancellation. The
[CSSOM View scrolling algorithm](https://drafts.csswg.org/cssom-view/#dom-element-scrollintoview)
defines ancestor scrolling and alignment; CSS scroll margins/padding and
logical writing directions also need coverage. Native smooth scrolling is
still open. Frame work must exist only during movement, and cached geometry
must not trigger unnecessary layout. The retained container record also
shows native lacks computed scroll-snap/margin/padding properties and leaks
nested overflow into some ancestor scrollWidth values; do not use those
unverified values to choose every ancestor as a scroll container.

No engine source changed for this diagnosis, so release tests/costs remain
those of `../2026-11-10-fractional-fit/`. The screen was stopped afterwards.
Stripe 0/30, corpus 0/100; no personal desktop install or reload.
