# The touchpad

Why MacBook touchpads misbehaved under Olo, what Olo does about it
without touching the kernel, what a driver fix would look like, and
where the numbers in Preferences ▸ Touchpad come from.

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

- **A touch counts only while it presses:** from pressure ≥ 13
  (`touch_on`) until it has stayed below 3 (`touch_off`) for 60 ms
  (`lift_ms`), which rides out the light dips of a moving finger.
- **Palms are left out:** a contact that reaches size 1600
  (`palm_size`, libinput's own threshold: a finger pressed flat
  measured 1030) is a palm for as long as it lasts, even if it shrinks.
- **Left-out contacts are reported with size 0**, so libinput sees them
  hover; positions and tracking ids are untouched. Contacts that count
  are reported major-first.
- **The click comes from the fingers alone:** the driver's `BTN_LEFT`
  is dropped and a new one is made from the summed pressure of
  non-palm contacts (press 120, release 75 by default; Light 80/50,
  Firm 180/115).

What it can't do: the *felt* click (the haptic) is the driver's, at its
own summed thresholds. A palm can still make the pad buzz, but the
click it would have sent no longer arrives. With Light or Firm you feel
the click at the driver's pressure, and Olo counts it at its own.

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

So the filter stays off for touch detection. What does help, and needs
no threshold, is the click from the fingers alone, palms staying palms,
and major-first sizes; a mode that counts every contact (no pressure
threshold) and keeps those is the next thing to try.

## Preferences ▸ Touchpad

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
