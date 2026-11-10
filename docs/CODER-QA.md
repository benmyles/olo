# Olo Coder validation — 2026-11-10

Release builds in the `olo-coder` worktree. All GUI work used isolated Agent
Screens or `tsession`; all model tasks used scratch workspaces and separate
service stores. Nothing was installed into the person's session. No development work was delegated to model agents; child lifecycle checks use
local provider fixtures.

Current provider support is documented in the final section below. Earlier Claude
checks are historical evidence; Claude support has since been removed from Coder.

## Functional checks

- `cargo build --release --workspace` passed.
- A complete `cargo test --release --workspace --no-fail-fast` run passed:
  204 suites, 1,634 tests passed, none failed, 138 ignored. The ignored live
  provider tests were then run separately as described below.
- After the final context-estimation and multiline alignment changes,
  `cargo test --release -p olo-coder-core -p olo-coder -p olo-ui` passed:
  32 core, 3 interface and 36 shared UI tests. A final workspace rerun passed
  1,358 tests across 138 suites before receiving SIGTERM during the unrelated
  VA-API decoder tests; no failure had been reported. The remaining targets
  were rerun using the workspace-built test executables: all 14 targets,
  179 tests passed. The workspace doctests (excluding GPUI, whose manifest
  disables doctests) then passed: 104 tests. This interrupted run is not
  reported as one complete command.
- Deterministic provider fixtures cover native Codex encrypted reasoning,
  Claude signed/redacted thinking, both compaction formats, fragmented UTF-8
  SSE, incomplete replies, usage/cache accounting, cancellation and size bounds.
- The long-session fixture completed 500 turns and 249 compactions, retained
  1,756 durable events and five active native items, then reopened the store
  and searched/replayed its history. This is an accelerated fixture, not a
  claim that a weeks-long live session has been observed.
- Effects: checked edits, deletion recovery, undo conflicts, symlink escapes,
  process-group cancellation, Landlock write confinement, manual tool approval,
  denied effects, spec revisions, budget stops, blocked keep-going goals,
  service upgrade draining, child configuration restrictions and FTS migration.
- Native Git fixtures cover staging, index locks, expected-head commits, CRLF,
  LFS clean storage, branches, worktrees and files inside untracked folders.
  GitHub and MCP HTTP fixtures cover scoped requests, negotiated protocols and
  bounded responses. A real approved GitHub repository read succeeded through
  Olo's credential integration; publishing was tested against a local fixture.

Two broader test invocations encountered existing feature-configuration limits:
selecting only the trailing packages omitted Smithay features supplied by the
full workspace, and explicitly forcing GPUI's disabled doctests required an
inspector macro unavailable in that configuration. The corresponding normal
workspace-built targets passed; no unrelated vendor code was changed.

## Real Token Pool

- Codex wrote a Rust GCD implementation, compiled it and ran three passing
  tests: nine turns, 23,294 input tokens and 644 output tokens in that run.
- Automatic approval reviewed and permitted a scoped workspace write.
- Codex returned encrypted reasoning, accepted its replay, compacted five
  native items into one encrypted compaction item, and recovered a remembered
  marker and arithmetic result afterward.
- Claude live requests returned HTTP 429 (`rate_limit_error`), including a
  later quota probe. Claude's protocol fixtures pass; live Claude reasoning
  and compaction remain unverified with this pool's available quota.

## Interfaces and recovery

120 visual cases: Work, Changes, Settings and five native forms, in light,
dark and night at 1×, 1.25×, 1.5×, 1.75× and 2×, changing scale while running.
Final result: **zero review findings**. The sweep found and fixed fractional
composer alignment and a one-pixel mixed-row form discrepancy at 1.75×.
`ui-lint --scales 1,1.5,2 --screen 2560x1600 coder` reported zero errors;
2560×1600 physical pixels at 2× exercises the minimum 1280×800-point screen.

Confirmed compositor reload restored the native session and message draft.
Confirmed binary re-exec restored a pull-request title and multiline description
in the same open form. TUI upgrade restored its attached session and draft.
Service upgrade tests allow already-pending approvals to finish while rejecting
new turns; clients reconnect by durable cursor. Terminal checks covered syntax
colors, Unicode wrapping, aligned tables, tab indentation and the link chooser,
including a visible selection marker when `NO_COLOR` is set.

Screenshots are in the [visual journal](journal/README.md). The TUI and GUI use
one command parser/API and one service; every native tool and session command
is callable from either composer or `--rpc`.

## Release performance

[Machine-readable results](coder-qa.json) include all benchmark runs, including
failures. “Before” is the earlier plain-text prototype developed in this task;
“after” adds the semantic rich document, syntax, native forms and multiline
composer. It is not a comparison against a previously released Coder.

An 18,392-byte, 80-section Markdown fixture was presented to both builds,
alternating before/after at 1×, 1.5× and 2×. `scroll-check` drove 90 Hz touch
reports, flicks and wheel events against a 120 Hz compositor. Across both builds:
**1,187 recorded frames, zero hitches**.

| Scale | Drag first move before / after | Wheel first move before / after | Slowest after scroll draw |
| --- | --- | --- | --- |
| 1× | 28.4 / 24.6 ms | 9.0 / 9.1 ms | 3.23 ms |
| 1.5× | 28.4 / 28.6 ms | 8.9 / 9.1 ms | 1.19 ms |
| 2× | 22.4 / 28.5 ms | 7.9 / 10.3 ms | 5.97 ms |

Three-second idle samples at every scale measured **0.0% CPU, zero wakeups and
zero frames**, before and after, with focus outside the composer. Separate
three-second samples of the TUI and service measured 0.0% CPU and zero wakeups.
A focused native text caret continues the kit's normal blink behavior.

`tools/dev/bench --apps coder --scales 1,1.5,2 --size 2560x1600` exercised launch,
hover, panes, menus, typing, resizing and theme changes at an 8.3 ms frame budget:

- Before: 337 frames; five failing steps, including compositor stalls and a
  13.88 ms resize frame. Typing medians: 1.94 / 1.67 / 1.19 ms.
- First after run: 336 frames; six failing steps, slowest non-transition frame
  20.57 ms on hover. Typing medians: 1.17 / 1.42 / 1.04 ms.
- Repeat after run: 336 frames; one failing step, a 9.59 ms tab frame. Typing
  medians: 0.96 / 0.76 / 0.74 ms; maxima: 1.06 / 0.82 / 0.95 ms.

The host was shared with other CPU/GPU work. These results do not establish a
hard worst-case latency bound or a completely passing 120 Hz sweep. The generic
benchmark's empty-session scrolling does not exercise document scrolling;
the separate long-transcript comparison above does.

## Explicit limits

The [user guide](CODER.md) documents the implementation's scope: native Git does
not yet provide fetch/push, branch switching, signing or worktree removal; MCP
supports stdio and Streamable HTTP tools but not interactive OAuth, sampling,
elicitation or resumable GET streams. Developer commands use cancellable argv
processes with captured streams, not interactive PTYs. History/artifacts have
bounded individual items but no automatic lifetime pruning. The process sandbox
confines writes; it does not isolate host reads or networking.

## Coordination and multi-session follow-up — 2026-11-10

This follow-up adds durable addressed messages, three steering boundaries,
searchable live session catalogs, pins/archive/tags, native tiled windows and
independent Ratatui tiles. [Detailed measurements](coder-coordination-qa.json)
retain the visual cases, benchmark runs and recovery evidence.

Validation:

- Final focused release suite: **84 passed**, zero failures (43 core, five app,
  36 UI tests); nine intentionally ignored live/doctest cases. Tests cover
  queued/after-tool/interrupt boundaries, skipped provider call results,
  Stop/Resume, inbox limits and edits, receipt idempotence, peer identity,
  offline delivery retry, cancellation of stalled peer requests, upgrade
  recovery and legacy schema-3 stores with opaque native context.
- A manual tool action cannot consume dormant collaborator messages or start
  inference because they exist. Both a regression fixture and a live local
  service check verified the pending inbox and model usage remained unchanged.
- An earlier full release workspace run passed 1,648 tests. A later full run
  passed 1,651 with one failure in unchanged `olo-net` code:
  `tests::a_nudge_wakes_a_waiter`. Rerunning that exact workspace-built test
  binary passed all nine tests. The final focused suite includes later fixes;
  the final `cargo build --release --workspace` also passed.
- **225 visual cases, zero findings:** six panels in 1040×700 and 530×357
  windows, plus Rename, Connect and Message forms, in light/dark/night at
  1×, 1.25×, 1.5×, 1.75× and 2×. Scale changed with the app running.
  Release layout lint at 1×/1.5×/2× reported zero errors. Four simultaneous
  530×357 windows fit beside Deskbar on a 1280×800 screen and also reviewed
  cleanly. Screenshots are in the [journal](journal/README.md).
- Four native windows restored their sessions, drafts and exact positions
  after binary re-exec. Two windows retained their drafts across two compositor
  reloads. Two TUI tiles retained their sessions, drafts, focus and delivery
  mode after re-exec; color and `NO_COLOR` were inspected. Initial startup
  with the service lock held retained the requested session and unsent draft
  through a client re-exec, then connected when the service became available.
- Cross-instance transport is currently owner-local Unix sockets with explicit
  links; remote-host transport is not implemented. Receipts describe admission,
  not task completion. Retries retain message IDs; collaborator messages never
  grant approvals. Waking an idle collaborator is opt-in at the recipient.

Release benchmark baseline is commit `da8583f0`, not another coding agent.
At 120 Hz and 1×/1.5×/2×, the baseline recorded 341 frames and one failing step
(slowest non-transition draw 9.67 ms, hover). The first follow-up recorded
335 frames and two failing steps (18.14 ms, resize); a repeat recorded 339 frames
and zero failing steps (4.83 ms, typing). Baseline typing medians were
1.14 / 1.00 / 1.23 ms; the repeat's were 0.87 / 1.01 / 1.42 ms. The panel selector
replaces three benchmark tab-click steps, so total steps differ (33 versus 30).
These shared-host samples do not establish a hard worst-case latency bound.
Three-second connected idle samples measured 0.0% CPU, zero wakeups and zero
frames before and after at 2×; the service and two-tile TUI also measured 0.0%
CPU and zero wakeups. The final build's separate idle sample was unchanged.

After the temporary reconnect-status change, the final build recorded 335 frames
and zero failing steps; its slowest non-transition draw was 6.68 ms (hover).
All four benchmark runs, including the initial failures, are retained in the
machine-readable report.

## Reliability audit follow-up — 2026-11-10

The next audit reproduced and fixed three failures with regression tests:
pre-cancelled process execution could still spawn (and sometimes report success),
MCP tool discovery did not observe cancellation until its 30-second timeout,
and the blocking event client treated premature EOF or a replay-gap notice as
successful completion. MCP discovery cancellation now measured 5–7 ms in the
fixture, and already-cancelled process work creates neither a process nor an
output artifact. Configured MCP tools can also be invoked manually through the
same approval/audit path, without starting inference or enumerating unrelated
servers.

Additional fixes cover parent/child restart intent, shutdown admission, queued
frontend prompts and script completion. Stop now clears descendant restart
requests while keeping the active reservation, retains completed children's
status, and closes shutdown's start/cancel race. The GUI and TUI send Stop on a
separate, short-lived worker: no extra thread or timer exists while idle.
Unsubmitted prompts carry their original session and cancellation generation;
a stop or attachment change restores them to the appropriate draft. `--exec`
waits for its own message's admission before accepting an idle event, reports
stops and lost streams as failures, and rejects malformed event cursors.

Live release GUI test: a peer deliberately withheld its reply for one second
while a local process was running. Stop completed after **1,025 ms before**,
versus **17 ms after**, with two subsequent runs at **21 ms and 11 ms**. The
cancelled process had no remaining in-flight action; the older unsent prompt
was restored to the composer rather than admitted into the session. Three-second
idle samples before/after measured **0% CPU, zero wakeups and zero frames**.
A separate CLI fixture verified a normal zero exit, nonzero exit on service
loss, interrupted-state recovery after restart, and invalid cursor rejection
before starting a service.

The scale sweep found a Send button outside the window when the compositor
forced it below its requested minimum width on a small display. Below 500 points,
delivery choices now occupy a separate row. **90 final visual cases passed with
zero findings**: all six panels, light/dark/night, scales 1/1.25/1.5/1.75/2,
changing scale while running, including a 454×369-point window. Cancellation,
retry, compaction and approval status now use readable, separated transcript
blocks; the original event data remains available through history/action details.

The final focused release suite passed **58 tests** (50 core, eight app), with
four opt-in live tests ignored. The release workspace build passed. A fresh live
Claude reasoning/compaction attempt still returned HTTP 429; live Claude
verification remains blocked by the pool's available quota. This is not a claim
of zero remaining defects or a weeks-long live soak.

The complete release workspace suite subsequently passed **1,661 tests across
204 suites, zero failures**, with 138 ignored cases. The same 120 Hz release
benchmark at 1×/1.5×/2× recorded 336 frames and zero failed steps before the
change (slowest draw 5.87 ms). The first after run, overlapping the full workspace
tests, recorded 335 frames and two failing steps: a 23 ms compositor-loop stall
and an 8.53 ms resize frame. Typing medians before were 1.18/1.16/0.88 ms;
after, 0.90/0.78/0.78 ms. All runs are retained in
[coder-reliability-qa.json](coder-reliability-qa.json); shared-host measurements
are not hard latency guarantees.

With this task's full workspace test workload stopped, the benchmark repeat
recorded **333 frames, zero failing steps**, with a **1.33 ms** slowest draw.
The earlier missed frames are retained rather than discarded. The final two
storage-failure regressions additionally verify that failed checkpoint writes
cannot cause a hot restart loop or prevent Stop from cancelling active work;
uncheckpointed effects remain explicitly unknown. Child admission rechecks the
parent's cancellation while holding the supervisor reservation, closing the
creation/Stop race. These final guards do not change the layout measured above.

After the storage and child-admission guards, the full release workspace suite
passed again: **1,664 tests, zero failures**, across 204 suites (138 ignored).
The final focused suite passed 58 tests and the workspace release build passed.
[Detailed audit evidence](coder-reliability-qa.json) includes the failing baseline
regressions, live Stop measurements, CLI recovery, visual cases and every
benchmark run.

## Durable delivery and provider audit — 2026-11-10

Two new regression tests failed against `6456d547`: a late failed peer attempt
could overwrite confirmed acceptance, and an incomplete Codex compaction reply
could replace the saved opaque context. Confirmed `sent` state now survives later
failed attempts and database reopen. The sender validates receipt identity,
destination, provenance and admission state while accepting pending text edited
at the recipient. Failed attempts remain in the audit history.

Explicit compaction now uses completion validation and rejects empty encrypted
payloads or unexecuted tool calls before replacing context. The regression checks
all three failures and verifies the original opaque fields after database reopen.
Claude streaming requires matching block starts/stops and a final stop reason;
duplicate indices, deltas after a block closes and unfinished argument streams
fail before tool admission. Unknown event types remain tolerated. Both providers
reject duplicate/empty tool IDs and non-object arguments.

The shared provider/MCP SSE decoder no longer rescans partial lines or shifts its
buffer for every line. A release benchmark decodes **851,968 bytes / 4,096 events**
nine times: median **42.878 ms before, 0.494 ms after** (ranges 42.159–53.298 ms
and 0.474–1.223 ms). This measures framing only, not inference, networking or
rendering, and is not a worst-case bound. Regression tests also cover Unicode,
CRLF and long lines split into chunks from one byte to a complete batch.

The final focused release run passed **56 tests**, with five explicitly ignored
cases (four live-provider cases and the separately run benchmark). The release
workspace build passed. Drawing/layout did not change in this audit; the preceding
90-case visual sweep remains the relevant visual evidence. The previous live
Claude reasoning/compaction attempt returned HTTP 429; this audit did not consume
more pool quota. Live Claude validation and a weeks-long live soak remain open.
See [coder-durability-qa.json](coder-durability-qa.json) for the measured evidence.

The full release workspace suite passed **1,670 tests across 204 suites, zero
failures**, with 139 explicitly ignored cases. The final core rerun includes the
empty-payload and unexecuted-tool compaction cases added during final review.

## Approval recovery and live journal order — 2026-11-10

Three regression tests failed before this audit's changes. A failed approval
decision checkpoint consumed its waiting channel, ending the active turn; both
allow and deny decisions now remain retryable after the storage problem is
resolved, and only one committed decision can release the tool. Automatic review
also replaced the coding session's context count with the reviewer's small window
(123,456 became 14 in the fixture), and malformed review decisions were omitted
from cumulative usage. Every completed reviewer request is now charged and
audited without changing the main model's active-context count. Invalid decisions
still require explicit approval.

Code inspection found that live publication happened after releasing the journal
lock. A later writer could publish first, advancing a subscriber's cursor past
the earlier event. All service writes now use one checkpoint helper that commits
and publishes under the same lock; bounded broadcast never waits for a reader.
The concurrency test checks four batches of 768 updates from 16 writers against
the durable sequence. It passed before as well as after: the test did not
reproduce the problematic scheduling interleaving.

Release journal benchmark, seven samples per run: **768 updates / 16 writers**,
median **951.426 ms before** (943.555–987.077), **1,076.085 ms after**
(1,026.669–1,606.023), and **1,001.663 ms on repeat** (919.534–1,079.468).
These are whole-batch wall times including thread startup and durable SQLite
commits on a shared host, not a speedup claim or a worst-case bound. The change
adds no idle polling or worker.

The focused release executables passed **60 tests**, with two performance cases
and four live-provider cases explicitly ignored; the journal benchmark ran
separately. The release workspace build passed. The first full suite stopped at
an existing `olo-net` fixture race: it asserted a socket option immediately after
the network generation changed, though the documented watcher order updates
generation before sockets. An isolated repeat passed; the fixture now waits for
both the generation and the socket effect before asserting. Production network
behavior is unchanged, and the initial failure is retained in the evidence.

No layout changes were made in this audit. Live Claude reasoning/compaction
verification remains open after the prior HTTP 429 responses, as does a
weeks-long live soak. [Detailed recovery evidence](coder-recovery-qa.json).

The final full release suite passed **1,674 tests across 204 suites, zero
failures**, with 140 explicitly ignored cases. The corrected namespace fixture
also passed ten consecutive isolated runs. Formatting and diff checks passed.

## Tool transport cancellation audit — 2026-11-10

Four new regression tests failed against `7437ff08`. MCP Stop waited for the
server's cancellation acknowledgement; pre-cancelled calls and completed server
errors sent spurious cancellation notices; valid empty SSE priming frames failed
JSON parsing; and an incorrect JSON-RPC version could be accepted as a successful
tool result. Both transports now share response validation, including mutually
exclusive result/error envelopes. Completed server errors leave the connection
reusable. Pre-cancelled work never dispatches a tool request or a cancellation
notice for one that was not issued.

HTTP cancellation runs independently of the local turn, bounded to five seconds;
session deletion waits for that cleanup, then has its own five-second bound.
The fixture verifies the matching request ID, one tool dispatch, no replay, and
cleanup before DELETE. An external effect may already have completed or may
continue; stopping locally is not confirmation of external rollback.

Release measurements against a local server delaying cancellation replies by
1,200 ms: Stop took **1,207.507 ms before**, and **4.845 / 5.363 / 3.521 ms after**.
A one-second tool deadline returned after **1,001.627 ms**, with one cancellation
notice and no replay. A GitHub write whose fixture withheld its response cancelled
locally in **0.029 ms**, reported its uncertain outcome, and was issued only once.
These are shared-host fixture measurements, not production-network or worst-case
latency guarantees. Cleanup adds no idle polling or permanent worker.

Provider, fetch and GitHub requests now prioritize cancellation before dispatch;
GitHub also stops waiting for credential lookup. The already-cancelled fixture
performs no credential lookup and sends no HTTP requests. The focused release
suite passed **66 tests**, with six opt-in performance/live cases ignored. The
release workspace build and formatting checks passed. No layout changed; live
Claude verification after the prior HTTP 429 responses and a weeks-long live
soak remain outstanding. [Detailed transport evidence](coder-transport-qa.json).

The full release workspace suite passed **1,680 tests across 204 suites, zero
failures**, with 140 explicitly ignored cases.

## Process output and artifact audit — 2026-11-10

Five regression tests failed against `0b0c7201`, after extracting the existing
drain/collection code without changing its behavior. An artifact write error
could wait for a still-running process; the final asynchronous write could fail
without reporting an error; streamed Unicode and truncated tails could acquire
spurious replacement characters; and the chunk crossing the artifact limit was
discarded wholesale, losing bytes that still fit within the limit.

Collection now stops on its first I/O error, letting the process-group guard
terminate remaining work. Each drain flushes its last write. Batches retain at
most three incomplete UTF-8 bytes, and truncated tails start at a character
boundary. The raw artifact retains exactly the permitted prefix. Tests cover
Unicode split at every byte, every byte pair streamed in chunks of 1/2/3/7/8,192,
and genuinely invalid or incomplete UTF-8; streamed replacement behavior matches
decoding the whole byte sequence.

Capture files are prepared before spawning, use mode 0600 and are removed if
process start fails. Successful capture waits for artifact data and directory
sync. Results report per-stream totals and truncation separately from saved
artifact totals. A real process fixture checks exact streamed text, truncated
tails, immediate full-artifact reads and permissions; another checks that capture
allocation failure cannot start the requested effect.

Release fault fixture: the old collector still waited when its **300 ms test
deadline expired** (301.432 ms observed); the corrected collector returned the
injected write error in **0.049 ms**. This uses a read-only file descriptor,
not a filled filesystem. The release capture benchmark runs a real sandboxed
process emitting **22,528 Unicode bytes**, nine samples: median **0.755 ms before**
(0.705–1.466), **3.292 ms after** (2.556–16.091). The extra latency includes
artifact syncing before success; it is an explicit durability cost, outside the
UI thread. Shared-host samples are not hard worst-case bounds. No power-loss
test was performed.

The focused release suite passed **75 tests**, with seven opt-in performance/live
cases ignored; the capture benchmark was run separately. The workspace release
build and formatting checks passed. No drawing/layout changed. Live Claude
verification after prior HTTP 429 responses and a weeks-long live soak remain
outstanding. [Detailed output evidence](coder-output-qa.json).

The full release workspace suite passed **1,689 tests across 204 suites, zero
failures**, with 141 explicitly ignored cases. After that workload finished, the
capture benchmark repeated at **2.677 ms median** (2.443–15.195); both after runs
are retained alongside the baseline.

A fresh opt-in Claude attempt after this audit again returned **HTTP 429** on
its first inference request. Retries were disabled for this quota check. The
live test now requires signed or redacted Claude thinking before claiming replay
coverage; the quota rejection prevented reaching that assertion or compaction.


## Child workspaces and native form recovery — 2026-11-10

Child tasks can now select an existing subdirectory or Git worktree inside their
parent workspace, through the native **Start Subagent…** form, `/agent JSON` in
either composer, or `spawn_agent`. A native Git fixture creates a branch and
worktree, writes through a child, verifies the parent files are unchanged, and
reopens the persisted child in its worktree. Separate fixtures reject traversal,
symlink escapes, non-directories and malformed tasks without orphan sessions.

The baseline admitted two children under a one-child limit because neither had
started running yet. Admission now counts supervisor reservations atomically,
including cancellation cleanup; resuming a child obeys the same cap. A waiting
parent also waits for an admitted child whose first turn has not yet started.
Counting queries only active IDs, independent of historical session count.

Live GUI and TUI checks against a local provider fixture each created one child
with the selected workspace, model and effort, inherited read-only policy and
one completed request. Native keyboard checks found and fixed Enter being
ignored in multiline forms. Return now inserts a newline there; single-line
Return and Command+Return submit. Main windows and forms now carry distinct,
persistent restore keys: a binary re-exec preserved both windows’ exact geometry
and every field, including the multiline task. Two identically titled main
windows and two identically titled forms also kept their own drafts and geometry
after re-exec. A compositor restart also
recovered the form draft. No model work was delegated for these checks.

All nine native forms were reviewed in light, dark and night at 1×, 1.25×,
1.5×, 1.75× and 2× while changing scale live: **135 cases, zero findings**.
The main window and new form also passed the initial 30-case sweep. Minimum
screen layout lint passed. The visual journal records the new form.

A workspace rerun exposed an unrelated network-test race: another test or the
network watcher can change the shared generation during a zero-duration wait.
The assertion now accepts that documented result while retaining the deadline
and wakeup checks. See [coder-workflow-qa.json](coder-workflow-qa.json) for final
release counts, benchmark results and the remaining validation limits. The
complete release workspace run passed **1,695 tests in 204 suites** (141
ignored); all eight interface tests passed after the final shortcut adjustment.
The final release build and layout lint passed. The final benchmark passed all
30 steps (322 frames): slowest non-transition Coder frame 2.55 ms versus
2.63 ms before, below the 8.3 ms budget. The baseline had three compositor
stall findings; the final run had none. These shared-host samples are not a
claim of a performance improvement. Settled GUI idle over three seconds used
0 CPU, 0 wakeups and 0 frames; the TUI emitted zero bytes during its separate
three-second idle observation.


## Custom endpoints and evidence-backed compaction

Claude support is removed from Coder. Codex retains Token Pool transport and native
encrypted reasoning/server compaction. Arbitrary OpenAI-compatible Chat Completions
endpoints have streamed tools, native reasoning replay, configurable authentication
by environment reference, a model catalog, and a native configuration form.

The new client compactor preserves immutable original records, exact source links,
raw instruction excerpts, recent complete tool exchanges, verified claims and a
checkpoint chain. Background work checkpoints each completed chunk and resumes it
only when source and policy identity still match. Prefix comparison protects newer
steering messages; restore preserves the suffix and never repeats filesystem effects.
Tests cover malformed summaries, rejected claims, cancellation, restart, repeated
compaction, forks, UTF-8 paging, migrations and authenticated custom tool loops.
System/tool snapshots and request manifests preserve the main model's inputs without
repeating the full transcript in every audit event.

Validation for this change:

- Release workspace build passed. Complete workspace run: **1,706 passed, zero
  failed, 205 suites**. Subsequent Coder-only changes received targeted release
  tests; final counts are recorded in `coder-compaction-qa.json`.
- Real Codex client-compaction smoke: **21 original items → 6 active items**, four
  source-checked entries, **7,086 input + 318 output tokens**. The package correction
  and approval constraint survived; every original item remained retrievable.
- Real Codex encrypted reasoning, provider compaction and replay passed; five
  native items compacted to one and the remembered marker/result survived.
- Custom endpoint authentication, catalog, complete tool loop and reasoning replay
  passed against local HTTP fixtures. No Fireworks account/model access is claimed.
- **30 window reviews** across light, dark and night mode at 1×, 1.25×, 1.5×,
  1.75× and 2×: zero findings. The test verifies actual scale and active night-light
  state. Live minimum-screen lint passed. The unfinished endpoint form survived
  an isolated GUI re-exec; its service also upgraded independently.
- Actual Ratatui session attachment, transcript retrieval, checkpoint history and
  detach passed against the same isolated custom-provider session as the GUI.
  Final service/GUI reload retained that session through database version 6;
  subsequent TUI retrieval passed. Settled GUI idle over three seconds measured
  zero CPU, wakeups and frames.
- Release UI benchmark: baseline maximum steady frame **28.82 ms** with six failed
  steps (shared-host/server stalls included); completed run **5.31 ms**, maximum including
  transitions **5.78 ms**, **325 frames / 30 steps / zero failures**. These are
  observed runs, not a controlled claim that this feature accelerates rendering.
  A subsequent catalog-refactor check measured **3.47 ms** maximum steady Coder
  frame, with one compositor stall finding; every Coder frame met its budget.
- Archive benchmark, 100 operations each: growing durable commits **163.43 ms**,
  unchanged-context commits **132.98 ms**, indexed searches **8.69 ms**.
  Reading 100 first pages from a 2 MB record took **0.87 ms** using incremental
  SQLite reads versus **29.55 ms** loading whole records. Ten forks of a
  10,000-record archive with six active items took **12.45 ms**, copying zero raw
  transcript records. Forks inherit frozen source ranges and checkpoint snapshots.
  Clippy completed; repository style/complexity warnings remain.

[Machine-readable results](coder-compaction-qa.json) and
[research, design and configuration](CODER-COMPACTION.md) describe the evidence and
limits. The deterministic fixtures and live smoke do not establish multi-week task
success or a universal advantage over other compaction strategies.

## Exact retrieval under tool-output limits — 2026-11-10

A follow-up audit found that the final tool-output clip could cut a transcript
page or checkpoint response inside its JSON. Raw text page limits did not account
for JSON escaping or the response envelope. The agent could therefore lose its
continuation cursor even though the original source remained in the database.

Transcript and artifact pages now reserve room for the encoded response.
Oversized tool results retain their complete JSON in an atomically written,
synced artifact; the model receives valid JSON with its reference and a preview.
Artifact reads preserve UTF-8 boundaries, return binary data as base64, and end
with a null continuation cursor. Small results take no disk-write path.

Validation: **104 targeted release tests passed, zero failures** (10 opt-in tests
ignored). A complete HTTP agent/tool loop reconstructs an escape-heavy original
record and a large checkpoint exactly at the minimum **1 KB** tool-output budget.
Additional tests cover binary bytes, incomplete UTF-8 at EOF, invalid offsets,
empty final pages and persistence failure. This changes service retrieval only;
the GUI/TUI layout is unchanged. The release workspace build passed. Clippy
completed with the existing style/complexity warnings and no errors.

Release benchmark on the shared host: **10,000 small replies, 1.94 ms before /
1.86 ms after**; **100 oversized replies, 15.52 ms clipping before / 1,670.96 ms
with durable artifacts after**. The extra oversized-result cost includes atomic
writes, file sync and directory sync; it runs off the runtime worker thread.
These samples are not a claim of a speed improvement. The earlier clipping path
discarded output from the model-facing response; the new path makes it retrievable.

## Compaction admission and cancellation — 2026-11-10

Two new HTTP regressions reproduced a budget overrun: both custom compaction and
Codex server compaction could exhaust the current run budget, then admit another
foreground request. Request admission now checks current billed usage under the
journal lock after compaction. The completed checkpoint remains available, and
an explicit Resume starts a fresh run budget. Already admitted concurrent model
requests can still exceed the budget when their actual usage is reported; this
is a request-admission budget, not a provider billing cap.

Checkpoint publication now checks cancellation under the journal lock. A worker
waiting there cannot overwrite Stop's interrupted checkpoint. A deterministic
thread/lock fixture exercises preparing, chunk and ready publication after Stop's
cleanup; all three are rejected. Cancelled checkpoint application preserves the
original context. Manual, server-compaction and foreground completion commits
also check cancellation before setting an idle state.

**107 targeted release tests passed, zero failures** (11 opt-in tests ignored).
The release workspace build passed. Clippy completed with existing style and
complexity warnings. The custom-provider regression also verifies a successful
explicit Resume from the saved checkpoint. These service changes do not alter
GUI/TUI layout, frame processing or idle behavior.

The interleaved release benchmark alternates execution order for 100 commits per
path. Checkpoint publication totaled **136.41 ms before / 138.77 ms after**;
median latency **1.245 / 1.256 ms**. Request-manifest publication totaled
**150.80 / 140.98 ms**; median **1.227 / 1.284 ms**. Both paths include durable
SQLite commits. The earlier sequential sample and paired p95/max latencies are
retained in `coder-compaction-qa.json`; shared-host samples do not establish a
speed improvement or isolate the guard's CPU cost.

## Selectable settings, supervision and native integrations — 2026-11-10

Model configuration now has named, persistent profiles shared by a service.
GUI menus and searchable TUI pickers choose profiles for the primary agent,
supervisor, approval reviewer and compactor. Provider catalogs supply model
identifiers; changing a connection invalidates its catalog. Session settings,
MCP authentication, recipients, delivery and password fields use choice controls.
Rejected submissions retain their drafts, with errors in the form. The kit's
structured ComboBox nodes now report the selected value, too.

The optional supervisor has bounded reviews and interventions, separate usage,
custom guidance and read-only transcript retrieval. MCP supports stdio and
Streamable HTTP, environment/Passwords tokens and OAuth with PKCE and sealed
credentials. Slack and Logfire presets use their official endpoints. Olo
Terminal titles and Agent Screens run through native protocols/libraries;
Passwords offers item/field metadata before an explicit field grant.

Functional checks cover profile discovery/save/application, all four model
roles, invalid numeric values, rejected MCP forms, stale catalog invalidation,
upgrade-restored drafts, compositor reconnect, local and linked-instance
recipient selection, and attributed message delivery. A custom HTTP fixture
advertised 100 models and selected a namespaced model without typed identifiers.
A Passwords picker fixture recorded two metadata calls and no secret-value
reads. PTY checks exercise settings search, catalog discovery, profile saves,
MCP rejection retention and the separate transcript-links shortcut.

A real Token Pool run completed with an enabled supervisor: the primary used
2,404 input / 5 output tokens; its supervisor used 691 input / 35 output tokens,
reviewed once, and correctly observed completion without an intervention.
Native Agent Screens launch/tree/capture/stop and Terminal title calls passed.
Slack/Logfire account sign-in was not performed against real accounts; OAuth,
refresh, cancellation and credential handling use protocol fixtures. Slack
requires an already registered app. No messages were sent to external services.

Detailed test counts, visual reviews and release measurements are retained in
[coder-integrations-qa.json](coder-integrations-qa.json). Shared-host timing
samples include scheduling outliers and are not a speed-improvement claim.

Validation: **190 targeted release tests passed, zero failures, 15 opt-in tests
ignored** across Coder, its core, Agent Screens, Passwords and the keychain.
The final change to preserve concurrent edits also passed all 17 Coder frontend
tests. The release workspace build passed; Clippy completed with style and
complexity warnings, no errors. Headless checks cover light/dark/night and
1×, 1.25×, 1.5×, 1.75× and 2×. Two initial 1.75× alignment findings were
reproduced and fixed; the final form reviews are clear. Live layout lint also
passed at the 1280 × 800 minimum screen.

Final paired 2× release runs: **120 / 121 frames**, ordinary-frame maxima
**6.83 / 3.59 ms** before/after; typing p99 **1.17 / 1.28 ms**, resize p99
**6.83 / 3.59 ms**. The baseline had two compositor failures (8.8 ms frame,
39 ms loop stall) and a 9.51 ms launch transition; the final app passed every
step. Both earlier serial runs also passed. A separate repeat during compilation
had seven failures, retained in the evidence rather than discarded. Synthetic
key dispatch to first frame start measured median **0.281 / 0.374 ms**, maximum
**0.556 / 0.563 ms**, excluding render time and display scanout. Final app and
service each recorded **0% CPU, zero wakeups** over three idle seconds; the
app's main/settings windows drew **zero frames**.
