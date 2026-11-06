# Work list

What's left before Olo is complete, highest priority first, and what
is finished, below it, most recently finished first. Keep it current:
when an item lands, strike it (with the commit) and move it to the top
of Finished; a new one goes where its priority puts it.

## Open (highest priority first)

- **Terminal: "please continue" for a coding agent that stopped** (Ben,
  2026-11-06): "when in olo terminal in a tab that has claude code or
  codex running, make an extra option available when right clicking the
  tab to enable a feature where olo will say "please continue" in the
  claude/codex session if they stop for more than 60s with no activity
  at all (any change should count as activity we just want to do this
  when they are not doing anything or processing anything or thinking
  about anything). limit it to sending up to one "please continue" per
  hour. when this mode is enabled it should add some kind of visual
  indicator in the tab too". So: the tab's context menu, on a tab whose
  shell runs Claude Code or Codex (Terminal knows: `agent_context`'s
  `own_agent`, the tab title hook), offers a check item (say "Keep It
  Going"); on, Terminal watches the tab for 120 s (Ben, after: "change
  it from 60s to 120s without activity") with nothing at all
  happening (no output, no screen change, no spinner or timer ticking,
  no input typed: any change restarts the count) and then types
  "please continue" and Return into it, at most once an hour per tab
  (Ben, a moment later: "in codex ignore `Pursuing goal (17h 52m)` when
  assessing whether anything has changed b/c that will just keep counting
  forever every minute regardless": a clock that counts on its own, as
  Codex's goal line's elapsed time, isn't activity);
  the tab shows that it's on (a small mark beside its title, in the
  palette's colours, at every scale and in light, dark and night). For
  agents too: the setting readable and settable over Terminal's
  commands socket, and the tab's state in its tree. Kept with the tab
  across an upgrade (`restored_state`); off for a new tab.

- **A YubiKey listed as a keyboard** (Ben, 2026-11-04, with a
  screenshot of Preferences ▸ Keyboard ▸ Keyboards: "yubikey is
  incorrectly showing up as a keyboard"): "Yubico YubiKey OTP+FIDO+CCID"
  under Swap Alt and the Windows Key, beside the built-in keyboard. Its
  OTP interface is a USB boot keyboard (it types one-time codes), so
  app_server's list (`udev.rs`, `keyboards`: every keyboard device with
  an A key) takes it. Security keys aren't keyboards to set up: leave
  them out (Yubico's vendor id, 1050, or a name saying so: YubiKey,
  Security Key, FIDO, U2F, Nitrokey, OnlyKey, Titan; pid.codes' and
  Google's ids are shared, so never a vendor alone there), still typing
  their codes. *Fixed* (2026-11-04: `olo_protocol::keyboards::
  security_key`, in app_server's list; `security_keys_arent_keyboards`).
  Left: Ben's look at the tab after the upgrade, the key plugged in.

- **A YubiKey approves sudo and passwords** (Ben, 2026-11-04): "add
  native yubikey support to olo -- setup should be in prefs-> security
  and once configured the yubikey should be able to approve any sudo or
  passwords request. i just plugged in a yubikey nano you can check it
  against." Preferences ▸ Security gets a YubiKey section: the key found
  (model, firmware, which applications it has: FIDO2, OTP, PIV),
  Set Up (a touch to register it, the account password once), a second
  key as a spare, Remove. Once set up, a touch of the key approves what
  asks for the account password: Terminal's sudo window, Passwords'
  unlock and `op`, polkit's dialog (Olo's agent), Approve Without
  Asking, the lock screen; the password stays the way in, and each
  window says which (touch the key, or type the password). Native, in
  Rust: CTAPHID over the key's hidraw node (FIDO2's `hmac-secret`
  extension, or the OTP application's HMAC-SHA1 challenge-response), no
  ykman or pam_u2f. One way that needs no PAM change and no root: Olo
  keeps the account password sealed by a secret only the key can give
  (`hmac-secret` of a credential made at setup, touch required) and
  hands the password on as Approve Without Asking does; the other is a
  PAM module of Olo's for sudo run outside Olo's windows. Udev's rule
  for the key's hidraw node (uaccess) is checked (Olo Doctor offers it).
  *Checked* (2026-11-04, from /sys, nothing opened): Ben's key is a
  YubiKey 5 Nano, USB-C (1050:0407 "YubiKey OTP+FIDO+CCID", firmware
  5.8, full speed), with three interfaces: OTP as a boot keyboard
  (hidraw, root's only), FIDO (hidraw, usage page 0xF1D0 usage 1:
  CTAPHID; the session's ACL already gives Ben read and write, so
  FIDO2 needs no root and no rule) and CCID (PIV, OpenPGP). So FIDO2's
  `hmac-secret` is the way in. Next: `CTAPHID_INIT` and
  `authenticatorGetInfo` (its extensions, PIN set or not), then the
  sealed-password design above.
  *Stray taps ignored* (Ben, 2026-11-04): "i have a habit of
  accidentally tapping my yubikey and it adds the code where i don't
  want it. when you add the native olo yubikey support, also make it so
  that stray taps are ignored -- the code should only be captured and
  used if we're actually in a context where we are waiting for a
  permission grant from the yubikey for sudo or passwords. also add an
  option for inserting the code anyway by using 3 taps so that someone
  could use it e.g. if they are just filling in a web form olo knows
  nothing about and want the code there. that option can be enabled by
  default too." A touch types a Yubico OTP (44 characters and Enter)
  through the key's OTP interface, a USB keyboard of its own: Olo takes
  that keyboard's keys itself (by the device, as the YubiKey is already
  told from keyboards: `olo_protocol::keyboards::security_key`) and
  passes nothing of it to the focused window. While a sudo, Passwords,
  polkit or lock-screen request is waiting for the key, a touch (FIDO2's
  user presence, or the OTP if that's how it's set up) answers it; at
  any other time a touch does nothing. Three touches in a row (within a
  couple of seconds each) type the next OTP into the focused window as
  the key would have: Preferences ▸ Security ▸ YubiKey, "Three touches
  type a code" (on by default). Each code an OTP that was swallowed was
  one the key used up; that's fine (Yubico's counter only moves on). A
  key's touches are told apart from a real keyboard's typing by the
  device, never by timing or the text.
  *Done* (2026-11-05, HACKING's "Security keys"): `olo-fido` speaks
  CTAPHID and CTAP2 to the key itself (no root, no PAM change); setting
  up in Preferences ▸ Security ▸ Security Keys (the password checked
  through PAM, two touches, a spare the same way, Remove); a touch
  answering Terminal's sudo window, Passwords' unlock, the polkit agent
  (the person's own identity) and the lock screen, each window saying
  so; stray touches typing nothing (app_server takes the key's keyboard
  by the device) and three in a row typing the code ("Three touches type
  a code", on). All of it tried headless with a software key
  (`OLO_FIDO_SOFT`), the PAM stand-in, a stand-in sudo and polkit
  helper; Ben's key asked what it is without a touch (firmware 5.8.0,
  hmac-secret, no PIN set, a key agreement in protocol 2). *Left*: Ben's
  setup with his key (its two touches), then sudo, Passwords, polkit and
  the lock screen touched for real, and a stray touch and three in a row
  in a text field; a key with a PIN that wants it (not asked for yet).
  Olo Doctor says when a key's FIDO node isn't the person's to open, and
  offers the rule (2026-11-05).
  *Setting up, with Ben's key* (2026-11-05): his first try "just kept
  flashing". With every CTAPHID message printed (olo-fido's `touch`
  example), his YubiKey 5 took both touches and gave its secret ("i was
  touching the wrong thing"). The box now says what to touch and when (its
  gold contact, once its light blinks), what to do as each touch is
  waited for, and asks again when the key stops waiting (three times).
  Left: Ben's setup in Preferences with the new box, then a touch for
  sudo, Passwords, polkit and the lock screen.

- **Face unlock: sudo and Passwords approved by a look** (Ben,
  2026-11-04): "support facial recognition as an alternative to the
  yubikey or password to unlock sudo and passwords. put the setup in
  prefs -> passwords. do whatever is needed to get olo working with it
  even if you have to write a custom driver or whatever." Set up in
  Preferences ▸ Passwords (enrol a face, a few angles, with and without
  glasses; Forget), then a look approves what the password, the
  fingerprint or the YubiKey approve now: Terminal's sudo window,
  Passwords' unlock and `op`, polkit, Approve Without Asking, the lock
  screen; each window says it's looking and falls back to the others.
  Only with the infrared camera: a photo or a screen can't pass it (IR
  shows skin and depth cues a printout and a display don't; the colour
  camera alone is too easily fooled, so never that alone). Native, in
  Rust and in process: the frames over V4L2 (no howdy, no OpenCV
  binaries, no linux-enable-ir-emitter), the emitter switched on by
  Olo's own UVC extension-unit control (found by trying the unit's
  controls with the camera's own IR stream watched for the flood
  lighting it, kept per camera), face detection and embeddings by a
  small model run in process (as Olo runs its others), liveness from IR
  (and the emitter's on/off frames), the enrolled templates sealed for
  the user only. The privacy shutter closed: it says so.
  *Checked* (2026-11-04, from /sys): the ThinkPad's camera is Chicony's
  04f2:b8b3 on USB (3-4): a colour camera ("Integrated Camera",
  /dev/video0 and 1) and an infrared one ("Integrated IR Camera",
  /dev/video2 and 3; UVC, interface 1.2), a camera HID interface (1.4:
  hidraw0) and a DFU one; Ben's account can open them (the video
  group's ACL). Next: the IR node's formats (GREY or Y8 at what size),
  its extension units' GUIDs and controls (UVC_GET_INFO, GET_LEN),
  which control lights the emitter, and what the HID interface reports
  (the shutter, perhaps).
  *Built* 2026-11-05 (HACKING's "Face unlock"): `olo-face` (V4L2, the
  emitter's UVC control found by trying, YuNet and SFace in process,
  infrared-only looks with a relief check), `olo-faced` (root, the system
  bus, the faces root's only, no password kept), `pam_olo_face.so` (sudo
  and polkit), Passwords and the lock screen asking it, and Preferences ▸
  Security ▸ Face Unlock (Install…, Set Up…, Forget…, Remove). Tried end
  to end without root or a camera (a private bus, portraits for frames, a
  stand-in PAM stack). *Left*, needing Ben at the machine: Install… (his
  administrator password), then Set Up… in front of the camera (the
  emitter found then: the read-only look found its units 10, 13 and 14,
  unit 14's control 6 the likeliest); sudo, polkit (under SELinux), the
  lock screen and Passwords tried with his face; the relief and match
  thresholds tuned on his real looks; what the camera's HID interface
  says (the shutter).

- **A USB 2 device in a Thunderbolt 5 port isn't seen** (found
  2026-11-04 with Ben's YubiKey): in the USB-C port on the Barlow Ridge
  Thunderbolt 5 controller (8086:5780 bridges, its xHCI 8086:5782 at
  0000:a9:00.0, buses 5 and 6, both root hubs runtime-suspended, the
  controller in D3hot) the key drew power (Type-C port 1 had a partner)
  but the kernel logged nothing at all, not even a failed enumeration;
  in a port on the chipset's controller (00:14.0) it came up at once. A
  SanDisk USB 3 drive had worked in that port (SuperSpeed, 6-2). Find
  whether the controller's USB 2 side misses its wake (PME) for a
  full-speed connect, whether `power/control` on (Ben's to try) or the
  USB4 router's own state brings it, and fix or work around it in Olo
  (a udev rule keeping it awake costs power: measure), with Olo Doctor
  saying so meanwhile.

- **Floating windows: the one clicked comes to the front** (Ben,
  2026-11-04): "when more than one floating "on top priority" window
  exists, one of them keeps itself on top and i can't go to the other
  floating one it just opens behind it. if we are in auto tile or focus
  mode the floating windows should probably stay on top of those
  focused/auto-tiled ones but we should be able to click on other
  floating windows to see them at front (or if another floating one
  opens then it should start at the very top). we just want it to be
  intuitive -- what you'd expect as a user". So: floating windows
  (dialogs, alerts, panels, windows floated by hand) stay over the tiles
  in Auto-Tiling and over the focused window in Focus, but among
  themselves they stack as windows do: the one clicked or activated
  (Deskbar, ⌘Tab, an agent's `activate`) comes to the front of them, and
  one that opens starts at the very top. Only a window's own children
  stay over it (an app's Find panel over its document, an alert over
  its window). Look at what re-raises the one that stays on top: the
  passes that put floats back over the tiles and the focused window
  (`keep_floating_above` in autotile.rs, `keep_floating_over_focus` in
  modes.rs, "in the order they were") and children over parents
  (`keep_children_above` in wm.rs), each run after a raise; and an app's
  floating panels (modes.rs, BeOS's floating feels). Reproduce headless
  in each mode (two floating windows of different apps, and of one app;
  click each, activate each, open a third), and keep a test; then a
  wm-fuzz check that the window just activated is the top one of its
  layer.
  *Fixed* (2026-11-04, HACKING's auto-tiling floats): seen headless with
  two Terminal windows floated by hand over a tiled one: activating the
  tile put it over both floats, and nothing put them back till a layout
  pass, which then raised only the float left under the tile, over the
  one just brought forward. Now the floats stay over the tiles (and
  Focus's window) on every raise, in their own order, all raised
  together when any is under. And a window joining a floating group
  floats with it (found by the new `auto-fuzz --floats`: it was a tile
  at the group's frame, over its neighbour). Two floats of different
  apps clicked and activated in turn, and a tile, behave; `--floats`
  seeds 1–4 pass 80 to 120 steps; 5 (step 51) and 6 (step 73) find an
  older mid-drag fault with groups (on HEAD's app_server too: a group's
  window behind showing as a tile over the one dragged), now in the
  auto-fuzz entry. An activation costs app_server the same (1.75–1.90
  ms of CPU each, 200 in a row, before and after). *Left*: Ben's go
  with the windows he had (which "on top priority" ones?), and Standard
  mode, where nothing keeps floats over others.

- **auto-fuzz: two tiles overlap as a Terminal opens** (found
  2026-11-04, there before that day's changes too): `tools/dev/auto-fuzz
  NAME --steps 80 --seed 7` on a `--session --shell` session at 3024×1890,
  1.75×, fails at step 37: "opening olo-terminal: tiles 4
  (olo.styled-edit) (8, 8, 764, 528) and 10 (olo.terminal) (610, 8, 380,
  528) overlap". Find why the new tile is laid over its neighbour's, fix
  it, and run the fuzzer long (several seeds, `--reloads`) until clean.
  *Step 37 fixed* (2026-11-04): the new Terminal answered its first
  configure with a frame at its own size (934 wide), and
  `keep_in_tile_area` pushed that inside the tiling area (x 610); it
  then drew its tile's size there, over its neighbour, till a layout
  four seconds later. A window that drew another size than it acked,
  one it can be, is left where its tile is now (`drawn_otherwise`).
  Seeds 1–3 pass 60 steps. *Step 39 fixed* too: a drag round was on
  its way as the drag was let go, and the tiles were laid out again
  (StyledEdit can't be as narrow as the drag left it), giving the
  followers new targets; the round's frames came at the drag's sizes
  and were placed by the new targets (9 drawn 876 wide, anchored on
  the new right edge: 18 px over 10, for a frame). A follower now keeps
  its last few asks and places a frame by the one it was drawn for.
  Seeds 1–5 and 7 pass 80 steps, 8 and 9 with `--reloads` too.
  *Seed 6's step 55 fixed* as well: the same for the window dragged
  (it wasn't a follower, so a frame of its last round's size, laid out
  again, was anchored on the new target's edge, 13 px off); it follows
  from where the drag put it as the drag ends, and stops following as
  a drag of it begins. Seeds 1–7 and 10–12 pass 120 steps, 8 with
  `--reloads` too. *Next*, both there before these changes and both
  around a reload: seed 13 with reloads at step 19, and seed 9 with
  reloads now and then at step 46 (settled, then moved: 4, 5 and 6 to
  the full width a moment after a drag). *Found* for seed 13 (rounds'
  log, 2026-11-04): not the drag. Window 1, a Terminal tab, came back
  from the reload only then, during the drag, and was put back where it
  was before the reload, at its saved size (`restore_placement`: x 1131,
  577 wide, asked at its first commit by `claim_placement`), though its
  tile's room (a `Kept` for it in the tree rebuilt after the restart)
  had moved with the drag; 5 ms later the layout put it in its tile
  (x 1091, 453 wide). One frame over its neighbour, out past the tiling
  area. The fix: a window coming back to an auto-tiled display whose
  tree keeps room for it asks its tile's size as it is now at its first
  commit and shows in that room, as a new window takes its tile
  (`auto_claim`), its saved place used only when there's no room kept.
  Seed 9's step 46 looks the same family: room held for a window not
  back yet (`Kept`, `RESTORING`) let go a moment after a drag, the
  tiles growing into it as the fuzzer watches for stillness; to see in
  the rounds' log, and either the room let go before the fuzzer looks
  or the fuzzer told that it's coming.
  *Fixed* (2026-11-04, evening; HACKING's auto-tiling restart): a window
  coming back claims its kept tile at its first commit, at the tile's
  size now (seed 13's step 19); a closed or away window's place, which
  took no room, comes back taking none (it held 19% of the display for
  20 s after every upgrade: seed 9's step 46, 14's step 42); a group's
  front given a tile once when several of its windows join it at once
  (a tree had `[7 / 7]`: 15's step 52); a window behind a tiled one in
  its group isn't moved by the restart check (13's step 54); and the
  fuzzer grabs an edge below the tabs (beside a tab is the desktop: 11's
  step 24). Seeds 1–16 then passed 120 steps with `--reloads` (7 and
  11 run again alone, 11 once the fuzzer grabbed below the tabs).
  *Next*: with the edges below the tabs every
  seed's drags land a little differently, and two new faults turned up
  in a sweep stopped early: seed 2 at step 67 ("reloading: settled,
  then moved") and seed 3 at step 103 (bringing a tab to the front of
  its group, then the tiles moved); then the sweep again to 16.
  *Seed 2's step 67, read* (its app_server log): the reload brought back
  29 windows, and some twenty of Terminal's came back as new windows,
  their saved places not matched at their first commit (only the first
  of each group is, by its key), so each split a tile ("a new window
  into its tile"), then folded into its group as it was put back, the
  tree churning for 0.7 s and the tiles still moving after the fuzzer
  saw them settle. *Fixed* (2026-11-04, late): those twenty were the
  Terminals racing each other (each adopted every other's shells as
  they all re-executed: olo-termd now lists a shell as its Terminal's
  while that process lives, `Shell::left_by`); and a tab put back is no
  tile of its own while its group forms (`awaiting`, `kept_for`), nor
  put back into another kept place once a hand has grouped it. Five
  Terminals and a Tracker now come back from four reloads in a row
  straight into their tiles. Seeds 1–16 with `--reloads` pass, all
  sixteen at once in 155 s (`tools/dev/fuzz-sweep`); seed 1 failed once
  under that load ("group 1's tab places are [(2, 0), (5, 0), …]": two
  windows at one tab place) and passed alone: a race to look at. Also with `--floats` (older, on HEAD too):
  seeds 5 (step 51) and 6 (step 73), a group's window behind showing
  as a tile over the window being dragged.
  *The fuzzers are Rust now* (2026-11-04, late; HACKING's fuzzers:
  `olo-fuzz auto`, floats always in, its own seeds: the Python ones
  above don't replay). A sweep of 26 seeds (`olo-fuzz sweep --confirm
  -- auto`, 120 steps; 132 s, the failures confirmed together in 107 s)
  finds ten faults, each at the same step every run. Mid-drag, tiles
  out of step or over each other: seeds 8 (step 112, two tiles 495 px
  past their neighbour for 400 ms), 10 (102), 12 (75, 1.4 s of it), 14
  (95), 19 (70), 21 (38), 23 (67) and 25 (88). A floating window under a
  tile after tiling one: 13 (112) and 18 (66). Fix them next, at the
  cause, then sweep again until a wave is clean (with `--reloads` too).
  *13 and 18 fixed* (2026-11-05): the window tiled was a Tracker alert
  (a seam drag had landed in a Tracker window and dragged a folder onto
  another: the alert said the move failed), and an alert stays over its
  app's windows as their child, so as a tile it went back over the
  floats each time they were put over it. Dialogs and alerts now float
  whatever is asked (`floats_always`). Seed 18 then crashed Tracker at
  step 81 (a band selection in its columns asked GPUI for an animation
  frame from a mouse move, outside drawing: a panic), fixed in Tracker
  and in GPUI. And the fuzzers now run in a sandbox (that drag could
  have moved the person's files). *Left*, from a wave of 50 in the
  sandbox (150 s, the failures confirmed together in 122 s), all
  mid-drag, tiles out of step or over each other: seeds 8 (step 112),
  10 (102), 12 (75), 14 (95), 19 (70), 21 (38), 23 (67), 25 (88), 28
  (101, 2.8 s of it), 44 (118), 47 (101, 236 px), 48 (94) and 50 (92).
  Nine more failed only under the load of 50 at once.
  *All thirteen fixed* (2026-11-05): a window a drag's round waited for
  could be under a float, so counted as not shown, and Smithay sent its
  frame callbacks once a second; GPUI's resize waits for one, so the
  round ran out of time (50 ms) and its neighbours went on over it for
  up to a second. Windows in a round now get theirs every frame. And
  GPUI acknowledged a resize's configure held for the next frame after
  a newer one (the drag's end): a protocol error, and app_server ended
  the Terminal ("wrong configure serial"); the held one is dropped once
  a newer one is acknowledged. A wave of 38 then: no faults (the 8
  failures the load's, or sessions past the 32 display names a runtime
  directory had: now 256). *Left*, with `--reloads` (a wave of 22): seed
  14 at step 22 (a Tracker settled, then moved, after a drag) and 16 at
  step 54 (one look out of step mid-drag). *14 fixed* (2026-11-05): 11 s
  after a restart app_server checks the windows that came back against
  what was saved, and puts back those moved since, unless moved on
  purpose; a tile floated by an agent (`set_floating` over IPC) wasn't
  counted as on purpose, so it was put back in its old tile's place.
  Floating or tiling a window now is. 16 passes by itself (the load's).
  *Seed 16 again* (2026-11-05, a wave of 24 with `--reloads`, 12 at
  once: 20 pass, three more pass alone): its step 54 is a StyledEdit
  window opening into a group mid-drag and drawn in front at a size
  the drag had moved on from (a group asks its windows for its size only
  between drags, and `mark_behind`'s wait for a window to draw applied
  only to one coming from behind). Now a window new to a group is asked
  for the group's size mid-drag too, one size on the way at a time, and
  waits to have drawn before it's the front, as one from behind does
  (`Decoration::fronted`): alone it passes 3 runs in 4. *Left*: under a
  wave's load it still shows for a look (the 100 ms wait,
  `groups::FRONTING`, runs out first), and alone it sometimes fails at
  step 68 ("ungrouping 3: settled, then moved": the two tiles left grow
  from 211 to 1064 px tall after settling) or step 83 (tiles overlapping
  for 4 looks mid-drag): races around a reload, next to look at.
  *Step 68 fixed* (2026-11-05): not a race. A place still kept from the
  restart before (for a window not back since) was saved again under
  its old slot number, which the new save had given to another window,
  so slot 4 was kept twice; one window came back into one, and the
  other held a fifth of the column for nobody until it timed out 20 s
  after the restart, the tiles beside it then growing (`SavedNode::of`:
  such a place is saved as room for its neighbours; unit-tested). Four
  runs of seed 16 with `--reloads` since: 68 passes in all; two passed
  outright, two failed at step 54 for one look of 193 (a StyledEdit
  opened before the restart showing only then, mid-drag, joining tile
  1's group and drawn at 290 px while its neighbour was where the drag
  began: the newcomer case above, still racing). *Tried* (2026-11-05):
  letting a newcomer that hasn't drawn its asked size wait 600 ms rather
  than `FRONTING`'s 100 before it's its group's front; it broke focus (a
  window opening into a group wasn't the front when the keyboard was
  given, so none had it: seed 16 failed at step 4 every run), so it was
  taken back. The fix wants the keyboard to follow such a window once
  it's fronted, or the newcomer's wait to apply only mid-drag.

- **The power mode sets the GPUs too** (Ben, 2026-11-04): "when
  changing the power mode in power prefs (and right clicking the battery
  in deskbar) it should also change the gpus to the equivalent
  power/perf setting too". Choosing Power Saver, Balanced or Performance
  (Preferences ▸ Power, or the battery's menu in Deskbar; both call
  `olo_system::power_profile::set`) also puts every GPU on the matching
  preset of Preferences ▸ GPUs (`gpu_tuning::Preset`: Cooler, Balanced,
  Fastest), applied as `olo-gpu-tune set` is, without an administrator's
  password asked each time (a polkit rule for that one action, or the
  power-profiles daemon's own change watched by a root helper). A GPU
  tuned by hand (Custom) is left as it is, or asked about once; the
  GPUs panel shows the preset the power mode chose. A mode changed
  elsewhere (the daemon, a hotkey, `powerprofilesctl`) follows too.
  *Done* (2026-11-04, HACKING's "Each GPU's tuning"): the power mode
  sets every GPU (Ben: "it should change for ALL gpus including
  integrated and all discrete as well as all cpus etc -- we want it to
  be a universal setting"), one tuned by hand too, with no way to turn
  it off; the processors and the firmware are the daemon's, and follow
  it already (all 16 checked). Done by a root service,
  `olo-gpu-follow.service` (`olo-gpu-tune follow`), installed by the
  GPUs panel's first Apply; Ben's Apply installed it (the first build's,
  which left hand-tuned GPUs alone): at Power Saver it put the Intel GPU
  on 650–1600 MHz and power saving and the NVIDIA one on 180–2160 MHz.
  *Left*: his Apply again for this build's (the panel offers it: the
  installed program is older), then Performance and Balanced tried from
  Deskbar.

- **AirPods missing from Sound's outputs** (Ben, 2026-11-04): "i
  paired my airpods and they show up properly in the input menu but in
  the sound output menu there's no airpods entry (in sound prefs)".
  Paired and connected, they're an input in Preferences ▸ Sound but not
  an output: their A2DP (or HFP) sink missing from what the output menu
  lists. *Looked at* (2026-11-04, AirPods not connected then): Sound
  lists every sink the sound server has (`audio::devices`, only Olo's
  None taken out), and the input that showed is Olo's own
  (`airpods_mic.ADDRESS`, made by olo-airpods over Apple's protocol,
  there whatever BlueZ's audio profile does). So the likely cause is
  their card with no audio profile up (A2DP failing, AAC say, or the
  profile left `off`): to see with them connected (`pactl list cards`,
  WirePlumber's log), then fixed at the cause, and Sound saying so
  (their output named, with why it isn't there) rather than nothing.

- **The ThinkPad P1 Gen 9, on the machine: everything in it first-class**
  (Ben, 2026-11-03, now running Olo on it: "olo doesn't recognize my
  builtin display. please fix." then "please do a full audit on all my
  hardware and add first-class olo support for it all", with each
  finding as a high-priority item here). The audit (lspci, lsusb,
  /proc/bus/input, sysfs, the kernel log, app_server's own report):
  - **The touch screen** (Wacom HID 53DD, Finger and Pen, on eDP-1):
    app_server maps touches to the built-in display, but GPUI has no
    `wl_touch`, so a touch on any Olo app, or on app_server's own
    frames and tabs, does nothing. Touch should work everywhere: as the
    pointer for clients without touch (and the frames), as touch for
    those with it (Chrome, GTK), with the pen as a tablet tool.
  - **The panel is an OLED** (Samsung Display ATNA60KA02, 3200 × 2000,
    239 ppi, 60/120 Hz): Display said "LCD (Olo's guess)" and RGB
    stripes. Then its HDR 600, P3 and VRR (40–120 Hz) to support.
  - **Preferences' windows didn't fit at 200 %** (Ben's screenshot of
    Display: a 658-point window scrolling a 804-point panel, its scroll
    bar against the boxes, the bottom cut off), and **the layout lint
    passed it**: a panel's own scroll view hid what ran past it, and
    the lint ran on a MacBook's screen with made-up devices. Ben:
    "make sure the linter is reliable regardless of the screen size",
    "do that for all of the prefs they should all adapt as needed", and
    "make sure all olo apps -- not just preferences -- follow this
    adaptive pattern too and make it a rule in the agent
    docs/instructions".
  - **Power modes** (Ben: "in prefs -> performance we need the ability
    to set the mode to e.g. Performance or Power Saving and that should
    change the default cpu governers and anything else relevant", then
    "just move it to the Power prefs app and then just have a button to
    open / go to power from in performance", and "by right clicking the
    battery icon in the tray"). tuned-ppd serves the power profiles API
    here (intel_pstate's energy preference, the ThinkPad's ACPI
    platform profile, tuned's settings).
  - **The built-in speakers and microphones don't exist**: SOF finds
    the cs42l43 codec and two CS35L57 amplifiers on SoundWire link 2,
    then "No SoundWire machine driver found for the ACPI-reported
    configuration" and "CODEC DAI cs35l56-ot25 not registered": no
    sound card but the NVIDIA HDMI one. A kernel gap (kernel
    7.2.11-300.fc45); Olo Doctor should say so plainly, with what to do.
  - **The NVIDIA GPU (RTX PRO 2000 Blackwell, GB206)**: nouveau's GSP
    fails ("gsp: init failed, -22"), so no driver has it, and with
    `power/control` "on" it stays powered (D0) the whole time, draining
    the battery. Doctor: let it sleep (runtime power management) with a
    button; say what's needed to use it (NVIDIA's open driver).
  - **The ThinkPad's keys**: the microphone mute key (F4, KEY_MICMUTE,
    with its LED, `platform::micmute`) does nothing in Olo; and the
    rest of the Fn row (F7 display switch, F8 airplane mode, F9–F12) to
    look at, with Fn+L/M/H (the firmware's power modes, which the power
    mode follows).
  - **The palm trainer doesn't work with this touchpad** (Ben: "the
    palm trainer doesn't work with the touchpad on this"): a Goodix
    GXTP5420 (27C6:0F90) over I2C, 133 × 86 mm, with pressure.
  - Also on it: the TrackPoint (shown as "PS/2 Logitech Mouse"),
    the IR camera (face sign-in), the proximity sensor (hid_sensor_prox
    fails to probe), the fingerprint reader (Synaptics 06cb:00f9:
    fprintd has it), the keyboard backlight and ThinkLight LEDs
    (`tpacpi::kbd_backlight`), battery charge thresholds
    (`charge_control_*_threshold`, `charge_behaviour`), Thunderbolt 5.
  - **sudo -n** with Approve Without Asking on: refused, as olo-sudo
    passed `-n` straight to sudo. Agents use `-n` so as never to wait.
  - **The dev tools on a new machine**: `tsession` needs busd, which
    nothing built (`tools/dev/busd-lenient` by hand).
  *Status* (2026-11-04), done, built and installed on the ThinkPad
  (Ben: "just do the upgrade for me this time"):
  - Touch screen: a finger is the pointer where touch isn't taken (Olo's
    apps, app_server's frames), touch where it is; `SyntheticEvent::Touch`
    and `tsession touch`/`touchdrag` for agents and tests. Since: a tap
    clicks, a move drags, two fingers scroll apps without touch, and the
    pointer hides while a finger is it (tried headless: a window dragged
    by its tab, a tap opening Activity's Processes, two fingers scrolling
    its list). Pinch too: GPUI has `PinchEvent`/`on_pinch` now (it had
    no pinches, so neither the screen's nor a touchpad's reached an Olo
    app), and Viewer zooms on it (tried headless: a picture zoomed about
    the fingers). Left: try it on the panel; the pen's pressure in
    Olo's apps.
  - The panel: Samsung's ATNA panels are OLED (`OLED_PANELS`), so no
    stripes. VRR: Preferences ▸ Display ▸ "Variable refresh rate" for a
    display that has it, off by default (tried headless, simulated).
    VRR: Ben turned it on and off, 2026-11-04, the mode following.
    HDR and wide colour (2026-11-04, HACKING's "Colour: wide gamut and
    HDR"): the panel's own primaries and luminance read from its
    DisplayID (a little wider than P3; 650 cd/m² full screen, 1600
    peak), sRGB shown accurately in its gamut by default, HDR (PQ,
    BT.2020) through Intel's eDP HDR interface (its DPCD says PQ decode,
    BT.2020 and tone mapping), Preferences ▸ Display ▸ Colour. Tried
    headless (a pretend P3 HDR screen) and the GPU's conversions read
    back on this machine. Tried on the panel (2026-11-04, Ben agreeing):
    the connector took HDR's metadata and BT.2020, but the panel's HDR
    control (DPCD 0x344) stayed 0x10, even through a full re-enable:
    Intel's driver uses an eDP panel's HDR interface only if the VBT
    asks, the EDID has HDR static metadata, or it's started with
    `xe.enable_dpcd_backlight=3`, and this OLED has neither of the first
    two. So HDR on it waits on that boot argument: Olo Doctor offers it
    ("The built-in display's HDR is off", editing the boot entries,
    /etc/kernel/cmdline and GRUB's defaults natively), Preferences says
    so, and app_server offers HDR on the panel only once started with
    it. Left: Ben to decide on the argument (the driver then sets the
    panel's brightness through that interface too, in nits), restart,
    and HDR tried again on the panel; external HDR displays (an EDID
    saying PQ) need none of this. Done besides: programs'
    own colours (`wp_color_management_v1`, tried with a test client),
    pictures converted from their profiles (`olo_images`), Viewer
    drawing in Display P3 while it shows a wide photo (GPUI's
    `Window::set_gamut`, tried headless), night light in HDR, and HDR
    video (HDR10, HLG) shown for SDR screens, and wide video (HDR's
    too) drawn in Display P3 rather than clipped to sRGB, and HDR video
    shown as HDR on a display whose HDR is on (Viewer's window in HDR10,
    its pictures PQ's in half floats; checked on a pretend HDR screen by
    `tools/dev/hdr-check`, with a real file too: Chromium's HDR10 test
    clip, its HDR said only in its stream) (2026-11-05). Screenshots
    and recordings of an HDR screen are right SDR pictures (SDR's white
    and HDR highlights white, 100 cd/m² as SDR shows it, not PQ's codes
    read as sRGB's: `tools/dev/hdr-check` checks a screenshot and a
    window's recording, 2026-11-05). Checked on the machine (2026-11-05,
    read only): wide colour is on for the OLED (its DisplayID primaries,
    Accurate) and the Studio Display (P3, Accurate), the Dell plain;
    `xe.enable_dpcd_backlight=3` is in the boot entries (written
    2026-11-04 12:32), /etc/kernel/cmdline and GRUB's defaults, but the
    kernel running started before (2026-11-04 00:35) and the driver's
    parameter can't change till it starts again (read only in sysfs),
    so app_server says the panel's HDR waits on a restart. After Ben's
    restart (2026-11-05), HDR on the panel works: it went blank at
    first, and Ben looked through each step with an HDR test card
    (`hdr_card`, the kit's example) while the panel's registers were
    tried by hand (`tools/dev/panel-hdr-check`). Found: with the
    argument the kernel drives this eDP 1.5 panel by VESA's luminance
    control, not Intel's HDR interface (that's for eDP 1.4b and older);
    the panel decodes PQ itself from the signal and its metadata,
    ignores the backlight in HDR and 0x344 written while on; it takes
    the signal's RGB as its own primaries (BT.2020's numbers looked
    washed out: now sent in the panel's own, colorimetry Default). The
    blanks came from dragging "Everything else's brightness" (each step
    a modeset: now only the frame re-encoded, 60 steps' IPC p50 1.39 →
    0.45 ms headless) and from Preferences' 15-second revert, which Ben
    didn't see (now an alert, Revert or Keep, counting down). SDR in HDR
    was dim at BT.2408's 203 cd/m²: a built-in panel's SDR white now
    follows its brightness unless set (80–600 cd/m²), and the
    brightness keys move it in HDR. Ben: "hdr looked good", SDR "about
    the same brightness now". Left: a real HDR grading (phone footage,
    Ben's) looked at.
  - Displays plugged in (2026-11-04): the Studio Display (Thunderbolt 5)
    was two displays, 5120 × 2880 and a copy of its right half: a tiled
    monitor, now shown whole through the connector offering the whole
    picture (`tiles.rs`; tried on the machine after the upgrade). The
    Dell S2425HS got no picture on the left USB-C port: that port's
    display path (the GPU's TC#1, in DP-alt) is also the Thunderbolt 5
    controller's second DisplayPort input, which the Studio Display's
    second stream holds (the kernel's Thunderbolt driver tunnels both
    of its streams even when Olo shows it whole through one). On the
    right-hand port (TC#4) it works. Lenovo's UCSI also says every
    partner is a DFP, so the kernel's `typec_displayport` refuses each
    (error -71); harmless here, the EC routes DisplayPort itself.
    Left: tell the person when a display on USB-C gets no picture,
    and why; free the Studio Display's second tunnel if the kernel
    ever lets us.
  - The touch filter never ran on the ThinkPad: libinput has no
    `BTN_MISC`, and the plugin unloaded at the first key pressed
    (fixed, with the test harness strict about evdev names).
  - At the 2026-11-04 upgrade: Chrome and xdg-desktop-portal-gnome
    crashed in GTK 4's `pointer_surface_fractional_scale_preferred_scale_cb`
    as app_server reloaded (a fractional-scale event for a pointer
    surface GTK had let go of); the NVIDIA GPU woke as the session
    reloaded; a headless app_server crashed on exit in libnvidia-eglcore's
    thread-exit destructor (its dmabuf-check thread had NVIDIA's EGL in
    it: glvnd asks `10_nvidia.json` before Mesa's). Left: each found
    and fixed.
  - Windows fit what they show: `WindowOptions::fits_content` in the
    kit (every alert and dialog, Preferences' panels, Drive Setup's forms,
    the network sheet), `TabView::grows`, whole-panel scroll views gone,
    Display side by side, Performance in two tabs; the lint on a
    1280 × 800 screen, following windows that resize themselves;
    `ui-lint --screen`; the rule in CLAUDE.md and HACKING. Left: the
    live lint's findings on every app at 1280 × 800: none (2026-11-05:
    `--screen 2560x1600` at 2×, 1920x1200 at 1.5× and 1280x800 at 1×
    all clean; the lint now opens apps at the first scale asked for
    when 2× isn't one, as 1280x800 opened at 2× was 640 × 400 points).
  - Power modes: Preferences ▸ Power (and a button there from
    Performance), Deskbar's battery menu; through tuned-ppd's power
    profiles API, following Fn+L/M/H.
  - Olo Doctor: the NVIDIA card kept awake (fixed here: D0 → D3cold),
    NVIDIA's driver to install (akmod-nvidia, PackageKit), built-in
    sound with no driver (kernel 7.2.15 and firmware updates offered),
    with progress; a look at login and every 15 minutes; its bag in
    Deskbar's tray while there's something to fix. Left: Ben to press
    Install Driver Updates and see whether 7.2.15 brings the speakers;
    else a kernel bug report for the P1 Gen 9 (17aa:235c, cs42l43 +
    2× CS35L57 on SoundWire link 2).
  - Sound: None as the output (the Sonos isn't forced on), kept across
    logins.
  - The microphone key (F4) mutes the microphone in use, shown on
    screen, and the hardware microphone under Olo's voice processing,
    so its light comes on (tried: on with the mute, off with the
    unmute). F7 opens Preferences ▸ Display, F8 is airplane mode (every
    radio, `/dev/rfkill`, shown on screen), F9 the notifications'
    history; F10–F12 (phone, star) go to the app in front.
  - Alt and the Windows key swapped per keyboard (Ben: "the alt key on
    the thinkpad is where the cmd key is on a mac", then "that needs to
    be a per keyboard option ... i have an external mac keyboard where
    it shouldn't swap"): Preferences ▸ Keyboard ▸ Keyboards. Left: try
    it on the ThinkPad's keyboard and the Mac one.
  - `sudo -n` goes through while approving without asking.
  - Passwords couldn't open ("Couldn't keep the key in the login
    keyring: … /org/freedesktop/secrets/collection/login"): this
    computer's login made no login keyring (gnome-keyring started with
    no default collection). `secrets::store` now makes it, as GNOME
    does, its password asked for. Found why nothing opens it at login:
    GDM's stack has `pam_gnome_keyring.so` only optionally, and Fedora
    45 doesn't install it with gnome-keyring (`gnome-keyring-pam`; only
    `pam_oo7.so` is there, for oo7, which isn't the Secret Service
    running). Doctor now says so and installs it ("Install the
    Keyring's Login Module"). Left: Ben to press it and log in again.
  - The palette opened on files: ⌘ Space's ⌘ (Ctrl to apps), let go
    after the palette opened, counted as a tap of Ctrl. A tap now counts
    only once Ctrl has been up since it opened; files are searched only
    while they're shown.
  - tsession builds busd itself on a new machine.
  - Deskbar crashed on its first start after the upgrade (Doctor's tray
    item uncounted): fixed, an uncounted item can't crash it again, and
    olo-session now starts a service it gave up on again when an
    upgrade comes.
  - The palm trainer on this touchpad: found why. app_server reads a
    pad's raw contacts only for Apple's (`touch_filter::DEVICES`, "Apple
    MTP"), and `contacts::Watch::new` needs ABS_MT_TOUCH_MAJOR; the
    Goodix GXTP5420 reports position, pressure, tracking id and
    ABS_MT_TOOL_TYPE (its firmware marks palms, MT_TOOL_PALM) but no
    contact size, so the trainer never hears a touch. Done: every
    touchpad libinput opens is watched, a size-less pad's contacts sized
    by their pressure, with the firmware's tool type (palm) on each; the
    trainer hears them. Left, on the pad by hand: the touch filter
    (`touchpad.lua`, Apple's thresholds, needs a size axis) on such a
    pad, so a model trained there is used there, the firmware's palm
    flag beside it.
  - The TrackPoint (the kernel's "PS/2 Logitech Mouse", not known as
    a stick): shown as the TrackPoint in Preferences ▸ Touchpad & Mouse,
    with its speed and acceleration, and scrolls while its middle button
    is held, as a ThinkPad's does. Left: try it on the stick.
  - Battery charge thresholds: Preferences ▸ Power's charge limit.
  - Thunderbolt 5: boltd runs, both domains ask first ("user") with
    the IOMMU guarding memory, so boltd lets devices in itself; Olo
    Doctor finds one plugged in and left waiting and allows it (Allow
    It: the kernel's `authorized`, root). Left: try with a dock.
  - The presence sensor (in the camera module, Chicony 04f2:b8b3, HID
    sensor hub): one instance fails to probe, but `iio:device0` "prox"
    works: `in_proximity0_raw` (someone there), `in_attention_input`
    (looking at the screen). iio-sensor-proxy exits at boot without it.
    Done: Preferences ▸ Power ▸ Presence, lock when you walk away and
    wake when you come back (off by default; watched only while on, on
    its buffer: `olo_system::presence`), and the udev rule that gives
    the session the sensor (Olo Doctor installs it). Left: try it in
    front of the machine (Ben: install the rule from Doctor, turn it
    on); dim when looking away.
  - The IR camera (face sign-in): built, see "Face unlock" above.

- **Palms read as fingertips on the ThinkPad's touchpad** (Ben,
  2026-11-04): "on the thinkpad touchpad when i try to do the palm
  training it just thinks my palms are fingertips and only seems to be
  registering a small dot when my palms are on the touchpad". The pad is
  a Goodix GXTP5420 (I²C HID, 27C6:0F90, `/dev/input/event6`), and its
  axes (from /proc/bus/input/devices, ABS=6e0800001000003) are X, Y,
  pressure, and for each contact slot, position, tool type
  (`ABS_MT_TOOL_TYPE`), tracking id and pressure (`ABS_MT_PRESSURE`):
  no contact size (`ABS_MT_TOUCH_MAJOR`/`MINOR`, `ABS_MT_WIDTH_*`),
  which the trainer draws and the palm model reads (the Mac's pad gives
  it), so every contact is a point. Next: see what it sends for a palm
  (an evdev recording of palms, fingertips and a flat thumb: whether the
  firmware marks palms `MT_TOOL_PALM` itself, as libinput expects of
  such pads, what pressure a palm gives, and whether a palm comes as
  several contacts close together); then make the model and the trainer
  work from what a pad has (the tool type where it says palm, pressure
  and the contacts' spread where there's no size), the trainer drawing
  a contact by what's known (a pressure-sized ring, not a dot) and
  saying when a pad can't tell palms by size; and the built-in model
  for pads like this one.
  *Built* (2026-11-05, docs/TOUCHPAD.md "The ThinkPad's touchpad"):
  two recordings of Ben's hands showed it all: no size, the pressure
  the click's force (a flat palm weighs 0: why palms were dots), the
  firmware right on every palm and never on a finger, but a resting
  palm called a finger again on up to 98% of its reports (libinput
  follows it), and the kernel ending a palm and bringing it back under
  a new id. Ben: "each touchpad type should have its own specific
  custom model type and weights". Each kind of pad now has its own
  model (`olo_palm::Kind`: features, size, base model, timing, training
  and judging); the ThinkPad's is made from Ben's captures (100% of
  palms, 0% of fingers, by the firmware's word, held). The filter runs
  there (profile "ThinkPad", Preferences' recommended one for this pad):
  a palm stays a palm until it lifts, as its tool type to libinput
  (replayed: 315 flips held, no finger told a palm). The trainer draws a
  palm as large as one and a fingertip as one, judges a click by the
  pad's button, and trains this pad's kind. Left: Ben's try after the
  upgrade (choose the ThinkPad profile in Preferences ▸ Touchpad if it
  isn't already; type with palms down; the trainer).

- **The palm trainer, full screen and driven from the keyboard** (Ben,
  2026-11-02, with a screenshot of its last step, "Try it", at 2× in
  dark mode): "so the training modal/flow needs work: for one i can't
  click the buttons because i am using the mouse to do stuff. we should
  make it so that the trainer is full screen and the continue steps
  etc. are indicated by pressing the spacebar or something, and it
  needs to be clearer about what is expected for each step with clear
  feedback as to whether i did it right, and when it finishes it needs
  to show a proper summary and tell me if it actually created the
  custom model or not and then in the touch preferneces it needs to
  also show if it's using the trained model or not and stats on it and
  you need to carefully qa every window/state screen for it to make
  sure everything fits and works together and looks good". So:
  - Full screen, and never needs the pointer: Space goes on, keys for
    again, back and quit shown on screen (typing steps take their
    sentence first, so a typed space never skips a step).
  - Each step says exactly what to do, judges each try as it happens
    ("that was a fingertip, rest the flat of your hand") and shows its
    count of good tries; a step is done when it has enough.
  - The end shows a summary: whether a model was made and saved and is
    in use now (or why not), touches gathered, and how it does against
    the built-in model and the size rule on touches kept back.
  - Preferences ▸ Touchpad says which model is in use (yours, with its
    date and numbers, or the built-in one) and can forget yours.
  - The typing exercise as a 90s typing tutor (Ben, 2026-11-02): "for
    the typing portion show the user something to type (style it like
    the old school mavis beacon typing tutor from macos -- generate some
    imagery that looks just like mavis beacon showed and make it a real
    throwback for people that remember it) because otherwise it's easy
    to struggle with just thinking about what to type and it's not
    natural". A lesson strip to type, an on-screen keyboard with each
    finger's keys in its colour and the next key lit, hands on the home
    row, speed and accuracy; drawn in Olo's own style (the look, not the
    product's name, logo or the person on its box).
  - Enabling the model requires training (Ben: "in fact, change the ux
    so that enabling it requires doing the training"): only a trained
    model is used; the check is there once one is.
  *Status* (2026-11-03): done and built for Ben to try (full screen,
  keyboard, judged touches with rings and dots, the typing tutor, the
  summary confirmed with app_server, the try, the Touch tab's status
  and Forget My Model; walked through headless with synthetic touches at
  2× dark). Found on the way: a w's points spilled out of its glyph's
  bitmap when emboldened (a grey box round the letter, any app), fixed
  in GPUI. Left: QA at 1 and 1.5×, light and night, and the trainer's
  screens as `ui_lint` tests with the new checks.
  - QA every screen and state at 1, 1.5 and 2×, light, dark and night.
  *Status* (2026-11-03): every step in every state is a `ui_lint` test
  (`ui_lint_the_palm_trainer`: 36 states, at 1280 × 800 and 1512 × 982,
  1, 1.5 and 2×, light and dark; night is app_server's warm tint, not
  a palette): clean. Then reworked (Finished: "A palm resting low…"):
  twelve exercises, each asking one thing, a "now" line, a banner when
  a step is done. Left: Ben's go on his pad with the new build (his old
  model stays in use until he trains again).

- **The touchpad's latency on Ben's machine, and what next** (from the
  palm model in Activity, 2026-11-03): Activity ▸ Inputs ▸ Touchpad
  now shows how long the pad's reports take to reach app_server (the
  filter and palm model included) and the main thread's scheduling;
  only a real pad says it, headless sessions have none. If its 99th
  percentile is over 4 ms (red: touches waiting on frames being drawn),
  give libinput a thread of its own, realtime (it never draws, so
  rtkit's 200 ms limit is safe there, unlike the main thread, which
  compiles shaders), handing events to the main loop; measure before
  and after. Also live counts from the plugin (contacts judged, palms
  left out, its own time a frame), which it doesn't report today.
  *Measured* (2026-11-03, Ben's session, read over IPC: `Request::Frames`):
  1222 reports in the last minute, 27 µs typically, 210 µs at the 99th
  percentile, the longest 96 ms. So no thread of its own: the 99th is a
  twentieth of the line. Left: what held the main thread 96 ms that once
  (`timings` while it happens). The live counts are done (2026-11-04):
  while Activity's Inputs section shows, it follows the pad's contacts
  (`TouchContacts`, which app_server reads only while someone asks) and
  judges them as the filter does, the same model and size rule
  (`apps/activity/src/palms.rs`): "Judged: N touches in the last minute,
  P taken for palms (S by their size)" and the model's time a report;
  nothing runs once the section is left (tried headless with synthetic
  contacts: a fingertip and a palm, 2 judged, 1 palm by its size, 1 µs
  a report).

- **The best baseline palm model there is** (Ben, 2026-11-02): "try to
  optimize the baseline as much as you can -- and if it would be better
  being even bigger / more features then do that but make it the
  world's best baseline palm detection model that blows away anything
  else". Today: `olo_palm::baseline()`, 14 features, 32 units, 22
  written by hand, judged only on synthetic contacts. To be the best it
  has to be judged on real hands, so:
  - Gather real captures: the trainer's samples (every labelled
    contact) saved to `~/.local/state/olo/palm/` as JSON when a run
    finishes, so they can be studied; plus the three two-minute
    manual captures from the finished item.
  - *Done* (2026-11-02): the nearest other contact, the angle
    (`ABS_MT_ORIENTATION`: the pad reports it) and the approaching size
    (`ABS_MT_WIDTH_MAJOR`) are features now, with two hand-written units
    on the nearest-contact one; 17 features, 609 weights, ~10 µs a
    frame. Ben: "you can do a great job just with hand-written but if
    you can find captures data online or anything we've tracked already
    feel free to use it": no captures online (see the research), and
    none tracked yet. Then: the seconds since a key was last pressed (app_server
    notes key presses; the plugin watches the keyboards' frames), with
    a unit for "while typing": 18 features, 641 weights, 33.7 µs a
    frame on. `MT_TOOL_PALM` where a driver gives it (2026-11-03):
    libinput leaves those out itself and the plugin passes the tool type
    on; Ben's pad reports none (docs/TOUCHPAD.md). *Left*: the
    real-capture tuning below.
  - A bigger network only if the real data wants it (64 units, or two
    layers), chosen by held-out error, never by feel; every unit still
    explained in the source.
  - The baseline's weights then set by training on the real captures
    (seeded from the hand-written ones) and written back into the
    source as constants, with its held-out numbers beside the size
    rule's in docs/TOUCHPAD.md.
  - The Lua filter follows every feature change, with the Rust/Lua
    agreement test.
  - Measured per frame as before (today ~10 µs a frame at 32 units).
  *Status* (2026-11-03): judged on real hands at last
  (`captures::on_real_hands`, Ben's two runs, docs/TOUCHPAD.md): as
  written it lost 53% of one run's fingers (scales guessed wrong, context
  units blind to size, threshold 0); now the real scales, size-gated
  units and a threshold of 2: with the size rule it beats size alone on
  both runs (72.3%/1.2% and 97.1%/0%, against 66.0%/1.2% and
  94.3%/0%). The trainer kept its runs in ~/.local/state/olo/olo/palm
  (a doubled "olo"): ~/.local/state/olo/palm from now (Ben's two are in
  the old place). Left: more runs (more sittings, other people) before
  any bigger network.
  *Status* (2026-11-03): Olo's own model is now the baseline trained on
  the real runs (`default_model`, `make_default_model`; the runs'
  fingers made into clicks and every touch made low, which they never
  showed), and it runs untrained: 93.3% of palms kept back caught, no
  finger lost; each run by one made from the other only 86.0%/8.3% and
  91.2%/0% (docs/TOUCHPAD.md). Next: run `make_default_model` again with
  each run Ben keeps (the next one has low palms, thumbs and his shirt);
  and the person's training learning from every run of theirs kept, not
  only today's (one run on its own overfits that sitting: run 1 judged
  by a model of run 2 loses 8.3% of its fingers).

- **x86 machines, starting with Ben's ThinkPad P1 Gen 9** (Ben; it
  arrives within days; **next once the GPUI migration is done**, before
  the rest of this list: "after all the gpui work is done, prioritize
  the thinkpad p1 gen9 special support next", 2026-11-01): stock Fedora, `git clone` the repo, run a setup
  app, and everything works and is supported in Olo, not just on
  the arm64 M2 MacBook Pro (Aurora). The setup app checks and installs
  what is needed (packages, Rust toolchain, the session entry, PAM,
  udev rules, keyd-style key mapping where wanted) with live progress,
  and is safe to run again. The machine: Intel Core Ultra 7 356H; 64 GB
  LPDDR5X; 2 TB PCIe Gen5 SSD (Opal); 16" 3200×2000 tandem OLED, touch,
  HDR 600 (1500 nits peak), 100% DCI-P3, VRR 40–120 Hz, factory colour
  calibration; NVIDIA RTX PRO 2000 Blackwell 8 GB (hybrid with Intel
  graphics); 5 MP RGB+IR camera (face sign-in) with dual microphones;
  Intel Wi-Fi 7 BE211 and Bluetooth 5.4; fingerprint reader; haptic
  touchpad; backlit US keyboard. All of it should work: hybrid graphics
  (app_server on the Intel GPU, the NVIDIA one for apps that want it,
  external displays on either), HDR and wide colour and the panel's
  calibration, VRR, touch screen, the haptic touchpad's gestures and
  click settings, IR face sign-in alongside the fingerprint for unlock
  and Passwords, brightness, keyboard backlight, power profiles and
  battery, suspend, and an x86_64 Chrome. Build and test x86_64 in CI or
  a VM before it arrives.
  Done so far: `tools/setup.sh` (checks by capability, installs with dnf or
  layers with rpm-ostree, Rust through rustup if it's too old, builds and
  installs; `--check` reports only); `tools/dev/x86-check` (the whole
  workspace type-checks for x86_64 from this Mac, clean); the GPU Olo
  draws with is the boot GPU (Intel on a hybrid ThinkPad;
  `ANVIL_DRM_DEVICE` overrides); touch screens, keyboard backlights
  (`*kbd_backlight`) and laptop panel brightness are already generic.
  Waiting on the machine: NVIDIA for apps and external displays, HDR and
  wide colour, VRR, the haptic touchpad, IR face sign-in.
  *Status* (2026-11-03, Ben: "update tools/setup to make sure it will
  recognize the system and do all that", for Fedora 45 on x86):
  - It says what it's on: the release (warning before 44), the
    processor, memory, free space, each GPU, and the login screen (GDM
    or SDDM). It names that screen when it's done, with how to choose
    Olo Desktop there.
  - It installs what a fresh Workstation lacks: Git LFS and its files
    (the voice model is built in from LFS, so a clone without it built
    a pointer); PipeWire's and SPA's headers; Mesa's DRI and Vulkan
    drivers and the Vulkan loader; Pillow for the dev tools.
  - Its Rust minimum is now the newest `rust-version` a crate asks for
    (1.95, filmcraft's; it said 1.88).
  - Every package and `pkgconfig(…)` it names resolves in Fedora 45's
    x86_64 repository (dnf repoquery against it). Fedora 45 has
    libdisplay-info 0.3.0, as built against here, and Rust 1.98.
  - Not run on x86 yet: that's the ThinkPad.
  - Ben, setting the ThinkPad up the same day: "the tools setup doesn't
    seem to be adding the login entry for gdm". The entry went into
    /usr/local/share/wayland-sessions, which GDM reads only when its
    XDG_DATA_DIRS names it; Fedora 45's GDM 51 daemon doesn't name
    XDG_DATA_DIRS, though it has /usr/share/wayland-sessions built in.
    Now it goes into /usr/share/wayland-sessions (atomic systems keep
    /usr/local's), and an entry in the other place is removed. The live
    layout lint, which could stop the install before the entry on a new
    machine, is left to the hooks during setup (`OLO_SKIP_UI_LINT=1`).
    Setup checks the entry is where the login screen reads it, and
    stops saying so when installing fails. Doctor looks in both places.
    Left: Ben's run on the ThinkPad.
  - brush's command model (olo-complete) computes with AVX2 and FMA on
    x86_64 (SSE2 without): its test (`simd_does_the_reference_arithmetic`,
    against the plain reference) passes on the ThinkPad (2026-11-04).
  - *On the ThinkPad* (2026-11-04): Olo runs there daily. Since: each
    display composed by its own GPU (Intel the panel, NVIDIA its HDMI
    port, which sleeps otherwise: apps, Chrome and app_server leave it
    asleep), Preferences ▸ GPUs, NVIDIA's driver through Doctor, the
    touch screen as pointer and scroll, the OLED known, power modes and
    the charge limit, the SoundWire sound once kernel 7.2.15 came, the
    ThinkPad's keys (F4 and its light, F7–F9), the TrackPoint, the
    presence sensor, Thunderbolt in Doctor. Left: in the ThinkPad item
    above (HDR, VRR, face sign-in, the haptic touchpad's palm filter).

- **Backup tried on the ThinkPad** (what's left of Ben's backup audit,
  2026-11-03; waiting on the machine): an export from this Mac (sealed,
  with passwords and sign-ins), imported on the ThinkPad (x86, another
  user name if it has one, other displays): every pane and app looked
  at, Passwords opened, the Wi-Fi networks there (NetworkManager asks
  polkit for their passwords and to save them: Olo's agent's prompts
  seen), the pool's accounts turned on once the Mac's are off.

- **AirPods and voice processing, tried on the real things** (Ben,
  2026-11-02; what's left of "AirPods, natively" and "Voice processing
  for every microphone", below): built and measured headless and in
  `tools/dev/pw-sandbox`; these need the hardware and Ben:
  - After upgrading, `sudo ~/.local/bin/olo-doctor --fix all` (Ben's to
    run): Bluetooth started with its advertisement monitor
    (`--experimental`), and rtkit given room (its 15 processes a user
    were used up, so the voice services' sound isn't realtime and their
    nice −10 was refused too).
  - Connect and switch automatically with AirPods Pro 2 and an iPhone:
    does the phone take them back by itself when it plays, or is Apple's
    smart routing needed for a handoff as seamless as between Apple
    devices (AACP 0x10 "hijack" messages, BlueZ's `DeviceID` as Apple's
    in /etc/bluetooth/main.conf; ask before touching it)? The 60 s
    no-tug-of-war rule seen working both ways.
  - Echo cancellation through the MacBook's and the ThinkPad's own
    speakers: the residual leak (−30 dB, fixed) tuned on real
    loudspeaker distortion, double talk, the delay found with real
    PipeWire buffers, and speakers and microphone on different clocks
    (render and capture frames matched one for one now).
  - The ThinkPad P1 Gen 9: the voice chain on x86_64 (AVX: tract, rustfft,
    the canceller's loops), its dual microphones, AirPods over the Intel
    BE211.
  - Calls ask the graph for a 10 ms quantum while recording: everyone's
    wakeups rise then; measure, and keep it only if it buys latency.
  - A low-latency noise model (10 ms) if a better one than
    DeepFilterNet3-LL appears (that one turns a voice down 5 dB when
    another was heard first).
  - Deskbar: the AirPods' levels from their broadcast while they're not
    connected (the service has them now).

- **Chrome scrolls slower than Olo's apps** (Ben, 2026-11-02): "in
  chrome scrolling does not seem to be the same speed as olo native
  apps (it's slower) and it's most noticeable on the studio display at
  2x where each scrollwheel push doesn't go very far at all". So: a
  wheel notch (and a touchpad's travel) moves Chrome's page as far as
  it moves a kit app's view at every scale. Found so far (2026-11-02,
  headless, Chrome 156): a synthetic notch (axis 15, value120 120, as
  libinput reports a plain wheel) moves Chrome 48 CSS px at 1× and at
  2× (devicePixelRatio 2), and StyledEdit 48 px at both (3 lines,
  `SCROLL_LINES`); Chrome takes value120 over the axis value
  (wayland_event_source.cc) and olo-wlproxy passes both through. So the
  difference is in what Ben's mouse (a Razer Viper V2 Pro) sends, or in
  his Chrome (page zoom, a flag): next, log what app_server gets from
  libinput for one of its notches (`amount`, `amount_v120`) and sends
  on, with him turning it once in each app; then make Chrome's step and
  the kit's one (Preferences ▸ Touchpad & Mouse's scrolling speed for
  both).

- **An app_server restart modesets every display** (seen 2026-11-01
  in the kernel log at each upgrade: apple-dcp `set_digital_out_mode`
  for each of the three displays, "Already set to timingID" yet done
  again: the screens freeze for a second or two per restart). The
  new app_server could keep a CRTC's mode when it is the one already
  set (the same mode blob, no ALLOW_MODESET commit) and go straight
  to page flips; where Smithay's surface init insists on a modeset,
  Olo's Smithay changes that. Until then upgrades of app_server are
  batched, not one a change. (Still: the kernel logged
  `set_digital_out_mode` 856 times in the day to 2026-11-02.)
  **Found** 2026-11-02: app_server opens each GPU with
  `DrmDevice::new(fd, true)` (udev.rs `device_added`), and Smithay's
  `disable_connectors` runs `reset_state`, switching every connector
  off: each display is modeset from dark at every start. The fix:
  open with `false`, and in Olo's Smithay let a surface take over a
  CRTC already showing its mode (`State`'s mode compared by timings,
  not the mode's type bits and name, which differ between a CRTC's
  mode and a connector's list), so the first frame is a page flip;
  switch off the planes the last app_server left on (a cursor or
  overlay plane still showing its buffer) in that first commit, no
  modeset needed; and if the driver refuses it, `reset_state` and
  modeset as now. It needs trying on the real displays (an upgrade
  the person agrees to try), so it isn't shipped blind.

- **Terminal: light mode hard to read** (*better* 2026-11-02: text kept
  at 4.5:1 from its cell, `colors::legible`, so a dark terminal's pale
  colours and the default text on bands programs draw read in light
  mode; 2026-11-03: ls, git log/show, vim, htop, btop, man/less and fzf
  run in all 16 schemes the looks offer, at 1× and 2× (headless,
  sandboxed), every one legible, vim taking light or dark colours from
  the OSC 11 answer; what's dark in light mode is the programs' own
  dark-only defaults (btop's black theme, fzf's selected line);
  light/dark reports (DEC mode 2031) so a program hears when the
  colours turn; *left*: Codex and Claude Code, which must be run on
  Ben's accounts, so by Ben) (Ben, 2026-10-31): "in light
  mode my terminal is really hard to read i'm not sure why or what's
  conflicting but can you figure out a way so that the terminal gets
  properly themed with colors that look beautiful and have high
  readability. make sure you test it with various terminal clis, tuis,
  claude code, and codex for every supported terminal theme light and
  dark and all terminal themes that come as part of the olo themes both
  the light and dark variants. every single one must look polished with
  excellent legibility and aesthetics." Seen in his screenshot (light):
  Codex's composer a mid-grey band with its placeholder and footer
  nearly invisible, its paths and dim text pale blue/grey on white, its
  added lines on a dark grey-green; Claude Code's diff on near-black
  green, its dim text and tips too faint. Likely causes to check: how
  programs learn the background (OSC 10/11/4 answers, `COLORFGBG`,
  `TERM`/`COLORTERM`), Codex and Claude Code picking dark styles
  (Claude Code's own theme setting), 256-colour greys and the bright
  colours chosen for dark backgrounds, faint (SGR 2) mixed too far
  towards the background, truecolor backgrounds a program blends from
  what it thinks the background is. So: answer the colour queries
  truly, a minimum contrast for text against its cell (as iTerm's and
  Ghostty's "minimum contrast"), faint kept legible, and each look's
  palettes tuned. Checked with ls, git diff/log, htop/btop, vim/nvim,
  less/man, tmux, fzf, lazygit, Claude Code and Codex in every terminal
  theme of every Olo look, light and dark, at 1× and 2×, as shots in
  the journal.

- **Keyboards we know, starting with Apple's Magic Keyboard with Touch
  ID (A2520), and its Touch ID as a first-class one** (Ben, 2026-11-03):
  "add special support in prefs -> keyboards for well known keyboards
  starting with the external apple keyboard A2520. model it after the
  way we do it in the mouse section but also it should be a bit more in
  depth and have it show an image of the actual keyboard and then add
  any customization options on there and, most importantly, figure out
  how to support the touch id sensor on this A2520 keyboard so that it
  works as an alternative first class touch id. if you have to reverse
  engineer it then do it -- it is plugged in at the moment". So:
  - Preferences ▸ Keyboard: a known keyboard recognised as Mouse's known
    mice are, with a picture of it (drawn to ICONS.md) and its own
    settings on it (its function row and media keys, the Fn/Globe key,
    the keypad, its battery and connection when wireless,
    what else it has).
  - Its Touch ID: a sensor the fingerprint service (and so the lock
    screen, the polkit agent, Passwords' unlock, approvals) uses as it
    uses the built-in one, enrolled and chosen in Preferences, reverse
    engineered as needed, in Rust and in process. Ben, 2026-11-03: "we
    also want the touch id sensor for it to work both when using on a
    mac and when on x86 like the thinkpad": the same on an Apple
    silicon Mac and on an x86 machine (the ThinkPad P1), so nothing of
    it may lean on a Mac's own Secure Enclave.
  - What Linux sees of it (2026-11-03, read from sysfs with it plugged in
    through the Studio Display's hub, 3-1.1): USB 05ac:029f "Magic
    Keyboard with Touch ID and Numeric Keypad", bcdDevice 0x0420, full
    speed. Interface 0 "Device Management" (HID, usbhid), 1 "Keyboard /
    Boot" (HID, usbhid), 2 "Touch ID": HID class but a single bulk IN
    endpoint (0x85, 64 bytes), so usbhid leaves it unbound; it's to be
    talked to directly (control transfers on interface 2 and that bulk
    endpoint). On a Mac the keyboard's sensor is paired with the Mac's
    Secure Enclave and matching happens there, so what it sends may be
    sealed for a Secure Enclave: find out first what it says unpaired,
    whether a template can be matched on our side at all, and say so
    before building on it.
    *Found* (2026-11-04): it can't be done. Read from the keyboard (a
    probe asking only descriptors, run once): interface 2's HID report
    descriptor is a vendor page (0xff00, usage 0x4b) with three input
    reports only, 0x20 (107 bytes), 0x21 (649) and 0x22 (62), no output
    or feature reports, and nothing sent unasked. Apple's Platform
    Security guide says how it's used: the keyboard's public key
    accelerator and the Mac's Secure Enclave pair by exchanging public
    keys rooted in Apple's certificate authority, with hardware-held
    attestation keys and ephemeral ECDH; Touch ID data between them is
    then AES-GCM with a 256-bit key; the sensor's own channel to that
    block is keyed in the factory; the keyboard stores no templates and
    matches nothing. So without a Secure Enclave Apple attests (no Linux
    machine has one, the ThinkPad nor an Apple silicon Mac running
    Linux), nothing can pair with it or read a fingerprint from it. What
    stays: the keyboard itself in Preferences ▸ Keyboard (its picture,
    function row, Fn/Globe, keypad), and Touch ID left out, said so
    there. (Apple: support.apple.com/guide/security/secf60513daa)
  - *Done* (2026-11-04): Preferences ▸ Keyboard ▸ Keyboards knows it
    (`olo_protocol::keyboards`): a box with its picture, maker and model,
    keypad and connection, and why its Touch ID isn't used. Its own
    picture (2026-11-05: `keyboard-aluminium`, in Classic and every
    theme). Its settings (2026-11-05): the top row as F-keys first or
    its printed features first (app_server, per keyboard, no root), the
    Globe key alone for emoji; its Mission Control and Spotlight keys
    the overview and the palette; and its Do Not Disturb key, which the
    kernel sends as KEY_SLEEP and logind took to suspend the computer,
    Do Not Disturb (app_server holds `handle-suspend-key`). *Left*:
    Ben's go with the keys themselves (nothing here can press them).

- **The kit takes over what every app re-implements** (Ben,
  2026-11-01: "anything these windows/apps are doing that's reusable
  should be made reusable so that we aren't reinventing the wheel
  everywhere"). A pass over the GPUI apps, each pattern moved into
  olo-ui (documented in HACKING) and every app switched to it: list +
  search + filter bar wiring (each app's matching and facets are its
  own; what's shared is a subscription, so not yet worth a kit piece).
  Done so far (2026-11-01):
  popups closing in any order, quitting taking windows down, window
  parts (`part::Part`), secrets on the clipboard (`copy::secret`);
  2026-11-03, an app's own files read and written whole (`store`:
  load, save off the thread in order, flushed on quit and upgrade),
  fourteen apps switched (six wrote straight over the file); one
  timer for the next thing due and a message that clears itself
  (`due::{Due, Notice}`: Passwords, Telegram, the only two); a
  window opened or brought forward (`open_or_raise`, `raise`: nineteen
  apps); closing with changes not saved asked the same way everywhere
  (`widgets::ask_to_save`: StyledEdit, Viewer); windows opening at
  the size they were left at (`WindowOptions::keep_size`, eleven
  apps; Passwords had its own). Found already the kit's everywhere
  (2026-11-03): single-instance launches (every windowed app forwards a
  second launch through `instance`; the rest are services, single by
  their bus name) and upgrade state (`on_upgrade`, `restored_state` in
  every app with windows to bring back; the lock screen, the polkit and
  portal dialogs, the screen saver and the supervisor have none);
  a window's content scrolling when the window is shorter than it
  (`WindowOptions::content_height`, in `Frame`: Preferences' panels).

- **Screen sharing: an Olo portal backend** (Ben, 2026-10-29; the
  design researched by another agent, session 05de188d, kept here).
  Apps never capture the screen themselves on Wayland: Chrome, Slack,
  Discord, Firefox and OBS all call the xdg-desktop-portal ScreenCast
  API; a portal backend shows a picker, the compositor sends frames as a
  PipeWire stream, and the app reads it. Chrome and Electron apps share
  WebRTC's capture code, so this is a good portal backend and a good
  PipeWire stream, plus a few app quirks, nothing app-specific.
  - **Done** (2026-10-30, HACKING.md "Screen sharing"): olo-portal is
    the session's ScreenCast backend (olo-session runs it and routes the
    portal to it); the picker (screens, live thumbnails) opens over the
    asking window; streams are PipeWire nodes fed from dmabufs app_server
    draws on the GPU only when the screen changed (0.26 ms median of
    app_server's loop at 5K); shares survive an app_server reload and an
    upgrade waits for them; restore tokens and repeat requests answer
    without a picker. Checked end to end through xdg-desktop-portal with
    `olo-portal share-test`, and with Chrome 156's getDisplayMedia (a
    sandbox Chrome on a test session): Chrome's own dialog, Entire
    Screen, Olo's picker, Chrome's live preview, Share (the second
    session Chrome opens is answered by the restore token, no second
    picker), 5120×2880 at the 30 fps asked for, and Chrome's Stop
    sharing ends the stream. Deskbar's tray shows what is shared, with
    Stop Sharing (the app is told it closed). Windows as sources
    (2026-10-30): the picker's Windows, window frames at the client's
    size, renegotiated as it resizes, ended when it closes. App scopes
    (2026-10-30): programs started from desktop entries run in
    `app-olo-<id>-<pid>.scope`, so the portal knows which program asks
    (Chrome and Slack already put themselves in scopes of their own).
    Dmabufs passed through (2026-10-30): an app that takes them gets
    the GPU buffers app_server captures into, no CPU copy (checked with
    GStreamer's pipewiresrc and the portal's own reader, a screen and a
    window, resized, across an app_server reload); the rest get memory.
    Frames carry timestamps now, and a share outlives a reload (it used
    to end, taking app_server's stop as the window closing).
  - **Build** our own backend, `olo-portal`, started and supervised by
    olo-session: it owns `org.freedesktop.impl.portal.desktop.olo` over
    zbus (which we already use), shows the picker as a kit window,
    produces the PipeWire streams, and gets frames from app_server over
    ext-image-copy-capture with dmabufs (no CPU copy), on the direct
    socket so the protocol stays hidden from outside programs. Not
    inside the compositor (niri's, KWin's, Mutter's way): a stream in
    app_server drops every share in the middle of a call at each
    upgrade; out of process it outlives app_server (the portal
    reconnects and frames carry on), and libpipewire stays out of the
    compositor, which never waits on a client. COSMIC is the precedent
    (a Smithay compositor in Rust with exactly this split: pipewire-rs,
    gbm, ext-image-copy-capture). Not xdg-desktop-portal-wlr/-hyprland
    (not native, generic pickers), nor imitating Mutter's private D-Bus
    API so xdg-desktop-portal-gnome works, as niri does (GNOME's picker,
    startup races, someone else's private API). pipewire-rs links
    libpipewire in process, as Smithay links libinput: not shelling out;
    the pure-Rust `pipewire-native` can't stream video yet, so move to
    it later.
  - **For Chrome and Slack** (from WebRTC's capture code):
    - formats: offer BGRx and BGRA first (WebRTC swaps the bytes of RGB
      formats on the CPU); offer dmabuf with modifiers, and
      shared-memory (MemFd) buffers as a fallback (Chromium and some
      Electron apps showed black screens on niri until it added that);
      handle WebRTC renegotiating when a modifier fails;
    - stream metadata: WebRTC asks for the frame header, crop, cursor
      and damage metadata; send frames only when something changed;
      all three cursor modes (hidden, drawn in, sent as metadata);
    - window shares: renegotiate the stream size when a window resizes;
      draw the window whole even when it's behind another tab in a
      group (app_server already does this for agent captures);
    - Slack's repeated pickers: Chromium opens several portal sessions
      for one share (a Slack patch author counted three, so three
      pickers; Hyprland users use a wrapper script): answer a repeat
      request from the same app, a few seconds after a choice, with that
      choice;
    - which app is asking (done): the portal leaves the app id empty
      for unsandboxed apps unless they run in an `app-<id>-….scope`
      systemd unit. Chromium and Electron put themselves in one
      (`app-com.google.Chrome-<pid>.scope`, `app-slack-<pid>.scope`);
      Olo starts other programs from desktop entries in transient
      scopes (`StartTransientUnit` over D-Bus), which also gives the
      Inhibit, GlobalShortcuts and Notification portals their app ids;
    - restore tokens: save a choice for next time (portal interface
      version 4); OBS uses it, and Chrome the kind that lasts while the
      app runs;
    - Slack must run as a Wayland client (no Xwayland; Electron 38 and
      later do by default); its drawing overlay will still misbehave,
      but sharing works;
    - Zoom's native app offers Wayland sharing only when
      `XDG_CURRENT_DESKTOP=GNOME`, so it needs a launcher quirk for its
      process; Zoom in Chrome just works;
    - sharing a Chrome tab never reaches us, and Chrome can only share a
      tab's audio, not the whole system's: audio is out of scope.
  - **Done since** (2026-11-05, HACKING "Kept out, and shown as
    shared"): Passwords, the polkit prompt, Terminal's sudo prompt,
    Deskbar's approve prompt (`WindowOptions::private`) and the lock
    screen blacked out of shares and recordings; a shared window ringed
    on screen while it is. Agent screens run their own portal (HACKING
    "A screen's own bus"), and the person's picker offers each agent
    screen as a source (HACKING "Agent screens as sources").
    RemoteDesktop done (2026-11-05, HACKING "Remote control"): the
    portal's Notify calls and ConnectToEIS, asked every time, app_server
    taking libei connections only from olo-portal after a grant, Deskbar
    showing it with Stop Remote Control; tested headless with
    `remote_test` (no real Chrome Remote Desktop or Deskflow tried yet).
    **Left**: the cursor sent as metadata (the third cursor mode; hidden
    and drawn in are done). Zoom's launcher quirk done (`olo_xdg::quirks`: Zoom's app
    started by Olo is told it's on GNOME, so it offers Wayland sharing;
    untried, Zoom isn't installed here).
  - **The Olo touches**: the picker, a kit dialog with Screens and
    Windows, live thumbnails and app icons, opened over the window that
    asked (agents read and drive it through the widget tree for free);
    while sharing, Deskbar shows what is shared and by which app, with
    Stop; a shared window gets an outline; Do Not Disturb turns on (the
    Notifications item); Passwords, the lock screen and the polkit
    prompt are blacked out of shares. Agent Screens as sources: the
    portal can connect to an agent screen's own app_server, to share an
    agent's screen into a Meet call; a working capture protocol also
    lets the Agent Screens viewer stop pulling raw-pixel screenshots
    eight times a second.
  - **Order**: 1. real capture in app_server, screens then windows
    (Smithay at our pinned rev already has window capture sources and
    the foreign-toplevel list; test headless with a small client);
    2. olo-portal with ScreenCast and the picker; 3. app scopes and
    answering repeat requests; 4. the Screenshot portal (cheap reuse of
    what we have); 5. RemoteDesktop over libei, for remote control
    (app_server already has a `libei` feature). Open question from the
    design: whether remote control (Zoom, Chrome Remote Desktop) matters
    soon, which decides how early the libei work comes.
  - Sources:
    [ScreenCast backend interface (v6)](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.impl.portal.ScreenCast.html);
    WebRTC's [screencast_portal.cc](https://chromium.googlesource.com/external/webrtc/+/master/modules/desktop_capture/linux/wayland/screencast_portal.cc)
    and [shared_screencast_stream.cc](https://chromium.googlesource.com/external/webrtc/+/master/modules/desktop_capture/linux/wayland/shared_screencast_stream.cc);
    [niri's Screencasting wiki](https://raw.githubusercontent.com/niri-wm/niri/main/docs/wiki/Screencasting.md)
    and [SHM-fallback notes](https://github.com/ljm-233/niri-portal-cast);
    [slack-on-wayland](https://github.com/hamolicious/slack-on-wayland),
    [Slack's Linux release notes](https://slack.com/release-notes/linux),
    [Electron's desktopCapturer](https://www.electronjs.org/docs/latest/api/desktop-capturer),
    [Electron PR #54550 (restore tokens)](https://github.com/electron/electron/pull/54550);
    [xdg-desktop-portal-cosmic](https://github.com/pop-os/xdg-desktop-portal-cosmic)
    ([Cargo.toml](https://raw.githubusercontent.com/pop-os/xdg-desktop-portal-cosmic/master/Cargo.toml));
    [pipewire-native](https://crates.io/crates/pipewire-native),
    [Collabora: PipeWire workshop 2025](https://www.collabora.com/news-and-blog/blog/2025/07/03/pipewire-workshop-2025-updates-video-transport-rust-bluetooth/);
    [Using portals with unsandboxed apps](https://blogs.gnome.org/ignapk/2025/06/04/using-portals-with-unsandboxed-apps/);
    [xdg-desktop-portal-wlr FAQ (Zoom)](https://github.com/emersion/xdg-desktop-portal-wlr/wiki/FAQ),
    [Zoom community thread](https://community.zoom.com/t5/Zoom-Meetings/Do-not-check-if-XDG-CURRENT-DESKTOP-is-gnome-for-wayland/m-p/108716).

- **Other programs feel like Olo's own: every integration point**
  (Ben, 2026-10-29): "is there a way where we can make chrome (and any
  gtk / qt apps) better integrate with olo, e.g. replacing their file
  picker with olo tracker picker and e.g. in chrome something like show
  in folder should show in tracker. basically every integration point
  we can do (whether cleanly through some protocol or 'dirty' through
  some hack) we should do so that everything feels as seamless as
  possible". Clean ways first (portals, D-Bus services, settings), a
  hack where there is none. Each is for Chrome, Electron (Slack,
  Discord, VS Code), GTK 3/4 and Qt 5/6 alike unless noted:
  - **File pickers** (started 2026-10-30: olo-portal answers the
    FileChooser portal with the kit's file panel, open, save and folder,
    several files at once, the app's filters by glob and MIME type as a
    Show pop-up, the portal's `choices`; Chrome's uploads, GTK 4 and GTK 3
    tried. The toolkit switches are deliberately not set for everything:
    they also send GTK's link opening through OpenURI, which asks for an
    app until one was picked 3 times (HACKING, "Open and Save"); done
    2026-10-30: the panel's list as Tracker's, Name, Kind, Size and
    Modified, sorted by a header, Favorites and Recent Folders in its
    folder pop-up shared with Tracker's Go menu (`olo_kit::places`),
    and Space for a look at a file in the Viewer): an Olo
    portal backend
    (`org.freedesktop.impl.portal.FileChooser` in `olo-portal`, beside
    ScreenCast) showing the kit's file panel (Tracker's columns,
    favourites, Quick Look, recent places); `olo-portals.conf` sends
    FileChooser to it. Chrome and Electron use the portal with
    `--xdg-portal` / `GTK_USE_PORTAL=1`-style switches or by default;
    GTK 4 apps use it by default, GTK 3 with `GTK_USE_PORTAL=1`, Qt with
    `QT_QPA_PLATFORMTHEME=xdgdesktopportal`: set these in the session
    environment. Save dialogs too (name, folder, overwrite).
  - ~~**Show in folder**~~ (done 2026-10-30: the desktop's Tracker
    serves `org.freedesktop.FileManager1`, each call an Open in
    Tracker). The OpenURI portal's `OpenDirectory` needs nothing more:
    xdg-desktop-portal (1.22 here) answers it with FileManager1's
    `ShowItems`, which Tracker serves, and only without one opens
    `inode/directory`'s default, which olo-session makes Tracker too.
  - ~~**Opening things**~~ (done: checked 2026-11-05, GIO opens a folder
    in Tracker and a link in Chrome from `olo-mimeapps.list`, xdg-open
    going through GIO on Olo's `XDG_CURRENT_DESKTOP`) (started 2026-10-30: the AppChooser portal is
    Olo's Open With window, so OpenURI's "which app?" is Olo's; olo-session
    writes desktop entries and `olo-mimeapps.list` defaults so GIO,
    xdg-open and OpenURI open files in Olo's apps as Olo does, links
    staying with the default browser; Olo's Chrome, once installed, is
    offered as the default browser by `~/.local/share/applications/
    olo-mimeapps.list`, behind any browser the person chose and ahead
    of the system's): the
    OpenURI portal and `xdg-open`'s MIME
    defaults resolved by Olo's own (`olo_kit::open`), so links from any
    app open in Olo's choice, files in Olo's apps; Olo as the default
    file manager and browser handler (`mimeapps.list`, the `x-scheme-
    handler`s), Chrome as the default browser offered by Olo.
  - ~~**Downloads**: Chrome's land in Tracker's Downloads with Olo's own
    notification (open, show in Tracker) and Deskbar's progress; a
    download's file shows as being written.~~ (done 2026-10-30: the
    extension tells olo-chrome's host of Chrome's downloads; Deskbar's
    progress through LauncherEntry, an Olo notification with Open and
    Open in Tracker, and Tracker shows a `.crdownload` under its final
    name with a bar and "3.1 of 10 MB", from `user.olo.expected_size`.)
  - **Look** (mostly done: app_server's `toolkits.rs` publishes the
    colour scheme and accent through GSettings, which the GTK portal's
    Settings serves as `org.freedesktop.appearance`; GTK 3 and 4 themes
    from Olo's palette, Qt through its GTK 3 platform theme; Olo's fonts;
    2026-10-30: fonts hinted as the kit, slight and grayscale, and Olo's
    Haiku cursors as an XCursor theme for programs that draw their own
    pointer; Olo's icons in the look as the icon theme, for the
    freedesktop names it has pictures for). Left, both waiting on Olo
    itself: `contrast` (once Olo has a high-contrast look) and
    `text-scaling-factor` (once Olo has a text size of its own; today
    the kit's text is one size, `fonts::PLAIN_SIZE`, which GTK already
    gets as its font size, so there is nothing to scale to).
  - **Menus and shortcuts** (GlobalShortcuts done 2026-10-30: asked
    once per app, kept, held through upgrades): ~~the GlobalShortcuts portal~~ (Chrome's
    extensions' shortcuts, Discord's push-to-talk) bound through Olo's
    keyboard profiles; ~~app menus where a toolkit exports one
    (`org.gtk.Menus`, `com.canonical.dbusmenu`) shown in Olo's menu bar
    style if worthwhile~~ (not worthwhile, 2026-10-30: Olo's menus are
    in each window, as BeOS's were, not in a bar at the top of the
    screen, and GTK 4, Qt and Electron programs already show theirs in
    their windows; a second copy in Olo's style would only duplicate
    them. Tray items' menus are Olo's, which is where it helps).
  - **Notifications, tray and badges** (tray done 2026-10-30: Deskbar is
    the StatusNotifierWatcher and host, items in its tray with their
    menus as Olo menus; tried with Slack's): ~~StatusNotifierItem (Slack's,
    Discord's tray icons) in Deskbar's tray with their menus~~;
    ~~the Unity LauncherEntry count (unread badges) on Deskbar's entries;
    progress on Deskbar (LauncherEntry progress)~~ (done 2026-10-30: the
    row's dot, shaking when the count rises, and a progress bar).
  - **The rest of the portals**: ~~Inhibit~~ (done 2026-10-30: the
    Inhibit portal and `org.freedesktop.ScreenSaver` in olo-portal, held
    by app_server's `KeepAwake`), ~~Background~~ (done 2026-10-30, with
    olo-session starting login items), ~~Print~~ (done 2026-10-30: Olo's Print
    panel, CUPS over IPP, `olo-print`), ~~Email~~, ~~Account~~ (done 2026-10-30: asked in the permission
    dialog with your picture), ~~Wallpaper~~ (done 2026-10-30), Screenshot (with screen sharing),
    ~~Camera/Location as Olo asks permission~~ (xdg-desktop-portal asks
    them through the Access dialog, Olo's), Secret (Passwords as the
    Secret Service, so Chrome's and Electron's stored secrets live in
    Olo's keychain; **held for Ben's go-ahead**, 2026-10-30: Chrome keeps
    its "Chrome Safe Storage" key in gnome-keyring through libsecret, so
    taking over `org.freedesktop.secrets` without first moving every
    existing item across would lose every saved cookie and password in
    Chrome and Slack. It needs a migration that reads the real keyring,
    as an explicit step in Preferences ▸ Security, tried on a copy
    first), and ~~Trash~~ (xdg-desktop-portal trashes through
    GIO, in the freedesktop layout Tracker's Trash uses: nothing to add). Done 2026-10-30:
    Screenshot (Olo's Screenshot app, region when interactive; PickColor
    with a loupe) and Access (the permission dialog, as Olo's BAlert).
  - ~~**Drag and drop and the clipboard**: files dragged from Chrome,
    Slack and GTK into Tracker and back, with the right actions and
    formats (`text/uri-list`, images, `x-special/gnome-copied-files`).~~
    (done 2026-10-30, tried with Chrome 156: a picture dragged out of a
    page lands as its file, a link as a clipping named by its text, files
    from Tracker open in Chrome; Tracker copies files as file managers do,
    `x-special/gnome-copied-files` and all, and pastes files, pictures
    (Chrome's Copy Image) and text; `Cx::paste_data`,
    `olo_kit::clipboard::files`, `dnd::named_file`, `dnd::moz_url`.)
  - ~~**Agent-readable**: the AT-SPI trees of GTK and Qt windows (and
    Chrome's beyond the extension) folded into Olo's widget tree, so
    agents can drive outside programs as they do Olo's.~~ (done
    2026-10-30: olo-screen's `tree` reads a non-kit program's windows
    over AT-SPI, roles named as AccessKit's; tried with GNOME
    Calculator, driven by name; Qt asked to join with
    `QT_ACCESSIBILITY=1`. Chrome's pages stay with the extension, which
    sees more of them.)
  - Where there's no clean way, a hack is fine if it's reliable: an
    LD_PRELOAD shim, a launcher wrapper, a policy file or a patched
    config, documented in HACKING with how it's tested.

- **Olo's own login and boot** (Ben, 2026-11-01, for after the GPUI
  refactor is complete): replace GDM with an Olo-native display
  manager, and have Olo take over the boot visuals as early as it can
  (Plymouth's place): an "Olo starting up" screen that is an homage
  to BeOS's boot screen, drawn with sunburst-made graphics
  (docs/ICONS.md), with a really nice startup sound. Related: an
  "Upgrading Olo" veil on every display while app_server restarts
  and the displays settle, in the same style. *Done* (2026-11-05,
  HACKING "The session and in-place reload"): the upgrade's veil, the
  wordmark and the icon set's art lit stage by stage; the same as the
  start-up screen as a session starts ("Starting Olo", till the desktop
  and Deskbar show); the start-up sound, made in process (a D♭ major
  ninth swell), once a session, with a switch in Preferences ▸ Sound.
  *Left*, both needing a try on the machine (they change what it boots
  into, and can't be seen without a reboot): the login screen in GDM's
  place (a daemon owning the seat, PAM and logind's session, a greeter on
  the kit) and the boot in Plymouth's (a theme of the same picture, or
  Olo drawing from the initramfs).

- **Text and everything drawn, better than Apple's** (Ben, 2026-10-31):
  "make sure our text system is SOTA -- rivalling apple's or anything
  else out there. perfect looking, adorably beautiful text is essential";
  "also the same applies to icons and everything in olo: everything must
  render absolutely beautifully, good enough that if steve jobs saw it
  he'd want to fire his team at apple because we put them to shame".
  *Done so far*: Olo's GPUI draws text as the kit does (hinted Noto,
  variable weights, the fallback chain; 13e20c7) plus a touch of stem
  darkening for small text (747a25d; now an emboldened outline, a third
  of the time a glyph took). Icons in the GPUI kit: the
  pre-scaled icon for the device size, on whole device pixels, never
  resampled; checked by the layout review at every scale.

- **Other programs' file handling through Olo, fixed** (Ben,
  2026-11-04): "the gtk/qt integration doesn't seem to be working
  properly e.g. even in olo chrome i can't click on the folder for a
  download and have it open tracker. also, when i'm trying to upload a
  file in slack it does show tracker but then if i press space on a
  file to quick look it works but then if i press down or up it doesn't
  properly show the other ones". Two bugs: Chrome's Show in folder (its
  downloads' folder icon) goes to `org.freedesktop.FileManager1`'s
  ShowItems, which Tracker serves (tracker's filemanager.rs; HACKING's
  "Open in Tracker"): find why nothing opens (the name not taken, the
  call not reaching it, Chrome asking through the portal's OpenURI or
  xdg-open instead) and test it from Chrome on an Agent Screen; and in
  the file panel the portal shows (apps/portal, `olo_ui::file_panel`),
  Quick Look open on a file must follow ↑ and ↓ (and ← →) to the
  file selected next, as it does in Tracker's own windows.
  *Quick Look: done* (9bf9eb3b: the look follows the panel's list both
  ways, `--follow`, HACKING's file panel). *Show in folder, looked at*:
  Tracker owns `org.freedesktop.FileManager1` in the session and its
  ShowItems opens the folder with the file selected (tested on a test
  session's bus); GIO's default for folders is Tracker too; the user
  services have WAYLAND_DISPLAY. What Chrome 157 calls is still to see:
  `busctl --user monitor org.freedesktop.FileManager1` (and
  `org.freedesktop.portal.Desktop`) running while its download's folder
  is clicked (a test session's Chrome stops at its terms on first run).
  *Found and fixed* (2026-11-05): Chrome 157 calls ShowItems itself
  (it asks whether `org.freedesktop.portal.Desktop` and FileManager1 are
  on the bus first; with the portal there too, as in Ben's session, it
  still calls ShowItems), and Tracker serves it: a folder's window with
  the file selected, brought back from minimized or opened on this
  workspace, focused (checked on a test session's bus with Chrome, the
  portal frontend and Olo's backend). But Ben's journal said, at each
  Olo upgrade on Nov 4, "tracker: not serving org.freedesktop.FileManager1:
  name already taken on the bus": Tracker asked for the name with
  ReplaceExisting and DoNotQueue, so when anyone held it (the last
  Tracker's connection not yet gone as it restarts, or Nautilus, which
  the system's activation file `org.freedesktop.FileManager1.service`
  starts whenever no Tracker has the name, and which doesn't allow it
  taken) Tracker gave up for the session and every "show in folder"
  went nowhere. It now waits in line for the name and allows it taken
  (the next Tracker takes over at once): checked with a stand-in owner
  that doesn't allow it taken (Tracker logs that it waits, takes the
  name the moment the other lets go, and Chrome's Show in folder opens
  Downloads). *Left*: Ben's try after the upgrade. Whether to keep
  D-Bus from starting Nautilus at all (Ben, 2026-11-05: yes, and "doctor
  should offer to install the change that makes it always use tracker"):
  *done*, Olo Doctor's "Always Use Tracker" (HACKING's Tracker), its
  file and Tracker's `--file-manager` tried on a private dbus-daemon and
  a headless session's bus.

- **olo-slack: Slack, Olo-fied, as olo-chrome is Chrome** (Ben,
  2026-11-04): "build olo-slack that is like olo-chrome but an olo-fied
  version of slack where it integrates deeply with everything olo".
  As apps/chrome does for Chrome (HACKING's "Google Chrome"): Slack's
  web app in Olo's Chrome as an app window of its own (`olo.slack`,
  its own profile, Wayland only), with an extension and native host
  that join it to Olo: its unread count and mentions on its Deskbar
  entry, its messages as Olo notifications (Notifications' own banners
  and history, Do Not Disturb), its status following Olo's (away when
  the screen locks, in a meeting, Do Not Disturb), its links opened
  with Olo's default apps, files dropped from Tracker, pictures in the
  Viewer, the palette's commands (jump to a channel or person, search),
  its windows and messages readable and drivable by agents (the kit's
  tree, Agent Screens), and passwords and passkeys from Olo Passwords.
  *Built* (2026-11-05, HACKING's "Slack"): `olo-slack` (apps/slack) runs
  Slack's web app in Olo's Chrome on a profile of its own, its window
  `olo.slack` (`olo_xdg::apps::WEB_APPS`); one window, `slack://` links
  and Slack's pages handed to it; no first-run dialog (its ticked "make
  Chrome the default browser"). The Olo extension and host (1.8.0;
  `slack.rs`, `slack-page.js`, `slack.js`): Slack's notifications as
  Olo's (a click goes to the conversation), its unread count on
  Deskbar, links out in the person's browser, Open in Viewer for
  pictures, its channels, people, Jump To… and Search… in the palette
  (`commands::serve`), agents' trees and Passwords as in Chrome. Tested
  against a stand-in page and Slack's real sign-in page; idle costs
  nothing. Left, needing Ben's Slack signed in: a look at each against
  Slack's real client (its title's count, its sidebar's markup for the
  palette, whether it notifies through `Notification`), and its status
  following Olo's (away when the screen locks, Do Not Disturb, a
  meeting), which needs Slack's client to build against.

- **Screen recording, for people and agents** (Ben, 2026-11-04):
  "create an olo native screen recording app similar to CleanShot X
  that can be used by humans but also is agent native so that agents
  can use it to create polished screen recordings themselves too, from
  within an agent screen. and make sure that it comes with agent skills
  about doing it that is included with the agent screens skill and that
  for screen recordings agents can have a high quality 4k agent screen
  with 4k screen recordings". An Olo app (beside Screenshot, sharing its
  region picker) that records the screen, a window or a region, with
  CleanShot X's polish: the pointer drawn larger and smoothed, clicks
  and keys shown, zoom that follows the action, a background and
  rounded frame around a window, the microphone and the computer's
  sound, a webcam bubble, pause and resume, then a trimming editor and
  export (MP4, H.264/HEVC/AV1 through the GPU's encoder, and GIF), all
  native in Rust (no ffmpeg), app_server's capture of the frames
  (screencopy / dmabuf, no copies), on a worker, cancellable. Agent
  native: everything a person can do there, an agent can drive through
  a documented API (start, mark, zoom to, caption, stop, trim, export,
  where it went), within an Agent Screen, without touching the person's
  screen. An Agent Screen made for it can be 4K (3840 × 2160, at 1× or
  2×) and record at 4K. And a skill for it, shipped with Olo Screens'
  skill (olo-screens): how to plan, pace and record a polished demo
  (rehearse, then record; the pointer's path, pauses, zooms; checking
  the result).
  *Started* (2026-11-04, HACKING's "Screen recording"): the recording
  itself, for people and agents: `olo-screenshot --record` (a display
  or a window, frames only as they change, H.264 into an MP4 written as
  it goes, `--stop`, `--seconds`), `olo-screen record-video` and
  `stop-video` on an Agent Screen, 4K checked (played back in the
  Viewer); `olo_video` shared with Agent Screens' Export Video; the
  skill says how; ⌘⇧6 records the screen or stops (Ctrl+Alt+Print
  with the Windows and BeOS profiles), Deskbar's tray showing a record
  light with Stop Recording while it goes, a notification with Show in
  Tracker when saved. *Then* (2026-11-04): a region (`--region
  X,Y,W,H` in points, or dragged out with ⌘⇧4's picker), pause and
  resume (`--pause`/`--resume`, the pause cut out of the video; Deskbar's
  light shows a pause sign, its menu Pause or Resume), Screenshot's
  Record… (Screen, Active Window, Region…), and for agents
  `olo-screen pause-video`, `resume-video`, `record-video --region` and
  `video-frame` (what a video shows at a time, to check it; the Viewer's
  seek into a still stretch fixed on the way: it showed the next change).
  The recorder no longer wakes on a timer (a still or paused recording:
  only NVIDIA's driver threads wake). *Sound* (2026-11-05): the
  computer's and the microphone's, either or both (Record…'s ticks,
  `--sound`), AAC beside the video. *GIF* (2026-11-05): Record… ▸
  Save as GIF, `--as-gif`, `--gif VIDEO`. *Clicks and shortcuts shown*
  (2026-11-05): Record… ▸ Show Clicks, Show Shortcuts (app_server
  draws them; typing never shows); and GPU recordings no longer lose
  their first 2.5 s (the encoder opened before the clock starts). *A
  countdown* (2026-11-05): 3 s on screen before a person's recording.
  *Trimming* (2026-11-05): the Viewer's Edit ▸ Trim…, `--trim`, copied
  without encoding again. *A window on a background* (2026-11-05):
  Record… ▸ Window on a Background, `--background`: the blurred
  wallpaper, rounded corners, a shadow. *The pointer larger and
  smoothed* (2026-11-05): Record… ▸ Larger Pointer, Smooth Pointer,
  `--pointer-size`, `--smooth-pointer`; a window's recording has the
  pointer. *Zoom* (2026-11-05): Record… ▸ Zoom on Clicks,
  `--zoom-follow`; `--zoom-to` and `olo-screen zoom-video` for agents.
  *Marks and captions* (2026-11-05): `--mark`/`mark-video` (chapters,
  shown in the Viewer's timeline and Go menu), `--caption`/
  `caption-video` (in captures only).
  *The camera bubble* (2026-11-05): Record… ▸ Show Camera, `--camera`,
  `olo-camera` (V4L2 in process), checked with the stand-in camera.
  *Left*: the camera bubble on a real camera (Ben's first try: Show
  Camera, then a recording of the screen; the laptop's camera and the
  Studio Display's were never opened here); in a window's recording
  done (2026-11-05: on a background, the recorder draws the camera on
  the background's corner itself, `inset.rs`; checked with the stand-in
  camera), and
  the GPU
  encoder below (OpenH264 on the CPU keeps about 9 frames a second at
  4K: 111 ms a frame). *HEVC and AV1* (2026-11-05): `--codec`,
  Preferences ▸ GPUs ▸ Format; every codec now at a measured constant
  quality (H.264's recordings a third smaller for the same picture).
  - **Encoding on the best GPU, chosen in its settings** (Ben,
    2026-11-04): "the screen recording app should also take advantage
    of a discrete graphics card for encoding when available (and make it
    a togglable setting to use cpu or gpu or specific gpu if more than
    one and default it to whatever the most powerful detected gpu is)".
    Its settings list the CPU and each GPU with a hardware encoder
    (NVIDIA's NVENC, Intel's and AMD's through Vulkan Video or VA-API),
    by name and what each can encode (H.264, HEVC, AV1, up to which size
    and rate), the most powerful found chosen by default (a discrete card
    over the built-in one); the frames handed to it as dmabufs, without
    a copy through the CPU where the driver allows. A GPU that fails
    mid-recording falls back to the next, saying so.
    *Started* (2026-11-04): NVIDIA's NVENC (H.264, `olo_video::nvenc`,
    its library opened at run time so nothing links it), the recorder's
    `--encoder auto|cpu|nvidia`, auto taking NVENC where it opens: 4K in
    13.4 ms a frame against the processor's 111; the setting in
    Preferences ▸ GPUs ▸ Screen Recordings (the most powerful by
    default, named; the NVIDIA GPU; the processor). *Then*
    (2026-11-05): HEVC and AV1 on NVENC, each codec at a measured
    constant quality; an encoder failing mid-recording opened again,
    else the recording kept to there (not yet falling back to another
    encoder: the file's sample entry describes one stream).
    *Found* (2026-11-05), for Intel's and AMD's: Fedora's own VA-API
    drivers (iHD from Fedora, Mesa's) leave out the patented codecs: on
    this ThinkPad's Intel GPU (xe, renderD128) iHD encodes only JPEG,
    VP9 (profiles 0–3) and AV1 profile 0, no H.264 or HEVC (those need
    RPM Fusion's `intel-media-driver` / `mesa-va-drivers-freeworld`). An
    H.264 encoder through VA-API was written (libva opened at run time,
    bindgen declarations from libva 2.24's headers, NV12 on a few
    threads, CQP, the driver writing the headers) and set aside: it
    couldn't be run here. What would run here is AV1 through VA-API,
    which needs its sequence and frame headers written by us (packed
    headers with the offsets the driver patches, as FFmpeg's
    vaapi_encode_av1 does). *Done* (2026-11-05, HACKING "AV1 on Intel's
    and AMD's GPUs"): AV1 through VA-API on the Intel GPU (`olo_vaapi`,
    its headers written by us, constant quality 40: 47.6 dB on a
    2560 × 1600 screen in about 8.5 ms a frame), `--encoder vaapi`, auto
    falling back to it for AV1, and Preferences ▸ GPUs offering it where
    the driver says it encodes AV1 (asked on the panel's thread, about
    15 ms: no cache needed). *Left*: the frames as dmabufs (CUDA's
    external memory; VA-API's DRM PRIME import with its video processor
    making NV12) rather than copied; auto preferring the Intel GPU on
    battery.

- **StyledEdit: Markdown edited as it looks (and as source), code with
  highlighting** (Ben, 2026-11-05, lower priority): "when opening a
  markdown file StyledEdit should natively support the Markdown and be
  more like Typora WYSIWYG editing, but also have an option to switch
  back and forth between WYSIWYG mode and source mode and in source mode
  it should be like a code editor with syntax highlighting and fixed
  width font. also make it support code/source mode for all major
  programming/scripting languages with syntax highlighting and fixed
  width font." Now: Markdown comes in through Import… (`convert.rs`,
  `from_markdown`) and goes out as Markdown, as a styled document, and
  the editor has no highlighting. To do: a `.md` opened is a Markdown
  document, edited as Typora does (headings, emphasis, lists, quotes,
  links, pictures, tables, code blocks and inline code shown as they
  look; the marks of the line being edited shown as typed) and saved as
  Markdown again, keeping what the person wrote (their list markers,
  emphasis characters, line breaks, front matter, HTML); a View toggle
  (and a key, ⌘/ say) between that and Source, which shows the Markdown
  itself in a code editor: the fixed-width font, syntax highlighting,
  line numbers, the place kept across the switch. Source mode for every
  major language as well (Rust, C, C++, Go, Python, JavaScript and
  TypeScript, shell, Ruby, Java, Kotlin, Swift, C#, PHP, Lua, SQL, HTML,
  CSS, JSON, YAML, TOML, XML, Makefiles, Dockerfiles, …), a file opened
  by its type going straight to it: highlighting from a Rust grammar
  library (tree-sitter's grammars, in process), colours from the
  palette in light, dark and night, incremental as it's typed (a file
  of 100,000 lines opens and scrolls at once: measured). In the kit
  (the code editor is one any app could use: Terminal's, Viewer's text),
  documented in HACKING.

## Maybe one day

Tabled (Ben, 2026-11-02): "performance seems pretty solid now in olo so
you can table the rest of the performance related items under a "maybe
one day" section in the todos and skip them". Not worked on unless he
asks again. But (Ben, same day): "still address any performance items
that are real and cause any noticeable impact to the user but we don't
have to keep optimizing just for the sake of optimizing": a slowness a
person notices (a stutter, a lag, a hang) is still fixed at once, in
Open; making fast things faster waits here.

- **Every frame on time, every display** (Ben, 2026-11-01: "dropping
  frames is never acceptable: olo must be buttery smooth on all
  displays all the time"; the pointer "definitely way slow/stuttery",
  worst on the Studio Display; app_server at 20% of a core). Done
  2026-11-01 (HACKING "Every frame on time"): per-display pacing from
  measured cost and observed drops; `{"type":"frames"}`,
  `capture_drops`, Activity Monitor ▸ Displays; the pointer on an
  overlay plane as cursor plane, clipped at the display's edge in
  Olo's Smithay (the first try, unclipped, had apple-drm refuse the
  flip and app_server restart eight times; a refused flip no longer
  kills it); windows as scan-out candidates; Smithay vendored with
  phase timings and scan-out reasons. Found: a 5K frame's 5 ms is
  all `render_output`, and in it the GPU submit (asahi's
  agx_flush_batch from the fence) and syncobj waits, ~3 ms a frame
  in the kernel; plane assignment and atomic tests cost 0.0 ms.
  **Found since** (2026-11-01, later): the 5 ms was Smithay's GLES
  renderer making a fence per texture drawn (a GPU submit an
  element); one fence a frame took the 5K's draw to 0.2/1.2 ms
  (p50/p95), drops to 0–1 a minute, app_server's kernel time from
  13% to 1.5%. The pointer's remaining "jitter" and "stuck" are the
  mouse's reports (Razer Viper V2 Pro, 2.4 GHz receiver on a hub,
  USB autosuspend on): 429 gaps over 5 ms a minute, the longest 39 ms,
  673 of 1000 reports a second (`PointerHealth`, Activity Monitor ▸
  Displays ▸ Pointer). `71-olo-input.rules` keeps receivers awake;
  the radio is the person's to fix (the receiver nearer, its
  extender, a lower polling rate). **Next**: windows on overlay
  planes (OLO_OVERLAY_PLANES=1) once the pointer and such a plane
  agree (a client's cursor on the plane rendered right, or the
  pointer's plane above every window plane and the window refused
  while the pointer is drawn in the frame over it); the "draws"
  0.2 ms and "finish" 0.2 ms are what's left of a frame; app_server
  at ~11% with a call on is the tick (now once per 8 ms) and looks;
  per-display render threads (its own TODO).

- **app_server's critical path on threads of its own** (Ben,
  2026-11-01: "why run the risk of a blocking call? threads would
  make sure we don't accidentally block"): input → pointer plane
  update → commit never shares a thread with anything that can
  block. Design: an input-and-cursor thread owning libinput and the
  cursor planes (a pointer move is a plane property commit, no frame);
  a render-and-commit thread per display owning its GL context, DRM
  surface and pacing (a slow 5K frame can't delay the built-in
  display's); the Wayland clients and IPC on the main loop, handing
  each display's thread a snapshot of what to draw. Our own
  synchronous work (importing a client's buffer, a fence, a DCP test
  or commit) is what blocks today, and the heartbeat (`timings`)
  finds it. After the reliability work.

## Finished (most recently finished first)

- ~~**The voice model's weights in f16 or int8, if voices stay as clear**~~
  (done 2026-11-05, HACKING's "Voice processing": the weights alone in
  f16, packed by tract's own optimizer for its f16-weights,
  f32-activations kernels (each matmul's constant weights swapped for an
  f16 copy); on x86 with F16C a frame back to back 391 → 296 µs, paced as
  live 773 → 536 µs, every voice trial's SDR the same to a tenth of a
  dB. int8 not needed for the caches' sake now. *Left*: the MacBook's
  numbers (NEON's fp16 kernels; it falls back to f32 where they don't
  optimize))
  (from the SIMD work, 2026-11-04): a live frame is 553–588 µs against
  388 back to back because its 8 MB of f32 weights fall out of the
  caches between frames; half or a quarter of that would stay. Try
  tract's f16 weights (F16C on x86, NEON's fp16) and int8, and keep one
  only if the voice trials (crates/voice tests, the AirPods chain) say
  quality holds. Measure on the ThinkPad and the MacBook.
  *Tried* (2026-11-04, x86 with AVX2): tract 0.23.9's
  `FloatPrecisionTranslator` (f32 → f16, the whole model) on the
  encoder and both decoders breaks DeepFilterNet: a clean voice comes
  out at −1.5 dB SDR (f32: 34.1), noise isn't taken out (7.5 → −2.0 dB
  SDR; f32: 17.7), and its 22 µs a frame is the network doing nothing.
  It turns the activations to f16 too (the GRUs' sums lose too much),
  where only the weights should be. What's left to try: weights alone
  in f16 (tract-linalg has f16-in-f32-out matrix kernels on x86,
  `f16f32` with F16C, and on NEON), which means packing the constant
  matrices as f16 in tract's own optimizer rather than translating the
  model; then int8 the same way, judged by the same trials.

- ~~**Notifications, robust and everywhere**~~ (finished 2026-11-05:
  every part below done, nothing more asked; on agent screens each
  screen's own centre since its own bus) (Ben): after Haiku's
  notification_server and `BNotification`, better where we can. ~~Done~~:
  grouped by app, the four kinds (information, important, error,
  progress), body markup with links, `value` progress, replace-by-tag,
  inline reply (`NotificationReplied`), `transient`, `category`, sound
  hints read, `ActivationToken` before an action, a click bringing the
  app's window forward; sounds played natively (the freedesktop theme
  for important notes and errors); a history kept across a restart
  (History window, Deskbar's bell with the unread count); Do Not Disturb
  by hand, for a while or on a daily schedule; per-app banner, sound and
  history choices (Preferences ▸ Notifications); agent-readable
  (`com.olodesktop.Notifications1`, `olo-notifications --list --json`,
  `--invoke`, `--dismiss`, `--clear`, `--dnd`); GLib's
  `org.gtk.Notifications` (GApplication apps, their actions run on the
  app) and Flatpak apps through the GTK portal; a kit API for Olo's
  own apps (`olo_kit::notify`: Tracker's long
  copies); Do Not Disturb while sharing the screen (2026-10-30: from
  olo-portal's sharing state, errors held too, banners already up taken
  down; "While sharing the screen" in Preferences, on by default).

- ~~**Google Chrome works great in Olo**~~ (finished 2026-11-05: every
  part done; drag and drop, the clipboard and file pickers with "Other
  programs feel like Olo's own". Not done: Chromium where Google has no
  build, which no machine Olo runs on needs, x86_64 and arm64 both
  having Google's) (Ben; Olo's web browser):
  many sites really need Chrome. When stock Google Chrome is downloaded and run, Olo
  recognises it: Olo window chrome (server-side decorations, or
  whatever it takes: flags, policies, runtime patches), light/dark
  following the desktop, and Olo Passwords filling and saving logins
  in it seamlessly (e.g. a native-messaging host plus an extension
  installed by policy, speaking the keychain protocol), including the
  verification code (TOTP) from a field's context menu beside the
  username and password. Drag and drop, the clipboard, notifications,
  file pickers, downloads into Tracker, default browser, Deskbar entry:
  all of it as well as Olo can make it. Ugly hacks are fine as long
  as it is reliable.
  ~~**Downloads and the default browser**~~ (done 2026-10-30: Chrome's
  downloads in Deskbar, Olo's notifications and Tracker; Olo's Chrome
  offered as the default browser; see "Other programs feel like Olo's
  own").
  ~~**Olo's look**~~ (done: the portals now start in a real session,
  so it follows light/dark; each profile starts with Olo's title bar
  and frame instead of Chrome's own buttons and with the GTK theme's
  colours; HACKING, "Other toolkits' programs").
  ~~**Passwords in Chrome**~~ (done: the Olo Passwords extension,
  packed and signed per user, listed in Chrome's `extensions` folder,
  with `olo-chrome` as its native messaging host; a field's context
  menu offers each login's username, password, one-time code or the
  whole login, the toolbar button lists the page's logins, and a login
  sent with a form is offered for saving or updating; HACKING, "Google
  Chrome").
  ~~**Notifications**~~ (done: Chrome sends sites' and extensions'
  notifications to Olo's notifications app over
  `org.freedesktop.Notifications`; tested headless: shown with Chrome's
  name and icon, a click reaching the page's `onclick`, Chrome's Settings
  button opening its notification settings. Test buses need
  `tools/dev/busd-lenient`; HACKING, "Notifications").
  ~~**Installed on first use**~~ (done: `apps/chrome`, `olo-chrome`;
  tested headless end to end, the real 139 MB download into a fixture
  home, then Chrome running on Wayland in Olo; kept up to date: once
  a day a start fetches a newer version in the background for the next
  start, tested from a stand-in old version. Left: Chromium where Google
  has no build) (Ben): Google Chrome is always listed in
  Deskbar's apps; the first launch downloads it (Google ships arm64
  Linux packages now: dl.google.com/linux/chrome/rpm/stable/aarch64,
  checked against the repo's sha256), unpacks it in process (rpm cpio
  payload, natively) into `~/.local/share/olo/chrome/<version>` with
  a `current` link, with live progress, and keeps it updated from the
  same repo. Chromium if Google's isn't there for the machine. Ben's
  machine has 156.0.8078.47 there now (installed by hand the same way,
  with `~/.local/share/applications/google-chrome.desktop`); it runs on
  Wayland under Olo.

- ~~**Agent Screens: Chrome opened from a screen's Deskbar lands on the
  person's display**~~ (done 2026-11-05, HACKING's "A screen's own
  bus": each screen on a D-Bus bus of its own, `olo-bus` (busd's broker
  on async-io, answering calls to absent names at once), with its own
  notification centre and portal (xdg-desktop-portal in front of Olo's
  and GTK's backends) and the person's accessibility bus; GTK apps and
  portal dialogs stay on the screen; Electron apps get the screen's own
  config folder; no keyring there, the person's secrets only through
  Passwords' grants) (Ben, 2026-10-31): "i was just using the "Take
  Control" feature in an agent screen, and i used the deskpad to try open
  a new chrome, but it actually opened it on my user screen instead of in
  the agent screen". Anything launched on an agent screen (its Deskbar,
  its Tracker, an agent's `olo-screen run`) must open there. Likely cause
  (to confirm): Chrome's single-instance handoff, where the new process
  finds the person's running Chrome by its profile and asks it for a
  window, which that Chrome opens on its own Wayland connection; so a
  screen's Chrome needs its own instance (its own profile dir, or a
  Chrome running on that screen), and every launcher there passes the
  screen's `WAYLAND_DISPLAY`/`OLO_SOCKET`. Check the other single-instance
  apps (ours via D-Bus activation, Slack, GTK's GApplication) the same way.
  *Status* (2026-11-02): Chrome done (bb4dbc4b, 2026-10-31: a profile
  of the screen's own, `CHROME_CONFIG_HOME`); Olo's apps are fine (their
  single-instance socket is the display's, `instance::socket_path`).
  *Left*, all through the session bus a screen shares with the person
  (on purpose: their keyring, notifications; a headless session never
  takes its services over): a GTK app's GApplication finds the person's
  copy by its bus name; an Electron app (Slack, VS Code) hands off to
  the person's through the lock in their config; and a portal dialog
  asked from a screen (Chrome's upload, Open With) shows on the
  person's display (olo-portal is the person's). The fix: a bus of the
  screen's own with its own portal (as test sessions have), reaching
  the person's keyring only through Passwords' grants ("Passwords for
  Agent Screens"); it changes what agents on existing screens reach, so
  it goes with that item.

- ~~**Move windows to and from Agent Screens**~~ (done 2026-11-05,
  HACKING's "Moving windows to and from screens": `olo-screen take` and
  `give`; an Olo app re-executed there with its state, one window of
  several (Tracker's, Terminal's windows and tabs) by a copy of the app,
  any other program's connection replayed there by olo-wlproxy; the
  window menu's, Deskbar's app and heading menus' and Terminal's tab
  menu's Move to Agent Screen ▸, and Agent Screens' Move Windows To…
  beside Take Control; the person's programs back when a screen stops
  or dies, a screen's given ones outliving it. Tracker has no tab menu
  of its own: its windows' tabs are the window menu's) (Ben, 2026-10-31):
  "everywhere we have menu options to move something to a
  display-workspace there should also be a section in that menu to move
  it to an agent-screen (new or existing) and when in the agent screens
  app next to take control there should be an option to move all windows
  in the agent screen to a user display-workspace". So: the window tab's
  menu, Deskbar's app menu, the workspace heading's menu, Tracker's and
  Terminal's tab menus (wherever Move to Workspace is) gain an Agent
  Screens section (each running screen, and New Agent Screen); Agent
  Screens gets, beside Take Control, a way to move every window on the
  screen onto one of the person's display-workspaces. The program keeps
  running as it moves (its connection handed between the two
  app_servers, or the app reopened there with its saved state).

- ~~**Viewer: select and copy text in PDFs and other documents**~~ (done
  2026-11-05, HACKING's Viewer and `crates/preview`: a PDF's text read
  natively, selected, copied and found; texts' and formatted documents'
  too, with the kit's find bar. A picture's OCR, and EPUBs (a book
  Viewer doesn't show yet), not done: neither has text Viewer can show) (Ben,
  2026-11-05, for later): "Viewer needs to support selecting and copying
  text in PDFs and any other relevant document type". In a PDF (and
  other documents Viewer shows with text in them: EPUB, a Markdown or
  plain-text file, Office documents if shown, a picture's OCR later
  perhaps), text is selected by dragging over it (by word with a
  double-click, by line with a triple), Select All, and copied with ⌘C
  and the right-click menu, as a selection anywhere in Olo is (the
  selection's colour from the palette, drawn over the page at every
  scale); the text read natively from the document (the PDF's text
  layer, its glyphs' positions and Unicode mapping, in Rust), in reading
  order, line breaks and hyphenation as a person would want them pasted.
  Find (⌘F) finds in it too and shows each match. Agents read a page's
  text from the widget tree and select it as a person does.

- ~~**Ethernet beside Wi-Fi: Deskbar, and Preferences ▸ Network in full**~~
  (done 2026-11-05: HACKING's Deskbar tray and Preferences ▸ Network)
  (Ben, 2026-11-05, for later): "in addition to the wifi icon that is
  shown in the deskbar tray, when ethernet is connected it should also
  show an ethernet icon. on both of them right-clicking the icon should
  also show some stats/metrics and have a link to open the relevant
  preferences. also, if both are connected at once the one that is the
  default route/link should be shown as "stronger" or in some way active
  vs the other one. and in preferences -> network it should also have
  full support for managing ethernet links as well as wifi and have the
  ability to choose/change the priority/preference of each link like you
  can in macos so e.g. by default you'd want ethernet highest priority,
  then wifi but if the user wanted to change they could. also have other
  common network settings in here like the ability to set custom dns
  servers, a tab to manage custom host entries, a tab to enable/disable
  and configure shaping, and whatever else might be useful to have." To
  do: an Ethernet icon in Deskbar's tray while a wired link is up (icons
  per ICONS.md); the one carrying the default route drawn as the active
  one (the other dimmer, or marked); right-click on either: its stats
  (address, gateway, DNS, speed or signal and rate, bytes in and out,
  live) and "Network Preferences…". Preferences ▸ Network: wired links
  managed as Wi-Fi is (DHCP or manual addresses, IPv6, MTU, 802.1X), the
  services' order as macOS's Set Service Order (Ethernet, then Wi-Fi by
  default, dragged to change: route metrics through NetworkManager's
  D-Bus API, natively), DNS servers and search domains of the person's
  own (per link and for all), a Hosts tab (entries of their own, kept in
  /etc/hosts through polkit), a Shaping tab (bandwidth and latency
  limits per link, on and off: the kernel's traffic control over netlink,
  in Rust), and what else a network panel wants (proxies, VPNs listed
  and switched, a hotspot). Agents read and change all of it through
  IPC as the person does.
  *Deskbar done* 2026-11-05: an Ethernet port beside Wi-Fi while a wired
  link is connected, the link not carrying the default route faint, and
  a right-click menu on each (joined to, in use or standing by, signal
  and rate or speed, addresses, router, DNS, interface, hardware
  address, bytes in and out, Network Preferences…), following
  NetworkManager's signals (`network::watch_links`; a strength or rate
  change taken from the signal, under a wakeup a second idle). `nm-fake`
  is a whole stand-in now (links, routes, profiles, Reapply, `--tell`).
  *Preferences done* 2026-11-05: Network has Wi-Fi, Ethernet (connect,
  DHCP or its own address, router and DNS, applied without
  disconnecting), Service Order (as a Mac's; each kind's route metric in
  NetworkManager's conf.d, so later networks fall in line), DNS (servers
  and search domains for every link, public resolvers to start from),
  Hosts (/etc/hosts, entries on and off, comments kept), Shaping (as a
  Mac's Network Link Conditioner: netem over rtnetlink, each way, with
  presets) and Details; root through `olo-net-admin`, asked for once
  while the panel is open. Tested against `nm-fake` end to end (the
  route moving, Deskbar's icons following) and shaping as root in a
  namespace of its own.
  *The rest done* 2026-11-05: a VPN tab (WireGuard and NetworkManager's
  plugins' VPNs listed, connected and disconnected, removed; a WireGuard
  `.conf` imported natively, its DNS kept from leaking), a hotspot under
  Wi-Fi (WPA2, the connection shared), and IPv6 and the MTU on a wired
  link. Proxies were left out: NetworkManager's proxy setting reaches
  only programs that ask libproxy for it, and Olo's own programs and
  browsers take theirs elsewhere; a proxy for the whole desktop is a
  thing of its own if wanted.

- ~~**Passwords is the SSH agent, on by default**~~ (done 2026-11-05,
  HACKING's Passwords: the agent protocol on a socket of its own in
  `SSH_AUTH_SOCK`, olo-session starting Passwords without a window for
  it; listing once unlocked, signing once the terminal session is
  allowed, as `op`; OpenSSH and PKCS#8 keys. ssh-add -L and ssh-keygen -Y
  sign tried through it. Ben's `~/.ssh/config` sends ssh to 1Password's
  app's agent: Security says so and switches it when he asks, from his
  next login; not done without asking. Left: a security key or face as
  the approval, with those items) (Ben, 2026-11-05): "build
  and enable by default ssh agent support in olo passwords so that the
  ssh keys in it are available for use automatically". Passwords keeps
  1Password items and fields of the SSH Key kind (`ItemFieldType::SshKey`,
  `onepassword::types`) and shows them; nothing serves them to ssh. To do:
  an agent speaking the SSH agent protocol (draft-miller-ssh-agent: list
  identities, sign, with RSA SHA-2, Ed25519 and ECDSA keys; in Rust, the
  `ssh-agent-lib`/`ssh-key` crates or our own, in process, no `ssh-agent`)
  on a socket in the runtime dir, `SSH_AUTH_SOCK` set for the whole
  session (olo-session's environment, systemd's user manager's, so
  Terminal's shells, git and editors use it), on by default with a
  setting in Passwords (and in Preferences) to turn it off. Each use
  asks as 1Password's agent does: which program (its process and
  command line) wants which key, approved by the person (Touch ID's
  equivalent here: the password, a security key or face, per the
  YubiKey and face items above), remembered for that program for a
  while; keys locked with Passwords lock. `ssh -T git@github.com` and a
  `git push` from Terminal work with a key from Passwords, tested
  headless with a fixture vault and an `sshd` of the test's own on a
  local port (or a Rust SSH server in-process).

- ~~**Arrow keys skip 10 s in a video, in Viewer and Quick Look**~~ (done
  2026-11-05, HACKING's Viewer: ← and → seek a video or a sound by 10 s,
  ↑ and ↓ the next file; tried headless with a test clip: 4 → 14 → 24 →
  14 s, the same file) (Ben,
  2026-11-05, for later): "when watching a video in viewer or quick look,
  the left and right arrows should skip back and forward 10s in the video
  (right now they go to the next file in the tracker)". Viewer's keys
  (`apps/viewer/src/main.rs`, ~1189 and ~1224) and Quick Look's step to
  the folder's next or previous file on ←/→; with a video showing they
  seek it by 10 s instead (clamped at its start and end; held, they keep
  seeking), ↑/↓ (or ⌘←/⌘→, say) still going to the next file, shown in
  the window's menu and its keys' help, and told to agents (the player's
  position in its widget tree). Tried headless with a video: ← and →
  move its time by 10 s, the file stays.

- ~~**Passwords: Copy Reference**~~ (done 2026-11-05, HACKING's op: in
  the item's right-click menu and Edit menu (its password, else its first
  secret) and each field's right-click menu (Copy, Copy Reference); the
  reference `op item get` gives, tested resolving back) (Ben, 2026-11-05): "add a copy reference
  menu item to items in passwords so that i can right click and get a
  reference that will work in the op cli shim to that secret". The `op`
  shim already resolves `op://vault/item/[section/]field` (`cli.rs`,
  the same references `op item get --format json` gives each field). An
  item's right-click menu (the list's and the window's Item menu) gets
  Copy Reference (the item's password, or its first secret field: what
  `op read` wants most), and each field's own menu in the item view Copy
  Reference for that field; the reference is the one `cli.rs` makes (one
  function for both), checked by resolving it back (`resolve`) in a
  test, and shown to agents as the menu items are.

- ~~**Faster Rust builds: the low-hanging fruit**~~ (done 2026-11-05,
  HACKING's Layout: there isn't any. Measured on the same edits:
  incremental release 103/46/3.4 s and one codegen unit 159/190/6.1 s
  against today's 92/47/2.8 s (kit, app_server, a leaf app); LTO was
  never fat; the linker is rust-lld already; kache caches. A kit change
  is ~1,000 CPU-seconds over 35 apps. Build the one app worked on
  (`-p`), the workspace at the end) (Ben, 2026-11-05): "see
  if there's any low hanging fruit to speed up the rust compilation.
  maybe lto = "fat" → "thin" and incremental = true if they're not
  already set?" As it is: `[profile.release]` sets only `debug =
  "line-tables-only"` (no `lto`, so Cargo's default, thin-local; no
  `incremental`, off for release; 16 codegen units), dev builds dependencies
  at opt-level 3, and there's no linker setting (the system's `ld`). A
  release build of the workspace after a small change takes 1–3 min here
  (2m 55s after a change to olo-session and Terminal, 7–55 s for one
  crate). Measure each candidate on the same change (a kit file, an
  app_server file, a leaf app), clean and incremental, and keep what
  helps without making the result slower (runtime measured, as Olo's
  rule asks): `incremental = true` for release; a faster linker (mold
  or wild; linking app_server and the GPUI apps is a large part of an
  incremental build); `split-debuginfo = "unpacked"`; codegen-units;
  the dependency tree (duplicate versions, heavy proc-macros, features
  nobody uses); `cargo build --timings` to see where the time goes.
  Numbers in HACKING.

- ~~**Viewer crashed when brought back from Deskbar after a video**~~
  (done 2026-11-05 as far as can be without it happening again: the
  crash reproduced headless was GPUI's atlas, fixed with a GPU test that
  fails without it; a panic in an app started from Deskbar now leaves a
  crash report naming it, so if Ben sees it again it says where)
  (Ben, 2026-11-04): "i was watching a video in viewer, then left it
  open while i did some other stuff, then when i tried to click it in
  the deskbar to come back it crashed". A video played in Viewer, the
  window left open (in the background, perhaps hidden or minimized, the
  video ended or paused) while other things were done, then its Deskbar
  entry clicked: Viewer crashed instead of coming forward. Find the
  crash (its panic or core in the journal, Logs), reproduce it headless
  (a video played, the window left behind other windows and minimized,
  time passing, then activated as Deskbar does), fix it at the cause,
  and keep a test that does that. *Looked at* (2026-11-04, minutes
  after): Viewer (window 22, opened 15:29) was gone, and it left
  nothing: no panic in the journal, no core (`coredumpctl`), no kernel
  line (segfault, OOM). So either it exited on purpose (on Deskbar's
  launch, say, handed to the running copy) or it died in a way nothing
  records: the next item is what would have told us. Why nothing: an
  app started by Tracker, Deskbar or another app (`olo_xdg::open::
  spawn_app`) has its stderr sent to /dev/null, so a panic's message
  is lost, and a panic exits 101 without a core. Not reproduced
  headless: a 1:51 H.264 video played, minimized, left past its end,
  then its Deskbar entry clicked: it came back at its end, no panic,
  memory steady (162 MB) while hidden. Try next with what the crash
  report says, else: behind other windows rather than minimized, on
  another workspace, across a lock or the display sleeping.
  *Likely found* (2026-11-05): the Viewer crashed headless with two
  videos open (a panic in GPUI's atlas: `flush_initializations` indexing
  an empty slot). A video makes a texture's picture a frame and lets the
  last go; the app's windows share one atlas (a menu, as Deskbar's is
  clicked, is a window too), so a texture made and freed before the
  next frame left its initialization and upload queued, and the frame
  panicked on the empty slot (or wrote a stale picture into the texture
  that took it). Fixed in GPUI (`BladeAtlas::remove`), with a test on
  the GPU that fails without it. Crash reports now name such a panic
  (crates/crash), so if Ben's happens again it says where.

- ~~**Terminal tells coding agents to keep scratch in /var/tmp**~~ (done
  2026-11-05, HACKING's agent context: `SCRATCH`, said at the start with
  the group context where /tmp is a tmpfs; checked by running the hook as
  Codex would and reading what it says) (Ben,
  2026-11-05, after the items above): "the olo terminal claude/codex
  integration should also inject instructions to those coding harnesses
  that if they need to write/use files to a temp location they should
  use /var/tmp and not /tmp and explain why". Terminal's hook that
  gives Claude Code and Codex the group context and the tab-title
  instructions (`agent_context.rs`, `agent_title::INSTRUCTIONS`) also
  says, when /tmp is a tmpfs: temporary files (scratch, build
  directories, downloads, screenshots, copies of builds) go in a
  directory of the agent's own under /var/tmp, removed when done, never
  in /tmp, because /tmp is held in RAM and whatever goes there takes
  the person's memory (as CLAUDE.md's rule for Olo's own agents says).
  Said once at the start, short; checked by reading what each agent is
  given (Claude Code's hook output, Codex's) in a test, and off with the
  group context's setting.

- ~~**Olo Doctor: programs' temporary files on disk, not in /tmp**~~
  (done 2026-11-05, HACKING's Olo Doctor: "Temporary files take memory"
  while /tmp is a tmpfs and the session's TMPDIR is unset or in memory;
  "Keep Temporary Files on Disk" writes an environment.d file of Olo's
  and sets TMPDIR, TMP and TEMP in the running user manager; olo-session
  takes them as it starts. Tested: the check's four cases, the file read;
  the finding reviewed headless at 2×, 1.5×, 1×. Sockets in $TMPDIR work
  on disk; /var/tmp is aged by systemd-tmpfiles after 30 days)
  (Ben, 2026-11-05, after the items above): "have olo doctor check what
  the env vars are for telling processes what temp dir path to use and
  if they're not present or not set to /var/tmp then have it provide
  that as a fix. /tmp is often in memory and not a good place for apps
  to use as temp". Doctor reads the session's environment (and
  systemd's user manager's, `systemctl --user show-environment`'s
  D-Bus equivalent) for `TMPDIR`, and `TMP`/`TEMP` that some programs
  read instead; when /tmp is a tmpfs (RAM: Fedora's default) and they
  are unset or point there, it offers the fix: set them to /var/tmp for
  everything the person runs from now on (an `environment.d` file of
  Olo's in ~/.config, which systemd's user manager and olo-session
  read, so apps started from Deskbar, Terminal's shells and user
  services all get it; programs already running keep theirs until they
  restart, which it says). Natively, no shelling out; undoable (the
  file taken away). Look first at what breaks: a program that puts its
  sockets in $TMPDIR (they work on disk too), and what systemd-tmpfiles
  ages out of /var/tmp (30 days there, not at boot) against what /tmp
  dropped at each restart; `XDG_RUNTIME_DIR` stays in RAM as it should.
  A test with a session whose TMPDIR is unset, then set by the fix.

- ~~**Terminal's suggestions see the session's recent commands and
  output, after a restore too**~~ (done 2026-11-05, HACKING's brush
  suggestions: a shell's session is its Terminal shell id, a restored
  tab's carrying on the old one's last ten commands; olo-termd keeps each
  shell's last 80 lines as text and olo-complete puts the latest command
  they name that fits first: Ben's restored tab, `cl`, his `claude …
  --resume` (a test), and headless, a printed command as brush's hint.
  2 µs a suggestion as before. Left as the model's job, not done: features
  of the output in the model itself (it would need retraining). The
  installed brush comes up to date through Olo Suggestions) (Ben, 2026-11-05, after the items
  above): "the olo terminal autocomplete model should be able to also
  receive like the last n commands (maybe 10 idk whatever makes sense)
  in the session the user wrote (and if it's a restored session after
  e.g. a restart it should still see it from the captured scrollback)
  and also the recent output so that it can take all that into account
  when determining the completion prediction too". What it has now
  (`crates/complete`): the daemon keeps each shell's last 8 commands in
  memory (`index::Session`, keyed by brush's process id), and the
  model sees them only as "after the last command", "two after", "uses
  in this shell" and "in the last ten"; nothing of the output. A shell
  restored after a restart or an upgrade (Terminal's scrollback brought
  back, `replay.rs`) is a new process, so it starts with none. To do:
  Suggest carries (or the daemon keeps per tab, not per process) the
  session's last commands (about 10, measured: what helps most without
  slowing a suggestion past its budget) and a digest of the recent
  output (the last command's status and lines: an error naming a file,
  a path, a branch, a test that failed, `git status`'s files), and a
  restored tab's are taken from its saved scrollback (Terminal knows
  each command's line and output from its prompt marks); the model gets
  features from them (a candidate naming a word in the recent output,
  repeating a command that just failed after an edit, the next step of
  a sequence run before), trained from the history as it was typed
  (the output kept for training only as those features, never stored
  raw). Measure accepted-suggestion rate before and after (Activity
  Monitor ▸ AI's numbers), and latency per suggestion (it's per key).
  Ben's example (2026-11-05, a tab restored after a restart): the
  scrollback ends with `claude --dangerously-skip-permissions` printing
  "Resume this session with: claude --dangerously-skip-permissions
  --resume 00000000-…", then `claude --allow-dangerously-skip-permissions
  --dangerously-skip-permissions --resume 00000000-…` run, then
  "Restored from Nov 5, 2026, 9:15 AM", `cd ~/src/notes`,
  and `cl` typed: it suggested `claude --help|grep allow`; "here i would
  expect the model's prediction to be that claude command to resume
  with the session id etc". So the candidates can't come only from the
  history index (`Index::candidates`): a command the output names (a
  line that is a command, "Resume … with:", "run `…`", `git push
  --set-upstream origin …`), and the last command of the restored
  session (rerun as it was), are candidates too, scored with the rest.
  Keep this as a test: that scrollback restored, `cl` typed, the resume
  command suggested.

- ~~**HDR versions of the wallpapers, shown in HDR**~~ (done 2026-11-05,
  HACKING's Backgrounds: a gain map beside each of the pack's 56
  pictures, made from it in Rust (`olo_images::gain`, `wallpaper-hdr`:
  what stands out shines up to two stops, plain bright surfaces stay put),
  7.5 MB in all; app_server multiplies it in on a display in HDR into
  half floats of linear light, drawn with that colour source; Backgrounds
  says so. `hdr-check`: the BeBox's lights at code 185 on the signal (SDR's
  white is 148), its median 141; 94 ms and 48 MB at 3200 × 2000 against
  SDR's 73 ms and 24 MB, off the loop. Rather than 16-bit HDR copies (GBs
  in Git LFS) or JPEG XL HDR (no encoder in Rust), the Ultra HDR way) (Ben, 2026-11-05,
  after HDR and the tab-title bug): "generate and add hdr versions of
  all the olo wallpapers that are used when in hdr mode". Every
  picture in the pack (`data/wallpapers/pack.txt`; each `4k/`,
  `ultrawide/` and `-dark`) gets an HDR version beside it, its
  highlights (light sources, glints, the sky's brightest) brighter than
  SDR's white and its colours past sRGB where the scene has them, kept
  as a deep format app_server reads natively (16-bit PNG in PQ with
  BT.2020, or JPEG XL HDR, which jxl-oxide decodes). Made from the
  SDR originals by a tool of our own in Rust (`tools/` or a `bin`, no
  outside programs), so it's repeatable as the pack grows: the
  picture's own SDR look kept (SDR white at the display's SDR white),
  only what's brightest lifted into the headroom, smoothly; checked by
  eye on the built-in OLED in HDR. `wallpapers::resolve` picks the HDR
  file for a display in HDR (the effective theme and screen shape as
  now), app_server's `wallpaper.rs` decodes it into the HDR blending
  buffer's scale (cd/m² relative to SDR white, not clipped to 1) and
  draws it there, and switches with HDR on and off live; a display in
  SDR (or a picture with no HDR version) is unchanged. Backgrounds says
  which pictures have HDR. Measure: decode and upload time and memory
  of a 4K deep picture against the SDR one (the worker, the strips).

- ~~**Other programs survive Olo's upgrades**~~ (done 2026-11-05,
  HACKING's olo-wlproxy notes: two causes, both at the proxy's replay
  onto the new app_server. Every program that had asked for touches had
  its connection ended: the new seat had no touch screen yet (libinput
  finds it seconds later) and Smithay answered the replayed `get_touch`
  with "missing touch capability" (Chrome, Slack, the GNOME portal on
  both days); now a touch waits for the seat's touch, and Smithay gives
  inert objects rather than ending a connection. And GTK crashed on a
  tablet cursor's leaked fractional scale, its data freed when told the
  tablet went (Chrome and xdg-desktop-portal-gnome, 2026-11-04); now a
  surface the program let go, or never showed, isn't told its scale.
  `tools/dev/outside-check`: GTK 4, Slack, Chrome and Qt 6 through five
  app_server restarts at 1.5× with a late touch screen and a pen tablet,
  each with its window; before, all four died at the first. Also, as
  Ben asked: each upgrade ends programs of ours left from sessions that
  have gone, `sweep_leftovers`) (Ben, 2026-11-05, after the
  HDR wallpapers): "every time i upgrade my non-olo apps, e.g. slack and
  chrome, crash or are no longer shown in olo. please fix so that they
  relliably survive upgrades." Olo's own apps reconnect as app_server
  reloads in place (`olo_ui::on_upgrade`); other Wayland programs
  (Chrome, Slack (Electron), GTK, Qt) either crash or keep running with
  their windows gone. Known so far: at the 2026-11-04 upgrade Chrome and
  xdg-desktop-portal-gnome crashed in GTK 4's
  `pointer_surface_fractional_scale_preferred_scale_cb` (a
  fractional-scale event for a pointer surface GTK had let go of). Find
  what each of them sees across a reload (the socket, the globals, the
  events in their order, WAYLAND_DEBUG on a test session's copy of each),
  for each failure (the crash, the windows not shown), and fix it at the
  cause on our side, so the person's other programs and their windows
  are exactly where they were after every upgrade: keep the clients'
  connections through the reload if that's what it takes (hand the
  listening socket and the client fds to the new app_server, with their
  state), or whatever makes it reliable. A test that upgrades a test
  session with Chrome, an Electron app, a GTK 4 and a Qt program open
  (windows, popups, a fractional scale), many times over, and checks
  each is alive with its windows shown.

- ~~**Agents can't set their Terminal tab's title**~~ (done
  2026-11-05, HACKING's "Coding agents' tab titles": three causes.
  Terminal took no handed-over launches since its GPUI port (no
  `instance::listen`), so every title failed "Terminal isn't
  answering"; Codex starts MCP servers with a bare environment (no
  XDG_RUNTIME_DIR), so its server found no snapshot and said the session
  wasn't in a tab (now `olo_xdg::runtime_dir()`, /run/user/UID); and
  olo-session stopped Claude Code's server 90 s after an upgrade as an
  old version, which Claude Code never restarts (now it re-executes in
  place between messages, and the stale check leaves alone what another
  program started). By folder, the tab running the same agent first.
  `tools/dev/agent-title-check` drives it as each agent runs it: all
  as they should be. Servers running from before are left as they
  are: Claude Code's work once Terminal is the new version (they have
  the runtime dir); Codex's need Codex started again; one already
  stopped comes back with its agent's next start) (Ben, 2026-11-05,
  next after HDR): "it seems like in codex (and maybe claude code too)
  setting the olo terminal title isn't working (the session most
  definitely was/is running in olo terminal)". Codex in the tab at
  ~/worktrees/olo/browser (group "olo") called
  `olo-terminal.set_tab_title({"title": "Browser CSS syntax", "cwd":
  "/home/ben/worktrees/olo/browser"})` and was told "This session isn't
  in an Olo Terminal tab (or Terminal's setting for it is off)": so
  `agent_title::find_tab` found no tab, even given the folder (Codex
  runs the server in its daemon, so it goes by the folder; the group
  context the hook gives that session lists the tab at that folder, so
  the snapshot has it). Claude Code fails another way: in the tab at
  ~/src/olo, after the conversation was compacted, the olo-terminal
  server was "disconnected" and the tool gone for the rest of the
  session (was the server's binary replaced by an upgrade under it, or
  did it exit?). Find each cause (the snapshot the server reads and
  where from: its environment in Codex's daemon, XDG_RUNTIME_DIR,
  display; how `tab_of` matches a folder; what ends the server under
  Claude Code), fix them, and keep tests that drive the server as each
  agent runs it (Codex: from a daemon outside the tab, by folder;
  Claude Code: under the tab's shell, across an upgrade of Terminal).

- ~~**Activity Monitor: several processes chosen, one action on all**~~
  (done 2026-11-05, HACKING's Activity Monitor: chosen the usual way,
  the row menu, buttons and Process menu acting on all, one Force Quit
  alert naming them; tried headless on three `sleep`s: chosen with a
  click and a ⇧-click, "Force Quit 3 Processes…", "Force 3 processes to
  quit (sleep ×3)?", all three gone)
  (Ben, 2026-11-04, for later): "in activity monitor i should be able
  to multi-select processes and take action on them all at once via
  right clicking". Processes ▸ the table chooses several rows the usual
  way (Shift-click a range, Ctrl-click one more, Ctrl+A all those the
  filter shows, Shift+arrows), and the row menu (`processes.rs`
  `row_menu`, now for one pid), right-clicked on any chosen row, acts on
  every chosen one: Quit, Force Quit… (one alert naming how many and
  which: "Force quit 12 processes (chrome ×11, olo-fuzz)?"), Send
  Signal, Priority (checked where all agree, a dash where they differ),
  Copy Process IDs / Command Lines (one a line). Inspect opens the
  first; Show Program in Tracker the distinct programs. The buttons
  below (Inspect, Quit, Force Quit…) and the Process menu act on the
  chosen set too; a row that leaves the list (quit, filtered out) leaves
  the set, so an action never reaches a process that isn't shown
  chosen. Rows with a tree open (Tree) choose their children with them
  when the parent is chosen collapsed. Agents get the same: the
  table's chosen rows readable, and the actions taking several pids.

- ~~**Themes after NeXTSTEP, Mac OS 9 and AmigaOS**~~ (done 2026-11-05:
  Cube, Platinum and Workbench in `olo_theme::looks::THEMES`, each a
  full set (135 icons at every size, drawn by gpt-image-2.5-sunburst at
  high against the theme's own five references, reviewed on contact
  sheets on light, dark and night, crisp at 1.5×), a wordmark
  (`tools/genwordmark.py`, new), wallpapers light and dark, and its own
  colours light and dark (Cube's greys and black tab, Platinum's
  lavender tab and lilac-blue, Workbench's grey and blue by day and
  1.3's deep blue and orange by night), with BeOS's and Catppuccin's;
  Preferences ▸ Themes lays the colour sets beside the ten rows, so it
  still fits 1280 × 800; the pack's old "Workbench" pattern is "Ring
  Pattern" now. Reviewed at 1×, 1.25×, 1.5×, 1.75× and 2×, light and
  dark: clean) (Ben, 2026-11-04):
  "add extra themes for olo- NeXTSTEP, MacOS 9, AmigaOS. they must all
  come with light and dark variants, and have a complete, max quality
  icon set generated in the style of each of those OSes using sunburst
  image generator on highest quality settings. the custom icon set must
  cover everything for every olo app and place where there's an icon
  used". Three more themes beside the seven in `olo_theme::looks::THEMES`
  (each a `data/themes/<name>/` with its icons, wordmark and wallpaper,
  and its own colour set made from `Seeds`, light and dark, plus BeOS's
  and Catppuccin's as every theme has): NeXTSTEP's (greys, black
  window bars, its bevelled 3D and the dimpled grey of its dock and
  icons, black-and-white 2-bit look carried to colour), Mac OS 9's
  (Platinum: light grey bevels, pinstriped title bars, purple-blue
  selection, its soft-shadowed icons), AmigaOS's (Workbench 3.x: blue,
  orange, white and black, its chunky bevels and pixel-crisp icons;
  dark the Workbench 1.3 blue). Each the look and the feel only, drawn
  as Olo's own (no Apple, NeXT or Commodore logos, names or artwork:
  the Classic theme's rule). Every icon a full set has (135 now in each
  of `data/icons` and every theme's, every size from 16 to 256 px, each
  app's, file kinds', places', tray's and controls'): generated with
  `gpt-image-2.5-sunburst` at quality `high` and full size as
  docs/ICONS.md says, with each theme's own reference icons made first
  and every icon after against them, so the set looks drawn by one
  hand; checked at 16 px and at every scale (`icons::paint`, one pixel
  to a device pixel), on light, dark and night; Preferences ▸ Themes
  showing each. Needs an `OPENAI_API_KEY` (not set in the sessions so
  far: the A2520 keyboard's icon waits on it too).

- ~~**wm fuzzer: its last faults**~~ (done 2026-11-05: seed 38, Zoom
  Horizontally twice on a Tracker sized since its zoom came back at
  the old width: unzooming both ways goes back to the frame it had
  zoomed one way, as it was then; seed 45, Tile Bottom-Left on a
  Terminal whose hand-made group's mate was just shown again: the
  group formed round the mate, in front, pulling the Terminal out of
  its tile: now the window arranged leads its group as it forms
  (`Groups::leads`, hand-made groups only). Waves: wm 48 of 48, mode
  33 of 33, auto --reloads 33 of 33 (three failed under the wave's
  load and passed alone, as before). Per raise unchanged: 1.27–1.30 ms
  of app_server's CPU, before 1.30.) (Seeds 6, 8, 19 and 25 fixed
  before: a zoomed window kept its size when its app resized itself;
  the check's straddling window.)

- ~~**The command palette's suggestions from a tiny model, as the
  Terminal's**~~ (done 2026-11-04, HACKING's "Ranking": olo_complete's
  kernels and trainer on 16 features of the palette's own (the match,
  kind, enabled, how often, how lately, this hour and weekday, after
  this typing, with this app, the last chosen, its kind's share, open,
  lengths); hand-written weights ranking as before; every choice kept as
  an example and the model tuned after each on a worker, 5.7 ms on a
  full history; 0–34 µs more a keystroke; checked headless, twelve
  choices tuning it) (Ben, 2026-11-04): "similar to the terminal commands tiny
  autocomplete model, make the command pallette suggestions also use a
  tiny simd accelerated model. they should get similar features like
  time of day, available commands, what the user has typed so far and so
  on. make sure it also auto trains/fine tunes periodically like the
  terminal one, keeping track of what the user types and ultimately
  selects and so on. if the fine tuning / training is super fast and
  lightweight then just do it after each new sample is collected." On
  the CPU's SIMD (never the NVIDIA GPU), measured per keystroke.

- ~~**Agent Screens: Stop keeps a screen; Archive and Delete**~~ (done
  2026-11-04: Stop and Archive and Delete were there, but a stopped
  screen was listed only by its history, so one no agent had acted on
  (made with New Screen) went from the list when stopped, and nothing
  started a stopped one. Now every screen is kept from its start
  (`screen.json` beside its history), listed as Stopped once stopped,
  started again as it was (Start in the menu, Start Screen in its
  header, `olo-screen start NAME`); the Archived section shows unless
  View ▸ Show Archived is turned off; `olo-screen list` says each one's
  state, with `archive`, `unarchive` and `delete`. On the way: Clear
  History deleted the screen's whole folder (its Chrome profile and
  shared passwords with it); now only its actions and frames. Checked
  in a sandbox, by the command line and the viewer) (Ben,
  2026-11-04): "stopping an agent screen makes it dissappear completely.
  it should just change to be in a stopped state and able to be started
  again. and then there should be an option to archive it and archived
  ones should be in the menu too in their own section. and then an
  option to delete fully as well." Stop ends a screen's session but
  keeps it (its home, its history, its name) as Stopped, listed with
  Start; Archive moves a stopped one to an Archived section of the
  menu (Start brings it back); Delete removes it and everything it
  kept, asked first. The same for agents: `olo-screen stop` (keeps),
  `archive`, `delete`, and `list` saying which is which (apps/screen,
  HACKING's "Agent Screens").

- ~~**app_server crashes as it exits**~~ (done 2026-11-04: the
  `dmabuf check` worker is told to quit and waited for as app_server
  ends, before the renderer and EGL go (`Checks::stop`, at the end of
  the udev and headless loops, and on drop), leaving what it holds for
  the process's end to free rather than destroying it during EGL's
  teardown; the wait is bounded (500 ms) so a stuck driver can't hold
  up a reload, and takes 0.16 ms. Fixed from the cores' stacks: the
  race is narrow and didn't happen on demand, neither before nor after,
  in 20 headless stops at 3024 × 1964 with seven windows and 12 reloads
  under olo-session) (found 2026-11-04 in the core
  dumps of headless test sessions, `coredumpctl list`): a SIGSEGV in
  the `dmabuf check` thread freeing what it keeps
  (`drop_glue<dmabuf_check::Kept>` → `dri_destroy` → a corrupt heap),
  others in NVIDIA's EGL and in `__eglFini` while that thread still
  holds a lock. The thread outlives the renderer it borrows from as the
  process ends. An in-place reload is such an exit: stop and join the
  thread (its images dropped while EGL is still up) before the
  renderer goes, and check a stopped session leaves no core.

- ~~**Every Olo program leaves what's needed to debug its crash**~~
  (done 2026-11-04: `olo-crash`, installed first by every program's
  `main` (a test fails one that doesn't): a JSON report for a panic
  (message, place, thread, backtrace with files and lines) or a fatal
  signal (async-signal-safe; the core still made), in
  ~/.local/state/olo/crashes; stderr that went to /dev/null now goes to
  the journal, so Logs has every program's log and panic; Olo Doctor
  tells of each crash once, its click opening Logs there;
  `olo-doctor --crashes`. Nothing runs until a crash: 18–35 µs at
  start. Not done: the last log lines in the report (it would cost on
  every line logged; the journal has them, a click away), and "Report
  to copy" (the report's path is a link in Logs)) (Ben, 2026-11-04): "make sure we capture debugging information
  anytime an olo app crashes -- they should all dump the debugging data
  we need consistently and universally and it should have zero perf
  overhead". Viewer's crash above left nothing to go on. One mechanism
  for every program (the kit's apps, app_server, the helpers and
  services), put in by what they all start with (`olo_ui::run`,
  `olo_kit_base`), nothing for an app to remember: a panic hook and a
  fatal-signal handler (SIGSEGV, SIGABRT, SIGBUS, SIGILL, SIGFPE, on
  its own stack, async-signal-safe, then the default action so a core
  is still made) writing a crash report to
  `~/.local/state/olo/crashes/`: program, build, thread, the panic's
  message and backtrace (symbolized: release builds keep line tables,
  or split debug info beside them), the signal and its address, how
  long it had run, and what it was doing (a few last log lines).
  Shown in Logs, told once by Olo Doctor ("Viewer crashed", with
  Report to copy), kept to a few per program. Zero cost until a
  crash: nothing polled or recorded per frame, measured (startup time,
  idle wakeups, binary size before and after). Then exits that aren't
  crashes (a program quitting on its own when it shouldn't) say why
  in the journal too.

- ~~**Olo Suggestions: install just, uv and mise**~~ (done 2026-11-04:
  just and uv from Fedora's repositories through PackageKit, mise from
  its latest GitHub release into ~/.local/bin, its published SHA-256
  checked, all native (`releases.rs`); each said installed, and where,
  once it is; the download's share shown as it goes) (Ben, 2026-11-04):
  "add installing just (for justfiles) to olo suggestions", then "add
  installing uv to olo suggestions", then "add installing mise to olo
  suggestions".

- ~~**Olo Suggestions: worktrunk, and a notification for each new
  suggestion**~~ (done 2026-11-04: worktrunk's `wt` from its GitHub
  release; its `worktree-path` set to Terminal's place,
  `<place>/{{ repo }}/{{ branch | sanitize }}`, the rest of its config
  kept, undone to what it was; checked with a real `wt switch -c` in a
  sandbox. Each new suggestion told once ever, by id and title, in a
  notification from Olo Suggestions whose click opens it. On the way:
  a banner's actions no longer run past its edge (stacked when they
  don't fit side by side), and `--apply` with an unknown id does
  nothing) (Ben, 2026-11-04): "add to olo suggestions the installation
  of https://worktrunk.dev/ and auto-configuration to have it put
  worktrees in the same place as olo terminal. also have olo
  suggestions show a notification when a new suggestion is available,
  but don't want it to repeatedly come up just show the notification
  once for any given suggestion."

- ~~**Escape ends a drag, anywhere**~~ (Ben, 2026-11-04): "when dragging
  something in olo if the user presses escape it should drop the drag".
  Escape while anything is being dragged, a file, text, a tab, a window
  (app_server's move grab: the window back where it was), a Deskbar
  entry, a list's band, cancels it: nothing dropped, the source as it
  was, the pointer's drag look gone. In app_server for a Wayland
  drag-and-drop (the data device's grab ended, `cancelled` to the
  source, as other compositors do) and its own grabs; in the kit
  (`olo_ui::dnd`, GPUI's drags) for drags within a window. *Done*: app_server ends a
  drag-and-drop on Escape (Smithay's cancel: the source told, the
  window under it left) and puts a window being moved back where it was
  (zoomed again if it was, an auto-tiled one in its tile); the kit's
  Frame ends GPUI's own drags in a window. The Escape reaches nobody.
  Tested on a test session: a file dragged onto a folder with Escape
  before the release stayed; a window moved by its tab came back; both
  without Escape as before. (GPUI's in-window drags not tried by hand.)

- ~~**Tracker: a drag that starts on a name sometimes doesn't take**~~ (Ben,
  2026-11-04): "in tracker when trying to drag and starting on the
  filename sometimes it doesn't register and i have to try again". A
  press on an icon's name that moves past the drag distance must drag,
  every time, as one on its icon does; look at `icon_release`'s
  click-to-rename (`on_label`, `pending_edit`, apps/tracker/src/
  folder.rs) and the press's drag threshold on the label, and test it
  on an Agent Screen (a press on the name, moved 3, 4, 10 points, in
  each view and at each scale). *Done*: not the
  name: any quick drag (press, one quick move, let go) was dropped on
  nothing, from an icon too, in any app. The app asks app_server to
  start the drag once the pointer has gone its drag distance; by then
  the pointer was over where it went and didn't move again, and the
  drag only learned of a target on the next move. app_server now tells
  the drag where the pointer is as it starts (`dnd_requested`). Tested:
  three quick drags from the name and one from the icon, all moved
  (none did before).

- ~~**Tracker: Back and Forward, from the keyboard and the mouse**~~ (Ben,
  2026-11-04): "in tracker, it should support forward and back via
  keyboard shortcut or mouse". A folder window keeps the folders it
  showed (each with its selection and scroll), and Go ▸ Back (⌘[) and
  Forward (⌘]) move through them, as Finder's; the mouse's back and
  forward buttons (GPUI's `MouseButton::Navigate`) and a touchpad's
  two-finger swipe sideways do the same. Open Parent (⌘↑) stays as it
  is, and adds to the history. Agents read and press them as menu
  items; HACKING's Tracker section told. *Done*: Go ▸
  Back and Forward (⌘[ ⌘], Alt+← → on the Windows profile) and the
  mouse's back and forward buttons, through the folders a window showed,
  in that window (Tracker stays spatial: opening a folder still opens
  its own window, whose Back goes to the folder it was opened from).
  Coming back out of a folder selects it. The swipe was left out: two
  fingers sideways scroll Tracker's lists.

- ~~**Tracker: ⌘N opens a new Tracker window**~~ (Ben, 2026-11-04): "in
  tracker, pressing super+n should open a new tracker window". ⌘N is
  New Folder now (apps/tracker/src/folder.rs, `background_menu`): it
  becomes File ▸ New Window (a window on the folder it's pressed in, or
  Home from the desktop, as Finder's), and New Folder moves to ⇧⌘N, as
  Finder has them; the shortcut help, the palette and HACKING's Tracker
  section told. *Done*: File ▸ New Window
  (⌘N, Ctrl+N on the Windows profile, none on BeOS's, where Alt+N is
  New Folder as in its Tracker): another window on the folder it's
  pressed in, Home from the desktop; `--new-window` always opens one
  too, as HACKING asks (it brought an open Home forward). New Folder is
  ⇧⌘N (`app_keys`' `tracker.new` and `tracker.new-folder`).

- ~~**Auto-tile: a click on a zoomed window's tab unzooms it**~~ (Ben,
  2026-11-04): "on a workspace in auto tile mode i zoomed the passwords
  app. it worked correctly, but then when i click the passwords yellow
  title bar (not on either of the buttons just the bar itself) it looses
  zoom. it should only loose the zoom if i click the zoom button again."
  A plain click on the tab (no drag) must leave the zoom as it is. *Done*: a press on a tab started a
  move that unzoomed the window at once (Smithay's example's way) and
  its release was taken as a drop; now a move starts only once the
  pointer has gone 3 points (a finger 6: `MOVE_AFTER`,
  `TOUCH_MOVE_AFTER`), and only then is a zoomed window unzoomed. A
  click changes nothing; a drag still unzooms it and drops it in a
  tile. Tested on a test session (zoom, click, drag).

- ~~**Backup: Select All and None, exporting and importing**~~ (Ben,
  2026-11-04): "the prefs -> backup app needs options to select all /
  unselect all for import and export". Each of the Export and Import
  lists of parts gets All and None (the box's LinkButton actions, or a
  row of them above the list), enabled as they'd change something. *Done*: Select All and Select None
  above each list, each enabled while it would change something; the
  Export and Import boxes side by side, so the panel still fits a
  1280×800 screen (ui-lint clean at 1×, 1.5× and 2×).

- ~~**Terminal: a new tab opens in the folder of the tab it came from**~~
  (Ben, 2026-11-04): "when opening a new tab from an existing one in
  olo terminal the new tab should open in the same cwd as the current
  one". File ▸ New Tab already asks for it (`Session::cwd`, the shell's
  `/proc/PID/cwd`, apps/terminal/src/main.rs "new-tab"), so find where
  it goes wrong: brush (Ben's login shell) may keep its working folder
  without changing its process's (then `/proc` says where it started:
  take OSC 7's folder, which shells report as they change it, first),
  and every other way to a new tab (the tab strip, a tab's menu, ⌘T,
  `--tab`, Deskbar) must pass the folder too. Test with bash, zsh and
  brush on an Agent Screen. *Done*:
  the same cause as the worktree tab (brush never moved its process on
  `cd`, and New Tab reads the folder from `/proc`): olo-brush now does.
  Every way to a new tab already passes the tab's folder.

- ~~**Terminal: a tab in a worktree isn't seen as the repository's**~~
  (Ben, 2026-11-04, with a screenshot): "for some reason terminal isn't
  picking up my worktree and github pr associated with them like it
  recognized the main work-repo repo but then didn't recognize the
  notes worktree". The first tab (in ~/src/work-repo) is
  in the work-repo group with its PR badge; the second, `cd`'d into
  ~/src/notes, sits outside it with no branch or PR. That
  folder is a linked worktree (its `.git` file: `gitdir:
  /home/ben/src/work-repo/.git/worktrees/notes`, branch
  `knowledge`), made at 11:37 that day, outside Terminal (git or
  worktrunk). apps/terminal/src/git.rs's watcher should take it as
  work-repo's, on its branch, with that branch's PR. *Done*: the
  worktree was fine (git and Terminal's `git::look` read it); the tab's
  folder wasn't. Terminal reads it from the shell's `/proc/PID/cwd`, and
  brush (the login shell) never moved its process on `cd`. olo-brush
  now does, at each prompt (a `prompt` hook in vendor/brush-interactive;
  tested through a PTY: `cd` into the worktree, `/proc` says so). The
  brush installed as the login shell is a copy: Olo Suggestions' Update
  brush brings this one.

- ~~**Grouped and tiled windows come back in their order**~~ (Ben,
  2026-11-04): "grouped windows and tiled tabs/windows should remember
  their order so that after e.g. an upgrade they come back in the same
  order". A group's tabs and an auto-tiled workspace's windows, restored
  after a reload or upgrade (`layout`, `autotile`), in the order they
  were in. *Done*: reproduced (a Terminal
  group reordered, auto-tiled beside StyledEdit, then app_server and
  the apps upgraded together: the moved tab came back third and the
  tiles were rearranged). Two causes, both fixed: the session nudged the
  apps to re-execute as it restarted app_server, so they re-executed
  twice over and Terminal saved its windows half gone (now app_server
  restarts first and what's still old is nudged 6 s after); and a
  window found its place by title, which many share (now by a key its
  app gives it, `WindowOptions::restore_key`, Terminal's a number per
  window kept in its state). `tools/dev/order-check` passes.

- ~~**Preferences ▸ Backup: choose what goes in, exporting and importing**~~
  (Ben, 2026-11-04; next): "in prefs -> backups made it
  so that i can choose each type of thing i want included in an export
  and when importing. e.g. maybe i have an export with everything but i
  just want to include token pooler settings/accounts for my import".
  Each kind of thing a backup holds (the desktop's settings, each app's,
  Token Pool's accounts and settings, passwords, …) a check box, all on
  at first: exporting writes only those checked; importing reads what
  the file has, offers only those (with what each holds), and restores
  only those checked, leaving the rest as they are. *Done*: a check box for each part (each app's
  files with the keyring's secrets that go with them, and the Wi-Fi
  networks) in Export (this computer's, read on a thread; those only a
  sealed backup holds disabled while it isn't) and Import (the
  backup's); `export_only`/`import_only` with a `Choice`, the rest left
  as it is here (what's here still all saved first); `--only "Token
  Pool,Passwords"` for agents. Tested (`only_the_parts_chosen`,
  `this_computers_parts`), and by clicks on a test session.

- ~~**Olo Rescue: Ctrl+Shift+Super+Delete, always**~~ (Ben, 2026-11-04):
  "an Olo Rescue app where, at any time ever in olo if you hold down
  ctrl+shift+super+delete then olo brings up a special, full screen
  rescue mode where the user gets a terminal as well as rescue
  functionality like killing processes and so on. this should run on
  its own thread with the highest priority and be as isolated from
  everything else as possible so that we minimize the chance that
  whatever problem the user is experiencing won't prevent them from
  using olo rescue. it should also switch -- just for olo rescue -- to a
  safe/simple configuration for displays etc. so that it can be used as
  a way to recover from bad display settings that left them unable to
  see anything. ideally they can also open any of the olo preferences in
  this mode too and change things there in case it was a preference that
  broke things." The chord caught in app_server's input before
  anything else (as the lock screen's), a realtime thread of its own;
  the displays put to a known-good mode (their preferred mode, sRGB, no
  HDR, no VRR, 100% or 200%, no mirroring) for Rescue only; a terminal,
  processes to end, and Preferences' panels, in a session that doesn't
  depend on what may be broken; leaving it puts everything back. *Done*: app_server's rescue.rs reads the keys first of all on
  its input thread (1.4 ns a key); every window put away on a
  workspace of Rescue's own; the displays, keyboard and pointer on safe
  settings (each display at its preferred mode and a scale that leaves
  it 1280×800 points, no HDR, VRR, rotation or mirroring, night light
  off, a too-dim screen brightened) while the person's own are what
  Preferences shows and changes, applied as Rescue ends; olo-rescue
  (apps/rescue, at rtkit's highest priority) with a terminal of its own
  in process (no olo-termd), the processes to end, Preferences' panels,
  Reset Display Settings, Start Over, Log Out, Restart and Leave Rescue;
  no way past the lock; kept through a restart in place, and the
  session starts app_server in Rescue after two crashes in a minute.
  HACKING's "Olo Rescue"; `tools/dev/rescue-check` (17 checks pass).
  Still to see on the machine itself: rtkit's priority (refused with
  EBUSY in test sessions, where many programs ask at once) and the
  keys on the built-in keyboard and the Magic Keyboard.

- ~~**Voice processing's input model: SIMD throughout, lowest latency**~~
  (Ben, 2026-11-04: "ensure that the voice processing model for input is
  fully SIMD accelerated and ultra-optimized for low latency (not
  gpu)"). Measured on the ThinkPad P1 Gen 9 (Core Ultra 7 356H: AVX2 and
  FMA, no AVX-512), release, `cargo test --release -p olo-voice
  live_frame_time allocations_a_frame -- --ignored --nocapture`:
  - CPU only, nothing on a GPU. A 10 ms frame: 388 µs back to back
    (99th 418), 553–588 µs paced as live (a frame every 10 ms, the
    processor idle between: cold caches and clocks; 99th 1.2 ms); on
    the E-cores 709, the low-power ones 1.99 ms; the realtime thread is
    kept on a fast core already (`util_min_self`). Echo 2–35 µs, noise
    290, level 20.
  - The hot loops are vectorised and chosen at run time: perf shows
    tract's AVX2+FMA kernels (`fma_mmm_f32_64x1` 38 %, `8x8` 12 %:
    the GRUs' matrix-vector products), pffft and rustfft's SIMD;
    DeepFilterNet's own loops are under 10 µs a frame. NEON on ARM.
  - Allocations: 871 a frame, 98 % inside tract's inference (each op's
    output: tract 0.23 has no arena), ~4 % of the frame by perf
    (malloc and free); DeepFilterNet's own ~10, echo 2, level 11.
  - The DF decoder on its own thread beside the ERB decoder: 388 →
    270 µs back to back, but paced as live 582 → 601 µs (99th 1.19 →
    1.42 ms): waking a thread each frame costs more than it saves.
    Not kept.
  - The 30 ms look-ahead stays (the 10 ms model chops voices: lib.rs).
  What's left is weights small enough to stay in cache: next item.

- ~~**Activity: a 10–11 ms frame opening its panels**~~ (found
  2026-11-04 in the bench at 2×, 1 of ~208 frames in 2 of 4 runs).
  Looked into, nothing of Activity's: walking all 14 panels at the
  bench's size twice, no frame over 4 ms; 90 s on Overview, 1525 frames,
  p50 0.59 ms, p99 1.18; the three over 4 ms were the first after a
  zoom (49 lines shaped) and two doing no more work than their
  neighbours, slow in layout once and paint once; the swapchain's
  acquire and the wait for the last frame never over 2 ms; no
  preemption. With the CPUs' lowest clock raised from 400 MHz to 2 GHz,
  60 s: 1 over 4 ms (3 before), none over 8.3 (1 before). So a laptop
  idling deep, waking a core for a frame: the "Every frame on time"
  item's, not Activity's code.

- ~~**Columns moved by dragging, remembered per table**~~ (Ben,
  2026-11-04: "columns in a table like in the activity monitor should be
  orderable by dragging their positions (and this should be at the ui
  kit level) and the order should be remembered in that app for the
  table"). The kit's ColumnListView: a title dragged sideways moves its
  column, the others making room as it goes; cells, sorting and widths
  by each column's own index; the order and widths kept for the app by
  the table's id and the columns' titles (Activity's GPU columns, added
  after its first sample, take their places). Tried in Activity: User
  moved after Memory and Name narrowed, both back after a restart.

- ~~**Preferences ▸ GPUs: each GPU and every knob it has**~~ (Ben,
  2026-11-04: "if there are any knobs etc. that can be tweaked for the
  nvidia gpu, e.g. to make it run cooler/slower or hotter/faster or any
  other things then please add a new GPUs preferences app with each GPU
  and a full set of knobs/tuning options in there"). A box for each GPU:
  presets (Cooler and Quieter, Balanced, Fastest) and its knobs. Intel's
  xe: lowest and highest clock, power-saving profile. NVIDIA's (NVML):
  highest GPU clock (locked clocks), power limit where it can be given
  one (the P1 Gen 9's RTX PRO 2000 can't: a laptop's is the platform's;
  tried, NotSupported), persistence. Applied by `olo-gpu-tune set`
  through polkit, kept in /etc/olo/gpus.json and set again at start by a
  udev rule running a root-owned copy; agents: `olo-gpu-tune show`. Tried
  as root: xe Cooler (1600 MHz, power_saving) and back to its own
  limits; NVIDIA's clock capped at 2100 MHz and reset. Not tried yet:
  the udev rule at a real start, and whether NVIDIA keeps its clock cap
  through its sleep (D3cold), to see under load.

- ~~**Date & Time's worker asks timedated every 2 s**~~ (found
  2026-11-04): it read timedated's status every 2 seconds for as long as
  Preferences ran, keeping timedated running. Now it listens for
  timedated's `PropertiesChanged` (`olo_system::time::watch`) and asks
  only then, after a change it made, and every 10 s while network time
  is on and the clock isn't synchronised yet (not signalled). Preferences
  with Date & Time open: 12 wakeups/s before, 5 now (the clock's
  second; the worker none).

- ~~**Nothing routine wakes the NVIDIA GPU; local models run on the
  CPU's SIMD**~~ (Ben, 2026-11-04: "make sure that nothing in olo is
  going to try to use the nvidia gpu for routine things like window
  display etc. that would cause a sleep lag, and for any local models we
  have for e.g. voice processing, touchpad, brush completion -- make
  sure they use SIMD instead of nvidia"; "it's fine if it uses it for
  windows/drawing if it's the composing gpu on an external hdmi but
  otherwise it shouldn't"). Found: every GPUI app loaded NVIDIA's Vulkan
  driver to list devices, which woke the sleeping GPU and waited for it
  (DeskCalc's first window 2530 ms; now 77 ms); Chrome's GPU process
  the same (and kept it awake); app_server opened its card at every
  start. Now GPUI leaves NVIDIA's driver out as it makes its context,
  `olo-chrome` starts Chrome without NVIDIA's Vulkan and EGL drivers
  (`olo_kit_base::gpu::asleep_env`), and app_server opens NVIDIA's card
  only once a display is plugged into it (`left_asleep`; tried on the
  sysfs here, the live session's reload to come). No local model has a
  GPU to run on (no GPU compute crate in the build: tract and plain
  Rust); their SIMD is the "Voice processing's input model" item's.
  HACKING.md: "The discrete GPU stays asleep".

- ~~**A window that fits what it shows opens at its size, at once**~~
  (Ben, 2026-11-04: "when the adaptive/auto-sizing windows open they
  tend to first open too small then update to be the right size -- that
  should all happen basically instantly and before the user sees the
  window"). The Frame measures its content as each frame is painted; a
  frame drawn at a size about to change isn't shown
  (`Window::skip_present`: nothing committed, the resize draws the
  next), and GPUI's Wayland window keeps its geometry and a fixed
  window's least and most sizes in step with the resize (the first
  configure had put the old size back). What a window asks for as it
  opens holds its first showing until it's in, 150 ms at most
  (`olo_ui::hold_first_show`: Preferences' settings and displays,
  Display's GPUs, Date & Time's status). Every Preferences panel now
  first shows at its final size; launch to first frame unchanged
  (bench: 152/212 ms at 1×/2×, 152/213 before).

- ~~**Choose the GPU Olo draws with**~~ (Ben, 2026-11-03: "provide an
  option to choose the GPU that should be used for olo"). Superseded by
  Ben's own word on 2026-11-04 ("that shouldn't even be an option", "we
  should just use whatever the native one is"): each display is composed
  by the GPU wired to it, nothing to choose; Preferences ▸ Display's
  Graphics box shows every GPU found (make and model from the PCI
  database, its driver, the displays wired to it). Its knobs: the open
  "Preferences ▸ GPUs" item.

- ~~**The GPU the displays use, chosen in Preferences ▸ Display**~~
  (Ben, 2026-11-04). Tried: composing on NVIDIA's had every frame of the
  built-in panel (wired to Intel's) copied through the CPU, 30 Hz; then
  Ben: "make it so that the nvidia drives external displays not the
  internal", "that shouldn't even be an option", "we should just use
  whatever the native one is". Done so: each display composed by the GPU
  wired to it, no choice; Display's Graphics box says what each draws.

- ~~**The tray's battery shows the power plugged in**~~ (Ben, 2026-11-04:
  "the battery icon in the deskbar tray should change when the power is
  plugged in"): it showed the bolt only while charging or full; plugged
  in and held at a charge limit ("Not charging") looked unplugged. Now
  `Battery::plugged_in`.

- ~~**Terminal: a tiny model of its own completes brush's commands**~~
  (Ben, 2026-11-03): "in prefs -> terminal have an option for using a
  tiny ai model for brush shell command autocompletion that is
  accelerated either via simd or gpu. it should be based on the
  existing bash history and take into account what dir the user is in,
  what commands have come before in the session, time of day and day of
  week, and any other pertinent things (maybe other running user
  apps?). it should fine-tune extremely efficiently and quickly, like
  maybe updating every minute or so. and the baseline should be
  hand-written weights by you and then as part of enabling it in the
  prefs it should do the first fine-tune from the brush shell history.
  the option should be disabled if brush shell is not active, with a
  link/button to open olo suggestions to install it first. and it
  should be integrated into activity monitor under a new AI section
  that shows the model performance, accepted completions and so on. and
  then when enabled obv it needs to be added to the users brush shell
  as the autocompletion/suggestions method. both the training and
  inference needs to be in rust with hand written ultra optimized gpu
  and/or simd code (probably best to support both and let the user
  choose which acceleration method they want it to use) and it must be
  extremely low latency inference, like <10ms. the idea is that this is
  a tiny tiny model kinda like the palm detector model (but can be
  bigger and slower than that one of course). no python is allowed as
  usual." So:
  - Preferences ▸ Terminal: an option to complete commands with the
    model, and which acceleration it uses (SIMD or the GPU, the person's
    choice). Unavailable while brush isn't the person's shell, with a
    button that opens Olo Suggestions to set brush up first.
  - The model: tiny, in Rust, training and inference both, with
    hand-written SIMD and GPU kernels (as the palm model's). What it
    sees: the line typed so far, the bash/brush history, the current
    folder, the session's commands before it, the time of day and day
    of week, and whatever else helps (the person's running apps,
    perhaps). Inference well under 10 ms.
  - Its weights: a baseline written by hand, then a first fine-tune
    from the brush history as the option is turned on, then a quick
    fine-tune about every minute from what's been typed since, off the
    UI thread, costing nothing while nothing new was typed.
  - brush: the model is its completion and suggestion source once
    turned on (and only then).
  - Activity Monitor: a new AI section showing how the model does
    (completions offered and accepted, latency, fine-tunes and what
    each cost).
  - Ben, the next day: "also just drop the GPU option since simd is so
    much faster -- we will just use simd for it".
  *Done* (2026-11-04): `crates/complete` and the daemon `olo-complete`
  (HACKING, "brush's suggestions"). Each candidate that starts with
  what's typed gets 16 features (how often and how lately run, in this
  folder, at this hour and kind of day, after the last one or two
  commands, in this shell, how it usually ends), scored by 16 → 16 → 1
  in hand-written NEON or AVX2/SSE2 (the GPU kernel worked, through
  blade, but took 258 µs a suggestion against 1 µs, so it went, and with
  it the choice). Hand-written baseline weights; the first fine-tune as
  it's turned on (5 to 11 ms for 640 commands), then one a minute after
  new commands (about 2 ms), the best pass kept and only if it ranks
  the newest commands as well; asleep, costing nothing, otherwise.
  brush (a hook added to brush-interactive, vendored) shows the model's
  hint, waiting 8 ms at most, and says what was taken. Preferences ▸
  Terminal ▸ Command Suggestions: on and off on a worker, the first
  fine-tune's result in words, unavailable without brush as the login
  shell, a brush from before the model told apart by a mark in its
  binary, Open Olo Suggestions… for either. Activity Monitor ▸ AI:
  offered and taken, latency with a graph, the fine-tunes and how they
  did, asked only while it shows. Tested headless at 1×, 1.5× and 2×,
  light and dark (reviews clean, `ui_lint_command_suggestions` over the
  box's six states), brush through a pty in a sandbox. Its x86 run is
  the ThinkPad's (the x86 item).

- ~~**Terminal: a scroll bar, slim, there when wanted**~~ (Ben,
  2026-11-03): "show an actual scrollbar in terminal but introduce a new
  reusable scrollbar type that is like a slim/low-profile scrollbar and
  that only appears when the pointer is near it or the user has started
  scrolling already". So: a kit scroll bar of its own kind (thin, no
  arrows, drawn over the content's edge), hidden until the pointer
  comes near that edge or a scroll starts, fading out a moment after;
  draggable, clickable, agent-readable as a ScrollBar; Terminal's
  scrollback shown with it; documented in HACKING with where to use it
  rather than the BeOS bar. Ben, the same day: "the terminal scrollbars
  should actually be scrollable like i should be able to click on the
  widget in it and drag it up and down to scroll": its thumb held and
  dragged scrolls the scrollback with the pointer (a click in its track
  pages there), in every pane of a split tab. *Done* (2026-11-03): the kit's `SlimScrollBar` (any
  unit: top, shown, total, and `on_scroll`): a thin rounded thumb over
  the right edge, shown when the pointer comes within 28 points of it or
  the view scrolls (not when output is added, nor on a resize), wider
  with a faint track under the pointer, fading 0.9 s after; the thumb
  drags the scrollback with the pointer, a click in its track pages,
  hidden it takes no clicks; agents read a `ScrollBar`. Every pane of a
  tiled window has its own. Measured on release (scroll-check, A/B with
  the build before): drags and wheel notches the same (no hitches,
  slowest draw 0.4–0.6 ms either way); a flick 16 frames more, the bar's
  fade after the glide; idle, faded or with the pointer resting near it,
  0% of a core, no wakeups, no frames. Reviewed clean with it showing at
  1, 1.25, 1.5, 1.75 and 2×, light and dark.

- ~~**The command palette: typing "n" first closes it; every shortcut in
  it; files only on Ctrl**~~ (Ben, 2026-11-03): "when i'm in the command
  pallette, if i start by typing "n" it closes the pallette. fix that
  bug. also, please have every keyboard shortcut available in the
  command pallette, even stuff like move to next word or move to end of
  line. it's a great way to learn the shortcuts and to get there if you
  can't remember. also, let's have the command pallette not show
  file/folder matches by default, and you have to press ctrl to have it
  show them -- so ctrl should cycle between showing the command/shortcut
  hits (default) and the file/folder hits. and have the help text or
  placeholder text say that too". So:
  - The bug: an "n" typed first closes the palette (find what takes it:
    a shortcut, a hint, a key the palette lets through), with a test
    that types each letter first.
  - Every keyboard shortcut there is in it, the text ones too (word and
    line movement, selection, deletion: the keyboard profile's, the
    editor's and the text field's), each with its keys, chosen to do it
    in the window it was opened over.
  - Commands and shortcuts by default; Ctrl switches to files and
    folders and back, and its hint (or placeholder) says so.
  *Status* (2026-11-03): the bug was a crash, not a key: Ben's logs
  have the palette panicking four times at 10:11 in the sort of its
  results ("user-provided comparison function does not correctly
  implement a total order"), windows comparing equal to each other but
  by title to the rest, which an "n" (tying many commands and windows)
  showed. Now a key (`best_first`), with a test that panics the old way;
  and app_server's overview and the table preview sort floats with
  `total_cmp` (a NaN broke theirs the same way). Left: every shortcut
  in it, and Ctrl for files.
  *Done* (2026-11-03): the crash fixed (the status above). Every
  keyboard shortcut: app_server's and the front app's menus' were in it;
  now text editing's too (`sheet::text_keys`, one table for the sheet and
  the palette: the line's and document's start and end, a word back or
  on, each with ⇧ to select, delete to the line's start, delete a word),
  each pressed in the window it was opened over as the person's own keys
  (app_server's new `Request::PressKeys`). Found on the way: ⌘← and ⌘→
  moved a word in Olo's own text fields and StyledEdit, and ⌘⌫ deleted a
  word, since the egui kit's `mac_text_keys` went; now the kit's
  `keys::mac_command` makes them the line's again. Commands by default,
  a tap of Ctrl for files and folders and back, a key hint at the field's
  right saying so and the placeholder what it finds. Checked headless:
  ⌘→ chosen in the palette lands in StyledEdit, ⌘←/⌘→/⌥⌫ in the
  palette's field, Ctrl both ways; reviewed with no errors at 1, 1.25,
  1.5, 1.75 and 2×, light and dark (a 1.25× cut of the field's text,
  there before, fixed). Ranking a keystroke: 136–159 µs among 600 things
  with or without the 14 text keys (noise).

- ~~**Terminal: the side strip crowds its close boxes**~~ (found by the
  review, 2026-11-03): with Tabs Down the Side, `tsession review` gives
  errors at 1.5× (and the other scales to check): each tab's close box
  (as agents read it) is 4 points from the group heading above it and
  from New Tab under the last, and the first group's chip 4.3 points
  under the menu bar's titles; and near-edges (the menu bar's first
  title and the first icon, an icon and a chip's name). So: lay the
  side strip out so the review is clean at 1, 1.25, 1.5, 1.75 and 2×,
  light and dark, without its look changing more than it must. *Done* (2026-11-03): agents read a side chip and
  New Tab as what shows (the pill; the + and its name), not their whole
  rows; the tabs' icons, the chips' names and New Tab's + sit at the
  window's inset under the menu bar's first title (`SIDE_FACE`,
  `side_chip`, each chip as wide as its name so all start at one
  place); and the band beside the strip (`STRIP_SIDE_HEAD`, Terminal's
  pane heads) is as tall as a strip along the top, 32, so the first
  chip is where it is there, clear of the menu bar (28 drew a
  near-height finding against the 26-tall bar). Reviewed clean at 1,
  1.25, 1.5, 1.75 and 2×, light and dark; the kit's lint has the strip
  along the top and down the side (`ui_lint_a_tab_strip_is_clean`),
  which fails on the old layout with these findings.

- ~~**Terminal: a tab hidden from tile mode**~~ (Ben, 2026-11-03): "ability
  to hide a specific tab in tile mode in terminal by right clicking on it
  and choosing hide when tiled (or show/unhide when tiled if it is
  hidden) and then it should have a different style in the tab bar to
  indicate it's hidden and shouldn't show up in the tile modes". So: a
  tab's context menu has Hide When Tiled (Show When Tiled once hidden);
  a hidden tab stays in the tab bar, drawn differently (dimmed, its
  label and icon faded, in light, dark and night at every scale), and
  every tile mode leaves it out (its tile's room going to the rest)
  until shown again; kept per tab through a reload; agents read it as
  hidden when tiled and can toggle it. *Done* (2026-11-03): a tab's menu has Hide When
  Tiled (Show When Tiled once hidden), and the Tabs menu the same for
  the tab showing. A hidden tab stays in the strip with its icon and
  title faded (the kit's `StripTab::faded`, at `STRIP_FADED`, along the
  top and down the side), every tiling leaves it out (`tile_rows_of`,
  tested) and its room goes to the rest; picked in the strip it shows
  on its own until a tiled tab is picked. A lone tile now has the whole
  window, without the empty head band it had. Kept per tab in
  windows.json, through restarts and shells restored after a reboot.
  Agents read "hidden when tiled" in the tab's name and toggle it from
  the Tabs menu. Reviewed clean along the top at 1, 1.25, 1.5, 1.75
  and 2×, light and dark; the side strip's own crowding findings came
  from before (the next item).

- ~~**Terminal: tile mode's group headers look unselected while every
  group shows**~~ (Ben, 2026-11-03, a bug): "in terminal tile mode when all
  tab groups are being shown (the default) the group headers aren't
  getting the "selected" style, they look unselected. they should be all
  selected by default and then i can deselect/reselect as i want". So:
  with every group shown (tile mode's default, nothing chosen yet), each
  group's header is drawn selected, as a chosen one is; clicking one
  deselects it (its group leaves the tiles) and clicking again selects
  it, from there as now; agents read each header's selected state as it
  is drawn; checked in light, dark and night at every scale. *Done* (2026-11-03): a header is drawn
  selected while its group shows (`tile_group_shown`: every group when
  nothing is chosen yet), and a click takes that one group out of the
  tiles or brings it back (`tile_groups_toggled`, which never leaves
  none and goes back to "every group" once all are chosen; a test of
  both); agents read the same selected state. The Group window's
  suggested name now comes selected, so typing replaces it. Checked
  headless with two groups, and reviewed clean at 1, 1.25, 1.5, 1.75
  and 2×, light and dark.

- ~~**Activity Monitor: an Inputs section, and the Displays panel
  polished**~~ (Ben, 2026-11-03): "add an Inputs section in activity
  monitor, move the pointer + touchpad in there, add a bunch more
  stats/metrics about them with graphs and such. also polish up with
  stats/metrics/graphs the Displays panel". *Done* (2026-11-03): Inputs
  has the pointer's and the touchpad's cards, moved from Displays, each
  with a graph over the span chosen (reports a second; the touchpad's
  latency typically and at the 99th), and What You Did: clicks,
  scrolling steps, gestures begun and keys pressed in the last minute
  (app_server's `input_counts`, how many and never which key), with a
  graph of each; its sidebar badge counts the pointer's gaps. Each
  display's card has graphs of its frames a second and drops, and of
  drawing's 95th percentile; what's on planes was there already
  (Scan-out). Reviewed clean at 1, 1.25, 1.5, 1.75 and 2×, light and
  dark. Not yet: the palm filter's catches (they'd need app_server to
  run the model on the pad's slots: the touchpad latency item).

- ~~**Sound ▸ Voice Processing: try it on your own voice**~~ (Ben,
  2026-11-02 and 2026-11-03): record a short sample, see "in real time
  some kind of audio frequency graph/visualization and what it's
  doing", the processing delay, play it back with and without, and
  "hearing how it sounds on the different settings … the added latency
  … and how it was accelerated (simd type or gpu etc)". *Done*
  (2026-11-03): Try It… in the Voice Processing box records six seconds
  from the microphone, tries each setting worth comparing on it (as
  set, each other noise level, the level the other way, the other
  mode), each listed as it's done with the delay it adds, its time a
  10 ms frame (median and worst), what noise suppression and levelling
  did, and what ran it (NEON here; AVX2/AVX-512/SSE on x86; no GPU:
  a frame is shorter than a GPU round trip); plays the original or the
  chosen setting's sound, switching at the same place, with a live
  spectrum of both (the kit's new `Spectrum`); "Use This Setting" saves
  it. Recording and playing in olo-system (`recording`), tested end to
  end in a PipeWire sandbox. Found on the way: `RadioButton` sat a
  device pixel off a button or label beside it at 1.25× and 1.75×
  (`RadioButton::height`), and every group box's inset was measured
  unrounded by the review (false overflows at those scales). Measured:
  292 µs a frame while playing (1.8 ms before the live part was a view
  of its own), 252 µs recording, nothing idle.

- ~~**A palm resting low on the pad, a shirt against it, and the trainer
  asking one thing at a time**~~ (Ben, 2026-11-03): "the palm tracking
  model and training works pretty well but it often misses detecting my
  palm when it's resting near the bottom of the trackpad. can we enhance
  the model for supporting that and add a training step to collect
  samples for it?"; "sometimes it's also resting against my shirt and
  that interfers can you detect that too"; "make this one that you're
  improving from my data the new default/untrained model too"; "clean
  up the training ux/steps a bit a few of them are confusing right now
  where it tries to ask me to do like two things at once … when the
  step is finished it's not really clear just a small icon it needs to
  be obvious and also some of the spacing/alignment is off". *Done*:
  - Why low palms were missed: his runs had no touch in the pad's lowest
    fifth, and a contact was judged in its first 250 ms from when the pad
    first saw it, hovering included; palms settling hover (15 of 35 in
    run 2 still hovering at 40 ms, some for all 250; no finger past
    56 ms), so a hand coming down slowly was never judged touching. Now
    from the first touch, in the tracker and the Lua filter alike.
  - The model: a hand-written unit for a heel resting low; inputs held
    within what it was trained on; training held near its start (on its
    own it drifted until a still fingertip was a palm). Olo's own model
    is that trained on Ben's runs, with clicks and low touches made from
    them, and runs untrained (on by default; the Touch tab and Activity
    say which model runs).
  - The trainer: twelve exercises, one thing each (the edges, pointing
    and scrolling apart; click, thumb click at the bottom, a hand resting
    low while a finger points above it, the shirt against the bottom edge,
    which can be skipped); a line under the title saying what to do now
    (rest, hold, point, lift); a green banner when a step is done (the
    kit's `Banner`), in a box the status line already filled so nothing
    moves; the summary's outcome in one too; the intro's list under the
    panels. Its runs are kept as `Run { from_touch }` for the next
    default.
  - Found on the way: a full-screen window kept its size when its
    screen's scale changed (app_server's `fixup_positions` now tells it
    the new one), and `KeyHint`'s cap sat a device pixel off its label
    at 1.25×, 1.75× and 2× (now one baseline): the trainer reviews clean
    at 1, 1.25, 1.5, 1.75 and 2×, light and dark.
  Measured: the filter, 3 contacts a frame, 33.9 → 35.3 µs with the
  same model (20.3 vs 18.7 µs off, an unchanged path: noise), 33.2–34.0
  µs with Olo's own. Left (Open, "The best baseline palm model"): Ben
  training on the new build, and the default made again from that run.

- ~~**Olo Passwords in every Chrome profile, exactly once**~~ (Ben,
  2026-11-03): "only my first olo chrome profile has the olo passwords
  extension (and it has it twice which is a bug too). i need all the
  chrome profiles to have it exactly once." *Done* (2026-11-03): the
  listing in Chrome's install (`extensions/<id>.json`) gives every
  profile the extension, those made later too; the other profiles
  hadn't got it yet only because that Chrome had been running since
  before they were made (Ben: "i just had to restart chrome is all").
  The second copy came from a test: an olo-chrome with a sandbox home
  running Ben's Chrome (through a `current` link) listed a copy signed
  with the sandbox's key in Ben's install. Now only the home whose
  `olo/chrome` holds the install lists the extension there, and it
  takes out other copies' listings in every version folder (Chrome
  uninstalls them as it starts); any other olo-chrome registers its
  host for the install's extension and lists nothing.
  (`extension::set_up`, test `one_copy_for_every_profile`.)

- ~~**Voice Processing's device named for the one it processes**~~ (Ben,
  2026-11-03): "when the voice processing is enabled if there is a new
  sound device added that gets the processing that device should be
  named appropriately, e.g. "AirPods (Voice Processing)" not just
  "AirPods" b/c there's no way to tell it apart". Now olo-voice's
  source is always "Microphone (Voice Processing)" (apps/voice
  engine.rs, `NODE_DESCRIPTION`), whatever it listens to, and a device
  that arrives and is processed shows under its own name. So: every
  processed source is named after the microphone it processes, with
  " (Voice Processing)" after it ("AirPods (Voice Processing)",
  "MacBook Pro Microphone (Voice Processing)"), renamed as the chosen
  microphone changes or a new one arrives and is taken up; the same
  name everywhere a device is listed (Preferences ▸ Sound, Deskbar's
  sound menu, Activity Monitor, Chrome's and other apps' pickers
  through PipeWire), so the raw device and the processed one are
  always told apart; tried with `tools/dev/pw-sandbox` and a device
  plugged in while it runs.
  *Done* 2026-11-03: olo-voice's source is named after the microphone
  it listens to ("Test Microphone (Voice Processing)", then "AirPods
  (Voice Processing)" as AirPods arrived and were chosen), renamed live
  (`pw_stream_update_properties`): PipeWire's node and its PulseAudio
  server's source both carry the new name, so Preferences, Deskbar and
  other programs' pickers list it beside the raw "AirPods". Tried in
  `tools/dev/pw-sandbox` with a microphone made while it ran.

- ~~**A tile too small for its window's least size, off its part**~~
  (found 2026-11-03 by `wm-fuzz --tiles`, seeds 5 and 9, the build
  before the drag-round fixes too): `tsession start fz
  2560x1600,1920x1080 --session --shell`, `tools/dev/wm-fuzz fz --seed 9
  --steps 46 --tiles`: step 44, Styled Edit on the smaller display
  tiled to the centre third (256 px) and on through tile-left and
  tile-top-right, ends 290 wide (its least) at x 268 rather than its
  part at 264: it grew 34 px but moved right, not inwards as
  `give_way` has it (seed 5, step 193: 290 wide at 1766, its part 1800
  and 256 wide). Find which of the three commands leaves it there, have
  a window too big for its tile give way the same every time (and
  wm-fuzz's check take a window at its least size, given way, as in its
  tile), and keep both seeds passing 200 steps with `--tiles`.
  *Done* 2026-11-03: app_server was right; the check wasn't. A window
  whose app won't be as small as its tile gives way as `give_way` says
  (one side held, the tile's own with its half gap: x 268, not the bare
  part's 264; moved in from the area's edge it would cross: 1766), and
  wm-fuzz's check now gives way the same, from the window list's
  `least`. The runs that found it had also restored the person's
  Terminal windows into the test (four Terminals for one opened), which
  made new windows miss the keyboard and groups' frames differ: test
  sessions now keep their apps' state and caches in their own directory
  (tsession), and seeds 5 and 9 pass 200 steps with `--tiles`.

- ~~**Backup: Wi-Fi networks too**~~ (the rest of Ben's backup audit,
  2026-11-03): saved Wi-Fi networks and their passwords live in
  NetworkManager, not in Olo's files.
  *Done* 2026-11-03: a sealed backup takes them along
  (`olo_system::network::saved_wifi`: `GetSettings`, `GetSecrets`;
  enterprise ones stay behind and are named), and an import saves those
  the computer hasn't (`add_wifi`), keeping its own. Tested by unit
  tests and end to end over D-Bus against `nm-fake`, a stand-in
  NetworkManager on private buses (`OLO_NM_BUS`): exported from one
  with three networks, imported into another with one of its own, the
  WPA one's password there. Left: the ThinkPad itself (open above).

- ~~**Backup: everything, and onto another machine**~~ (Ben, 2026-11-03,
  soon: he sets up his ThinkPad by exporting from this Mac and
  importing there): "do an audit of the import/export backup feature
  and make sure everything from all the preferences / relevant olo apps
  is covered and included -- including passwords -- and that none of
  it assumes it's imported on the exact same machine (e.g. the same
  macbook and user)". So: list what every Preferences pane and Olo app
  keeps (settings, layouts, Passwords' vault, accounts, Integrations,
  keyboard and touchpad settings, Terminal's profiles, Deskbar,
  Tracker, the palm model, …) against what the export takes, and add
  what's missing; nothing in it tied to this machine (paths under this
  user's home, display and input device names, Apple-only hardware,
  aarch64), what can't carry over (a device's own pairing, a key sealed
  to this machine's TPM or keyring) left out or put in again on import,
  and said so; tested by exporting from one sandbox and importing into
  another with a different user, home and displays (and an x86 one when
  the ThinkPad is here), every pane and app then checked.
  *Done* 2026-11-03, the audit: the backup took both of Olo's
  directories but none of the keyring's secrets, so on another computer
  Passwords and the Token Pool couldn't be opened (their keys are in the
  login keyring), nor GitHub or Telegram signed in; it took Agent
  Screens' homes (1.7 GB of the 1.7 GB here); paths under the home
  folder stayed the old home's; and app_server's settings for displays
  known by their port ("eDP-1": the Mac's panel) would have gone to the
  ThinkPad's. Now: every secret Olo keeps is in one list
  (`olo_system::secrets::KEPT`), and a backup sealed with a password
  (Argon2id, XChaCha20-Poly1305) takes them along; import puts them in
  the new keyring, moves home paths to the new home, leaves out
  port-keyed displays, keeps Telegram's sign-in on its own computer,
  pauses the pool's accounts (two computers renewing one sign-in sign
  each other out) and says what didn't come along. What was there is
  saved first, sealed with a key in the keyring, secrets and all. Left
  as they are (fine elsewhere): the touch filter (Apple's pads only),
  keyboard profiles (the person's; the Mac one noted). Tested: unit
  tests (another computer and home, a wrong password, a changed byte,
  no keyring, undo), end to end with real gnome-keyrings on private
  buses (a Passwords vault made at home A opens at home B), and the
  panel at every scale, light and dark. Left: Wi-Fi networks, and the
  ThinkPad itself (above).

- ~~**Drag rounds: the last looks out of step in deeper trees**~~ (found
  2026-11-03 by auto-fuzz's mid-drag check): `tsession start af
  3024x1890 --scale 1.75 --session --shell`, `tools/dev/auto-fuzz af
  --seed 11 --steps 150`: 9 drags of the 150 steps have one to four
  looks (of ~200) with tiles out of step, mostly at the very start of a
  fast drag or as it's let go, in trees of four or more tiles with
  groups (e.g. step 146: two tiles 227 px ahead of the two across the
  edge for one look). Found so far (step 85, traced): the drag stops a
  tile at its least size as `tiling.rs` counts it (`min_frame`, the
  group's: 180 px of content), then the tree laid out as it's let go
  holds it 16 px bigger and its row's other split 40 px over (a group's
  least width the drag never touched), so the release snaps the tiles
  there; it's in the last round, but a look can catch a tile at its new
  frame while the dragged one is still at its old (a group's tab behind,
  set to the group's frame at once?). So: the drag's limits and the
  tree's least sizes made the same (one function for both), a drag
  never leaving a tile under its least, and the look found; the seed
  passing at 1×, 1.5×, 1.75× and 2×.
  *Done* 2026-11-03: four things, found with a trace of each round
  (`RUST_LOG=info,olo::rounds=debug`). A neighbour following a drag
  wasn't told it was being resized (xdg's `Resizing`, the dragged
  window's alone), so the kit's apps made a new swapchain at every
  step, four new buffers each; a round didn't wait for a new buffer's
  check (`dmabuf_check`), so it showed without that window, whose frames
  then queued behind the check and landed a round or two late; and the
  check worker freed every buffer let go of at once, a check waiting
  behind it 45 ms (a round's whole `LIMIT`). Now neighbours are
  `Resizing` while the drag lasts, a round waits for its buffers' checks
  as for their fences (`Gate::in_round`), and the worker frees two at a
  time. And the drag's limits are the tree's: the window dragged keeps
  its own least size (its group's), an auto-tiled drag goes only as far
  as its tree lays the tiles out where it has them (`auto_keeps`,
  `lays_out_as`), an edge moved a pixel or two is kept (`take_ratios`'
  `still`: it was 0.3% of the area, 5 px here), and a tile that only
  moves as it's let go moves with the last round (`round_move`). Seed 11,
  150 steps: 14 drags out of step before; none after, at 1×, 1.5×,
  1.75× and 2× (release). New buffers in a run: 8,324 before the
  `Resizing` change, 2,188 after.

- ~~**⌘` moves a window from another workspace to this one**~~ (found by
  wm-fuzz, 2026-11-03, seed 5 step 124 on a `2560x1600,1920x1080`
  session; the build from before the Focus fixes fails it the same way):
  after a run of tiling, arranging, a display's scale changed and an
  Undo, ⌘` ×3 left window 6, an app's window on workspace 4, on
  workspace 1. Twitcher shows a window from another workspace while it
  is chosen (`twitch_show`) and must put it back when another is
  chosen or Twitcher closes on another; find the path that keeps it
  (an Undo of an arrangement in between, or the display's scale), fix
  it, and keep wm-fuzz's seed passing.
  *Done* 2026-11-03: not Twitcher's doing. A tab torn out of its group,
  then an Undo of an arrangement remembered before the displays were
  rearranged, put the window at a frame on no display at all (only its
  tab on one, then none as the displays changed again); parked with its
  workspace, it was parked on the main display (`window_output`'s
  fallback), and when ⌘` showed that workspace it was mapped at its
  frame, on the other display, which didn't switch back with it. A
  window now comes back on the display it was parked on (`unpark`:
  brought into its area if its frame says another). wm-fuzz seed 5
  passes its 200 steps (seeds 9 and 13 too).

- ~~**Auto-tiling: an in-place reload puts the windows in each other's
  tiles**~~ (found 2026-11-03, testing the drag rounds): three tiles
  `[Terminal | [StyledEdit / Tracker]]`, `tsession reload`: the tree
  comes back right (`tiles kept from before the restart …
  [·#3 | [·#1 / ·#2]]`, Terminal put back with slot 3), but each window
  is claimed as a new tile at its first commit (`a new window into its
  tile`, `auto_claim`, before its saved place is known) instead of
  taking the tile kept for it (`auto_returned` never logs "back in its
  tile"), and 20 s later the kept tiles are given up and the windows
  laid out afresh in another order. Find why the returning windows
  aren't known as returning when they're first claimed (their placement
  comes after the first commit?), fix it, and add the case to
  upgrade-check (a session in Auto-Tiling: every window in its own tile
  after a reload, at 1×, 1.5× and 2×).
  *Done* 2026-11-03: Olo's GPUI committed a new window before naming it
  (its first commit goes at once, before the renderer starts, and the
  app id and title came after), so at that commit app_server couldn't
  tell a returning window from a new one and gave each a new tile. The
  app id and title now go with the first commit (`WindowParams::app_id`),
  and each window takes back its own tile ("back in its tile"), at 1×,
  1.5× and 2×, after two reloads in a row. Broken since GPUI was
  vendored (2026-10-31). `upgrade-check --auto-tiled` arranges the case
  itself: the old build fails it, the new passes (reload and re-execute).

- ~~**Auto-tiling: resizing a window is choppy and windows overlap until
  let go**~~ (Ben, 2026-11-03, a bug): "when in autotile mode and
  resizing a window it is super choppy while resizing and the windows
  can overlap and it doesn't all snap into place until i let go. it
  should be smooth the whole time and the windows should react properly
  as i resize them". So: while a tiled window's edge is dragged, its
  neighbours give way frame by frame (the layout worked out at each
  move, not at the release), every window's new size sent at once and
  shown together when the clients have drawn it (no window drawn over
  another, no gap), at the display's rate; measured with `timings` and
  the bench (a live resize in auto-tiling at 120 Hz, no dropped frames).
  *Done* 2026-11-03: each step of an edge drag is a round
  (app_server `shell/rounds.rs`): the dragged window and the tiles
  following it are asked their sizes at once, and what each draws waits
  (a commit blocker) till all have drawn it at the size asked and their
  GPU fences are done, then all show in the same frame; the next round
  goes as one shows, and letting go is the last. Measured with a sampler
  every 3 ms through drags (release builds, 120 Hz): before, tiles a gap
  apart drifted by up to 295 px in 60% of samples of a fast drag (632 px
  in auto-fuzz); after, none in any sample, one layout a frame (a round
  7 ms, at 60 Hz 15 ms). Found on the way and fixed: auto-tiling's
  layout asked the following tiles back to their old tiles every turn of
  the drag; Olo's GPUI dropped a configure that came while a resize was
  throttled, never acknowledging it (now done at the next frame); a tile
  at its least size beside the area's edge was laid out 4 px wider as
  the drag ended (`Inset`, with a test). auto-fuzz now fails a drag on
  any look with tiles out of step (`MidDrag`); it failed every drag of
  the old build and passes 90 of 90 drags of three tiles at 1×; at 1.75×
  with groups and deeper trees a few single looks stay out of step (the
  next item). The last round has what auto-tiling lays out as the drag
  ends too (`round_collecting`).

- ~~**Focus mode: a link opened in a browser on the same desktop doesn't
  bring its window forward**~~ (Ben, 2026-11-03, a bug): "when in focus
  mode if i click on e.g. a url and my browser that it should open in is
  on the same desktop with a window already open then it correctly opens
  it in a tab but it doesn't change focus to the window. weirdly, it does
  work properly and change focus if the browser is on another display
  (and maybe another workspace i didn't check). when you fix this also
  make sure we have extensive tests and fuzz tests around focus mode
  (and the other window arrangement modes) because we've hit quite a few
  bugs with their different behaviors and want to make sure they won't
  keep coming back". So: find why an activation (xdg-activation with
  the opener's token) on the same display and workspace is refused or
  lost in focus mode, fix it; then tests for every arrangement mode
  (standard, focus, auto-tiling, …) of activation, focus and stacking
  on the same and other workspaces and displays, and a fuzz test that
  drives random sequences (open, close, activate with and without
  tokens, move, switch workspaces and modes) against the modes' rules.
  *Done* 2026-11-03: two causes. olo-chrome handed a running Chrome the
  token in its environment, the opener's (Terminal's, long spent), as
  `launch_activation_token` returns none when one is there: it now asks
  app_server for a fresh one first (`issue_activation_token`), and
  app_server takes a token whose input is its maker's, the app with the
  keyboard, though another app's window comes forward. A Firefox window
  opening in Focus also aborted a debug app_server (`new_fractional_scale`
  took two surfaces' locks at once; `surface_locks.rs` now catches bare
  calls too). Tests: `tools/dev/mode-fuzz`, random steps across Standard,
  Auto-Tiling and Focus on two displays checked against each mode's
  rules, and `--matrix [--chrome]`, every mode × a window behind, hidden,
  on the other display and another workspace × Deskbar's activation, a
  relaunch, and a relaunch with the opener's token (72 cases, Chrome's
  included; with olo-chrome's old token every Chrome opener case fails).
  What they found, fixed: leaving Focus left a group zoomed (whichever
  of it was in front led the rest back; now the whole group goes back),
  and reset another display's Focus state; a new window Focus zoomed was
  unzoomed for a moment before it was placed (a flicker); new windows
  opened under the pointer while apps are told the active display is the
  keyboard's (Tracker opened a second window for a folder it showed);
  and a window whose group came apart around it jumped back to an old
  frame after a workspace switch. `WindowInfo::stack` gives agents the
  stacking order.

- ~~**Agent Screens: copy and paste between the person's display and a
  screen they're controlling**~~ (Ben, 2026-11-03): "when i am controlling
  an agent screen i should be able to copy/paste from/to it". So while
  the person controls a screen, what they copy on it reaches their own
  clipboard and what they copied on their own display pastes on it
  (text, pictures, files as the clipboard has them), both ways, only
  while they're in control (an agent's screen never reads the person's
  clipboard on its own).
  *Done* 2026-11-03: the screen's clipboard and the person's joined while
  they control it (`clip.rs`, on the kit's new `clipboard::watch`,
  `hold_on` and `read_all`), every form both ways, none echoed back;
  theirs taken off the screen when they hand it back. Tried on a
  sandboxed screen with `pbcopy`/`pbpaste` on each side: person to
  screen, screen to person, then handed back: the screen's clipboard
  empty, and a new copy of the person's not reaching it.

- ~~**Tracker: folders by name, files newest first, a folder's own order
  kept**~~ (Ben, 2026-11-03): "default sort order of folders in tracker
  should be lexographically and files should be by last modified (most
  recent first) -- but if the user changes the sorting remember it for
  that specific folder". So, in every view: the folders in a window
  first, by name (natural order, as Tracker's names sort), then the
  files, most recently modified first; a column clicked (or a sort chosen
  from the menu) in a folder is kept for that folder (with its window's
  other state, as BeOS kept it in the folder's attributes) and used each
  time it opens, every other folder keeping the default.
  *Done* 2026-11-03: Olo's order in every view (folders by name, then
  files newest first, the header marked Modified ▼); a column clicked or
  a Sort By chosen is kept for that folder (`sorts.json` in Tracker's
  state) and used whenever it opens; Sort By ▸ Folders by Name, Files by
  Date goes back. Tried in a headless session: the default, Name chosen,
  the window closed and the folder opened again by name, then back.

- ~~**Agent Screens: an agent waits while the person has the screen**~~
  (Ben, 2026-11-03): "if the user is controlling the agent screen all
  kinds of weird stuff happens when the agent tries to do things. when
  the user is controlling and the agent tries to do something it should
  just tell the agent that the user is controlling and that they should
  wait for the user to finish before they do anything else." So while
  the person is controlling a screen (Agent Screens' Take Control), every
  `olo-screen` action that would act on it (click, type, keys, drag,
  scroll, start an app, a menu command) is refused at once with a clear
  answer: the person is controlling this screen, wait until they hand
  it back (and how to wait for that), doing nothing to the screen; what
  only reads it (a look, the tree) still answers. The skill says so too.
  *Done* 2026-11-03: while the person controls a screen the viewer marks
  it (`NAME.control`, its pid) and `olo-screen` refuses every action on
  it, doing nothing, with that message (kept in the screen's history);
  reads still answer; `olo-screen wait NAME --free` waits for the
  hand-back. Tried on a sandboxed screen: refused while controlled,
  `wait --free` returned as ⌘K handed it back, the next click went
  through. The skill says what to do.

- ~~**The olo-screens skill teaches agents to use the passwords shared
  with their screen**~~ (Ben, 2026-11-03, a priority): "the olo-screens
  skill isn't properly teaching agents how to access the passwords
  shared on their screen. explain to them how they should use the olo
  passwords integration in olo chrome by right clicking in the field and
  choosing olo -> passwords in the menu, or by opening the passwords app
  in the gui. also teach them how they can use the op cli command to get
  the creds onto the clipboard and paste in themselves that way (and
  explain that op is a shim for 1password that goes through passwords
  app)." So the skill says, read from the code that does each:
  - What's shared with a screen (chosen on the person's own display, in
    Agent Screens' Passwords tab), and that only that is there.
  - Filling a login in Olo's Chrome: right-click the field, Olo ▸
    Passwords, choose the item.
  - Opening Passwords on the screen and copying from it.
  - `op`: Olo's 1Password-compatible command, a shim that goes through
    Passwords (and so sees only what's shared with the screen): getting
    a username, password or one-time code onto the screen's clipboard,
    then pasting it where it goes.
  - Ben, the same day: "the agents also still seem to want to open the
    system chrome not the olo chrome variant. make sure we're super
    clear about using the olo chrome and not any other chrome that may
    be installed". So the skill names Olo's Chrome (`olo-chrome`, the
    one Deskbar starts) as the browser to use, says plainly never to
    start another Chrome or Chromium installed on the machine (and
    why: no Olo integration, no shared passwords, not the screen's
    profile), and shows the exact way to start it on a screen.
  - Ben, later the same day: "what's actually happening is agents are
    getting a fresh chrome profile in an agent screen and that's asking
    them to accept tos and showing first run window. when using
    olo-chrome if there is no profile set then we should have it use one
    that has the tos etc. accepted so that it's ready to go for the
    agents". So `olo-chrome` started without a profile of its own (an
    agent screen's first run) starts in one that's ready: first run,
    terms and welcome pages already behind it (Chrome's first-run
    sentinel, its master preferences / initial prefs, the policies that
    skip them), checked by starting it on a fresh screen and seeing a
    browser window, not a first-run one.
  *Done* 2026-11-03: the skill's "A web browser: Olo's Chrome, only"
  (never another Chrome, never from the agent's own shell) and
  "Passwords on your screen" (what's shared; never `type` a secret,
  as the history keeps it; Chrome's Olo ▸ Autofill / Passwords menu;
  Passwords' Copy; `op` with the screen's environment and `pbcopy`,
  then ⌘V). And the cause of the terms window: a new screen's Chrome
  profile had never run, so Chrome showed Google's terms; on a screen
  olo-chrome now writes Chrome's `First Run` mark, sets the New Tab page
  at startup and skips the default-browser check (tried on an empty
  profile in a headless session: straight to a browser window, the
  second start too). The installer's first start sets the profile up
  too now (Olo's defaults and Passwords' extension were missed).

- ~~**StyledEdit: icon toolbars, as Gobe Productive's**~~ (Ben, 2026-10-31,
  for later): "stylededit should have icon toolbars (side and top)
  reminiscent of gobe productive from beos. build out the kit to support
  the ui components etc in a reusable way and make sure everything is
  pixel perfect, looks great, gets properly themed etc". So: a kit
  toolbar (icon buttons, toggles, pop-up pickers for fonts, sizes and
  colours, separators, groups; along the top and down the side, as
  Gobe's tool and format bars), its icons drawn to ICONS.md at every
  scale, themed in light, dark and night, keyboard- and agent-reachable,
  documented in HACKING; StyledEdit's commands on it.
  *Done* 2026-11-03: the kit's `widgets::ToolBar` (commands, toggles,
  font, size and colour pickers, separators, wrapping to rows and a »
  menu, top and side, every tool its menu item's state; cached, drawn
  again only as what it shows changes), 25 new icons in Classic and
  every theme, StyledEdit's top and side bars (Document ▸ Show
  Toolbars), reviewed clean at 1, 1.25, 1.5, 1.75 and 2× in light and
  dark.

- ~~**Passwords for Agent Screens**~~ (Ben, 2026-10-31): "in the Agent
  Screens app i can choose items from passwords that i want to make
  available by default to all new and existing agent screen sessions and
  these items should be available to the agent by just opening the
  passwords app in the agent screen and they'll see it unlocked
  automatically and scoped to just the items i've chosen to make
  available. on any given agent screen session i should be able to
  further customize it in a new "Passwords" tab where i can optionally
  remove any grants already provided or add more. and for both places
  when defining grants it needs to be powerful: i should be able to
  search and filter to add/remove multiple matching items in bulk". So:
  default grants in Agent Screens (settings), per-screen grants in a
  Passwords tab of each screen (taking away defaults or adding more);
  Passwords opened on an agent screen opens unlocked, showing (and
  filling, `op`, Chrome) only the granted items; both editors with the
  kit's search language and filters, selecting many matches to grant or
  revoke at once.
  *Done* (2026-11-03, HACKING "Passwords for agent screens"): the
  person chose a copy for each screen with a key it uses on its own
  (over sharing only while their Passwords is unlocked). Every screen's
  items in Screen ▸ Passwords for Every Screen…, each screen's more or
  fewer in its Passwords tab, searched, filtered and changed many at a
  time; kept sealed in the person's store, changed only from their own
  display; a screen's Passwords opens its copy unlocked, read only,
  following changes. Checked headless end to end (two sandboxed
  sessions, a fixture store), and that an agent on a screen is refused.

- ~~**Tooltips for text that doesn't show in full**~~ (*done in the kit*
  2026-11-02, HACKING "Text cut short": every label the kit cuts with
  "…" or that runs past its clip, Tracker's icon names, Telegram's chat
  list, Passwords' titles; ⌘ shows it at once; and app_server's window
  tabs, `tabtip`; a text field whose text runs past it, whole in a tip
  when it hasn't the keyboard, 2026-11-02, with the field scrolling
  sideways to its caret while edited, which it never did; Twitcher's
  label whole on a plate under the panel and the overview's chosen
  title whole, 2026-11-03, `tabtip::plate`; Telegram's attachment chips
  and the lines its bubbles cut (reply quotes, forwards, files, polls),
  2026-11-03; and GPUI's own text elements (truncated, clamped short of
  their rows, a line past its clip: `gpui::CutText`), 2026-11-03,
  finishing it; text under other text or a control is a layout bug the
  lint finds, `text-over-text` and `overlap`, not a tip's; agents read
  cut text whole since
  2026-11-02, `agent::whole_name`) (Ben, 2026-10-31):
  "when hovering over anything in olo or any olo app where the text has
  been truncated or occluded or otherwise is not being displayed in full
  then there should be a tooltip that appears showing the full text".
  Everywhere: kit widgets (labels, list cells cut with "…", tab and strip
  titles, menu items, text fields scrolled sideways, buttons, Deskbar's
  rows and tray, the chat list's previews), app_server's window tabs, and
  text painted by hand; a part hidden under something else counts too.
  Best done once in the kit (where text is cut to fit, remember the full
  text and its rect; show it in the kit's tip, `olo_kit::tip`, after the
  usual hover pause) so every app gets it, and agents read the full text
  as the element's name. Then (Ben, same day): "if the user presses the
  super key while hovering over it then it should show the tooltip
  immediately without a delay" (Super while the pointer is on cut-short
  text shows its tip at once; app_server's own text, window tabs, too).

- ~~**Viewer: HEVC plays**~~ (left from the Viewer's video work): HEVC
  (mp4, Matroska) video shows only its facts; decode it natively as
  H.264, AV1 and VP9 are. *Status* (2026-11-02): crates.io has
  pure-Rust HEVC decoders now (`rusty_h265` 0.6, from the ITU-T spec,
  with a SIMD companion crate; `rust_h265` 0.4, Main, Main 10 and range
  extensions), so it no longer needs libde265: pick one by correctness
  on real phone footage (10-bit HDR included) and speed against VP9's
  ~120 pictures/s at 1080p, then wire it as VP9 is (an `hevc` module in
  crates/images giving `av1::Frame`s, `Codec::Hevc`, `hvc1`/`hev1` and
  `V_MPEGH/ISO/HEVC`, the poster, `playable`).
  *Done* (2026-11-03): rust_h265 0.4 (MIT/Apache), as `olo_images::hevc`,
  chosen over rusty_h265 0.6 by measuring: both bit-exact on 35 JCT-VC
  conformance streams (8 and 10 bit, tiles, WPP, dependent slices,
  unequal bit depths) and identical on two real 1080p clips; rust_h265
  faster alone (104 and 87 pictures/s against 93 and 68) and threaded
  (250 and 341 at four). HEVC plays from mp4 (`hvc1` and `hev1`: the
  mp4 crate skips `hvcC`, so `media::mp4_hevc` walks to it and `colr`)
  and Matroska (`V_MPEGH/ISO/HEVC`), with posters, seeks and its colour
  (the container's matrix and range, else BT.709 from 720 lines up):
  160–240 pictures/s at 1080p through the player (VP9 124–132, AV1
  111–125). `hevc_plays` checks all of it; phone footage (10-bit HDR)
  still to try: no HDR tone mapping yet, as for AV1. *HDR tone mapping
  done* (2026-11-05, HACKING's "HDR and wide-gamut video"): HDR10 and
  HLG (and BT.2020 or P3 SDR) from AV1, HEVC and VP9, the colour from the
  stream or its container, shown for an sRGB screen (tested on
  synthetic HDR10 pictures and sample entries; real phone footage still
  to try).

- ~~**Logs: a drag in its second list is uneven at 1.5× now and then**~~
  (found by `tools/dev/bench`, 2026-11-03): "frames moved 0.73–1.27× as
  far as the fingers", once in four runs. *Done* (2026-11-03): with the
  cores busy it was every run (16 of 16, as bad as 0.53–1.53×, 23
  frames more than 25% off), the frames themselves regular: app_server's
  pacer placed each finger report by when it read it, so a report read
  a few ms late (its loop busy, or the test's driver late to send) was a
  short step then a long one. Reports are placed by their own time now
  (the pad's kernel time; a synthetic one's `at_us`, which the bench and
  scroll-check send): 1 run of 16 uneven, 2 frames off. Quiet runs and
  the glide as before (bench: Logs, Tracker, StyledEdit at 1.5 and 2×,
  every drag 0.94–1.01, every glide +0.00).
- ~~**Palm size and the palm model, said plainly**~~ (Ben, 2026-11-02): "is
  the palm size setting etc still applicable when using the model?
  should they be disabled and unused if the model is enabled, like
  either/or? it's confusing right now so either make it clear that they
  are not mutually exclusive or make it so one or the other is chosen".
  They work together (a contact is a palm if it's as large as the size
  or the model says so: the size is a floor the model can't take away),
  so the Touch tab says so: the model first, with which one is in use,
  then "Always a palm when as large as", with a note on how they mix.
  *Done* (2026-11-02, a2669801): the Touch tab's slider says "Always a
  palm when as large as", under the model, with "Whatever the model
  says: it adds palms, never takes these away" (or "Without a palm
  model, size alone tells a palm").
- ~~**UX linters for every Olo app, run with the build**~~ (Ben,
  2026-11-02): "also when you do the fixes for the trainer ux can you see
  if you can add some ux linters that check spacing and layout and
  window fit and everything and that run as part of the build because
  most of these silly positioning/spacing things should be flaggable
  automatically we keep hitting them and it just shouldn't be such a big
  problem to have consistent, polished ux"; "and when i say "add some ux
  linters" i mean make some and integrate them of course"; "and the
  linters should apply to all olo native apps / prefs etc and fix any
  issues you find when you run it". Today `cargo ui-lint` (olo-ui and
  Preferences only, run by hand and by install-session) enforces the
  review's pixel errors plus overlap / outside / text-over-text; the
  review's spacing findings (near-edge, near-level, off-centre,
  uneven-rows, near-height) are only advice. So:
  - The checks in the shared review (`layout_review`), so a live app's
    review (`tsession review`, `olo-screen review`) runs them too, plus
    new ones: content against the window's edges (the 12-point inset),
    controls crowding each other, controls almost lined up (left and
    right edges), uneven gaps in a row of buttons, a fixed window or
    dialog that doesn't fit its content (cut, or far taller).
  - Every Olo app with a window linted live: a runner that opens each in
    a headless session at 1, 1.5 and 2×, light and dark, and fails on
    errors; the palm trainer's screens as `ui_lint` tests.
  - Run with the build: on every commit that touches UI code (a hook),
    and before install; fix everything they find.
  Also (Ben, 2026-11-03, with a screenshot of the Touch tab): "honestly
  everything is too cramped as well -- there should be more space
  between all the elements and it should look polished, clean. update
  the linter to reflect that and then fix everything everywhere"; "not
  being cramped isn't just for preferences it's for everything"; "you
  could make some of the preferences wider like a 2 column layout
  instead of one"; "extend the lints to support all the different ux
  things we have so that it won't miss anything"; "improve the fit check
  to catch it -- anything you find that the linter didn't find, improve
  the linter so that next time it will find it"; and of the opened-up
  Deskbar tray: "i actually think i preferred the deskbar icons like
  they were before" (put back, a grid of its own to the review).
  *Done* (2026-11-03): the checks are in the shared review (collapsed,
  outside, cut, overlap, text-outside, text-cut, text-over-text,
  overflow, crowded, window-edge, crowded-lines, control-edges,
  uneven-gaps), the lint's `fit` says how tall a window must be, and
  `tools/dev/ui-lint` runs every app live; the metrics opened up (TIGHT
  6, STACK 10, SPACING 12, WINDOW_INSET 16, BOX_INSET 14); every panel
  refitted from the lint's numbers, Touchpad & Mouse in two columns;
  DeskCalc, Deskbar's clock, Doctor, About, Date & Time, Notifications
  fixed; 36 of 36 targets clean live at 1, 1.5 and 2×; Sound and AI in
  two columns (800 × 466, 760 × 408); the live runner plays Sound
  through olo-fakesound (it only saw "No sound server" before); the
  hooks (tools/hooks: rustfmt and `cargo ui-lint` before a commit, the
  live lint of changed apps before a push); the palm trainer's screens
  as `ui_lint` tests (every step and state, full screen); AirPods in two
  columns (960 × 622, was 520 × 902). Themes and Default Apps stay one:
  a list of wide theme rows over a row of colour cards, and tabs of
  lists, want the width, not boxes side by side; both fit a MacBook's
  982 points.
- ~~**Logs draws while idle**~~ (found by `tools/dev/bench`, 2026-11-03):
  90 to 125 frames in the bench's "idle 1 s". *Done* (2026-11-03): it was
  the journal still loading, a frame for every 4,000-line batch, every
  vblank for as long as a long journal takes (4 s for 400,000 lines
  from a cold cache). What's read now goes to the window at most every
  100 ms as it loads (the first batch at once), continuations still
  joined to their lines. The bench at 2×: launch and load 272 → 32
  frames, idle 1 s 93 → 8 (the live tail's 10 Hz).
- ~~**The palm model in Activity, and first in line**~~ (Ben,
  2026-11-02): "also, the activity monitor should show perf stats about
  the palm model when it's enabled and the supervisor should also make
  sure it prioritizes that model running since it's very latency
  sensitive". *Done* (2026-11-03): Activity ▸ Displays has a Touchpad
  card: the pad's reports a second and their latency from the kernel to
  app_server over the last minute (typical, 99th percentile, longest:
  libinput's and the filter's work, the palm model's included, and any
  wait for the main thread), red past 4 ms; which judges palms (your
  model, trained when on how many touches, and how it tested; or the
  size alone; or libinput's own) and what the main thread runs at (nice,
  realtime, a fast core). app_server keeps the last minute (21 ns a
  report). The supervisor holds Olo's tiers every pass instead of once:
  app_server's main thread, where the model runs, raised again if it's
  lowered, asked again a minute after rtkit says no. Realtime for the
  main thread was weighed and not done: it draws and compiles shaders,
  and rtkit's 200 ms limit would kill it; a thread of its own for input
  is the next step if Ben's numbers say so (Open).
- ~~**The whole repo formatted, and kept so**~~ (Ben, 2026-11-03): "you
  can and should also just do a cargo fmt on everything so that it's
  all consistent and add that as something that happens automatically
  with the linters moving forward". *Done* (2026-11-03): `cargo fmt` over
  the workspace (450 files), each by the rustfmt.toml nearest it: Olo's
  wide style at the root (200, `use_small_heuristics = "Max"`), upstream's
  in gpui/ (Blade's crates 2021) and smithay/ (110); vendor/ left as its
  authors wrote it (`cargo fmt --all` would format it too). The
  pre-commit hook formats what's staged (tools/hooks, installed by
  tools/setup.sh). Built and tested after: the same tests pass (Activity's
  two that need priorities this sandbox lacks aside); olo-sync's
  lock-order test now runs only in debug builds, where locks are checked.
- ~~**Each section of a preference panel back to its defaults**~~ (Ben,
  2026-11-02): "there should also be a way to reset any tuning
  slider/dropdown etc to the defaults for just that section instead of
  just one defaults button for everything". *Done* (2026-11-03): a box
  whose settings aren't their defaults has Reset at the end of its top
  line (the kit's `BBox::action`, a new `LinkButton`), putting back that
  box's settings alone: every control bound through the form leaves its
  setter and value in its boxes; set on the defaults they show whether
  the box changed, and where. Hand-built pickers (wallpaper, desktop
  colour, screen saver module and corners, colour sets) bound too;
  Defaults stays for the whole panel. Tried headless: Windows ▸ Space
  Around Arranged Windows changed, Reset shown in that box only, clicked,
  back to 8 and gone; light and dark at 1, 1.5 and 2×. The lint draws
  every resettable panel with every Reset showing too. Found on the
  way: an auto-hiding ScrollView whose content overflowed on its first
  frame never showed its bar and drew its content over what's below
  (Touchpad ▸ Mouse over Defaults and Revert, now and then: the settings
  index changed between runs); fixed in the kit, and the review no
  longer takes any clip with a scroll view in it for the scroll view's,
  which hid that and two more (Default Apps' "Olo's Choice" 12 pt into
  the inset, Tracker's count bar a line past the window), both fixed;
  the Mouse tab in two columns.

- ~~**⌘W doesn't close windows**~~ (Ben, 2026-11-03): "cmd+w shortcut
  doesn't actually seem to close windows. it just ignores me when i
  press it". *Done* (2026-11-03): ⌘W was only bound by apps with a
  Close of their own (StyledEdit, Terminal, Tracker); in the rest
  (Preferences, DeskCalc, Logs, …) nothing took it. The kit's Frame now
  takes the profile's ⌘W when the app didn't, and closes the window as
  its tab's close button does (`Window::request_close`, GPUI: the
  should-close hook first, so an app's confirmations still ask, then
  closed, on the event loop's next turn). Tried headless: Preferences,
  DeskCalc, Logs, StyledEdit and Tracker each closed by ⌘W, the last
  window quitting its app.

- ~~**Viewer: VP9 plays**~~ (left from the Viewer's video work): VP9
  (WebM, Matroska, mp4) showed only its facts. *Done* (2026-11-02):
  decoded in process by `filmcraft-vp9` 0.9 (clean-room pure Rust, MIT/
  Apache, profiles 0 to 3), as `olo_images::vp9` beside `av1`: the same
  planes (`av1::Frame`), so the same RGBA/BGRA conversion, queue and
  posters. Pictures keep their sample's time, so a damaged sample
  doesn't shift the ones after it; a seek resets the decoder and keeps
  its workers; Matroska block groups' key frames are read from VP9's
  own header. 1080p VP9 decodes (and turns RGBA) at ~120 pictures a
  second on this machine, AV1 at 139 to 190. Tests: `vp9_plays` (mp4
  and 10-bit WebM, every picture in order, posters, facts), VP9 in
  `seeks_to_the_frame` and `keeps_up`.

- ~~**The palm trainer in a modal of its own**~~ (Ben, 2026-11-02, with a
  screenshot of the Touch tab at 2× in dark mode: the trainer's box ran
  under Defaults and Revert): "uh that's not going to work it doesn't
  fit. it should be a new modal for the trainer". So: the Touch tab
  keeps only a button, "Train for Your Palm…", beside the model option;
  it opens the trainer (apps/preferences/src/palm_trainer.rs, its
  steps, animation, live pad, feedback, progress and test as they are)
  in a window of its own, a modal dialog of the Preferences window
  (`olo_ui::open_window` with the kit's dialog options, as the palette's
  `overlay`/window_menu does; sized for its content, ~560×480 at 1×,
  never scrolling), with Cancel and the step buttons in its bottom
  row; closing it stops the worker. The Touchpad panel goes back to
  460×586. Check at 1, 1.5 and 2×, light and dark, headless with
  `SyntheticEvent::Contacts`.
  *Done* (2026-11-02): a 560×440 dialog from "Train for Your Palm…";
  the Touch tab is back to 460×586. The film is a laptop's deck (keys,
  the pad between the palm rests); the pad live is the pad's own
  shape, contacts clipped to it. Also, from Ben's question about one
  hand's palm over the left or right edge: a sixth exercise for it, a
  hand-written unit, and no verdict before a contact is 40 ms old (an
  edge finger's first report looked like a heel). Review clean at 1,
  1.5 and 2×.

- ~~**Palms told apart by a tiny model, trained in Preferences**~~ (Ben, 2026-11-02): "enhance the
  palm detection with an optional new feature (can be enabled in
  touchpad prefs) that uses a tiny gpu accelerated (or simd accelerated
  if it's faster) model to detect palm detection and ignore the palm on
  the touchpad. do research to see if there's anything like that out
  there already, and if not then train up a small model to do it. if
  you can't find anything or it's not viable then try to do it with
  heuristics/decision trees or something but i bet you can find a model
  out there to do it".
  - Today: Olo's touch filter (app_server `touch_filter.rs`, the
    libinput Lua plugin `touchpad.lua`, docs/TOUCHPAD.md) calls a touch
    a palm by its size alone (`palm_size`, 1600 on the MacBook, from
    measured fingers and palms), and takes the click from the fingers'
    pressure.
  - Research first: published palm-rejection models (ChromeOS's
    touchpad palm model in its gestures library, Apple's and Microsoft's
    papers, academic ones on capacitive pads) and the features they use
    per contact over time: size, pressure, shape (major/minor,
    orientation), speed, position from the edges, how long it has been
    down.
  - Use one where there's one fit to run; else train a small one
    (a few-kilobyte MLP or gradient-boosted trees) on captures from the
    MacBook pad (/dev/input/event5, read-only), labelled by deliberate
    sessions: typing with palms resting, thumbs at the edge, real
    gestures.
  - Run it in the filter's path per frame of contacts, on the CPU with
    SIMD unless the GPU is measured faster: microseconds a frame, no
    allocation, nothing when no finger is down.
  - An option in Preferences ▸ Touchpad & Mouse ("Tell palms apart with
    a model"), off by default, beside the size rule it improves on.
  - Measured against the size rule on held-out captures (palms missed,
    fingers lost), and its cost per frame, with the numbers in the
    commit.
  - *Research* (2026-11-02): there is no open touchpad palm model to
    take.
    - arXiv, searched up to 2026, has nothing on touchpad palm
      rejection: its "palm" papers are palmprint biometrics.
    - Hugging Face has no such model or dataset.
    - Closest:
      - Chromium's `NeuralStylusPalmDetectionFilter`, in Android's
        `libpalmrejection` (BSD): a small network for Chromebook
        touchscreens. It scores a stroke from its first 3–12 samples
        (size in mm, pressure, edges, speed) and its nearest
        neighbours; 173–325 inputs; output thresholds 0.9–4.5.
      - ChromeOS's touchpad gestures library: heuristics only (palm
        pressure 200, width 21.2 mm, edge zones, a "fat finger"
        reclassified by low peak pressure).
      - Synaptics' neural palm rejection (CES 2025): proprietary.
      - A 2021 Brno thesis ("Laptop Touchpad Palm Detection with
        AI/ML"): reports good accuracy, but published no code or data.
      - PalmTouch (2018): a CNN on phones' raw capacitive images, which
        the MacBook pad doesn't give (contacts only: position, major,
        minor, orientation, pressure).
    - So: our own small model, Chromium's kind of features taken to the
      pad, trained on captures of Ben's MacBook.
  - **How** (Ben, later the same day): "actually, let's do it by adding
    a "Train for your palm" section in the touchpad preferences that
    then walks the user through a training regime, asking them to do
    different things like rest their palm and tap/scroll with and
    without palm and so on to collect enough samples to build the model
    and then let the user test -- it should be visual with generated
    olo style instructional animations and walk the user through the
    whole process from calibration to testing". So: a guided trainer in
    Preferences ▸ Touchpad & Mouse, made from the kit's parts:
    - Calibration: the pad's size and a resting finger's and palm's
      contact.
    - Exercises, each with an Olo-style animation of the hands on the
      pad and a live drawing of what the pad sees: rest the palms; type
      with palms resting; tap, point, scroll and click, with and without
      a palm down; thumbs at the edge. Each says how many samples it
      still needs.
    - The model trained on the machine, off the UI thread with live
      progress, and kept with the person's settings.
    - A test step: the pad's contacts drawn as fingers or palms, as
      the model and the size rule each call them.
    - The option to use it, and to train again.
    - Ben, also: "it needs to be a very interactive process with the
      user instructing them through the whole thing with live feedback
      and so on". Each step:
      - says what to do now and shows it;
      - shows the person's own contacts on the pad as they happen;
      - says how it's going ("Good, keep resting it", "That was a
        finger: lift it and rest only your palm", "3 more taps");
      - moves on by itself once it has what it needs;
      - can be gone back to or redone.
  - Ben: "for any training you need to use rust (you can use libtorch
    through c/c++ if there's no better rust native solution) don't add
    python". Training, features and inference are Rust (a Rust-native
    crate, or our own: the model is tiny); no Python anywhere.
  - *Status* (2026-11-02): the model's side is done:
    - `olo-palm`: features, training and threshold, with tests;
    - the filter runs it, matching Rust to 1e-4, at about 2 µs a frame;
    - app_server's live contacts (`EventKind::TouchContacts`, and
      `SyntheticEvent::Contacts` headless);
    - the settings for it.
    Ben, also: "you should also write a baseline model by directly
    writing out the weights that provides a good starting point so that
    the user training is really fine tuning that base model you made".
    *Done* (2026-11-02): `olo_palm::baseline()`, weights by hand from
    docs/TOUCHPAD.md's measurements; untrained it beats the size rule on
    the synthetic contacts (>85% of palms, <10% of fingers lost); the
    filter runs it with `use_palm_model` on and nothing trained;
    `train()` fine-tunes from it (lr 0.01). *Left*: the trainer says it's
    refining Olo's own.
    *Next*: the trainer in Preferences ▸ Touchpad & Mouse, a "Train for
    Your Palm" box on the Touch tab (apps/preferences/src/touchpad.rs,
    `touch()`; the panel is 460×586, `panel.rs`), built from:
    - the pad drawn with `gpui::PathBuilder::fill()`/`stroke()` and
      `window.paint_path` (as `style::latch`), contacts as ellipses from
      `Event::TouchContacts` on a watcher thread like `touchpad::watch`;
    - animations by `window.request_animation_frame()` in a canvas, as
      `spotlight.rs` does, stilled when nothing moves;
    - progress with `widgets::StatusBar::new(fraction)`;
    - training on `cx.background_spawn`, the model written through
      `ui.edit`/`prefs.update(.., p.edit(..))` into
      `s.touchpad.filter.palm_model`, `use_palm_model`;
    - the trainer's state in `touchpad::State` (reached by `state_of`),
      redrawn with `Msg::Redraw`;
    - tested headless with `SyntheticEvent::Contacts` through
      `tsession ipc`.
  - (The manual captures below are the fallback, and for measuring.)
  - *Needs Ben* (superseded by the trainer): labelled captures, recorded read-only with no grab
    (`touch_filter`'s `replay_capture` reads them,
    `OLO_TOUCH_CAPTURE`). Two minutes each:
    - `sudo timeout 120 cat /dev/input/event5 > palms.evdev`: typing,
      palms resting as they do, the pad not used on purpose (every
      contact a palm, thumb or knuckle);
    - `… > fingers.evdev`: the pad used as usual (pointing, scrolling,
      clicks, gestures), palms kept off it (every contact meant);
    - `… > mixed.evdev`: real work, both at once, to try it on.
    - Then: features per contact, a model trained and checked on
      held-out parts, run in `touchpad.lua` (tens of multiply-adds a
      contact: Lua costs microseconds, where a GPU dispatch would cost
      far more), the Preferences option, and the numbers.
  *Done* (2026-11-02; docs/TOUCHPAD.md "Palms told apart by a model",
  "The trainer"): `olo-palm` (14 features, 32 hidden units, 22 written
  by hand: the baseline beats the size rule untrained), the filter
  running it at ~10 µs a frame, app_server's live contacts, the guided
  trainer in Preferences ▸ Touchpad ▸ Touch with animations, live pad,
  feedback, progress and a test step, the option beside the size rule.
  The model is in `Settings`, so Backup's export/import carries it
  (Ben, 2026-11-02). *Left for Ben*: try it on his own hands (the
  synthetic contacts can't say how real palms score), and tune the
  hand-written units from what the Test step shows.

- ~~**Rendering made for each display**~~ (Ben, 2026-10-31): "we should also
  detect the display types and optimize rendering for the specific
  displays and their types/characteristics don't just assume all
  displays are the same". Each display's density (pixels per inch from
  its size), subpixel layout (DRM's, else a guess: a low-density external
  LCD is RGB stripes, an OLED by its model has none) and refresh rate,
  with Preferences ▸ Display to correct the layout (a ClearType-tuner
  style picker); told to clients in wl_output's geometry, so other
  toolkits benefit too. Olo's GPUI picks per window, by the display it's
  on: LCD subpixel text (FreeType's LCD filter, dual-source blending) on
  low-density striped LCDs over opaque backgrounds; grey-scale with light
  hinting in between; grey-scale unhinted, as macOS, at Retina density.
  Ben's displays: the MacBook's XDR (254 ppi, 120 Hz), a Studio Display
  (218 ppi) and a Dell S2425HS (92 ppi, 1x: where it matters most).
  Preferences ▸ Display (Ben, 2026-10-31): "in the display prefs app we
  should show each display name/manufacturer/model and
  technology/features/characteristics and all the settings we've
  selected/tuned for each one and a little help hint or something
  explaining why but give the user the ability to override if they
  want". app_server makes the choices (one place), tells apps and
  Preferences (`OutputInfo`), and keeps overrides in the display's
  settings.
  *Done*: `olo_protocol::panels` (each display's density, technology
  guessed from what it is, stripe order from the display or guessed,
  and the text rendering with why; overrides in `OutputConfig`),
  reported in `OutputInfo::panel` and in wl_output's subpixel; Olo's
  GPUI draws by it (LCD subpixel text with dual-source blending,
  unhinted at Retina density). `OLO_HEADLESS_PANEL=530x300,rgb` tests
  it headless. Preferences ▸ Display's Panel box shows it all, with why,
  each correctable (Automatic or a choice). The kit passes a person's
  choice to GPUI (`theme::apply_outputs`, `set_display_text_rendering`)
  and every window draws again; make and model show right after a late
  EDID (`OutputInfo`).
  *Done* (checked 2026-11-02, headless on a 92 ppi RGB panel,
  `OLO_HEADLESS_PANEL=530x300,rgb`). DeskCalc drew its text in RGB
  subpixel, as chosen for the panel. `configure_output` with `text:
  hinted` (Preferences ▸ Display's choice) redrew the running app in
  grey-scale at once: 0 coloured pixels where there were 4672. Back to
  Automatic, it was pixel for pixel as it had been.

- ~~**Terminal tells coding agents to use Agent Screens**~~ (Ben,
  2026-11-02): "check if terminal already injects context to claude
  code and codex to tell them about using olo's agent screens and how to
  use them. if not, please add that as a setting in the prefs ->
  terminal that can be enabled so that claude code and codex will get
  those instructions and tell them too that they should prefer olo agent
  screens for the best experience. also make sure that the olo agent
  screens instructions include information about olo chrome and how to
  start it so that they don't start the wrong chrome by accident in
  their agent screen". Checked: nothing injects it today. The session
  installs the `olo-screens` skill for both agents
  (crates/session/src/skills.rs, from apps/screen/SKILL.md), but an
  agent reads a skill only when it thinks it's relevant. Terminal's
  `olo-terminal --agent-context` hook (apps/terminal/src/agent_context.rs,
  `SessionStart` and `UserPromptSubmit` in Claude Code's
  `settings.json` and Codex's `hooks.json`) carries only the tab group
  and the tab-title line. To do:
  - A third setting in Preferences ▸ Terminal, off by default ("Tell
    coding agents to use Olo's Agent Screens"). When on, the same hook
    says at `SessionStart`:
    - prefer Agent Screens for anything graphical;
    - the commands to use;
    - start the browser with `olo-chrome` (a sandbox `--user-data-dir`),
      never `google-chrome`/`chromium`/`chrome` or the person's profile;
    - point to the skill for the rest.
  - The skill's own Chrome section says the same just as plainly, and
    that the text is shared with the hook (one source).
  - Tests with a sandbox HOME, never Ben's agents' real settings.
  *Done* (2026-11-02): Preferences ▸ Terminal ▸ Coding Agents ▸ "Tell
  coding agents to use Olo's Agent Screens" (`agent_screens`, off).
  - Terminal's hook, at `SessionStart`, gives any agent in an Olo
    session `apps/screen/agent-context.md`:
    - prefer a screen of your own for anything graphical;
    - the commands;
    - web pages in `olo-screen run NAME olo-chrome URL`, never a
      `google-chrome`/`chromium` of your own or the person's profile;
    - the skill for the rest.
  - The skill's Chrome section now says the same; its old
    "the person's profile by default" was out of date, since screens
    have their own.
  - Checked: the hook prints it at the start only, not with prompts and
    not outside an Olo session, in 3 ms (no wait for a tab it doesn't
    need).
  - The panel was made taller (420×472). The review is clean at 1, 1.5
    and 2×, light and dark.
  - New test: `agent_screens_words_name_olo_chrome`. The settings index
    was regenerated.

- ~~**A window's tab menu: Close and Quit with their keys**~~ (Ben, 2026-11-02): "for
  some reason "Close" in the tab context menus doesn't have a shortcut.
  on the mac profile super+w should close and for the other profiles set
  them to whatever is standard". The menu is the palette's
  (apps/palette/src/window_menu.rs): its last item is a plain
  `.item("Close", "close")`, so no key shows next to it.
  - app_server's `close-window` shortcut has no key on the Mac and BeOS
    profiles, and Alt+F4 on Windows (crates/protocol/src/shortcuts.rs).
  - Each app's own close is `app.close` (crates/protocol/src/app_keys.rs):
    ⌘W on Mac, Ctrl+W on Windows, Alt+W on BeOS.
  - Show the profile's key next to Close: ⌘W on Mac, Ctrl+W (with
    Alt+F4) on Windows, Alt+W on BeOS. Take it from the same tables, so
    it follows the person's own key changes.
  - Don't give app_server's `close-window` ⌘W. app_server would then
    take ⌘W before apps, and apps whose ⌘W closes a tab rather than the
    window (Terminal, Chrome) would lose it. Choose how the menu's key
    and the apps' key stay one thing.
  - Ben, later: "if it is a window with multiple tabs then cmd+w should
    still close the tab, it would only close the window when the last
    tab is closed too". So ⌘W closes the tab whose menu it is: one
    window of a group (`f.grouped`) leaves the group's other tabs open,
    and an app's own tab (Terminal's, Chrome's) goes as that app's ⌘W
    does. The whole window closes only with its last tab. The menu's
    Close does the same, and its label says which (Close Tab while
    there are others).
  - Ben, later still: "also add a Quit option to menus and that should
    be cmd+q on mac and whatever is normal on the other ones. and the
    diff is cmd+w closes each tab then/or window whereas Quit closes the
    whole app".
    - Add Quit (Quit APP, the app's name) under Close.
    - Show the app's own quit key from `app.quit` (or its own:
      Terminal's `terminal.quit` is Ctrl+Shift+Q on Windows): ⌘Q on Mac,
      Ctrl+Q on Windows, Alt+Q on BeOS.
    - Choosing it quits the whole program, every window of it:
      - a kit app is asked as its own Quit would be (it may ask to save
        first);
      - another program gets each of its windows closed, as Deskbar's
        Quit does.
  - Ben, also: "quitting or closing a window should both still show the
    warning/confirmation (i think we already have this in some places)
    if there is activity in the tab/window/app to make sure the user
    meant to close it".
    - Close (a tab, a window) and Quit from this menu go through the
      app's own close and quit, never around them. Terminal still asks
      "Close anyway?" for a tab running something (apps/terminal/src/ui.rs
      `ConfirmClose`), and an editor still asks to save.
    - Quit asks once for the whole app, naming what's going on.
    - Check every kit app that asks today, and give the ones that should
      ask but don't (a download, a copy under way) the same ask, in the
      kit.
  - A test in window_menu.rs's `keys_of`.
  *Done* (2026-11-02):
  - A kit app's Close and Quit are its own, run in that window (the new
    `commands::menus_in` / `invoke_in`), with its labels and keys.
  - Another program's Close closes the window, and its Quit closes each
    of its windows. Both show the profile's keys for every app's Close
    and Quit.
  - `olo_xdg::apps::app_name` names the app, shared with Deskbar.

  Tried headless (release) at 1×:
  - StyledEdit with unsaved text: "Close ⌘W" and "Quit StyledEdit ⌘Q",
    and each asks "Save changes…" first.
  - Terminal with two tabs: "Close Tab ⌘W" closed one and kept the
    window. With one busy tab, "Close" asked that `sleep` was still
    running.

  The palette's test covers the Mac and Windows keys, another
  program's items and a kit app's. A program's own tabs (Chrome's) go
  with its window from this menu; ⌘W in it still closes one.

- ~~**Deskbar's Olo menu sorted**~~ (Ben, 2026-11-02): "in the olo
  deskbar menu the applications / other applications / preferences items
  (and sub items) should all be sorted lexicographically". Today
  Applications and Preferences follow the order of `OUR_APPS` and
  `PREFERENCE_PANELS`. Other Applications follows a fixed category order
  (Accessories … System, then Other), and its entries come in the order
  they were found (apps/deskbar/src/main.rs, `olo_menu_with_icons`).
  - Sort every level by its shown name: case-insensitive and in
    natural order, with ties broken by the exact name.
  - Keep "Other" last in Other Applications.
  - Put the sort in the kit, so every menu built from a list can use
    it.
  *Done* (2026-11-02): `Menu::sorted()` in the kit, used for all three.
  Checked in a headless shell session: Applications from Activity
  Monitor to Viewer, Preferences from AI to Workspaces, and Other
  Applications' kinds and what's in each. Other stays last. The menu is
  built only when it opens, so the sort costs nothing per frame.

- ~~**Agent Screens: Take Control is laggy**~~ (Ben, 2026-10-31): "when
  using take control in agent screens it is very laggy -- this needs to
  feel as buttery smooth and be as performant as if we were using the
  desktop directly."
  *Done* (2026-11-02). Live shows the screen's own frames, shared as
  dmabufs and never copied:
  - The screen's app_server captures each changed frame on the GPU into
    one of three buffers (`olo_capture::live`, the portal's capture
    client made a crate).
  - The person's app_server shows each as a subsurface of the viewer's
    window (GPUI's new `Window::place_external` /
    `set_external_buffer`).
  - Agent screens start at 120 Hz.

  Measured headless, a window dragged on the screen for 5 s:

  | | Before (pictures) | After (live) |
  |---|---|---|
  | Frames shown | ≤ 8/s, none for pointer-only moves | ~60/s at 60 Hz, ~120/s at 120 Hz |
  | Viewer CPU | 30.0% of a core | 0.6–0.8% |
  | Agent app_server | 2.4% | 4.2–4.4% |
  | Person's app_server | 1.2% | 1.4–2.2% |

  The frame line is flush at 1–2× and at odd sizes. Input already went
  out at once. Input-to-photon wasn't timed end to end: now it's the
  screen's next frame plus the person's next frame.

- ~~**An outside program's `zwp_primary_selection_offer_v1` request
  refused**~~ (seen 2026-11-01 through olo-wlproxy for pid 21201:
  "app_server's error on wl_display@1: invalid method 2, object
  zwp_primary_selection_offer_v1"; the interface has receive and
  destroy only). Find which program sends opcode 2 (a newer
  interface version it was told it has?) and whether app_server
  advertises a version it doesn't serve. (Not seen since 2026-11-01
  13:19, through two days of upgrades.) *Lead* (2026-11-02): not a
  version (primary selection has only v1); opcode 2 is exactly
  `wl_data_offer.destroy`, on 0xFF000002, a server-made id: the program
  destroyed what it held as a clipboard offer whose id app_server had
  already given to a primary-selection offer. Something freed the data
  offer's id before the program destroyed it: app_server destroying an
  offer's resource itself (a selection replaced), or olo-wlproxy's
  server-id mapping (`s_up`, `free`) across a restart. olo-wlproxy
  swallows requests to zombies and maps server ids since; a test with
  a program holding a data offer across `tsession reload`, then
  destroying it, would show which.
  *Found and fixed* (2026-11-02, the test the lead named): GTK's text
  editor through olo-wlproxy, a selection made, app_server restarted
  (`tsession reload`), a new selection: cut off with "invalid object
  0xff000002". The proxy forgot its mapping of a destroyed server
  object before translating the destroy, so it went up under the
  program's id; ids match until a restart, after which the next
  app_server's objects get other ids where the program still holds the
  old ones. Translated first now: the editor survives three restarts in
  a row, copying and pasting each time.

- ~~**A window opens 15–18 ms sooner**~~ (found porting the Viewer,
  2026-11-01: its first frame came 23 ms after egui's). *Done*: its
  renderer is made once, at its device size and the transparency it
  keeps (it was made transparent at scale 1, then made again
  opaque, a second set of pipelines with it, then again at its
  size), and the pipelines are made side by side (10.4 → 4.5 ms).
  The Viewer's first frame presented 25–31 ms after exec, from 43;
  DeskCalc pixel for pixel the same. Left: egui's first frame is
  still ~5 ms sooner (its device and one pipeline).
  *Then* (2026-11-02): the GPU context (4.4 ms) and GPUI's shader
  (WGSL through naga, 3.0 ms) are made on a thread of their own from the
  moment the app starts (`BladeContext::ahead`), where they came one
  after another once the first window was asked for (~3 ms in): the
  first window made 10.2 → 8.4 ms (DeskCalc, median of 10; 6.9 at
  best), 11.4 → 9.4 ms (StyledEdit). What's left after it is the
  pipelines, 1.6 ms (Mesa's disk cache serving them).

- ~~**GPUI text a glyph at a time is slow at full screen**~~ (found
  2026-11-01 by the benchmark): a window full of GPUI kit text painted
  each glyph on its own, so the Notifications History at 1× on a
  3456×2234 screen took 12–25 ms a frame and Log Viewer 9–22 ms.
  *Better* 2026-11-02 (gpui/README.md): every line keeps its glyphs as
  last painted, a scene sorts runs of glyphs rather than each, and draw
  orders come from a grid (bench at 1×, p50 before → after: History's
  pointer 14.7 → 7.1 ms, its scrolling 14.7 → 9.4, its menus 25.9 → 8.1;
  Log Viewer's scrolling 10.8 → 5.1, its menus 11.1 → 5.9). *Left*: a
  screen of History is 36,000 glyphs drawn afresh for a hover (paint
  5.3 ms, its GPU 2.4–3.8 ms); its live resize (35–47 ms: every card
  measured and laid out again) and theme switch; Log Viewer's frames as
  a new line or a search arrives (23–26 ms: rows shaped on the UI
  thread); an Activity Monitor panel's first frame (18 ms). *Then*
  History's cards each paint through a paint memo (`Window::paint_memo`:
  a card unchanged is replayed, moved when scrolled): at 1×, its paint
  for a hover 5.3 → 3.2 ms; at 2×, frames skipped while scrolling 60 →
  10. *Left*: at 1× History scrolls at 60 frames a second (its frame
  ~6 ms and its GPU's ~3 ms together pass the frame: fewer glyphs and
  less overdraw, 2.84 screens of quads); frames skipped scrolling at
  2× with frames of 1.4 ms (the scroll events' pacing against the
  frames, not their cost); Log Viewer shaping new rows on the frame.
  *Then* the cull cuts grounds by their biggest covers first and
  shadows skip their blur inside: History's GPU at 1.5× 4.8 → 3.3 ms a
  frame, its drag's hitches 9 → 3 (its glide still misses a frame now
  and then).
  *Then* (same day) shaped lines are aged rather than dropped, shape
  plans kept, and ASCII shaped by words (gpui/README.md): History's
  live resize, slowest frame 60 → 17 ms (p50 11.8); Log Viewer's rows
  arriving with a search, 31.6 → 10.8 ms.
  *Then* (2026-11-02) frames on the fast cores: the scheduler put a
  mostly idle UI thread on a slow core at a low clock, and the frame
  after a pause ran three or four times as long; GPUI's main thread and
  app_server's ask for a fast core while they run (`uclamp.min`,
  HACKING rule 9). Paint memos are kept through a cached view replayed
  whole, an icon coming in and a display event (Activity's 1,400 cells
  were painted afresh the frame after each). Activity's first frame of
  its processes 13–17 → 3.7–4.6 ms. The full bench: 21 failed of 382 →
  2 of 396, every app's slowest frame at most 4.0 ms (from 14.9),
  app_server's frames all within budget. *Left*: Notifications'
  first scroll skips a frame at 1× and 1.5× (its frames 1.5–3.3 ms: the
  pacing, not the cost).
  *Done* (2026-11-02): Notifications scrolls evenly at 1× and 1.5×
  (steps 0.96–1.01 of the fingers', its glide within 1%, frames at most
  2.5 ms); Log Viewer's list (a ColumnListView, which didn't report its
  offset, so its scrolling went unmeasured: it does now) drags at
  1.00–1.00 and glides evenly at 1× and 2×, frames at most 2.4 ms.

- ~~**app_server: a 16 ms frame during Terminal's live resize at 2×**~~
  (the bench, 2 runs in 5: "16.2 ms drawing 7.7 Mpx; 1.0 ms on a CPU,
  3 waits"): app_server waits, not computes. Not seen resizing
  StyledEdit or Tracker, nor the egui Terminal; 2 of 8 runs of the 2×
  bench, none of 5 resizes done as the bench does in a session of its
  own. Not the wallpaper (Terminal's surface is opaque) nor importing
  (that's gathering): the GLES draw blocks, likely on the GPU while the
  client makes its screen-sized swapchain for the resize (three of 123
  MB at 2×, on the press). Time app_server's passes on the GPU around
  it and keep the client's allocation from stalling the compositor.
  *Gone* (2026-11-02): four bench runs of Terminal at 2×, its live
  resize's app_server frames at most 0.58–1.84 ms (and Terminal's own
  at most 0.76–3.75), since the GPU-wait fixes and GPUI drawing a live
  resize in part of a bigger swapchain.

- ~~**Telegram: changes that never reached Telegram**~~ (Ben,
  2026-11-02: "telegram still doesn't work"; "is it possible that when
  i unarchive it isn't ever getting to telegram server? like maybe i'm
  quitting before it syncs?"; "add a status icon or something and when
  it's quiting wait up to 30s whilst showing sync status"). It was:
  each change a request nothing kept, quitting cutting off those in
  flight, bulk unarchive sent as one request a chat (Telegram throttles
  that). Now an outbox on disk (sent until Telegram has it, again on
  the next start; archive moves 100 to a request; waits honoured), a
  sync line under the chat list, and quitting that waits up to 30 s
  showing the changes go (Quit Now). And Olo's own programs started by
  Deskbar, Tracker or the palette log to the journal (they went to
  /dev/null: Telegram opened from Deskbar left nothing to read).

- ~~**app_server: its frames wait on the GPU behind clients' work**~~
  (found 2026-11-02, the full bench: 33 of 376 steps failed, most of
  them app_server frames of 9–51 ms as a menu opened or a window
  resized). Smithay's damage tracker now names a slow step: every one
  is `frame.finish()`. The headless backend made its GL context at
  normal priority, the DRM backend at high: made high too (the bench's
  worst, Passwords' menus, 51 → under 8.3; Viewer's 21 → 15.7).
  *Found* (same day, the kernel's syscall trace and kprobes): not the
  GPU. `finish()` sat in `glFenceSync`, waiting (`futex`) on Mesa's
  lock of its device's buffers, held by the dmabuf check worker through
  `DRM_IOCTL_ASAHI_VM_BIND`, which spent 7–35 ms in
  `drm_gpuvm_bo_deferred_cleanup` freeing the window buffers let go of
  since the last bind (1–4 ms each: pages and IOMMU mappings). *Done*:
  the worker checks on a device of its own and holds each buffer till
  the renderer has let it go, freeing it there as no check waits
  (`dmabuf_check`). Viewer's first menu 33–37 → 0.7–1.0 ms; the full
  bench 31 → 21 failed of 382, app_server's failures ~18 → 4.
  *Left*: app_server's own big buffers (a screenshot's 31 MB texture
  and read-back) are kept by Mesa ~3 s for reuse and then freed in the
  renderer's next bind (Screenshot's live resize, 18–21 ms; its scroll
  at 1×): keep them (a few, long-lived) rather than make one per shot.
  (Clients making a new swapchain for every size of a live resize:
  done, GPUI draws in part of a bigger one while a resize lasts,
  `BladeRenderer::update_drawable_size_within`.)
  *Gone* (2026-11-02): the bench's Screenshot app, live resize and
  typing at 1× and 2×: app_server's slowest frame 0.29–0.87 ms (from
  18–21), its own big buffers no longer felt; the full bench since had
  every app_server frame within budget.

- ~~**Glyphs drawn in a frame are slow the first time**~~ (found
  measuring Telegram, 2026-11-01): a chat opened with 230 glyphs new
  to the process spent 6–9 ms of its frame (release) on first-time
  work, ~3 ms of it drawing glyphs (12 µs each: outline, hinting,
  emboldening, filling) and up to 0.4 ms a colour emoji. *Done so
  far*: glyphs are filled to their exact coverage by an area
  rasterizer kept between glyphs (5.9 µs a glyph against vello_cpu's
  7.6–9.7, and true to the outline: vello's was up to 57 levels of
  grey off on curves). *Done*: glyphs drawn ahead, off the UI
  thread, for faces drawn as running text and those an app asks for
  (`Window::warm_glyphs`), so a frame only copies them; and a
  window's first frame drawn once, at its display's scale and text
  rendering (it was drawn at 1, then again: 6–8 ms, and its glyphs
  for nothing). Telegram's chat switches: slowest frame 9.6–10.2 ms
  → 6.6–7.6 ms, 0.7 MB more memory. *Done* (2026-11-02, the emoji
  picker): colour glyphs are kept for every window of an app once drawn
  (up to 32 MB, other scales' let go first), and `Window::warm_text`
  draws a text's ahead off the UI thread (the picker's 1,900: its
  slowest frame at 2× 8.1 → 1.5 ms). With colour glyphs kept, a chat's
  emoji cost 0.05–0.07 ms each the first time the app draws them, once:
  Telegram on the bench (its pretend 3,000 chats; scrolled, typed in,
  resized, menus) has no frame over 2.02 ms at 1× or 2×.
  Deskbar's battery and Wi-Fi now follow UPower's and
  NetworkManager's signals (idle: 0 wakeups a second, from 7–9).

- ~~**A resident overlay keeps ~60 MB after its window closes**~~
  (2026-11-02): it doesn't. The 60 MB was resident pages, most of them
  shared and the system's to drop (Vulkan's driver code, mapped fonts).
  In private memory (`Private_Dirty`, smaps_rollup) the palette is
  2.8 MB never shown and 13.6–13.7 MB after 1 to 15 openings (no leak;
  Ben's 118 MB is its file and password indexes of a real home); the
  emoji picker 5.6 MB never shown and 38 MB hidden after, steady over 12
  openings, 17 MB of it its emoji kept for the next opening (let go
  after ten minutes); its copy of the emoji font (read whole to check
  which emoji it draws) is a mapping now, 50 → 38 MB.

- ~~**Emoji picker**~~ (Ben, 2026-10-31: "an olo native emoji picker
  app that inserts the selected emoji into wherever the text cursor is
  in the active window… fuzzy keyword search… ask ai to choose the
  emojis based on a description… super+enter… 5 different
  suggestions"). `olo-emoji-picker`: ⌃⌘Space (Windows' ⊞.) over
  everything, resident; search as you type over names, CLDR keywords
  and shortcodes (letters left out forgiven, the plainest and most used
  first); a tab a group and Frequently Used; skin tones kept; ⌘↩ asks
  the model chosen in Preferences ▸ AI ▸ Emoji (new: on, Astra 6, low)
  for five, each with its reason. Return puts it into the field that
  had the keyboard in any app: app_server's new `Request::InsertText`
  (text-input-v3's `commit_string`, app_server the input method just
  long enough, else keys on a keymap made for the text; tried in
  StyledEdit, Chrome 156 and GTK). `olo-screen type` puts in what a US
  keyboard can't type the same way. Agent-readable throughout. On the
  way: GPUI now shows ✋ ⚡ ✨ ✅ and other default-emoji symbols in
  colour, and skin-toned ✌🏽 (`olo_emoji::starts_emoji`, shared); the
  kit keeps colour glyphs and draws them ahead (`warm_text`); bench has
  an `emoji` step (every frame ≤ 1.5 ms at 1×–2×).

- ~~**Telegram: unarchived chats go back to Archived on every reopen,
  and read chats come back unread**~~ (Ben, 2026-11-02: "every time i
  reopen telegram all my unarchived conversations move back to
  archived"; "it keeps saying that my messages for justin are unread";
  "every time i start it very briefly shows me the right chats not
  archived but then flashes to put them archived"). The cause: the
  updates stream's place (pts) was never saved (grammers keeps it only
  when asked, `sync_update_state`), so every start replayed every update
  since signing in, after the list was read: each old message counted
  unread again (Justin's 35), each old move to the archive undid the
  later unarchive. Now the place is saved after each update; a start
  more than six hours on begins from now; a message counts unread only
  if newer than the row's last and than what's read, a read only if it
  reads further; and an update that changes a chat's state has the chat
  read again from Telegram (`getPeerDialogs`, batched), which wins.
  Kit apps log Olo's own info again (`warn,usvg=error,olo=info`, as
  documented; it had become `warn` with the move from egui), so the
  chat list's loads and moves are in the journal. (Earlier the same
  day: a failed page asked again, the cache following each change, a
  move shown where Telegram put it.)

- ~~**Agent Screens: archive and delete, one or many**~~ (done
  2026-11-02: the kit Sidebar chooses several, `also`/`on_pick`/
  `on_menu`/`on_select_all`) (Ben, 2026-10-31):
  "i should be able to archive stopped agent screens so that they no
  longer show in the list but there should be a menu option to show
  archived to bring them back into the list but in the list the archived
  ones should have their own section. there should also be an option to
  delete. and for both archive/delete i should be able to multi-select
  agents in the sidebar list and right click to take bulk actions like
  archive/delete etc". So: Archive (stopped screens) hides a screen from
  the list; View ▸ Show Archived brings them back in their own section
  (with Unarchive); Delete removes a screen and its history, recordings
  and stats (to the Trash where they're files, asked once for many); the
  sidebar takes ⇧/⌘-click and ⌘A selection, and its context menu acts on
  every selected screen (Archive, Unarchive, Delete, Stop).

- ~~**Agent Screens: rename a screen**~~ (done 2026-11-02: Screen ▸
  Rename… or a click on the header's title; `olo-screen rename`; kept
  with the history, shown in the list, the header and `olo-screen list`;
  Deskbar shows only a count, and move-to menus don't exist yet) (Ben, 2026-10-31): "i should be able
  to rename an agent screen". A name the person gives (in the list, the
  screen's title and its menu, as Tracker renames: click the name or
  Rename…), kept with the screen's history and shown wherever the screen
  is (Deskbar's item, move-to menus, `olo-screen list`, the agent's tree).

- ~~**Displays: kept where they are, numbered for good**~~ (done
  2026-11-02) (Ben, 2026-10-31):
  "we absolutely must get these upgrades (and connecting/disconnecting
  monitors) keeping things where they already were on the right
  display-workspaces (unless the display is no longer available in which
  case move it to the primary display on the matching workspace) … also
  make sure that we are using some kind of permanent identifier for each
  display behind the scenes so that what display is 0/1/2 etc doesn't
  change and remains consistent across connect/disconnect, restart etc.
  … and it has to work with auto tiling properly so that they come back
  with the same positions and any custom resizes etc". Upgrades: done
  (7a7c5fd: displays known by their monitors before use, windows back on
  their own displays; `tools/dev/layout-check`). Numbers: done (each
  monitor's number, D0, D1, …, given once and kept in the settings by its
  key, `DisplaySettings::numbers`). Unplug and plug: done (2026-11-02:
  `display_leaving`, `windows_back_home`, `AutoTiler::leave` /
  `come_back`; `plug_display` for test screens, `layout-check
  --unplug`). And `layout-check --auto` (the reload)
  passes again (it failed before the unplug work too): a group's float
  is every window's in it, so whichever comes back in front floats it,
  and a window placed onto an auto-tiled display from another floats
  there; the check matches the only window of an app whose title
  changed (a Terminal's, re-executed).

- ~~**Telegram crashed: Vulkan ERROR_SURFACE_LOST_KHR**~~ (done
  2026-11-02: blade-graphics, GPUI's renderer and Wayland client) (seen
  in the logs at 17:02, during an upgrade): an upgrade restarts
  olo-wlproxy and app_server one after the other, and Telegram's window
  lost its Vulkan surface between two queries
  (`surface.rs:244` unwrapped), so the upgrade closed it instead of
  re-executing it (Ben then reopened it by hand). Every surface call
  now answers a lost surface with no frame, the renderer makes the
  surface again, and a program re-executing connects again if the
  compositor goes before its registry answers (the second panic the
  same test found). Tried twelve times running: a copy of the binary
  replaced, SIGURG and `tsession reload` together.

- ~~**The file panel after Quick Look: tiled, and Open does nothing**~~
  (done 2026-11-02: the look floats over the panel, centred on it; Open
  answers after a look) (Ben, 2026-11-02): "when i'm in slack and attach
  a file i correctly get the olo tracker as the file picker. however,
  when i hit space for quick look then the preview gets tiled (i think
  quick look should also be floating). and worse, then i hit the open
  button in the tracker picker but nothing goes into slack. if i attach
  without previewing first then it does add correctly." The look is a
  dialog, never a tile or a tab, and opens centred over the panel: the
  panel exports its window (xdg-foreign, `Window::export_handle`, new in
  Olo's GPUI) and passes the handle to `olo-viewer --look --over=`;
  app_server keeps a dialog that sizes itself after opening centred
  over its parent until it's moved (`keep_centred`). Open after a look
  was tried as Slack asks, through the real xdg-desktop-portal with
  Chrome's upload (Electron's code) over a private bus, in Standard,
  Focus and Auto-Tiling, with the look up and put away by Space or
  Escape: the file reached the page every time. Ben's report came from
  the build where the look took a tile; if Slack still drops it, it's
  something of Slack's own to look at.

- ~~**Notification banners draw over Deskbar's menus**~~ (done
  2026-11-02: app_server render.rs, `shell::layer_menus`) (seen
  2026-11-01: a banner's layer sits above a menu popup). A layer
  surface's menus went with it, so a banner (a Top layer surface that
  came after Deskbar) drew over Deskbar's menus and took their clicks.
  Every layer surface's menus but the Overlay's are now drawn, hit-tested
  and given focus over the whole Top layer, as the desktop's already
  were over the windows.

- ~~**`pbcopy` and `pbpaste` from the command line**~~ (done
  2026-11-02: `apps/pb`, named by the session) (Ben, 2026-11-02):
  "have olo install a pbcopy shim that copies to the clipboard from the
  cli". Native, as macOS's: `pbcopy` holds what it reads from stdin on
  the clipboard (as text, the data-control protocol app_server speaks,
  `olo_kit_base::clipboard`), so Paste in any app gets it, and stays
  until something else is copied; `pbpaste` prints the clipboard's text.
  The session puts them on PATH (`~/.local/bin`, as `olo-logs` is), in
  every shell and Terminal, and agents use them as people do; no
  wl-clipboard, nothing wrapped.

- ~~**Menus taller than the screen scroll**~~ (done 2026-11-02: in the
  kit's `menu`, for every app) (found 2026-11-02 in
  Preferences ▸ Default Apps, whose app pop-up listed every app): a
  kit menu longer than the display runs off its edge, and what's past
  it can't be reached. As BeOS's did: a menu is kept to the display
  (the compositor's constraints), and one that doesn't fit scrolls,
  with arrows at its ends that scroll while the pointer rests on them,
  the wheel, and the keyboard's ↑↓ bringing the item into view; agents
  read every item, shown or not.

- ~~**Olo Suggestions**~~ (done 2026-11-02: `olo-suggestions`, Olo
  Doctor's body with a `Kind`; brush built with Olo as `olo-brush` and
  applied as the login shell through AccountsService, undone with a
  button; Terminal follows the account's shell at once. Not applied on
  Ben's machine by the tests: his to press) (Ben, 2026-11-02): "similar to olo doctor, add an
  "Olo Suggestions" app and in there it should have things that Olo
  suggests doing to have the best experience in Olo. they're things
  that are optional / nice to haves, and Olo should still work fine
  without them. each one should have a button to apply it and an apply
  all button. the first suggestion i want is to change the user shell
  to https://github.com/reubeno/brush (bash rewritten in rust with more
  features) and if the user applies it then we should install it and
  update the user's shell to use it as default shell". So: an app
  built as Doctor is (`apps/doctor`: its checks, its window, its
  privileged helper and the approval it asks), listing suggestions
  that aren't problems (Doctor keeps those): each with what it does
  and why, its state (applied or not, checked off the UI thread), an
  Apply button, and Apply All; applying shows live progress, can be
  cancelled, and is safe to run again; agents read and apply them as
  people do. First suggestion, **brush as the login shell**: build and
  install brush (`cargo install` of the `brush-shell` crate, as the
  setup app installs Rust things, into `~/.cargo/bin` or a system
  path a login shell can be), add it to `/etc/shells`, and make it the
  person's login shell (`/etc/passwd` through the helper, with the
  admin approval), Terminal's new shells following; undoing it puts
  the shell back as it was. Doctor's "Fix in Olo Doctor…" style link
  from Preferences where it fits.

- ~~**Telegram: archived right, unknown senders apart, bulk actions,
  smaller avatars**~~ (done 2026-11-02, as far as the demo shows: the
  list now comes as the main list then the archive, each authoritative,
  and chats the live list no longer gives (cached from before, left,
  deleted) go once it's whole: two such, archived, named like active
  chats, would look just like this; Unknown Senders; ⌘-click, ⇧-click
  and Select All Chats with Archive, Unarchive, Mark as Read, Mute,
  Unmute and Delete on them all; avatars 40 pt from 46. *Ben to check*:
  whether Justin Martinez and The Real Bots Team still show archived
  after the upgrade; if so, Chat Info on one says which chat it is)
  (Ben, 2026-11-02): "in telegram app, make the
  avatars a bit smaller they look oversized now, and also add bulk
  select and bulk actions to the list of people so that i can e.g.
  bulk archive, bulk unarchive, delete etc etc and so on. also make it
  so that any messages from anyone i've never sent a message to go
  into unknown senders and don't send a notification for them
  (they're almost always spam) and also don't treat them as
  unseen/unread. and also with archived for some reason some of my
  most active chats are saying they're archived, like with "Justin
  Martinez" and "The Real Bots Team" so i think there's something off
  with how you're deciding what is and isn't archived." So: (1) **the
  bug first**: find how a chat is decided archived (the chat list's
  folder id, `chatListArchive` against the main list, what an update
  moving a chat says) and why active chats show as archived; check it
  against what Telegram's own apps show for the same account; (2) an
  **Unknown Senders** list: a private chat with someone Ben has never
  written to (no outgoing message in it) goes there, notifies nothing
  and counts as neither unread nor unseen (Deskbar's badge, the Dock's,
  the list's), until Ben writes back, which makes it a chat like any
  other; (3) **bulk select** in the chat list (⌘-click, ⇧-click, ⌘A,
  a band) with the actions on all chosen: Archive, Unarchive, Mark as
  Read, Mute, Delete (asking first, saying how many), in the list's
  context menu and the menu bar, agent-readable; (4) **avatars a bit
  smaller** in the chat list, measured against the row's text at 1×,
  1.5× and 2×. Test with the demo account only.

- ~~**The file picker opens where it was last, for each app**~~ (done
  2026-11-02: `places::last_for` / `left_in`, the portal's panel by the
  asking app, the focused window's when the portal names none) (Ben,
  2026-11-02): "olo tracker when launched as a file picker from another
  app should remember and reopen in the last location it was in for
  that app specifically (if any)". So: the file panel `olo-portal`
  shows for an app (FileChooser, by the asking app's id) opens in the
  folder it was last in for that app, Open and Save each, kept in
  Olo's config (`places`, beside the recent folders) and kept across
  restarts; the app's own `current_folder` only when there's none (a
  save of a file it names starts in that file's folder), and a
  remembered folder that's gone falls back to it. (Olo's own apps start
  their panels where they mean to, a document's own folder, so this is
  the portal's.)

- ~~**Dialogs float on top, never tucked into another window's group**~~
  (done 2026-11-02: it wasn't the group. Focus covered an alert naming
  no window, as an app's own panel, while its app's windows were
  covered: Passwords' passkey question over Chrome stayed hidden, with
  the keyboard. Such an alert now always shows, in the middle of the
  display the person is on, with the keyboard; Standard and Auto-Tiling
  already showed it over everything. Showing it found a Smithay bug
  that drew 60 frames a second under any window floating over Focus's,
  fixed) (Ben, 2026-11-02): "in focus mode (and maybe other modes too) when i
  am in chrome and a passkey dialog appears instead of appearing over
  chrome it appears over the passwords app and i don't even know it's
  there i have to go to passwords to even see it. dialogs like this
  should always open floating at the top layer". Why: the passkey
  prompt is a Passwords window (`olo.passwords`), and in Focus mode
  `shell::groups::group_key` groups every window of an app, so it
  becomes a tab of Passwords' group, behind wherever Passwords is. So:
  a dialog (a toplevel with a parent, a modal, an approval or request
  window an app shows for another, as Passwords' passkey and password
  prompts, polkit's, the portal's, Chrome's own) never groups or
  tiles, in any mode; it opens floating, in front of everything on the
  display the person is working on (centred over the window that
  asked, Chrome here, not over its own app's), gets the keyboard, and
  stays in front until answered. Apps say a window is a dialog through
  the kit (a window kind agents can read too), and app_server knows
  the rest by xdg's parent and fixed size. Check every mode (Focus,
  tiling, free), with the asking app on another display or workspace.

- ~~**Voice processing for every microphone**~~ (done 2026-11-02:
  a88b0abb, 9aae5937, cdd46746, 9368dfef, 1aaa7314, 8ba9f10b) (Ben,
  2026-11-02): "research what the SOTA approach to background noise
  filtering / echo cancellation etc. are for linux and support them too
  (for all mic sources)", vendored and owned, SIMD, real time, volume
  normalization on by default, a Calls and a Recording mode. `crates/voice`:
  echo cancellation of Olo's own (a fullband frequency-domain Kalman
  filter, delay found, two-path, residual suppression in the noise model's
  STFT; WebRTC's AEC3 measured and not used), DeepFilterNet3 (vendored,
  tract 0.23, fixed: silence skip, thresholds), WebRTC's AGC2 (vendored
  Sonora, turning loud talkers down too). `olo-voice --service`: a
  processed source in front of the chosen microphone, captured only while
  recorded from; Preferences ▸ Sound and the speaker's menu; supervised,
  upgraded in place, in the audio tier; Olo Doctor's realtime check and
  `--fix`; `tools/dev/pw-sandbox` to test sound apart from the person's.
- ~~**AirPods, natively**~~ (done 2026-11-02: a98ac95d, 858dd256,
  10e1070e, 6184b742, f0d67027, d9c70db6, ea5c0221) (Ben, 2026-11-01):
  "comprehensive with all features at the highest quality", Olo's own
  implementation (no LibrePods), on the MacBook and the ThinkPad.
  `apps/airpods` and `olo_system::airpods`: Apple's accessory protocol
  (levels, ear detection, noise control and Adaptive, Conversation
  Awareness, press-and-hold, renaming, every setting), their broadcast
  (levels and where they are, connect and switch automatically, never a
  tug of war); their own microphone as a wideband source (`olo-eld`, an
  AAC-ELD decoder in Rust) while music stays at AAC quality; Preferences
  ▸ AirPods, Bluetooth and Sound, Deskbar's tray and speaker menu, icons
  in every theme; panels taller than the screen scroll.

- ~~**Default apps and Open With**~~ (done 2026-11-02: the person's
  defaults and curated lists in `olo_xdg::open`; Open With ▸, Always
  Open With ▸ and Other… in the kit, `olo_ui::open_with`, on Tracker's
  items, the desktop's, Quick Look's and the file panels'; Preferences ▸
  Default Apps with its three tabs: defaults, the person's own kinds
  (`olo_xdg::mime::own_kinds`), and what Open With offers, in order) (Ben, 2026-11-01): "ability to choose
  the default app to open different file types with, both through a
  right click context menu and through a centralized default apps
  preference app. also, add an open with menu item when right clicking
  a file/folder that shows a curated set of apps appropriate for that
  file/folder and allow it to be customized in the preference app too".
  So: (1) a Preferences ▸ Default Apps panel listing kinds of file
  (grouped: documents, pictures, movies, music, archives, code, folders,
  links' schemes) with the app each opens in, changeable there; (2) an
  **Open With ▸** submenu on any file's or folder's context menu
  (Tracker, file panels, the desktop, Quick Look) offering a curated
  few apps that suit that kind (Olo's own first, then those whose
  desktop entries claim the MIME type, never a long raw list), with
  "Other…" (Olo's Open With window) and "Always Open With" making the
  choice the kind's default; (3) the curated list per kind editable in
  the same panel (add, remove, order). All in `olo_xdg::open` (the
  defaults already live in `olo-mimeapps.list`, so GIO, xdg-open and
  OpenURI follow), the submenu a kit menu every app shares, agent-
  readable like any menu. BeOS: FileTypes and Tracker's Open With.
  Then (Ben, 2026-11-02): "in the default apps prefs we should be able
  to set and manage custom extensions/types and their apps too". So:
  Default Apps lists the person's own kinds too (a name, the
  extensions it covers, the app it opens in), added, changed and
  removed there; an extension no MIME database knows becomes a type
  of Olo's own (`olo_xdg::mime` reads them, so Tracker, Open With and
  the opener know them), and a known type can be given another app.

- ~~**A window on another workspace has a blank thumbnail in the share
  picker**~~ (done 2026-11-02: drawn from what it last showed, with its
  display's frame; a share of it starts too, where app_server used to
  stop it, its size unknown) (found porting the portal, 2026-11-01;
  egui's the same): its capture never answers while it isn't shown. app_server could draw it
  from its last buffer (as a minimized window's Exposé picture), so it
  can be chosen by what it looks like.

- ~~**Tracker: Go to File or Folder**~~ (done: `goto.rs`, Go ▸ Go to File or Folder…, ⇧⌘G; the clipboard's path, completion, `go-to:<path>` for agents; the palette lists it with Tracker's menus, and offers "Go to <path>" itself for a path copied or typed) (Ben, 2026-11-02): "add a go to
  file/folder option in tracker with a keyboard shortcut (and it should
  also show up in command pallette) -- it's useful when i want to jump
  to a path i have in the clipboard". So: Go ▸ Go to File or Folder…
  (⇧⌘G, as Finder's Go to Folder), a small sheet with one path field,
  filled from the clipboard when it holds a path (absolute, `~/…`,
  `file://…`, quoted or with a trailing newline) and selected, so Return
  goes there at once; it completes as it's typed (Tab, the matches
  listed under it, kept as the work-follows-change rule says: read once
  per folder typed into), opens a folder in its window or the folder
  holding a file with the file selected, and says plainly when nothing
  is there. In the command palette as a Tracker command, and with a path
  in the clipboard the palette offers "Go to <path>" itself. Agents get
  it as a Tracker command (`go-to:<path>`) too.

- ~~**Command palette finds settings**~~ (done: HACKING, Preferences' "Every setting, findable" and the palette's Sources; `olo_ui::spotlight` lights it) (Ben, 2026-11-01): "in the
  super+space command palette thing also match on the actual settings
  etc. that are available in each preferences app to make them easy to
  find in there". So: every setting in every Preferences panel (its
  label, its box's title, words it's known by: "natural scrolling",
  "dark mode", "key repeat", "night shift") is a palette result, named
  with its panel ("Touchpad ▸ Pinch to zoom"), that opens the panel
  with the control shown and lit for a moment, as macOS's System
  Settings search does. The index is made from the panels themselves
  (form.rs records each control's label and box as it lays them out,
  so a new setting is found without listing it again; a test checks
  every panel's controls are in it), shipped with Preferences like
  `PREFERENCE_PANELS`, and agents can ask for the same.

- ~~**"gpui: window not found" from the palette and Deskbar**~~
  (29ce0b15: the platform window's callbacks finding the window gone
  say nothing): none logged since the 09:58 upgrade on 2026-11-02,
  where the palette, Deskbar and Terminal had logged 11 that morning.

- ~~**A tab clicked in a group flashes**~~ (fixed 2026-11-02, 83760a5d) (Ben, 2026-11-02): "sometimes
  when i'm in an app, e.g. terminal, and it has multiple grouped
  windows, if i click on the tab of another window in the group i get
  like a flash or something before seeing the other tab/window that is
  really jarring. i am in focus mode when i saw it but it might (idk)
  happen in other modes too." ~~Fixed~~: a window behind others in its
  group got frame callbacks once a second, so its tab clicked showed
  what it drew up to a second before; now it's told to draw at once and
  the group's old front stays until it has (`Decoration::fronting`).

- ~~**Menus over Deskbar**~~ (fixed 2026-11-02, c2f4a04e) (Ben, 2026-11-02): "if i am in an app and
  right click close to the deskbar and a menu opens it is under the
  deskbar -- it should always be on top". ~~Fixed~~: a window's menus
  (its popups) are drawn over the Top layer, Deskbar included, and take
  the pointer there (`menu_elements`, `window_menu_under`). And none is
  pushed away from the pointer by Deskbar: `unconstrain_popup` keeps
  menus on the display, not out of Deskbar's corner (checked
  2026-11-02: the desktop's menu, opened beside Deskbar, opens at the
  pointer over it).

- ~~**The keep-warm pixel redraws whole displays**~~ (done 2026-11-02:
  it stays in its place on a display without a full-screen window, only
  its commit changing: one frame a second damaging 1 pixel, from two
  full redraws of 7.7 Mpx at 2× on a 3456×2234 screen) (found porting Agent
  Screens, 2026-11-01): `render::keep_warm` puts a transparent pixel at
  the top of each display's elements for a frame every second (while
  the desktop was used in the last two minutes), and removes it again
  so a full-screen window can go straight to the display. smithay's
  damage tracker counts an element's place from the top of the stack,
  so the pixel coming and going moves every other element's place and
  damages it whole: two full redraws a second instead of a pixel
  (headless: the whole 1024 × 640 screen each time). Keep the warm work
  to one pixel's damage without blocking direct scanout (an element
  that keeps its place, or the damage of a changed place limited to
  what moved, in smithay's tracker), and measure on a display before
  and after (`timings`).

- ~~**The layout lint on GPUI**~~ (done 2026-11-02: `olo_ui::lint`,
  `cargo ui-lint`, run by install-session.sh; HACKING, "The layout
  lint": the kit's form and every Preferences panel and tab, light and
  dark at 1×, 1.5× and 2×, with long device names; it found Sound's and
  Display's menu fields running out of their windows and tabs' click
  areas overlapping, both fixed in the kit) (left when egui went, 2026-11-02): the
  egui kit ran a window's view headlessly at its size, light and dark at
  1×, 1.5× and 2×, and failed a test (and `install-session.sh`) on
  anything that ran past its area, controls over each other, labels
  cut short, and the layout review's errors (`ui_lint_*`, every
  Preferences panel with made-up long device names). On GPUI the review
  runs only on a live app (`tsession review`). Bring it back: a view
  drawn in GPUI's test platform with Olo's text system at a size and
  scale, the review's `Parts` and the kit's areas and controls checked,
  `cargo ui-lint` for every fixed-size window and dialog, the kit's own
  widgets clean and its sizes on the 2-point grid.

- ~~**Painting a glyph costs ~0.45 µs**~~ (done 2026-11-02, 7ccac1fb:
  a line keeps its glyphs' sprites as last painted, for its phase,
  scale and rendering, and paints them as a copy; gpui/README.md)
  (measured scrolling the
  Viewer's code view, 2026-11-01: 1,500 glyphs, 0.68 ms of a 2 ms
  frame; egui's frames scrolling the same were 1.2 ms on average,
  GPUI's 2.3). Each glyph looks up its raster bounds and its atlas
  tile (two locked maps) every frame; a shaped line could keep them,
  for its origin's subpixel phase, scale and rendering (the atlas's
  generation checked), and paint as a copy.

- ~~app_server: Viewer's first menu costs a 14–28 ms frame~~ (the
  dmabuf check worker's lock, above; 2026-11-02).

- ~~Tracker's scroll: a glide frame off, a flick's frames skipped~~
  (bench 2026-11-01): passes at every scale (2026-11-02: scroll p50
  2.12 / 1.99 / 1.20 ms at 1× / 1.5× / 2×, slowest 4.07 ms, glide
  within 1%); labels are kept by text, font, width and scale and what
  shows is laid out without copying entries. What's left of its frame
  is mostly painting text a glyph at a time (below).

- ~~**Wi-Fi that joins reliably, supervised and healed**~~ (done
  2026-11-01: WPA3-only networks joined with SAE; the brcmfmac firmware's
  stuck state found and healed by `olo-wifi-heal`, installed by Olo
  Doctor; see HACKING "The Wi-Fi healer". Still open: what exactly puts
  the firmware in that state; the healer's reports carry the times to
  match against the journal.) (Ben, 2026-10-31;
  after the move to GPUI is done): "something is wrong with the wifi- if
  i try to connect to the normal Home-WPA3 it just says network cannot
  be joined and if i try to connect to another like Home it says it
  is connecting but then fails to connect. idk if the driver is flaky or
  if it's something else but whatever it is please root cause it and make
  it so it will reliably work -- olo should supervise this stuff and
  fix/heal anything it needs to fix/heal (if it's something outside olo
  itself causing it)". Root-cause both failures (Olo's join path through
  NetworkManager in olo-system, the secrets agent, NetworkManager's and
  wpa_supplicant/iwd's logs, the brcmfmac driver and firmware on the
  Apple machine), fix what is Olo's, and have the supervisor watch Wi-Fi
  and heal what isn't (a stuck supplicant, a wedged driver, a stale
  connection profile), saying what it did.

- ~~**Supervisor priority tiers**~~ (done 2026-11-01, f4e2bd7f: tiers through rtkit, re-asserted by the supervisor) (Ben, 2026-11-01: "it should always
  give the highest priority to the core olo processes and any
  system+input processes important for responsiveness/smoothness,
  and only then should it raise the relative priority of the focused
  app vs other background processes"). apps/supervisor.

- ~~**From egui to Olo's own GPUI**~~ (done 2026-11-02: every app on Olo's GPUI, egui left the workspace) (Ben, 2026-10-31): "i think we need to
  rethink the decision to use egui -- it is not performant enough. …
  shift gears to do a full refactor to rip out egui and replace with
  gpui. vendor the gpui code into our repo -- we will own and change it
  as needed so that it becomes the olo gpui and we won't worry about
  keeping it up to date with upstream gpui we will just treat it as the
  starting point for us. we need to keep all the styling, behavior,
  features etc. that we've already built in olo (although where gpui can
  make it look and work even better we should do that we don't want to
  try to replicate stuff egui isn't doing right) and we need to make sure
  everything is super performant, minimum 120fps always, memory and cpu
  efficient (using gpu for as much as possible)." In stages, each
  leaving every app working:
  1. Vendor GPUI 0.2.2 (Apache-2.0) into `gpui/` as Olo's own: Wayland
     only (its X11 code and dependencies gone), no fetching images by URL
     (`http_client`), its `util` cut to what it uses (no git2), no
     serde_json `preserve_order` for the whole workspace; building in the
     workspace. *Done* (gpui/README.md).
  2. A GPUI window on Olo (blade on Vulkan, Honeykrisp): measured against
     egui drawing the same thing (frame time, CPU, memory, wakeups).
     *Frames done*: upstream drew and committed 60 times a second while
     idle (62 wakeups/s); Olo's draws only when something changed (idle:
     0%, 2 wakeups/s; an animation at the display's rate, 3.6%). The
     `blocking` crate's threads waking twice a second for good (under
     zbus and smol): *done*, patched (5d30fbc9).
     *Memory and threads done*: hello_world 153 → 59 MB RSS (egui
     DeskCalc: 80 MB), threads 20 → 8 when idle.
  3. The kit on GPUI: the platform (layer shell for Deskbar, popups for
     menus and tips, fractional scale, viewport, text input, the
     accessibility tree and the commands socket agents use, frame
     timings, the layout review), the style (palette, metrics, fonts,
     pixel snapping at every scale), then the widgets, documented in
     HACKING.
     *Begun* (olo-ui, HACKING "The GPUI kit"): run, windows, the theme,
     style primitives on device pixels, the agent tree and socket,
     frame stats, TextField, menus as popups, floating, upgrades. Next:
     the layout review for GPUI frames (`{"review":true}`), icons (the
     pre-scaled set, a texture pixel to a device pixel), the widgets
     (buttons, check boxes, radios, menu fields, sliders, BBox, string
     views, lists and tables, scroll views with the egui kit's eased
     wheel, tabs), the menu bar, keys from the keyboard profile, layer
     shell (Deskbar, the desktop, notifications, Screenshot's overlay),
     session lock, xdg-dialog alerts, xdg-foreign parents, idle
     inhibition, menus' scrolling and press-drag-release. (Done since:
     layer shell, the hints protocol by surface, menus' icons and key
     combos, drags out, idle inhibition.)
  4. The apps one by one, small first (DeskCalc, Screenshot, the lock
     screen, Preferences' panels) to the largest (Terminal, StyledEdit,
     Tracker), each checked at every scale and theme and benchmarked;
     egui leaves the workspace with the last. *Done*: DeskCalc (57 MB
     against 83 MB; 42 frames for 20 clicks against 210), the polkit
     agent, the lock screen, Supervisor, Token Pool, DiskUsage (ring
     slices as GPU sectors), Screenshot (and its region and colour
     pickers), DriveSetup, the command palette (with its window menu
     and shortcuts sheet; windows now share the GPU pipelines and the
     glyph atlas, so a window or menu opens without building them: the
     palette in 28 ms against egui's 47), Log Viewer (with the GPUI
     kit's search field, filter bar and text view), the Chrome app's
     installer, Deskbar (with the GPUI kit's drags out, tips in popups
     of their own, the wordmark, minimized windows in menus, menus put
     away by a press in their window, keyboard hints by surface; 57 MB
     against 141, 32 frames for 30 pointer moves against 150),
     Telegram (with the GPUI kit's multi-line text field, empty
     states, pills on baselines, child windows, activation tokens and
     agent nodes kept with cached views; its panes cached views of
     their own: 96 MB against 133, 8 frames for 30 pointer moves over
     messages against 157), the Viewer (with the GPUI kit's
     `ImageView`, olo-preview's GPUI views (`ui::PreviewView`,
     `RichView`, `TableView`), runs at sizes of their own, nearest
     sampling and keeping the screen awake; against egui on release at
     2×: 115 MB against 155 for a photo, 61 against 77 for Markdown, 64
     against 56 for a PDF; 1–3 frames for 30 pointer moves against
     ~156; scrolling's slowest frame 2.6–3.8 ms against 1.3–1.9, within
     a 120 Hz frame), Notifications (banners on a layer surface and the
     History window; with activation tokens from a click, a layer's
     keyboard changed as it shows, words that fit kept on their row
     (spaces hang past the edge), objects made for a window destroyed
     with it (a closed banner's fractional scale ended the app on a
     reload) and agents reading hidden windows as they are; on release
     at 2×: 61 MB against 153 with four banners, 71 against 108 with the
     History open, a banner in ~9 ms against 14–30 (the first, with the
     GPU device, 46 against 30), the History in 16 ms against 30,
     scrolling 200 notes 1.3–1.8 ms a frame against 3–5, nothing idle).
     The portal (its picker, permission, wallpaper, shortcuts and Open
     With dialogs, file and Print panels; with xdg-foreign parents, the
     GPUI kit's `ThumbnailPicker` and olo-print's GPUI panel; on release
     at 2×: a permission dialog in 7–13 ms against 17–34, 61 MB after ten
     of them against 91 and growing, 191 MB with the picker open against
     210 and 97–120 after against 172; the picker's thumbnails no longer
     fill the GPU's atlas). Agent Screens (with the GPUI kit's
     `Sidebar`, `TabView` and dashboard cards; Live takes a picture
     only when the screen draws something new, app_server's `Painted`
     event, and History plays back drawing only what changes; on
     release at 2×: watching a screen at rest 0.4% of a core and 10
     wakeups a second against 4.1% and 48, its app_server 0.5% against
     2.2%; an agent clicking four times a second 2.4% against 3.5–4.7%,
     each change shown as it comes rather than at the next of eight
     polls a second; playing a history back 3 frames a second and
     1.4–2.4% against 60 and 7–8%; 115–130 MB against 143). Activity
     Monitor (its window cached parts with its graphs on a layer of
     their own, the kit's `GraphLayer`; graphs' fills drawn by a new
     GPU primitive, `paint_area`, and all of a window's graphs moving
     on one clock; on release at 2×, its UI thread at or under egui's in
     every section with graphs, CPU 3.9% of a core against 5.2, Graphs
     2.2 against 2.9, the same frames a second, 93–96 MB against
     119–123; Processes 0.7% against 0.4: its frame for each sample is
     6 ms against 3.7). Passwords (its panes cached parts of its
     window, the kit's `part::Part`; a one-time code counting down draws
     its pane alone, once a second; no tick: one timer for the next
     thing due; a secret copied stays pasteable after Quit until it's
     cleared, the GPUI kit's clipboard linger, as the egui kit's apps;
     on release with debug assertions (so a test session can unlock it) at 2×: 0 at rest either way; a one-time code showing, 1 frame a second against 2; arrowing through the list 0.8% of a core against 1.5, typing a search 0.6 against 1.6, moving over fields 1.2 against 3.0, as it draws once for a change where egui drew four or five times; its frames 1.7–2.5 ms against 1.2–1.7; 90 MB against 124). The screen saver (every module on a `Canvas` of
     their own, olo-screensaver on GPUI; per 8 s at 1440×900 2×, CPU
     equal or less in all eleven, Lissart 3.2% of a core against 4.0,
     Spiral 9.8 against 12.6, StringTheme 3.8 against 4.0, Lens and
     Message within noise; 56–75 MB against 91–122; the benchmark's
     slowest frame 6.0 ms at 3456×2234 at every scale). Preferences
     (2026-11-02: every panel on `form.rs`, controls bound to their part
     of the settings; the kit's `ColorControl` and `pictures`; at 1× on
     a 3456×2234 screen: launch 42 ms against 81, idle 0 frames against
     2, the pointer swept over it 3 frames against 183, its slowest frame
     1.15 ms against 6.7, 0 failed). StyledEdit
     (with the GPUI kit's `editor`, `Ruler` and spelling; Page View,
     headers and footers, tables, pictures, Find, printing; and for it
     GPUI's kept glyph runs, hidden quads culled, a resize's swapchain
     with room to grow, spelling checked on a worker, paragraphs shaped
     once whatever their width; every step of the benchmark within a
     120 Hz frame at 1×, 1.5× and 2× on a 3456×2234 screen, 0 failed:
     scrolling's drag 0.93–1.05× the fingers and its glide within 1%
     (egui's 0.90–1.04 and 1%), the slowest frame 6.3 ms against egui's
     7.3 (its launch's 18–21 against 6–8), a live resize's 6.1 against
     5.0; and 3 frames for the pointer swept over it against 182, 18
     for its menus against 300, 59 typing against 218), Tracker (with the
     GPUI kit's workspaces (a folder's window on each workspace),
     tinted icons, GPUI's paste as a type and clipboard signal; the
     folder views, desktop, Get Info, Find, Tracker Status and Quick
     Look on olo-preview's GPUI views; drops taken by GPUI's own drop
     API), Preferences (the screen saver's preview with it), Terminal
     (with the GPUI kit's TabStrip, redesigned: a raised plate for the
     selected tab and a capsule of its colour round each group, nothing
     above the strip, tabs as wide as their titles need; its screens
     painted from cached rows of glyphs; olo-termd asleep when idle,
     its socket and the session's pid file followed with inotify;
     every frame within 2.7 ms at 1, 1.5 and 2× against egui's 6.5,
     0 frames idle and for the pointer swept over it against 4 and 182,
     85 for its menus against 303). *Done* 2026-11-02: egui left the
     workspace: the olo-kit crate, olo-print's and olo-screensavers'
     `egui` features, olo-preview's halved copies (made for egui's lack
     of mipmaps; Quick Look's edits and document pictures still made
     them), the vendored epaint, egui-wgpu and wgpu-hal patches, and 535
     lines of Cargo.lock; HACKING's "Writing an app" in olo-ui's terms.
     Left from it: the layout lint (below). GPU device loss came back
     on GPUI the same day (blade marks the device lost; the app starts
     anew with its state, as for the compositor gone).

- ~~**Telegram, as an Olo app**~~ (Ben, 2026-10-29; done: HACKING
  "Telegram", Preferences ▸ Integrations ▸ Telegram; the bench runs it
  with 3,000 chats, every frame under 6 ms. Not yet: recording voice
  notes (no Opus encoder in Rust), animated stickers move (their first
  frame shows), and signing in to a real account past the phone step is
  untried here, needing the person's own API ID): "please build an olo
  native telegram client that's an absolute pleasure to use and looks
  really great and supports all the core features of telegram". MTProto
  natively in Rust (a crate such as grammers; no Telegram Desktop or
  tdlib process): sign in with a phone code or QR and two-step
  password, the chat list with folders, pinned and archived chats,
  unread counts and search; messages with replies, forwards, edits,
  reactions, mentions, threads and topics; photos, video, voice notes,
  files and stickers (animated too); sending all of those, drafts and
  typing status; groups and channels; notifications through Olo's
  (with reply from the banner); contacts; secret storage of the session
  in Olo's keychain. Fast with thousands of chats (cached, streamed,
  never blocking), and Olo-looking: kit widgets, light, dark and night,
  every scale; agent-readable like every Olo app.

- ~~**olo-terminal uses far too much CPU**~~ (Ben, 2026-10-31; done:
  each row is compared with the cells it was drawn from and only rows
  that changed are rebuilt; a full-screen agent spinner frame 4.5 ms →
  1.2 ms whole, its grid 2.2 ms → 0.1 ms; the process 14% → under 4% of
  a core with ten windows and six spinners; hidden windows still draw
  once a second, under a millisecond): "same with
  olo-terminal: look at 4356 it is using wayu more cpu than it should".
  Measured (read-only, /proc and its `{"frames":true}`): 14% of a core on
  its UI thread, ~74 wakeups/s. Its window "olo — ◑ …" (a tab whose
  coding agent is working: the spinning title, the TUI's own spinner)
  drew 128 frames in 5 s, each ~4.5 ms (UI 2.5 ms, tessellating 1.4 ms,
  submitting 0.5 ms): the whole screen laid out and tessellated again for
  a spinner. A hidden window ("scraper") still draws once a second at
  5.5 ms. Wanted: single digits at most. Redraw only what changed (keep
  each row's shapes from frame to frame; repaint the title/tab spinner
  without the screen), draw a spinner no faster than it turns, and make
  a hidden window's once-a-second frame cheap or skip it.

- ~~**Agent rules: fast and light, measured**~~ (Ben, 2026-10-31; done:
  CLAUDE.md, HACKING "Fast and light", `tools/dev/cost`): once
  app_server's and Terminal's CPU are fixed, "add agent rules about this
  … so that moving forward everything is very carefully benchmarked and
  cpu cycles are never wasted and everything is ultra optimized. one of
  the core tenets of olo is that it is super fast and lightweight"
  (CLAUDE.md and HACKING.md: how to measure a change's CPU and frame
  cost, what budgets hold, and that a change isn't done until it's
  measured).

- ~~**app_server's idle CPU**~~ (Ben, 2026-10-31; done, measured
  headless; on the real displays to be seen after the upgrade: HACKING,
  rule 9's resting displays and "Fast and light"): every display looked
  for damage every frame forever (on a real one with a DRM test commit),
  and every app's kit threads took every event. Now displays rest after
  3 empty looks (every 100 ms, woken at once by commits on them, input,
  requests, window changes), the event loop's timeout is 100 ms, not
  16, and apps ask only for the events they use. Idle session: 10 looks
  a second instead of 61 (240 on two 120 Hz displays), 26 wakeups a
  second instead of 119, same scrolling latency. "i often see
  olo-app-server using 20%+ cpu. look at pid 3266 right now to see what
  it is doing and how it can be made much more efficient: it should
  never need to get above single digit cpu in a core." Measured
  (read-only, /proc): 21–26% of a core, all on the main thread, about
  half of it in the kernel (sys 13–14%), ~290–320 wakeups/s, with three
  displays (eDP-1 and DP-1 at 2×, DP-2 at 1×) and Terminal redrawing ~26
  frames/s for an agent's spinner (above) and Activity Monitor's live
  graphs open. So every client frame costs app_server several
  milliseconds. Find where (sampling its main thread), then: composite
  only the damaged region of only the display it's on, skip outputs
  with no damage, keep GPU work from re-uploading what didn't change,
  and see whether the keep-warm pixel (one frame a second per display
  while in use) and the GPU buffer ioctls (agx VM binds) are part of it.

- ~~**Deskbar still shows agent screens running after they've all
  stopped**~~ (Ben, 2026-10-31; fixed): "i have stopped all the agent
  screens but in the deskpad it still shows the icon that indicates
  agent screens are running". Cause: Deskbar's 30-second re-count (for a
  screen killed without a word) held the status lock while re-reading,
  which takes it again: a deadlock the first time it ran with a screen
  up, after which nothing in the tray changed (agent screens, the bell's
  count, Upgrade, battery, Wi-Fi, sharing); since fc8fd47 (2026-10-30).
  Seen in the running Deskbar:
  both status threads waiting on the lock. The lock is let go first now.

- ~~**StyledEdit, much more featureful**~~ (done 2026-10-30: every feature below; HACKING, "StyledEdit") (Ben, 2026-10-29, after
  everything else): "please improve StyledEdit to be much more featureful
  -- it doesn't need all the bells and whistles of, say, modern Word or
  Excel, but maybe something like ClarisWorks / Gobe Productive". After
  Gobe Productive (BeOS's own office suite) and ClarisWorks: a word
  processor first (paragraph styles, rulers with tabs and indents,
  lists, tables, pictures, headers and footers, page view and printing
  to PDF, find and replace, spelling, word count), opening and saving
  the formats people have (RTF, Markdown, DOCX, ODT, HTML) natively.
  And (Ben): "as you build out new widgets/components/patterns etc for
  anything that is reusable they should go into the kit so we can use in
  other apps -- use it as an opportunity to enhance the kit too": rulers,
  style menus, tables, page views and the like are kit widgets,
  documented in HACKING.
  Done so far (2026-10-30): paragraph styles (Title, Heading 1–3,
  Quote, Code, Body), bulleted and numbered lists that nest and number
  themselves, indent levels and alignment per paragraph (the Paragraph
  menu; Tab, ⇧Tab, Return and Backspace in lists), a word count, all
  saved in the file's attributes; the kit's editor sets each paragraph's
  indents, spacing and marker (`editor::Paragraph`); File ▸ Import… for
  Markdown, Word and OpenDocument, and Export as any of those or a web
  page (`olo-richtext`, links kept; checked in LibreOffice); spelling as
  you type with suggestions (the kit's `spell`, Hunspell dictionaries);
  printing and PDF (`olo-print`: the Print panel, CUPS over IPP, PDF by
  krilla, pages laid out as the editor lays them out); RTF in and out
  (`olo-richtext`'s `rtf.rs`, checked both ways with LibreOffice);
  tables in `olo-richtext` (Word, OpenDocument and RTF, read and written,
  checked in LibreOffice), and Quick Look reading Word, OpenDocument and
  now RTF through it; tables in the editor (the kit editor's rows, the
  Table menu, in and out of every format, printed), column widths
  dragged by their rules; a ruler (the kit's `ruler`: indents and tab
  stops, left, centre, right and decimal, which epaint now lays out),
  kept in every format; headers and footers (Document ▸ Header &
  Footer…, left, centre and right parts with the page number and count,
  printed, and kept in Word, OpenDocument and RTF); pictures (characters
  in the text the kit's editor draws: Insert Picture…, pasted or
  dropped, sized from the Edit menu, kept beside the document, and in
  Word, OpenDocument, RTF, web pages, Markdown and print); Page View
  (sheets of the paper it prints on, breaking as printing does, headers
  and footers on them; the kit editor's `Options::pages`).

- ~~**app_server: a slow frame now and then in the bench's live resize**~~
  (found 2026-10-30, at 1.5× and 2×; done 2026-10-30): 15–48 ms once in
  a run, in `render_output`. Timed by part (app_server now logs a slow
  frame's parts, and which window or layer took the time): it was never
  drawing but gathering the elements, and two things in it: a new
  wallpaper's 30 MB texture uploaded whole in the first frame to show
  it (20–24 ms), and a window's tab redrawn by cosmic-text when its look
  changed (4–13 ms: a face's first use, glyphs at a new size, its lock).
  The wallpaper is now strips of about 4 MB, a new one uploaded a strip
  a frame with the old one up meanwhile; a changed tab is drawn on a
  worker, the old one shown till it's done. The bench (Activity Monitor
  and Log Viewer at 1.5× and 2×): 0 of 36 steps fail, where 2–3 did
  every run.

- ~~**Scrolling feels the same everywhere**~~ (app_server `scroll.rs`: Chromium ×1/12 a touchpad, ×0.4 a wheel; GTK 4 ×0.4 a touchpad; GTK 3 by the window's height (it scrolls by its view's, ^⅔), 94–97% of Olo's distance; measured headless) (Ben): in Chrome scrolling is
  far too fast (maybe GTK apps too). A wheel notch or touchpad swipe
  should move Chrome and GTK apps as far as it moves an Olo app, with
  the same smooth scrolling (app_server's axis values, value120 and
  discrete steps, and what each toolkit multiplies them by).

- ~~**A group taller than its display, from a member's least size**~~
  (found by wm-fuzz, seed 3 steps 90 and 118, two displays 1600×1000
  and 1280×800 at 2×; also seed 1 step 229). Done: it wasn't a least
  size. wm-fuzz's step 90 sizes a Terminal group by hand 23 px taller
  than its 384-high display, which is the person's to do and stays. At
  step 118 a Tracker put in that group, asked for 247×407 but unable to
  be narrower than 250, drew 250×407; `keep_on_screen` took that as the
  app sizing itself and fitted the whole of it to the screen, height
  too, and the group followed. Now it fits only the sides the app chose
  (a side taken as asked is Olo's), so the group grows to 250 wide and
  stays as tall. wm-fuzz's Cmd+Tab and ⌘-drag-in checks also let a
  window coming into a group grow it to its least size, as group_window
  did (seed 3 step 244: a Tracker back from hidden into a Terminal group
  sized 151 high meanwhile; seed 2 step 50), and a tiled group may grow
  inside its tile area (seed 1 step 229: a right-hand tile grows
  leftwards). And the case the TODO was about, for tiles (seed 2 step
  134): a tiled Terminal grown to a Tracker's least size by grouping
  stayed that size after the Tracker was taken out, bigger than its
  tile, until a zoom round trip put it back. A tiled window or group now
  goes back to its tile's size as soon as the window that needed more
  leaves (`back_to_tile`); wm-fuzz lets it.

- ~~**Bug: after an upgrade, an auto-tiled window under Deskbar**~~ (Ben,
  2026-10-30; fixed): "after an olo hot upgrade i sometimes see windows, e.g.
  terminal, in auto tile mode opens back up such that it extends further
  horizontally beyond the deskbar and is under it. if i click to another
  window then back it adjust it to fit properly." The tiles stop beside
  Deskbar's bar on their display, and after an upgrade that bar came
  late or on the wrong display: a display coming back wasn't told to
  Deskbar until the power tick happened to (up to 5 s), and Deskbar's
  bars, reopened by name before the kit knew the displays, all went on
  the main display. So the tiles were laid out without Deskbar there.
  Now app_server tells of a display as it comes, the kit opens a layer
  naming a display once that display is known, and app_server keeps a
  layer that comes before any display instead of closing it (Deskbar
  and the desktop quit over that in a session whose displays come
  late). Reproduced headless (three displays coming late, a Terminal and
  a Tracker group tiled: wide every upgrade, until 5 s on or for good);
  with the fixes every upgrade brings them back beside Deskbar.

- ~~**Bug: after an upgrade, ⌘Space's palette opens on the first display**~~
  (Ben, 2026-10-30; fixed): "every time after a hot upgrade the cmd+space
  pallette seems to open on d0". The resident palette was first started
  by ⌘Space as `olo-palette --toggle`, and an upgrade re-executes an app
  with its first arguments: it opened itself as it came back, before
  any window had the focus again, so on the first display. Restored
  now, it leaves its first launch's asks alone (`save_state`). Seen
  headless after an upgrade (palette on D0 by itself, the focused window
  on D1); with the fix nothing opens, and ⌘Space opens it on D1.

- ~~**Bug: an app at a whole core after an upgrade**~~ (Ben, 2026-10-30; fixed):
  "the olo supervisor has detected that olo-passwords looks stuck using
  98% of a core for over 5mins". Its window was back on a hidden
  workspace at another size: the kit's swapchain was to be made to fit
  1.5 s on, but only at a frame, and a hidden window's frame callback
  never came, so the loop was woken for it at once, again and again.
  Now that waits for the frame (`next_timeout`), and a window with
  nothing to draw into isn't asked again at once either. Reproduced
  headless (a window shrunk, then sent to another workspace: 99% before,
  0% after).

- ~~**Bug: scrolling a window that isn't active**~~ (Ben, 2026-10-30; fixed, to try after the upgrade):
  "some/most apps don't let me scroll their scrollable areas unless
  they're active. all scrollable areas should be scrollable even if
  their window isn't the active one". The wheel and the touchpad scroll
  whatever scrollable area is under the pointer, in any window, active
  or not (and without making it active), as macOS and BeOS do: Olo's
  kit apps, other Wayland programs (Chrome, GTK, Qt), app_server's own
  views. Find where it's lost (app_server's pointer axis routing, the
  kit taking axis events only with keyboard focus, egui's hover state
  in an unfocused window) and check it for every kit scroll view (lists,
  text views, cards, Tracker, Terminal, Activity Monitor, Preferences)
  at every scale.
  Ben, live: Activity Monitor scrolled, Agent Screens and Token Pool
  didn't, until a click on that display; StyledEdit didn't while Slack
  did; then an inactive Tracker on the active display didn't. Cause:
  windows that hadn't drawn lately (any display) were taken for not
  shown after each frame with nothing new (the DRM backend counted
  nothing as presented), so Smithay sent their frame callbacks once a
  second, and Olo's apps scroll in their frames; Activity Monitor
  redraws on its own and Chromium draws on its own clock. Now only a
  drawn frame changes which screen shows a window. Also: a motion gave
  the pointer to the surface under where it was before the motion.
  Headless sessions render every frame whole, so they couldn't show it:
  checked there that every kit view scrolls inactive (wheel and
  touchpad, on either display, mixed scales).

- ~~**Bug: Quick Look's full screen spans displays**~~ (Ben, 2026-10-30; fixed):
  "there's a bug with quick look when i have multiple displays i hit
  space and correctly see the quick look preview then i hit f and it
  went to a different display well actually it kinda spanned displays
  -- it should fullscreen it on only the one active display". With
  several displays, Quick Look (Space) shows on the right one, but F
  (full screen) moves it to another or stretches it across displays.
  It must fill only the display it's on (the active one, where the
  person is looking), and come back from full screen to where it was.
  Find the cause (what Quick Look asks for, and how app_server takes a
  full-screen or zoom request with more than one display) and test it
  with a headless two-display session at every scale.
  Cause: app_server took the size from the display the window touched
  first but filled the space's first display with it, and the other
  displays still drew the window where it was, now display-sized, over
  their edge. Now it fills the display the window is on (else the
  pointer's), and only that one draws it. Before and after journalled
  (two headless displays; 1×, 1.5×, 2×).

- ~~**Terminal: coding agents keep their tab's title current**~~ (Ben,
  2026-10-30; done: HACKING, Terminal ▸ Coding agents' tab titles): "terminal app should also make available to claude code
  and codex coding agents a tool to update the terminal title and inject
  instructions for them to keep the terminal title updated at all times
  to ensure it reflects the active work". A Claude Code or Codex started
  in a Terminal tab gets a tool to set that tab's title (the tab only,
  not the window's or another tab's; through the agents' own tool
  mechanisms, an MCP server of Terminal's given to the agent the way the
  group context is, not by editing the person's files beyond what that
  needs), and instructions in its context to keep the title saying what
  it is working on now, updated as the work changes. The title the agent
  sets is shown as a tab title the person set is (and the person's own
  rename wins until they clear it); readable and settable by agents
  outside the tab too (Terminal's widget tree and commands). A setting
  in Preferences ▸ Terminal, beside the group context's.
  Done so: `set_tab_title` from an MCP server of Terminal's
  (`olo-terminal --mcp`), named in Claude Code's `.claude.json` (and
  allowed in its settings) and Codex's `config.toml` while the setting
  is on; the instructions come from the same hook as the group context
  (at the start, and a reminder line with each prompt). There is no
  person's rename of a single tab to lose to it (the person names
  groups); outside the tab, `olo-terminal --set-tab-title SHELL TITLE`.
  Tested with Claude Code 2.1.314 (`claude mcp list`: connected) and
  Codex 0.166 (`codex mcp list`), and the tool driven from a tab.

- ~~**Preferences ▸ Performance: a supervisor that keeps the desktop
  responsive**~~ (Ben, 2026-10-30; done: HACKING, The supervisor): "a new preferences -> performance app,
  and the first setting should be one to do smart process prioritization
  to ensure user responsiveness. the idea is that olo will run a
  supervisor/watchdog that monitors processes and ensures that stuff like
  compilers (e.g. rustc) or other background processes have low priotity
  and that higher priority is given to the active window app in the
  desktop, and absolute highest should be olo itself and the app server
  etc. so that responsiveness is always optimal. and the supervisor
  should watch for stuck processes that seem to be in a cpu spin loop for
  a long time, or zombie processes, or processes that appear to have
  memory leaks or are using such a large amount of memory that it is
  impacting overall system performance or stability. everything that
  this supervisor/watchdog does should be surfaced in activity monitor,
  and if there is an urgent thing like stuck processes or processes
  eating up all the memory then it should show the user a warning dialog
  with a button to kill the processes if they want and it should also
  have a button to go to the detail in activity monitor to see exactly
  what the process is doing and the evidence we have. there should also
  be an option to ignore a process so we don't keep getting warnings
  about it if it's not an issue. make sure in the preferences that any
  constants used in the hueristics/algorithm for detecting this stuff
  are tuneable, but also make sure that our detection algorithms are
  very smart/advanced don't just rely on one number/signal to detect
  these things. this feature should default to off for now, and there
  should be a tray icon to toggle it and right clicking the tray icon
  should have options to go to details in activity monitor."
  - **Where it runs**: a supervisor of olo-session's (a thread, or a
    small `olo-supervisor` it starts and keeps), off by default
    (Preferences ▸ Performance, a new panel, and a tray icon in Deskbar:
    a click turns it on or off; its menu: Show in Activity Monitor,
    what it has done lately, Performance Preferences…). Only the
    person's own processes unless Activity Monitor's full access is
    given; nothing it changes outlives it (priorities put back when
    it's turned off or exits).
  - **Priorities**: Olo itself highest (app_server, olo-session,
    wlproxy, Deskbar, the portal, input), then the active window's app
    and what it started (by its process tree and its scope, `scope.rs`),
    then the other apps on screen, then background work lowest:
    builds and compilers (rustc, cargo, cc, ld, clang, go, javac, node
    bundlers…), indexers and batch jobs, known by name, by their parent
    (a terminal's job not in front) and by behaviour (long CPU-bound
    runs with no window). Through CPU weight and `nice`/`ioprio`
    (`SCHED_IDLE`/`SCHED_BATCH` for builds), or cgroup `cpu.weight` and
    `io.weight` on the apps' scopes where the session's systemd allows;
    following focus changes within a frame of them, without blocking
    anything.
  - **Watchdog, from several signals each, not one number**: a stuck
    spin loop (high CPU for long with no progress: no syscalls or I/O,
    the same stack or instruction pointer sampled, no window updates or
    an unresponsive window that doesn't answer pings); zombies (and the
    parent not reaping them, for how long); memory leaks (resident and
    anonymous memory growing steadily over a long window by a trend fit,
    not one jump, against its own history, with swap and page-fault
    rates); memory that threatens the system (its share of RAM, the
    system's memory pressure, PSI stalls, swap thrash, how close to the
    OOM killer). Each finding carries its evidence (the samples, the
    trend, the pressure) and a confidence.
  - **Tunable**: every threshold and window in those heuristics in
    Preferences ▸ Performance (with sensible defaults and a reset), and
    an ignore list (processes, by name or command).
  - **Surfaced**: everything it does and finds in Activity Monitor (a
    Supervisor section and each process's inspector: its priority and
    why, its findings and their evidence, over time); an urgent finding
    (stuck, eating memory) asks in an Olo alert naming the process and
    why, with Stop Process, Show in Activity Monitor and Ignore This
    Process. Agents read and drive all of it (the panel, the tray, the
    alert, Activity Monitor).
  - **Done so**: `olo-supervisor`, a service of the real session, idle
    until turned on. Priorities by nice, `SCHED_BATCH` and idle I/O on
    every thread (unprivileged: the session's processes are all in one
    systemd scope, so no cgroup weights; `SCHED_IDLE` can't be undone
    without privileges, so batch); the active window's app's tree and
    Olo's own never lowered. The watchdog's signals as above, short of
    stack sampling (needs ptrace) and window pings; every threshold in
    the panel. Activity Monitor: a Supervisor section and an inspector
    tab. Agents: `olo-supervisor --status --json`, `--on`, `--off`.
    Found and fixed on the way: an alert of a program started again
    was put where the last one had been (on a screen of another size,
    off centre) rather than in the middle; the kit's single-instance
    socket (and other per-display files) broke when `WAYLAND_DISPLAY`
    named its socket by full path; alerts with long button labels ran
    off their left edge (`WindowOptions::alert_buttons`).

- ~~**Token Pool: restart Codex's service seamlessly, holding new requests**~~
  (Ben, 2026-10-30; done: HACKING, "Token Pool" ▸ Clients and Upgrades of
  the pool itself): "when token pool daemon thing restarts we recently
  changed it so that it doesn't wait for a quiet period in codex/claude.
  instead, let's do it smarter: make it so that when a restart is
  happening we still accept new requests but then we just have them wait
  as if they're waiting on the server and let the ones already in-flight
  finish, then seamlessly do the restart and then process the new ones
  that came in for real." A restart (the pool's settings changed under a
  running Codex service, or Restart in Clients) drains rather than cuts:
  - Requests already under way run to their end (their streams too).
  - Requests that come meanwhile are taken and held, the client seeing
    a request still being answered (no error, no retry storm), up to a
    limit of time after which they go ahead as before.
  - Once nothing is under way, the service restarts onto the pool's
    settings, and what was held goes on, in order, as if it had just
    been slow.
  Codex's windows talk to its service over its socket, and the service's
  own requests go out to OpenAI: holding at both ends (the pool's proxy
  for what reaches it; between Codex's windows and its service, a
  stand-in on its socket for the moment of the restart) so a turn going
  on is never cut and none is lost. Claude Code's requests, through the
  pool already, are held the same way while the pool itself restarts
  (an upgrade). Shown in Clients (Restarting: n under way, n waiting),
  for agents too.
  Done differently where that is better: a Codex service holds its
  turns' state, so restarting it in the middle of one can't be made
  seamless (the turn goes with it). Instead it no longer needs
  restarting: unchecking a client only pauses it, still pointed at the
  pool's proxy, which decides request by request whether the pool
  serves it or passes it through on the client's own sign-in, so
  turning the pool off or on for Claude Code or Codex takes effect from
  its next request with nothing restarted or cut (a restart remains
  only when Codex is first pointed at the proxy, or taken off it with
  `olo-pool --put-back`). And the pool's own restarts (an upgrade) are
  now exactly as asked: new connections wait in the queue, what is
  under way finishes, the new version takes the listening sockets over
  and answers what waited (`drain.rs`; `/pool/status`'s `drain`).

- ~~**Terminal: Tile All Tabs, rows' heights dragged**~~ (Ben, 2026-10-30; done: HACKING, Terminal ▸ Tile All Tabs):
  "in terminal tile mode with multiple groups let me drag the group
  divider/header up and down to give a group more or less vertical space
  relative to the other groups. if i right click the divider/group
  header then there should be an option to reset group heights (if they
  have been changed)". In Tile All Tabs with several groups' rows, the
  divider between two rows (its group's header) drags up and down, the
  rows on either side growing and shrinking (each down to a least height
  that still shows a line of text), the panes reflowing live within a
  frame; the shares are kept per group (by its name, so a row keeps its
  share when others come and go), saved with the window and restored
  after a reload. The header's context menu gets Reset Row Heights
  (enabled once they've been changed) to share the height evenly again.
  Agents can read the shares and set them (the strip's info, and a
  command). Done: each divider is a `Splitter` ("Row Height: GROUP",
  its share as its value) agents drag; Reset Row Heights is a command
  (`tile-reset-heights`). Found on the way and fixed: after any popup
  menu closed (Escape or an item chosen), the window didn't get the
  keyboard back (app_server couldn't find a gone popup's window, and
  smithay's grab ignored the focus change); and a click on Deskbar went
  through to the desktop under it, which took the keyboard from the
  window being typed into (Deskbar's menus too).

- ~~**Agent Screens: a revamp, so it doesn't look like an afterthought**~~
  (Ben, 2026-10-30; done: HACKING, "Agent Screens"): "in the agent screens app the ux is all messed up
  and the scrollbar in history actions doesn't work properly, also the
  actions don't seem to update in real time, we should have filters, and
  the ux just generally needs revamp/polish so that it's not looking like
  an afterthought".
  1. *The History list's scroll bar* works as every other list's does
     (drag the knob, click the track, the wheel, kept in range as the
     list grows; following the newest action when at the bottom).
  2. *Live:* a screen's actions appear in History as they happen (and
     the stats with them), not only when the window is reopened or the
     screen chosen again.
  3. *Filters:* by kind (Pointer, Keys, Typing, Menus, Programs, Windows,
     Reads), by result (failed only), by time, with a search field, as
     Log Viewer filters; the count of shown and all.
  4. *A revamp:* the window laid out as a real app, live view, the
     screens' list, History, Replay and Stats each with its own place,
     sized and spaced from `metrics`, every state (no screens, a stopped
     screen, an empty history) designed, checked with `tsession review`
     at 1×, 1.5× and 2× in light, dark and night.
  Done: the scroll bar was pinned because the list was asked every frame
  to reveal the playhead's row, its context reaching past the list's
  end (the kit's `ColumnListView::reveal` now stops at the ends, and
  History reveals a row only when it is new); a reveal or scroll to rows
  just added drifted back, egui springing back from beyond last frame's
  bounds (the kit's `ScrollView` now goes there over two frames). The
  history is followed as it is written (`history::Reader`,
  `record::index_known`), with filters (kind, result, time, search;
  picked within 2 ms a frame), a sidebar of running and stopped screens,
  a header, Live, History and Stats tabs, and a detail of each action.

- ~~**Notifications: one of the same, not several**~~ (Ben, 2026-10-30):
  "add a preferences -> notifications setting, enabled by default, that
  dedupes notifications so that if we get >1 active notification from
  the same app with the same text/content then we just show the latest
  one and drop the earlier one". Preferences ▸ Notifications, on by
  default: a notification from an app arriving while one of the same app
  with the same title and body is still active (a banner showing, or
  unread in the history) replaces it: the new one shown (and kept), the
  earlier one taken away, as if it had been updated in place (its count
  in Deskbar's dot unchanged, its fade starting anew). Off: each kept,
  as now. Done (HACKING, Preferences ▸ Notifications): "Only the latest
  of the same notification", in the History box; pinned ones stay.

- ~~**Terminal: tell coding agents about the other repos in their tab
  group**~~ (Ben, 2026-10-30): "add a preferences -> terminal setting for
  injecting context to claude code and codex coding agents to tell them
  about worktrees/repos that are in the same terminal group. useful when
  using a group per feature and a feature spans repos b/c that way the
  coding agent knows about all the repos. it shouldn't make assumptions
  about what they are, just add a block to the coding agent context that
  it is part of a terminal group named '...' and inside that group are
  the following repos (including its own): ...". Preferences ▸ Terminal
  gets "Tell coding agents about the repositories in their tab group"
  (off by default). When on, a Claude Code or Codex started in a tab of
  a named group gets a block in its context saying only that: it is in
  Terminal group "NAME", and the group's tabs are in these repositories
  and worktrees (each one's path, with its branch; its own marked),
  nothing assumed about what they are for. Given through the agents'
  own context mechanisms without editing the person's files (Claude
  Code: `--append-system-prompt`, or a session-start hook in a settings
  layer of Olo's; Codex: its `-c` instructions or developer message),
  kept current as tabs join or leave the group (on the agent's next
  turn where it can, else its next start), and readable by agents
  (Terminal's widget tree says it's on). Done (HACKING, Terminal ▸
  Coding agents' context) through the agents' hooks, which both take a
  hook's output as context (`SessionStart`, and `UserPromptSubmit` when
  the group changed): the one place it can't avoid the person's files,
  so the hook is put in Claude Code's `settings.json` and Codex's
  `hooks.json` only while the setting is on, everything else in them
  left as it was, and taken out when it's off. Codex asks once to trust
  it (`/hooks`). Told only when the group's tabs are in more than one
  place. (Tested with a sandboxed Codex 0.166, which ran it and passed
  the session's folder, and with Terminal's own groups.)

- ~~**Terminal: "Tile All Tabs", a row per group**~~ (Ben, 2026-10-30):
  "change how the show all tabs option in terminal works. first, rename
  it to "Tile All Tabs" and then i want you to make it so that each group
  gets a row and then each tab within that group is a col within that
  row." The command is renamed Tile All Tabs (menu, keys, agents'
  command id kept working under its old name too); it lays the window's
  tabs out as a grid where each group is a row (in the strip's order,
  its name and colour heading it) and each of its tabs a column in it,
  ungrouped tabs a row of their own; rows share the height and each
  row's columns its width, every pane live and typed into as before,
  and the same command (or picking a pane) goes back to one tab. Done
  (HACKING, Terminal ▸ Tile All Tabs; the command id stays `tile-tabs`,
  ⌘⇧D); with no groups, the grid as before.

- ~~**Terminal: Tile All Tabs, choosing which groups by their headers**~~
  (Ben, 2026-10-30): "make it so that in terminal in tile all mode, if i
  left click on a specific group header it actually makes that group
  selected (similar visual style to a selected workspace in deskbar) and
  then when it is selected it only tiles the tabs for that group (using
  a full grid not just one row). if i left click it again then it should
  go back to showing all the groups. and if i left click another group it
  should add that group too as a second row and so then e.g. each group
  gets half of it (and if you were to select all groups then basically
  it'd just be like none were selected and you'd see them all). and if i
  right click on a group header then i should get the context menu that i
  currently get with a left click." In Tile All Tabs, a click on a group's
  header selects it (lit as Deskbar's showing workspace is, in the
  group's colour) and toggles it: with one selected only its tabs tile,
  as a full grid; with several, a row each, sharing the height; none (or
  all) selected, every group shows. The header's right button opens the
  menu its left button opens now. Agents read which are selected and
  choose through the headers. Done (the kit's `TabStrip::group_selection`,
  for the side strip too; the chosen groups kept with the window).

- ~~**Deskbar: every display's workspaces on every Deskbar; the display
  in the tray**~~ (Ben, 2026-10-30): "the deskbar on all displays should
  show the display-workspaces for all displays not just the current
  display. and, let's also show the display id/number in the deskbar
  tray with like a little monitor icon and the display number in it, and
  clicking on it gives you the option to switch to a different display".
  This replaces the earlier rule (each Deskbar listing only its own
  display's workspaces): each Deskbar lists every display's workspaces
  and their apps, its own display's first ("D0 Workspace 1" …), the
  others' as they are on their displays (clicking one goes there, on
  that display). In the tray, a small monitor icon with the display's
  number in it (drawn at every scale, in the tray's style); clicking it
  lists the displays (their names and numbers, the pointer's marked) and
  choosing one moves the pointer and focus there (its front window, else
  its desktop). Agents read the display's number and choose through the
  menu. Done so (HACKING, Deskbar and multiple displays): `Request::FocusDisplay` goes to a
  display; the icon shows with more than one display, its menu listing
  each by number and name.

- ~~**Move to Workspace: every display's workspaces, from a window's tab
  and from Deskbar's apps**~~ (Ben, 2026-10-30): "when right clicking a
  window title bar, under move to workspace it only shows the current
  display's workspaces. it should show all the workspaces for all the
  displays as targets to move to just like in the deskbar when you right
  click a workspace group title and get the option to move all windows
  to ->. and, speaking of the deskbar, when right clicking a running app
  in the deskbar all the move window options should be available in
  there too for that app." The tab's menu's Move to Workspace lists each
  display's workspaces ("D0 Workspace 1" …, as Deskbar names them; the
  window's own display first), moving the window to that display and
  workspace; an app's menu in Deskbar gets the same moves for all its
  windows (Move to Workspace ▸ every display's, Move to Display, and the
  window moves the tab's menu has), done with `MoveToWorkspace` and the
  display moves app_server already has. Done so (HACKING, "The window
  menu" and Workspaces): the tab's Move to Workspace lists every
  display's, left to right as Deskbar's heading menus do, its own
  display's with their keys and a check; `MoveToWorkspace` takes an
  `output` (to the same place there, tiled or zoomed as it was, hidden
  if that workspace isn't showing); an app's Deskbar menu has Move (All)
  to Workspace ▸ every display's and (All) to the Next / Previous
  Display. And a bug found doing it: after the tab's menu was put away
  with Escape, the next click (Deskbar's, say) went nowhere. The menu
  asked for its grab while the right button was still down, with no
  press of its own to name, and was refused, so it ran without one and
  its clear layer outlived it, taking that click; it now takes the
  pointer from the press, and a menu's window gets a frame as the menu
  goes (it draws on frames, which a still screen doesn't make).

- ~~**Links from a shell: Chrome's "zygote" errors on the terminal**~~
  (Ben, 2026-10-30): `pulumi whoami`'s browser sign-in printed "Opening in
  existing browser session." then "Failed sending zygote boot message:
  Broken pipe" (it worked). xdg-open runs `olo-chrome URL`, which started
  a second Chrome only to hand the link to the running one, and Chrome
  prints those as that one goes. Now `olo-chrome` hands it over itself
  through Chrome's singleton socket (HACKING, "Google Chrome"): nothing
  printed, the page open in a millisecond, its window forward.

- ~~**Token Pool: Codex still not through the pool; its service's
  details and Restart; activity over time**~~ (Ben, 2026-10-30): "codex
  still doesn't seem to be going through token pooler, only claude is.
  you can look at the codex session i ahve running now and it's not going
  through the token pool. pls fix." · "you can just update the daemon it
  doesn't need to wait for non busy" · "also add a way to restart the
  daemons and see details about them in the token pool app itself too" ·
  "add a slick chart or something too that shows activity over time".
  The cause: the pool found Codex's daemon (started the day before, on
  the settings from before) but waited for it to be between turns, and
  Ben's Codex was in one agent turn for hours, all of it straight to
  OpenAI. Restarted his daemon then (Codex's own `daemon restart`, as
  the pool does; his window reconnected and carried on, now through the
  pool), and the pool now restarts a stale daemon at once. Done
  (HACKING, "Token Pool" ▸ The Clients window): Pool ▸ Clients (⌘L, or
  the Clients button) shows the activity over time (requests a minute
  by client over the last hour, six hours or day, kept a day), Codex's
  service (version, since, settings, where its connections go, its
  conversations) with Restart, and each Claude Code and Codex running
  and where its connections go; `olo-pool --clients [--json]` and
  `--restart-codex-service` for agents.

- ~~**Bug: upgrades still move windows; restore an exact snapshot**~~
  (Ben, 2026-10-30): "when I upgrade it is still messing up my windows
  and moving them around on the displays and the sizes and everything. i
  think we need a more reliable approach to ensuring that windows stay
  where they are on upgrades: we need to be able to save the current
  state/location/size/mode/etc of every display-workspace then do the
  upgrade then restore everything back to that exact state instead of
  trying to rely on the logic to place everything back where it was
  before." Before the upgrade, app_server writes one snapshot of the
  whole desktop: every display (its mode, scale, position) and each of
  its workspaces (its windowing mode, auto-tiling tree, what shows), and
  every window exactly (display, workspace, frame, content size, zoomed,
  rolled, minimized, tile and group with its tab order, stacking order,
  focus). After it, as each window comes back it is put exactly there
  and asked for exactly that size, with nothing else placing it
  (auto-tiling, cascades, groups' placing and fitting held off until all
  have come back or a deadline passes); whatever still differs once
  they're back is set from the snapshot again, and the result is
  compared with the snapshot and any difference logged (and checked by
  `tools/dev/upgrade-check` with several displays, every windowing
  mode, groups and auto-tiling). Done (HACKING, upgrade-check): the log
  of Ben's 09:18 upgrade showed the windows reconnecting before DRM had
  found the displays (0.4–0.9 s later); reproduced headless
  (`OLO_HEADLESS_LATE_MS`): windows on a display not found yet were put
  on another, and auto-tiled ones swapped tiles (the trees of displays
  not found yet were dropped and made afresh). Now a returning window
  waits for its display, those displays' trees are kept, and what came
  back is checked against what was saved and put back where it differs
  (logged). Two displays, both auto-tiled, a floating window and one on
  another workspace: 6 upgrades and 2 reloads unchanged (they swapped
  in 1 of 2 before), and an ordinary session's 3 unchanged.

- ~~**New windows always become the active window; an optional glow**~~
  (Ben, 2026-10-30): "please make sure that any time a new window opens
  for any reason it becomes the active window with focus. also, add a
  setting in preferences -> appearance to give new windows a subtle glow
  for n seconds (default off, and default seconds when on should be 5s).
  the glow needs to work with the active theme and day/night mode and
  look really great and be fairly subtle but enough to draw attention to
  the window." Every new toplevel (Olo's apps, other programs, dialogs
  and alerts, windows opened in the background by a program or a
  portal) is raised and given the keyboard focus as it first shows, on
  the display and workspace it opens on (switching to them if it opens
  elsewhere), checked across every way windows open (restored after an
  upgrade excepted: those come back as they were). Preferences ▸
  Appearance gets "Glow new windows for [5] seconds" (off by default; a
  number of seconds when on, 5 by default): app_server draws a soft
  outer glow around the new window's frame in the look's accent (its
  own for each look, and for day and night), fading out over the last
  second, drawn at device pixels at every scale (1× to 2×, 1.25× and
  1.75× too), costing nothing once it has gone, and said in the
  window's info for agents. Done (HACKING, "New windows"): focus held in
  every case tried (other programs and background launches, another
  workspace's app, auto-tiling, tab groups; wm-fuzz checks it each
  step); the glow checked at 1× and 2×, light and dark.

- ~~**Notifications preferences: banners, sounds and history for all apps
  at once**~~ (Ben, 2026-10-30): "the notifications preferences already
  have the ability to turn on/off banners/sounds/history for each app
  but there should also be an option to do it for all apps". Above the
  apps table in Preferences ▸ Notifications, an "All Apps" row with the
  same three checkboxes: checking or unchecking one sets that column for
  every app; each shows mixed (–) while the apps differ. Applies to apps
  seen later too only if the person asks for it (the default for new
  apps stays as now). Readable and settable by agents through the
  panel's widget tree. Done (HACKING, Notifications; the kit's
  `CheckBox::mixed`); the panel is taller for it (and the Deskbar box).
  Also fixed: a scroll view squeezed below 18 points crashed its app
  (the knob's least length passed the track's).

- ~~**Deskbar: an app's notification dot drains with its notifications**~~
  (Ben, 2026-10-30): first "the blue notifications dot for an app in the
  deskbar should fade like the notifications in notifications app as
  that apps notifications fade out (based on the settings) -- make it so
  that like when all of that apps notifications are shown with 100%
  visibility still then the blue dot is 100% and then as each individual
  notification starts to fade that takes the relevant strength out ...
  based on the fraction of that notification ... make sure there's a
  min/floor ... where it still looks good like a dot", then instead:
  "instead of changing the opacity based on the notifications that are
  fading let's have the amount of the blue dot that's filled in change,
  filling from the left to the right so it drains as the apps
  notifications drain". The dot is filled from its left edge to a
  fraction of its width: the mean of the app's notifications' strengths
  (`NotificationSettings::strength`, from the fade settings: each 1/n of
  it), so it is full while none has begun to fade and its right side
  empties as each fades; the empty part is the dot's outline (or a faint
  well) in the same colour, so it always reads as a dot, never a mark,
  with a floor to the fill where it still reads as one (chosen by eye,
  in every look and at every scale, its edge snapped to device pixels).
  With fading off, full. Updated as the notifications age (no faster
  than the fill can show a change), and the fraction in the entry's
  status for agents. Done (HACKING, Deskbar): floor 0.3, the well at
  28% of the dot's colour; checked at 1×, 1.5× and 2×, light and dark.

- ~~**Deskbar: the notification count beside the dot, as an option**~~
  (Ben, 2026-10-30): "a preferences option in notifications to also
  include the notification count next to the blue dot. off by default
  but enableable. make sure you qa visually at all display scales to
  ensure it looks good and fits well". Preferences ▸ Notifications gets
  "Show the number of notifications in Deskbar" (off by default, in
  `NotificationSettings`); when on, the app's entry shows its count of
  notifications beside the dot, in the dot's colour and fading with it,
  in the kit's small type, fitting the entry at every width (a long
  name is cut before the count is), beside the dot as it drains,
  shaking with the dot when one
  arrives, and in the entry's status for agents. Checked by eye and with
  `tsession review` at 1×, 1.25×, 1.5×, 1.75× and 2×, in light, dark and
  night, with counts of 1, 9, 10 and 99+. Done (HACKING, Deskbar):
  Preferences ▸ Notifications ▸ Deskbar; checked with 2 and 99+ at 1×,
  1.5× and 2×, light and dark.

- ~~**Bug: Empty Trash from the desktop's Trash icon didn't empty it**~~
  (Ben, 2026-10-30): "right clicking on trash on desktop and choosing
  empty trash didn't seem to work so i opened trash (still saw the
  folder in there to delete) then did empty trash again from the
  tracker menu and it worked", then "i think it was b/c there was a
  confirmation dialog it opened that i didn't see. make sure all dialogs
  like that get centered on the screen and on the top/front layer".
  Done (HACKING, alerts): an alert with no parent (asked from the
  desktop, a menu, Deskbar) opened in the next cascade slot, the top
  left of a display, unseen; now it opens in the middle of the display
  the pointer is on (kept off Deskbar's column), in front and focused,
  and stays over its app's windows until answered. One with a parent
  still opens over it.

- ~~**wm-fuzz: failures now and then**~~ (found 2026-10-30, from a loop
  over seeds 3, 3, 3, 5, 1, 2, 4, 6 at 300 steps each; done 2026-10-30:
  seeds 1–7 pass all 300 steps, each from a fresh session). They were a
  checker that couldn't tell a slow client from a wrong frame, and five
  real bugs:
  - The checker waited a quarter-second of quiet; a Terminal busy for
    longer looked wrong. Now `WindowInfo::settling` says a window hasn't
    drawn the size it was asked, and wm-fuzz (and `olo-screen wait
    --idle`) wait for none to be; a failure prints every frame of the
    step (`--trail` for steps that pass).
  - Tiling a grouped Terminal moved it at once but kept its old size till
    it drew, so for a frame its middle was on the other display and
    per-display grouping took it out of its group; drawn, it rejoined
    and took the group's frame (seed 1 step 7). A window's display is
    now where it is heading (`heading_frame`).
  - Back from another workspace, each window of a group was placed by its
    own least size before the groups were made again, then pulled to the
    group's (seed 4's flickers): least sizes count the mates a window
    is coming back with (`mates_to_be`).
  - A zoomed window that joined a group on another display came back
    unzoomed, as `Arrangement` kept tile, part and axis but not zoom
    (seed 6 step 116); it keeps zoom and the frames Restore and unzoom go
    back to now.
  - A window that joined a group, then was zoomed into a frame that
    happened to be the group's, "restored" its old arrangement on its own
    again (seed 6 step 202): untouched means the group's frame and
    arrangement both (`Joined::untouched`).
  - Placing a window (`Request::Place`) from behind its group's front
    didn't take the group (it hadn't drawn at the group's size), and
    Restore afterwards took it back to its frame from before it was
    tiled (seed 1 step 210): a placed window leads (`Groups::lead`) and
    its placed frame is its own (`forget_own_frame`).

- ~~**Activity Monitor, revamped**~~ (Ben, 2026-10-29, at the end):
  "revamp activity monitor to include everything a nerd could ever want
  to know, with live views, drill downs, tuning settings and ability to
  take actions etc. it should look really slick / aesthetically
  pleasing, be extremely fast/performant/buttery smooth and extremely
  powerful. if needed it should just ask for elevated perms on startup to
  be able to show everything." Processes (tree, threads, open files and
  sockets, memory maps, environment, cgroups, I/O, priorities; signal,
  renice, pin to cores, limits), CPU (per core, frequencies, C-states,
  governors), memory and swap (zram, pressure, page cache), disks (per
  device and process I/O, latency, SMART), network (per interface and
  connection, per process), GPU, power and thermals (sensors, fans,
  battery), the kernel's pressure stall information and tunables; live
  graphs at 120 Hz, history, drill-down from any graph to who causes
  it. Read natively from /proc, /sys, netlink and the like (no ps, top
  or lsof); elevated access through Olo's polkit agent, asked once at
  start, for what needs it. Reusable graphs and inspectors go in the
  kit.
  Done (2026-10-30): the sections, the inspector, full access through
  pkexec (the sampler runs as root; only in the person's own session),
  idle states, NVMe health (read with full access: not yet seen on this
  machine as root), looking back from any graph; the kit's `Sidebar`,
  `dashboard` and `Graph`; graphs repaint a device pixel at a time (3.5%
  of a core for the Overview's six); every frame within 8.3 ms in the
  bench at 1×, 1.5× and 2× (the kit's lists got faster for it: one draw
  for a list's text, a per-frame budget for new cells); each process's
  network traffic (netlink `sock_diag` with TCP's byte counts, summed by
  the sockets' owners: Sent and Received columns, "Programs Using the
  Network"), and connections and interfaces read by netlink rather than
  /proc/net (done 2026-10-30, `netdiag.rs`).

- ~~**wm-fuzz seeds 3 and 5**~~ (found 2026-10-30; seed 5's also on the
  app_server before the fixes above). Seeds reproduce only from a fresh,
  empty `--home` (a used one restores windows and changes the path).
  Fixed (3a8568a): a display back from mirroring kept its closed Deskbar
  bars, empty, in its layer map, which moved its tiling area over (seed
  5 step 213's window came back off the display); and a window its app
  keeps bigger than its tile jumped and came back each time it was put
  back. Then the rest (seed 3 passes 300 steps three runs in three, seed
  5 its 300):
  - seed 5 step 213: a window sent to another display that joins its
    app's group there by itself goes back as the round trip while it is
    still as it joined (the trip notes the join, `note_joined`), with
    the arrangement it left with (its tile was the group's).
  - seed 5 step 246 and seed 3 step 268: a window was a member of its
    group only if it was in that same group on that same display before
    (`was_member` by the group's number, `seen_on` by display), so one
    moved to another display joins a group there rather than leading it
    to its own frame.
  - seed 3 step 271: the checker allowed for the group growing to its new
    window's least size (`least` in `WindowInfo`, the smallest frame its
    app allows, tab and border included).
  - seed 3 step 243: a window straddling two displays (its middle on
    neither) groups on the display of the lowest-numbered window among it,
    its group and the window it was put with (`group_display`), so a
    group's windows aren't split by a few points of size.
  Run: `tsession start NAME 2560x1600,1920x1080 --session --shell --home
  EMPTY` (`TSESSION_PROFILE=release`), `wm-fuzz NAME --seed N --steps 300
  --keep-going`.

- ~~**Agent Screens: each screen's history, replays, videos and stats**~~
  (Ben): Agent Screens keeps, for each screen, what was done on it:
  every action (`click`, `type`, `key`, `invoke`, `drag`, `run`, … with
  what it matched and where, timed, from the requests app_server already
  gets) beside the frames the screen drew, only where they changed.
  1. *History:* a screen's actions as a list in the app (filter, search,
     select one to see the screen as it was then).
  2. *Replay:* plays it back, the frames in time with the actions, the
     pointer, clicks and keys shown; play/pause, speed, scrubbing,
     stepping action by action.
  3. *Export as MP4:* encoded in process in Rust (no ffmpeg), on a
     worker with progress and Cancel. Stretches where nothing happened
     are cut, and a card in the video says how long each was ("4 min
     12 s with nothing happening").
  4. *Stats:* per screen, when it was started and last used, how long
     it has run and been active, actions of each kind, the programs run
     in it, the windows open now; the screens list can sort by last use.
  Agents can read all of it too (`olo-screen history NAME [--json]`,
  `olo-screen stats [NAME]`). The history is kept within limits on
  size and age (Preferences), and outlives the screen until it is
  cleared.
  So far (2026-10-30): the actions' history and the stats, for agents
  (`olo-screen history` with `--kind`, `--search`, `--last`, `--clear`;
  `olo-screen stats`; HACKING, "Agent Screens"), kept under 8 MB, and
  the frames beside them (`record.rs`, changed tiles only;
  `history NAME --look N`), and History and Replay in the window
  (`replay.rs`), and MP4 export for agents (`history NAME --export`,
  `export.rs`), and Export Video… in the window (a worker with progress
  and Cancel), the stats in the window and the list by last use
  (stopped screens with a history too), and the limits in Preferences ▸
  AI (size and age).

- ~~**Deskbar: a dot for apps with notifications**~~ (Ben, 2026-10-29): "in
  the deskbar we should show a notification badge for each app that has
  active notifications. we don't need a count, just a dot on the right
  and it should be themed and not red/alert/problem colored but more
  notification colored i.e. not in your face but just letting you know
  it's there. and when a new notification arrives for that app have the
  dot bounce side to side subtly to indicate a new one arrived". A small
  dot at the right of the app's Deskbar entry while it has notifications
  in the history unread, in the palette's quiet accent (not failure red);
  a new one arriving shakes it gently side to side (a few frames, eased,
  at 120 Hz; not while Do Not Disturb hides banners, or quieter);
  agent-readable (the entry's status says "has notifications"), every
  scale and look.
  Done (HACKING, Deskbar): matched by the notes' desktop entry, else
  name; checked at 1× to 2× (1.25× and 1.75× too), light and dark, and
  under Do Not Disturb (no shake).

- ~~**Token Pool: requests fail through it; make it truly transparent**~~
  (Ben, 2026-10-29): "please take a look at token pooler logs and figure
  out why requests started failing- i turned off token pooler for claude
  and then those same accounts worked fine going directly so something is
  broken with token pooler. please make sure that the proxy is truly
  transparent and acts exactly the same to claude code. actually what
  would be even better is if we can change it so that it doesn't put
  claude code into api mode with a custom endpoint but instead just makes
  the network data go through our local transparent proxy in a way where
  claude code just thinks it's using the normal subscription requests not
  a custom endpoint. same for codex." First the failures: the pool's
  logs (`olo-logs --print --program olo-pool`) against the same accounts
  used directly, request by request (headers, body, streaming, retries,
  token refresh), until the pool's requests are byte-for-byte what Claude
  Code and Codex send themselves. Then: no custom endpoint or API mode;
  Claude Code and Codex run as on a subscription, and their traffic to
  Anthropic's and OpenAI's hosts goes through a local transparent proxy
  (an `HTTPS_PROXY` they honour, or Olo's own network layer), which
  picks the account and passes everything else through untouched, with
  the logs and heatmap as now.
  Done (HACKING, "Token Pool"). The failures: account f919e666 on
  2026-10-29 13:05–13:13, requests getting no answer at all (status 0,
  28–116 s, no headers), while the quota reads hit 429 every minute: the
  pool read each account's usage report twice a minute, and was itself
  rate limited for it (now every five minutes, backing off on a 429).
  And the proxy: Claude Code and Codex on their own subscriptions with
  `HTTPS_PROXY` to the pool, which opens only their own connections to
  the two hosts (from /proc), serves the model requests from the pool
  and passes everything else through byte for byte; tested with the real
  clients (`tests/clients.rs`).

- ~~**wm-fuzz seed 2, step 192**~~ (found 2026-10-29, also before today's
  changes): a tiled Tracker window drawn as the bottom-right quarter is
  refitted to the whole bottom half when its workspace is shown again
  (`tools/dev/wm-fuzz NAME --steps 192 --seed 2` on a fresh
  `2560x1600,1920x1080 --session --shell` session with a fresh home):
  its tile state and its frame part ways somewhere earlier (grouping,
  arranging, a display moved); find where.
  Done (seed 2 now passes 400 steps with `--tiles`, a new check that
  every tiled window is in its tile's part, which `list_windows` now
  gives as `part`): a hidden window passed over by ⌘` joined its old
  group for a moment and kept the group's tile when it went back to its
  own frame (its own tile is kept with its frame now); Tile All's
  window, still at its old size, was pushed inside the tiling area by
  that size and stayed there (`keep_in_tile_area` waits for the size
  asked); a group's back tab took a text-size change late, when the
  group came apart (`behind` in `WindowInfo`: the kit counts it not
  shown); and undoing an arrangement or unzooming pulled a window left
  hanging off an edge back in (`put_back_at`: back exactly while some
  of its top edge is on a display).

- ~~**Quick Look: TOML highlighted**~~ (Ben, 2026-10-30): "add support for
  toml syntax highlighting to quick look". The bundled grammars (syntect's
  defaults) have no TOML (or INI, which `.toml`, `.conf`, `.desktop` and
  `.service` were sent to, so they show plain): grammars of our own for
  them, built into the preview's syntax set, coloured as the others.
  Done so (`crates/preview/src/syntaxes`): TOML (every kind of value,
  arrays and inline tables over lines, Cargo.lock and the like too) and
  INI (systemd units, .desktop entries, git and editor configs).

- ~~**Terminal's tabs and groups look like Deskbar's list**~~ (Ben,
  2026-10-29): "can you make the terminal tabs styled more like the olo
  deskpad tabs that style looks much nicer than the almost childish look
  of the tabs/groups in terminal right now." Down the side: groups as
  Deskbar's workspace headings (bold, a band, no pill), tabs as its rows
  (icon, title, the one showing in its band), quiet; along the top, the
  same restraint.
  Done so: down the side, Deskbar's rows on the panel's ground with a
  hairline under each, the one showing recessed, groups under small bold
  headings with a dot of their colour; along the top, chips are a dot
  and a quiet bold name instead of a pill. `tsession review` clean at
  1×–2× in every layout, light, dark and night looked at. Then (Ben,
  with a picture: "the pr icons seem to be a little bit too big, can we
  make them a touch smaller and just generally polish up how that
  vertical tab looks"): a tab's pull request mark is as big as the text
  (12 pt, its lines whole pixels at every scale) in the close box's
  place, which takes it over under the pointer, and each group's
  colour bar stops short of the next's.

- ~~**Auto-tiling finds an arrangement where every window fits**~~ (Ben,
  2026-10-29): "can you work on the auto tiling algorithm it currently
  sometimes overlaps windows that would have all fit if the arrangement
  was different we are not finding the optimal solution right now".
  Done so (HACKING, auto-tiling): where the tiles can't give every
  window its least size, `autofit::refit` turns the fewest splits, else
  searches every slicing arrangement (shape curves: any grouping up to
  8 tiles, runs of their order beyond) for one where all fit, else
  takes the one that overlaps least. Five windows on 1440×900 that
  overlapped three ways now all fit.

- ~~**Show All Tabs: each pane's head in its group's colour**~~ (Ben,
  2026-10-29): "can you make the tile headers follow their groups color
  scheme so we can visually see whats what".
  Done so: a pane's head is tinted with its group's colour, a 2-point
  line of it across the top.

- ~~**Tracker: selection as in modern file managers**~~ (Ben, 2026-10-29):
  "in tracker i should be able to click a file/item, hold down shift then
  click above/below it to select all of them and any other common
  selection/movement/action behaviors and patterns in modern file
  managers". Shift-click ranges, ⌘-click to add or take one away,
  ⇧-arrows to extend, ⌘A, a rubber band in every view, type to select,
  and the rest Finder and Nautilus do.
  Done so: `ListState` has the selection every view shares (click,
  ⌘/⇧/⌘⇧-click, ⇧ with the arrows, Page keys, Home and End growing and
  shrinking from the anchor, bands that add with ⇧ and flip with ⌘ and
  scroll on), `widgets::type_select` for typing (Find, the file panel
  and every Tracker view); a plain click on one of several leaves it
  alone; Enter on several opens them all; Open Parent selects the folder
  it came from. Checked by driving each view in a test session.

- ~~**Passwords: passkeys, and filling the right account in Chrome**~~
  (Ben, 2026-10-29; next, after Terminal's git work: "this is really
  really important"): "olo passwords needs to support passkeys with a
  scan qr code feature and also we need to do a better job at matching
  the correct accounts in chrome so that we look at all the available
  information (domain, current user profile/account, and text matches on
  the screen to an email/username we know about) and then in the right
  click menu there should be an "Autofill with Passwords (<account
  username/email>)" option and then the "Passwords" entry that shows all
  the other possible matches. we need to be able to reliably determine
  and then remember which e.g. google account to use b/c i have like 6
  google accounts and it'll be a different one for each chrome profile.
  and we need to do a better job at finding the correct inputs to fill.
  and it needs passkey integration so that it supplies the passkey if we
  have one and if not then it can advertise that it can do one and should
  integrate fully with everything so that it's just as seamless as
  possible and everything just works".
  - Passkeys: Passwords keeps passkeys (WebAuthn credentials, made and
    used natively: P-256 keys, attestation "none", sign counts) and
    syncs 1Password's; Chrome gets them through Olo's extension as the
    page's WebAuthn provider (`navigator.credentials.create`/`get`
    answered by Passwords, so a site offering passkeys offers Olo's,
    and one with a passkey there signs in with it); and "scan a QR
    code": a phone's passkey through caBLE / hybrid transport (the QR
    code a site shows, and Olo's own for a phone to use Olo's
    passkeys), over Bluetooth natively.
  - The right account: rank the candidates by everything known — the
    page's domain (and its related ones), the Chrome profile (its
    signed-in account, its name), emails and usernames on the page
    (an account chooser, a "signed in as"), what was filled there
    before in this profile — and remember the choice per profile and
    site, so each of six Google accounts goes with its own profile.
  - The context menu: "Autofill with Passwords (ben@…)" for the best
    match, then "Passwords ▸" with every other candidate.
  - The fields: find the username, email, password, new-password and
    one-time-code inputs reliably (autocomplete attributes, labels,
    names, types, forms split over steps, shadow DOM and iframes), and
    fill them as typing does, so sites' scripts see it.
  Done: the right account, the menu and the fields (HACKING,
  "Passwords in Chrome"): ranked by what the page shows, what was used
  there before in this profile (an autofill or a sign-in by hand), the
  profile's own account, then the site; Chrome puts the two items under
  Olo ▸ (an extension can't have two at the top). Checked in a test
  Chrome: the menu, a two-step sign-in filled through, a sign-in by
  hand as another account remembered for the profile. Passkeys
  (HACKING, "Passkeys"): Passwords is the page's passkey authenticator
  (sites see one, and offer passkeys); it makes them (an alert in
  front: Use Another Device… / Cancel / Save Passkey, into the
  account's login) and signs in with them (an alert, a choice of
  account when there are several), suggests them in a sign-in's field
  menu (Olo ▸ Sign In with Passkey), hands over to Chrome's own sheet,
  with its QR code for a phone's passkey and security keys, when it has
  none or the person chooses another device, takes its alert down when
  the page gives up, and asks for an unlock only when it has a passkey
  for the site. Checked in a test Chrome with a page verifying as a
  site does (client data, site hash, flags, attestation, the signature
  with WebCrypto): making two, signing in with each, Cancel, abort,
  locked both ways, the field menu, Chrome's QR sheet. Not done: Olo's
  passkeys used from another computer (Olo playing the phone, scanning
  that computer's QR code with a camera); a phone's passkeys in Olo
  work through Chrome's sheet.

- ~~**Passwords search and filters; rich search and filtering controls in
  the kit, for Logs too**~~ (Ben, 2026-10-29, while on Passwords): "while
  you're doing stuff with passwords can you also make the passwords
  search work better and add filtering. and add a full set of rich
  search and filtering reusable controls we can use in the log viewer
  too b/c right now the log viewer filtering/search is a mess".
  Done so far: `olo_kit::search` (HACKING, "Searching and filtering"):
  the query language, ranking, highlighting, `SearchField` with
  suggestions and `FilterBar`; Passwords uses them (every item's
  username and non-secret text searched, filters for tags, kinds,
  vaults, sites, users, one-time codes, attachments, weak and reused
  passwords, best matches first, matches marked). The Log Viewer too:
  one search with Program (several, or all, or left out), Level and
  Time as menus beside it, `pid:`, words marked in the messages, and
  `olo-logs --print --query` taking the same text. Review clean at 1×
  to 2× for both, wide and narrow.

- ~~**Drag a window into another's tab bar to make it a tab**~~ (Ben,
  2026-10-29; done: HACKING, the kit's `TabStrip` and app_server's window
  moves): "when there are apps with multiple windows and they also
  support tabs we should be able to drag the window back into the tab
  bar area to have it go back to being a tab instead of a window, and
  the opposite we should be able to drag a tab out into being a window",
  and "we should be able to drag a group out to make that whole group a
  new window with those tabs". Done: a tab or group out into a window
  (the kit's `TearOff`, `TearOffGroup`, `WindowOptions::under_pointer`).
  To do: a window (by its tab, which app_server moves) let go over
  another window of the same app's tab strip joins it there as a tab
  (or its tabs, its groups kept), and a tab dragged out of one window's
  strip onto another's goes into it: app_server tells the app where a
  window of its own was let go (over which of its windows, where), the
  kit says where its strips are, and the strip shows where it would go
  while the window is over it.
  Done so: a tab or group pulled out of the strip is a window under the
  pointer that moves with it (`Cx::begin_move`); a window moved over
  another of its app's goes see-through and that window's strip marks
  where it would go; let go there, its tabs (and groups) go in, and it
  closes. Checked in a test session: a tab out and back, a lone window
  dropped by its tab, a group out and back after another group.

- ~~**Terminal: several groups, tabs moved between them, groups
  reordered**~~ (Ben, 2026-10-29; done: HACKING, Terminal's Groups and
  the kit's `TabStrip`): "in terminal make sure that i can
  actually create multiple groups, move tabs between groups, reorder
  groups, and so on. because last time i tried (which was before your git
  worktree stuff) it didn't work." Today a group is a whole window (one
  name and colour, Tabs ▸ Group), and a tab can only go to a new window.
  Make groups runs of tabs in a window's strip, as a browser's: each led
  by its chip (name, colour); a tab's menu has Add to Group ▸ (New
  Group…, each group) and Remove from Group; dragging a tab into a run
  joins it, out of it leaves it; dragging a chip moves its whole group
  (reordering groups); a chip's menu renames, recolours, saves, ungroups,
  closes or moves the group to a new window; tabs and groups dragged
  onto another Terminal window's strip move there. In the kit's
  `TabStrip`, for other apps; repositories' own groups (repos.rs),
  saved groups, `save_state` and agents (the tree) follow. Done as
  written, except dragging onto another window's strip (next item);
  meanwhile a tab's Move to Window ▸ moves it to another window. Also
  done from the next item: a tab or group dragged out of the strip
  becomes a window under the pointer. Checked by drags in a test
  session, at 1×–2× (and 1.25×, 1.75×), light and dark, along the top
  and down the side, and through an upgrade.

- ~~**Terminal: ⌘-click opens any URL or file, reliably**~~ (Ben,
  2026-10-29, bug; done: HACKING, Terminal's Links): "in terminal we should be able to super+click any
  url and it should open in the default browser, or any local file and
  it should open in tracker. it should be able to figure out relative
  paths too based on the cwd in the terminal"; and, told it has them
  (`links.rs`: ⌘-hover underlines, ⌘-click opens; URLs by pattern, paths
  that exist relative to the foreground program's directory): "if it
  already has them then it doesn't reliably work". Find every way it
  misses (programs reporting the mouse, as Claude Code does; links
  wrapped over lines; paths in a TUI's boxes or quotes; the working
  directory of a program that changed its own; the key under each
  keyboard profile) and fix them; files open in Tracker, shown.
  Found and fixed: a ⌘-click on a Terminal that wasn't focused only
  focused it (the link was looked up on frames already knowing ⌘); a
  click used the previous frame's link; long URLs Claude Code wraps
  itself opened cut short; a Ctrl-click went to programs taking the
  mouse; files opened in their app. Checked with a real Claude Code
  reply: a URL, a long one it wrapped (from either piece), `path:line`
  and a plain path, at 1×, 1.5× and 2×.

- ~~**Window tabs: titles of unfocused windows update late**~~ (Ben,
  2026-10-29, bug; done: HACKING, `Cx::set_title`): "yellow tabs that are not focused don't have their
  titles (specifically i'm looking at agent status icons) updated very
  frequently it seems like they do update but not very often". A window's
  title (a Terminal's agent status icon in it) must show in its tab the
  moment it changes, focused or not. Cause: app_server lets a window
  whose content isn't shown (behind another in its tab group, covered)
  draw once a second, and Terminal set its window's title only while
  drawing. Now it sets it the moment a tab is retitled: a window behind
  another in its group showed 4 of 30 titles in 3 s, now all 30 (1×,
  1.5×, 2×).

- ~~**Kit apps survive the GPU device being lost**~~ (2026-10-29, from
  the upgrade checks; Ben: "never leave a bug like that as a todo not
  done"; done: HACKING, "The session and in-place reload"): once in ~60
  test upgrades on Asahi, a restart made Mesa's Vulkan WSI fail
  (`DRM_IOCTL_SYNCOBJ_TRANSFER failed`, then `VK_ERROR_DEVICE_LOST`), and
  Deskbar panicked in egui-wgpu (`egui_uniform_buffer` invalid). Now the
  device-lost callback, or a panic in GPU work on any thread, makes the
  app start itself anew with its saved state, as an upgrade does, and
  its windows come back exactly: place, size, tile, workspace, stacking,
  and which window has the keyboard (app_server no longer gives a window
  back on its own the focus it didn't have, or puts it on top; Deskbar
  keeps a gone app's place in its list). Pixel-identical at 1×, 1.5× and
  2× for StyledEdit (a tab group), Tracker and Deskbar; losing it again
  within 10 s ends the app instead of looping. Tested with the kit
  socket's debug-only `{"lose_gpu":true}`.

- ~~**Windows never move in an upgrade**~~ (Ben, 2026-10-29: "can you
  make sure when i do an olo upgrade my windows don't change positions or
  sizes or workspaces or displays or anything even if auto tiling is
  enabled? right now they still sometimes change"; done: HACKING, "The
  session and in-place reload", auto-tiling, and `tools/dev/upgrade-check`).
  Measured with the upgrade's own order (every app re-executing as
  app_server restarts), which the old reload-only tests missed: the
  auto-tiling trees are now kept across the restart (ratios as laid out,
  each tile naming its window's saved place), parked windows keep their
  display and minimized state, a hand's float choice is kept, kit apps no
  longer die in the race (texture deltas kept, unconfigured surfaces
  never drawn on, the handshake retried), GTK 4 windows tile, and
  displays changing scale keep their windows. 40+ upgrades exact at 1×,
  1.25×, 1.5× and 2× with two displays, hidden auto-tiled workspaces,
  minimized tiles, GTK windows in tiles and windows hanging over onto
  another display. The first upgrade from an app_server before this one
  can still move auto-tiled windows once (it keeps no trees).

- ~~**Terminal: git repos, worktrees and GitHub**~~ (Ben; done: HACKING,
  Terminal "Repositories"; Preferences ▸ Terminal), native (no `git`
  or `gh` subprocesses):
  - a tab in a git repo goes into a group named after the repo on its
    own;
  - the group's context menu manages the repo: its worktrees (every one
    found, a click opens it in a new tab in the group) and New
    Worktree…, which asks for a name, makes it and opens it in a new
    tab;
  - Preferences ▸ Terminal gets a worktrees location for new ones,
    `~/worktrees/[repo-name]/[worktree-name]` by default;
  - a tab in a worktree has Rename Worktree… and Remove Worktree… in
    its context menu;
  - GitHub: with an open PR for the branch, the tab's menu has an item
    to go to it, and the tab shows the PR's status with an icon.

- ~~**Multiple monitors, managed like macOS Displays.**~~ (done: the last
  clause, the pointer crossing at matching scale, holds because displays
  are arranged in one logical space, each scale already in it, and the
  pointer crosses wherever screens touch, `clamp_to_screens` with tests;
  each display has its own workspaces and Deskbar too) Hotplug of external
  displays (USB-C/Thunderbolt on the M2), each with its own resolution,
  refresh rate, scale factor (2× default on HiDPI, 1×/1.5× etc. on
  others) and rotation; mirror or extend; choose the main display (where
  Deskbar lives). Preferences ▸ Displays shows the screens as proportional
  rectangles you drag to arrange and align (edges snap; the arrangement
  sets the global layout), with per-display settings beside them and an
  "Identify" overlay. Remembered per monitor (EDID make/model/serial) and
  restored on reconnect; a revert-in-15-seconds confirmation after mode
  changes. Windows move off a removed display; workspaces and Deskbar
  follow the main display; cursor crosses edges at matching scale.
  Done: per-output mode/refresh/scale, drag-to-arrange with snapping,
  revert countdown, saved per connector and restored at start-up,
  neighbours follow a resized screen, multi-screen headless tests.
  Rotation, the main display (Deskbar and desktop follow it, also on
  unplug) and Identify done too. ~~Settings keyed by EDID~~ (make,
  model, serial; connector entries migrate), ~~mirroring~~ (fit and centred,
  "Use as" in Preferences).

- ~~**Quick Look: scroll, filter, search and sort tables**~~ (Ben,
  2026-10-29; done: HACKING, `crates/preview`'s spreadsheets, and the
  kit's `ColumnListView` scrolling sideways): "in quick look it shows a csv with rows and cols which is
  great but it doesn't let me scroll horizontally to see all the cols we
  need to support horizontal scroll and also add the ability to
  filter/search/sort in there too". CSV, TSV and spreadsheets: scroll
  sideways to every column; search across the cells; filter rows; sort
  by a column (header click, again to reverse), on a worker for big
  files.

- ~~**Always show the tab bar, even with one tab**~~ (Ben; done):
  Terminal's `TabStrip` shows with one tab. Window groups needed nothing:
  every window shows its yellow tab, as in BeOS, and a lone window's tab
  is its group's bar with one tab in it (decided rather than asked:
  another strip over a single window would be a second title).

- ~~**Flicker, still, now and then**~~ (Ben; done: HACKING, "Finding
  flicker"): most of all on Quick Look (Space on an image in Tracker):
  the window flashed as it opened. Recorded frame by frame (the new
  `record` request): its first frame was in its cascade slot, the next
  over Tracker. Windows were placed in a post-commit hook, which runs
  before the buffer is taken in, so placement waited a commit; now it
  happens in the commit handler once the window knows its size, before
  it is drawn (every dialog and every returning window had the same
  frame elsewhere first).

- ~~**olo-wlproxy upgrades only at login**~~ (done: HACKING, "Upgrading
  the proxy"): an in-place upgrade kept the running proxy (restarting
  it would close every outside program's windows), so a protocol added
  to it (data-control was) reached programs only after logging in
  again. Now an upgrade starts the new proxy on a fresh socket put in
  the old one's place: new programs get it at once, and the old proxy
  keeps the programs it has until they close (then exits, from this
  version on). Carrying the running programs' state over into the new
  proxy (re-executing with every object, buffer and descriptor) was
  weighed and left: a mistake there closes every outside window.

- ~~**Quick Look: Markdown rendered nicely**~~ (Ben, 2026-10-29; done:
  HACKING, `crates/preview`'s formatted documents, `rich.rs`): Quick
  Look's preview of Markdown files should be much more polished than it
  is now. Now a set page: a measure, a 20-point line, headings with room
  and rules, hanging indents, task boxes, alerts, code coloured by its
  language, tables lined up as written, the document's own pictures,
  links that open (and `#anchors` that scroll), laid out once and drawn
  only where seen.

- ~~**Notifications: pin a notification**~~ (Ben, 2026-10-29; done:
  HACKING, Notifications' Pin and Snooze): a pinned notification stays
  put: it doesn't fade out over time and isn't dismissed by dismissing
  all. Pinned, a banner stays on screen until closed (and after a
  restart), and the history keeps it at full strength.

- ~~**Notifications: snooze a notification**~~ (Ben, 2026-10-29; done:
  HACKING, Notifications' Pin and Snooze): snoozing puts it away and
  brings it back after a while (15 minutes, an hour, three hours or nine
  tomorrow morning; `--snooze ID MINUTES` for any time), as new.

- ~~**Notifications: one click on a card acts**~~ (Ben, 2026-10-29; done:
  HACKING, Notifications' Cards): "in
  notifications app card view, i should only need a single click of a
  notification card to activate / go to the action".

- ~~**Instant updates when files change**~~ (Ben, 2026-10-29, bug; done:
  HACKING, rule 7 "Stay current without polling", `olo_kit::watch`): "currently
  when something changes on the desktop it doesn't reflect immediately,
  e.g. if i empty the trash we don't see the icon change for a few
  seconds. we should use event based notifications with optimized linux
  file watching or whatever so that this happens instantly. and that same
  principle should apply to everything in tracker and everywhere in olo:
  we want event based instant updates to any file changes so that olo
  always reflects the latest state. and it must all be done super
  efficiently so that it uses minimal resources." Every view of files
  (Tracker's windows and desktop, the Trash icon, Disk Usage, file
  panels, Viewer and StyledEdit's open documents, the palette's index)
  follows the kernel's change events (inotify / fanotify), at once,
  with no polling and little memory.

- ~~**Auto-tiling follows the sizes I give**~~ (Ben, 2026-10-29; done:
  HACKING, auto-tiling; edges no longer snap when let go): "auto-tiling
  should let me resize windows however i want and then adapt around how i
  have sized them, instead of locking me to some sort of hidden grid like
  it does right now". A resize by hand keeps the size it was let go at
  (no snapping to quarters and thirds), and the neighbours take up the
  rest around it; new and closed windows keep the sizes given.

- ~~**Keyboard profiles**~~ (Ben, 2026-10-29; done: HACKING, "Keyboard
  profiles"): Mac, Windows or BeOS keys for everything, true to each
  platform; every app's menu keys come from the profile; the shortcuts
  sheet (⇧⌘/); Tracker's ⌘↑ and ⌘⌫ work (they arrived as Ctrl+Home and
  a line deletion before).

- ~~**Keyboard shortcut profiles**~~ (Ben, 2026-10-29; done: HACKING,
  "Keyboard profiles"): Mac (the default), Windows and BeOS profiles in
  Preferences ▸ Keyboard ▸ Shortcuts. Each has every shortcut,
  app_server's and the apps' menu keys, with the command key and its
  key behaviour. A change makes "Mac (Customized)", Save As… and Delete
  keep or drop named ones, and they go in Backup with the settings.
  ⇧⌘-arrows select text again (Mac tiling is Rectangle's ⌃⌥), and the
  shortcuts sheet comes up on ⇧⌘/ (Win+Shift+/, Alt+Shift+/).
  Not yet: keys for apps that aren't Olo's (Chrome's own).

- ~~**Twitcher: choose with the pointer**~~ (Ben, 2026-10-29; done:
  HACKING, Twitcher): while ⌘ (or ⌥) is still held, the pointer over a
  cell chooses it and a click there ends the switching on it.

- ~~**Terminal resizing itself**~~ (Ben, 2026-10-29; done): selecting
  text with Auto-Tiling on squeezed the Terminal (wl-copy's 1x1 window,
  made because the session's proxy predates data-control, took a tile;
  now kept out of sight); a window zoomed vertically keeps its height
  as its text size changes (app_server says which sides it holds, the
  kit keeps them); hidden windows no longer jump to a new text size
  when they come back, and covered ones take it at once.

- ~~**Every display has its own workspaces**~~ (Ben, 2026-10-29; done:
  HACKING, "Workspaces, one set per display"): each display switches
  among its own 1–4 (or however many) on its own; Deskbar is on every
  display (the setting is gone), each listing only its display's
  workspaces, headed "D0 Workspace 1" with more than one display, with
  the workspace's windowing mode at the heading's right (no longer in
  the tray) and a menu to show, hide, close or move all its windows to
  any workspace of any display; Twitcher's columns and the overview's
  panes are named the same way. Also fixed: with the external displays
  plugged in, the laptop's screen had no Deskbar (the first bar, opened
  before the displays were known, named none and followed the main
  display to the Studio Display, while Deskbar took it for the
  laptop's). The tray's icons share their rows evenly.

- ~~**Open in Tracker, everywhere it makes sense**~~ (Ben, 2026-10-29;
  done: HACKING, Open in Tracker): `olo_kit::tracker` and Tracker's
  `--reveal`; Terminal's tabs and menus, StyledEdit, the Viewer,
  DiskUsage, DriveSetup, Preferences' backups, the palette (⌘↩ on a
  file), every window's yellow tab (`Cx::set_path`) and Tracker's Find
  results (Open Parent).

- ~~**Opening an app makes a new window on this workspace**~~ (Ben,
  2026-10-29; done: HACKING, "Single instance", `Cx::on_this_workspace`):
  opening Tracker (or a folder), StyledEdit or a Terminal tab when the
  app's windows are all on another workspace opens a new window here
  instead of going there; each workspace has its own group of the app's
  windows. Single-window apps (Preferences, Activity, …) still go to
  their window's workspace.

- ~~**Screenshots: the saved file's path on the clipboard**~~ (Ben,
  2026-10-29; done: HACKING, Screenshot and `olo_kit::clipboard`): Save,
  ⌘⇧3, Print Screen and ⌘⇧4 all put the file's path on the clipboard.

- ~~**Auto-tiling: a neighbour spills past Deskbar after a resize**~~
  (Ben, 2026-10-29; done: autotile.rs, tiling.rs, grabs.rs, tested by
  `tools/dev/auto-fuzz`): making Terminal wider resized Tracker beside
  it, but Tracker then reached past Deskbar's edge until the next layout.
  The edge drag's last size went to Terminal after the tiles were laid
  out for the one before it; a tile laid out while a size it was asked
  for was still on its way took that size late, unplaced; a neighbour
  stopped being followed once it answered, before it drew; and a new
  window's least size, learnt as it first drew, waited for the next
  layout.

- ~~**Windowing modes**~~ (Ben, 2026-10-29; done: HACKING, "Desktop
  shortcuts and tiling", shell/modes.rs): auto-tiling is one of three
  modes, per workspace (and per display with a Deskbar on each). Deskbar's
  tray icon shows the mode and opens Window Arrangement, which chooses
  it; Preferences ▸ Windows sets the default; ⌃⌥⌘T and ⌃⌥⌘O, the palette
  and `set_windowing_mode` too, each with its OSD. Standard (the default)
  leaves windows where they are put; Auto-Tiling as before; Focus shows
  only the active window, zoomed, its app's others grouped behind it as
  tabs, dialogs, alerts and floating windows over it. Activating another
  shows that one; the rest are covered (not drawn, no input, ready to
  show at once). Leaving Focus puts each window back; arranging commands
  leave it for Standard. Old auto-tiling settings are read as modes.

- ~~**Auto-approve for a while**~~ (Ben): Deskbar's key in the tray
  (click: 15 minutes; right-click or Olo ▸ Approve Without Asking: 5
  minutes to a day, Ask Again) turns it on with the account password
  (not Touch ID: Terminal answers sudo with it). While on, Terminal's
  sudo window and Passwords' unlock and `op` access go ahead without
  asking, each in a quiet notification and the log; the time left is in
  the key's tip and menu. It ends at its time, when the screen locks
  (sleeping locks first) and with app_server, and the password is wiped.

- ~~**⌘[ / ⌘] are Back / Forward everywhere**~~ (Ben, 2026-10-29; done:
  HACKING, BeOS idioms, keymap.rs): in every program that isn't Olo's
  (Slack too, not only browsers) they reach it as Alt+← / Alt+→. ⌘← / ⌘→
  never navigate: they are the line's start and end everywhere.

- ~~**Terminal restores itself after Quit**~~ (Ben; done: HACKING,
  Terminal "Restoring"): the state is saved as
  it changes, so quitting and starting Terminal again brings back every
  window, group, tab and working directory. If olo-termd is running
  with the sessions, reattach them; if not, still restore the tabs,
  their directories and their scrollback.

- ~~**Chrome's tree: inside iframes**~~ (Ben; done: HACKING, "Chrome's
  widget tree, for agents", Iframes): an iframe is opaque to the
  tree agents get (`tree.js` runs in the top frame only, `frameIds:
  [0]` in `background.js`), so embedded logins, payment forms, consent
  dialogs and the like can't be read or driven by name. Run it in every
  frame (`allFrames`, cross-origin ones too), tell which iframe element
  each child frame is (the parent posts a token into each
  `contentWindow`; the child's content script keeps it and `tree.js`
  reports it), then stitch each frame's nodes under an `Iframe` node at
  the element's content box (its rect plus border and padding, the
  child's own scroll and zoom), clipped to it, nested frames in turn.
  `click`, `set` and `wait` then work inside them as they do on the
  page. HACKING, "Chrome's widget tree, for agents".

- ~~**Spotlight the chosen app while switching**~~ (Ben, 2026-10-28;
  done: HACKING, "Twitcher", spotlight.rs): while ⌘Tab (or ⌘\`) is
  held a moment, everything but the chosen app's windows (and their
  dialogs) dims, following the choice; it fades out as ⌘ is let go, and
  Esc or the lock takes it away at once. A quick ⌘Tab flips without it.

- ~~**An overview of everything, on ⌃Space**~~ (Ben, 2026-10-28; done:
  HACKING, "The overview", overview.rs): every display's workspaces as
  panes, their windows spread so none covers another, live (those on
  other workspaces too), lettered; a click or a letter makes one the
  active window on its workspace, the arrows and Return walk them, Esc,
  ⌃Space or a click on nothing leaves. Windows showing glide into place
  and back; the rest fade. Agents: `{"type":"overview"}`. Not yet:
  dragging a window to another workspace's pane, adding a workspace.

- ~~⌘Tab across workspaces~~ (Ben): ⌘Tab (⌥Tab keeps to this
  workspace's apps, ⌃Tab is Next Tab; the other way round at first): in
  order of use whatever the workspace, so ⌘Tab flips between the last
  two apps, and Deskbar highlights the app chosen; a row of apps per workspace;
  ↑/↓ change rows (and workspaces), ←/→ walk a row, Esc puts everything
  back, and with keyboard hints on each cell has a letter to pick it by.

- ~~**Auto-tiling mode**~~ (Ben, 2026-10-28; done: HACKING, "Desktop
  shortcuts and tiling", shell/autotile.rs): Deskbar's tiles beside the
  cup, ⌃⌥⌘T, the palette and `set_auto_tiling`, per workspace (and per
  display with a Deskbar on each), remembered. A tree of splits per
  display (dwindle: a new window halves the focused tile along its longer
  side, a closed one's room goes to its neighbour, a window group is one
  tile); Zoom over the tiles and back, Zoom Vertically / Horizontally a
  full column or row, the tile commands set a window's part with the rest
  around it, edges dragged (snapped to quarters and thirds then; now they stay where let go), tabs drag to swap or go
  beside. Dialogs, alerts, fixed-size windows, the kit's
  `WindowOptions::floating()` (xdg-toplevel-tag) and portal and
  picture-in-picture windows float; Float Over the Tiles (window menu,
  ⌃⌥⌘F, `set_floating`) by hand. O(windows), only on a change, a new
  window opening at its tile's size, minimum sizes kept, ⌃⌘Z, through an
  in-place reload. Not yet: a master-and-stack layout (dwm, Amethyst),
  resizing every tile in one transaction (Sway), a snap-layout picker.

- ~~Terminal: ⌘- / ⌘= and Ctrl+_~~: ⌘- and ⌘= (Smaller, Bigger) did
  nothing, the shell's Ctrl+_ was out of reach, and the menus read ⇧⌘N
  for ⌘N. ⌘ now reaches terminals on the right Ctrl, which the kit marks
  as `command` ("Raw keys" in HACKING), and the menus read as pressed.

- ~~DiskUsage~~ (Ben): a new app for finding and reclaiming disk space,
  Haiku's ring chart beside a list by size, with DaisyDisk's Collector
  (drag in, then Move to Trash or Delete); volumes from mountinfo, a
  streaming worker scan that stays on one file system.

- ~~Touchpad drags, palms and stalls~~ (Ben): Olo's touch filter (a
  libinput plugin app_server loads; docs/TOUCHPAD.md) counts a touch
  only while it presses, ignores palms and makes the click from the
  fingers alone; Preferences ▸ Touchpad detects the pad, recommends a
  profile (MacBook, Magic Trackpad, Standard, Custom) and has every
  tunable: filter thresholds, click pressure, scrolling speed, glide,
  three-finger drag, drag lock, button maps, acceleration, typing
  timeouts. The driver-side fix is written up, not done (Ben's call).

- ~~⌘Space finds an app's open windows first~~ (Ben): typing an app's
  name lists its windows before the app, so the palette switches windows
  too.

- ~~Group an app's windows~~ (Ben): an app's windows share one frame
  with a proper tab bar (side by side, never overlapping); Group Windows
  in Deskbar's team menu and Tracker's menu for an app turns it off or
  on per app, saved in `ungrouped_apps`.

- ~~⌘V doesn't paste in Terminal~~ (Ben): settings saved before the
  rename listed `tabula.terminal` as a terminal, so Olo's Terminal got
  ⌘V as Ctrl+V (bash's quoted-insert). Old app ids in saved settings are
  Olo's now (`adopt_old_app_ids`).

- ~~Deskbar's Olo menu arranges windows~~ (Ben): Window Arrangement,
  Workspaces (with Move Window To) and Show or Hide All Windows, acting
  on the window that had the focus.

- ~~Zoom All, Window to Workspace 1–9~~ (Ben): ⌃⌥⌘↩ zooms (or unzooms)
  every window on the display; ⌃⇧⌘1–9 move the focused window to a
  workspace.

- ~~⌘Tab keeps the stacking~~ (Ben): windows passed over go back where
  they were; only the chosen one is raised.

- ~~Deskbar shows the wordmark alone~~ (Ben), centred in its tile.

- ~~Rolled-up windows share one tile~~ (Ben): Tile All (and Tile App) gives
  the rolled-up windows one cell between them, their tabs fanned out along
  its top (a row lower when full), not a nearly empty cell each; tab
  offsets now survive a reload.

- ~~Cascade Others, Fan Out Others~~ (Ben): ⌃⇧⌘\ and ⌃⇧⌘/ arrange every
  window but the focused one, which is left where it is, in front.

- ~~Keyboard hints everywhere~~ (Ben; opt-in, Preferences ▸ Keyboard):
  pressing Control twice letters every control, menu item, list row, icon, window and
  Deskbar entry; a letter picks it or letters what's inside, as Vim's
  EasyMotion does. From the kit's trees (Tracker's icons and column rows
  are in them now, as are layer surfaces), so every kit app takes part;
  agents read the letters with `{"type":"hints"}`.

- ~~Resizing tiled windows is slow~~ (Ben): neighbours following a
  dragged edge got a configure per pointer motion and fell seconds
  behind, overlapping the dragged window by up to 100 px. They're paced
  like the dragged window and moved only when they draw (a 500 Hz drag:
  302 configures and 1.8 s to catch up before, about 110 and 0.03 s now;
  the gap stays even).

- ~~AI features use OpenAI's models~~ (Ben): Preferences ▸ AI offers
  Astra 6 (default), Sol 6.1, Terra 6.1 and Luna 6, through the Token
  Pool's Codex accounts; a Claude model chosen before reads as Astra.
  Every call retries 429/503/529 with backoff (up to 4 tries). The pool
  no longer benches an account for a 429 that isn't a spent meter (it
  was turning every account away with "out of quota" at 13% use) and
  passes the provider's own message back.

- ~~Tabula is now Olo Desktop~~ (Ben): plain `olo` in code (crates,
  programs, app ids `olo.X`, `OLO_*`, `~/.config/olo`…, D-Bus
  `com.olodesktop.*`), "Olo Desktop" in public; the brand (mark, wordmark,
  desktop icon) in Deskbar, About, the session entry and the README. A
  running Tabula session upgrades through the green arrow into Olo with
  every window, shell and setting kept (`olo_xdg::legacy`; HACKING
  "Renamed from Tabula"). The wallpaper pack (data/wallpapers) too.

- ~~Icons in the Deskbar menu~~ (Ben): every item in the Olo menu
  has its icon (applications, panels, .desktop entries, commands,
  categories; twelve new sunburst icons); Appearance ▸ Show icons in the
  Deskbar menu, on by default. Kit menus take `.icon(name)`.

- ~~A widget tree for Chrome~~ (Ben): the Olo extension's host
  answers Agent Screens' `tree` for Chrome, with each window's page as
  assistive technology sees it; the olo-screens skill says so.
  HACKING, "Chrome's widget tree, for agents".

- ~~NetPositive removed~~ (Ben): Olo's web browser is Chrome. Gone
  with it: apps/browser, the Servo and WebRender patches (and epaint's,
  needed only because of Servo), the kit's embedded surfaces, and
  passkeys in Passwords (only NetPositive asked for them).

- ~~1Password's `op` from Passwords~~ (Ben): `olo-op`, linked as `op`,
  answers `op inject`/`read`/`run`/`item get`/`whoami`/`account list`/`signin`
  from Passwords, asking once per terminal session; Preferences ▸
  Security turns it off. HACKING, "op: 1Password's command line".

- ~~Upgrades keep apps open~~ (only a new app_server reloads; Olo
  programs re-execute in place), ~~interactive resize keeps up~~,
  ~~focus not stolen by xdg_activation without input~~ (15c2b27).

- ~~Keyring prompts are Olo's~~: the polkit agent owns
  org.gnome.keyring.SystemPrompter (gcr's protocol and secret exchange,
  natively), so unlocking the login keyring (gh's git credentials hung
  on a GTK prompter that had lost its display) asks in an Olo alert.

- ~~Kit opener instead of `xdg-open`~~: `olo_xdg::open` (shared-mime-info
  globs + sniffing, our apps first, mimeapps.list, Exec field codes).

- ~~**Other programs survive an Olo upgrade**~~ (done: `olo-wlproxy` holds their connections and replays their state on the new app_server; tested headless with GNOME Text Editor, kitty and Chrome through several reloads, a menu open during one, an olo-session re-exec and a proxy crash; HACKING, "The session and in-place reload". Deskbar's upgrade alert names only programs that won't survive (`WindowInfo::kept`), and windows put back after a restart stay where they were even when they hang off the screen. X11 is out: Olo is Wayland only, Xwayland support removed) (Ben): when app_server
  restarts for an upgrade, Chrome and other non-Olo Wayland apps
  keep running with their windows, as Olo's own apps do (e.g. hand
  the listening socket and client connections to the new app_server,
  or a small proxy that holds client connections across the restart
  and replays their state).

- ~~**Settings & state backup: export/import in one file**~~ (Preferences ▸ Backup and `olo-preferences --export/--inspect/--import`; tested headless) (Ben): a native
  Olo utility that exports every Olo setting and piece of saved
  state, across all Olo apps (Preferences settings, Tracker views,
  Deskbar, Passwords' vault
  file, keymaps, window state, …), into a single compressed file, and
  imports one back (with a summary of what it holds and a confirm before
  replacing anything). In Rust, in process (e.g. tar + zstd crates);
  secrets stay encrypted as they are on disk.

- ~~Cursor shapes~~ (Haiku's cursors via olo-hvif, cursor-shape-v1);
  ~~night light~~ (CRTC gamma ramps, faded); ~~libinput settings~~ (touchpad tap, tap-drag, dwt, click
  method, natural scroll, speed; udev only); ~~centred alerts~~
  (`WindowOptions::alert`, set_parent, placed over the parent).

- ~~**Screen lock**~~: ext-session-lock in app_server, `olo-lock`
  (PAM + Touch ID), Ctrl+⌘Q / Deskbar / idle display-off / sleep / logind
  Lock; never fails open; survives reload (see HACKING "The lock screen").
  Lid close goes through logind's suspend, so it is covered when the lid
  sleeps.

- ~~**Screensaver**~~: the eleven R5 modules (olo-screensavers),
  olo-screensaver on every output, Preferences ▸ Screen Saver (Fade and
  Modules tabs, live preview, Test, hot corners), lock-after drawing over the
  lock screen; Flip and Lens see the desktop on every backend.

- ~~**Caffeinated mode**~~: Deskbar cup (click; right-click or Olo ▸
  Stay Awake for 1–8 hours or with the lid closed), Ctrl+⌘A, timed end,
  shown in the cup's tip and menu.

- ~~Roll up windows~~ (Ben): ⌃⌘M, Roll Up in the tab's menu, the palette,
  or double-click the tab: only the tab stays; again rolls it down.

- ~~Longest tab title~~ (Ben): Preferences ▸ Appearance ▸ Longest window
  title (20–200 characters, 100 by default); longer titles end in "…".

- ~~Terminal: Rename Group and Ungroup did nothing~~ (Ben): no context
  menu of Terminal's worked (group, tab, text); the kit now sends a
  popup's choice that no widget takes to `App::command`.

- ~~AI preferences, and Smart Tiling~~ (Ben): Preferences ▸ AI uses the
  Token Pool by default (or a saved Anthropic key); Smart Tiling (on,
  Claude Opus 5.5 at medium; also Sonnet, Haiku, Fable and Astra 6)
  shows the model a screenshot, each window's app, title and widget
  outline, and applies its tiling (⌃⇧⌘↩, undoable, with progress and
  Cancel). tsession sessions now get their own D-Bus.

- ~~Resizing after Tile All moves the others~~ (Ben): Tile All and Tile
  App now tile each window into its cell (`Tile::Cell`), so dragging an
  edge resizes the neighbours on that line.

- ~~Token Pool follow-ups~~: a reset credit about to expire is spent at
  its best moment rather than lost (projected from each window's rate),
  and Claude Code's settings.json keeps its key order when edited.

- ~~Screenshot Window from the palette~~ (Ben): takes the window the
  palette was opened over (`--window ID`), not whatever is active then.

- ~~Token Pool heatmap marks~~ (Ben): a cell's "C"/"X" is centred on its
  capitals and kept inside the well.

- ~~DriveSetup: usage and free space~~ (Ben): a Free column with a bar of
  how full each mounted volume is (red past 90%), "N free of M" in the
  status line, `space` in `--list`, re-read every 5 s while open.

- ~~Fan Out and Undo Window Arrangement~~ (Ben): ⌃⌘/ stacks a display's
  windows with their tabs side by side, narrowing them to stay clear of
  the Deskbar (never below a quarter of the screen); ⌃⌘Z undoes the last
  arrangement (and again redoes). Also: Deskbar keeps its app list after
  an in-place reload (a layer surface came back at its first size).

- ~~Terminal: Show All Tabs, and tab groups that look right~~ (Ben):
  Show All Tabs (⌘⇧D) tiles the window's tabs as live panes, typing into
  the one clicked (it used to tile the group windows instead); the tab
  strip is taller with the group's band clear of the icons; a dialog's
  first field starts with its text selected.

- ~~Token Pool: a live heatmap of requests~~ (Ben): the requests open
  now as lit cells, each account's latest 25 as a strip coloured by how
  they ended, click for the details; the kit's new `CellGrid` (Pulse's
  LEDs). HACKING, "Token Pool" ▸ Activity, and Widgets.

- ~~Token Pool: reset credits~~: shown per account, spent when every
  account is out, or by hand.

- ~~Num Lock on by default, and a setting for it~~ (Ben): Preferences ▸
  Keyboard ▸ "Num Lock on when Olo starts".

- ~~Token pooling for Claude Code and Codex, natively~~ (Ben): Token
  Pool (`olo-pool`, crates/pool): a loopback gateway the clients are
  pointed at, subscriptions drained earliest-expiry-first, sign-ins
  sealed with a keyring key, usage counted without content. HACKING,
  "Token Pool".

- ~~**Notification banners: never, Deskbar-wide, compact**~~ (Ben; done:
  HACKING, Notifications main.rs): "Show banners for:" has Never (only
  errors show); banners are the Deskbar's width, flush with the right
  edge in its column, with tighter padding and no big icon unless the
  app sent a picture.

- ~~**New Window from a running app's context menu**~~ (Ben; done:
  HACKING, "Single instance"): Tracker offers New Window on a running
  Olo app's program or link (folders and desktop), and Deskbar in
  the app's menu; both forward
  `--new-window` to the running copy instead of launching a second one
  (Tracker, Terminal and StyledEdit; `apps::NEW_WINDOW`). Agents run
  `olo-x --new-window`.

- ~~**Window menu on the title tab**~~ (Ben; done: HACKING, "The command
  palette", window menu): right-clicking a window's yellow tab opens a
  kit menu of Hide, Zoom, Back to Its Own Size, Center, Tile ▸, Move to
  Workspace ▸, the displays, Send to Back and Close, the shortcuts' own
  commands. Not yet: Always on Top (app_server has no such state) and
  "tile with…" (tiling beside a chosen window).

- ~~**Notifications: Dismiss, and Activate becomes Open**~~ (Ben; done:
  a selected note has Dismiss, which takes the whole conversation when
  it gathers several; the default action always reads Open; By App's
  per-app Clear became Dismiss too; HACKING, history.rs).

- ~~**Terminal tabs, tab groups and coding agents**~~ (Ben; done
  2026-10-27): tabs (the kit's `TabStrip`; HACKING, Terminal "Tabs"),
  tab groups that can be saved and reopened (a window is a named,
  coloured group; "Groups"), Show All Tabs tiling a window's tabs,
  and native recognition of Claude Code and Codex in a tab, its state on
  the tab and a notification when one needs attention ("Agents").

- ~~**Drag and drop everywhere, across apps**~~ (done) (Ben): everything that shows
  files or data can be dragged and dropped, within and between apps and
  to and from other Wayland clients (wl_data_device, text/uri-list and
  the data's own types): Tracker items to any app (e.g. an image into
  Terminal, so Claude Code gets its path), Tracker accepting drops from
  everything, StyledEdit text, Viewer
  images, Passwords fields, Deskbar launches…
  Study how BeOS did it first (the Be Book: BMessage drags with negotiated
  types, drop targets, Tracker clippings) and borrow what still fits.
  **Done:** the kit layer (`olo_kit::dnd`: negotiated types and
  actions, drag pictures with names, drop outlines); Tracker drags from
  every view and the desktop and takes drops on folders, windows, the
  Trash and the desktop (move/copy by file system, ⌥ copies, a
  secondary-button drag asks Move/Copy/Link, text becomes a clipping);
  desktop icons placed freely and kept (Clean Up puts them back on the
  grid); Terminal types dropped paths (shell-quoted) or text; Viewer
  and StyledEdit open dropped files. **Left:** ~~dragging text out of and
  into text fields and StyledEdit (move within, copy between)~~ (done:
  kit `text_drag`, tested headless), ~~Viewer
  images out (the file and image/png)~~ (done, with Quick Look's), ~~Passwords fields (a username or
  password onto another app's field)~~ (done), Deskbar (~~drop files on an app to
  open them with it~~ done; ~~drag an app out to make a link~~ done), dragging from
  ~~Tracker's Get Info~~ (done), ~~spring-loaded folders (hover on a
  folder while dragging to open it), scrolling a view when a drag nears
  its edge~~ (done).

- ~~**Notifications history: denser, better looking, several views**~~
  Done 2026-10-27: two-line rows (16 px icon, bold title with app and
  time, one line of body; the selected one opens to its whole body and
  actions), a dot for what is new, day headings, search, the keyboard
  (arrows, Return, Delete, Escape, ⌘F), and List / By App / Cards /
  Conversations from the View menu (⌘1–⌘4), remembered.
  (Ben, 2026-10-27): each entry now wastes a lot of room (a 32 px icon beside a
  stack of app name, title, the sender's site on its own line, a blank
  line, then the body), so only three fit in a 380 px window. Make each
  take far less space and look much better: the app and time on one
  line with the title, the body right under it (one or two lines,
  ellipsized, the whole of it on selection or hover), the site folded
  into the app line, no blank line, a smaller icon, tighter rows, a
  clear unread mark. Then offer view styles (View menu, remembered):
  1. *List:* one compact row each, newest first.
  2. *By app:* grouped under each app (icon, name, count), groups
     collapsible, newest group first; Clear per group.
  3. *Cards:* a grid of small cards, as on a phone's lock screen.
  4. *Conversations:* entries from the same thread (same app and title,
     such as one chat channel) stacked into one, newest on top, with a
     count, expanding in place.
  Also worth trying: a timeline with day headers (Today, Yesterday),
  search/filter, and keyboard navigation (arrows, Delete, Enter to open).
  Keep it agent-readable (rows in the tree, named by their text).

- ~~**Resizing a tiled window resizes its tiled neighbours**~~ Done
  2026-10-27: dragging an edge moves the whole split (neighbours across
  the gap and in line), live, stopping at apps' minimum sizes; the parts
  are kept, also across a reload; only command-tiled windows take part.
  (Ben, 2026-10-27): e.g. one window tiled to the left half, one to the top
  right, one to the bottom right; dragging the bottom-right one's left
  edge to make it wider makes the left one narrower, and dragging the
  split between the right two moves both. Only windows that were tiled
  by a tiling command or shortcut take part (remember that per window,
  and forget it once the window is moved or sized away from its tile by
  hand); windows placed automatically or by hand are never pushed. It
  should work as you'd expect: shared edges move together, nothing
  overlaps or leaves a gap, neighbours keep a sensible minimum size.

- ~~**Agent Screens: bugs and improvements from an agent using it**~~
  Done 2026-10-27, all 16: popups in `tree` (Menu/MenuItem) and
  `windows` (`popups`); `scroll`, `drag` and `wheel` paced over time (a
  scroll no longer flings 20,000 px; drags move knobs and corners; the
  kit's wheel read only `discrete`, so every mouse notch was 15 px, now
  48 from `value120`); `menus`/`invoke` use the only window when none has
  the focus, and errors list the windows; rows were already clipped
  (a62e672) and now say `row N/M` and `selected`; `wait`, find by
  name/role/match/text for `click`, `set`, `select`, `scroll-to`; tree
  filters and one line per row; `focus`/`close`/`minimize`/`zoom`/
  `resize`/`place`; JSON from actions with the focused element after;
  `look` with no file, `--crop`; `type`/`set` warn when keys go to
  something that isn't a text field; skill doc updated. (Bug 5, focus
  lost after clicking a row, didn't happen here; the fallback covers it.)
  (Ben, 2026-10-27; feedback from an agent working with `olo-screen`).
  Bugs:
  1. *Popups are invisible:* a dropdown's menu (Log Viewer's level filter)
     shows on screen but is in neither `tree` nor `windows`. Popups belong
     in `tree` (under the window that owns them) and in `windows`.
  2. *`scroll` never ends the touchpad gesture:* scrolling past the top of
     a list leaves the overscroll gap (first row stuck at y≈240, knob
     wrong). Send the gesture end after the movement (`--no-end` to leave
     it open).
  3. *`drag` is too abrupt:* dragging a scrollbar knob or a window's
     corner does nothing, while `down`/`move`/`up` in steps works. Send
     intermediate moves with small delays (`--steps N`,
     `--duration MS`).
  4. *`wheel` stops short:* `wheel agent -30` didn't reach the top of a
     ~60-row list. Capped per call, or merged into one event? Not yet
     known.
  5. *Focus lost:* after a click on a list row, `windows` shows the only
     window unfocused and `invoke` fails ("no window has the focus").
     Find out why (perhaps the app). With one window, `invoke` and
     `menus` should just use it.
  6. *Rows take the header's names:* a table row partly under a sticky
     header gets the header's titles in its name, and hidden rows are
     reported under the header. Clip rows to what shows and mark them
     clipped or offscreen. (a62e672 clipped rows and cells; check against
     this report.)
  Improvements:
  7. *Waiting for the app:* `olo-screen wait agent --idle` (until the
     window stops redrawing), `wait --text "61 lines"` or `--role Row
     --name …`, or `--wait` on actions. Show in Context took 5–9 s and
     Find ~1 s; sleeps still read stale state.
  8. *Click by name:* `click agent --name "Errors Only"`, `--role Button
     --name "="`, `--match 'Row:panicked'`, returning what matched.
  9. *Filter the tree:* `--role`, `--name-contains`, `--visible-only`,
     `--depth`, `--max-len`. Big tables print each row three ways (Row,
     Cell × n, Unknown).
  10. *Set fields and choose items directly:* `set agent --name Find
      "PAM"`, `select agent --name Level "Errors Only"` (a dropdown opens
      with the current item under the pointer, so a second click lands on
      the wrong item).
  11. *Window commands:* `resize agent --window 1 1300x850`, `maximize`,
      `focus`, `close`, `move`; the tree could report resize handles.
  12. *Tables:* `scroll-to agent --name …`; the tree giving scroll range
      and row count (`rows=61 visible=13..25`), or every row with an
      offscreen flag.
  13. *JSON from every command:* `click`, `type` and `key` return the
      focused element or window afterwards.
  14. *Pictures:* `look --crop x,y,w,h`, saving to a scratch folder by
      default with the path printed, and hiding the pointer.
  15. *Clearer errors:* `type` says which element got the text (it went
      silently to a list); `invoke`'s focus error lists the windows.
  16. *Skill doc:* prefer `wheel` for lists (until 2 is fixed); dropdowns
      open relative to the current item; Find-style fields can be slow
      to update; `windows` doesn't list popups (until 1 is fixed).

- ~~**Deskbar: an Agent Screens item, and a tidier tray**~~ Done
  2026-10-27: the date and time have their own row (click: Date & Time);
  the well holds only icons, 16 px on a 22 px grid, centred, wrapping
  evenly; a monitor-and-pointer item (with a count) opens Agent Screens.
  (Ben, 2026-10-27):
  1. While any agent screen is running, the Deskbar shows an icon for it
     (in the tray), perhaps with a count; clicking it opens the Agent
     Screens app.
  2. The tray no longer fits all its icons. Move the date and time to a
     row of their own above the tray, so the tray holds only icons.
  3. Make the tray icons' size and spacing consistent and pixel-perfect
     (same size, same gaps, aligned to the pixel grid at 1× and 2×,
     wrapping to more rows evenly), so it looks neat rather than messy.

- ~~**Multiplayer screen control for agents**~~ (Ben; done: Agent Screens,
  `apps/screen`, each screen a headless Olo session of its own, watched
  and taken over in the Agent Screens app; `olo-screen tree` from the
  kit's widget tree; the `olo-screens` skill put in Claude Code's and
  Codex's skill folders by a real session; HACKING, "Agent Screens"): an agent starts a
  Olo screen-control session and gets its own workspace and its own
  cursor and keyboard focus, so its input never disturbs the person, who
  can watch that workspace live (and step in). Complete control: pointer
  moves, clicks, drags and drops, scrolling, typing and shortcuts; visual
  updates (frames on change, sequences over time) and structured metadata
  about what is on screen (windows, widgets, text, roles, bounds, from
  the kit's own widget tree rather than OCR). An agent-friendly
  protocol/API (app_server IPC, documented, stable) plus agent skills
  injected automatically into Claude Code and Codex when they run under
  Olo. Agent-first from the ground up: everything designed to work
  seamlessly for agents and people alike.

- ~~**Disk manager**~~ (done: DriveSetup, apps/drive-setup, tested against
  `udisks-fake` on a private bus rather than loop devices) (Ben): a native Olo utility for disks and
  partitions, after BeOS's DriveSetup (and Haiku's): the disks as a tree
  with their partitions drawn to scale, and per partition: mount and
  unmount, initialize (format) with a file system, create/delete/resize
  partitions, set the name, show what is on it (size, used, file system,
  where mounted). Improve on it where we can: SMART health and
  temperature, encryption (LUKS) unlock and setup, USB sticks and SD cards
  (write an image, eject safely), a benchmark, and live progress for long
  jobs that can be cancelled. Native: talk to UDisks2 over D-Bus (zbus)
  rather than running parted or mkfs; every destructive step behind a
  clear alert naming the disk; polkit for privileges; agent-readable over
  IPC. Never test it on the real disks: loop devices in a test session.

- ~~**GNOME/GTK 4 apps look at home**~~ (done: the portals start
  in a real session (they never did, so GTK 4 had no settings at all),
  and GTK 3, GTK 4 and libadwaita take Olo's colours, fonts and
  close-left/zoom-right buttons in both modes; GTK windows without a
  headerbar get Olo's tab and frame through KDE's server-decoration
  protocol, tested headless with GTK 3 and 4; headerbar and libadwaita
  windows draw their own, by design, in Olo's colours; HACKING,
  "Other toolkits' programs") (Ben): a GTK 4 / libadwaita app
  opened in Olo follows light/dark mode properly (and switches live),
  and gets Olo's window chrome (yellow tab, BeOS borders) rather than
  its own client-side decorations: e.g. asking for server-side decorations
  via xdg-decoration / GTK's settings, and whatever libadwaita needs
  (portal `org.freedesktop.appearance color-scheme`, dconf) to agree.

- ~~**Quick Look / Viewer: basic image editing**~~ (Ben): crop, resize,
  rotate and flip images, then Save / Save As (as the PDF tools do). Done:
  the Viewer's Image menu (rotate, flip, crop overlay, Adjust Size) with
  undo, Save / Save As / Revert; Quick Look rotates and saves in place.

- ~~Long menus scroll~~: kit popups shrink to the screen and scroll
  (arrows at the ends, wheel, keys).

- ~~Hide Window everywhere~~: ⌘M (Preferences ▸ Keyboard) and the
  palette's "Hide window", for any app's window.

- ~~Log Viewer~~ (70aea93).

- ~~**Log viewer app**~~ (done: `apps/logs`, Log Viewer; the journal read
  natively, live; Find, program, level and time filters; syntax
  highlighting; Show in Context; `olo-logs --print … [--json]` for
  agents; HACKING, "Log Viewer"; 2026-10-30: the apps' own log files
  beside the journal too, the Agent Screens' listed in File, any other
  opened, `--print --file`) (Ben): an Olo app that shows every Olo
  event in one place (app_server, olo-session, Deskbar, Tracker and
  every app's log) so issues can be spotted and tracked down: live tail,
  filter by program, level and text, a time range, warnings and errors
  highlighted, and the lines around an error. Read the user's journal
  natively (the journal file format, in process, not `journalctl`) plus
  the apps' own log files; never block the UI (stream it, on a worker
  thread). Agent-first: the same query is available over IPC, so an agent
  can ask "what went wrong in the last ten minutes" without screenshots.

- ~~**Passwords settings in a new Security preferences panel**~~ (Preferences ▸ Security: sync automatically or only when asked, lock after idle, lock with the screen, clipboard clearing; Passwords follows them live; carried over from passwords.json; the panel and settings round trip tested headless, Passwords' side not yet run against them) (Ben):
  choose automatic sync with 1Password or sync only on demand, and how
  long Passwords waits before locking itself (or never). Preferences
  gets a Security panel for them (and a home for other security
  settings later); Passwords follows changes live.

- ~~**Band selection everywhere**~~ (kit `ColumnListView`, so Tracker's list view and every list; Tracker's column view from below a column's items; icon view and desktop as before; scrolls on near the edges; tested headless) (Ben): dragging from empty space draws a
  selection band and selects every item it touches (⇧/⌘ add to the
  selection) in Tracker's list and column views as it already does in the
  icon view and on the desktop, so several things can be picked to move.
  Put it in the kit (ColumnListView and any list of items) so every app
  selects the same way; check how BeOS did it (Tracker's BPoseView,
  BListView, the Be Book's selection conventions) for one consistent
  behaviour across apps.

- ~~**Unzoom goes back to where it was**~~ (a tiled window unzooms back into its tile, one placed by Tile All back into its cell; dragging or resizing it by hand forgets the tile; Restore still goes back to its own frame; tested headless with the shortcut and the zoom box) (Ben): zooming a window that was
  tiled (e.g. to the right half) and then unzooming must put it back in
  that tile, not some older frame (PreZoom should be the frame just
  before the zoom, tiles included, and the tile state restored).

- ~~**Night light does nothing on the MacBook**~~ (confirmed: apple-drm has no gamma ramp, `gamma_length` 0, but every CRTC has `CTM`; app_server now sets a diagonal colour matrix there, in a non-blocking atomic commit retried at vblank; needs Ben's eyes on the real screen) (Ben): turning it on in
  Preferences changes nothing on screen. app_server gets the request and
  fades (log: `night light kelvin=Some(…)`), but no gamma warning is
  logged, so the Apple display driver most likely offers no legacy gamma
  ramp (`gamma_length` < 2) and `udev.rs` `set_gamma` returns quietly.
  Do it where it always works: use the CRTC's `GAMMA_LUT`/`CTM`
  properties through atomic modesetting when the driver has them, or
  else tint in the compositor's own render (a colour-matrix pass over
  the frame, which also covers screenshots and headless outputs). Show
  in Preferences when the screen can't do it; check external displays
  too.

- ~~**Dialogs always come to the front**~~ (the window focused before an
  upgrade or reload kept a claim to focus for good, so a window reopened
  in its remembered place, like the Upgrade alert Deskbar closes and
  reopens, came up under it; now the claim lasts only while windows come
  back) (Ben): the Upgrade Olo dialog
  from the Deskbar tray sometimes opened behind every other window
  (after Tile All). A newly opened dialog or alert must be raised and
  focused.

- ~~**Passwords: sync with several 1Password accounts**~~ (ticked in File ▸ 1Password Accounts…, one review, the sidebar grouped by account) (Ben): setup now
  makes you pick one account; Passwords should sync with every account
  the 1Password app has signed in (each account's vaults shown under it,
  items keeping which account and vault they belong to, per-account sync
  state and errors), with accounts added or removed later in its
  settings. The store and sync (`vault.rs`, `sync.rs`) key remote state
  by account as well as vault.

- ~~**Passwords: sync every vault, Private included**~~ (archived items failed whole batches; now fetched active-only with a one-by-one fallback, and a Sync Problems window) (Ben): a sync on
  the real account doesn't bring in all vaults (Private missing). Every
  vault the account can read is synced, and errors or items that can't
  sync are listed clearly (by vault and item, with the reason), not as a
  wall of red text.

- ~~**Tracker: restore from the Trash**~~ (File ▸ Restore and the item
  menu, `ops::Kind::Restore`; a clash gets a number instead of asking) (Ben): a Restore command (File
  menu and the context menu in the Trash) that puts the selected files
  back where each came from (the trash's `.trashinfo` original path),
  recreating missing folders and asking on a name clash.

- ~~**Terminal: ⌥←/⌥→ in Claude Code**~~ (fixed: under the kitty keyboard
  protocol they're now `CSI 1;3 D/C`) (Ben): Option+Left/Right move by a
  word in plain bash but not in Claude Code. Check what Terminal sends for
  them (bash binds ESC b / ESC f; others expect CSI 1;3D / 1;3C) and
  whether Claude Code's input understands it; send what xterm does.

- ~~**Passwords: never change anything on sync without approval**~~
  (Ben): plan, Review Sync, apply (sync.rs); plus ~~File ▸ Reset
  Passwords…~~ (local only).

- ~~**Confirm before quitting apps with state that would be lost**~~
  (Ben): Terminal asks before
  closing a window with a program running, and its Quit leaves the
  shells in olo-termd; StyledEdit asks about unsaved documents.

- ~~**Video plays the wrong parts at the wrong speed**~~ (Ben): OpenH264
  was flushed after every packet, breaking B-frame reordering; now
  `Flush::NoFlush`, mp4 edit lists applied, and WebM seeks fixed
  (vendored matroska-demuxer).

- ~~**Passwords keeps its own store, synced with 1Password both ways**~~
  (Ben): going to 1Password for every look-up is slow and unreliable.
  Passwords should hold every item itself (encrypted at rest, unlocked
  with the account password or Touch ID) and answer the app, Chrome
  and the palette from it at once; 1Password is only for syncing, both
  ways (changes either side merge by item version, conflicts kept),
  in the background and when asked. Done: `vault.rs`, `sync.rs`; try it
  on the real account (Ben).

- ~~**Tracker: File ▸ Open in Terminal**~~ (Ben): the folder shown (the
  active column's), or a right-clicked folder, in a new Terminal window;
  also in the window's and the desktop's context menus, as a new tab in
  the Terminal window last in front.

- ~~**Deskbar: a click on a running app with several windows offers them**~~
  (Ben): one window comes forward at once; with more, the left click
  opens the same menu as the right click (each window, Show All).

- ~~**Find: Space shows the selected result in Quick Look**~~ (Ben), as in
  a Tracker window; the arrow keys step through the results.

- ~~**Tracker opens in column view by default**~~ (Ben), set in
  Preferences ▸ Tracker; open windows keep their own view.

- ~~**Momentum scrolling and Mac touchpad behaviour**~~ (Ben): a flick
  glides on after the fingers lift and stops when they touch again
  (app_server); taps reach
  every kit control as a press then a release; two-finger tap is the
  secondary click, no middle-click emulation or drag lock.

- ~~**Tracker column view scrolls sideways**~~ (Ben): touchpad,
  Shift+wheel or the new scroll bar; it no longer snaps back to the
  active column, which it only follows when the selection moves.

- ~~**Command palette on ⌘Space**~~ (Ben): apps, the front app's menu
  commands, files, Passwords items, Preferences panels, appearance,
  windows and every desktop shortcut with its keys; a calculator; the web.

- ~~**Menus open on a touchpad tap**~~ (Ben): a tap's press and release
  came in one frame and the press was missed; a second press on the
  title closes the menu.

- ~~**Quick Look: F for fullscreen**~~ (Ben), F again for the window
  (`Cx::set_fullscreen`).

- ~~**⌘A in a text field**~~ (Ben) selects the field's text, not the
  page or folder.

- ~~**Windows while rearranging displays**~~ (Ben): each window stays on
  its screen at the same place as screens are dragged around
  (`fixup_positions`); windows on a removed screen go to the main one,
  cascaded, and ones that no longer fit are fitted (`rehome_windows`).

- ~~**Terminal sessions that outlive Terminal**~~ (Ben): done. Shells
  live in olo-termd (one per display); quitting, crashing or upgrading
  Terminal reattaches every window with its scrollback, and olo-termd
  upgrades in place too. Terminal ▸ Terminal Daemon… shows it and can
  restart it (keeping the shells) or start it over. Images come back
  too.

- ~~Login keyring in a real session~~ (the session finishes starting the
  daemon PAM unlocked), ~~Passwords lists 1Password's accounts~~.

- Password manager: ~~Rust 1Password SDK~~ (desktop-app integration;
  fake server for tests), ~~Passwords app~~ (browse, copy, edit, create,
  archive/delete, generator, lock with ~~Touch ID through fprintd~~ or
  password), ~~browser integration~~ (the Olo Passwords extension in
  Chrome).

- Viewer: ~~image zoom/pan~~ (full resolution up to the texture limit,
  View menu, pinch/⌘-scroll/double-click, drag and scroll to pan; halved
  levels for smooth fitting); ~~video playback (frames + audio)~~ (H.264 in mp4/Matroska, AV1 in
  Matroska/WebM, synced to the sound, seeking, keeps the screen awake;
  ~~AV1-in-mp4~~ (2026-10-30; 600 pictures a second at 640×360 in
  release); VP9 and HEVC still show facts only), ~~AVIF/HEIC/Opus~~.

- ~~Icons for new file kinds~~: spreadsheet, presentation, word-document,
  pdf, font (audio, video, archive already had theirs).

- ~~Kit: asynchronous `read_clipboard`~~ (`paste` reads on a helper
  thread and delivers the Paste event through a loop channel).

- Terminal: ~~find in scrollback~~ (Edit ▸ Find…, Find Again, Find
  Selection; regex over screen and history); ~~release-build memory
  check~~ (fonts mapped, wgpu MemoryUsage: every app ~100 MB smaller);
  ~~colour emoji~~ (olo-emoji, COLRv1 via skrifa + tiny-skia).

- Preferences, remaining panels: ~~Network~~ (Wi-Fi list, join with
  password, disconnect, forget, interface details; ~~WPA-Enterprise~~: PEAP/TTLS/TLS
  sheet), ~~Bluetooth~~ (search, pair with passkey/PIN agent, connect,
  remove, battery), ~~Date & Time~~ (calendar, clock face, network
  time, time zone, Deskbar 24-hour/seconds/day), ~~Touch ID~~ (enrol with progress, remove); ~~Restart Olo~~; ~~a Choose… file panel for the
  wallpaper~~ (app_server now draws it); ~~Display mode switching and drag-to-arrange~~ (with keep/revert
  countdown; modes and positions restored at start-up).

- ~~**polkit authentication agent**~~: apps/polkit-agent (real sessions
  only; fprintd then password through polkit-agent-helper-1).

- ~~Deskbar 24-hour clock option~~ (Preferences ▸ Date & Time ▸ Clock).

- ~~Activity Monitor~~: apps/activity (graphs of every /proc, /sys and
  hwmon source; processes with Quit / Force Quit).

- ~~**StyledEdit**~~: styled text in attributes beside plain text, Font /
  Document menus, Find / Replace windows, own undo, atomic saves, reload
  keeps unsaved work.

- ~~**DeskCalc**~~: R5's keypad, Haiku's scientific rows or none; typed
  or clicked expressions (own evaluator: functions, π/e, implicit ×,
  degrees/radians), history on ↑/↓, keys flash as you type.

- ~~**Screenshot**~~: Haiku's app (screen or active window, border,
  pointer, delay, five formats, folder choice, clipboard), ⌘⇧3/⌘⇧4/⌘⇧5
  (area) and Print Screen; every backend captures.

- ~~Power management and caffeinated mode~~: app_server turns the display
  off and sleeps when idle (Preferences ▸ Power), honours zwp_idle_inhibit,
  handles the brightness and volume keys; caffeinated mode from the Deskbar
  cup or Preferences holds a logind inhibitor. Display ▸ Brightness. ~~On-screen
  level indicator~~. ~~Keyboard backlight~~
  (keys, Preferences ▸ Keyboard, off with the display). ~~Real DPMS off~~ (CRTCs
  cleared, rendering paused until input).

- ~~Preferences ▸ Sound~~: output/input device, level, mute, live from the
  sound server (`olo_system::audio::{devices, watch_devices, set_*}`).

- ~~Preferences app~~: launcher, Appearance, Backgrounds, Display, Keyboard
  (incl. the ⌘ / Caps Lock / tap-⌘ toggles), Touchpad, Workspaces, About;
  `Cx::activate` over xdg-activation.

- ~~nmcli, wpctl, systemctl, gsettings, dbus-update-activation-environment~~
  → `olo-system` (NetworkManager, PulseAudio protocol with a change
  feed, logind, dconf, activation environment).

- ~~Keyboard mapping in app_server, replacing keyd~~: ⌘ as command,
  Caps Lock as Ctrl, ⌘Tab / ⌥Tab / ⌘\` switching, ⌘ arrows / ⌘⌫, tap ⌘ /
  ⌘Space for Find, terminals keep physical Ctrl (`keymap.rs`), Ctrl+⌘Q
  locks. keyd can now be disabled system-wide; while
  it runs it grabs the keyboards and app_server sees its output (handled:
  its ⌘ tap chord and Super+Tab are understood).

- ~~Viewer: pop-out from Quick Look, PDF forms, text, ink, signatures,
  Save / Save As / Export Flattened~~ (903ba8b, f01bff9).

- ~~Kit FilePanel~~ (903ba8b).
