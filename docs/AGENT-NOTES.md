# Notes for agents

What agents working on Olo have learned the hard way: facts about this
computer and its tools, and mistakes not to make again. Read it before
starting; when you learn something non-obvious that the code, HACKING.md
or the Book don't already say, add it here (or correct what's here), in
the same commit as the work. Notes live in this file, in the repository,
never in an agent's own memory outside it (Ben, 2026-11-16: "don't store
memories in claude global memories with tools instead store any memories
in markdown files in this repo and keep them up to date").

Each note: what to do, then why (what happened), then how to apply it.

## Running and testing

- **Run test programs with `tsession run`, never `eval "$(tsession env
  …)"`.** A failed start left Ben's display set in the shell, and what
  ran next opened on his screen. Start what you test with `tsession run
  NAME PROGRAM`, which sets its environment itself.
- **No app binaries from the shell.** Anything but a flag that prints
  and exits opens the app on Ben's screen (the shell has his
  `WAYLAND_DISPLAY`). Read the program's `main.rs` to find one
  (`olo-doctor --report`, `olo-logs --print`), and mind backticks
  inside a double-quoted grep: one ran a binary once. `--help`,
  `-h`, `--version` and `-V` as the first argument no longer open
  anything (an agent ran `olo-terminal --help` and a window opened,
  2026-11-17: the kit now refuses them in `instance::forward` and
  `olo_kit::run`, and Terminal has a `--help` of its own and refuses
  options it doesn't know), but any other argument, a wrong one
  included, is still a launch. When a window did open by mistake: find
  the process you started by its start time and parent (`ps -eo
  pid,ppid,etimes,args --sort=etimes`), check it's yours and idle, and
  end that one PID.
- **A click that opens a link, tested headless: a stand-in for the
  browser.** In a `tsession`, a web or mail link opens through
  `olo_xdg::open`, whose app for `https:` is the session's chosen
  browser, else Olo's Chrome, which may hand the page to the person's
  running Chrome. Give the session a stand-in first: a desktop entry in
  its HOME's `.local/share/applications` (`--home DIR`) whose `Exec`
  writes `%u` to a file, named for `x-scheme-handler/https`, `http` and
  `mailto` in `/var/tmp/olo-dev/NAME/config/mimeapps.list` (the
  session's `XDG_CONFIG_HOME`), then read the file (links in Calendar's
  notes, 2026-11-19). A `tel:` link opens `olo-phone` from the dev
  build (tsession puts `target/debug` first on PATH; give the app
  `OLO_PHONE_DEMO=1` to see a card): an instance socket is per display
  (`{app_id}-{display}.sock`), so it can't reach the person's Phone.
- **Kill test processes by PID, only after checking they're yours.**
  `pkill -f` once killed Ben's real Tracker. Find the PID, check its
  `/proc/PID/environ` names your test session (`olo-dev/NAME`), then
  kill that PID. Chrome rewrites its environment block when it renames
  its process (empty `environ`): tell its processes apart by what they
  have open instead (`/proc/PID/fd`: the screen's own profile folder).
- **`olo-screen stop` leaves processes behind.** After stopping a
  screen, end what had that screen's `OLO_SCREEN` by PID. A screen's
  Terminal once overwrote Ben's `windows.json`.
- **An `olo-screen` older than 2026-11-16's, given an
  `XDG_RUNTIME_DIR` of its own, ends every other Agent Screen on the
  computer.** Its `start` and `stop` end "strays", the screen sessions
  its own folder doesn't list, once two minutes old: under a private
  folder that was everyone's (another agent's screen, in the middle of
  its work, each time tools/dev/readme-shots ran). Fixed in
  apps/screen (`listed_in`: a session started under another runtime
  folder is never a stray), and readme-shots runs the tree's own
  build; never run the *installed* `olo-screen` with another
  `XDG_RUNTIME_DIR` until an upgrade carries the fix. If a screen you
  didn't stop is "stopped", this is why: look at `olo-screen history
  NAME --last 3` for when it was last used.
- **Agent Screens have no sound** (`PULSE_SERVER` points at a socket
  that isn't there), on purpose: an agent never hears or uses Ben's
  microphone. To test what a program does with sound devices, start a
  private sound server (`tools/dev/pw-sandbox /var/tmp/me/pw start`),
  make the devices with `pw-cli create-node`, and run the program on
  your screen with `olo-screen run NAME env
  PULSE_SERVER=unix:/var/tmp/me/pw/run/pulse/native PROGRAM`. Chrome on
  Linux reads its devices when it starts: restart it to see new ones.
- **tsession's clicks and crops are logical; its shots are device
  pixels** (divide by the scale). Anything that talks over a bus needs a
  private bus.
- **The Speed Test without the person's line.** `olo-speedtest --print`
  opens no window, but it is a real test: 16 seconds of everything the
  line carries, each way (over a gigabyte here). To work on the app, run
  `target/release/olo-speedtest --serve 5055 3000000 &` (a server on
  this computer, 3 MB/s a connection; kill it by its PID after) and
  start the app with `OLO_SPEEDTEST_SERVER=127.0.0.1:5055`, or with
  `OLO_SPEEDTEST_DEMO=1` for made-up results. This shell has an
  `HTTPS_PROXY`; the test never uses one, and `curl` needs `--noproxy
  '*'` to measure anything.
- **A kernel module is tried in a guest, never in the person's kernel.**
  `tools/dev/kvm-guest SCRIPT` runs this computer's own kernel in QEMU
  with KVM (`/dev/kvm` is everyone's here; no root), a root made of this
  computer's programs: a start takes under a second. Its script's
  standard output comes back clean (a serial port of its own); the
  console's only on failure with `--quiet`. `ip netns add` needs
  `/var/run` (it's there); `seq` isn't (use `for ((i…))`); a program
  more is `--bin`. An argument can't hold a double quote.
  A disk for the guest is `--disk SIZE[,OPT…]` (`/dev/vda`, `vdb`, …;
  `throttling.bps-write=10485760` makes a 10 MB/s one); a module with
  parameters is `--module "brd rd_nr=1 rd_size=1048576"`. The script's
  standard error isn't shown with `--quiet`: say what matters on
  standard output. `/dev/mapper/NAME` isn't made (no udev): a dm
  device is `/dev/dm-0`. `basename` isn't there (`${x##*/}`).
- **Judging a kernel setting** (kernel/TUNING.md): `tools/dev/kernel-check`
  and `kbench` (crates/system/examples). A guest's timings carry the
  host's noise (a build here, Ben's own work): run each way in turn,
  several guests each, and take the middle; one lazy guest in six did
  40% more pipe messages than the rest. A thread woken by a *timer* on a
  busy processor is switched to at once even by a lazy kernel (the
  interrupt returns to user space, which is where lazy gives way): the
  delay shows only when it's woken from another processor (`kbench
  relay`) or the processor is in the kernel. Memory that's all zeros
  but two bytes compresses to nothing in zram: a hog meant to fill
  memory needs `--random 2048`.
  A USB stick for the guest is `--usb-disk 1G` with `--module
  usb_storage` (it appears a second after the start: wait for
  `/sys/block/sda`); udev runs there as `/bin/systemd-udevd --daemon`
  (`--bin /usr/lib/systemd/systemd-udevd --bin udevadm`, the rule with
  `--file`), then `udevadm trigger --action=add --subsystem-match=block;
  udevadm settle`: how data/udev/61-olo-slow-disks.rules was shown to
  work. A disk's `bdi/max_bytes` reads back a little under what was
  written (16,773,338 for 16,777,216: the kernel keeps a share of its
  whole limit), so compare `strict_limit`, not the bytes.
  `scx_lavd` loads in the guest once the kernel's build options are
  where its loader looks (`--bin /usr/local/libexec/olo/scx_lavd --file
  /lib/modules/REL/config:/boot/config-REL`; without them: "Failed to
  load BPF program: Invalid argument").
- **The next kernel, without installing it.** `tools/dev/kernel-next`
  does it all; by hand: `dnf5 repoquery --available --latest-limit=1
  kernel-core` (as anyone; run it from /var/tmp), `dnf5 download
  --destdir DIR kernel-core-V kernel-modules-core-V kernel-modules-V
  kernel-modules-extra-V kernel-devel-V`, `rpm2cpio X.rpm | cpio -idm`
  for each in one folder, `depmod -b FOLDER RELEASE` (the package's own
  scripts would have), then `kvm-guest --kernel
  FOLDER/lib/modules/RELEASE/vmlinuz --modules-dir
  FOLDER/lib/modules/RELEASE` and `make -C
  FOLDER/usr/src/kernels/RELEASE M=…` for a module. `--ko FILE` only
  puts the file in the guest (`/ko/NAME.ko`): the script loads it.
  The older kernel still installed here (7.2.11) has no headers left
  (`kernel-devel-matched` keeps the newest's).
- **The phone without a phone** (see HACKING, "The phone"): a private
  bus (`target/release/olo-bus unix:path=/var/tmp/ME/bus.sock --ready`),
  `phone-standin --address ADDRESS [--unpaired] [--android]` on it (a
  stand-in BlueZ with a stand-in phone), then `olo-phoned` with
  `DBUS_SESSION_BUS_ADDRESS` and `OLO_PHONE_BLUEZ` both that address
  (its notifications go to whatever bus is its session's: the person's,
  if you leave theirs), `OLO_PHONE_DIR`, `OLO_PHONE_SETTINGS` and
  `XDG_STATE_HOME` in scratch, and `env -u OLO_SCREEN` if your shell has
  one. Preferences ▸ Phone against it: `tsession run NAME
  DBUS_SYSTEM_BUS_ADDRESS=ADDRESS OLO_PHONE_DIR=… olo-preferences phone`
  (**without `DBUS_SYSTEM_BUS_ADDRESS` its first step starts a search on
  the person's real Bluetooth**). `phone-standin --address ADDRESS
  --tell receive "" 22395 "Your Acme Bank verification code is
  482913."` makes a code come; `--tell set messages no` turns the
  phone's switch off. Chrome for it: `cp -a --reflink=always
  ~/.local/share/olo/chrome/VERSION` into a scratch
  `XDG_DATA_HOME/olo/chrome/`, a `current` link and a fresh `.checked`
  beside it; `tsession run NAME HOME=… XDG_DATA_HOME=… OLO_PHONE_DIR=…
  target/release/olo-chrome --user-data-dir=… --no-first-run URL`, the
  page served from 127.0.0.1. A second Chrome with `OLO_SCREEN=agent`
  in its environment (and a profile and HOME of its own) is "on an
  agent's screen" to the service.
- **A test's Passwords can't be unlocked** (it wants the account's
  password, a face or a key; approving without asking wants the
  password too). Run it as on an agent's screen instead, where it's
  open with only what's shared: `tsession run NAME OLO_SCREEN=x
  XDG_STATE_HOME=S XDG_CONFIG_HOME=C target/release/olo-passwords`, with
  a fixture vault (`cargo test --release -p olo-passwords otp_fixture --
  --ignored` and its like, `OLO_FIXTURE_VAULT`, `OLO_FIXTURE_KEY`)
  copied to `S/olo/screens/x/passwords.vault` and its key beside it as
  `passwords.key`; the test Chrome gets the same `OLO_SCREEN`. Its
  socket is the session's (`$XDG_RUNTIME_DIR/olo-keychain-wayland-N.sock`),
  asked with `target/release/examples/olo-keychain-ask` (which answers
  only the kind). And **`cargo build -p A -p B --example E` builds only
  the example**: the programs stayed old, and a test Chrome ran the old
  extension. Build the programs in a command of their own.
- **A test session shares the person's runtime directory**
  (`env.json`'s `XDG_RUNTIME_DIR` is `/run/user/1000`), so a program in
  it that asks Passwords asks the person's own (the socket is found
  there): a click on something that lists logins (Integrations ▸
  Google ▸ Add a Google Account…) would read their vault and could ask
  them to unlock it, on their screen. Give the program a stand-in's
  socket (`OLO_KEYCHAIN_SOCKET=PATH`: a few lines of Python answering
  `{"type":"shares",…}` with made-up items, as
  `apps/preferences/tests` and DiskBackup's tests do), and never press
  what would start a real agent (Start the Agent) in a test. The phone
  service is kept out by `tsession` itself: each session has its own
  `OLO_PHONE_DIR` (in `env.json`, and given to what `tsession run`
  starts), so a test's Chrome host or Phone finds no phone service. A
  program started outside `tsession` needs it set by hand (a test's
  Chrome told the person's phone service "a browser is listening",
  2026-11-19, before that).
- **Run the tests, read their result, then commit: never in one
  command.** `cargo test … | grep …; git commit …` commits whatever the
  tests said (a commit went in with a failing test, 2026-11-17, caught
  before it was pushed). And a test that writes a version or a count
  out again fails at every change of it (the extension's version, twice):
  have it read the thing itself.
- **The layout lint lays out a panel only as its test state shows
  it.** Preferences ▸ Bluetooth went out with its last button cut off
  (a phone chosen shows four buttons) because the lint only ever saw it
  with nothing chosen. A panel whose controls change with what's chosen
  starts, under `cfg!(test)`, at its fullest: the most buttons, the
  longest words (Bluetooth's chooses its test phone).
- **What a lint window holds, in a language**: a throwaway test that
  calls `Lint::trees` and prints each tab's nodes (role, name, y,
  height) shows which control runs lowest or widest, where the lint's
  finding only says "make it 752 tall". Run it as the lint is built
  (`OLO_LANGUAGE=fr cargo test -p olo-preferences --features
  olo-kit/lint NAME -- --nocapture`; `--release` builds a whole other
  profile). A window at 736 with a `ScrollBar` "Scroll Bar" in its
  tree is taller than the 1280 × 800 screen allows, its Frame
  scrolling it: French's keyboard hints ran a line longer than
  German's. A finding the lint makes only in one language with a
  fixture's name in it (a volume, a keyboard) can be the fixture
  translated: a name the system gives is never looked up, so keep it
  as itself in the catalog.
- **A window's frame, cropped from a shot, holds more than the window**:
  beside its tab, above the window's body, is whatever is behind it. A
  French Preferences window seemed to have a Spanish menu bar there
  ("Visualización"): it was Logs', started before it in Spanish. Crop
  from the content's top (`list_windows`' `content`) to judge a window.
- **A finding the live lint made once, at every look, and never again**
  can be a race at the app's start: the lint changes the scale as soon
  as the app answers, and app_server may not have its first frame yet.
  To make such a race happen every time, hold the frame: relay the
  app's Wayland connection and hold its first `wl_surface.attach` of a
  buffer (`tools/dev/first-frame-check`'s relay, from the olo-debug-help
  investigator, 2026-11-19). The same app reviewed in an ordinary
  session at the size it was given (drag it to its least) shows the
  finding as the lint saw it.
- **A service in a program that also has a window pays for the
  window's kit**: linked beside `olo-phone --service`, it cost 3.5 MB
  at rest for nothing it used. A service that runs all session is a
  program of its own (`olo-phoned`, `olo-termd`): a second `[[bin]]` in
  the crate that doesn't use the kit doesn't link it.
- **A stand-in must hand over what the real thing does, not what's
  convenient.** The stand-in BlueZ passed ordinary blocking sockets;
  BlueZ's are non-blocking (and packet sockets when it connects over
  L2CAP), and the phone service worked in every test and failed at its
  first read with Ben's iPhone. When a stand-in passes a descriptor, a
  buffer or a handle, read the real program's source for how it made
  it (flags, type, who closes it), and check the stand-in reproduces a
  failure by taking the fix out once.
- **`tsession click` takes logical points, not the screenshot's
  pixels**: at 1.5× a button at (1204, 616) in the shot is clicked at
  (803, 411). A click that seems to do nothing: divide by the scale
  before suspecting the app.
- **Never call a blocking zbus method from inside a zbus interface
  method on the same connection**: the thread that would read the
  answer is the one waiting (the pairing agent did, and every question
  a device asked hung). Take `#[zbus(connection)] conn: &zbus::Connection`
  and await the call.
- **`cargo test` with `OLO_TEST_BUS` set runs every bus test at once**
  on that one bus (dnf-fake's failed once so, among the phone's): give
  the variable to the test you mean (`--test NAME`), not to a whole
  crate's run. **Without it those five tests pass by doing nothing**
  (`cargo test --workspace` and the hooks never run them): after
  changing the phone, the network portal, disks or updates, run its
  own (`OLO_TEST_BUS=unix:path=… cargo test --release -p olo-phone
  --test service`; `-p olo-system --test phone_standin`, `nm_portal`,
  `udisks_fake`, `dnf_fake`) against an `olo-bus` of your own. The
  phone service's had failed since the day its step went stale
  (2026-11-17), unseen.
- **A drag held a second over a folder springs it open** (Tracker): a
  picture taken "mid-drag" after a long hold is of the new window, and
  everything after it lands on the wrong one. Hold under half a second
  for a picture; and a second Tracker window in a test session is a tab
  in the first's frame (same place, the last one in front), so rows
  read from the tree by title may be of a window that isn't showing:
  one window a test, started fresh, its view set
  (`{"invoke":"view-list"}`) after every start.
- **Terminal is one program per session, a test session too**: a
  second `tsession run NAME olo-terminal` hands its window to the one
  running and exits, so the PID `run` printed is gone and the window is
  the first one's, built from whatever binary that was (a measurement
  of a new build read nothing: the old one was still running). Stop the
  one that has the windows (`list_windows`' `pid`) before starting a
  new build.
- **In a test session ⌘ is Ctrl** (`tsession key NAME 29 33` is ⌘F in a
  kit app; Terminal's are with Shift too), and a text's line is in
  StyledEdit's status ("Line 10, Column 20"), which says where a find
  landed without a picture.
- **The commit hook's layout lint runs over the working tree**, not
  what's staged: committing one thing while another's panel is half
  done fails on that panel (a new Preferences panel needs the settings
  index made again first: `OLO_WRITE_SETTINGS=1 cargo test -p
  olo-preferences --features olo-kit/lint ui_lint_settings_index`).
- **Icons are drawings; the tools that look at them need `svgicon`
  built** (`cargo build --release -p olo-xdg --example svgicon`:
  `icontrace.py review` and `compare` render with it, and show a blank
  where it's missing or stale). `review OUT.png NAME…` is the sheet to
  judge a drawing on (128 px and every small size, light and dark);
  `svgicon --theme platinum IN.svg OUT.png SIZE` shows it in a theme's
  colours. A drawing is checked by `cargo test -p olo-xdg svg` (on the
  grid, renders, within 2 to 62 of the canvas: what reaches the edge is
  cut off, as Activity Monitor's arrow was).
- **Start a drawing from `tools/dev/iconforms.py`**, not from nothing:
  a box, a plate, a badge, the page, the CRT and the rest are there,
  and every drawing made of them agrees with the set. In a plane's
  group (`FRONT`, `PLATE`) a stroke is scaled with the plane, so an
  outline (`class="o"`) there is uneven: outline a form in screen
  coordinates (`rounded`, `slab`) and draw only its faces in the plane.
  A shape `extrude`d and then mirrored needs its points reversed.
- **Anything cargo runs keeps its rendered icons in
  `/var/tmp/olo-test-cache/icons`**, not the person's cache
  (`svg::kept_dir`): safe to delete.
- **This computer's built-in display is usually in HDR**, where its
  backlight (`/sys/class/backlight/intel_backlight`) does nothing: a
  brightness that "doesn't change" there is SDR's white not moving, not
  logind refusing the write. Never try it on the real panel (it's
  Ben's screen): `tools/dev/brightness-check` has a headless screen
  stand in for it, in HDR and out, and a copy of the installed
  `olo-app-server` in a scratch `CARGO_TARGET_DIR/release` runs the
  same check on the build Ben is running, to see a report fail there
  first.
- **Is a window showing the pixels it was given?** Render what it
  should show yourself and compare with a `tsession shot`, with PIL
  (`/usr/bin/python3 -I`): `ImageChops.difference` and its mean and
  max; and where they differ, strip by strip, which shift (−1, 0, +1)
  fits each strip best. A stretch shows as an error growing across a
  tile and then a clean one-pixel shift; a dropped column as a step
  part of the way across; a stale render as no scale fitting at all.
  An exact reference for a picture drawn smaller is
  `src.resize((W, H), Image.LANCZOS, box=(0, 0, W / k, H / k))`. The
  page's place comes from the app's own tree (`{"tree": true}` on
  `olo-menus-PID.sock`: only the PID you started; the session's
  runtime folder is the person's own, with every app's socket in it)
  plus the window's content origin (`tsession ipc NAME
  '{"type":"list_windows"}'`). A fit zoom that isn't clamped isn't a
  round number: work it out from the tree, or search for it. Viewer at
  1×, 1.5× and 2× showed three different faults this way (2026-11-16).
- **A deadline in seconds on the wall isn't one on a busy computer.**
  `tools/dev/ui-lint` runs at idle priority; with other work going (a
  load of 38 on 15 processors: a build, another agent's tests) its apps
  hardly ran in the 20 s they had, and a push was refused for "Kache:
  no window came up". Its waits now leave out the time the app and its
  window server were ready with no processor to run on
  (`/proc/PID/schedstat`'s second number, `kept_waiting`). Do the same
  for any wait on something that runs at low priority.
- **Two files written one after the other can have the same time.** A
  file's time comes from the kernel's coarse clock, a tick (1 ms) wide:
  a test that writes two files and expects the second to be newer fails
  once in a great many runs (Doctor's start-up chime test did,
  2026-11-16). Set the times the test goes by (`File::set_modified`).
- **Probing the kernel from a module: what bit.** A kretprobe is skipped
  for a call that arrives while another probe's handler runs on the same
  processor (an interrupt's TCP work during a send's): the self-check's
  first truth probe missed one transmission in about 100,000 and blamed
  the module. An fprobe (ftrace's) isn't. An `int` a probed function
  returns is in the low half of the register only: `(int)` it. A netem
  or tbf qdisc on the *sender's* own interface changes what TCP does
  (its own queue limits see the held packets): shape on a router
  namespace, and let `tbf` cut TSO packets to the wire's size there.
- **This kernel's TCP, for whoever touches kernel/bbr3.** `tcp_rate_gen`,
  `tcp_rate_skb_sent` and `tcp_rate_skb_delivered` are inlined (no
  symbol); `tcp_mark_skb_lost`, `__tcp_retransmit_skb` and
  `__tcp_transmit_skb` are there. A FIN takes a sequence number and no
  packet when it rides on data. A packet's timestamp is set even when
  its sending then fails. `tcp_bbr_info` can't grow from a module (the
  kernel copies a fixed union). The reference kernel with Google's
  patches is five minutes to build (kernel/bbr3/README.md); its source
  and build were in /var/tmp/claude-bbr/ref, which is scratch.
- **What a test session's app was sent and what it holds.**
  `tsession run NAME WAYLAND_DEBUG=1 olo-APP` writes every Wayland
  message the app gets to `/var/tmp/olo-dev/NAME/olo-APP.log`
  (`wl_keyboard#31.key(serial, time, code, state)`). A kit app's
  widgets, with their values, come from its own socket: `{"tree":
  true}` on `olo-menus-PID.sock` in the session's runtime folder
  (`env.json`'s `XDG_RUNTIME_DIR`). Keys go down and up apart with the
  session's IPC (`{"type":"synthetic","events":[{"type":"key","key":
  CODE,"pressed":true}]}`), and its loop is held with
  `{"type":"hold_loop","ms":N}` where the session was started with
  `OLO_TEST_SEAT=1` in the environment (headless takes no seat from
  it). tools/dev/key-repeat-check does all four.
- **cargo test can wait behind kache.** Build with `cargo test -p CRATE
  --no-run --message-format=json` and run the test binaries it names
  directly. Never pick a binary with `ls -t deps/`: that once ran the
  Coder app.
- **Test scratch folders need names of their own.** Two tests in one
  binary that both used `/var/tmp/olo-doctor-kept-PID` removed each
  other's files as they ran in parallel (16 of 20 runs failed). Put the
  test's own name in the folder's.
- **Test Passwords' stand-in PAM is debug-only.** A release build once
  checked "secret" against Ben's real account. Never point a release
  build at the real PAM in a test.
- **Look before trusting "the same".** A pixel compare can be of a
  leftover overlay, or of the wrong window. Open one shot of each batch.
- **Alternating builds** (old, new, old…): Terminal reuses a running
  process, and termd starts it again within 2 s, so end both; Ctrl+digit
  switches workspace; compare drawing with `OLO_DAMAGE=0`
  (`tools/dev/damage-check`); where a measurement and the code's own
  description disagree, find out why before trusting either.
- **Watch memory on live tests.** Sample GPU and RAM through any test
  that runs per frame for long: an atlas leak once took 35 GB.

- **Never `pkill -f PATTERN` (or `pgrep -f`) with PATTERN in your own
  command line.** It matches the shell running it: `pkill -f
  "holdwatch.py 3887902" -n` killed its own shell (exit 144,
  2026-11-16). Stop what you started through its task, or by the PID
  you recorded when you started it.
- **A long capture's last lines are not flushed until it ends.** A
  Python sampler printing with `flush=True` only on its heading lines
  left the details in its buffer; flush every line you may want to read
  while it runs.
- **Don't split a tool's output on spaces in a `for` loop.** `for t in
  $(… | python3 -c "print('ERR', message)")` turned each compiler
  message into a hundred "test binaries"; write the list to a file, one
  item a line, and read that.

- **An Agent Screen's `--size` is in points**, its pictures device
  pixels: `olo-screen start NAME --size 1920x1080 --scale 2` is a 4K
  screen (3840 × 2160 asked at 2× gave 7680 × 4320 shots). app_server's
  IPC (settings, `place`, `overview`, `list_windows`) is on the screen's
  `OLO_SOCKET` (`olo-screen env NAME`), a line of JSON each way:
  tools/dev/readme-shots drives a screen with both.
- **A test binary can be seven folders under the repository**
  (`target/debug/build/CRATE/HASH/out/`, as kache lays them out), and
  `olo_xdg::data_dirs` found the tree's `data/` only four up (now
  seven): tests that read data read the *installed* copy instead, so
  the veil's passed on files that had gone from the tree. A test that
  must read the tree's data regardless: `OLO_DATA=$PWD/data`.
- **The system Python has PIL and nothing else** (no numpy, fontTools
  or torch). A venv for a one-off: `uv venv /var/tmp/NAME` and `uv pip
  install --python /var/tmp/NAME/bin/python …`; the GPU's is
  `/var/tmp/olo-sr` (torch, spandrel, numpy, pillow:
  tools/finishwallpaper.py's header). A later `uv pip install` of
  something that depends on torch can replace the torch you installed
  (spandrel's moved 2.11+cu128 to 2.15+cu132): look at `torch.__version__`
  after.
- **A program started while another has the keyboard doesn't take it**
  (app_server's rule: a window comes forward for input in its own
  program, not by itself). In a tsession, `tsession run` a second
  program and `tsession key` still types into the first: activate the
  new window (`tsession ipc NAME '{"type":"activate","id":N}'`, the id
  from `list_windows`), or end the first, before sending keys.
- **Token Pool in a test session**: `tsession run NAME env
  OLO_POOL_PORT=27891 olo-pool --service` (a port of its own: 17891 is
  the person's pool; the proxy's port stays theirs, so it has no proxy).
  `curl http://127.0.0.1:27891/pool/status` says its `drain` (`beside`:
  another copy at work beside it). `tsession reload NAME` makes it hand
  over to a copy of itself, changed binary or not, in about half a
  second: the copy is a child of the one handing over (found in
  `/proc/PID/task/*/children`: the file lists what that one *thread*
  started, so `task/PID/children` alone misses it), then of the first
  one, which stays as the keeper (`olo-pool` with no arguments). To
  hold a copy mid hand-over, `SIGSTOP` it as its child appears. End
  them with SIGTERM to the keeper's pid.
- **A hung program's stacks before it's killed**: `gdb -p PID -batch -ex
  "thread apply all bt 14"` (a second or two, and the program runs on).
  Token Pool hung for six hours (2026-11-18) and was killed with nothing
  to read; the cause had to be found from the code and made to happen
  again in a test. `flock` holders are per opening of the file, so two
  threads of one program wait on each other through it like two
  programs, and olo-sync's cycle check doesn't see it.
- **A test that starts a program ends it in a `Drop`**, not with a
  `kill()` at the end: a failed assert unwinds past that, and a
  `Child` isn't killed when dropped. DiskBackup's `set_up_drive` left
  a service running for hours that way (2026-11-18, found holding an
  inotify instance); its `Running` and `Scratch` guards end the program
  and remove the folder however the test ends.
- **Inotify instances run out with many screens** (2026-11-18): the
  kernel allows an account 128, and each agent screen's Deskbar,
  Tracker, Notifications, Weather and portal take one each (Tracker
  two), each Chrome two: with ten screens open 125 were taken, and
  kit-base's watch tests failed with `watch(…).unwrap()` on None. This
  computer has 8,192 now (`/etc/sysctl.d/90-olo-inotify.conf`, which
  Olo Doctor's `inotify-instances` writes). If watches fail again, count
  them: `ls -l /proc/PID/fd | grep -c anon_inode:inotify` per process.
- **A kit program's tree lists what was painted, not only what shows**:
  a scrolled drawing's nodes outside the view are there too, with a
  `y` above or below it (the scroll view's band). Filter by the
  window's height before saying what's on screen.

### DiskBackup's window in a state of your choosing

`OLO_DISKBACKUP_DIR=DIR olo-diskbackup` (in a test session) reads its
settings from `DIR/diskbackup-settings.json` and what the service is
doing from `DIR/olo-diskbackup.json`, so a window in any state needs no
service: write both (the state's `pid` a process of yours that's alive,
the app's own will do: with 0, or one that's gone, the window says
"DiskBackup isn't running"). A test session runs no DiskBackup service
(it's one of the real session's), so nothing there backs anything up.
For the service itself, point HOME, every XDG folder and
`OLO_DISKBACKUP_DIR` at scratch and give `OLO_DISKBACKUP_KEY` (64 hex):
`--set-up-folder` keeps its key in the login keyring, so don't run it
in a test.

### Pictures of a window, and what has caught them out

Learned making To-Do (2026-11-19 and 20):

- **A session's screen is in device pixels.** `tsession start NAME
  1280x800` at the default 2× is a screen of 640 × 400 points, on which
  every window is in its narrowest layout (Calendar and Weather showed
  no side panel, and a before-and-after "differed nowhere"). For the
  smallest screen at 2×: `2560x1600`.
- **app_server remembers a window's size for the session.** Change an
  app's first size (`WindowOptions::new`) and its window in a session
  that had it open comes back the old size: stop the session and start
  it again.
- **A made-up clock is the app's only if the app asks nothing else.**
  With `OLO_TODO_NOW` (or `OLO_CALENDAR_NOW`) a window believes it's
  another day, and anything stamped by the real clock
  (`Timestamp::now()`) is then of another day than the picture's: a
  to-do checked off in pictures of the 19th read "Done tomorrow" once
  the real date had moved on, and both reviewers took it for a bug of
  time zones. Stamp through the same function the window reads the
  time with.
- **Run the layout review at 1.25× and 1.75× too, in every state you
  picture.** Clean at 1×, 1.5× and 2×, To-Do had six kinds of finding
  at the other two (and one at 1.5×): a menu field's words a pixel left
  of a text field's under it, a label of another size beside a row's
  title a pixel off its baseline (`style::baseline_at`, then
  `label_on_baseline`), an icon a pixel off the name beside it (centre
  it on the snapped baseline less half the capitals), rows of 30
  points landing on 52.5 device pixels (make a row a whole number of
  them at the scale: `(points * scale).round() / scale`), and a line
  drawn at the top of one flex child a pixel under the bottom of the
  one over it (two siblings' shared edge isn't one pixel row at every
  scale: draw the line from the pane both are in, `view::foot_line`).
- **Reviewers' pictures: absolute paths, one command each.**
  `codex-look` and `opus-look` run from the pictures' folder; started
  together in one shell line with `&`, one ran from the repository.
  Run each as its own background command.
- **A bare `TextField` shows nothing until it has a size.** One line:
  a container of a set height with the field in it `absolute().
  inset_0()`. Several lines: `.scrolled_by(&handle)`, in a `ScrollView`
  of that handle. Its hint is drawn in the faint colour: for a row
  whose hint is what the row is for, draw your own label over it while
  it's empty.
- **A `ScrollView` that can scroll across shows its bar always.**
  `.horizontal(true)` reserves and draws the bar whether or not what's
  in it is wider: ask for it only when it is.
- **Keys reach a view's `on_key_down` only through what has the
  keyboard.** A handler on a plain `div` hears keys a focused field
  under it lets through, and nothing once no field has the keyboard:
  give the div a `FocusHandle` (`track_focus`) and focus it where the
  person presses (To-Do's ↑ ↓ and Space did nothing until then, and no
  picture shows that: try the keys).
- **Look for what a picture can't show.** To-Do's board had no field
  to type a new to-do in at all (its + focused a field that wasn't in
  the window): nine review rounds of pictures passed it. Drive each
  thing the window offers once (`tsession click`, `type`, `key`) and
  picture what happens.

### What a window does on the way: pictures mid-move, and the wire

Learned making windows snap and glide (2026-11-20):

- **Picture the middle of anything that moves.** A glide's first and
  last frames were right and every frame between was the window's old
  picture stretched to the size on the way (the kit committed a size
  before its frame: HACKING, "A new size is committed with its frame").
  Only a picture taken 30 ms in showed it: send the move and ask for the
  `screenshot` in one script, through tsession's own `ipc` and `synth`
  (load `tools/dev/tsession` as a module, as `tile-qa` does: a call is
  under a millisecond; `tsession move` from a shell is 60 ms, and the
  glide is over by the third).
- **`WAYLAND_DEBUG=1` on one app says who did it.** `tsession run NAME
  WAYLAND_DEBUG=1 olo-weather` puts every request and event in the app's
  log in the session's folder (the kit's own client prints them, Mesa's
  queues too, marked `{mesa …}`). `configure`, `set_destination`,
  `ack_configure` and `commit` with no `attach` between was the whole
  bug, read in a minute, after an hour of counting frames from outside.
- **`list_windows` says what's drawn, `cost` what the app drew.** A
  window's `frame` there changed thirty times in a glide while
  `tools/dev/cost` counted four frames of the app's own: the two
  disagreeing was the stretch (sizes committed without frames).
- **A held drag draws on.** While a window is held by its tab the
  displays don't rest (`keeps_awake`, by design: the next move shows at
  once): 120 looks a second that find nothing new, a quarter of a
  millisecond each. Count a drag's frames against that, not against 0.
- **`tsession key` takes evdev codes** (1 is Escape, 45 X), not names.
- **A place's letter can't be timed from a shell either**: a check that
  must act mid-glide belongs in app_server's tests or isn't made; what
  `tools/dev/snap-check` does is wait for each frame to settle
  (`settled`) and read it.

### Keys in a test session: together, in turn, and what a made-up press skips

Learned making shortcuts of two parts (2026-11-20):

- **`tsession key` is one chord**: every code down, then every code up.
  Keys in turn with something still held (⌃⌥⌘ held across two letters)
  are one `synthetic` request with the key events in order
  (`{"type":"synthetic","events":[{"type":"key","key":29,"pressed":true},
  …]}`, as `tools/dev/chord-check`'s `events` sends them): app_server
  takes them one after another with nothing between, which is also how
  "typed straight through" is tested (two `tsession key` calls are 100
  ms apart, a person's pause).
- **A made-up pointer press isn't a real one's code.** Real input goes
  through `pointer_button`, `synthetic` events through
  `synthetic_input_inner`: a second, shorter path. What's hooked on a
  press (a waiting group of shortcuts let go of) has to be hooked on
  both, or it works for the person and not for the test, or the other
  way round. The same for keys' side effects: look at both paths.
- **`{"type":"layer_clients"}` says which layer windows are up** (pid
  and namespace: the chord panel's is `chord`), the way to check that
  something app_server started for a moment (a panel, a banner) came
  and went, without a picture.
- **A check that toggles must leave no toggle behind.** One failed step
  that left a group waiting made the next three fail for a reason of
  their own (the first chord pressed again picked from the group).
  Read the first failure; the rest are usually its echo.
- **`olo-preferences PANEL --tab NAME`** opens any panel on that tab
  (it did only for Network, Power, Performance and Security until
  2026-11-20; elsewhere the tab was silently ignored).

### A program with no network, without root

`unshare -rn PROGRAM` runs it in a network of its own with only
loopback (a user namespace: no root needed): no way out, no wire, no
Wi-Fi, as its netlink sees it (`/sys/class/net` still lists this
computer's). DiskBackup's "Only when connected to Ethernet" was tried
so (2026-11-19).

### `/proc/PID/environ` is what a program was started with

It never changes while the program runs (not when the program sets a
variable itself, not when the session's environment is set), so
nothing that judges by it can be fixed without restarting that
program. Olo Doctor's language finding stayed after "Set the Language"
said "Fixed." for that reason (2026-11-18). To see what's started on
demand gets, ask systemd's user manager
(`olo_system::env::manager_environment`); to see what a program hands
down, read the code that starts its children. And when writing a
Doctor check, try its fix and look again: the finding must be gone.

## Measuring

- **Count instructions unscaled: `perf stat --no-scale`.** This
  machine's cores are of two kinds (`cpu_core`: CPUs 0–3, `cpu_atom`:
  4–15), each counting on its own, and a thread is counted only on the
  kind it's running on. perf's default scales each count up by the time
  it wasn't counting, so a thread that moved between kinds was counted
  up to twice, at random: the same Logs build read 5.0 or 9.4 M
  instructions a frame. Sum both kinds' counts. For a benchmark of the
  code alone, pin it: `taskset -c 1 perf stat -x, -e
  cpu_core/instructions/u,cpu_core/cycles/u BINARY` (not CPU 2: Olo's
  realtime threads have it) repeats to within a few hundred
  instructions.
- **Measure with a great many of whatever the window lists.** To-Do
  with its demo's fifteen to-dos cost nothing anywhere; with 5,000
  (a store made by a script: a vdir is files), choosing a card on a
  board of 900 took 40 ms a frame, and checking a to-do off 7. What it
  was: something found by its name inside per-row code (`Book::item`
  is a search of every to-do, and each card's key and drawing asked
  it: find by place, as rows do), every card made whether it showed or
  not (`sized_rows`), each card's wrapped title measured again at each
  `body` (keep the layout, keyed by what it's made from), the sidebar's
  counts counted at each `body` (count in the one place the book
  changes), a sort that worked out its key at every comparison, and a
  copy of the whole book made to pretend one to-do was still open.
  `tools/dev/cost PID` while `tsession wheel`, `key` and `click` drive
  it says which; time the parts with `Instant` in a release build to
  find where a frame goes. And this computer is rarely idle (a load of
  ten is common): one slow frame in a run is as likely another
  program's doing; measure thrice.
- **Measure in absolute terms**, against a frame's budget (8.3 ms at
  120 Hz), CPU at rest and while animating, and latency; compare a
  change with the build from just before it, never with another toolkit
  (CLAUDE.md).
- **Measure browsers alone.** Olo against Chrome: each with the machine
  to itself, from `/proc/PID/stat`, the load average checked first;
  `tools/dev/cost` misses short-lived threads.
- **A stall is traced in the kernel, as root** (Ben gave sudo for it,
  2026-11-16: ask). `sudo tools/dev/gpu-holds` names who holds the
  buffer lock app_server's GPU submission waits for, with the holder's
  kernel stack; `/proc/PID/task/TID/wchan` and `stack` (root) say where
  a thread waits. bpftrace: `kretprobe` on a module's function may
  never fire here (xe's didn't): use `fexit:MODULE:FUNCTION`, which also
  gives typed arguments (`args.shrink->private_data`).
- **`swapoff` alone doesn't last.** systemd turns a swap area back on
  the next time any system service starts (swap's target wants its
  unit: off at 11:47, on again at 11:58 with PackageKit). To keep swap
  off for a measurement: take the unit away first (an empty
  `/etc/systemd/zram-generator.conf`, `systemctl daemon-reload`), then
  `swapoff -a`; and look at `/proc/swaps` at the end of the run, not
  only at its start.
- **Run a Doctor fix for real before calling it done**, as root the
  way Doctor does (`sudo target/release/olo-doctor --fix ID`: the steps
  run there, the scan is the sudo user's; what the fix does after as
  the person, `after_fix`, doesn't run under sudo: do that by hand, as
  `systemctl --user daemon-reload`). Three of this day's fixes passed
  their tests and failed on the machine: a module SELinux wouldn't let
  a service load from /var/lib; `swapoff` undone by systemd eleven
  minutes later; a zram device whose reset raced udev, with Doctor
  saying "done" over two failed units. Look at what the fix was for
  (`/proc/swaps`, the unit's state, the limit), not at its exit status.
- **PipeWire's `mem.mlock-all` reads in everything it has mapped**
  (`mlockall` without `MCL_ONFAULT`): 40 MB become 260 to 310 for each
  of its three processes. Try a sound setting in `tools/dev/pw-sandbox`
  first (its config folder takes drop-ins before `start`).
- **The NVIDIA GPU's float32 ceiling is 11.2 TFLOP/s** (an RTX PRO
  2000, held to 60 W: about 1,700 of its 3,090 MHz). A network that
  runs at that with the GPU at 100% is already as fast as it goes in
  float32, whatever the code (FBCNN: 45 TFLOP for a 4K picture, 4.2 s);
  half precision doubled it. `nvidia-smi` isn't installed: `uv pip
  install nvidia-ml-py` and `pynvml` give the utilisation, watts, clock
  and limit. What made a GPU job slow here was the processor's work
  between its steps (a third of each picture's time) and running the
  network where its result was thrown away: count a model's FLOPs (a
  forward hook on its convolutions) and the card's rate before
  rewriting anything.
- **Lossless WebP's slowest setting buys nothing**: method 6 at quality
  100 took 22.8 s for a 4K picture and method 4 at 75 took 2.9 s, to
  the same 3.67 MB.
- **Check which scheduler runs before measuring.** scx_lavd (sched_ext)
  ran from 2026-11-10 to 2026-11-15 02:30 and is disabled since (the
  kernel's EEVDF): look at `/sys/kernel/sched_ext/state`.

- **Resident memory has two parts**: `VmRSS` counts a program's own
  pages (`RssAnon`) and the pages of its file it has touched
  (`RssFile`: its code and tables, shared and reclaimable). Linking a
  big crate grows `RssFile` (Tracker 58 → 66 MB when the text
  recogniser was linked) without the program holding any more. Read
  both in `/proc/PID/status` before calling a change heavy.
- **A cargo test isn't a kit app's allocator**: a kit app has four
  malloc arenas (`few_arenas`), a test binary one a thread. What stays
  resident after a burst of work differs a lot between them: measure
  memory in the app, in a headless session.

- **Pictures of Terminal with coding agents** come from
  `tools/dev/readme-shots terminal-agents`: stand-ins named `claude`
  and `codex` on a PATH of their own (Terminal knows an agent by its
  program's name), which title their tab with `olo-terminal
  --tab-title` and print what the hook tells an agent there.
  `OLO_SHOTS_DEBUG=1` prints the tree and keeps a picture at each step
  of a scene being made (`debug()`).
- **A Terminal with programs running asks before it closes**, and
  olo-termd starts Terminal again while its shells live. A script that
  closes windows and then ends what's left gets a Terminal back: end
  the programs in its tabs first (by pid), then close.
- **Where a key goes**: `WAYLAND_DEBUG=1` on the program (its stderr to
  a file) shows `wl_keyboard.enter`/`leave` and `xdg_popup.grab` in
  order; that is how the submenu's lost keyboard was found.

- **A long build is lowered, and so are the tests it runs.**
  olo-supervisor puts background work (cargo, rustc, and what they
  start) in the batch class with its nice raised once it has been busy
  a while. A test that passes alone and fails in `cargo test
  --workspace` may be reading its own class or nice: run it as lowered,
  `chrt -b 0 nice -n 12 cargo test -p CRATE` (and `chrt -i 0 …` for the
  idle class, which nothing leaves without privilege). A nice only
  goes up: a test asks for one above what it has, never a fixed one.

- **A click's cost: check every click lands where you mean.** Picture
  the window before measuring clicks, and again if anything about it
  changes. Calendar's week opens scrolled to an hour that follows the
  time of day, so the same points hit an event in the evening and an
  empty slot after midnight (2026-11-19: half the clicks of a "7.0M a
  click" run let go of the selection, a cheaper frame; on events it was
  15.4M). Scroll to the top first, then down a known number of notches
  (one notch is an hour there), and say in the numbers what was
  clicked.

- **Night mode isn't in pictures.** The night light scales the screens'
  gamma ramps at scan-out (`crates/app_server/src/night.rs`), so
  `tsession shot` in night mode is the dark picture. To judge night,
  multiply a dark picture's channels by the night light's gains (its
  3,600 K default: red ×1.0, green ×0.769, blue ×0.585; the formula is
  `night::white_point`).

### What a change costs to build, measured without touching the person's builds

Learned measuring catalogs as data (2026-11-20):

- **A copy of `target/release` is free here** (`cp -a --reflink=always
  target/release /var/tmp/YOURS/target/release`, with
  `target/.rustc_info.json`: btrfs shares the 33 GB) and cargo takes it
  as fresh (`CARGO_TARGET_DIR=/var/tmp/YOURS/target cargo build
  --release --workspace`: 13 s, only what the working tree changed).
  Then make the edit to measure and build again there: the time and
  the `Compiling` lines are what the edit costs, and the session's own
  `target/release` (which it watches to offer an upgrade) never
  changes. Put the edit back, remove the copy.
- **A test built elsewhere reads the tree's data** now
  (`olo_xdg::data_dirs` takes the package's folder from cargo at run
  time): before, with `CARGO_TARGET_DIR` set, a test read the
  installed icons and would have read the installed words.
- **A background job's last command is its exit code**: a `grep -c`
  that counts none ends 1, and the job is reported failed with every
  number in its output right. Read the output before believing either.

### Reading a day's logs

Learned on the second pass (2026-11-20):

- **`olo-logs --print --since 1d --level warning --all-programs`** to a
  file under /var/tmp, then grouped by program and by the message with
  its numbers taken out, is the whole method; `--program olo-app-server
  --level info` around a moment says what else was going on. Keep the
  file local: it has the person's programs' lines too.
- **The journal has every test session's programs in it**, and tests'
  own (PAM's lines from `cargo test -p olo_system`, a reviewer's crashed
  test binaries): a warning there may be a dev build's (the kit's
  "menu keys not in …" is said only with debug assertions). Check the
  pid against the person's session before calling it theirs.
- **Who wakes a sleeping GPU**: watch its `power/runtime_status` (a
  sysfs attribute: reading it doesn't wake it) every 50 ms and, the
  moment it leaves `suspended`, list the processes that have
  `/dev/nvidia*` open or its libraries mapped, and those started in
  the last seconds (`/proc/PID/{stat,fd,maps}`). Fifteen minutes of it
  showed the kernel log's "Enabling HDA controller" lines were another
  program's, not Olo's. Don't test this by running Olo's programs and
  reading the status after: something else wakes it meanwhile.
- **A test that stashes or checks out to compare can't be run while a
  reviewer reads the tree** (`codex-help`): compare by copying the one
  file aside and putting it back, or wait.

## Ben's computer

- **Don't probe sudo.** Even `sudo -n` runs face unlock (the IR camera)
  and fprintd. Anything that needs root goes to Ben (`! command`), or to
  Olo Doctor's helper, which asks through polkit. (Unless he has said
  to use it for the task at hand, as on 2026-11-16 for the stalls: then
  only for that.) A fix's steps can be run as root as Doctor's own
  helper runs them, without the window: `sudo target/release/olo-doctor
  --helper` reads a line of JSON, `{"Run":{"id":1,"steps":[…]}}`.
- **Kernel facts** (7.2.15-300.fc45, 2026-11-16): Secure Boot is off
  and lockdown none, so unsigned modules load; live patching is built
  in; SELinux is enforcing, and a system service may load a module only
  from `/lib/modules` (`modules_object_t`), never from `/var/lib`;
  `kernel-devel-matched` is installed (NVIDIA's akmod needs it), so
  each new kernel brings its headers. Olo's live patch
  (`olo_gpu_memory`, HACKING "Windows' buffers stay in memory") is in
  since 12:23 that day, by `olo-gpu-memory.service`; bpftrace was
  installed for the tracing.
- **Memory, as set on 2026-11-16** (Doctor's fixes, run as root at
  Ben's word): swap is 62 GiB of zram (`zram-size = ram`), swappiness 1
  (`/etc/sysctl.d/80-olo-memory.conf`), the account's locked-memory
  limit unlimited (the limits file, and raised in place for what was
  running), PipeWire's three services never swapped (drop-ins in
  /etc/systemd/user). uresourced gives `session.slice` 250 MB that is
  never reclaimed (`memory.min`), which is where PipeWire and
  WirePlumber run; Olo's own programs are in the login's
  `session-N.scope`, which has none: they lock their own memory.
- **A running program's limits can be changed from outside**
  (`prlimit --pid PID --nofile=SOFT:HARD`): how app_server running out
  of file descriptors was reproduced in a test session. Its own count:
  `ls /proc/PID/fd | wc -l`; what they are: `ls -l /proc/PID/fd`.
  `tsession start NAME --session` runs olo-session, the proxy and
  app_server only: no Deskbar or Tracker until you run them.
- **Test sessions don't show a low limit on open files by themselves**:
  they inherit this terminal's, raised. Start one from a held shell to
  see what a login gives: `bash -c 'ulimit -Sn 1024; exec
  /usr/bin/python3 tools/dev/tsession start …'`.
- **Is the pointer drawn?** `tsession shot NAME OUT.png X Y W H
  --pointer` draws it in; the same shot without doesn't: compare the
  two (strong black or white where they differ, if a video plays
  underneath). A plain shot never shows the pointer.
- **Killing by a `/proc` scan matches your own shell too**, as `pgrep
  -f` does: a loop over `/proc/*/cmdline` for a pattern that is also in
  the loop's own command line killed the shell running it (exit 144,
  2026-11-18). Match on `readlink /proc/PID/exe` first (the program),
  then on its arguments.
- **Chrome in a scratch profile** (`olo-chrome --user-data-dir=DIR`)
  opens on its terms of service the first time: start it with
  `--no-first-run`, don't click Accept for the person. `WAYLAND_DEBUG=1`
  in its environment (`tsession run NAME WAYLAND_DEBUG=1 …`) puts every
  request and event in the session's `olo-chrome.log`.
- **A second investigator for a cause you can't find**: the
  `olo-debug-help` skill (`tools/dev/codex-help BRIEF.md`, a headless
  Codex in the background; its log's first lines say the model, the
  effort and the sandbox it got).
- **Notifications in a test session**: `tsession start NAME --session`
  has no notification service; run `olo-notifications --bus
  "$DBUS_SESSION_BUS_ADDRESS"` in it (`tsession run`). They show at the
  bottom right, newest lowest, and aren't in `list_windows`: find a
  button in a screenshot and click its point; a notification gone
  above moves the rest, so look again before each click. A service
  that keeps files in the runtime folder (olo-phoned's state and
  socket) must not share the person's: give it an `XDG_RUNTIME_DIR` of
  its own, and a client that needs both that and the session's display
  an absolute `WAYLAND_DISPLAY` (`/run/user/1000/wayland-N`).
- **Two kinds of volume here**: the Studio Display's (USB, Ben's usual
  output) is the card's own (its `PCM` control; the sound server says
  `HW_VOLUME_CTRL`), the built-in speakers' is the sound server's (the
  SoundWire card's use cases name no control for them, so the card
  plays at full and a mute is only in WirePlumber's state). Anything
  that plays at a set loudness has to handle both
  (`olo_chime::level`); read which with `olo-notifications
  --startup-level` and `olo-chime --which` (both only read). Never
  play a sound or set a volume on the real server to test: point
  `PULSE_SERVER` at `olo-fakesound` (HACKING, "To test sound").
- **olo-fans is running**: a root service keeping the fans on the cool
  profile. It needs `thinkpad_acpi fan_control=Y`, kept in
  `/etc/modprobe.d` since 2026-11-12.
- **vkms is allowed for display tests**: `sudo modprobe vkms` on a seat
  of its own is fine for display work (ask Ben to run it); clean up
  after.
- **kache is Olo's fork**: `~/src/kache`, branch `olo`, installed to
  `~/.cargo/bin` (dynamic admission, a cap for tests). Its config,
  restart and rollback notes are in `/var/tmp/claude-kache`.
- **The history was rewritten on 2026-11-07**: commit ids from before
  then are dead (a map of old to new is in `~/backups`). Never bring the
  old history back.
- **Testing on Amazon and sites that bar bots is fine** (Ben): look
  only, no shopping actions.

- **The clock is on Pacific time.** Calendar's demo events are on the
  person's own clock wherever they are, but one on another clock (the
  demo's Call with New York) lands at that zone's time here: 9 AM, not
  the 6 PM it is in Amsterdam.

## Working with the repository

- **Claude's usage report answers 429 to a token read too often.** A
  handful of reads of `/api/oauth/usage` for one account within a few
  minutes (a capture, a check by hand, the pool's own) got "Rate limited"
  for more than ten minutes after (2026-11-19). Token Pool reads each
  account every five minutes; anything read by hand on top of that
  should be one read, then wait. The pool's state (`olo-pool --status
  --json`, `synced`) says what it last read without asking again.
- **Captures of the clients run them for real.** Token Pool's capture
  (crates/pool/src/capture.rs) runs the installed Claude Code and Codex
  with an account's access token in a sandbox in `~/.cache/olo/
  pool-capture/` (removed after; one a killed capture leaves is removed
  by the next) and passes their reads to the providers. To work on it,
  `OLO_POOL_CAPTURE_ONLY=session` runs one step and
  `OLO_POOL_CAPTURE_TRANSCRIPT=DIR` keeps what a terminal showed. Never
  let a claim, a renewal or a model request through, and never send a
  made-up credential to a provider (Ben: "that is not polite").
- **A test session's Passwords can show a lock that checks the person's
  real login password.** A session's config folder
  (/var/tmp/olo-dev/NAME/config) outlives `tsession stop`: started again
  under the same name with a new `OLO_PASSWORDS_KEYFILE`, Passwords finds
  the old vault, can't open it, and shows "Passwords is locked", whose
  field goes to PAM as the person. Never type into it (failed attempts
  count against their account: `sudo faillock --user ben` shows them);
  remove the session's folder before starting it again, and check for
  the lock before typing (2026-11-19: a script typed four item names
  into it, unsent; the tally stayed empty).
- **kache can hand a build script another worktree's path.** It keys a
  workspace's paths as `<WORKSPACE>`, so worktrees share its cache, but a
  build script's `env!("CARGO_MANIFEST_DIR")` is compiled into the
  program: the main checkout's release build ran olo-installer's build
  script compiled in the power worktree (its `.d` said `# env-dep:
  CARGO_MANIFEST_DIR=/home/ben/worktrees/olo/power/apps/installer`), and
  with that worktree removed it found no `data` folder and panicked, the
  push refused (2026-11-19). Build scripts read it as they run
  (`std::env::var_os("CARGO_MANIFEST_DIR")`), never with `env!`. A build
  script that fails on a path that exists: read the `env-dep` lines of
  its `target/*/build/PKG/*/out/build_script_build.d`.
- **Codex subagents** (`tools/dev/agents`, the `olo-codex-agents`
  skill, which has the details). What codex-cli 0.170 does that its help
  doesn't say: `--approve-for-me` brings the workspace-write sandbox and
  refuses `-s` beside it ("cannot be used with"); with `-c`, `--enable`
  or `--disable` it runs without the shared background server (a
  warning, harmless, but `codex queue` can't reach it: steer through
  tmux); text typed into its TUI quickly is a paste, an Enter in the
  same burst a new line in it (send Enter apart); its sandbox writes
  `/tmp` and `/var/tmp` as well as the repository; the posthog MCP
  warning at its start is the person's unlogged server, harmless.
- **Commit and push regularly**: push to origin after commits (LFS
  through the proxy may need a second try: find why if it fails twice).
  Push from a worktree of its own (`git worktree add --detach
  /var/tmp/NAME COMMIT`), as the pre-push hook builds, and remove it
  after. Amend a commit only before its push has started: the push
  sends the commit as it was when it began.
- **Merge main often** on a branch of your own (olo-coder and the
  like): fetch and merge `origin/main`; in a journal conflict keep both
  entries and run `tools/journal.py redo`.
- **Self-healing, not patches**: design what can fail so it recovers by
  itself (an HTTP MCP server reconnects, stdio never does), never with a
  manual step for Ben.
- **BlueZ 5.87 can be stuck busy with the phone.** After the phone
  goes out of reach, BlueZ's hands-free connecting fails ("Host is
  down") and never ends, so every ConnectProfile after is refused
  `org.bluez.Error.InProgress: br-connection-busy` until the device's
  `Disconnect` (harmless on a device that isn't connected) or
  bluetoothd's restart. The phone service now does that itself; if a
  phone seems stuck "out of reach", read `olo-logs --print --program
  olo-phoned` for "busy" lines, and look (only) at
  `busctl --system get-property org.bluez PATH org.bluez.Device1
  Connected`; never connect or disconnect the person's phone yourself.
- **The embedded controller's event storm (ThinkPad P1 Gen 9,
  2026-11-17).** Nine seconds after the Lenovo 140 W charger went in
  (the Studio Display, which had powered the laptop, unplugged two
  minutes before; BIOS/EC 1.09, PD firmware 1.04), the EC began
  reporting its adapter attached and detached 23 times a second (EC
  0x46 bit 4, a steady 42.9 ms period), charger in or out, on any port,
  whatever the kernel or Olo did; the port held a 28 V, 5 A contract
  and the battery supplied everything (77% to 14% in two hours). The
  kernel queues a handler for every EC event without limit
  (drivers/acpi/ec.c: 195,778 queued; Lenovo's adapter handler sleeps
  0.6 s under a firmware mutex, ten a second at most), so brightness
  keys and USB-C notifications, events in the same queue, never ran,
  and the load average read 270 (blocked kernel workers, no CPU). Only
  a full power-off cleared it. Ben then installed Lenovo's firmware
  1.17 (BIOS/EC 1.10, PD 1.05; fwupd's catalog stopped at 1.15, the
  capsule came from Lenovo's ISO): whether that cures it isn't known,
  the test is the same hand-over, display out and charger in, while
  running. Signs to look for: `cat /sys/class/power_supply/AC/online`
  flipping, hundreds of `kec_query` kernel workers in state D
  (`ps -eo stat,comm`), "Not charging" on a charger. The traces, the
  DSDT and a kprobe module that drops the stale handlers
  (`olo_ec_storm.c`, tried live with Ben's word: keys back in ten
  seconds) were in /var/tmp/claude-blueferry/{keys,ecstorm}, scratch:
  what matters of them is here and in docs/TODO.md.
- **Test sessions share the person's runtime folder** (`/run/user/UID`):
  a program that writes a machine-wide file there (the supervisor's
  `olo-state/ec-storm.json`, which the person's own Olo Doctor reads)
  must be given one of its own when tried: `tsession run NAME env
  XDG_RUNTIME_DIR=/var/tmp/DIR WAYLAND_DISPLAY=/run/user/UID/wayland-N
  PROGRAM` (the display by its whole path, so it's still found; its
  notifications still go to the session's own bus). To find such a
  program again, look in `/proc/PID/environ` for the folder: started
  through `env`, `ps`'s command line doesn't show Olo's path.
- **A worktree never shares the main checkout's target folder**
  (2026-11-19). To measure the code before a change, a `git worktree`
  of the old commit built with `CARGO_TARGET_DIR` set to the main
  `target/` wrote the same files: cargo names a workspace crate's units
  by its path relative to the workspace root, so the two trees' units
  are one. Each build then replaced the other's, and the main tree's
  sources, older than what the worktree had just built, made those
  outputs look fresh to it: its "after" test binary was the old code
  (`cmp` said so), and the release build the pre-push hook makes would
  have linked it. Build a worktree with a target folder of its own under
  `/var/tmp` (kache makes it seconds); if it happened, `cargo clean
  --release -p` each crate the worktree's build compiled (its
  `Compiling … (/var/tmp/…)` lines). The release bench of passing a
  request on is `cargo test --release -p olo-token-pool --test intercept
  pass_on_cost -- --ignored --nocapture`.
- **Dependency build units vanishing from `target/release`: explained
  (2026-11-19).** It was Olo's own nightly sweep (`tools/dev/sweep`,
  `olo-sweep.timer`, between 00:00 and 01:00): it took a unit's
  `invoked.timestamp` for its last use, which cargo writes only when it
  compiles one, so every crate from outside the workspace looked unused
  two days after it was compiled, and it swept with a build running
  (2026-11-18 00:37: 4,284 release units, a build's link failed). It
  now reads a unit's last use from its fingerprint files' access times
  and holds cargo's build locks while it sweeps. If units vanish again,
  `tools/dev/unit-watch` (bpftrace, as root) names whoever removes a
  unit folder: `sudo tools/dev/unit-watch >> /var/tmp/NAME/rmdir.log`;
  the sweep's own runs are in `journalctl --user -u olo-sweep`.


- **Power mode readback on the P1 (2026-11-19).** TuneD 2.28 reports
  PPD ActiveProfile before it finishes applying its profile. In a real
  transition Olo saw the change 47 ms before the completion entry.
  Subscribe to TuneD's `com.redhat.tuned.control.profile_changed` at
  `/Tuned` to observe completion. With a 90% cap already present,
  throughput-performance's 100% minimum left intel_pstate at 90/90
  even after Olo restored powersave and balance_performance. Check
  both global limits. Xe's reported render minimum followed RPe
  through 650, 700 and 850 MHz with 650 requested. Its media GT
  has separate clock limits. Use PCI runtime status and DRM connector
  `enabled` for inspection of a sleeping NVIDIA GPU.
- **A test session shares the system's Bluetooth**, as it shares the
  runtime folder: Preferences ▸ Bluetooth and ▸ Phone, run in one,
  list the person's own phone and devices by name, and their buttons
  are the real adapter's. Picture them with `OLO_BLUETOOTH_DEMO=1` and
  `OLO_PHONE_DEMO=1` (`=connect` for the first step); a picture taken
  without is the person's, kept local and removed (2026-11-20).
- **Retiring a word**: take it out of `data/words/en.txt` and run
  `tools/dev/words merge LANG PAIRS` for each language (with the pairs
  for what replaces it): a merge writes only what `en.txt` has, so the
  old entry goes from every catalog. Words another crate keeps in
  English (the protocol's shortcut titles) aren't found by `words
  gather`: list them with a throwaway example in that crate, printed
  and removed (`shortcuts::entries()`).
- **`-p NAME` may be a library**: `olo-calendar` and `olo-diskbackup`
  are engines; the apps are `olo-calendar-app` and
  `olo-diskbackup-app`. A lint that still fails after "building the
  app" was looking at the old binary.
- **A change a reviewer holds again and again wants a design, not a
  next patch.** Token Pool's sign-in rework (2026-11-20) was held at
  each of six looks by the second investigator, each time for new
  sequences the pool gets right today, each made possible by the last
  fix. After the third such round, stop: park the work on a branch
  with what each look found (`pool-sign-ins-attempt`), write the
  cases into TODO as the design's tests, and land only what stands by
  itself. Ask the reviewer one narrow question each round ("does this
  do anything HEAD doesn't that loses X"): it kept the looks short and
  the findings comparable.
- **BlueZ announces every device to every program**, and a search some
  other program holds (Chrome while a passkey prompt is up) makes that
  constant: `InterfacesAdded` and `InterfacesRemoved` for strangers,
  several a second. A service that takes any announcement as news of
  its own device does its work again each time (the phone service
  abandoned its own asking at each, 2026-11-20). Compare what changed
  for the device you follow; and read `busctl --system get-property
  org.bluez /org/bluez/hci0 org.bluez.Adapter1 Discovering` when a
  Bluetooth fault makes no sense.
- **BlueZ's "br-connection-busy" is an asking in flight**, the
  program's own as likely as not (`dev->pending || dev->connect ||
  dev->browse`, src/device.c): the first asking's answer (a page
  timing out) comes when BlueZ is done, up to a minute on. Asking
  again over it gets "busy" and learns nothing. bluetoothd's journal
  (`journalctl -u bluetooth`, readable here) has what each attempt
  came to ("Unable to get … SDP record: Host is down").
- **A push's live lint could reset the person's GPU** (2026-11-20):
  fifteen resets that day, each freezing the screen and starting
  Terminal, Deskbar and Activity Monitor again. The cause was Olo's
  own: headless app_servers asked the GPU's lowest priority, the
  firmware gives such a queue nothing while ordinary work is queued,
  and xe (Linux 7.2) resets the whole GPU when a job has waited five
  seconds to start (HACKING, "The GPU has priorities too"). Nothing of
  Olo's asks for less than ordinary any more, but **a screen or test
  desktop started before that change keeps its low queue until it
  starts again**, and any other program's low queue is the same
  hazard: don't load the GPU for seconds on end (a live lint, many
  `tsession`s drawing) while such screens are up and the person is at
  the machine. No push until the lint draws off the GPU (TODO). What
  a reset looks like: `journalctl -k | grep "Timedout job\|reset
  started"` (the person's compositor shows there as `systemd-logind`,
  which opened its device). And after any lint or check, see that no
  `olo-app-server` of yours is left (`ps -eo pid,etimes,args | grep
  olo-app-server`: a test's has a HOME under /var/tmp).
- **EGL says High whether or not it is.** `eglQueryContext` reports the
  priority asked for; what the kernel was asked is seen only there
  (`sudo tools/dev/gpu-priority-check asked`, a probe on xe's
  `exec_queue_set_priority`: bpftrace fires twice a call, the second
  with a fourth argument that isn't 0, which the tool leaves out).
- **Measuring what waits for a processor**: `sudo perf sched record -o
  /var/tmp/…/sched.data -- sleep 6`, then `perf sched latency -p
  --sort max` and `perf sched timehist -w`. The first 0.3 s of a
  capture holds waits counted from the capture's start (a "166 ms"
  that wasn't): leave them out. A thread's name with a space in it
  (a window's looper, `w>…`) shifts `timehist`'s columns: parse from
  the right.
- **Where a frame waits after app_server hands it over**: bpftrace on
  `tracepoint:gpu_scheduler:drm_sched_job_queue` (in the program's
  own thread: filter by pid), `…_job_run` (the kernel worker passing
  it on) and `…_job_done`, keyed by `fence_context, fence_seqno`;
  `…_job_unschedulable` marks a job that waited for another program's
  drawing (1% of the compositor's, under 3 ms).
