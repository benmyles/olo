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
