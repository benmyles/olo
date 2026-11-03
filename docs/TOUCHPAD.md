# The touchpad

Why MacBook touchpads misbehaved under Olo, what Olo does about it
without touching the kernel, what a driver fix would look like, and
where the numbers in Preferences ▸ Touchpad & Mouse come from.

## The symptoms (Ben, 16" M2 Max, October 2026)

In Chrome especially:

- it thinks you are dragging when you aren't;
- a palm resting at the top right "does stuff";
- scrolling suddenly stops, or nothing responds.

## What the pad and driver do

The pad is "Apple MTP multi-touch" (`hid_magicmouse`, the Asahi/Aurora
kernel's MTP support for M2 and later). It reports 157 × 96 mm at
98 units/mm, with per-contact `ABS_MT_PRESSURE` (0–6000) and
`ABS_MT_TOUCH_MAJOR`/`MINOR` (0–5000).

Recorded read-only from the device (no grab), about 400 contacts:

| Contact | Pressure | Size (max of major/minor) |
| --- | --- | --- |
| Finger, pointing or scrolling | peaks ≥ 14, mostly 20–60; dips to ~6 while moving | ≤ 856 |
| Finger hovering just above the glass | 0 | 150–450 |
| Left palm resting | up to 462 | 3000–3800 |
| Right-edge palm (the "top right") | ≤ 12 | 640–1116 |

Four things follow:

1. **Hovering fingers are touches.** libinput picks size-based touch
   detection whenever a quirk gives a touch-size range (the stock Apple
   MTP quirk does: 150:130). A hovering finger is larger than 150, so it
   counts as a second finger, which turns pointing into scrolling or a
   two-finger gesture, and ends a scroll when it lifts.
2. **Palms are not tagged.** The driver reports every contact as
   `MT_TOOL_FINGER`. libinput's size-based palm detection (stock 1600)
   misses the right-edge palm (640–1116).
3. **The click is the sum of everything touching.** The driver makes
   `BTN_LEFT` itself, from the *summed* pressure of every contact
   (`haptic_press_threshold` 120, `haptic_release_threshold` 75,
   module parameters). A resting palm plus a light finger click; one
   recorded click was a palm alone. A click held this way, with a
   finger moving, is a drag.
4. **Major and minor come swapped.** A light finger reads major 368,
   minor 698. libinput compares *major* with the range's high end, so a
   light finger can fail to start (with the local 450:150 override in
   `/etc/libinput/local-overrides.quirks`, it must reach 450).

## What Olo does: the touch filter

libinput 1.30 added Lua plugins that sit between the kernel and libinput
and may rewrite each evdev frame. app_server writes one,
`crates/app_server/src/touchpad.lua`, with the settings filled in
(`touch_filter.rs`), into `$XDG_RUNTIME_DIR/olo/libinput-plugins`, and
loads it into its own libinput (`udev.rs` `start_libinput`). Plugins are
read once, so a change to the filter's settings starts libinput again.
Nothing system-wide changes: no quirks, no module parameters, no root.

For devices named "Apple MTP" that report pressure and touch size:

- **A touch counts only while it is on the glass:** from pressure ≥ 1
  (`touch_on`) until it has had none (`touch_off` 1) for 60 ms
  (`lift_ms`), which rides out a moving finger's dips. Hovering fingers
  report no pressure; a fast light swipe can peak at 5, so a higher
  threshold loses it. libinput's own size rule still applies after.
- **Palms are left out:** a contact that reaches size 1600
  (`palm_size`, libinput's own threshold: a finger pressed flat
  measured 1030) is a palm for as long as it lasts, even if it shrinks.
- **Left-out contacts are reported with size 0**, so libinput sees them
  hover; positions and tracking ids are untouched. Contacts that count
  are reported major-first.
- **A palm can't click:** with no palm on the pad the driver's
  `BTN_LEFT` passes as it is; with one, it counts only while the
  non-palm contacts alone press hard enough (press 120, release 75 by
  default; Light 80/50, Firm 180/115). Olo never clicks when the driver
  doesn't: a contact just short of palm size pressed 134 without the
  driver clicking, and a click nobody feels is worse than none.

What it can't do: the *felt* click (the haptic) is the driver's, at its
own summed thresholds. A palm can still make the pad buzz, but the
click it would have sent no longer arrives. Light can't make clicks
lighter than the driver's; Firm makes them firmer while a palm rests.

Tests: `cargo test -p app_server touch_filter` runs the plugin in Lua
(mlua) against scripted frames, with a stand-in for libinput's plugin
API. `examples/touch_filter_probe.rs` loads plugins into a libinput path
context on a real device (root, read-only) and prints what comes out.

## Getting pressure to the plugin (27 October 2026)

Turned on in Ben's session the first time, the filter made the pad dead:
libinput never handed the plugin a pressure, so no touch ever started.
libinput 1.31 drops pressure on pads it takes for *pressure pads*
(`tp_init_pressurepad`, evdev-mt-touchpad.c): those with
`INPUT_PROP_PRESSUREPAD`, or whose `ABS_MT_PRESSURE` has a resolution.
The MTP driver does both (properties 0x85, resolution 1), because the pad
clicks by pressure.

Two things now let it through, both only while the filter is on:

- app_server writes a quirk into the user's own quirks,
  `$XDG_RUNTIME_DIR/libinput/olo-touchpad.quirks` (libinput reads that
  directory after the system's, at seat assignment, which comes after
  `touch_filter::install`): `AttrInputProp=-INPUT_PROP_PRESSUREPAD`, and
  `AttrPalmPressureThreshold=0` so libinput's own palm detection by
  pressure (130 by default, which a firm finger would reach) stays off.
- The plugin sets the pressure axes' resolution to 0 in
  `new-evdev-device`, which runs before the touchpad is set up.

Pressure doesn't otherwise change libinput's behaviour here: touch
detection stays size-based (a touch-size range wins), and thumb detection
by pressure needs a quirk of its own. If pressure still never arrives,
the filter passes frames through untouched (test
`without_pressure_frames_pass_untouched`), and a plugin that errors is
unloaded by libinput, leaving the pad as stock.

Checked on the real pad (`touch_filter_probe`, read-only, while Ben used
it): the plugin saw 1306 pressure events where the device sent 1349,
same peak. With the logging plugin over 90 s, all 44 real touches counted
(the lightest peaked at 14, against `touch_on` 13) and the 5 left out
were hovers (peak 0, ≤ 1.3 mm travel). Run side by side with stock on the
same input, scrolling matched (314 vs 316 frames) and the motions the
filter left out were the hovers'.

A second side-by-side run (90 s: pointing, scrolling, clicking with a
palm resting, a palm at the top right, hovering) found:

- **Clicks are right.** All 5 clicks came through, 4 of them with one
  or two palms resting; the filter's click came from the fingers alone
  (sums 120–143), with no click lost or added.
- **Fast, light swipes are lost.** Two real touches never reached
  `touch_on` 13: one peaked at 11 over 33 mm in 255 ms, one at 5 over
  12 mm. Scroll frames fell from 360 (stock) to 147. Hovers peaked at
  0, occasionally 3–5, so no pressure threshold separates them from such
  swipes, and the right-edge palm (≤ 12) sits in the same range.
- **Hovers were already small.** With the local 450:150 touch-size
  quirk, stock libinput ignores most hovers by size (they measured
  48–604 here), so leaving them out by pressure gains little.
- **Palms here measured 1700–1950**, just over `palm_size` 1600; a
  palm that is still growing can count for a moment before it is
  recognised.

So `touch_on` went to 1: anything on the glass counts, and libinput's
size rule decides the rest, as stock. A short third run with that showed
the contact above (size 1576, pressure 134) making a click the driver
didn't, hence the click rule above. Ben's session has had the filter on
since 27 October 2026, 21:55, with `touch_on` 1 and the driver's own
click (`palm_proof_click` off) until a build with that rule is in.

### Frames never grow (Ben, 2026-11-01: on his lap, pointing and
scrolling "keeps stopping")

libinput takes a plugin's frame back only if it has room: the frame it
handed the plugin had 64 slots, and a frame it had held back (a plugin
before ours queued it) is a copy with none to spare. Given more, it logs
"plugin bug: too many events in frame" and keeps the frame as the pad
sent it. The filter used to add each changed contact's size after the
frame (a slot event, major, minor, and the slot put back), so with
several contacts on the pad a frame grew past 63; Ben's session logged
it 6 times in an evening. Each time, libinput took the pad's own sizes
for that frame, a light stray contact's (a sleeve, a blanket, a
resting knuckle: in a 90 s capture on his lap, small, still and light
contacts lasting 0.3–8 s) among them, while the filter believed it had
hidden them and never sent their size again: libinput saw a finger
that wasn't there until the stray left, so pointing became two-finger
scrolling and scrolls ended.

The filter now returns no more events than it was given: sizes the
frame carries are written in place, pressure (which libinput doesn't
use here) and the driver's click make room, a size the frame doesn't
carry goes in its slot's part of the frame or after it only if there's
room (major first: without one there's no touch), and what libinput
was last given for each slot is kept, so anything left over goes with
a later frame. Replaying the capture (`touch_filter`'s `replay_capture`,
`OLO_TOUCH_CAPTURE`): the old filter returned a bigger frame than it
was given in 6151 of 7325 frames, the new one in none, and libinput's
touches match what the filter meant in every frame.

## Palms told apart by a model of the person's hands (November 2026)

The size rule (`palm_size`) misses palms that are small or light: the
right-edge palm that rests at 640–1116 while typing. So the filter can
also run a small network trained on the person's own hands (Ben: "a tiny
gpu accelerated (or simd accelerated if it's faster) model").

**Research first** (2026-11-02): there was nothing open to take.
- arXiv (searched to 2026) and Hugging Face have no touchpad palm
  model or data.
- Chromium's neural palm filter (Android's `libpalmrejection`, BSD) is
  for Chromebook touchscreens.
- ChromeOS's touchpad library goes by thresholds.
- Synaptics' model (CES 2025) is proprietary.
- A 2021 Brno thesis published no code.
- PalmTouch needs raw capacitive images, which this pad doesn't give.

**The model** (crates/palm, `olo-palm`, all Rust):
- Seventeen features a contact, made by `Tracker` from each report: its
  size and smaller size, pressure, pressure for its size, place,
  distance from the nearest edge, age, smoothed speed, the most size and
  pressure it has had, how many others are down, its shape (round or
  long), how it has grown since it landed, the distance to the nearest
  other contact (a palm sits far from the typing fingers; scrolling
  fingers sit close), its angle (`ABS_MT_ORIENTATION`, which this pad
  reports), its approaching size against its size
  (`ABS_MT_WIDTH_MAJOR`), and the seconds since a key was last pressed
  (palms rest while typing: app_server notes each key press,
  `contacts::note_key`, and says it with the contacts; the plugin
  watches the keyboards' frames itself).
- One hidden layer of 32 units: 641 weights. Ben (2026-11-02): "make
  the model even bigger and write them out as best you can for an
  optimal baseline model and then we can frame the training as optional
  to personalize it": 22 units written by hand, each one sign of a palm
  (large; at its largest; light for its size at an edge; still with
  others down; heavy; long; grown; at the bottom corners; very large;
  large and still; light and just landed; long at an edge; many down;
  pressed) or of a finger (small and moving; small, round and young;
  small in the middle; fast; two small together; a fingertip pressing;
  small at its largest), ten spare for training.
- Trained on the machine by full-batch Adam on the class-weighted log
  loss, in a fraction of a second.
- A contact is a palm once its score passes the threshold within its
  first 250 ms; after that it stays what it was (a finger turning palm
  mid-gesture would stop the gesture).
- The threshold is chosen so that at most 1% of the fingers kept back
  from training (every fifth contact) are taken for palms: a palm let
  through costs less than a finger lost.
- On made-up MacBook-like contacts it catches the light edge palms the
  size rule misses, with under 5% of fingers lost on contacts it never
  saw (`learns_palms_without_losing_fingers`).

**Olo's own baseline** (`olo_palm::baseline`, Ben: "a baseline model by
directly writing out the weights … so that the user training is really
fine tuning"): the same network with weights written by hand from the
measurements above, each hidden unit one sign of a palm (large now,
large at its largest, light for its size near an edge, still with others
down, heavy and large) or of a finger (small and moving, small for its
pressure), with three spare units for training to fill. Untrained it
catches over 85% of the synthetic palms (the size rule's light edge
palms included) and loses under 10% of fingers
(`the_baseline_beats_the_size_rule_untrained`). The filter runs it when
the option is on and nothing is trained yet; `train()` starts from its
weights (taken to the samples' scale) with a small learning rate, so a
few of the person's samples refine it rather than start over.

**Where it runs:** in `touchpad.lua` itself, since the filter has to hide
a palm before libinput sees it, in the same features and arithmetic
(`palm_model_scores_as_rust_does`: Lua and Rust agree to 1e-4).
- Scoring costs about 13 µs a frame at this size (`frame_cost`, three
  contacts: 19.9 µs off, 33.7 µs on at 18 features and 32 units;
  20.8 µs before any model; the 10-unit first try was 22.3 µs), about
  0.17% of a core at the pad's 125 Hz. A GPU dispatch would cost more
  than the whole network.
- A contact is scored only in its first 250 ms.
- With no model, or with it off, the filter is as it was.

**The trainer** (apps/preferences/src/palm_trainer.rs, Preferences ▸
Touchpad & Mouse ▸ Touch ▸ "Train for Your Palm"): optional, to
personalize the model. Five exercises (rest the palms; type with them
resting; point and scroll; tap and click; point with a palm resting),
each an Olo-style animation of hands on a pad beside the pad live (a
palm orange, a finger blue, a ring where the size rule disagrees), a
line on how it's going ("Good. 7 more.", "That looked like a finger:
rest only your palms here", by the baseline's call) and a bar; Next
once it has enough, Again to redo. Contacts come from
`EventKind::TouchContacts` on a worker that follows them with
`Tracker` and labels each as it lifts (palms-only or fingers-only
steps; in the mixed one the first down that stays still is the palm).
Training runs on `background_spawn` with progress, the model goes into
the settings (`use_palm_model` on), and a Test step draws each touch as
the model calls it, with how it did on touches kept back beside the
size rule. Checked headless with `SyntheticEvent::Contacts`. The model
is in `Settings`, so Preferences ▸ Backup's export and import carry it
with everything else.

**Its samples:** the pad's contacts as reported, from app_server
(`contacts.rs`). libinput gives out only pointer motion and gestures, and
a palm the filter hides gives nothing. So libinput's way to the
devices (`contacts::Interface`, through the session) keeps a copy of the
pad's descriptor. The loop wakes on it with each report, as it does for
libinput. Only while someone subscribes to `EventKind::TouchContacts`,
it reads every slot with `EVIOCGMTSLOTS` (the kernel's state, nothing
consumed) and sends `Event::TouchContacts` when it changed. Headless,
`SyntheticEvent::Contacts` stands in for a pad.

## Preferences ▸ Touchpad & Mouse

- **Touchpad:** the pad app_server found (`Request::InputDevices`:
  kind, size, whether it reports pressure, the driver's click
  thresholds) and the profile recommended for it. Also tracking speed,
  tap to click, tap and drag, natural scrolling, two-finger secondary
  click.
- **Touch:** the filter's thresholds and click pressure; disable while
  typing and its timeout (libinput 1.31), the pointing-stick
  equivalent (dwtp), off while a mouse is plugged in.
- **Gestures:** scrolling speed, momentum and how far it glides
  (app_server's own, `momentum.rs`), two-finger vs edge scrolling, what
  two and three fingers do (tap and clickfinger button maps), three- or
  four-finger drag (libinput 1.28), drag lock (off, timeout, sticky:
  1.27), pointer acceleration (adaptive or flat).
- **Mouse:** speed, acceleration, left-handed, middle-click emulation,
  focus follows the pointer.

Profiles set how the pad *feels* and leave personal choices (tapping,
natural scrolling, speed, two-finger click, left-handed) alone:

| Profile | For | Sets |
| --- | --- | --- |
| MacBook | Apple MTP / SPI pads | filter on, momentum, adaptive, clickfinger maps lrm |
| Magic Trackpad | external Magic Trackpads | as MacBook, filter off (not MTP) |
| Standard | everything else | libinput's own behaviour, no momentum |
| Custom | — | set as soon as any tuning is changed by hand |

## Research: what else can be tuned without a driver

- **Quirks** (`/etc/libinput/local-overrides.quirks`, root):
  `AttrPalmSizeThreshold` (people on Apple silicon report ~1000 works
  better than the stock 1600, matching what we measured),
  `AttrTouchSizeRange`, `AttrThumbSizeThreshold`. `AttrSizeHint` does
  nothing when the kernel reports a resolution. Pressure-based touch
  detection can't be chosen: any touch-size range wins, and `0:0` is
  rejected. The filter covers all of this per user, so Olo doesn't
  write quirks.
- **libinput configuration** (all exposed above): 3-finger drag (1.28),
  clickfinger button map (1.26), sticky drag lock (1.27), dwt timeout
  (1.31), dwtp, accel profiles, send-events "disabled on external
  mouse", scroll method, middle emulation, left-handed.
- **Acceleration and scroll distance** on large pads: people tune
  speed down slightly and scroll distance per mm; Olo has scrolling
  speed and glide for that.
- **Module parameters** (`/sys/module/hid_magicmouse/parameters`,
  root): the driver's haptic thresholds. Olo shows them read-only.

## The proper fix, in the driver (not done)

Ben decided against carrying a kernel patch for now. For when someone
takes it upstream (Asahi `hid_magicmouse` MTP support):

1. Report `MT_TOOL_PALM` for contacts larger than a palm threshold (or
   with the firmware's own palm flag, if the MTP report has one), so
   every compositor gets palm rejection.
2. Make the click (and haptic) from the strongest finger, or the sum of
   non-palm contacts, not the sum of everything touching.
3. Report touch major ≥ minor.
4. Optionally drop contacts with zero pressure (hover) or report them
   with `ABS_MT_DISTANCE`, so libinput can use pressure-based touch
   detection.

## On this machine

The local `/etc/libinput/local-overrides.quirks` section
`[Apple MTP touchpad hover]` (`AttrTouchSizeRange=450:150`) was an
earlier attempt at hover. The filter now does that job by pressure, and
reports sizes major-first, so the override is harmless but no longer
needed. Removing it (root) puts libinput back on the stock 150:130.
