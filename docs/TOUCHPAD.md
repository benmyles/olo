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

**Where a driver tags palms itself** (`MT_TOOL_PALM`: hid-multitouch's
confidence bit, on most x86 precision touchpads), libinput leaves those
contacts out before anything else; the plugin passes the tool type on as
it came (it rewrites only sizes, pressure and the click), so that works
with Olo's filter on too. Apple's MTP driver reports no tool type
(`ABS_MT_TOOL_TYPE` isn't among event5's axes: checked 2026-11-03 in
`/sys/class/input/event5/device/capabilities/abs`), so on this pad it's
the size rule and the model.

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
- Eighteen features a contact, made by `Tracker` from each report: its
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
  to personalize it": 27 units written by hand (`baseline`), each one
  sign of a palm (large; at its largest; light for its size at an edge;
  still with others down; heavy; long; grown; at the bottom corners;
  very large; large and still; light and just landed; long at an edge;
  many down; pressed; over a side edge; resting low on the pad, still,
  long and light: a hand's heel below the finger pointing, on a tall
  pad, not at the edge and short of palm size) or of a finger (small and
  moving; small, round and young; small in the middle; fast; two small
  together; a fingertip pressing; small at its largest), five spare for
  training.
- **Judged from when it touches the glass** (2026-11-03): a contact is
  followed from its first report with some pressure, not from when the
  pad first saw it. A palm settling hovers first: in Ben's second run 15
  of 35 palms still hovered 40 ms after they appeared, some for the
  whole 250 ms window, while no finger hovered past 56 ms, so a hand
  coming down slowly (resting low on the pad while pointing, say) was
  never judged touching at all, and then stayed a finger. A pad that
  reports no pressure is touching from its first report.
- A contact is a palm once its score passes the threshold between 40 ms
  and 250 ms on the glass. Not before 40 ms (`MIN_AGE_MS`, five
  reports): in a contact's first report nothing has moved, so a finger
  landing at the edge looks like a heel over it (the baseline took 85%
  of edge scrolling fingers for palms until it waited for their speed).
  A heel hidden 40 ms late moves nothing and can't tap. Not after
  250 ms: a finger turning palm mid-gesture would stop the gesture.
- Trained on the machine by full-batch Adam on the class-weighted log
  loss, in a fraction of a second, held near where it started (the
  penalty is on how far each weight has gone from its start, not on the
  weight): a few hundred touches can't say what a contact unlike any of
  them is, so there the units keep what they were written to mean. On
  its own, fine tuning drifted until a still fingertip mid-pad scored
  9.6 against a threshold of 3 (`fingertips::a_resting_fingertip_is_a_finger`);
  from a pull of 3e-3 it's well under, and the touches kept back do best
  there.
- Inputs are held within what the model was trained on (`lo`, `hi`,
  each feature's least and most), so of a touch where it never saw one
  it says what it says at the nearest it did, not whatever its weights
  make far past it.
- The threshold is chosen so that at most 1% of the fingers kept back
  from training (every fifth contact) are taken for palms: a palm let
  through costs less than a finger lost.

**Olo's own model, made from real hands** (`olo_palm::default_model`,
`src/default_model.rs`; Ben, 2026-11-03: "make this one that you're
improving from my data the new default/untrained model"). It's the
hand-written baseline trained on every run the trainer has kept
(`make_default_model`, `cargo test --release -p olo-palm
make_default_model -- --ignored --nocapture`, which writes the file), so
it runs before anyone trains (the Touch tab's check is on by default),
training starts from it, and the person's model is kept only if it
does at least as well. Runs kept before the window started at the touch
(a bare list) are made over as the tracker follows contacts now
(`Run::on_the_glass`: the hovering frames dropped, age, growth and peaks
counted from the first touching one), and the runs are made to show
what they don't (`with_what_they_dont_show`): no exercise had asked for
a click, so in Ben's runs fingers never pressed past ~70 while resting
palms pressed 100–460, and a model of them took pressing fingers for
palms, which a click is (every click is a finger pressing past the
driver's 120; a palm can't click); and nothing touched the bottom fifth
of the pad. So each finger is made again pressing on to a click (its
pressure ramping there over 150 ms, its size a quarter larger as it
flattens), and every contact again low on the pad (a fingertip is one
anywhere, a thumb waits at the bottom to click, a palm rests there).

From Ben's two runs (197 contacts, 437 made from them), on the touches
kept back (30 palms, 96 fingers): 93.3% of palms caught, no finger lost
(the hand-written baseline 86.7% and 1.0%, the size rule 86.7% and 0%).
Each run judged by one made the same way from the other only (another
sitting's hands, the honest test): run 1 86.0% and 8.3% (its odd
fingers: two at the very top edge, three light ones on the right edge
much like run 2's edge palms, one larger than palm size), run 2 91.2%
and 0%. The runs' own touches made into clicks and low ones: no click,
no low finger and no low thumb taken; 90.9% of palms moved low caught
(the baseline 84.4%). On made-up low heels against thumbs at the bottom
(`made_low`), 76% of heels caught and no thumb; trained on some, all of
them. Two runs of one person's hands are little to go on: each run kept
from now on makes the next default better (run `make_default_model`
again).

On real hands (2026-11-03, `captures::on_real_hands`, runs made over as
above; cost = palms missed + 4 × fingers lost; Olo's own was made from
these, so the cross-run numbers above are the fair ones):

| | run 1 (dialog trainer: 43 palms, 72 fingers) | run 2 (full-screen trainer: 34, 48) |
|---|---|---|
| size rule (1600) | 69.8% caught, 1.4% lost (0.358) | 94.1%, 0% (0.059) |
| Olo's own | 90.7%, 0% (0.093) | 97.1%, 0% (0.029) |
| Olo's own, or the size (as the filter runs) | 90.7%, 1.4% (0.149) | 100%, 0% (0) |

Before (the hand-written baseline, by the runs as kept then): 72.3% and
1.2%, 97.1% and 0% with the size rule. As first written it took half of
a run's still fingertips for palms: several features' typical values
were guesses every real contact missed, its context units hardly
weighed size, and its threshold was 0.

**Where it runs:** in `touchpad.lua` itself, since the filter has to hide
a palm before libinput sees it, in the same features and arithmetic
(`palm_model_scores_as_rust_does`: Lua and Rust agree to 1e-4, a
contact hovering before it touches and bounds that bite included).
- Scoring costs about 15 µs a frame at this size (`frame_cost`, three
  contacts: 18.7 µs off, 33.9 µs on before the window started at the
  touch and inputs were held; after, 20.3 µs off (that path didn't
  change: the rest is noise), 35.3 µs on, 33.2–34.0 µs with Olo's own
  and its bounds), about 0.2% of a core at the pad's 125 Hz. A GPU
  dispatch would cost more than the whole network.
- A contact is scored only in its first 250 ms on the glass.
- With the model off, the filter is as it was.
- On app_server's main thread (`CORE_NICE`, a fast core), which the
  supervisor keeps there every pass. How long the pad's reports take to
  reach app_server, the filter and model included, is live in Activity
  Monitor ▸ Displays ▸ Touchpad (HACKING, "The touchpad's reports").

**The trainer** (apps/preferences/src/palm_trainer.rs, "Train for Your
Palm…" on Preferences ▸ Touchpad & Mouse ▸ Touch).
- **Full screen, from the keyboard** (Ben: "i can't click the buttons
  because i am using the mouse to do stuff"): `WindowOptions::fullscreen`,
  nothing clickable; Space goes on once a step has what it needs,
  Backspace starts the step over, Esc quits (twice once touches are
  gathered: they'd be dropped), R trains again from the summary and the
  try. The keys show at the bottom as `KeyHint`s; agents read them and
  press them.
- **Twelve exercises, each asking one thing** (Ben, 2026-11-03: "a few
  of them are confusing right now where it tries to ask me to do like
  two things at once"; `each_step_asks_one_thing`): rest both palms
  (10), one palm half over the left edge (5), the right edge (5), type
  with the palms down (8), move the pointer (10), scroll with two
  fingers (10), tap (10), click (6), point with a palm at the edge (3
  rests), point with your hand resting low (5 rests), click with your
  thumb at the bottom (5), your shirt against the bottom edge (5; it
  can be skipped, as a shirt may never reach the glass). Each has a
  film of hands on a laptop's deck beside the pad live. The typing one
  is a 1990s typing tutor (`typing_tutor.rs`); Return goes on there, so
  a typed space never skips the step.
- **A line under the title says what to do now**, from the hands on the
  pad (`TrainerWindow::now`): "Rest the heel of one hand half over the
  left edge." → "Hold it there…" → "Good. Now lift it."; the two that
  point while a hand rests go rest → hold → "Now point or tap with a
  finger above it: 3 more." → "Good. Now lift your hand.", so one hand
  resting and the other's touches are one guided cycle, the fingers
  learned as fingers with a palm down and the rest counted once it has
  had three. Low means landed in the pad's lowest 35%.
- **Every touch judged as it lifts** (`judge`, pure and tested): learned
  if it's what the step asks for ("A palm, resting. Good."), dropped with
  why if not ("That was a fingertip: …", "That didn't press: press
  until the touchpad clicks.", "That rested too high: …"), so nothing is
  learned under the wrong name. A ring on the pad where each touch
  lifted, green or red, fades in 1.5 s; the step's count is a row of
  dots. The line on the last touch sits in a quiet box; once the step
  has what it needs, a green banner takes its place ("This step is
  done", what's next, the key: the kit's `Banner`), as the summary's
  outcome is.
- **Training** on `background_spawn` with progress. The model is kept
  only if it does at least as well as Olo's own on the touches kept back
  (`Report::better_than_baseline`, a finger taken for a palm counting
  four palms missed). The summary says which, and whether app_server
  really has it: it asks (`Request::GetSettings`) rather than trusting
  Preferences' own copy. It shows touches gathered and how the new
  model, Olo's own and size alone did on those kept back. "Try it"
  (only for a model kept) draws each touch as the new model calls it,
  with counts. The run's touches are kept in `~/.local/state/olo/palm`
  (`Run`, `from_touch: true`) for the next default.
- The Touch tab says which model is in use: yours (when it was trained,
  on how many touches, how it did) or Olo's own (made from how many
  touches, how it did), with "Train for Your Palm…" / "Train Again…"
  and "Forget My Model" (back to Olo's own); the size rule is "Always a
  palm when as large as", with a note that the model only adds palms to
  it. Activity Monitor ▸ Displays ▸ Touchpad says which runs.
- Checked headless with `SyntheticEvent::Contacts`. The model is in
  `Settings`, so Preferences ▸ Backup's export and import carry it.

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
