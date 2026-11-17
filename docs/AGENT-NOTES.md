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
- **No app binaries from the shell.** Even `--help` opens the app on
  Ben's screen (the shell has his `WAYLAND_DISPLAY`). Read the program's
  `main.rs` to find a flag that prints and exits (`olo-doctor --report`,
  `olo-logs --print`), and mind backticks inside a double-quoted grep:
  one ran a binary once.
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
- **The layout lint lays out a panel only as its test state shows
  it.** Preferences ▸ Bluetooth went out with its last button cut off
  (a phone chosen shows four buttons) because the lint only ever saw it
  with nothing chosen. A panel whose controls change with what's chosen
  starts, under `cfg!(test)`, at its fullest: the most buttons, the
  longest words (Bluetooth's chooses its test phone).
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
  crate's run.
- **The commit hook's layout lint runs over the working tree**, not
  what's staged: committing one thing while another's panel is half
  done fails on that panel (a new Preferences panel needs the settings
  index made again first: `OLO_WRITE_SETTINGS=1 cargo test -p
  olo-preferences --features olo-kit/lint ui_lint_settings_index`).
- **`tools/iconsheet.py` has no `--help`**: its first argument is the
  sheet to write (`iconsheet.py /var/tmp/ME/sheet.png NAME
  data/icons/src/candidates/NAME-*.png NEIGHBOUR…`). A theme's
  candidates are in `data/themes/ID/src/candidates/`; they look small
  beside shipped icons only until `mkicons.py --theme ID NAME` trims
  them.
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

## Working with the repository

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
