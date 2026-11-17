---
name: olo-hardware
description: Add first-class support for this computer's hardware to the Olo desktop - read what the machine has (display, graphics, input, sound, power, fans, radios, sensors), compare it with what Olo already supports, and implement, test and measure what is missing, as was done for the ThinkPad P1 Gen 9. Use when someone running Olo on a computer it wasn't developed on asks to make Olo work well on it, or reports hardware that misbehaves under Olo.
---
<!-- Kept up to date by Olo; changes here are replaced. Delete this line to keep your own version. -->

# First-class support for this computer in Olo

Olo is developed on few computers (a Lenovo ThinkPad P1 Gen 9 above all).
On those, everything a person touches was looked at, measured and made
right: the panel's HDR and refresh rate, both GPUs, the touchpad's palms,
the special keys, the fans, the battery's charge limit, the speakers'
profile, the infrared camera. This skill is how the same is done for the
computer you are on, by you, with its owner.

You need a clone of Olo's repository (`git clone
https://github.com/benmyles/olo`; if Olo was installed with the
Installer, clone it first) and to be running on the computer in
question, under Olo.

## The rules (they are the repository's own)

Read `CLAUDE.md` at the repository's root and `docs/HACKING.md` before
changing anything: they are not optional. In particular:

- **Leave the person's session alone.** Never reload, upgrade or install
  into it. Build with `cargo build --release --workspace`; the running
  session offers the new build in Deskbar's tray, and the person chooses
  when to Upgrade.
- **Test headless** (`tools/dev/tsession`), never on the person's
  display, and never with their real devices where a mistake costs them
  something: don't pair, unpair, re-flash, change firmware settings or
  write to `/sys` or `/etc` by hand. What needs root is an Olo Doctor
  fix (`apps/doctor`), applied by the person with a button, with an undo.
- **Measure, don't guess.** A claim about this hardware is a reading you
  took from it, kept in the commit message and the docs.
- **Everything native**: read the kernel's interfaces and speak the
  protocols in Rust, in process. No shelling out to vendor tools in what
  ships (reading with them while you investigate is fine).
- **Nothing breaks the computers already supported.** What you add is
  keyed by what identifies this hardware (below), with tests on stand-in
  file trees for it *and* the existing ones still passing.
- Scratch files go under `/var/tmp`, never `/tmp`.

## 1. Read the computer

Read, don't change. Write what you find to a scratch file as you go;
it becomes the first section of what you add to `docs/HACKING.md`.

| What | Where to read it |
|---|---|
| Which computer | `/sys/class/dmi/id/{sys_vendor,product_name,product_family,product_version,board_name,bios_version}` |
| Processor, its kinds of core | `/proc/cpuinfo`, `/sys/devices/system/cpu/cpu*/topology/`, `cpufreq/` (driver, `energy_performance_preference`), `/sys/devices/system/cpu/types` |
| Graphics | `/sys/class/drm/card*/device/{vendor,device,driver}`, each connector's `status`, `edid`, `modes`; which GPU drives which connector |
| The panel | its EDID and DisplayID (`olo_system::edid`): size, native mode, refresh rates, VRR range, HDR (static metadata, peak luminance), colour primaries; `/sys/class/backlight/*` (which one moves it, its range) |
| Keyboard, special keys | `/proc/bus/input/devices`; each key's code with `libinput debug-events` or `evtest` read-only: brightness, volume, microphone mute, keyboard light, display switch, airplane mode, the Fn row; LEDs under `/sys/class/leds/` |
| Touchpad, pointing stick, touch screen, pen | `/proc/bus/input/devices` (names, properties), `libinput list-devices`; whether the pad reports pressure and contact size (`ABS_MT_PRESSURE`, `ABS_MT_TOUCH_MAJOR`); its size and resolution |
| Sound | `/proc/asound/cards`, `pw-dump` (cards, profiles, which is chosen), whether the speakers need a UCM profile or SOF firmware; the microphone's mute LED |
| Battery and charging | `/sys/class/power_supply/*` (charge thresholds: `charge_control_end_threshold`), `/sys/firmware/acpi/platform_profile{,_choices}` |
| Fans, temperatures | `/sys/class/hwmon/*/name` and what each exposes; the vendor's ACPI driver (`thinkpad_acpi`, `asus-nb-wmi`, `dell-smm-hwmon`, `hp-wmi`, `ideapad_laptop`, `applesmc`/Asahi's `macsmc`) |
| Sleep | `/sys/power/mem_sleep`, `/sys/power/state`; what wakes it (`/proc/acpi/wakeup`) |
| Radios | `/sys/class/net/*/device/driver`, `rfkill list`, Bluetooth's controller (`/sys/class/bluetooth`) and its firmware in `journalctl -k` |
| Camera, sensors, readers | `v4l2-ctl --list-devices` (an infrared camera for face unlock?), a fingerprint reader (`lsusb`, fprintd's supported list), an ambient light or presence sensor (`/sys/bus/iio/devices/`), a lid and tablet-mode switch |
| Ports | Thunderbolt/USB4 (`/sys/bus/thunderbolt/devices`), what a dock adds |
| What the kernel said | `journalctl -k -b` for firmware it couldn't load, devices that failed, ACPI errors |
| What Olo already says | `olo-doctor --report` (its findings on this computer, with no window) |

## 2. Compare with what Olo does

For each thing the computer has, find how Olo treats that kind of
hardware and whether this computer's is covered. `docs/HACKING.md`'s
sections are the map; read the ones that apply in full:

- **The panel**: "Fractional scaling", "A display that leaves without a
  word", HDR and the panel's own decoding (search "HDR"), variable
  refresh (search "VRR"), "Night light", the backlight.
- **Graphics**: "Each display on its own GPU" (a laptop with two), the
  GPUs' tuning and power in "Power".
- **Keyboard**: "Keyboard profiles", "Key repeat", "Desktop shortcuts and
  tiling" (the special keys: search "KEY_MICMUTE", "KEY_SWITCHVIDEOMODE"),
  "Keyboard hints", "Caffeinated mode and the level indicator".
- **Touchpad**: "The touchpad" and `docs/TOUCHPAD.md` (how a pad's palms
  are told from its fingers, per pad: `crates/palm`, its captured data
  in `crates/palm/data`, app_server's `touch_filter.rs`, whose
  `THINKPADS` list names a pad by its device name); "The touch screen";
  "The pointer".
- **Sound**: "Voice processing", "The voice service", the output's
  profile (search "SoundWire").
- **Power**: "Power" (the platform profile, the processors' and GPUs'
  tuning per mode, the charge limit, the power button), "Realtime
  threads and the kept core" (which cores are which), "The scheduler",
  "The kernel's settings".
- **Fans**: "The fans" (today a ThinkPad's, through `thinkpad_acpi`).
- **Radios**: "The Wi-Fi and Bluetooth healer" (today for chips that
  need it), "AirPods", "The network changing".
- **Signing in**: "Face unlock" (an infrared camera), "Security keys",
  the fingerprint reader (Preferences ▸ Security ▸ Sign In).
- **Everything that needs the system changed**: "Olo Doctor" and "Olo
  Doctor's suggestions": every check is a function of a file tree
  (`Roots`), so a computer is a stand-in tree in a test.

Then try it, as its owner would, and write down what is wrong or
missing. Ask them: they know what annoys them. Typical findings on a new
computer: a special key that does nothing or the wrong thing; palms
moving the pointer, or taps not registering; the wrong scale or refresh
rate at first start; HDR or variable refresh not offered; the brightness
keys moving the wrong backlight; speakers thin or silent until a profile
is chosen; fans loud, or the computer hot, in a power mode; a charge
limit Olo doesn't offer; a camera or reader Olo doesn't see; Wi-Fi or
Bluetooth lost after sleep; a dock's display not coming back.

## 3. Agree what to do

Show the person the list, most felt first, and agree which to do. Each
item is one piece of work, done to the end before the next: found,
fixed, tested, measured, documented, committed.

## 4. Do each one properly

- **Key it by the hardware.** Match on what identifies the part, not on
  "this is my laptop": a device's name or ids (as `THINKPADS` does), a
  driver's presence (`/proc/acpi/ibm/fan`), an attribute being there
  (`charge_control_end_threshold`), the panel's EDID, DMI only when
  nothing nearer says it. Where another vendor has the same thing by
  another name (a platform profile, a charge limit, fan levels), make
  the general thing and add this vendor's way to it, rather than a
  special case beside the ThinkPad's.
- **Test on a stand-in.** Copy the files you read (sysfs attributes,
  EDIDs, an input device's description, a capture of touches) into the
  test as a file tree or data file, as the existing tests do (search the
  tests for `proc/acpi/ibm/fan`, `crates/palm/data`). The test must fail
  before your change. Leave the existing computers' tests passing.
- **Captures of touches** (a touchpad's palms): record read-only, with
  the owner doing what they do (rest palms while typing, tap, scroll),
  as `docs/TOUCHPAD.md` describes; keep the capture in `crates/palm/data`
  with nothing personal in it; replay it in a test.
- **What needs root** (a kernel module's option, a udev rule, a
  modprobe.d file, a firmware package) is an Olo Doctor finding with a
  fix made of `Step`s and an undo; never a manual instruction.
- **Look at it** at 1×, 1.5× and 2×, in light and dark, with
  `tsession review` on any window you changed.
- **Measure** what runs per frame, per event or at rest before and
  after (`tools/dev/cost PID`, `perf stat --no-scale`), and say the
  numbers in the commit.
- **Document** in `docs/HACKING.md`: a section for this computer (what
  it has, as read in step 1), and each change in the section of the
  thing it changes, with the owner's own words for what was wrong and
  the date. What you learned about the machine or a tool that the docs
  don't say goes in `docs/AGENT-NOTES.md`.
- **Journal** visible changes (`tools/journal.py`), and commit each
  piece of work by itself, with what was measured.

## 5. Hand it back

Tell the owner, plainly: what now works and how you know; what you
couldn't do and why (firmware that is missing, a kernel driver that
isn't there: say which, and what would fix it upstream); what they need
to do (Upgrade from Deskbar's tray; an Olo Doctor fix to apply). If the
work would help others with the same computer, offer to open a pull
request against `benmyles/olo` with it: one per piece of work, each with
its tests.
