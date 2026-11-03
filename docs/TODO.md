# Work list

What's left before Olo is complete, highest priority first, and what
is finished, below it, most recently finished first. Keep it current:
when an item lands, strike it (with the commit) and move it to the top
of Finished; a new one goes where its priority puts it.

## Open (highest priority first)

- **Auto-tiling: resizing a window is choppy and windows overlap until
  let go** (Ben, 2026-11-03, a bug): "when in autotile mode and
  resizing a window it is super choppy while resizing and the windows
  can overlap and it doesn't all snap into place until i let go. it
  should be smooth the whole time and the windows should react properly
  as i resize them". So: while a tiled window's edge is dragged, its
  neighbours give way frame by frame (the layout worked out at each
  move, not at the release), every window's new size sent at once and
  shown together when the clients have drawn it (no window drawn over
  another, no gap), at the display's rate; measured with `timings` and
  the bench (a live resize in auto-tiling at 120 Hz, no dropped frames).

- **⌘` moves a window from another workspace to this one** (found by
  wm-fuzz, 2026-11-03, seed 5 step 124 on a `2560x1600,1920x1080`
  session; the build from before the Focus fixes fails it the same way):
  after a run of tiling, arranging, a display's scale changed and an
  Undo, ⌘` ×3 left window 6, an app's window on workspace 4, on
  workspace 1. Twitcher shows a window from another workspace while it
  is chosen (`twitch_show`) and must put it back when another is
  chosen or Twitcher closes on another; find the path that keeps it
  (an Undo of an arrangement in between, or the display's scale), fix
  it, and keep wm-fuzz's seed passing.

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
  a palette): clean. Left: Ben's go on his pad with the new build.

- **The touchpad's latency on Ben's machine, and what next** (from the
  palm model in Activity, 2026-11-03): Activity ▸ Displays ▸ Touchpad
  now shows how long the pad's reports take to reach app_server (the
  filter and palm model included) and the main thread's scheduling;
  only a real pad says it, headless sessions have none. If its 99th
  percentile is over 4 ms (red: touches waiting on frames being drawn),
  give libinput a thread of its own, realtime (it never draws, so
  rtkit's 200 ms limit is safe there, unlike the main thread, which
  compiles shaders), handing events to the main loop; measure before
  and after. Also live counts from the plugin (contacts judged, palms
  left out, its own time a frame), which it doesn't report today.

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

- **Sound ▸ Voice Processing: try it on your own voice** (Ben,
  2026-11-02): "in the sound settings -> voice processing section, add
  something where the user can test the settings by recording a short
  sample and then we show in real time some kind of audio frequency
  graph/visualization and what it's doing and then report the
  processing delay stats/metrics and also of course let them play it
  back to hear it (let them play it back both with and without the
  processing applied so they can see and hear the difference)". So: a
  Test button in the Voice Processing box records a few seconds from
  the chosen microphone, raw and through `olo-voice`'s chain with the
  settings as they are (both kept, in memory, never written out
  unasked); while recording and playing, a live spectrogram (or
  spectrum) of the raw and the processed side by side, with what each
  stage is doing shown on it (echo removed, noise suppressed, level
  turned up or down: the chain's own figures); then the delay it adds
  (median, worst, per stage) and how hard it worked (its share of a
  core, frames late); and Play Original and Play Processed to hear the
  difference, switching between them at the same place. Drawn by the
  kit (a chart any app could use), the work on threads of its own, as
  fast as the live path; agents read the figures. Test with
  `tools/dev/pw-sandbox`, never the person's own sound.

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

- **Agent Screens: Chrome opened from a screen's Deskbar lands on the
  person's display** (Ben, 2026-10-31): "i was just using the "Take
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

- **Activity Monitor: an Inputs section, and the Displays panel
  polished** (Ben, 2026-11-03): "add an Inputs section in activity
  monitor, move the pointer + touchpad in there, add a bunch more
  stats/metrics about them with graphs and such. also polish up with
  stats/metrics/graphs the Displays panel". So: a sidebar section of its
  own for input devices, with the pointer's card (`PointerHealth`) and
  the touchpad's (`TouchHealth`) moved out of Displays; more about each
  (reports a second, gaps and latency over time as graphs, the palm
  filter's catches, clicks, scroll and gesture rates, the keyboard's
  too); and Displays given the same: frame times and drops over time as
  graphs per display, refresh rate, what's on overlay planes.

- **Terminal: a scroll bar, slim, there when wanted** (Ben,
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
  pages there), in every pane of a split tab.

- **Terminal: a tab hidden from tile mode** (Ben, 2026-11-03): "ability
  to hide a specific tab in tile mode in terminal by right clicking on it
  and choosing hide when tiled (or show/unhide when tiled if it is
  hidden) and then it should have a different style in the tab bar to
  indicate it's hidden and shouldn't show up in the tile modes". So: a
  tab's context menu has Hide When Tiled (Show When Tiled once hidden);
  a hidden tab stays in the tab bar, drawn differently (dimmed, its
  label and icon faded, in light, dark and night at every scale), and
  every tile mode leaves it out (its tile's room going to the rest)
  until shown again; kept per tab through a reload; agents read it as
  hidden when tiled and can toggle it.

- **Terminal: tile mode's group headers look unselected while every
  group shows** (Ben, 2026-11-03, a bug): "in terminal tile mode when all
  tab groups are being shown (the default) the group headers aren't
  getting the "selected" style, they look unselected. they should be all
  selected by default and then i can deselect/reselect as i want". So:
  with every group shown (tile mode's default, nothing chosen yet), each
  group's header is drawn selected, as a chosen one is; clicking one
  deselects it (its group leaves the tiles) and clicking again selects
  it, from there as now; agents read each header's selected state as it
  is drawn; checked in light, dark and night at every scale.

- **The command palette: typing "n" first closes it; every shortcut in
  it; files only on Ctrl** (Ben, 2026-11-03): "when i'm in the command
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

- **Terminal: a tiny model of its own completes brush's commands**
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

- **Move windows to and from Agent Screens** (Ben, 2026-10-31):
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

- **Notifications, robust and everywhere** (Ben): after Haiku's
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

- **Google Chrome works great in Olo** (Ben; Olo's web browser):
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
  - **Opening things** (started 2026-10-30: the AppChooser portal is
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
  and the displays settle, in the same style.

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
  still to try: no HDR tone mapping yet, as for AV1.

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
