# Windows that snap into place as they're dragged: the study, and Olo's design

Ben, 2026-11-18: "add support for window "snapping" when dragging so
that e.g. if i drag to different areas of the screen it takes that tile
location while dragging but isn't "saved" until i let go, e.g. like if i
drag to one of the corners it shows it in that corner, if i drag to the
top half in the center area that'd be top half, very top of the screen
anywhere should be full screen, over another window should take its
place. make it intuitive, look at how other operating systems handle it.
windows should smoothly animate when they move not jarringly just redraw
somewhere else. also, when dragging, show each hotspot as a
semi-transparent overlay with a letter in each. the user can drag to
that hotspot and let go, or they can just press the letter and it'll be
like they dragged it there."

## How the others do it

- **Windows 11.** A window dragged to the left or right edge takes that
  half; to a corner, that quarter; to the top edge, the whole screen. As
  the pointer reaches the place a see-through outline of where the
  window would go grows from it, and letting go puts it there; dragged
  on, nothing happens. Dragged towards the top middle, a small bar of
  layouts drops down (Snap Layouts), each a few zones to drop on. After
  a snap, Snap Assist offers the other windows for the room left. A
  snapped window dragged away goes back to the size it had. Win+arrows
  do the same from the keyboard, and Win+Z shows the layouts with a
  number on each zone to press.
- **macOS Sequoia.** The same edges (left and right halves, corners
  quarters, the top fills the screen), shown as an outline before the
  window is let go; with Option held the outline comes as soon as the
  window nears an edge. Dragged away, a tiled window is its old size
  again. The green button's menu and fn-Control-arrows for the
  keyboard.
- **GNOME.** Top: whole screen; left and right: halves; a see-through
  rectangle that grows to the place. No quarters. Dragged away, the old
  size.
- **KDE Plasma.** Halves, quarters and the top as Windows has them (how
  much of an edge is a corner can be set); with Shift held, a layout of
  tiles the person drew is shown and the window dropped into one.
- **Rectangle** (for the Mac, before Sequoia had any). The same edges,
  thirds along the bottom edge, a "footprint" shown before letting go;
  every place also a key.
- **FancyZones** (Windows' PowerToys). With Shift held, every zone of
  the person's layout is shown see-through with its number; the one
  under the pointer lights up; let go, the window fills it.
- **BeOS** had none of it. **Haiku's Stack and Tile**: with the Option
  key held, a window dragged against another's edge sticks to it (they
  move and resize together from then on), and one dropped on another's
  tab stacks with it; the edges that would join light up.

What they agree on, and Olo keeps: the edges and corners of the screen
are the places (nobody has to learn them); what will happen is shown
before the window is let go; letting go is what makes it so; dragged
away again, the window is as it was. Where they differ from what Ben
asked: every one of them shows an outline and leaves the window under
the pointer, and none letters its places while dragging (FancyZones
numbers them, to look at; Windows' numbers are a different, keyboard
way in).

## Olo's

**The places.** Nine, a map of the screen on the nine keys at the
keyboard's left, and one for each other window:

    Q  top left quarter      W  top half          E  top right quarter
    A  left half             S  the whole screen  D  right half
    Z  bottom left quarter   X  bottom half       C  bottom right quarter

The letters are keys' places, as keyboard hints' are: the same keys on
every layout, whatever they type there. The other windows on the display
take the right hand's keys (J K L H U I O P …), the one in front first.

**Where to drag.** Each place has a pad, a small plate with a picture of
the screen, the place filled in, and its letter on a key cap as keyboard
hints draw theirs; the pad is where the pointer goes: at
the four corners; at the middle of the left and right edges; at the top
in the middle, the top half; and the very top of the screen along its
whole width between the corners, the whole screen (a bar, as Ben asked:
"very top of the screen anywhere"); at the bottom in the middle. A pad
at the screen's edge counts out to the edge itself, where the pointer
stops. Another window's pad is at its middle, not the whole of it (a
window dragged across a screen of windows passes over others all the
way, and must not jump into each), and says whose it is: a small window
and that window's title.

**What's seen.** The pads come up as the drag begins (past the few
points that tell a drag from a click) and go as it ends. While the
pointer is in a pad, the window itself is in that place, as Ben asked,
not an outline of it: it glides there and takes the size, and the other
pads step back to their letters alone, so the screen shows whole. Out of
the pad again, it
glides back under the pointer at its own size. Let go in a pad, it stays
(tiled there: Restore, or dragging it away, gives it its own size back,
as any tiled window); let go anywhere else, it stays where it was let
go. Escape puts everything back as it was when the drag began. A letter
pressed is the window let go in that pad.

**Another window's place.** The dragged window takes that window's
frame (and its tile or its zoom, if it has one), and while the pointer
is on the pad the other is shown where the dragged one was when the
drag began, glowing as a new window can. Let go, and the two have
swapped: nothing is covered and nothing lost.

**Which windows.** Those that tile: not dialogs, alerts, windows of a
fixed size or full screen. A window in an auto-tiled display's tree
keeps what dragging does there already (onto a tile: swap; onto a
tile's edge: beside it). A finger's drag on a touch screen snaps as the
pointer's does.

**Gliding.** A window put somewhere at the person's word (a place taken
while dragging, a drop, a swap, Escape; a tiling shortcut, Zoom,
Restore, Centre) goes there in a little over a quarter of a second,
most of the way at once and easing in, not in one frame. It's the window
itself at each size on the way, drawn by its app, as when its edge is
dragged: nothing is stretched (Olo never resamples a picture). Each step
is where the glide has got to by the clock, so an app that draws slowly
takes fewer steps and arrives on time. What arranges many windows at
once (auto-tiling, Tile All) still shows them all in one frame.

**For agents.** `{"type":"snap"}` says the places while a window is
dragged (each pad's rectangle, letter and what it is); dragging is the
pointer's (`synthetic` button and moves), and a letter is a key.

## What the building of it found

- **The kit committed a new size before its frame.** A window given a
  size acknowledged it and set its viewport to it at once; if a frame's
  callback came before the app had laid out for the size, the
  acknowledgement was committed with the buffer before it, and the
  compositor showed that stretched to the new size until the next
  frame. A glide sends each size as a frame begins, so every step of
  one was the old picture stretched (seen in a picture taken 30 ms into
  a glide; `WAYLAND_DEBUG=1` on the app showed `set_destination`,
  `ack_configure`, `commit` with no buffer between). Fixed in the kit
  (HACKING, "A new size is committed with its frame"): a zoom's or a
  tile's first frame is right now too.
- **A zoom reached by a glide** would have kept the zoom's own frame as
  where to unzoom to (`zoom_window` takes where the window is): kept as
  it was instead (`zoom_there`).
- **See-through, as asked, didn't hold.** Ben asked for "a
  semi-transparent overlay with a letter in each". The pads were that,
  at 20%, then 80%, 92% and 95% opaque: at each, a window's words behind
  a pad read through it as if they were the pad's own (both reviewers,
  each round). A hotspot the size of a zone can be see-through; a pad
  the size of a button, over text, can't without a blur Olo doesn't
  have. So the plates are opaque (one constant, `PAD_ALPHA`, to try
  otherwise), and what's see-through is the screen: while the window is
  shown in a place, every other pad is only its letter.
- **The pads, reviewed five times by Astra 6.2 and Opus 5.5**: first
  drawn as tinted rectangles with a letter (web overlays, said Opus),
  they became raised plates with a shadow, a sunken picture of the
  screen and the keyboard hints' key cap; the window pads a small
  window of Olo's (first a folder, by both) and their window's title;
  the pad the window is in, a pressed cap; the others, while it is,
  faded (a smudge over text), then outlines (boxes struck through it),
  then their letters on a collar. The outline of where a swap's other
  window goes, drawn over everything, became that window's own glow.
- **A window at the top was the whole screen the moment it was taken**:
  its tab is inside the whole screen's bar. The pad a drag begins in
  counts only once the pointer has left it (`tools/dev/snap-check`
  found it when the bar grew four points).
