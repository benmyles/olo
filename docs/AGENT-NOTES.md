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
- **A lock file is locked, then checked to be the file at its name.**
  Open, then `flock`, leaves a gap: whoever held the lock can unlink
  the file meanwhile and someone else make a new one of that name, and
  the lock then got is on a file with no name. `tsession`'s clean-up
  of a display took such a lock for the display's, and deleted the
  sockets of a session that had just begun on that display: a layout
  lint's `[Errno 2] No such file or directory` with no file named, and
  a push refused (2026-11-22). After the lock, compare the open file
  with the path (`os.path.samestat(os.fstat(fd), os.stat(path))`;
  app_server's `sockfile::still_named`), and delete what the lock
  guards before letting the lock go. The second investigator found it
  in ten minutes by replaying two sessions' stop and start in a
  private runtime directory with the calls traced
  (`/var/tmp/…/repro-lock.py`): one that happens once in fifty runs is
  made to happen by holding one side where the gap is.
- **A socket that isn't there says "No such file or directory" of no
  file**: an `OSError` from `connect` carries no name. Say which socket
  and what was being asked when you raise it; the lint now does, and
  keeps a failed session's logs and crash reports
  (`/var/tmp/olo-dev/ui-lint-kept`), which it used to delete with the
  session.
- **A wait repeated for each of many things has no bound: give the
  whole run one allowance.** The same lint waited up to a quarter of
  an hour for the machine to have room before *each* app's session;
  with the machine busy all day (a load of 30 to 80) a push's lint of
  a dozen apps could take hours (2442 s once, then a session timed
  out). It is a quarter of an hour for the run now, all its sessions
  together (`wait_for_room`, `ROOM_ALLOWED`), then on at whatever pace
  the machine gives. A deadline belongs to the job the person is
  waiting for, not to each of its steps.
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

- **A window listed the moment it appears has a placeholder's frame**
  (a few points across, until the app's first size arrives). A script
  that took `list_windows`' frame as soon as there was a window, and
  pictured that rectangle before and after, compared 20 × 62 pixels of
  background, and "0 of 18 with stale pixels" passed twice before a
  picture was opened (2026-11-22). Read the frame after the window has
  settled (again before each picture: a scale change moves it), refuse
  a picture smaller than the window should be, and open one of the
  pictures a comparison passed before believing it.
- **A session's screen is in device pixels.** `tsession start NAME
  1280x800` at the default 2× is a screen of 640 × 400 points, on which
  every window is in its narrowest layout (Calendar and Weather showed
  no side panel, and a before-and-after "differed nowhere"). For the
  smallest screen at 2×: `2560x1600`. A window put in a part of it
  with app_server's `arrange` (`{"type":"arrange","windows":[{"id",
  "x","y","w","h"}]}`) is given fractions of the screen's area, in
  points at the scale it has: a vertical third of the smallest screen
  on a `2560x1600` session is `w` = s/6 and `h` = s/2 at scale s (at
  1× a third of the whole session would be 853 points, not 427).
- **A resident app keeps answering after a rebuild** (2026-11-24): the
  palette is single-instance and stays resident, so each `olo-palette
  --window-menu …` run in a session is handed to the first one, which
  runs the old code and the catalogs it read at its start (a translated
  menu showed English after the words were added). Stop it, by the pid
  `tsession run` printed and after checking that pid's environment is
  the session's, before picturing a change to it.
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
  `astra-look` and `opus-look` are given the pictures' whole paths (each
  works on copies in a folder of its own); started together in one
  shell line with `&`, one ran from the repository's other checkout
  (`cd X && A & B`: the `cd` is part of the first job only, and B ran
  the main checkout's old script, 2026-11-22). Run each as its own
  background command.
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
- **An alert a demo opens as its app starts sits at the screen's
  corner, inactive.** (`OLO_REX_DEMO_OPEN=host.move:dev-box` and the
  like, 2026-11-25.) No window has the keyboard yet, so it opens with no
  parent, and app_server places it neither over its window nor in the
  screen's middle: its tab lies on its window's, grey. Opened from the
  menu it is over its window and active. Reviewers flag it each round:
  say so in the brief (it is in docs/TODO.md to root-cause).

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

### zbus drops what nobody listens for

A `zbus::Connection`'s reader hands each message to the streams that
exist then, and drops it where there is none (`set_await_active(false)`,
`connection/mod.rs`): a `MessageStream::from(&conn)` made after
`build()` has missed whatever was read first, and with a client that
sends in the same write as its handshake (every D-Bus client's `Hello`)
that is its first message, whenever the reader's thread runs first. For
a server's side of a connection, `Builder::build_message_stream()` (the
`bus-impl` feature) makes the stream before the reader starts. `olo-bus`
lost one `Hello` in seventy with the machine busy until it did
(2026-11-26; HACKING, "Agent Screens", `crates/bus/tests/start.rs`).
How it was found, for the next hang at start: the stuck process's
stacks (`gdb -batch -ex "thread apply all bt" -p PID`: all parked, so
not a lock), its sockets (`ss -x -a -p`: one connection, nothing queued
either way, so each side waits for the other), then a loop that starts
it a thousand times at once with a deadline (1.3% hung), then the same
under `strace -f -e trace=sendmsg,recvmsg`: the bytes were the same in
a good start and a stuck one up to the `Hello` read and never answered.

### Two builds compared need the same data, and a timing its own program

A build kept somewhere else (`cp --reflink` into `/var/tmp`, for a
before and after) finds no `data/` beside it: it draws with the
system's fonts and no icons, and every pixel of text differs from the
tree's build for that alone. Give both `OLO_DATA=…/olo/data`
(`tsession run NAME OLO_DATA=… PROGRAM`); the first comparison of the
layout fix was all font (2026-11-26). And take the noise out first: the
build before against itself (clocks, a chart by the hour: Token Pool,
Kache, Weather). `tools/dev/kit-compare` runs one window at a time in
one session; for many at once, a session a window, as
`at_once.py` does for the Rex rig.

`jato test … -- --nocapture` gave one `--ignored` cost test's printed
time three runs running, to the tenth of a microsecond: a kept result
said again, not three runs. To time a test, run its own program
(`jato test --release -p PKG --no-run`, then the file it names under
`target/release/build/PKG/*/out/`, with `NAME --ignored --nocapture`).

### A port the computer has one of is a live service too

Token Pool's gateway and proxy are 127.0.0.1:17891 and 17892 on every
computer that has a pool, and on a Rex host the agents there (yours
too: `HTTPS_PROXY` in your own shell is the proxy) send every request
through them. An `olo-pool` started from the tree, its demo too, tried
to listen on both: here it failed ("Address already in use", in the log
of every picture's Token Pool), and on a computer with no pool running
it would have been the pool. Fenced since 2026-11-26 (HACKING, "Every
session gets its own…"). Look for it with `ss -ltnp` before and after a
test of anything that listens, and read a test program's log for what
it tried.

### Nothing is committed while a push-then-publish runs

`tools/publish-release.sh` publishes at HEAD as it is when its turn
comes, and refuses a HEAD that isn't on origin/main. Chained after
`tools/dev/push` in one background command (the push's checks take
minutes), a commit made meanwhile is that HEAD: the push sends the
commit it began with, and the publisher stops ("HEAD … isn't on
origin/main: push it first"; update 13, 2026-11-26: four minutes lost,
nothing wrong published). Commit before starting the chain or after it
ends; work that waits goes on outside the repository, or uncommitted.

### A test that waits for a program's line waits with an end

`read_line` on a child's stdout has no deadline: a program that never
prints it holds the test, and `jato test --workspace` behind it, for
good (25 minutes before it was looked at, 2026-11-26). Read the line on
a thread and wait for it on a channel (`recv_timeout`), as
`crates/bus/tests/common` does. And a wrapper's "done" is not the
run's: `cmd | grep … ; echo done` ends 0 whatever `cmd` did; write its
exit after it (`echo "exit $?"`) and read that.

### A race shown: the test on one processor, and a run that ran it

(2026-11-26.) AirPlay's `the_older_way_from_a_place_in_the_film` failed
once in a whole workspace's tests on a busy machine ("TEARDOWN" missing
from what the stand-in receiver was asked). Two things about showing
such a thing:

- **Check the run ran the test.** A crate has several test programs
  (its library's, its binary's, `tests/*.rs`); a name given to the
  wrong one runs nothing and passes. Twelve "passes" of the binary's
  program said nothing: `1 passed` in the output is what counts, not
  the status.
- **`taskset -c N PROGRAM TEST`, a hundred times.** With every thread of
  the test on one processor, a thread that wakes another is often put
  aside at once, so a window of a few instructions is hit most runs:
  96 of 150 before the fix, 0 of 150 after; free on 64 processors it
  was one in hundreds. (The cause: the thread that asks the receiver
  how the film goes said "ended" while still holding the connection;
  whoever heard dropped the play, whose goodbye takes the connection
  only if it is free.) What says "it's over" lets go of what the
  listener will want first.

### A test that decides by the machine it runs on

(2026-11-26.) The background keeper's
`keeps_while_something_is_in_the_group_and_only_one_does` passed all
day and failed in one run of the whole workspace: `left: "max", right:
"0"`. Not a race: the keeper under test read the real `/proc/meminfo`,
and whether it closes the swap ceiling depends on what is available
against twice its reserve. The dev server is shared; with other
people's builds it had 55 GB available of 251, under 2 × 12%, and the
keeper rightly left the ceiling open. Run again at that moment it
failed every time; an hour before, never. In its crate's tests the
keeper now reads a computer made up for it (`Memory::now` under
`cfg(test)`), and the file's reading has its own test (`parse`). When
a test fails once on a shared machine, look at what of the machine it
reads (memory, load, the clock, free ports, the home folder) before
calling it a race; `grep MemAvailable /proc/meminfo` and `uptime` at
the moment say more than a second run. (And: `/tmp` here is memory;
the sessions' folders under `/tmp/claude-UID` are other agents' as
well as yours, 15 GB of it that afternoon, none of it this session's:
theirs to clear, said to their lead.)

### A helper for hosts, tried by hand

(2026-11-26.) The helpers a build of Rex carries
(`target/PROFILE/build/olo-rex/HASH/out/olo-rex-link-x86_64`, the newest
`out` by its time) are data there, not programs: copy one (`cp
--reflink=auto`) and `chmod +x` it to run it. `target/debug/olo-rex-link`
is the workspace's build of the same program with none of its features
(`jato build --workspace` puts it back so: "built without screens of
this host's own"): `jato build -p olo-rex-link --features
agents,screens` for one that has its commands, or the carried copy,
which is what a host gets. And a clean-up that ends programs by their
command line (`kill` of whatever's `cmdline` holds a word) ends the
shell that typed the word too: the same trap as `pkill -f`, by hand.

### What was unit-tested "for want of a Chrome" can be tried now

(2026-11-26.) The dev server has Chrome since a host's own screens: in
the trial account (below), `olo-chrome --olo-install` once, then a
page served by `python3 -m http.server` on 127.0.0.1 opened with
`screen run NAME olo-chrome URL` and driven with `tree`, `set`,
`select`, `click`. The first thing tried so had never worked (Chrome's
own sign-in on a screen's profile: the setting written was one Chrome
overwrites as it starts), which only a live Chrome could show: a
setting written into another program's file is tested against that
program, read back after it has started, not against what was
written. Where a note says "not tried live (no Chrome here)", try it.
A page that stands in for a framework is small: a field whose value is
written back each frame from the page's own state, which only `input`
events change; a header `position: sticky`; a `keydown` handler that
takes `/`.

### A new section's first look: what both reviewers found that a careful build still had (2026-11-26)

The Host window's Screens tab was built by copying the window's own
row pattern, measured, lint-clean at five scales, and tried on the rig,
and its one round of looks still found sixteen things. None was a
pixel the lint could see. What to check before the next new section
goes to them:

- **Every state has one structure.** What a thing is and how it is
  updated stay; what this state has to say goes in one slot. Lines
  swapped in and out per state made the section under it stand at
  three different heights, and left "Fetched from Olo's releases when
  you choose" reading as an error's second sentence.
- **A word means one thing in a window.** "A whole Olo desktop of this
  host's own" over "`--on desktop`" (this computer): a careful reader
  maps it backwards.
- **A number is said once, by name.** "about 600 MB" beside "2.3 GB"
  were a download and what it holds on disk, unlabelled; and the
  helper's "bytes" (Olo's alone, 1.8 GB) beside the window's "2.3 GB
  on the host" would have been two again: measure what the words claim.
- **An error says what to do, and whose it is.** "Couldn't remove" with
  "Olo is taken off once they are stopped" said it failed and that it
  would happen by itself; "GitHub couldn't be reached" read as this
  computer's trouble when it was the host's.
- **A greyed choice says why**, in its own note; and its note is as dim
  as its label.
- **No button that guesses** (a greyed Install beside "Asking…") or
  repeats the one just pressed.
- **What is typed inside a sentence** takes the sentence's line height
  and a point of room after it (`host_panel`'s `typed`).
- Lines that wrap with one or two words over are rewritten to fit, in
  every state: read every state's picture for them before the round.

And of its second part (the list of a host's screens, the install's
bar and Cancel, the same day), two things a real trial on the rig
showed that nothing else had:

- **`jato test -p PKG` doesn't build the program the rig runs.** It
  builds the test binary; `target/debug/olo-rex` stays as the last
  `jato build -p olo-rex` left it. A trial run after edits and a green
  test run was of the app from before the edits (its words said so:
  "Downloading Olo, 0%", which the code no longer wrote), and three of
  its failures were commands that app didn't have. Build the program,
  then run the rig; and nothing builds that package while a rig runs.
- **A list's row is one node for agents**: role `Row`, its cells'
  words joined in its name, `selected` on it (`ColumnListView`); there
  are no nodes for cells. A check that looked for a node named
  "review" found none with the row on the screen.

And of measuring Rex "at rest" in its demo: it isn't at rest. The
demo's first tab has an agent at work (two, with the Activity list), so
its working ring is drawn ten times a second: 10 frames a second and
about 150 wakeups in software drawing, by design. (Until 2026-11-26
that read as "0 frames, 40 wakeups a second": the ring's angle came
from the seconds since 1970 as an `f32`, which stands still for 128 s
at 1.8 billion, so every repaint was the same picture. A count of
frames that is nought beside a count of wakeups that isn't is a
repaint that changes nothing: look for what is painted again, with
`OLO_FRAME_PHASES=1` in the app's log, "paint (1 of 92 nodes)".) For
Rex truly at rest, choose the demo's `server-logs` tab: 0 wakeups.

And of the picture script (`/var/tmp` scratch, each state a session of
its own): a window that fits its content has its frame for good only a
moment after it first shows, so ask for the frame until two looks a
second apart agree, then take the picture (taken at first sight, most
were blank); and a session as big as the smallest screen in points
times the scale (1280 × 800), or the window is taller than its screen
and its Frame scrolls it.

### What a provider's client does with its headers is in the client: read it there (2026-11-26)

Whether Claude answers an account from usage credits isn't in any
document: it is in Claude Code, which decides its own "Now using usage
credits" from the answer's headers. The program installed here
(`readlink -f "$(command -v claude)"`, a 400 MB ELF with its JavaScript
inside as text) can be searched with Python's `mmap` and `find` for a
header's name (`unified-overage-status`), then read around the hit:
`isUsingOverage = status === "rejected" && (overageStatus ===
"allowed" || overageStatus === "allowed_warning")`, the header names
(`…-overage-in-use`, `…-overage-disabled-reason`), the claims it knows
(`five_hour`, `seven_day`, `seven_day_overage_included`). `grep -o`
with a long context refuses ("exceeds complexity limits"), and
`strings` loses the minified lines. The same for Codex's binary. It
settled in ten minutes what the Token Pool had wrong: it took only a
429 for a meter running out.

The live pool's own state can be read without touching it:
`curl -s http://127.0.0.1:17891/pool/status` (each account's meters,
what blocks it, the last 50 requests and who answered each). Never
print its accounts' names or addresses into anything kept; number them.

### The dev server has a host's own screens (since 2026-11-26)

`olo screen install` was run on the dev server's own account that day
(the released Olo under `~/.local/share/olo/desktop`, 1.8 GB; Chrome
under `~/.local/share/olo/chrome`, 460 MB; Noto Color Emoji in
`~/.local/share/fonts/olo-screens`), for the agents that work there:
`olo screen --on host start NAME`. `olo screen status` says what is
there, `olo screen install` again brings it up to the newest release,
`olo screen remove` takes Olo and the font off (Chrome stays). The
host's default is still the desktop's screens. It is the released Olo,
not the tree's: a change of yours is not in it until it is released
and installed.

### A stand-in that does more than the real thing hides the bug

(2026-11-26.) The test of "a screen ends with its tab" had a made-up
`olo-screen` whose `stop` ended the screen's whole process group. The
real one asks the session to end and leaves the group alone, so the
helper's keeper, which is in that group, lived on after a screen was
stopped by hand, ready to stop whatever screen had the name later. The
test passed for a day's worth of runs; the first real `stop` on a real
account showed it (`ps` for `keep-until` after the stop: one). When a
stand-in ends, removes or answers something, read the real program's
code for exactly how much it does, and make the stand-in do no more:
the bug lives in the difference. And after any real trial, look for
what is left (`ps`, by each process's own words: a search for a word
your own shell's command line holds counts the shell).

### A host's own screens, tried without touching the host

(2026-11-26.) An account of its own for the trial, as a test session
has: `HOME`, `XDG_CACHE_HOME`, `XDG_CONFIG_HOME` and `XDG_RUNTIME_DIR`
(mode 700) under `/var/tmp`, `OLO_HOST_DESKTOP` for where Olo goes,
the fences of `tsession` (`REX_SERVER_SOCKET`, `OLO_REX_AGENTS_DIR`,
`OLO_POOL_PORT=0`, `OLO_POOL_PROXY_PORT=0`), and
`DBUS_SESSION_BUS_ADDRESS` unset (a screen asks the session bus for
the accessibility bus, which would start one on the account's). A
prefix of the tree's own programs: copies of `target/debug/olo-*` in
`PREFIX/bin`, `PREFIX/share/olo` a folder of links to `data/*` (not
the wallpapers) with a `version` file. What a screen runs is found by
each process's program (`readlink /proc/PID/exe` under the trial's
folder), which is also how to see none is left. Its pictures name the
machine (Tracker's disk, a terminal's prompt and title): crop or mask
before one leaves it.

## Measuring

- **Which programs have a maker's libraries loaded: `/proc/PID/maps`.**
  The last field of each line is the file mapped; whether a program
  holds a device open is in `/proc/PID/fd` (the links). Read so
  (names only, 2026-11-20): every app_server, the person's and each
  agent's screen's, had NVIDIA's nine EGL libraries loaded (glvnd
  loads every maker's library to ask it for a display), every Deskbar
  had NVIDIA's NVML and held one of the card's nodes open, and the
  person's Chrome processes had the EGL ones. Nothing in Olo's code
  names those libraries: look at what is loaded, not at what the code
  asks for. (And so a rule of "run nothing that loads NVIDIA's
  libraries" was broken by every scratch agent's screen started that
  day, unknowingly: a headless app_server no longer loads them,
  `app_server::egl_vendor`.)
- **What an idle screen costs, and what makes it draw.** Start a
  scratch agent's screen from the release build
  (`env -u OLO_SCREEN target/release/olo-screen start NAME`; its
  programs are that folder's), let it settle 40 s, then over a minute
  take each program's processor time and wakes from /proc: `utime +
  stime` of every `/proc/PID/task/*/stat`, and `voluntary_ctxt_switches`
  of every `task/*/status` (by thread, with `task/*/comm`, it says
  which thread wakes; a window's looper's thread is named after the
  window's title: print "a window's looper", not the name). Its
  programs are the ones whose `/proc/PID/environ` holds
  `OLO_SCREEN=NAME`. What it draws: subscribe to its app_server's
  `painted` events (`{"type":"subscribe_to","events":["painted"]}` on
  the `olo_socket` that `start` printed) and count them. What made
  Deskbar draw: start the screen with
  `RUST_LOG=warn,olo_deskbar=debug` (the environment reaches its
  programs) and read `$XDG_RUNTIME_DIR/olo-screens/logs/NAME.log`:
  "told what=…" for each message, "status changed=[…]" for which parts
  of its status differed. `strace -p` works on one's own programs here
  (`ptrace_scope` 0), and showed app_server's frame timer and who
  connects to it. That is how an idle agent's screen went from 1.9% of
  a core to 0.1% (2026-11-20): each guess before the log was wrong
  (the supervisor's file changed once a minute when nothing built, and
  39 times in 90 s when something did).
- **Who calls it: this machine gives no call graph for a release
  build** (2026-11-22). `perf record --call-graph lbr` can't open its
  events here; `--user-callchains --call-graph dwarf,8192` records,
  and unwinds almost nothing of a release build (line tables only,
  no frame pointers); gdb with a breakpoint on `memcpy` and `bcmp`
  that prints a backtrace gets about one call a second, the first
  second's and no more. What works without a call graph: the flat
  profile (`perf record -e instructions:u -c 200000 -p PID`, `perf
  report --sort symbol`) names the Rust functions; for a libc
  function's callers, build the one program with
  `RUSTFLAGS="-C force-frame-pointers=yes"` into a target directory
  of your own under /var/tmp (a release build of every crate: a
  quarter of an hour) and record with `--call-graph fp`.
- **A derived `Hash` of an array is a length and then bytes**: a
  two-word id went through the hasher's byte path, and a hasher
  written for words spent more there than the standard one saved.
  Write the key's `Hash` to hand its words over (`write_u64`), and
  look at the profile after, not only at the total.
- **Count instructions unscaled: `perf stat --no-scale`.** This
  machine's cores are of two kinds (`cpu_core`: CPUs 0–3, `cpu_atom`:
  4–15), each counting on its own, and a thread is counted only on the
  kind it's running on. perf's default scales each count up by the time
  it wasn't counting, so a thread that moved between kinds was counted
  up to twice, at random: the same Logs build read 5.0 or 9.4 M
  instructions a frame. Sum both kinds' counts (`-x,` prints a line
  for each kind: a script that read the first line alone had the share
  of the work that happened to run on that kind, 143 to 773 M for the
  same work, 2026-11-22). For a benchmark of the
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

- **A copy of `target/release` is free here** (on the person's laptop
  and the Fedora dev server: btrfs; the first dev server's disk is ext4,
  where `--reflink=always` fails, "Operation not supported": copy
  plainly there, a binary or two) (`cp -a --reflink=always
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

### A desktop that isn't at rest: who keeps it busy

Learned on the person's desktop (2026-11-21: three displays drawing 59
frames a second with nobody at it):

- **Three read-only looks find it**, none needing root. The
  compositor's own count: `{"type":"timings","quiet":true}` twice,
  seconds apart, on `$OLO_SOCKET` (each display's frames; quiet, so
  its heartbeat isn't started). Every thread of every program of the
  person's, sampled twice ten seconds apart from `/proc/PID/task/TID/
  {stat,status}`: processor time and voluntary context switches (who
  is busy, and who wakes 60 or 120 times a second: who draws every
  frame). And which files change: an inotify count (`IN_MOVED_TO`,
  `IN_CLOSE_WRITE`) of `$XDG_RUNTIME_DIR/olo-state`,
  `$XDG_RUNTIME_DIR/olo-glance/DISPLAY` and `~/.config/olo` for a few
  seconds names the writers by their files. A file written twenty
  times a second by one program and read by another is a link of a
  chain: follow it both ways.
- **`perf` on the person's programs needs root** (they hold a
  capability: "A process that holds a capability can't be looked
  into", below; without root, "Failure to open any events"), and
  `perf report` as root refuses a recording a failed try left behind
  under your name ("not owned by current user or root"): record into
  files that aren't there yet, and pass `-f` to report. Its DWARF
  call chains of Olo's release programs often stop at the leaf: read
  the leaf's callers in the source.
- **The figures to compare are release builds'**: keep the release
  programs as they are before the change (`cp --reflink=always
  target/release/* /var/tmp/YOURS/before/release/`: only the
  programs, free), build the changed packages with
  `CARGO_TARGET_DIR=/var/tmp/YOURS/after cargo build --release -p …`
  over a second copy of them (the unchanged programs a session needs
  are then there too), and run the same tool twice with
  `CARGO_TARGET_DIR=… TSESSION_PROFILE=release` (tsession takes its
  programs from there). `tools/dev/rest-check` is built that way.
- **A stand-in for a coding agent**: Terminal knows one by its
  program's name and what its screen says, and only as what a tab's
  shell runs (the tab's own program isn't looked at). A script named
  `codex`, started by typing its name at the tab's shell, printing
  "• Working (41s • esc to interrupt)" on the alternate screen: the
  tab shows its working mark. Started with `-e`, it never did.
- **The notification service may play a sound**: in a test, write its
  state file in its place (`$XDG_RUNTIME_DIR/olo-state/
  notifications-DISPLAY.json`, `NotificationState` as JSON, renamed
  into place) and remove it after. A test desktop has no notification
  service of its own (`olo-notifications` refuses outside a real
  session without `--bus`).
- **`cargo build -p A --example X -p B` builds examples only**: the
  target filter holds for every package named, so B's binary is left
  as it was, and says nothing. Rex was pictured twice with a look it
  no longer had (2026-11-22: a picture named for one look showed
  another, and the person saw it first). Build an example and a
  program in two commands, and look for a word of the change in the
  binary (`strings BIN | grep -c WORD`) before picturing it. (A word
  of plain letters: `strings` breaks a phrase at a curly apostrophe or
  an ellipsis, and counts none.)
- **Open every picture before it is used or shown**: a montage of the
  candidates, each labelled, takes one look and catches a wrong one.
- **The tiling lint as the push will run it**: `tools/dev/ui-lint
  --dev --no-cache --tiles --screens "2560x1600@2,$(tools/dev/ui-lint
  --person-screens)" APP…`. Without `--screens` it tiles on one
  screen, and a pane that is whole there is cut in a third of the
  person's 1920 × 1080 one: Phone's passed here and the push was
  refused (2026-11-22). And `--no-cache`, or an app the lint remembers
  as clean isn't run again.
- **A test session has two looks, not three**: `tsession theme NAME
  light|dark`. Any other word (`night`) is taken without complaint and
  changes nothing: six pictures named "night" were the light look, and
  both reviewers said so (2026-11-22).
- **A minder's wait is two minutes at a time**: a foreground command of
  an Agent-tool subagent is moved to the background by its harness
  after about three, so `agents watch --for 2`, again and again.
- **Two reviewers started together**: set what they share on lines of
  their own, then `( astra-look … ) & ( opus-look … ) & wait`. With
  `S=… && ABOUT=… && ( … ) & ( … ) & wait` the assignments are part of
  the first job, and the second runs without them (its pictures'
  paths were empty: "no picture /NAME.png").

### An app written in one go, taken apart by several (Rex, 2026-11-21)

- **A client written from a protocol's documents is a guess until it
  has talked to the server**: the first button pressed failed on a
  reply wrapped in one more object than the type expected, and with it
  split, resize, move, ownership and version were all wrong on the
  wire. Before anything else of such a client: every call made against
  a scratch server, the raw frame kept beside the type.
- **One crate an agent, and a list that travels**: each agent kept a
  list of what it found in another's crate (file, line, what the
  server really sends), passed on through the lead; the client's agent
  found the app clearing its activity marks, the engine's agent found
  Find dying after a restore. The lead's part is the passing on, the
  decisions (keys are never replayed into a terminal), and saying no
  to scope.
- **The audit that changes nothing is worth its run**: a read-only
  agent asked only "where is this not an Olo app" found two freezes by
  using the demo (Defaults hanging the Settings window on its own
  lock; the panes not following the selection), which no fixer had
  been looking for.
- **The lead's own soak finds what the agents' tests don't**: every
  agent's suite passed while each tab closed left two threads behind
  (the kit's, not theirs). Run the finished whole under load yourself.
- **Pictures for outside eyes: no shell prompt** (it carries this
  computer's names). `PS1='$ '` in a scratch shell; a crop otherwise.
- **The weekly Codex allowance is one pool for subagents, the helper
  and the picture look**: four agents at xhigh took a third of a week
  in three hours. Stop an agent that is done; keep the last tenth for
  the looks a commit needs.

### Taking a settings window apart (Rex, 2026-11-21)

What was learned making Rex keep its settings as Olo's apps do:

- **Ask of each setting who reads it, then of each reader whether it
  acts.** A table of fields against their uses (grep, one row a field)
  found two thirds of sixty with no reader. It didn't find the ones
  whose only readers were their own switch and its check mark (Developer
  Mode), nor a toggle whose state nothing ever advanced (Auto Approve:
  its `tick` was marked `#[allow(dead_code)]`). `grep -n
  "allow(dead_code)"` over an app is a list of what claims to exist.
- **A test that calls `olo_kit::settings::update` writes somebody's
  settings.** The settings' socket is found by `OLO_SOCKET` alone, so
  under `env -u OLO_SOCKET` nothing happens; run from a shell of a
  session (the person's Terminal, as they might), the same test would
  write that session's settings. What tells the desktop is a parameter
  of the thing tested (`shared::adopt_telling`), and the test tells
  nobody.
- **A test session's config is its own folder**
  (`/var/tmp/olo-dev/NAME/config`, its `XDG_CONFIG_HOME`), not
  `--home`'s `.config`: a file an app is to find at start goes there.
- **What a menu is made of is made at every key that may be an item's**
  (`MenuBar::of`). Time what it calls: a list that looked free
  (`looks::terminals`) copied every theme's whole palette, 228 µs a
  time, in Terminal for months.
- **A before and an after measured an hour apart measure the hour.**
  The same build's frame took 0.5 ms and then 0.9 ms, the machine's
  load average having gone from a few to 36 (other work of the
  person's). Run the two in turn, twice (before, after, before, after),
  and say the load beside the numbers.
- **A worktree for the next piece while the tree is in a push.** The
  hook's release build and lint take a quarter of an hour during which
  the tree must not change: `git worktree add /var/tmp/…/NAME -b BRANCH
  HEAD`, work there (its own `target/`, kache behind it; tsession's
  programs come from the worktree's `tools/dev/tsession`), and bring it
  back as a commit when the push has ended. Two things a fresh worktree
  lacks: the programs a reloadable test session starts (`tsession start
  --session` printed nothing and no window came: `TSESSION_SESSION_EXE`
  at the main tree's `target/debug/olo-session`, or build it there);
  and, for `tools/journal_html.py` only (`journal.py add` no longer
  builds the page), the journal's cache of re-encoded pictures
  (`docs/journal/.cache`, ignored by git), without which it re-encodes
  all nine hundred (two and a half minutes): `cp -r --reflink=always`
  it from the main tree first.

### Keys: press them, in the app and in a control (2026-11-21)

Rex's command-key shortcuts had never worked, and nobody had pressed
one: every test typed text. What was learned:

- **`tsession key` goes through app_server's real key path**
  (`synthetic` → `physical_key`: the keyboard profile's remapping, the
  command key as the right Ctrl). 125 (left Super) is ⌘ on the Mac
  profile, 29 the physical Ctrl: `tsession key N 125 20` is ⌘T,
  `tsession key N 125 42 44` ⇧⌘Z. Check what a key did by its effect (a
  tab more at the server, a file the shell wrote), not by a picture.
- **An app's "platform" modifier never arrives.** The command key
  reaches an app as Control (the right one). Code written for a
  toolkit's `platform`/`meta` modifier compiles, reads well and is
  dead: grep for it in anything brought in from elsewhere.
- **A control in the app that works.** The same presses in Olo's
  Terminal said what was Olo's rule and what was a limit of every
  terminal's window: ⌘C with nothing selected interrupting is the rule
  (a shortcut whose item is off is the shell's); ⇧⌘Z undoing is the
  limit (TODO, "The command key's Shift in a terminal"). Without the
  control the first would have been "fixed" and the second blamed on
  Rex.
- **A sequence that can only go one way proves nothing.** Undo then
  Redo passes whether or not ⌘Z and ⇧⌘Z are told apart (only one of the
  two items is on at a time). The sequence that tells is the one where
  the wrong item is the only one on: Bigger, ⌘Z, ⇧⌘Z, **⇧⌘Z**, ⌘Z,
  **⌘Z**. And start from a clean config folder: a run that began at
  the largest size showed seven equal numbers and "passed".
- **A fix that passes every test of yours can still be wrong: the
  keyboard path gets a second reader before it lands.** Sending Olo's
  own terminals the command key's chords as pressed made every check
  pass, in Rex and in Terminal. Astra, reading the diff alone, found
  that Ctrl held before ⌘V would now paste, that with no profile read
  plain Ctrl+V matched Paste, and that an old Terminal beside a new
  app_server (a half-installed upgrade) would send ⌘C as an interrupt
  with text selected. None of those is a chord a test of mine pressed.
  The change was withdrawn, Terminal's path left as it was, and the
  list of what a fix must hold is in TODO. Ask "what else can reach
  this code, in what order, from which version" of anything between
  the keys and the shell.
- **A command you prune may be one a test performs.** "Nothing
  dispatches it" was read from the window's command switch; six of the
  names were what a host's program asks of this computer, carried out
  by the hub and declared to the server, and a reconnect test performs
  one. Grep the whole crate, tests included, for a name before calling
  it dead.
- **The person's shell in Terminal is their login shell** (not bash):
  `sleep 3; echo $?` after an interrupt goes on to the echo there (130)
  where bash would stop the line. Read the control's result knowing
  which shell ran it.

### A faster way of something exact: held to the old way, in the tests (2026-11-22)

- **Keep the old way under `#[cfg(test)]` and assert both say the
  same**, of the cases you can think of and of hundreds generated. The
  kit's scene comparison was first made faster by reasoning ("lines
  alike in place differ where their kept glyphs do; others have
  changed whole"): my tests passed, the stale-pixel pictures passed,
  and Astra's review found six ways it reported a change where the
  glyphs in place were the same to the byte (so a frame for nothing),
  two where it reported a whole line for a glyph, and one where it
  reported less than changed. Done again as the same comparison,
  walked in place without the copies (`Scene::glyphs_changed_since`
  against `glyphs_changed_by_copies`), there is nothing to argue: the
  tests say whether the two agree.
- **An oracle that asks "is each thing somewhere in the other"** is
  blind to order and to count: two glyphs one over the other, drawn
  the other way round, passed it with nothing reported. Compare what
  is left outside the reported part as a sequence
  (`scene::tests::same_outside`), and test the oracle itself with a
  case it must refuse.
- **A cached bound is a second way of computing something**: a line's
  reach was the union of its glyphs taken at the line's corner, then
  moved, while the glyphs were each moved; in floats the two differ
  far from the corner. Whatever takes the bound at its word needs the
  producer to make it exact (or not to make it): say where it is
  exact, and test either side of that line through the real producer,
  not a helper that computes it the right way round (the test helper
  did, and hid it).
- **A fixture's glyphs with alpha 0 and tiles of one size** test the
  records, not what shows: fine for a comparison of records, but say
  so, and give the cases that turn on colour real colours.
- **Bounds joined with `union` can end short of what they join, and
  padding them is not the cure**: a union keeps a corner and a size,
  the size a difference of floats that rounds, half the time down
  (whole numbers under 2^24 never round, which is every window; a
  drawing's own coordinates can be millions, and a fraction like
  -511.1 rounds anywhere). I padded the size a step of the float where
  it fell short (`join`), put it wherever bounds are kept and taken at
  their word, and the third review found two frames for nothing at
  ordinary coordinates: an edge at exactly 8 padded to 8.00003 touches
  a quad that starts at 8 (another draw order, so "changed"), and is a
  column of damage past a clip that starts at 8. Whether an edge
  touches or crosses is decided on the exact edge: keep the four edges
  (a least and a most don't round) and make a corner and a size only
  where something needs them. Taken out again; TODO, "The damage path
  at the edge of a float".
- **A fold of unions depends on its order** for the same reason: two
  ways of walking the same glyphs (each side's joined, then the two
  together; or one after another) came to rectangles a pixel apart
  once rounded out. A faster way must join in the old way's order to
  give its bits, or prove the numbers exact where it doesn't.
- **Give a fixture the same finding only where the real thing would
  have it**: two lines of different glyphs under one `found` id were
  passed as "made the same way" and the comparison under test never
  ran. Assert in the test that the shortcut didn't take it.
- **Check a test has teeth**: take the guard out, run it, see it
  fail, put it back. The 600 generated drawings passed without the
  guard they were written for; the reviewer's one case, by name,
  failed as it should.
- **A reviewer reads the working tree as it is when it gets there**:
  the second review ran while I went on editing and reviewed both
  the diff it was given and what the file had become (it said so, by
  hash). Fine here, but don't edit what is under review unless you
  mean it to see the edit: a third look was given a tree that stood
  still.

### A test looks only at what it started (2026-11-22)

`pgrep -x olo-rex` (any app's name) lists the person's own running
app beside a test's. A repro of mine took "two olo-rex processes" for
the test's two and attached gdb to the second for its stacks: it was
the person's Rex, thirteen hours up, and it was stopped for the moment
of the dump (it went on, no tracer left; nothing of it was read but
function names, and the dump was deleted). Never look a process up by
name to signal, trace or inspect it. A test's processes are the pids
its launcher gave it (`tsession run` prints "started olo-rex pid N";
app_server's `list_windows` has each window's pid, of that session
only), and what a pid is can be checked before anything is done to it
(`/proc/PID/environ` has its `WAYLAND_DISPLAY`; its start time). The
same goes for a helper: a brief that says "take the stuck process's
stacks" says whose, by pid, and that the person's own copy runs on
this computer.

### A launch handed over and lost: who takes it, once (2026-11-22)

One `olo-rex --hosts` in thirty did nothing on a busy machine: no
window, no second copy. The kit takes every forwarded launch before
the app's handler has it (`crates/kit/src/app.rs`), and Rex's handler
took it again: when the listening thread had heard the first, told the
launcher "ok" and dropped its end in between, the second asking was
answered "no" and Rex dropped the request. `Launch::take` answers the
same however often it is asked now, and Rex asks once. Found by the
second investigator in a few minutes of reading (its notes named the
two lines), while my own repro was looking at the wrong process (the
note above). The repro: `handoff.py` in the Rex scratch (N launches,
each timed; 6 lost of 141 before, 0 of 120 after, at a load of 40 to
70).

### A lock held through an `if let` (2026-11-22)

`if let Some(r) = app().renaming.take() && … { app().hub… }`: the
guard `app()` returns lives to the end of the `if`, so the second
`app()` in its body asks for a lock this thread holds. A debug build
panics there ("Found cycle in mutex dependency graph": olo-sync checks
the order locks are taken in), which in a window's handler leaves the
window open and unanswering to that key; a release build would wait
for ever. Take what is needed in a statement of its own (`let r =
app().renaming.take();`), then use it. What told: the test's crash
reports, in the scratch `XDG_STATE_HOME` it was started with
(`…/olo/crashes/*.json`: `message`, `thread`, `backtrace`): look there
whenever a headless test's key or click "does nothing".

### A menu command, run headless (2026-11-22)

`{"invoke":"command.id"}` on the app's socket
(`$XDG_RUNTIME_DIR/olo-menus-PID.sock`, a line of JSON in, one out)
runs a menu command of the app's front window, as choosing it would;
`{"menus":true}` lists them with whether each is enabled (`invoke`
doesn't ask: check `enabled` first if the test is about that). A
window the command opens has the keyboard, so `tsession type` and
`key` go to it.

### A push refused for GitHub's LFS budget (2026-11-22)

`git push` ended at once with "batch response: This repository
exceeded its LFS budget. The account responsible for the budget should
increase it to restore access": GitHub takes no new LFS object (a
journal picture, a wallpaper, a model) until the person raises the
budget in their GitHub billing settings; only they can. It says
nothing of the commits: a push that brings no new LFS object goes
through. What had used it up was the journal's page (above:
journal.html, 69 MB, a new copy with every entry), no longer kept.
While it lasts: commit the work without its journal entry and push
that; keep the entry (the picture, `entries.json`, `README.md`) as a
commit of its own on the local branch `journal-pending`, each new
entry on top of the last, and bring the branch onto main (rebase, then
fast-forward) once a push with a picture goes through again. Never
skip the LFS upload (`GIT_LFS_SKIP_PUSH`, `--no-verify`): the commit
would arrive pointing at a picture GitHub doesn't have. Tell the
person; `git lfs push --dry-run origin main` lists what a push would
upload.

### What a drag keeps must outlive a layout (2026-11-22)

Rex's split dividers kept their drag (where the pointer took it, the
ratio then) in an `Rc<Cell>` made in the view's `body`, and each move
changed the ratio, which said the view again: the handlers of the next
layout had a new, empty cell, and the drag ended at its first move. A
control's state between its handlers belongs to the element
(`cx.node_state::<T>()`, with an `.id(..)` that is the same in the
next layout: `SlimScrollBar`'s thumb, Rex's `SplitDrag`), or to the
view. Test a drag in several moves and read the value back after each
(`tsession down`, `move`…, `up`; the Splitter's value in the tree):
one move passes with the bug. And two things sharing a row by a ratio
take `flex_grow` the ratio with no basis: with a basis of the ratio
times a thousand points and equal growth, they are right only in a
row a thousand points wide.

### Setting a host up for Rex, and trying it (2026-11-22)

- **Rex's own instructions are in four places**, and they agree: the
  comments of its units (`rpm -ql rex`, then read
  `/usr/lib/systemd/system/rex*.service`: the login server's says how
  it is started, what its accounts file is, and why the shared server
  and it can't both run), `/etc/rex/accounts.jsonc`'s own page of
  comments, `rex server install --help` and `rex login-server --help`,
  and its documentation (`https://www.superlogical.com/rex/docs/`,
  pages `linux/installation`, `linux/server-modes`,
  `linux/login-server`, `linux/shared-server`, `linux/remote-access`,
  `linux/upgrading`; they are public and fetch with curl). Its
  installer is `https://pkgs.superlogical.com/rex/install.sh`
  (`--dry-run` prints what it would do; `--channel`, `--user`,
  `--tarball`).
- **The installer takes its own default channel** when a copy
  installed alone recorded none: a host on the snapshot channel was
  given the preview's package (an older build of another version).
  Ask for the channel the host's `rex version` names
  (`provision::Facts::channel`); switching back is the installer again
  with `--channel`.
- **One copy of Rex on a machine**: the bare one in `/usr/local/bin`
  comes first on the PATH and is the one every `rex` command and
  personal server runs, whichever is upgraded. Install the package,
  stop the server the bare copy started, remove the copy.
- **Over SSH a person reaches their own server** in every mode but
  the shared one (`rex __internal dial-stdio` as that account): the
  login server is for callers known another way (Tailscale), and on a
  machine with logind an account's server under it is the same
  personal server. So "login mode" for a person who connects over SSH
  is: the package, the account listed (`peer:UID`), the login server
  enabled, and the account's own server kept (`rex server install`).
- **A popup menu isn't a window of its own to a test**: `list_windows`
  has the window it belongs to, and the app's tree has its items with
  no place to click. Open it with a click and choose with the keys
  (`tsession key NAME 108` down, `28` Return).
- **`{"type":"close","id":N}`** on a session's app_server closes a
  window by its id (a stage of a dialog that offers no way out).
- **Two runs of a picture script with one session name stop each
  other**: a run sent to the background by a timeout was still going
  when the next began, and its `finally` stopped the session under the
  other ("Rex's window didn't come"). Wait for the first, or name the
  sessions apart.
- **Keep a test host tidy as you go**: each run that makes a session
  there ends it (`rex ls`, `rex kill ID`), and the check that puts the
  host back to "not set up" does it by the same commands a person
  would run, not by restoring files it didn't write.

### A real remote host for Rex (2026-11-21)

Ben gave a server "just for rex", reached over SSH with a key the
Passwords agent holds (this shell's `SSH_AUTH_SOCK`), to do with as
needed. Its address is not written in the repository, a commit or a
brief: keep it in a file of your scratch folder and read it from there.

- **It finds what a loopback fixture can't: time.** One round trip
  there is 224 ms. Typing waited for an answer a key (65 characters,
  18.7 s); nothing local had shown it. Measure by the host's own clock
  (the remote shell writes `date +%s.%N` to a file when the typed line
  runs), not by polling over a second SSH call.
- **Use a known-hosts file of your own** (`-o UserKnownHostsFile=…`,
  `StrictHostKeyChecking=accept-new`) and a scratch HOME for the app,
  so the person's `~/.ssh` is neither read nor written; the app then
  asks about the host's key, which is a path worth testing.
- **The agent may stop signing** (Passwords locked while the person is
  away): `ssh -v` stops at "Server accepts key" and hangs. Nothing to
  do but wait for the person; never type into Passwords. Every call has
  a `timeout`, and a test says "the agent didn't sign" rather than
  failing a step later.
- **A window's text fields are `TextInput` in the tree**, each by the
  name its app gave it (`TextField::name`): Rex's had none (three
  nameless fields in Hosts, 2026-11-22), and a script that looked for
  "TextField" found nothing. Print the roles a window has before
  writing a check against it (`{"tree":true}` on the app's socket
  answers with a list, a tree for each of its windows).
- **A program left running there is started apart from the SSH call**
  (`setsid -f sh -c '…' </dev/null >/dev/null 2>&1`): started with `&`
  alone, ssh waits on it and the call never returns. And ended by a
  pattern that can't match the shell carrying it (`pkill -f
  '[h]ttp.server 8765'`).

### While a push's hook runs, build nothing (2026-11-21)

The pre-push hook's live lint starts each app in a headless session and
waits for the machine to have room; with whole test suites building in
a worktree beside it (and the person's other work), one app's session
timed out after forty minutes and the push failed: "DiskBackup: the
lint failed: timed out". Read and plan during a push; build after it.
Again 2026-11-23, in its rest check: a build beside it slowed the
Inbox's provider to half the check's changes ("the check itself is
off"). The check now waits for room as the lint does and says the load
when it fails; a push with a build beside it is still slower: wait for
its completion notice. Again 2026-11-24, with the rig's cases run beside
it: the lint waited for room ("processors short 44%"), the hook ran
eleven minutes, and GitHub closed the push's connection meanwhile
("Connection to github.com closed by remote host", exit 141, nothing
pushed though the hook passed). And again the same day with nothing of
ours beside it (others' builds had the load at 50): the cause was the
hook itself, checking for minutes with the connection open. Push with
`tools/dev/push` (in the background), which checks before connecting;
the hook then passes the commit at once (HACKING, "With every commit
and push"). Building beside the checks only slows them now.

### A soak counts threads and descriptors, not only memory (2026-11-21)

- **What a program holds after load is three numbers**: resident
  memory, open descriptors (`/proc/PID/fd`) and threads
  (`/proc/PID/task`), each before, at the peak and at rest after. A
  build that gave its memory back (62 MB where 238) held 278 threads
  and 177 sockets after 200 tabs: memory alone said it was better.
  Name what stays (`comm` of each task, `readlink` of each descriptor)
  before guessing whose it is: the names said "notify-events" and
  "zbus", which was the kit's notifier, not the app under test.
- **A test desktop has no notification service**, so every notice an
  app posts there fails: which is exactly the case that leaked, and
  the case a real desktop is in while the service starts again in an
  upgrade. A program's behaviour with a service away is worth a run of
  its own.

### Codex's hooks and our agents (2026-11-21)

- **A new or changed hook in `~/.codex/hooks.json` stops an
  interactive Codex at its start** ("Hooks need review … Trust all and
  continue"): two subagents sat there doing nothing until looked at
  (`agents list` said "idle" from the first second). The person's
  hooks are for their own sessions and theirs to trust: our Codex
  agents ran with `--disable hooks` (the helpers are agy now, with
  none of the person's hooks or settings). An agent idle within a
  minute of its start: `agents peek` it before anything else.
- **`agents forget NAME` removes everything in its folder**, what the
  agent gathered too (notes, captures), not only its builds: copy what
  the next agent will want before forgetting one.
- **The person's other servers**: a tool of theirs that this desktop
  has a client for (their terminal-session server) runs on this
  computer with their work in it. Agents get a scratch one (an
  explicit socket, a home of its own, or a bwrap box as
  `crates/rex/tests/live.rs` has it), and pictures that go to outside
  reviewers hold no shell prompt (it has this computer's names).

### What the layout review does and doesn't say (Tracker's columns, 2026-11-21)

- **A fill drawn over a line is not a finding**: a row's highlight a
  point too wide covered half of the line beside it at 2× and all 60
  runs of the review passed. Seams (a device pixel apart) it finds;
  overlaps of paint it doesn't. Read the pixels across an edge
  (`Image.getpixel` along a row of a `tsession shot`), at every scale.
- **A thing to click thinner than a point is "collapsed"**, and one
  that overlaps another thing to click by two points is an overlap: a
  splitter's place in the agents' tree is a point wide and between
  its neighbours, whatever its grip is to the pointer.
- **A width taken from the frame before lags a resize by a frame, and
  nothing asks for the next one**: lay out by `cx.viewport_size()`,
  which is the window as it is now, not by a scroll view's `bounds()`.
- **`style::shade` darkens in the dark looks too**: a line "darker
  than the ground" sinks into a dark ground. A line that is to show in
  every look goes part of the way from the ground to the text's
  colour (`olo_theme::mix`).
- **`tools/dev/kernel-check` and other tools with no argument run
  their default case**: ask with `--help`, or read the file's head.

### A stand-in shaped like the real thing

- **The check's Codex ran outside every tab; the real one's server is
  a child of the first Codex window** (2026-11-21). The headless check
  of agents' tab titles passed all along while titles went to the
  wrong window on the person's desktop: its stand-in was "a
  daemon of its own", and Codex's daemon isn't one. Before writing a
  stand-in, look at the real program's place in the process list
  (`ps -eo pid,ppid,comm,args`): who started it, what it is called,
  whose child it is.
- **A check that never failed proves nothing until it has**: the new
  case was run against the build from before the fix, and failed
  there, before it was trusted to pass.

### A guest's last line, cut

Learned from one failed check (2026-11-21), with the second
investigator, who made it happen at will:

- **A serial port throws away what it hasn't sent yet when its last
  holder lets go without waiting, and a program ended by a signal
  doesn't wait.** A guest's script printed its only row and then
  stopped the programs it had started, one of which had the port as
  its output: the row came back 128 characters of 172 (eight loads of
  the port's sixteen), the script's own end 0, the tool's too. It
  showed in one run of three and not in any of twelve tries to force
  it with long lines and a busy computer. A guest switched off with
  the port still held loses characters as well.
- **`tools/dev/kvm-guest` now holds the port open itself and waits
  for it to have sent everything before the guest ends** (`stty -F
  PORT "$(stty -F PORT -g)"`: a setting is applied once the output
  has gone out, and this one changes nothing). A first guard (end
  what the script left, look until nothing is left, close last) was
  shown wrong by the investigator in a quarter of an hour: a program
  whose memory is gone hasn't let go of its files yet.
- **A row that isn't whole is a string, not a failure of the tool**:
  `kernel-check` said "didn't finish cleanly" with the cut row in the
  message, which is how it was seen at all. Read the message to its
  end before deciding what it says (mine was cut again by my own
  `cut -c1-300`).

### What a program holds: measured in turn, and a first run apart

Learned measuring the allocator (2026-11-21):

- **A before and an after an hour apart measured the GPU's shader
  cache.** Deskbar held 27 MiB of its own in the first two runs of a
  GPU test desktop and 11 in every run after, whatever was set: the
  first runs compiled its shaders (Mesa, in the program, in its
  threads' arenas) and the later ones read them from
  `~/.cache/mesa_shader_cache`. The allocator's setting was credited
  with 16 MiB it never saved, until the two were run in turn. With
  `MESA_SHADER_CACHE_DIR=` an empty folder every run is a first one.
- **In turn, or it isn't a comparison**: A, B, A, B, each for the same
  time after start (a session's memory at 8 s and at 25 s differ).
- **A switch tried through the environment reaches what you think it
  reaches only if you look**: `tr '\0' '\n' < /proc/PID/environ` of
  each program measured.
- **Whose memory**: `/proc/PID/smaps_rollup` for the sums (Pss,
  Private_Dirty), `smaps` for where: `[heap]` is the main thread's,
  an anonymous mapping starting on a 64 MiB boundary is another
  thread's arena (followed by the rest of its 64 MiB, unreadable and
  unused), a large one elsewhere a block mapped by itself.
- **The bench's `cpu_ms` is the app's; the compositor's own cost over
  a run** is its process's time at the end (`bench --keep`, then
  `/proc/PID/stat` of the session's app_server, then `tsession stop
  bench`).

### A second thread that does kernel work for a first

Learned from two reviews of the background group's keeper
(2026-11-21), each of which broke the form before it in a guest:

- **What the kernel was asked can't be called off, and takes the
  asker's time**: a write of `memory.high` under what a group holds,
  a write of `memory.reclaim`, anything that ends in the disk's
  queue. Whoever must stay responsive doesn't make such a call; and
  whoever makes it for them acts for a decision that may no longer
  stand by the time the call starts. Give every ask the decision it
  is for (a number that changes with each decision), read the state
  again right before each call, and at the end wait for that thread
  before saying it is over (the reviewer paused the thread just
  before its write, stopped the keeper, and resumed it: the "lifted"
  the keeper had reported was no longer true). The check and the
  syscall are separate. A newer look can run between them. A process
  allocating against an installed high can also swap without a taker
  request. Exercise both paths with the allowance read from the kernel.
- **What must never happen is the kernel's to refuse, not the
  program's to be quick about**: three forms tried to keep "the
  background's own memory isn't swapped unless the rule says so" by
  deciding often and checking before each call. Each check is one
  moment and the call another; and the kernel does the forbidden
  thing by itself between two looks (a program growing under its
  allowance). A swap ceiling set at every look holds whoever acts and
  whenever. Look first for the limit the kernel already enforces.
- **And a limit the kernel enforces, it enforces against everyone**:
  the ceiling that kept the background's own out of swap kept it out
  of the kernel's reach when the desktop suddenly needed more than
  there was, and the kernel took the desktop's (121 MB to 4). A hard
  limit is right only where nobody else can come to need what it
  withholds: closed with memory to spare, open before the line.
- **What looks ample after the fact was paid for**: `MemAvailable`
  was back over the line once the kernel had swapped the desktop
  out, so a rule that read only that saw no shortage. Count what the
  shortage cost (the desktop's own memory in swap) as still missing.
- **The reviewer's own patch gets reviewed too**: the third form was
  the second review's tested patch, taken whole, and the third review
  (the same reviewer, asked to fault it once more) held it.
- **A balance isn't a flow**: "what went to swap lately" was counted
  from a balance by adding up its rises. A balance that goes up and
  down adds up to anything (909 MiB "gone" around a gigabyte that
  hadn't moved). From a balance, count the net; and say that what
  came and went between two readings isn't seen.
- **What is tuned by the look changes when looks come sooner**: the
  cache's credit halved at each look that saw the desktop wait, and
  the retry of a part waited "until the next look". Both were tuned
  at four looks a second; at twenty the credit went five times as
  fast and one stage never got its turn. Whatever depends on how
  much time went by takes the time, not the count.
- **A check that passes when nothing happened shows nothing**: "the
  background's swap is no less than the desktop's" held at nothing
  and nothing; and the guest it ran in couldn't reach the state it
  was named for (640 MB is all within the band: the ceiling never
  closed there). Ask of each check what it would say if the thing
  under test did nothing at all, and of each scenario whether it can
  get where its name says.
- **Words reviewed like code**: the fifth and sixth reviews held the
  comments' "never", "always" and "first" against what the code
  does, and half didn't stand ("attempts", "is meant to", "mostly").
  A sentence about a property is a claim to test like any other.
- **Two readings of a figure are two figures**: a decision made on
  one and applied to the other invents a difference (the group grew
  between them, and that growth was taken for memory to swap).
  Carry what the decision allowed, not only what it decided.
- **A check that passes with the thing absent proves nothing**: the
  slow-swap check passed with no swap at all (its wait fell through
  on a timeout). A check says what it saw happen (swap started, the
  disk wrote) before it judges what followed.
- **`kill $!` after `f() { ( … exec "$@" ); }; f … &` kills the
  shell that called `f`, not the program**: start what is to be
  killed as `( … exec … ) &` itself. (The check's "desktop lets go"
  let go of nothing.)
- **The reviewer's provider can end a run** (its content filter did,
  on a brief that asked to "break" kernel code): ask for the answer
  to be written as it goes, keep its notes, and word a brief as the
  reliability review it is.
- **Two builds measured one after the other measure the hour too**:
  the first series (all of one, then all of the other, the review's
  guests beside the second) showed a worst round six times longer
  that the builds in turn (`OLO_KBENCH`, pair by pair) didn't.
- **`kernel-check groups 2` is three runs**: a bare number was never
  its `--reps` (its usage said `[N]`; it says `--reps N` now).
- **A kernel from before a feature, really booted**: a Fedora
  release's first kernel stays in its archive
  (`archives.fedoraproject.org/pub/archive/fedora/linux/releases/42/Everything/x86_64/os/Packages/k/`:
  `kernel-core` and `kernel-modules-core` are enough for a guest
  with a disk and zram); `rpm2cpio | cpio -idm` into a scratch
  root, `depmod -b ROOT RELEASE`, then
  `OLO_GUEST_KERNEL=ROOT/lib/modules/RELEASE tools/dev/kernel-check …`.
  The archive can crawl (one package came at 6 KB a second and the
  next at once): `curl -C -` to go on with what came, and `rpm -K
  --nosignature` says whether it is whole.

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
  reviewer reads the tree** (`agy-help`): compare by copying the one
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
  `olo-debug-help` skill (`tools/dev/agy-help BRIEF.md`: an agy in a
  tmux session, read with `tools/dev/agents peek help-FOLDER`).
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
- **jato was tried in kache's place, twice, and Olo stays on kache
  for now** (Ben, 2026-11-21: "please start using jato instead of
  kache", then "if you find jato is slower or worse than kache then go
  back to kache and tell the astra 6.2 max agent working on jato what
  it needs to improve/fix"). jato is the person's own build cache
  (`~/src/jato`; `jato build …` in `cargo build …`'s place, or
  `RUSTC_WRAPPER="$(command -v jato)"` for a tool that starts cargo).
  - 0.8.0 cached nothing here: all 251 compiler calls were bypassed as
    `unknown_arguments` (its parser had no case for `-Z`, and every
    unit of Olo carries one: `-Z embed-metadata=no`, which nightly
    cargo passes by itself on every library, and `-Zthreads=8` from
    `.cargo/config.toml`).
  - 0.9.0 takes those and caches, and was timed on `olo-system` (dev,
    232 crates, scratch target folders, a busy computer): no cache 149
    s; jato's first build 202 s; jato with the folder emptied and
    built again at the same path 127 s, then 97 s (123 units hit, 127
    missed, each time); kache 35 s into a folder it had never seen and
    2.0 s rebuilt. What never hits: whatever is linked through mold
    (every build script and proc-macro crate, a fifth of the units),
    whatever takes a proc macro, crates with assembly, and everything
    above those (`JATO_LOG=debug` says why, a line a unit; `jato stats
    --json` before and after says how many).
  - Its agent has both reports (`tools/dev/codex-say SESSION now
    FILE`, the olo-codex-message skill: `codex queue` alone left the
    first one waiting eighty minutes behind the turn the agent was
    in), and answers in a file in scratch. To try a newer one: the
    same three builds (`rm -rf` the scratch `CARGO_TARGET_DIR` between
    them), kache's two beside them, in one script so that they share
    the computer's load, then the workspace's release build.
  - Known besides: `jato limit SIZE` (0.9) sets one budget for
    everyone who uses the store, where before each run evicted to its
    own `JATO_MAX_SIZE` (1 GiB unless said); it keys absolute paths,
    so a second target folder reuses nothing of the first's without
    its worktree mode (`jato worktree build … --artifact-dir`, not
    tried: Olo's tools and the running desktop look in `target/debug`
    and `target/release`). While kache is still wired into the
    person's shell and cargo config (its shims first on PATH, among
    them `cargo`, `cc` and `rustdoc`;
    `CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER`;
    `build.rustc-wrapper`), `jato …` takes the wrapper's place all the
    same (the variable wins); to measure jato alone, take the shims
    off PATH, unset the runner, and set `HOST_CC=cc HOST_CXX=c++
    RUSTDOC=rustdoc` for the command.
- **Old builds go by themselves, on kache's terms** (Ben, 2026-11-21:
  "can you make it so old builds we no longer need will get cleaned up
  automatically"). Its daemon removes a whole build folder no build
  has used for a day and, from those that stay, the units no build has
  used for three (`auto_clean_unused_units_days` in
  `~/.config/kache/config.toml`; kache's own default is 30). Under 30
  the incremental compiler state it keeps for each unit
  (`target/debug/incremental.kache-auto`) had reached 161 GB in
  sixteen days, 135 of it not used for three: a unit's name takes in
  every argument to rustc, so each change to a crate's dependencies
  leaves its state, and all its dependents', unused for good. It
  cleans at most hourly, and only in a minute when no cargo command
  runs anywhere on the computer (three passes between the 18th and the
  21st): `kache clean -n` says what the next pass takes, and nothing
  forces one. What it doesn't know it never cleans: a subagent's
  `target/` (`tools/dev/agents` removes it a day after the agent's
  last build) and a build folder its registry forgot (`kache clean
  DIR -n` finds those: two worktrees' were 70 GB, the older idle for
  eight days). Never delete under a `target/` by hand while builds
  run: its units are locked one by one. `/var/tmp` is aged by the
  system at thirty days (`tmp.conf`), so scratch left there goes too,
  late: remove your own when you're done all the same.
  Its clean-up takes one lock for the whole computer, which every
  Cargo command through its shim holds for as long as it runs: while
  another project's `cargo watch` commands run (from login to
  logout) kache cleans nothing, here or anywhere, and there is no safe
  way round that lock yet (TODO; FORK.md). `tools/dev/prune-graphs`
  reclaims idle incremental graphs at a moment when no Cargo command
  runs (it takes that same lock: with the watchers up it skips); its
  first form went by Cargo's and the units' own locks instead, ran
  once on this repository, and was shown wrong by review before it
  ran again (its own text says how).
- **Something of Olo's that changes the kernel's behaviour: three
  lessons from `olo_gpu_pages`' reviews** (2026-11-21; each was a
  failure made to happen in a guest, not an opinion). (1) *Nothing
  real before the self-check has passed*: a probe registered live and
  then checked had changed 5,539 of another task's allocations on a
  kernel it went on to refuse. Register closed (the handler compares
  an address with the module's own test object before it reads
  anything), check, then publish. (2) *The check goes through the
  same test as the real thing*: a check with its own way round the
  selector passed with the selector broken. (3) *What a module can't
  check as it loads is checked before it is loaded*: where a build
  keeps a private function's arguments is a property of the build, so
  the service loads it only into builds on a list, each looked at
  (type information, machine code, callers) by a person, a tool
  helping. And a green
  check proves little until its assertions have been attacked: the
  reviewer deleted rows from the tool's input and it still passed.
  Three more, from the third review: (4) *run a tool through its own
  command before saying it works*: `--qualify`'s last step called a
  number (the guest's exit status had taken the function's name), and
  only its parts had been tried by hand; (5) *a tool that loads
  something must be unable to leave it working*: the trial's module is
  on only while the trial holds a descriptor of the module's open, so
  a kill it can't catch switches it off (the kernel closes what a dead
  process held; nothing in the tool's own cleanup is needed for it),
  and the tool is itself run in guests and killed there; (6) *who
  loaded it owns it*: a service that finds its module in the kernel
  must know it is its own load (a name given at load and kept) before
  it switches, counts or removes it; "a module of that name is in" is
  not that. And from the fourth: (7) *a test that makes its own
  warning tests its own word for one*: the trial tool's test wrote
  "WARNING:" into the kernel's log and the tool stopped; a real
  warning of the driver's, at warning level without the word, went
  by. Test with what the kernel really says (its sources have the
  strings), and by the level, not the wording. (8) *`journalctl -g`
  ends 1 when nothing matches*, exactly as when it can't read the
  journal: ask for the lines and pick them out yourself.
- **A lock taken with `flock` is let go, not only closed**
  (`olo_system::background`, 2026-11-21). It belongs to the open file,
  and every child is born with a copy of every open file until it
  becomes its program: close yours while another thread is starting a
  program, and the lock stays with that child for a moment; the next
  to ask is told another holds it. A test of the keeper failed once
  under load (other tests starting programs beside it); a test of
  just that (four threads starting /usr/bin/true, the lock taken and
  dropped 20,000 times) failed at once, and passes with `LOCK_UN`
  before the close. Whoever comes after a holder that died asks more
  than once, for the same reason.
- **An edit script over several files: assert everything, then write
  everything.** A Python edit that writes file A, then fails an
  assertion on file B, leaves half a change (three times in one
  evening, 2026-11-21; once the build still passed and hid it). Read
  all the files, make every replacement in memory with its count
  asserted, and only then write; and assert against the text as
  rustfmt left it (it reflows what was written a minute before).
- **A word has one translation, whatever it means where it's shown**
  (the catalogs are keyed by the English: "Drawing" is a PDF
  annotation, a frame's drawing time and a box's title). Before
  reusing a short word as a title, read its translations
  (`tools/dev/words pairs LANG`); a wrong sense in one language is a
  reason for another English word, or for a translation that serves
  every use.
- **A row's value that doesn't fit ends in an ellipsis, and no review
  finds it**: `tsession review` and the lints say nothing of a
  `Facts` value cut short. After translating, picture the window in
  every language (`OLO_LANGUAGE=fr …`, one sheet of crops) and read
  the rows; a paragraph that wraps to one more line is fine, a cut
  row is not. Six of twelve languages cut one row here.
- **A push builds the release workspace only for what the lint looks
  at.** The pre-push hook builds it (and so offers the person the
  upgrade) when `apps/`, the kits or `vendor/` changed; a push of
  app_server, the protocol or a library alone lands in a second and
  builds nothing (the measure of a key to its frame, 2026-11-21: pushed,
  and nothing was offered until the workspace was built by hand). After
  such a push: `cargo build --release --workspace`, in the background.
  With builds given only spare processors (the session's background
  group, when the person has that on) it can take half an hour beside
  other agents' work: that is the setting doing what it is for.
- **A lock that looks wider than it need be: have the narrowing broken
  before it is installed.** kache's whole-computer lock looked like
  caution to spare (each unit has a lock of its own, Cargo has its
  own). Two narrowings, each with its tests passing and an argument I
  believed, were each broken by Astra with real Cargo inside the hour:
  a test binary gone between its build and its run, and a build's
  graph deleted under the build's own lock after a `cargo clean` had
  replaced the folder (2026-11-21). What the tests held still was what
  broke it: a folder replaced under held locks, a command starting
  after the check. The brief that got those was "try to break it,
  with a concrete sequence of events", and the diff. A third time the
  same day, with a tool of Olo's own that did the same by hand
  (`prune-graphs`, through folder descriptors this time): an
  interrupted `cargo clean` leaves the unit's folder with a new lock
  and graph in it, and the descriptor reached those. Three arguments
  I believed, three counterexamples: where a deletion races other
  programs' deletions, the lock those programs take is the only
  exclusion there is, however wide. (And the reviewer's service
  refused the third brief once for its wording, "I want it broken":
  ask for a correctness review, with the interleavings to check.)
- **The history was rewritten on 2026-11-07**: commit ids from before
  then are dead (a map of old to new is in `~/backups`). Never bring the
  old history back.
- **Testing on Amazon and sites that bar bots is fine** (Ben): look
  only, no shopping actions.

- **The clock is on Pacific time.** Calendar's demo events are on the
  person's own clock wherever they are, but one on another clock (the
  demo's Call with New York) lands at that zone's time here: 9 AM, not
  the 6 PM it is in Amsterdam.

### systemd's user manager: what a transient unit keeps, and what a test leaves there

- **A property given at a unit's making loses to Fedora's drop-ins.**
  What `StartTransientUnit` is handed is the unit's own file; a
  drop-in for every unit of its kind comes after it and wins
  (`/usr/lib/systemd/user/slice.d/10-oomd-per-slice-defaults.conf`:
  `ManagedOOMMemoryPressure=kill`, 80% for 20 s, on every slice of a
  person's). Only a property set on the unit afterwards
  (`SetUnitProperties`, a drop-in of the unit's own) wins over that.
  Read the value in force (`systemctl --user show UNIT -p NAME`, and
  `oomctl dump` for what systemd-oomd watches) on a unit made fresh
  under a new name: one that was already there had been through the
  other road, and passed (2026-11-20).
- **An empty transient slice stays.** It is "active" with nothing in
  it until it is stopped: a test that makes one stops it at its end
  (`StopUnit`, or `systemctl --user stop`). Nine of my tests' and
  trials' stayed in the person's manager, eight of them on
  systemd-oomd's list (removed, 2026-11-20).
- **A group in the person's manager competes for the processors as
  the manager.** The manager's group and the login session's scope
  are siblings of the same weight: `CPUWeight=idle` on a slice there
  is idle among the manager's own slices only, and nice orders
  nothing across groups. Background work moved there gets more of
  the processors against the desktop than at nice 10 beside it
  (kernel/TUNING.md, "The first build, and why it is held").
- systemd-oomd here watches `system.slice` and each slice of the
  person's manager for pressure (80%, 20 s); not the manager's own
  group, nor the login session's scope. Swap: nothing watched.

## Working with the repository

- **A shell script isn't edited while it runs.** bash reads a script
  as it goes, by its place in the file: the helper's script (then
  `tools/dev/codex-help`), rewritten while a helper it had started was
  still at work, read on
  from the same place in the new text when the helper ended and
  stopped with a syntax error "at line 96" in lines that were sound
  (2026-11-20; the helper's answer was already written, by luck, and
  the exit code said 2). Before changing a tool under `tools/dev`,
  see that nothing is running it (a background task of this session,
  `tools/dev/agents list`); or write the new text to a file beside it
  and rename that over it, which leaves the running shell its old
  file.

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
- **Every helper is an agent in tmux** (`tools/dev/agents`, the
  `olo-agents` skill, which has the details; Gemini 4 Neon at high in
  `agy`, and Opus 5.5 in `claude` for the design review, since
  2026-11-22: Codex and Astra before; the look at pictures Astra 6.2
  at high in `codex` again since 2026-11-23). What codex-cli 0.171
  does for a look: drawn inline (`--no-alt-screen`), "Working (…  esc
  to interrupt)" stands above its field, never on its last line, so
  `state` looks in its last eight lines; started in a folder it hasn't
  seen it asks whether to trust it and waits, which `-c
  projects."DIR".trust_level="trusted"` answers for that run alone;
  `-a never` with `-s workspace-write`: what its sandbox refuses is
  refused, and nothing is asked. What agy 1.5.14 does that its
  help doesn't say:
  - With no `--mode` it starts in `auto`, which judges for itself what
    is safe and runs a command outside its sandbox when the sandbox
    refuses it: `rm` and a `sudo -n true` went through unasked. Our
    agents run in `accept-edits`, with lists.
  - `--settings FILE` is for that run only and lies over the person's
    `~/.gemini/antigravity-cli/settings.json`: `trustedWorkspaces`
    (without the folder there, agy asks "Do you trust the contents of
    this project?" at its start and waits), `permissions.allow` and
    `permissions.deny`, lists of `command(PREFIX)` (also
    `read_file(/abs/path)`, `write_file(/abs/path)`, `mcp(*)`). A
    compound command (a pipe, `&&`) goes ahead when each part is
    allowed; output sent to a file with `>` asks whatever the command;
    a denied one fails at once, unasked.
  - Its question's third choice, "… (Persist to settings.json)", writes
    the rule into the person's settings: the tool refuses to take it.
    (A trial of mine took it once, for `ls`; put back the same hour.)
  - `agy --print` answers only at its end, and nothing shows whether it
    is thinking or waiting: a look at three pictures sat silent for
    eight minutes. Never used (CLAUDE.md, "Every helper in a tmux
    session").
  - While it works, a message typed to it is queued ("alt+m to send
    immediately"); its last line then reads "Press up to edit queued
    messages", not "esc to cancel", and only its spinner says it is
    working. `agents say NAME --now` sends alt+m.
  - Its banner prints the account it is signed in to: `agents` leaves
    the banner's lines out of everything it prints. Don't
    `tmux capture-pane` an agent's session by hand into a file that is
    kept or sent.
  - At xhigh, a look with nothing to measure with went round three
    pictures for ten minutes without writing a word; at high, with a
    brief that says to judge by eye and write, minutes.
  And the Claude CLI (2.1.330), for Opus's review: `--tools` and
  `--add-dir` take every word that follows them, so the prompt goes
  after an option that takes none (it was taken for a folder to add,
  and the session sat at "Is this a directory you created or one you
  trust?"); `--restricted` keeps its file tools to the folder it
  starts in, and leaves out the person's settings and hooks.
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
  hazard: don't load the GPU for seconds on end (many sessions
  started with `TSESSION_GPU=1` drawing) while such screens are up and
  the person is at the machine. A test session draws in software now
  (HACKING, "Test desktops draw in software": the live lint's
  desktops and their programs hold no GPU device open, checked), so
  the hook's lint and a push are safe again. What
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
- **A guest's timings are the host's when the host is busy**
  (2026-11-20): `kernel-check`'s wake figures from guests run at a
  load of 20 to 30 had tails of milliseconds that weren't the
  setting's (a guest's processors are made to wait too), and said the
  opposite of the same test run on the computer itself. Costs
  (throughput) survive it roughly; latencies don't. Measure latency
  on the computer when it's busy (`kbench relay` beside `kbench spin`,
  each setting in turn, several rounds), or in a guest when it's quiet.
- **`vm.swappiness` means three different things** on this kernel
  (7.2; mm/vmscan.c): under the multi-generational LRU, 1 is "files
  first" a generation at a time (idle programs still go to swap under
  a build) and 0 is "never" (a program is ended with swap free); only
  under the older reclaim (`echo 0 > /sys/kernel/mm/lru_gen/enabled`)
  is 0 "when the cache is all but gone". kernel/TUNING.md, "Swap used
  last (a setting for an hour)".
- **Never change memory policy on the person's computer to see what
  it does** (2026-11-20, the worst hour of this project): "swap used
  last" had passed every guest measurement, was put on on Ben's
  loaded computer, and within six minutes NVIDIA's driver had failed
  an allocation, then its runtime suspend, and the card was dead with
  their work on it ("did you just kill my nvidia gpu?? i was using
  it"). A guest measures programs, whose allocations wait; it has no
  drivers whose allocations may be refused, no card that suspends,
  and no 30 GB of the person's own work in memory. For anything that
  changes how memory, swap or reclaim behave (a sysctl, a cgroup
  limit or protection, `swapoff`, huge pages): the guest first; then
  the person's computer only when they say so, at a quiet moment,
  with the minute record running
  (/var/tmp/claude-blueferry/mem/sample.py: free, cache, swap in and
  out), a way back written down first, and never anything that
  reduces what the kernel may reclaim or swap in total. Putting it
  back at once (as was done) doesn't bring back what a driver lost.
- **A fix that calls into a driver can hang for ever, in the kernel**
  (the same day): Doctor's "Start It Again" for that card unloaded
  NVIDIA's display module while the card didn't answer, and
  `delete_module` never returned (nvkms waits for the GPU with no way
  out, a whole processor spinning; the module left "Unloading", the
  helper unkillable, Doctor's window saying "Fixing…"). Before
  unloading or unbinding a driver, know that its device answers, and
  never make such a call on the only thread that reports progress.
  How to tell where a thread that never sleeps is: `/proc/PID/stack`
  is empty for it; `perf record -g -p PID` for a few seconds as root
  names the functions.
- **What holds programs "making room" is mostly 2 MB pieces, and
  not always ones the program asked for**: `sudo
  tools/dev/memory-holds` before blaming memory being short. A linker
  asks for huge pages itself (`madvise`; mold for its allocator's
  arena and its output) and waits for them at its page faults; a
  thread that *draws* never asked, and is held just the same, because
  the GPU driver's memory pool asks for 2 MB pieces on its behalf
  (TTM under xe: kernel/TUNING.md, "Graphics memory in 2 MB pieces").
  The first trace of this (2026-11-20) counted by program only and
  the write-up took Chrome for a program asking for huge pages; the
  kernel's stack at the hold (`kstack` as a map key) said whose
  request it was a day later. Count by path, not only by name.
- **A thread's name is what a trace calls `comm`**: a kit app's
  drawing thread is "display" in every app, a window's thread
  "w>" and its title (the person's: it stays on this computer), and
  only a process's first thread has the program's name. The threads
  named "display" that were held 76 times in one trace were Olo's own
  apps, read at first as some other program's.
- **Memory "pressure" here is mostly not shortage**
  (`/proc/pressure/memory`, the "waited for … memory N%" in
  app_server's lines): it counts linkers compacting for their own
  huge pages and the background group's compilers reclaiming under
  their allowance. `pgsteal_direct` in `/proc/vmstat` counts the
  group's reclaim too (3 million of its limit's events had "stolen"
  1.5 TB in four hours with nothing short); `allocstall_*` and
  `compact_stall` are the whole computer's.
- **One process without huge pages, the computer unchanged**:
  `prctl(PR_SET_THP_DISABLE, 1)` before it starts is not enough for
  mold (it calls `prctl(PR_SET_THP_DISABLE, 0)` itself, and
  `madvise(MADV_HUGEPAGE)` on its own mappings whatever its
  allocator's options say). A seccomp filter that answers 0 to those
  two calls without making them is (`SECCOMP_RET_ERRNO` with errno 0;
  give the other way the same filter with both let through, so both
  pay for one). `/proc/PID/status`'s `THP_enabled` and
  `smaps_rollup`'s `AnonHugePages` say whether it took.
- **bpftrace 0.26 here**: a `kprobe` and `kretprobe` pair on a
  module's function recorded nothing on the way out (no error; two
  probes fewer "attached" than written); `fentry:ttm:NAME` and
  `fexit:ttm:NAME` work (the module has type information:
  `/sys/kernel/btf/ttm`). Without root it won't even list probes: a
  script's first run is as root, so keep to forms already run.
- **Run tests without the person's display in the environment**
  (`env -u WAYLAND_DISPLAY -u OLO_SOCKET cargo test …`), as the commit
  is made. A unit test can no longer connect to their compositor (the
  kit connects only for a program that runs an app, 2026-11-20), but
  what a test starts as a child still can.
- **A dead NVIDIA card crashes programs that only list devices.**
  NVIDIA's user-space library (libnvidia-glsi, under its EGL and its
  Vulkan driver) crashed a program that was merely creating a Vulkan
  instance while exiting; the Vulkan loader loads every driver on the
  computer to list devices (thirteen libraries here, NVIDIA's waking
  its card each time). A crash "at exit" in a test with all tests
  passed: `coredumpctl info PID` and look at every thread.
- **Writing "on" to a device's `power/control` wakes it.** It is the
  kernel's `pm_runtime_forbid`: it resumes the device then and there
  (a sleeping one is woken, one in the middle of suspending is waited
  for without limit). There is no way from user space to keep a
  device from suspending without that (`pm_qos_resume_latency_us`
  set to "n/a" forbids the next suspend without a resume, where a
  device has the file: PCI cards mostly don't). Reading
  `runtime_status` first proves nothing: it can change before the
  write. Found by Astra's review of a hold I had committed
  (2026-11-20), reproduced in a guest with a USB hub.
- **Look for what Olo already does before saying what it doesn't**
  (the same day): I told Ben that Olo's programs woke the NVIDIA
  card by loading its Vulkan driver at every start, and built on
  it; Olo had kept that driver out since 2026-11-04 (`asleep_aside`,
  the interface kit's), and it failed only in that afternoon's
  state. `grep -rn` the names of the thing (the loader's variables,
  the driver's name) across `crates/` and `apps/` before proposing
  a mechanism, and say "not traced" until it is.
- **A second review before anything that touches a driver, a kernel
  interface or memory is relied on** (Ben, 2026-11-20: "very important
  so that there are no mistakes"; the reviewer was Astra then, Gemini 4
  Neon now): `tools/dev/agy-help` with a brief that names the commits,
  asks for the kernel's source behind each claim, and asks plainly
  whether each is safe to run. The first such review found a hold that
  woke what it was to protect, a finding that said "stuck" at one
  glance, three claims in commit messages that weren't shown, and a
  spin in a root service that had been there for days. Nothing
  reviewed so is pushed before its findings are dealt with.
- **An agent's question shows as `asking`** in `agents list`, and
  `watch` returns for it at once: `agents approve NAME y|p|no` for
  leave to run something, `agents answer NAME N` for another question's
  Nth choice, `agents keys NAME down enter` where the choices have no
  numbers.
- **A new crate for an agent needs its Cargo.toml first** (`agents
  start` refuses a folder that isn't a crate): a stub that builds,
  then start it. Its dependencies change Cargo.lock in the shared
  tree, which the push hook counts as dirty: push before it starts,
  or once its work is in.
- **A kit program loads one Vulkan driver** (the GPU's the computer
  started on, handed to the loader: HACKING, "One Vulkan driver,
  handed to the loader"). To have the loader choose as it would by
  itself (to tell a fault of this from a driver's): any of the
  loader's own variables (`VK_LOADER_DRIVERS_SELECT='*'`; that is how
  a test session gets lavapipe). `OLO_VULKAN_SOFTWARE=1` draws one
  program with Mesa's software driver. Which it did is in the
  program's log with `RUST_LOG=info` ("Vulkan: … alone, handed to the
  loader directly"), and in what it has mapped (`grep vulkan_
  /proc/PID/maps`: one library, not a dozen).
- **An "open" that fails still happened**: Mesa's Intel driver opens
  every DRM render node to see whose it is. Looking at a program's
  open files afterwards doesn't show it; `strace -e openat` on a
  made-up node does (Astra's review found it so). Before claiming a
  program never touches a device, trace the opens.
- **Measuring a program's start**: /var/tmp/claude-blueferry/vk/starts.py
  was the way (two builds in turn in one `TSESSION_GPU=1` session:
  time to the window in app_server's list, memory from
  `smaps_rollup`, processor time); worth a tool of its own under
  tools/dev the next time it's needed.
- **Every `sudo` a tool runs is asked of the person's Terminal**, and
  until auto-approval starts nothing first (TODO) each one runs the
  face camera for a few seconds: one `sudo sh -c '…'` or one script
  run as root, not a `sudo` a line.

### Control groups, a guest's tools, and asking SELinux

- **What SELinux lets a domain do can be asked without installing
  anything**: the system's Python has setools
  (`/usr/bin/python3 -c "import setools; p = setools.SELinuxPolicy();
  …TERuleQuery(p, ruletype=['allow'], source='xdm_t',
  target='cgroup_t', tclass=['dir'])"`). The login's worker is
  `xdm_t`: it may make and remove cgroup directories, write and chown
  their files, and use the capabilities the session's division and
  the hand-over need (asked 2026-11-20).
- **systemd marks what it delegates**: `user.delegate=1` on the
  cgroup's directory (readable by anyone; `trusted.delegate` too, for
  root). A login session's scope has none unless a drop-in says
  `Delegate=`.
- **A cgroup's file takes a write as the whole value; a test's plain
  file doesn't**: open with truncate (as a shell's `>` does), or a
  short value written over a long one leaves its tail ("max" over
  eleven digits read back as neither).
- **`tools/dev/kvm-guest` has bash and a short list of programs**
  (`ALWAYS`): no `sh`, `tr`, `stat` or `rmdir` unless named
  (`guest(…, bins=("stat", "rmdir"))`); and `kernel-check`'s `guest()`
  reads the last field of a row as JSON, so a bare number comes back a
  number.
- **A guest's kernel answers what a made-up tree of files can't**:
  who may move which process where, what a write to
  `cgroup.subtree_control` refuses, what survives a change of user.
  `kernel-check session-groups` is the pattern: the product's own
  functions run as root by `kbench`, then the same as an ordinary
  user, each answer compared with what it should be.

### A process that holds a capability can't be looked into

Where the person's compositor and `olo-session` hold `CAP_SYS_NICE`
(the owner's choice: HACKING, "The desktop first on the GPU"), a
same-UID reader can normally read status, cmdline, cgroup, stat and
limits. In a guest, it could enumerate a dumpable holder's numeric fd
entries. Reading their targets, fdinfo, the executable link, environ,
maps, smaps_rollup and io was denied. The reported wchan was 0.
A same-UID reader could still open the crash JSON written by the
capable process's own panic and signal handlers.

### Files swapped for a measurement: save in one step, restore from it (2026-11-24)

Measuring a change against the last commit means putting the
committed files in place for a build and the changed ones back after.
A command written `build && cp CHANGED SAVED && git show HEAD:F > F &&
build; cp SAVED CHANGED` ran its restore after the `;` though the chain
had stopped at its first step (the build's path came back empty), so
it put back the copies saved by the run before: an hour's changes to
two files were gone, and the next measurement was of the older code
(the numbers said so: a function no longer inlined). Save the changed
files in a command of their own, check the copies are there, and only
then swap and restore from them; a `diff -q` against the saved copies
before going on shows the tree is as it was.

### A crate's whole tests before its commit, not the one you wrote

Doctor's two fixture tests (a well set up machine, everything wrong)
list every finding and every fine line by name; a new check changes
both. I ran only the new check's own test (`cargo test -p olo-doctor
a_session_is_divided`) and committed: both lists were wrong on the
main line for three commits (2026-11-20; neither hook runs a crate's
tests). `cargo test -p CRATE` with no filter before committing to it.
And a lint that asks the computer something (whether the session is
divided) comes out differently the day the computer changes: what a
test's windows show mustn't depend on where it runs.

### What a review signed off is what gets built

The background group's design was reviewed (2026-11-20) and the
answer was to build it inside the login session, made by Olo's
login. I built it in the person's systemd manager instead, because
that needed no root and no new login, and the review of the build
found what the first had said, and more. A shorter road than the
design reviewed is a new design: it gets its own review before it is
built, or it isn't built. And `tools/dev/ui-lint` has no `--help`:
an argument it doesn't know starts the lint (read its first lines).

### Typing on the Rex rig (2026-11-24)

The rig's `Rig.type` types unshifted characters only (`CODES`): an
uppercase letter is a KeyError, and `>` typed nothing a command could
use (an `echo … > FILE` wrote no file, and the test looked like a key
bug). Write a test's command without Shift (`touch a b c` in a folder of
its own, then look at what it made), and press a key with modifiers
through `r.ts("key", r.name, "56", "105")` (Alt and ←, evdev codes,
pressed together).

### The bridge's Lua on a host, and what it can and can't see (2026-11-24)

The host side of Rex's bridge is a shell script and a Lua script run by
`rex do` (`apps/rex/src/bridge_olo.sh`, `bridge_olo.lua`). No `luac` is
installed here; Rex's own Lua checks a script without running it:
`rex do -e 'local f, e = loadfile("PATH"); return { loaded = f ~= nil,
error = tostring(e) }'`. `rex.exec({argv…})` returns `{code, ok,
stdout, stderr}` and runs on the host, in the server's folder, not the
caller's: make a path absolute from `PWD` first. A Lua string in single
quotes takes `\n` as a newline. What a host sends with a request is
data, never this computer's to name: a host's `rex do olo.op` can carry
any arguments, so the desktop side (`bridge::OpCarry`) is the boundary,
not the shell script. And the script reads standard input only where
the command takes it: an agent's shell may leave a pipe open, and
`head` on it waits for ever.

### The agents service and its tests: what the Rex server gives, and what a test brings with it (2026-11-26)

Learned moving `olo agent` from the app to the host
(`crates/rex-link/src/agentd`; HACKING, Rex, "The children of a tab are
their host's"):
- **A test run from an agent's shell carries that agent's marks to
  everything it starts.** Run from Claude Code, the environment has
  `CLAUDE_CODE_CHILD_SESSION`, `CLAUDECODE`, `AI_AGENT` and ten more; a
  scratch Rex server inherits them, every tab it makes has them, and a
  real Claude Code started in such a tab **saves no transcript** (its
  screen says "Transcript saving is off — inherited
  CLAUDE_CODE_CHILD_SESSION marker"): no conversation id, no last
  message, three live tests failing for a reason that is nothing of the
  code's. The rigs take out every variable beginning `CLAUDE` or `CODEX`
  and `AI_AGENT` (`tests/agents.rs`, `scratch_env`); a suite that passed
  when Antigravity ran it had simply been run from another agent.
- **What a terminal shows is the server's to say, cheaply where it is
  the alternate screen.** `com.superlogical.terminal.format` gives the
  screen *and* its history, as the server's own engine reads it. The
  agents draw on the alternate screen, which has none: 2 to 14 kB (ask
  `modes` for `screen`). With `max_bytes` set and more to give, the
  answer has an `id` and the server **keeps the whole text about five
  minutes** (`blob_read`): never ask that on every look at a main screen
  with megabytes of history (a terminal keeps 64 MiB). `rex capture
  --help` says the limits; `rex api describe METHOD` each method's
  arguments and what it fails with.
- **`session.attach_all` is not `session.attach`.** With `attach_all`
  (scope `session`) a connection gets every session's window and block
  events and may call methods on all of them, but `data.connect` and a
  block's own events (`block_event`: `program_status_changed`,
  `process_changed`) need `session.attach` to that session.
- **A new tab takes `size`, `cwd`, `path_prepend` and `exit` in its
  shell's options, and no environment** (`rex api call
  block_creator.list` lists a flavor's options): a variable is set in
  front of the program (`env K=V PROGRAM…`), and unset the same way.
  Left out, `size` is 80 by 24 until a client that owns the block
  resizes it.
- **A scratch server under bwrap has process numbers of its own**
  (`crates/rex/tests/scratch`, `--unshare-pid`): what `terminal.process`
  says there is not a number `/proc` here knows. A test that reads a
  child's processes (whose child an asker is, an agent's transcript by
  its pid) starts its scratch server plainly, with a home and runtime
  folder of its own (`Rig::serve`).
- **In a script, a command put in the background reads nothing**
  (bash gives it `/dev/null` where job control is off): the stand-in's
  recorder (`cat >> keys &`) recorded no key until it was given the
  terminal by name (`exec 3<&0; cat <&3 … &`). And **`wait` with no
  argument waits for every background job**, the scratch server too: a
  measuring script that started its server with `&` and then waited for
  thirty starts never came back. Wait for the numbers you started
  (`pids+=($!)`, `wait "${pids[@]}"`).
- **A build of mine became a host's live agents service for 2 min 21 s
  (2026-11-26, 10:15:25 to 10:17:46 UTC), under four of another lead's
  agents hours into their work.** Five things met; each is closed, and
  each is a thing to look for again:
  1. A picture's script pressed Escape (`tsession key NAME 1`) on Rex's
     "Stop 2 subagents?" alert to put it away. Escape took an alert's
     first button whatever it said, and that alert's first is the
     stop. **Escape now takes Cancel wherever it stands** (`escape_of`).
     Put a dialog away in a script by its Cancel, and know what a key
     does before a script presses it.
  2. Rex's **demo** has a "This computer" that is the computer it runs
     on: the stop went to the real host's helper. The demo now runs no
     helper at all (`activity_watch::may_ask_hosts`).
  3. The service handed over to "a later build", and any newer
     `olo-rex-link` file counted: the tree's debug build did. **Only a
     helper put beside the running one** takes over now.
  4. A test session has the account's runtime folder and a state
     folder of its own (`XDG_STATE_HOME`), and the service found its
     socket by the one and its registry by the other: it came up on the
     live socket over an empty folder. **State is found from the home**
     now, and another home has another socket (`wire::Paths`).
  5. Nothing in a test session kept Rex from the account's own Rex
     server and service. **Every `tsession` now has `REX_SERVER_SOCKET`
     at a socket nothing listens on and `OLO_REX_AGENTS_DIR` in its own
     folder**; a test that needs a server gives its own to `run`.
  Put right by retiring the wrong service and asking once through the
  installed helper from the real environment (`~/.local/share/olo/
  rex-link/olo-rex-link-ID-ARCH agent retire`, then `… agent list`):
  the registry on disk was untouched, because the wrong service kept
  its own. `olo agent ping` and `readlink /proc/PID/exe` say which
  build serves; `tr '\0' '\n' < /proc/PID/environ` what it was started
  with. **On a host where others' agents run: nothing built from the
  tree speaks to the live socket; a replacement of the service is said
  in their file before it happens; and `~/.local/state/olo/
  host-upgrade.hold` (anyone's, any words) holds it.**
- **A node keeps "the pointer is here" when another element takes it
  over** (the phantom tip, 2026-11-26). The tree matches elements to
  nodes by place where they have no key of their own, so a list's row
  for an item that went is given the next item's element, `hovered` and
  all, and the first row's `on_hover` never hears `false` (only a node
  destroyed under the pointer did: `left_behind`). What is tied to what
  a row is about, not where it stands (a tip, a card, a preview), must
  say so: `hover_key`. To see it: the pointer resting, then the list
  changed by something other than the pointer (`tsession touch` moves
  the pointer for hover too; Rex's `OLO_REX_DEMO_CHILD_ENDS` doesn't).
- **Rex's word for a pane isn't the agent's turn.** The Rex server says
  a pane is working while a shell the agent left in the background
  still runs (Claude Code with a dev server: "1 shell" in its footer,
  no "esc to interrupt"), so anything that takes that word for "the
  model is in a turn" misses the turn's end. The agent's own screen is
  what says it (at its prompt, nothing saying a turn is on). Reproduced
  with a child of one's own in a minute: `olo agent start claude …
  --prompt 'run sleep 900 in the background, then answer and end your
  turn'`, then `status --json` (ShiftUp's item 24).
- **The helpers are Rex's own subagents now** (2026-11-26; Ben: "when
  you use subagents for review/qa please use the new olo/rex native
  subagent support you built"). `tools/dev/agents` starts each as a child
  tab of the lead's tab with `olo agent start` (agy and the Claude CLI as
  the agents they are, so Rex says Working or Waiting of them; a tmux
  session only with no Rex pane, or `OLO_AGENTS_VIA=tmux`), and reads,
  types into and stops it with `olo agent read`, `send` and `stop
  --save`. What was learnt doing it:
  - The host answers `olo agent` by itself once the desktop has
    connected with a build that has it (it puts the helper and the `olo`
    script here): `~/.local/share/olo/rex-link/olo-rex-link-* agent
    --protocol` prints 1. Before that the tool falls back to tmux by
    itself (`rex_here`).
  - Who asks is the process that asks: the tool run from the lead's
    shell, or from the minder (a subagent of the same Claude process),
    is the lead's tab, and the children are under it. A helper can't
    start or steer others (`OLO_SUBAGENT`, and the service keeps a child
    to its own descendants).
  - A child is known by the id `start` answers with (kept in its meta as
    `rex_id`): a name is free again once its child has ended, and a look
    on the same folder is started under the same name.
  - A child's screen is the asking pane's size (tmux gave 200 × 60), and
    `read --scrollback --lines` gives 5000 lines at most: the whole of
    what it said is kept by `stop --save` (transcript.txt), not by
    reading.
  - Its first message is short and names the brief's file: a page of
    rules isn't sent as a command's argument through a tab's command
    line.
  - A choice's digit can't be pressed in a child tab: `olo agent send`
    pastes its text where the program takes pastes (a pasted "1"
    answers nothing), refuses text while a child asks leave, and takes
    no bare character for a `--key`. The tool answers as the service
    does, by what the choice means (`olo agent approve --once |
    --session | --no`), and moves the mark down for a choice of no such
    kind (`choose`). The first approval a child tab asked for went
    unanswered until this was so: a minder said yes three times to
    nobody.
- **An agent's shell has a proxy on its own loopback, and Podman hands
  it to every container** (`HTTPS_PROXY=http://127.0.0.1:PORT` in a
  Claude Code session here; Podman's `--http-proxy` is on by default).
  Inside a container 127.0.0.1 is the container, so anything that
  fetches there fails: the release's check on plain Fedora ("dnf …
  Could not connect to server … via 127.0.0.1"), which threw dnf's
  words away and ended `tools/publish-release.sh` at step 5 with no
  "failed:" line (2026-11-26; its wrapper's "exit 0" was mine, an
  `echo` after it: read the script's own last line, never the task's
  status). The script now passes no loopback proxy on
  (`--http-proxy=false`) and says why a check couldn't run. The network
  itself is reachable without the proxy (`curl --noproxy '*'`). For any
  container of your own that needs the network: `--http-proxy=false`.
- **Reading each agent's real question costs a minute, and beats a
  remembered one**: started in a new folder under `/var/tmp` in a tmux
  server of its own (`tmux -L NAME new-session -d … claude`, the
  shell's `CLAUDE*`, `CODEX*` and `AI_AGENT` unset, then `capture-pane
  -p`), Claude Code 2.1.332 drew its folder trust with the mark on
  "No, exit" and the yes under it, unnumbered; Antigravity with the
  mark on "Yes, I trust this folder"; Codex 0.172 asked nothing in a
  folder that is no repository. Killed with `tmux -L NAME kill-server`,
  nothing answered, so no agent's settings are touched.
- **A test session's app runs in the person's own home** (`tsession`
  without `--home`: only what apps keep between runs is the session's).
  Rex, started there for a picture, repointed `~/.local/bin/olo` at the
  session's own folder under `/var/tmp/olo-dev` (`bridge::
  ensure_local_bin_dir` took any Olo link for its own to replace), on a
  host where thirty agents were using `olo`; stopping the session would
  have left the link hanging. Seen only because `ls -la` was asked of
  the link for another reason. The app links there only from a folder
  inside the home now. Before a headless app is run on a machine other
  agents work on, ask what it writes outside the session's folder
  (`strace -f -e trace=symlinkat,renameat2,openat -o …` on a first run,
  or read its start-up for `HOME`).
- **What is told must already be written.** An event reached a watcher
  a few milliseconds before its line reached the log's file; the kill
  test (`kill -9` of the service right after a watch returned) failed
  once in five runs with a cursor that had gone backwards. A test that
  passes four times in five is a bug: run a new suite several times
  before believing it (`for i in 1 2 3 4 5; do jato test …; done`).

### Testing Olo Rex and Rex rev-19 servers

- **Rex rev-19 `data_ref_id` is `"terminal"`, not `"output"`.** Both
  `session.bootstrap`'s `data_tickets` request and `data.connect` must
  pass `data_ref_id: "terminal"` with subprotocol `rex.data.terminal.v1`;
  passing `"output"` connects silently with zero bytes delivered.
- **Scratch Rex server flags:** start an isolated scratch Rex server with
  `XDG_DATA_HOME=… XDG_STATE_HOME=… XDG_CONFIG_HOME=… rex server run --listen unix:///var/tmp/…/rex.sock`
  (the flag is `--listen`, not `--socket` or `--data-dir`), and point
  `olo-rex` / `olo-rex-glance` at it with `REX_SERVER_SOCKET=/var/tmp/…/rex.sock`.
  For offline UI/tiling QA without touching any Rex server, pass `OLO_REX_DEMO=1`.
- **A person's command key in a test session:** `tools/dev/tsession
  key NAME 125 47` presses Super with V through the session's own
  app_server and its keymap, so a terminal app (Terminal, Rex) gets
  what it gets on the person's screen, Ctrl+Shift+V, and any other app
  Ctrl+V; `29 47` (Ctrl with V) is not the same test. The command
  key's Cut, Copy and Paste did nothing in any text field of Rex's or
  Terminal's while every test typed text or pressed Ctrl (2026-11-23).
  A field's text is read back from the app's tree (`{"tree": true}` on
  its `olo-menus-PID.sock`: role `TextInput`, its `value`).
- **Rex's Server window in the demo:** `OLO_REX_DEMO=1
  OLO_REX_DEMO_SERVER=STAGE` (or `@FILE` naming the stage, read each
  time the window opens: one run pictures them all) and
  `{"invoke": "host.server:dev-box"}` on the app's `olo-menus-PID.sock`
  open it on a made-up host (`server::demo`'s stages); nothing is
  connected to, whatever is pressed. Add Host's is
  `OLO_REX_DEMO_SETUP` with `app.add_host`.
- **What a host says of its Rex server, as root** (asked of a real
  one, 2026-11-23): `rex version` and the package spell one build two
  ways ("0.1.0-snapshot.2506", dpkg's "0.1.0~alpha.2506"; apt's
  candidate is in the package's spelling). An account's open sessions:
  `runuser -u ACCOUNT -- rex --autostart=false ls -q` (one id a line;
  it fails, with a message on stderr, where no server runs). The
  shared server listens at `/run/rex/server.sock`, which is not where
  the `rex` account's own server would be: ask it with `-S
  /run/rex/server.sock` (a path or `unix://…` both do, as `rex` or as
  root). Each running server is a process `rex server run …`, the
  shared one with `--listen PATH`. apt reads one source again with
  `apt-get update -o Dir::Etc::sourcelist=FILE -o
  Dir::Etc::sourceparts=- -o APT::Get::List-Cleanup=0` (Rex's is
  `/etc/apt/sources.list.d/rex.sources`; 0.6 s). The journal's lines
  (`-o short-iso`) are `2026-11-23T01:06:20+00:00 HOST rex[PID]:
  2026/11/23 01:06:20 INFO  words key=value`: Rex writes its own time
  after the journal's.
- **The Rex server takes two requests at once.** Two requests that
  arrive together on one connection may be done in either order:
  forty one-letter writes sent without waiting for each answer came
  back "acbdefgjhilknm…" (2026-11-23, a real link, Nagle's algorithm
  putting two in a packet). Anything whose order matters (what is
  typed, a focus then a write) is sent one at a time, each answered
  first. To time typing over a real link:
  `HOME=SCRATCH OLO_REX_TEST_HOST=ssh://USER@HOST cargo test -p
  olo-rex-client --test host typing -- --ignored --nocapture`
  (`OLO_REX_TEST_TRIPS=120` samples the round trip longer,
  `OLO_REX_TEST_BUSY=1` has another terminal there print meanwhile);
  it ends the sessions it makes.
- **Try it against the real thing in every mode it has.** The Server
  window passed every test on a made-up host; a real one found a comma
  left in the accounts file and, run as a shared server, a count of
  sessions asked at the wrong socket. A host made up from what the
  code expects agrees with the code.
- **Serialize per-host terminal writes FIFO:** never spawn one tokio task
  per keystroke for `terminal.write` / `client.key` — rapid typing races
  and transposes characters. Route writes through a per-host unbounded
  FIFO channel (`HostRuntime::input_tx`).
- **Kit `MenuField::width(w)` is total width (`label_w + popup_w`),** not
  just the popup button width — pass `.width(div_x + POPUP_W)`.
- **The terminal engine's source is not in the repository, and its
  header's guides aren't in the header**: for what a flag means, read
  the source at the pinned commit (`tools/build-ghostty-vt.sh` names
  it; `https://raw.githubusercontent.com/ghostty-org/ghostty/COMMIT/
  src/terminal/render.zig`, `c/render.zig`, `Terminal.zig`,
  `page.zig`). Learned there (2026-11-22): the render state's
  `update` does a full redraw whenever the viewport's pin changed
  (every line printed at the foot of a full screen), rebuilding every
  row and marking every one dirty; the copy of the packed row it
  keeps has its dirty bit cleared (`RowBuilder.row`), so under a full
  redraw nothing says which rows the terminal touched; a row's id is
  its page's serial and its index there, never reused;
  `ROW_DATA_CELLS_RAW` lends a row's packed cells in one call.
- **A packed cell names its style by an id, and ids are reused**:
  the page's style set counts references, and once the last cell
  with a style is erased its id is free for the next style. "The same
  words, so the same row" kept a red X where a green one had been
  written (Astra's review, 2026-11-22; I had reasoned "an id a cell
  still uses can't change", which is true and beside the point: the
  cell was rewritten between two looks). Anything kept beside a
  packed value (styles, graphemes, hyperlinks) needs its own check.
- **One of a kind speaks for the rest only where all show the same
  parts**: checking "the first cell with this style id" was held by
  the second review, because a cell that is only a background in its
  word shows that background and not its style's, while a letter with
  the same id shows the style's. Ask of a representative what it
  hides.
- **Read a manifest as what it is**: I searched the engine's JSON for
  `"style_id":{` and then for `"lsb":`, and a field with no `lsb`
  took the next field's. `serde_json`, the path, the schema's number,
  and none when anything isn't as expected (there is a fallback: use
  it).
- **Ask the engine two ways and compare**: the terminal's own text of
  a row (`viewport_row_text`) against what the render state gives
  (`extract_rows`). They disagreed after a combining mark written on
  its own, which is how the engine's missing "row is dirty" was found
  (TODO, "A mark joined later"): my test's expectation failed in the
  frame that takes every row too, so it wasn't my change.
- **A generated test may never make the case you wrote it for**: the
  6,000 generated writes passed with each guard taken out, because
  "a second mark on a letter that already has one, with a capture
  between" needs three particular writes in a row. Name the cases the
  guards are for, assert in each that it did what it means to (the
  grapheme is now three codepoints; the colour is the new one), and
  only then let the generator look for what you didn't think of.
- **`vendor/ghostty-vt` static library:** rebuilt via
  `tools/build-ghostty-vt.sh` (Zig 0.15+, `ghostty` commit
  `6d244209978e599277904ff3ced7b8a2ebcc528d`, MIT), tracked in Git LFS.

## Rex over a slow link, tried here (2026-11-22)

- **The rig** is `tools/dev/rex-slow`: `rig.py` starts a scratch Rex
  server, `slowlink.py` in front of its socket (a delay each way,
  jitter, stalls), and the Rex app headless in a `tsession` on the far
  side; `try.py typed|stall|connect` are the tries made of it, and
  `tui.py` stands in for the programs people type into (`box`: as
  Claude Code draws, the cursor hidden and a caret in reverse video,
  the whole of it redrawn in a synchronized update; `line`: as agy and
  Codex draw, the terminal's own cursor in a composer). Its files are
  under `REX_SLOW_DIR` (`/var/tmp/olo-rex-slow`). To freeze the link,
  stop the proxy it started (`os.killpg(r.procs[1].pid, SIGSTOP)`,
  then `SIGCONT`): its pid, never a name.
- **Rigs run at once**: each with its own `REX_SLOW_DIR` and
  `REX_SLOW_NAME` (its session) never meets another, and
  `tools/dev/rex-slow/at_once.py SCRIPT JOB… [-- ARG…]` runs a script
  for several jobs so, each job's output after it. A picture script
  written as one job per look and scale (`script.py win-dark-1.75`)
  takes all its pictures in the time of one: fourteen in 13 s, seven
  minutes one after another (2026-11-24, on the dev server).
- **A test session's apps read their settings from the session's own
  folder**, `/var/tmp/olo-dev/NAME/config/olo/…`, not from the `--home`
  given: a `rex.json` written under the home was never read, and two
  runs meant to differ by a setting were the same run.
- **A script's children must not hold its output.** A server started
  with `Popen(..., start_new_session=True)` and the script's own
  stdout keeps a pipe the script is read through open after the script
  has died: the command "hangs" on nothing. Give them `DEVNULL`, and
  write their pids where a cleanup can find them.
- **The dev build checks the order locks are taken in** (tracing-mutex:
  "Found cycle in mutex dependency graph", a panic with the newest
  lock's backtrace). It only sees the paths a run takes: Rex took the
  app's lock inside the hub's state lock in one arm that runs only
  while a host is connecting, which a slow link makes long; found the
  first time the rig ran.
- **A typed letter is "drawn" where app_server says so**: its `timings`
  list `felt` (`to_draw_us` of the last key), which the rig's
  `Rig.felt()` reads. With nothing drawn ahead that is the echo's
  frame; with it, the key's own.
- **A flood's instruction count has a spread of a twentieth** between
  runs of the same build (the frames a run happens to paint): one run
  each "showed" a regression of 4% that five runs each did not.
- **How the agents' programs draw their caret** (what typing ahead has
  to find): agy shows the terminal's cursor in its composer, with a dim
  hint right of it where nothing is typed, between two rules (read off
  a running helper with tmux's own `cursor_flag`, its text not looked
  at); Claude Code, as it runs here (a design review's own session,
  read the same way), shows the terminal's cursor too, in a composer
  between two rules, on the alternate screen, with no cell in reverse
  video; older ones, and programs built on Ink, hide the cursor and
  draw a cell in reverse video (its issue reports say so), which is
  why both are found; Codex is believed to show the cursor (its TUI
  sets it). `tools/dev/rex-slow/caret.py SOCKET PANE` does the
  reading: tmux's own word of the cursor (`cursor_flag`, its place,
  `alternate_on`) and the pane's cells by class (blank, text, reverse
  video, on a background), never its words.
- **The host's `rex` command needs a folder it can read to stand in**:
  as another account from root's home it says "getting current working
  directory: stat .: permission denied"; `cd` to the account's home
  first.
- **A Rex server puts no word of what was typed into a terminal's
  output**, and says the terminal's input mode (`echo`, `canonical`)
  only when asked: there is no event when a program stops echoing. A
  session's shell on a fresh Ubuntu host is a login bash with bracketed
  paste on and no prompt marks (OSC 133).
- **Connecting costs seven round trips** between the window opening
  and its terminal showing (`try.py connect`: 2.1, 4.3 and 8.3 s at
  150, 300 and 600 ms each way).
- **An app's instructions in a test session: count its own threads,
  not the rasteriser's.** A headless session has no GPU, so the app
  draws with Mesa's llvmpipe, in the app's own process (threads named
  `llvmpipe-0`, `llvmpipe-1`): nine tenths of the instructions `perf
  stat -p PID` counts while a terminal is typed into, and wandering
  between 1.09 and 1.56 G for the same 200 letters, run to run, in the
  same build (where the app's own threads spent 184.1 to 186.3 M every
  time). Three runs each "showed" the host helper costing a quarter
  more, and it was this. `perf stat --no-scale --per-thread -x, -e
  instructions:u -p PID`, and leave out the lines whose thread begins
  `llvmpipe` (in a profile, `perf report --sort comm` shows the same
  split; this computer's two kinds of core each give a table of their
  own, to be added).
- **The helper for hosts is built by `apps/rex`'s build script**, for
  two musl targets, with `rust-lld`: no C compiler for another machine
  is here or needed (no `musl-gcc`, no `zig`), and the targets come
  with the toolchain file. A first build of each takes about nine
  seconds, into `target/rex-link`, whatever the profile.
- **The test host's `sh` is dash**, this computer's is bash: what a
  host is to run is tried here first (`ssh::dial_tests`,
  `helper::tests`), and against the host before it is believed
  (`tests/host.rs` with `OLO_REX_TEST_HELPER_FILE`).
- **A far host that is this computer, for what Rex only does over
  SSH** (`tools/dev/rex-slow/farhost.py`, `Rig(far=True)`): an sshd of
  the rig's own on 127.0.0.1 (run as you, keys and an agent of its
  own under `REX_SLOW_DIR/far`, a scratch HOME by `SetEnv`), saved in
  the app as "far". The helper put on a host, what a host is asked,
  the status bar's marks: all with nothing of the person's read or
  shown, so its pictures can go to a reviewer. Three things it
  taught: the host's own `rex` finds, or *starts*, a server of its
  own unless the address names the scratch server's socket
  (`ssh://you@127.0.0.1:PORT/…/rex.sock`; `REX_SERVER_SOCKET` is the
  Olo app's variable, not `rex`'s; a server so started is to be
  stopped with `rex -S unix://… server stop`); its HOME must be the
  scratch server's sessions' HOME, or what a script typed into a pane
  installs isn't what a command over SSH finds; and the app's window
  is to be waited for by its title, since the host-key question is a
  window too.
- **Root through sudo is tried on the far host; root's login isn't**
  (2026-11-24): its sshd runs as this account, which may use sudo
  without a password on the dev server, so `provision::RootBy::Sudo`
  runs there for real (read-only commands only: `sudo -n true`, `id
  -u`, the probe, `pwd`), and it can never log root in. The host test
  (`crates/rex/tests/host.rs`, all ignored) is built with your own HOME
  (`jato test -p olo-rex-client --test host --no-run`, which prints the
  binary) and run under a scratch HOME whose `.ssh/known_hosts` has the
  far host's key (`[127.0.0.1]:PORT` and `REX_SLOW_DIR/far/host_key.pub`),
  `SSH_AUTH_SOCK` its agent, `OLO_REX_TEST_HOST=ssh://USER@127.0.0.1:PORT`
  and `OLO_REX_TEST_SUDO=1`: cargo under a scratch HOME looks for its
  registry and toolchain there. The test of root's login fails at its
  root step there, as it must.
- **A Fedora 44 host of the rig's own** (2026-11-25): a container
  (`tools/dev/rex-slow/fedora-host/Containerfile`, built with `sudo
  docker build -f Containerfile -t olo-rig-fedora44 .` in that folder;
  `farhost.py`'s `FedoraHost`, or `farhost.py fedora` to keep one for a
  run by hand), systemd its first process, `fedora` with password-less
  sudo, no polkit, as a cloud image has it. It must have what a real
  host has for a login to get its runtime folder: `systemd-pam`
  (pam_systemd, a weak dependency of systemd's), sshd through PAM, and
  dbus-broker (without, `loginctl` had no bus and user@.service failed,
  "Trying to run as user instance, but $XDG_RUNTIME_DIR is not set").
  The setup's host test runs against it as the sudo one does (below):
  `/var/tmp`'s script starts the container, copies its `known_hosts`
  into a scratch HOME, runs the test binary with `OLO_REX_TEST_HOST`,
  `SSH_AUTH_SOCK` and `OLO_REX_TEST_SETUP=login|personal`, and the
  container is removed with it. It reaches the network (Rex's installer,
  dnf). Never the person's host for a test that changes it.
- **Testing your own change while a subagent edits a crate yours
  builds on** (2026-11-25: the engine agent mid-edit in `crates/rex-sync`,
  the Rex client depending on it, its build broken for an hour): a
  worktree at HEAD with only your changed files copied in (`git worktree
  add --detach /var/tmp/YOURS HEAD`, then each file of `git status` but
  the agent's crate; the lock file's lines for the agent's new
  dependencies taken out), the suites run there with `--locked`; commit
  in the main tree only your paths (`git add` each), which is what the
  worktree tested. jato's cache makes its first build quick.
- **Counting instructions on the dev server needs root** (its
  `perf_event_paranoid` is 4): `sudo -n perf stat --no-scale -e
  instructions:u -p PID -- sleep 30` around a process of yours, or `--
  sudo -u $USER BIN` for one it starts: a binary it starts itself runs
  as root, and what it writes is root's (a bench's tree in /var/tmp,
  2026-11-25: removed with sudo after).
- **The whole account move, for real** (2026-11-25):
  `tools/dev/rex-slow/move.py` makes an Ubuntu and a Fedora container on a
  network of their own, sets each up for Rex with the host test's real
  setup (personal), gives the source stand-in agents in a session with a
  work list, and runs `crates/rex/tests/moving.rs` (the move as the app
  drives it) with one agent holding both hosts' keys and a scratch HOME
  that knows both; about five minutes, most of it the two setups. It
  needs the helper built with `move` in `target/debug`. A connection to a
  host's Rex server goes through Olo's helper there
  (`SshSessionPool::set_helper`), and reading a session needs the
  connection attached to it first (`session_attach`). Its stand-ins
  answer `--version` (the move checks each agent starts there; without
  it each waited 30 s on the SSH command's open input), and the stand-in
  Claude Code keeps `~/.claude/sessions/PID.json` as the real one does.
  `OLO_REX_TEST_MOVE_EACH=1048576` sends its small home in parts all the
  same (the parallel copy).
- **Which session a running Claude Code is in** (2026-11-25): it keeps
  `~/.claude/sessions/PID.json` for each process of it (`pid`,
  `sessionId`, `cwd`, `procStart`: the kernel's start ticks, field 22 of
  `/proc/PID/stat`, so a record left by a pid used again is told
  apart; the current session after a `/resume` or a compaction too).
  Codex has no such record: its windows' turns run in its daemon
  (`codex app-server`, pid file JSON), so no window holds a rollout open;
  the daemon's control socket lists the open conversations with their
  folders (`thread/loaded/list`, `thread/read`: crates/pool's
  `codex_daemon` and the helper's `panes.rs` ask it).
- **A subagent's engine work in a worktree of its own** (2026-11-25):
  a worker changing a crate that nearly everything compiles (rex-sync)
  would break the lead's builds mid-edit; so it worked in `git worktree
  add -b NAME /var/tmp/claude-olo/wt-X HEAD`, its brief naming the
  worktree's crate as its files and every command `cd`'d there, the
  minder told to allow those; its diff applied to the main tree with
  `git apply` once read whole, the worktree and branch removed after.
  `agy models` names Neon's efforts (`gemini-4-neon-xhigh`).
- **The dev server's `sleep`, `cat` and the rest are one program**
  (uutils' coreutils, multi-call): a copy of `/bin/sleep` under another
  name refuses to run ("coreutils: unknown program"). For a test
  process with an odd name, a `bash` that writes its own
  `/proc/self/comm` and waits (`printf NAME > /proc/self/comm; sleep 30
  & wait`). Its `/bin/sh` is dash (no `exec -a`).
- **`docker cp` into a rig container's `/tmp` writes nowhere**: it is a
  tmpfs the container mounts (`--tmpfs /tmp`); copy through `/opt` and
  install as the account (`docker exec -u USER`), or `~/.local` is
  root's and the account can't write under it.
- **An Ubuntu 26.04 host beside it, and hosts that reach each other**
  (2026-11-25, for moving an account): `ubuntu-host/Containerfile`
  (`olo-rig-ubuntu2604`, built the same way; `ubuntu`, polkit, git,
  rsync, as Ubuntu's server image), `farhost.py`'s `UbuntuHost`; both are
  `ContainerHost`s, and given `network=` (a `rig_network()`, a docker
  network of the rig's own, removed with its `with`) they reach each
  other by their `name` on port 22, while this computer reaches each at
  its `address` (a local port: never what one host should use for the
  other). Stand-in agents for a host (`stand-ins/agent.sh`, linked as
  `claude`, `codex` and `agy` in its `~/.local/bin`): each keeps a session
  where the real one keeps its own, takes a resume among its own flags
  and writes its arguments into its session's first line.
- **The rig can put the helper behind its slow link** (`Rig(helper=
  True)`, `try.py order|timed --helper`): the link runs it for the
  control connection, as sshd would, with the scratch server's own
  `dial-stdio` behind it.
- **A scratch script named as one of Python's own modules is run by
  whatever imports that module beside it**: a `typing.py` among the
  test host's scripts was run, whole, when a new script there imported
  PIL (which imports `typing`): it made and ended a session on the
  host before it failed. Name scratch scripts for what they are of
  (`rex_typing.py`).
- **A picture of Rex against a real host shows that host's sessions**,
  whoever's they are: the sidebar lists them all. Before one goes to a
  reviewer, crop to what the review is of (the status bar, a tip), and
  give the host a name of its own in the test's settings
  (`saved_hosts`, `label`), so no address is drawn.
- **A backslash before a quote is lost in a Python heredoc's
  triple-quoted string** (`\"` comes out as `"`): a test's JSON
  written so (`"a\"b"`) reached the file as `"a"b"`, and the test
  that failed was blamed on the parser for a while. Write such text to
  the file with a quoted shell heredoc, or double the backslash.

- **Boxes in a row are each laid on whole pixels by themselves**: a
  row of 13-point boxes 6 points apart at 1.5× (19.5 and 9 pixels) had
  gaps of eight pixels and seven, and a mark that reckoned the row's
  end from its own box's edge was a pixel or two off it (half a pixel
  either way, then rounded). What must be evenly set by the pixel is
  drawn by the row, in one canvas, from one edge of its own (the kit's
  `widgets::indicators`); measure gaps and both insets at 1×, 1.25×,
  1.5×, 1.75× and 2× from a picture at each (thirty lines of Python:
  the commonest colour beside the marks is the fill, and runs of
  other columns are the marks).
- **A no-break space joins words for the kit's line wrapper** (a
  menu's path in a tip: `"Agents\u{a0}▸\u{a0}Install…"`), and is as
  wide as a space. A wrapped text's lines can be shorter at a narrower
  width than at the width it was first wrapped at (the break moves a
  word back): fit to the longest line, wrap again, until it is no
  narrower (`tip_size`).
- **Two reviewers who pull apart, or one who asks for the opposite of
  what it asked two rounds before, are shown candidates, not another
  build**: sketches of each drawing (the one built, the one asked for
  drawn exactly, the one the other passed) side by side at true size
  and enlarged, in both looks, with "CHOICE: A/B/C" asked for on the
  second line and the earlier rounds' asks listed in the brief. Seven
  rounds on a 15-by-11 bridge ended in the one look that did this;
  the reviewer's own latest prescription came last in both rankings.
- **A hover left by one picture is in the next**: move the pointer off
  what has a tip (and wait for it to go) before a picture that isn't
  of the tip.
- **A word made longer is a layout change**: picture every window
  the word is in before the commit, not only the one the work is
  about. "Manage Server…" renamed "Manage Rex Server…" made four
  buttons, each as wide as the widest label, wider than Hosts and
  Ports, and it was pushed so (mended in the next commit). The static
  lint passes a flex row that shrinks its children, and the live lint
  saw only the windows Rex shows at its start; it opens Hosts and
  Ports now (`OLO_REX_DEMO_OPEN`). `grep -rn "the old words"` over
  `apps/` says where a label is used and what is sized from it.
- **A picture of Rex's Hosts and Ports holds the account's name** (the
  far host's address, `USER@…`): paint the cell over before it goes to
  a reviewer (the rig's script finds the label's node by its text and
  fills it with the ground beside it), or use the demo
  (`OLO_REX_DEMO=1 OLO_REX_DEMO_HOSTS=states`, made-up hosts).
- **`&app().hub.clone()` in a call's arguments holds Rex's app lock
  for the whole call**: what the call reaches must not ask for
  `app()` again (it isn't re-entrant: the window stops for good, and
  nothing says why; a click that "did nothing" and every command after
  it going nowhere is this). Take the hub in a `let` of its own, or
  keep what is needed where the window's state is (the Host window's
  title: `host::Shown::name`).
- **The rig keeps Rex's window where a run left it**
  (`state/olo/windows`): a run on a smaller screen left it too small
  for its sidebar, and every run after "never showed the far host".
  The rig forgets window places now; if a rig's start fails on what a
  window shows, look at the window's size first.
- **Rex's demo is where pictures with a host's address come from**
  (`OLO_REX_DEMO=1 OLO_REX_DEMO_HOSTS=states OLO_REX_DEMO_OPEN=
  app.hosts,host.manage:dev-box`, in a test session with a scratch
  HOME and `REX_SERVER_SOCKET`): made-up hosts in every state, no
  account's name. What needs a real host to change (an install, a
  failure) is pictured on the rig's far host with the address painted
  over; check a sheet of every picture's head before any goes out (a
  label split in two had stopped the painting, and the first sheet
  showed it).
- **A thing "being done" is pictured by making the host wait**: a
  named pipe where the install's first file goes (`mkfifo`) holds the
  host's script at its `cat >` until the test reads the pipe.
- **A dev build says a lock asked for twice**: Olo's mutexes
  (`olo_sync`) panic in dev builds with "Found cycle in mutex
  dependency graph", and the app's crash report (in the test's
  `XDG_STATE_HOME/olo/crashes`) names the thread. A test whose window
  "does nothing" after a click: count the crash reports there before
  reading the code (the rig's cost scripts print the count; two were
  from the Host window's title, minutes of reading saved had they
  been looked at first).
- **A change to the played sessions' model makes them all new
  sessions** (one more number drawn, and every session after it
  differs): when Rex's predictor learnt to ask the host about its
  prompt, the 4,800 sessions found seven holes in rules that had
  passed the old 4,800. Take each as a real bug in what was there
  (most were), fix it where it is, and expect several rounds; a
  failure names `seed`, which is `n % 2400`, so play both `n` and
  `n + 2400` (`PLAY_ONLY=N PLAY_SAY=1`), and a temporary `eprintln!`
  of the candidates in `found()` under `PLAY_SAY` says in one run what
  an hour of reading the trace doesn't.
- **A test that never ends**: `cargo test` with no `timeout` sat ten
  minutes and was moved to the background when a fuzz's loop had no
  end (an event rescheduled itself for ever). Run a test that plays a
  loop under `timeout 400`, always.
- **A stand-in for a server answers as you think the server does.**
  Rex's helper passed all its tests against its stand-in while
  looking for a block event's name under `event`; the real server
  writes `name` (the app's parser takes either, which is how the
  guess looked right). Found on the rig in one run, by writing what
  the helper saw of the real server to a file for that run. Whatever
  is read from a server's own messages is tested against the real
  server (`crates/rex-link/tests/server.rs`, on the scratch server of
  `crates/rex/tests/scratch`), and a new field or event is read from
  a real message first, not from the client's model of it.
- **A model in the played sessions, to keep the old sessions as they
  were**: draw its chance from a number generator of its own (the
  helper's in `predict/played.rs` is `Lcg(seed ^ …)`), and the 4,800
  sessions are the same ones with the new thing added, not all new.
- **Instructions per message, in a small program, are its parses and
  what it makes each time**: the helper's cost for a letter doubled
  when two of its parts each read the message (16,600 to 32,900
  instructions), and `memchr::memmem::find` makes its searcher anew at
  every call, which for messages this short cost more than the search
  (5,000 of a letter's 21,800). Read once and hand the result on; make
  searchers once (`Finder`, in a `LazyLock`).
  `tools/dev/rex-slow/helper-cost.py` counts it: the helper run
  against its tests' stand-in, `perf stat -p` on it while 1,000
  letters pass one at a time, 50 lines are ended, and at rest.
- **The rig's keys** (`tools/dev/rex-slow/rig.py`, `CODES`) are the
  ones somebody needed: a key it hasn't is a `KeyError` at `type`.
  Add it there (digits came with `sleep 2`).
- **A picture between two timed keys takes as long as the wait being
  measured** (a `tsession shot` is 0.4 to 0.6 s): time the keys from
  one clock and compare the picture after with one taken later, when
  everything has settled.
- **What a WebSocket client sends doesn't pack**: every frame is under
  a mask of chance. SSH's compression made what a far host sends a
  third to a tenth of its bytes and left what Rex sends as it was,
  until its frames went under a mask of nothing
  (`olo_rex_client::plain`). Count both ways before believing a
  packer works (`tools/dev/rex-slow/try.py bytes`).
- **russh's compression names are swapped** (`ZLIB_LEGACY` is
  `zlib@openssh.com`, the modern one), and its default order puts
  `none` first.
- **The rig's far host opens its connection at once** whatever
  `far_delay` is (the link delays what passes, not the connecting), so
  what Rex decides from how long a connection took to open (packing)
  is set there by `OLO_REX_SSH_COMPRESSION`.
- **The rig answers the far host's key question by waiting for its
  button**, not by looking once after a moment: on a busy computer
  the window was there before its button, the question stayed, and
  the run failed a minute later saying the host's session never
  showed. A failure of the rig that names something far from its
  cause: look at the picture it took first (`shots/far-missing.png`
  showed the question still open).
- **Count round trips on the wire, not in the code**: the rig's link
  notes every chunk that passes with its time (`REX_SLOW_TRACE=FILE`,
  `tools/dev/rex-slow/slowlink.py`), and with a delay of 300 ms each
  way the round trips of anything are the steps of 600 ms in that
  file. Connecting was "seven round trips" by reading the code and
  nine by the trace. For SSH (encrypted) the sizes and times still
  show who waits for whom: a 68-byte packet up, a 36-byte one down
  and only then the 236-byte upgrade was how OpenSSH's window of
  nothing was found, after a guess about Nagle's algorithm had been
  written into the code and shown wrong by the same measurement.
- **A test that makes something on the real host ends it whatever
  happens**: `connecting_is_timed` panicked before its last line twice
  and left two sessions on the test host. What is made there is ended
  from outside the part that can fail (a task of its own, waited for,
  then the ending), and a test clears its own leftovers (by its own
  label) when it starts.
- **A Rex server gives tickets, and attaches, only for the session it
  picks** unless one is named (`preferred_session_id`): a test with
  one session on a scratch server passes, and fails on a host with
  the person's sessions on it.
- **A link with a delay isn't a narrow link.** The rig's link only
  delayed, and nothing behind a flood ever waited; with a rate
  (`REX_SLOW_RATE`, `FarHost(rate=…)`) a small request waited more
  than 40 s behind SSH's 2 MiB window. Anything about floods, files
  or several things at once is tried on a narrow link.
- **A window knob that "changed nothing" was measuring something
  else**: the stale output after Ctrl-C was the same with a 64 kB SSH
  window as with 2 MiB, because the Rex server queues all of a
  terminal's output for a slow stream; what the window changes is how
  long *other* things wait. Measure the thing a change is for
  (a request beside the flood), not the nearest visible symptom.
- **russh's `adjust_window` hook**: called at each top-up of a
  channel's window (when half has come); the published 0.65.1 client
  ignores what it returns (`vendor/russh` fixes that). Top-ups come in
  bursts (many packets in one read), so time between two says nothing
  of the link: count bytes in `Handler::data` over a stretch.
- **`rex api list` and `rex api describe METHOD`** (against a scratch
  server: the rig's `r.rex("api", …)`) say what the server's API
  takes and gives, with examples and notes: read them before guessing
  what a request can do (a terminal's present screen by itself: it
  can't).
- **A key the rig types can repeat** on a busy computer (its release
  comes late, and the session repeats the key): `seq 500000` became a
  number with many more noughts and a flood without end. Probably
  also the one run of twelve that passed twice the bytes while sixty
  keys were typed. What a measurement runs is written into the
  session's own program where it can be (`try.py behind`), not typed.
- **A rig pane's `rex` reaches its own server only by `REX_SERVER`**:
  the rig's far server runs on a socket of its own
  (`$REX_SERVER_SOCKET` in its panes is the rig's name, which `rex`
  doesn't read), so `rex do` there finds the account's default server,
  starts an empty one, and says "block target default not found: no
  sessions". On a real host the session's server is the default. A
  test that runs `rex` (or `olo`) in a far pane sets
  `REX_SERVER="unix://$REX_SERVER_SOCKET"` first.
- **The Rex server takes one stream of a terminal from a client at a
  time**: a second one's hello is refused ("AlreadyConnected") until
  the first has ended, and its end takes a moment to reach the server.
- **The pre-commit hook refuses a file staged in part** ("formatted, but
  these have unstaged changes too"): to commit some of a file's hunks,
  stage them (`git apply --cached` of their diff), set the rest aside
  with `git stash push --keep-index`, commit, `git stash pop`.
- **A new field in the desktop's settings needs the test session's
  app_server built again** (`jato build -p app_server`): an older one
  drops the field when it is set (`tsession ipc … set_settings`), and
  the app under test never hears of it.
- **The rig's app can never ask the person's Passwords**: it always
  runs with `OLO_KEYCHAIN_SOCKET` at a path of the rig's own; a test
  that wants an answer puts a stand-in there (`r.keychain()`). And the
  rig notes what it starts (`REX_SLOW_DIR/started`), so a run that is
  killed is cleaned up by the next (`farhost.swept`): a run cut off by
  its timeout had left agents, links and a test session running.
- **`tools/dev/ui-lint --help` is a run of the whole lint**, not help
  (its options are in the file's head).
- **"Failed to open channel (ConnectFailed)" from an OpenSSH server is
  its `MaxSessions`** (ten sessions a connection by default), not a
  network failure: `sshd` answers a session it won't open with that
  reason. The rig's far host takes `FAR_MAX_SESSIONS=N` to try it.
- **A scratch machine to change as root**: a container with systemd
  and an SSH server (`podman --root /var/tmp/… --runroot /var/tmp/…
  build --network host` from Fedora's image with `systemd
  openssh-server` installed, then `run -d --systemd=always`). Its
  storage goes under your own scratch folder, not the person's
  `~/.local/share/containers`. A build needs `--network host` here
  (the default network reaches no mirror), and a rootless container
  can't raise its limits or write `vm.*`: what that breaks (a sign-in
  refused by PAM's limits) is worth finding there first.
- **A static program for Rex hosts with C in it** (the pool's:
  aws-lc, zstd) is built with zig as the C compiler: a wrapper for
  each target (`zig cc -target ARCH-linux-musl -fno-sanitize=undefined`,
  dropping cc's own `--target=`, which zig doesn't take as Rust names
  it; `zig ar`), `CC_<target>` and `AR_<target>` set to them,
  `-Clinker=rust-lld`, and `CARGO_RESOLVER_FEATURE_UNIFICATION=selected`
  (this workspace unifies features across all its packages: without
  it the host's build takes every package's). Without
  `-fno-sanitize=undefined` the link fails on `__ubsan_handle_*`; C
  objects built before the flag are kept under
  `target/TRIPLE/release/build/NAME/` (not `NAME-HASH`) and must be
  removed. musl's `libc::sched_param` has more fields than glibc's:
  make one with `zeroed()`, not a literal.
- **A scratch host with a systemd user session over SSH**: the same
  container (above) run with `-p 127.0.0.1:PORT:22`, an account made
  in it with a scratch key in its `authorized_keys`, and `rm -f
  /run/nologin`: an SSH sign-in there gets `XDG_RUNTIME_DIR` and a
  working `systemctl --user`. It can't reach this computer's loopback
  (a test's stand-in provider here isn't reachable from it), and has
  no polkit (`loginctl enable-linger` is refused).
- **The far host of the rig is this computer**: anything put there
  that listens on a fixed port meets the person's own (their Token
  Pool on 17891 and 17892). The rig's SSH server sets ports of the
  rig's own for a pool there (`OLO_POOL_PORT`, `OLO_POOL_PROXY_PORT`)
  and a provider that isn't there (`FAR_POOL_UPSTREAM`).
- **UDP isn't TCP on the way to the test host**: a QUIC connection's
  round trip there was 225 ms on five connections of eight and 166 on
  three, with TCP's at 164 to 183 every time (2026-11-23). A latency
  compared across the two must say which path each run got (the
  link's own `round_trip()`), or it measures the network's choice.
- **Tests against a host name themselves "Olo Rex test"**
  (`ClientIdentity::a_test`, since 2026-11-23): as "Olo Rex", an app,
  each left a client the server listed as lost for a while, and `rex
  do` there couldn't choose between apps (the person's agent met it).
- **`tsession theme` takes `light` or `dark`**, nothing else: "night"
  is the night light warming the whole screen (app_server), not a look
  the apps draw, so there is no night picture of a window to take (a
  picture asked for as "night" came out the dark one, and both
  reviewers said so).
- **Picture 1.25× and 1.75× whenever a list or anything sized in rows
  changes**: both of Rex's lists had live scroll bars only there (a
  height added up by hand, a fraction of a device pixel short), and
  1×, 1.5× and 2× showed nothing.
- **A new field in the desktop's settings needs the desktop server
  rebuilt for a test** (`cargo build -p app_server`): the session's
  app_server reads and writes the settings whole, and one built before
  the field drops it without a word (a repository added in Rex's form
  was gone a moment later: the settings came back without it).
- **Give `tsession run` no `XDG_CONFIG_HOME` of your own when the
  program is Terminal** (or anything that leaves a daemon): a
  session's processes were recognised by its display and its config
  folder, and a Terminal run with another folder left its olo-termd
  and its shells alive past `stop`, at the next session's display: the
  next Terminal there reattached to them (what I typed went into the
  last run's program). `stop` goes by the session's name too now; and
  after a Terminal test, `ls $XDG_RUNTIME_DIR | grep olo-termd` shows
  the person's display alone.
- **A coding agent's screens are tried with a stand-in**
  (`tools/dev/stand-ins/agy RECORD`, run as `bash …/stand-ins/agy`: its
  name is what tells it apart), never a real agent in a test session.
  What the real one does with a key is tried in one of the helpers'
  own tmux sessions (`tmux -L olo-agents send-keys -t =NAME:0.0
  Enter`), on a specimen brief that touches nothing.
- **Auto Approve asked for through the commands socket asks the
  person** (an alert, "Turn On"): a test presses it, as a person.
- **After anything run against the person's test server, look at
  what it left**: `rex ls` there showed eight sessions named
  `olo-connect-test` in the person's sidebar (2026-11-23; the
  client's `connecting_is_timed` and runs of it that were cut short).
  End them by that name, and only them.
- **A host's limits are looked at through a new connection**: PAM
  sets them once for an SSH connection, and every session on it has
  those; `ssh HOST 'ulimit -Hn'` from a shell is a new sign-in, a
  pooled connection's exec is not.
- **"Permission denied (publickey)" on a host with the keys given**:
  `ssh -v` there says how many keys the agent returned and which were
  offered; `ssh-add -l` through `~/.local/state/olo-rex/agent.sock`
  counts them (it asks nothing of the person). Count before theories:
  the forwarding was whole, and Passwords offered one key of three.
- **Never `pidof`/`pgrep` one of the person's programs to look at it**
  (I did, once, to read where two of them ran from: it told nothing
  the logs hadn't).
- **podman carries this session's proxy settings into a container**
  (`HTTPS_PROXY`, set here for the Token Pool's proxy on 127.0.0.1,
  which inside a container is the container): nothing in it reaches
  the network, and `dnf -q` says nothing of it. `podman run
  --http-proxy=false`.
- **The rig's far host is this computer**: its sessions had the
  person's own `PATH`, so their agents were "installed" there before
  any test put one (farhost.py keeps the person's folders off it
  now). What a test looks for by name on the "host" may be yours.
  And a command its SSH server runs starts in the account's home as
  the system keeps it (`/home/…`, this computer's real one), while
  `$HOME` is the rig's scratch home: anything run there that goes by
  its working directory, not `$HOME` (`cd` first, as the app's scripts
  do), writes into the real home (2026-11-24: a sessions copy into
  `.` put a test's transcripts in the real `~/.claude/projects`, found
  and taken away). The helper takes a relative folder from `$HOME` now.
  Run what the rig tests with `RUST_LOG=…,olo_rex=info` in its `env`
  to see the app's own lines: `tsession run` gives programs `warn`.
- **A command run with `SshSessionPool::exec` went on after its
  caller had gone**, until 2026-11-23: russh closes a channel only
  when told to. `exec` closes it now when dropped; a script that must
  end with its caller is wrapped in `ready::cancellable`.
- **Rex's Lua** (`rex do SCRIPT`): `io` and `os.getenv`/`os.exit` are
  there, `os.execute` isn't (`rex.exec{ "sh", "-c", … }` returns
  stdout, stderr, code, ok); `rex.call(method, table)` returns the
  answer or nil and why; a long loop over a big string (a base64
  decoder over 800 kB) dies with "registry overflow"; a Lua sequence
  is a JSON array, an empty table an object.
- **Olo's Chrome on an Agent Screen gives no page tree for `file://`
  pages** (the extension isn't let into them): serve a test page from
  loopback. A change to `extension/tree.js` can be tried without
  installing anything: a page that fetches the script and runs it on
  itself (`apps/chrome/tests/tree-text.html`).
- **The dev server has no Google Chrome, but `tree.js`'s test pages run
  in any headless Chromium** (2026-11-24, a browser-testing tool's
  build in `~/.cache/ms-playwright/chromium-*/`, where there is one):
  serve `apps/chrome` from loopback and `chrome --headless=new
  --no-sandbox --user-data-dir=/var/tmp/YOURS --virtual-time-budget=5000
  --dump-dom URL` prints the page with its PASS title. `--dump-dom`
  doesn't wait for fetches begun after load without the budget; to time
  the script, load it with a synchronous request and call it in the
  page (in the top frame it runs synchronously).
  The extension and its host can be tried the same way: copy
  `extension/` to a scratch folder and `--load-extension` it (its id is
  the folder's path's sha256, first 32 hex digits as a to p), write
  `PROFILE/NativeMessagingHosts/com.olodesktop.passwords.json` naming
  `target/debug/olo-chrome` for that id, and start it with a scratch
  `XDG_RUNTIME_DIR` and `HOME`: the host answers
  `RUN/olo-menus-PID.sock` (`{"tree":true}`, `{"invoke":…}`) for the
  browser process (not the pid you started). Headless takes one address
  at start: open more with `--remote-debugging-port` on 127.0.0.1 and
  `curl -X PUT 'http://127.0.0.1:PORT/json/new?URL'`.

- **`olo-screen ACTION NAME --help` acts.** Only `olo-screen --help`
  prints help: `olo-screen type agent --help` typed "--help" into the
  focused window of that screen (a terminal on the person's host, at
  its prompt; cleared with Ctrl+U before anything ran). Read
  `olo-screen --help` for an action's arguments, never ask the action.
- **A Rex moved to an agent screen forwards that screen's keys.** It
  reconnects from the screen, and its keychain is the screen's
  Passwords (what the person shared with the screen), so a host's
  forwarded agent offers none of the person's own keys until Rex is
  back on their display. (Seen 2026-11-23: 0 keys on the host after
  the move, 2 before.)
- **Most Linux hosts give every login a live, empty `ssh-agent`**
  (`SSH_AUTH_SOCK=/run/user/UID/openssh_agent`, the openssh user
  service): "has the shell an agent?" isn't "has it keys?"; ask
  `ssh-add -l`.

- **Codex started on a scratch home leaves its server running.** Its
  first start (the TUI, or `codex app-server`) starts a daemon from
  `$CODEX_HOME/packages/app-server-daemon/`, which goes on after the
  program you ran ends, and writes into the folder: removed under it,
  the folder came back (2026-11-23). Before removing a scratch
  `CODEX_HOME`, end what runs on it: the processes of yours whose
  environment has `CODEX_HOME=` that folder (found by that, never by
  name), or `CODEX_HOME=… codex app-server daemon stop`.

### The dev server (2026-11-24)

Work moved from the person's Olo computer to a dev server they reach
through Rex (Ben: "you are now running on my remote dev server with
rex"). How to set one up and work on it is the `olo-dev-server` skill
(`.claude/skills/olo-dev-server`), kept current with this section. What
this one is, and what differs:

- **A second one since** (seen 2026-11-26): Fedora 44, 64 processors,
  251 GB, btrfs under `/home` and `/var/tmp` (so `cp --reflink=always`
  works there and a copy of a build is free), no GPU either, `perf`
  under sudo the same. What is said of Ubuntu and ext4 below is the
  first one's; check which you are on (`/etc/os-release`, `stat -f -c
  %T /var/tmp`) before trusting either.
- **The README's and the site's pictures are taken here too**
  (`tools/dev/readme-shots`): an Agent Screen draws in software where
  there is no render node (the tool sets what `tsession` sets), 3840 ×
  2160, a whole desktop with Deskbar and Tracker's. From a release's
  own build, no second workspace build: `OLO_SHOTS_BUILD=DIR` (a folder
  of `/var/tmp/olo-release/target/release`'s programs, reflinked, with
  dev builds of what you changed laid over), `OLO_SHOTS_OUT=DIR` to
  look before anything in `docs/img` is replaced. The tool gives the
  screen this tree's data (`OLO_DATA`: a build made elsewhere finds no
  icons, wallpaper or fonts beside it, and its desktop is bare), a name
  of its own for the desktop's disk (`OLO_TRACKER_DEMO_VOLUME`: the
  disk's name is the computer's, which is nobody's to publish), and no
  Rex server or agents service but its own. The desktop scene took
  about a minute, all eighteen scenes (31 pictures) about twenty. Three
  things a server lacks that a picture shows the want of: an emoji
  font (every emoji a box: `OLO_SHOTS_EMOJI_FONT`, the file out of
  `dnf download google-noto-color-emoji-fonts` unpacked with `rpm2cpio
  | cpio -idm`, nothing installed); a person (Deskbar on an agent's
  screen shows no live widgets, by design: `OLO_DESKBAR_DEMO_PERSON=1`,
  which the tool sets); accounts (Integrations' Google accounts are a
  made-up file taken for signed in, `OLO_PREFS_DEMO_GOOGLE=signed`).
  `place(w, x, y)` with no size gives a window the size it had when it
  opened, not the one it has: pass its frame's (`tuck`).
- **Ubuntu 26.04 in a VM** (the first: 32 processors, 125 GB): `setup.sh` is
  Fedora's (dnf), so what Olo needs came from apt: Mesa's software
  Vulkan (`mesa-vulkan-drivers`, lavapipe), Python's imaging
  (`python3-pil`), Olo's fonts (`fonts-urw-base35`, `fonts-noto-core`,
  `-mono`, `-cjk`, `-extra`, `fonts-liberation`,
  `fonts-jetbrains-mono`). `sudo` needs no password; `gh` and `zig`
  are installed.
- **No GPU.** The one display device is QEMU's (`bochs-drm`); NVIDIA's
  driver packages are installed with no card for them. Test desktops
  draw in software anyway (HACKING, "Test desktops draw in software"),
  and work here: `tsession` and its pictures, the lints.
- **`/tmp` is memory here too**: scratch in `/var/tmp`. It is ext4,
  so `cp --reflink=always` fails ("Operation not supported"): run a
  build where it is (`TSESSION_SESSION_EXE` takes any path; the
  pre-push hook's release build of the last pushed commit is at
  `/var/tmp/olo-prepush/tree/target/release`, a ready "before") or copy
  it plainly, and remove the copy after.
- **Docker / rootless Podman in `tools/publish-release.sh`**: on Ubuntu
  where Docker needs `sudo` (the account isn't in its group),
  `tools/publish-release.sh` says `sudo docker` by itself where it must.
  On Fedora 44 where `docker` is rootless Podman (`podman-docker`),
  `--user "$(id -u):$(id -g)"` without `--userns=keep-id` maps host
  files to `root:root` inside the container and fails with `EACCES` on
  `.cargo-build-lock`; `tools/publish-release.sh` now detects rootless
  Podman and passes `--userns=keep-id` automatically. Releases are built
  in the Fedora 44 container, never on the host.
- **The person's Olo desktop** is reached from here through Rex (the
  `olo-rex-bridge` skill): a link opened there, an Agent Screen of
  one's own there, a secret from their Passwords once they allow it.
- **Builds go through Jato** (CLAUDE.md); the hooks use it where it is
  installed (`tools/hooks/install` switched them on in this clone).
- **`perf` counts only under sudo here**: `kernel.perf_event_paranoid`
  is 4 (Ubuntu's), so `perf stat` as the account counts nothing and says
  so; `sudo -n perf stat --no-scale -e instructions:u -p PID -- sleep 10`
  counts (the VM has the counters). The setting is left as it is.
- **Use its 32 processors** (Ben, 2026-11-24: "try to reduce the dev
  loop time as much as possible and take advantage of the hardware"):
  headless sessions are cheap here, so pictures are taken one session
  per look and scale, all at once (`at_once.py` for the Rex rig; any
  `tsession` likewise, each its own name), and their crops and sheets
  made in a process pool; `try.py agents` runs its three agents at
  once (25 s, from about 90).
- **Codex isn't signed in here**, so Astra's looks can't run until the
  person signs in (`codex login --device-auth`); `astra-look` says so at
  once (it started at Codex's sign-in screen and waited there). Never
  sign in for them.
- **Claude Code asks once per computer whether to trust a folder**: the
  Opus look's own folder (`/var/tmp/olo-agents/opus-review`) was asked
  about the first time here, and the look waited. `agents watch`
  answers Yes by itself for an agent's own folder now.
- **Rex's server names a script's program by its interpreter**: a bash
  script is "bash", with no script path (`argv0` "/bin/bash"), so a
  stand-in agent run as a script is never told apart there; run it
  under a copy of bash named for its agent (`try.py agents` does).
  Real agents run under their own names (Claude Code's own build sits
  under `…/claude/versions/`, its name its version).
- **Terminal never counts a tab's own shell as an agent**, and `bash -c`
  has no job control (what it runs stays in its process group, the
  foreground the shell's): to try an agent's stand-in in Terminal, run
  it from an interactive shell (`olo-terminal -e bash -i -c "STAND-IN
  RECORD; exec bash"`), not as `-e STAND-IN`.

- **Docker's containers are network changes to whatever watches the
  network**: each one started or stopped is some thirty rtnetlink
  messages (a veth pair, the bridge, IPv6 addresses), all of them news
  to `olo_net`'s watcher on this computer. A release built in
  containers (`publish-release.sh`) while the pre-push hook's rest
  check ran was when Calendar's demo widget provider was seen waking
  2.2 times a second (it ran the watcher then; it doesn't now). To
  measure anything that watches the network, start containers beside
  it: `sudo -n docker run --rm registry.fedoraproject.org/fedora:44
  true`, five in a row (`crates/net/examples/watch.rs` is the watcher
  alone). The watcher's own test of real changes,
  `crates/net/tests/netns.rs`, says "not tested" here (Ubuntu's AppArmor
  refuses an unprivileged user namespace): `jato test -p olo-net --test
  netns --no-run` names its binary; copy it out of the home folder
  (`/var/tmp`) and `sudo -n env -i PATH=/usr/bin:/bin COPY --exact
  network_changes --nocapture` runs it, its cut and wait times printed.
  Run where it was built, it says "Permission denied" as root too: in
  its user namespace root has no rights to the home folder (`drwxr-x---`),
  so it can't start itself again from there.
- **The runtime folder gathers what test desktops leave** (`olo-glance/
  wayland-N/`, `olo-state/attention-wayland-N.json`): `tsession` forgets
  them as a desktop stops and before one starts (`tidy`), and
  `tsession tidy` does it by hand. A widget provider whose lock a
  provider of an ended desktop held never ran, and a check that waited
  for its widget's file found the old one.
- **What the dev server differs in, that tests assumed** (2026-11-24,
  `jato test --workspace --no-fail-fast`, which the pre-push hook doesn't
  run: run it after a change that could reach other crates): `/bin/sh`
  is dash (no line editor: a terminal reads it a canonical line, 4,096
  characters at most, and its `kill` takes no `--`); the umask is 0002
  (folders a test makes are group-writable, which grant and cgroup code
  rightly refuses: set the mode a test means); the filesystem is ext4
  (a folder lists in its hash order, not the order files were made);
  the account's shell is in `~/.local/bin` (out of a sandbox's binds);
  AppArmor refuses unprivileged user namespaces (bwrap has a profile
  for them); netem isn't loaded; the GPU is lavapipe, which crashes a
  process at exit after two Vulkan contexts were made and let go at
  once in it. Each of these broke a test that passed on Ben's computer.
  Ext4 has no reflinks either: `cp --reflink=always` fails ("Operation
  not supported") between `target/` and `/var/tmp`; copy plainly (a
  release test binary is about 100 MB). And `sudo` starts its command
  with a clean environment: a variable a measured program needs goes in
  through `env` inside it (`sudo -n perf stat -e instructions:u
  --no-scale env OLO_WRAP_COST=plain PROGRAM …`), or every case runs
  alike and the counts agree for the wrong reason.

**A package's name isn't always its program's** (2026-11-24): To-Do's
app is the package `olo-todo-app` (its program `olo-todo`); `jato build
-p olo-todo` builds the engine crate of that name, and a test desktop
then runs the program as it last was, which looked like a key handler
never called. Before trusting a picture of a change, check the
program's time (`ls -l target/debug/PROGRAM`) or build by its package
(`grep '^name' apps/X/Cargo.toml`).

**`tmux attach -r -f ignore-size` on a detached session still uses the
client PTY size** (2026-11-25): `ignore-size` excludes the client only
relative to *other* attached clients; when a tmux session has zero other
attached clients (such as every detached helper session under
`tools/dev/agents`), tmux resizes the window to the attaching client's
PTY minus `status` rows (e.g. a `97×31` PTY with `status on` shrinks a
`97×31` detached pane to `97×30`). To attach read-only without changing
a detached pane's dimensions by even one row, size the viewer PTY to
`#{window_width} × (#{window_height} + status_rows)` alongside `-r -f
ignore-size`.

**Modern `codex` CLI rejects `--full-auto` in interactive mode**
(2026-11-26): the installed `codex` binary exits with `unexpected
argument '--full-auto'`; pass `-a on-request -s workspace-write` (or
`--approve-for-me` / `--dangerously-bypass-approvals-and-sandbox`)
instead, and set reasoning effort via `-c model_reasoning_effort="<LEVEL>"`.
Rex's `olo agent start codex` maps `--full-auto` to `-a on-request -s
workspace-write` automatically unless `-a`/`-s` are specified explicitly.
