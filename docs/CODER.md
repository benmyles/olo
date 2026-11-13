# Olo Coder

Olo's Rust coding harness has three clients for one service: a Ratatui terminal,
a native `olo-ui` window, and a versioned JSON API. The service owns each turn;
closing a client detaches it. Reopen either interface with the session ID to
continue watching the same work. All commands are available in both interfaces.

```sh
olo-coder --tui --workspace /path/to/project
olo-coder --gui --workspace /path/to/project
olo-coder --gui --session SESSION_ID
olo-coder --rpc '{"op":"list"}'
```

Without a mode, a terminal opens the TUI; a desktop launch opens the GUI. `--help`
never opens a window. The first client starts the service. State lives in
`$XDG_STATE_HOME/olo/coder` (normally `~/.local/state/olo/coder`); `--data DIR` or
`OLO_CODER_HOME` chooses another store. Its directory and socket are owner-only.
A separate store is a separate service. Do not share one database over a network
filesystem. Codex requests go to the loopback Olo Token Pool, `OLO_POOL_PORT`
(17891 by default). Custom OpenAI-compatible endpoints use environment-referenced
credentials in the service; key values are never stored in session configuration.

See [Supervisor and integrations](CODER-INTEGRATIONS.md) for supervisor controls,
MCP OAuth/token setup, Slack, Logfire, Terminal, Agent Screens and Passwords.

## Workflows

Type a request and press Enter. The delivery selector controls when new input
enters the running conversation: after the next tool, after the current response,
or immediately by interrupting current work. The default is after the next tool. Escape stops the TUI's
turn; the GUI has Stop and Agent ▸ Stop. Closing a view leaves the turn running.
TUI Ctrl+D detaches, PgUp/PgDn scroll, Alt+Enter inserts a newline, Up/Down recall
input, Tab completes command names, and Ctrl+O opens settings and Ctrl+L opens transcript links (including action details). The GUI composer uses Shift+Enter for a newline. In the GUI, the transcript supports
selection and copy; View ▸ Toggle Follow Conversation lets you read earlier
text without the next chunk scrolling it away.

Both composers accept these commands:

| Command | Effect |
| --- | --- |
| `/new [directory]`, `/sessions [query]`, `/open ID` | Create, search, attach |
| `/pin [on\|off]`, `/rename TITLE`, `/tags TAG…` | Organize a session |
| `/archive`, `/restore` | Hide finished work; restore it before resuming |
| `/interrupt TEXT`, `/after-tool TEXT`, `/queue TEXT` | Choose an explicit delivery boundary |
| `/inbox`, `/cancel-message ID`, `/retry-message ID` | Inspect receipts; cancel pending input; retry an unconfirmed peer send |
| `/connect ALIAS STORE`, `/peers`, `/peer-sessions ALIAS`, `/disconnect ALIAS` | Link and inspect separate local services |
| `/message [ALIAS/]SESSION TEXT` | Send an attributed collaborator message |
| `/status`, `/stop`, `/resume` | Inspect, cancel, resume |
| `/goal objective`, `/keep on` | Track a goal; continue after answers until completed or a run budget stops work |
| `/todos JSON`, `/memory text` | Set tracked work and durable handoff notes |
| `/spec on`, `/spec text`, `/accept [revision]` | Collaborate without workspace effects; accept exactly the current revision to implement it |
| `/attach path` | Attach a workspace text file, PNG or JPEG (up to 20 MB), while idle |
| `/model ID [effort]`, `/provider codex\|openai`, `/models` | Select any provider model ID; discover the configured provider's catalog |
| `/approval ask\|auto\|allow`, `/guidance text` | Configure action approval |
| `/approve [CALL_ID]`, `/deny [CALL_ID]` | Resolve a particular pending action, once |
| `/agents on`, `/readonly on` | Enable optional child agents or restrict workspace effects |
| `/compact`, `/history [query]`, `/fork [title]` | Compact context, search saved history, fork an idle checkpoint |
| `/checkpoints`, `/undo EDIT_ID` | List recoverable edits/deletions; restore only when newer work will not be overwritten |
| `/event SEQUENCE` | Inspect the complete durable action behind a transcript detail link |
| `/git OP [JSON]`, `/github OP JSON` | Native repository and GitHub operations |
| `/tool NAME JSON` | Manually run a native or configured `mcp__SERVER__TOOL` tool through the same approval and audit path |
| `/config [JSON]`, `/set dotted.key JSON_VALUE` | Inspect or change session configuration |
| `/defaults [JSON]` | Inspect or persist defaults for subsequently created sessions |
| `/api JSON` | Run any command in the machine-readable API |
| `/help`, `/quit` | Reference, detach |

Changing configuration requires a stopped turn. A provider switch needs an empty
session: encrypted reasoning cannot be translated between providers. A model
change within a provider keeps its original opaque items; the provider decides
whether the selected model accepts them. Model IDs are never silently replaced.
Set effort to `""` to omit it for models that do not support effort.
`model.max_output_tokens` limits custom Chat Completions output; the Codex
subscription endpoint controls its own per-response limit. Run budgets cover
both providers, approval review and client compaction.

## Known services

Coder knows how to set up some OpenAI-compatible services, so choosing one is a
single step: **Settings ▸ Model ▸ Runs on**, `/provider ID`, or `"provider":"ID"`
in `spawn_agent`. Each is an ordinary custom endpoint in the configuration,
recognised by its address (`crates/coder/src/providers.rs`), with its models'
windows, output limits and reasoning efforts filled in.

| ID | Service | Models |
|---|---|---|
| `fireworks` | Fireworks AI (`https://api.fireworks.ai/inference/v1`) | DeepSeek V4.1 Flash on Ultrafast (`accounts/fireworks/routers/deepseek-v4p1-flash-1017-ultrafast`, the default) and on the standard path (`accounts/fireworks/models/deepseek-v4p1-flash-1017`): 1M context, 128K output here, effort `high`, `low` or `max` |

Fireworks requests carry `x-session-affinity: SESSION_ID`, which keeps a
session on one replica so its prompt prefix stays cached. The key is the
service's **Key** field: an Olo Passwords field reference (`op://vault/item/field`,
read by the service when it first needs it, kept in memory only, read again
after the service refuses it) or the name of an environment variable the
service has (`FIREWORKS_API_KEY` until changed):

```text
/provider fireworks
/set model.endpoint.api_key_env null
/set model.endpoint.api_key_password "op://Private/Fireworks/credential"
```

## Custom OpenAI-compatible endpoints

Choose **Settings ▸ Model ▸ Runs on: An OpenAI-compatible endpoint**, or
configure an empty session:

```text
/endpoint {"base_url":"https://example.com/v1","api_key_env":"EXAMPLE_API_KEY"}
/model YOUR_MODEL_ID
/set model.context_window 262144
```

Use the model/deployment ID and window published for your account (a new
custom endpoint assumes 1M, so set yours). `/models` queries the endpoint's
`GET /models` catalog when supported. Requests use `POST /chat/completions` below
the configured API root. The GUI saves endpoint and model together. The TUI's
`/config` or JSON `configure` command can also update them atomically.

A key in an environment variable (`api_key_env`) must be in the service's
environment when it starts; `api_key_password` (a Passwords field) needs no
restart. For a variable, export it in the shell that starts the service, then
launch Coder. Changing a client's environment does
not alter an already-running service; after its work is stopped, its explicit
`--rpc '{"op":"shutdown"}'` command allows restarting it from the new environment.
Never paste a key into a session message or configuration. An unauthenticated local
server can leave `api_key_env` null. `header_env` maps additional HTTP header names
to environment variable names. HTTPS is required except for loopback HTTP;
redirects are not followed.

`endpoint.stream` and `endpoint.include_usage` default to true. Set the latter
false for gateways that reject streaming usage options. `token_limit_field` can
be `"max_tokens"` (default) or `"max_completion_tokens"`. Model-specific fields go
in `model.extra`; required routing, message, tool and completion-count fields
remain managed by Coder. `model.thinking` supports provider-specific thinking
configuration. Native assistant reasoning, including `reasoning_content`, is
retained and replayed across tool turns. Ordered `reasoning_details` blocks,
including opaque encrypted data, also survive streaming and replay
([gateway reasoning format](https://openrouter.ai/docs/guides/best-practices/reasoning-tokens)). Custom models must support function tools
for the full agent workflow. They use Coder's [client compactor](CODER-COMPACTION.md),
with immutable transcripts, evidence-linked checkpoints and background preparation.

Claude is no longer a Coder provider. Existing Claude sessions are paused and their
original state is archived before conversion to an empty Codex working context.
The independent Olo Token Pool's support for other applications is unchanged.

Protocol references: [Fireworks OpenAI compatibility](https://docs.fireworks.ai/tools-sdks/openai-compatibility),
[Fireworks Chat Completions](https://docs.fireworks.ai/api-reference/post-chatcompletions),
[DeepSeek thinking and tool replay](https://api-docs.deepseek.com/guides/thinking_mode/).

## Sessions, tiles and steering

The native Sessions panel searches titles, tags and workspaces, sorts pinned
sessions first, and includes archived sessions on request. Open a session here
or in another window. Session ▸ New Window starts an independent view; View ▸
Tile Coder Windows separates Olo's window groups and tiles the windows on the
current workspace. Compact windows use a panel selector to keep every operation
available. Four windows fit the minimum 1280×800-point screen beside Deskbar.
Windows, attached sessions, unsent drafts, selected panels and delivery modes
survive an in-place upgrade. Switching a view retains its draft for each session.

In the TUI, **Ctrl+P** opens the searchable session picker; **Alt+P** pins the
selected row. **Ctrl+T** opens a session in another conversation tile (up to four),
**Ctrl+N** focuses the next tile, and **Ctrl+W** closes the active tile when there
is more than one. Each tile has its own transcript, scroll position and drafts.
**Alt+M** cycles delivery modes. An upgrade restores all tiles. Ctrl+D detaches
all of them without stopping work. `NO_COLOR` is respected; the active tile and
selected row still have text markers. Ctrl+A/E move to the beginning/end of the
composer; Ctrl+U/K remove text before/after its cursor.

Stop uses a separate control connection, so a slow peer or another pending
frontend request cannot hold it up. Prompts still waiting in the frontend are
returned to their session's draft when stopped, or if the attachment changes
before submission. Already accepted messages remain in the durable Inbox.
Stopping a parent clears pending restart intent throughout its child tree;
completed children retain their completion state.

| Delivery | Running session | Idle session |
| --- | --- | --- |
| After next tool | Finish the current/next tool, record skipped results for the rest of its old batch, then admit the message | Admit on starting work |
| Queue after response | Finish the current tool chain and assistant response, then admit the message | Admit on starting work |
| Interrupt now | Cancel current work, journal its outcome, skip unstarted calls, then restart with the new message | Admit on starting work |

Pending messages are durable. The Inbox panel lets you edit, reorder, cancel, or
retime an unconsumed message. An interrupt cannot be edited or cancelled after
cancellation has begun; Stop prevents the restart and allows editing or cancelling
the pending interrupt. Stop leaves pending input
available for explicit Resume. Archiving requires stopped work and no unresolved
calls or messages. Skipped provider calls always receive matching tool results,
so steering preserves valid provider history, including opaque reasoning items.

## Communication between instances

Windows and terminals attached to the same store already share one service.
Sessions address each other by ID. Separate `--data` stores have distinct,
persistent instance identities and independent journals. Link them explicitly:

```text
/connect review /path/to/review-service-store
/peer-sessions review
/message review/SESSION_ID Please review the cancellation behavior.
```

The other service must be running. Linking from the composer establishes the
reverse link too (named `peer-` followed by the initiating instance's short ID).
Links use owner-checked local Unix sockets; they are not a network transport.
Unlinking removes the local contact. Each outgoing message carries its immutable
sender instance/session, destination, UUID, text and delivery mode. A replaced
store with a different identity is rejected until deliberately linked again.

Collaborator text is explicitly identified as untrusted data. It cannot grant
approval or change session policy. `accept_messages` defaults to true;
`wake_on_message` defaults to false. Starting idle work requires both the
recipient's opt-in and the sender's `wake: true`. The `/message` shortcut leaves
idle work asleep. Model `send_message` calls pass through normal approval policy.
The `sessions`, `peers` and `session_messages` tools expose structured discovery.

Receipts distinguish `queued`, `delivered` (admitted into context), `cancelled`,
`sending`, `sent` (peer acceptance confirmed) and `uncertain`. They do not claim
that requested work was completed. A timeout, cancellation or service restart
can leave delivery uncertain. `/retry-message ID` resends the original message
with its original UUID; the recipient deduplicates it even if its pending copy
was edited or has already been consumed. Retry never wakes a recipient that
has not opted in. A rejected request is not silently retried with a new ID.
The sender validates the receipt's message identity, destination, sender and
admission state before recording `sent`. Once acceptance is confirmed, a later
failed attempt cannot downgrade that fact to `uncertain`; its failure remains
in the audit history. A receipt may contain text edited at the recipient.

The API exposes `browse`, `organize`, `steer`, `edit_message`, `move_message`,
`cancel_message`, `messages`, `receipt`, `send_message`, `retry_message`, `peers`,
`link_peer`, `unlink_peer` and `peer_sessions`. Supply `message_id` when retrying
a `steer` or `send_message` request after an ambiguous transport failure. Reusing
an ID for different content fails. `messages` pages by sequence; `browse` pages
by offset (100 by default, at most 500). A `subscribe_sessions` JSON-line request
with a `filter` streams catalog snapshots after changes, without idle polling.
An inbox accepts at most 128 messages and 4 MB of pending text; each message is
limited to 1 MB. These bounds protect other sessions from an unattended sender.

## Keys

The window's menus have the keys a person expects and Coder's own (Preferences
▸ Keyboard can change them; `olo_protocol::app_keys`): ⌘N New Session, ⇧⌘N New
Window, ⌘O New Session in Folder, ⌘D Fork, ⌘W, ⌘Q; ⌘Z/⇧⌘Z, ⌘X, ⌘C, ⌘V, ⌘A (the
message being written when it has the focus, else the conversation), ⌘F Find
Session, ⌘, Settings; ⌘↩ Send, ⌘. Stop, ⌘R Resume, ⇧⌘Y Allow, ⇧⌘D Deny, ⇧⌘G
Set Goal; ⌥⌘S Sessions, ⌥⌘I Work, ⌘1–3 Tasks, Changes and Context, ⌘↑ ⌘↓ the
previous and next session, ⌥⌘↓ ⌥⌘↑ open and close every action, ⌘/ Commands.

## Tasks and status

The task list beside the conversation (the window's Tasks tab; Alt+T in the
terminal) is a tree: tasks with subtasks, each `pending`, `in_progress`,
`done` or `blocked`. The agent makes it with `set_todos` (a subtask names
its `parent`) and keeps it current with `update_todos`; marking a task done
needs evidence. Coder reminds it, with the session state of each request,
when it has worked 8 actions without updating the list, or 3 without
making one, and when 5 have gone by without a `report_status`: what it has
recently done, what it is doing now and what is next, shown on the Now card
(and the terminal's Now/Next line). Its first report names the session (`title`, 2–6
words); until then a session is called by its first message's first line,
cut at a word. Once the person renames it (`organize`), the agent's names no
longer replace theirs (`named`).

The person edits the list as freely: click a task's mark to move it on
(to do, in progress, done), or use its menu (any status, Add Subtask,
Rename, Move Up/Down, Indent, Outdent, Delete); type in Add a task to add
one beside the chosen task. In the terminal: Space, `b` (blocked), Enter
(rename), `a`, `s` (subtask), Tab and Shift-Tab, Alt-Up/Down, Delete.
Each edit is an `edit_todos` command applied to the list as it is then,
so the agent's updates and the person's never undo each other; the agent
is told what changed at its next step, and the conversation notes it.

```text
/api {"op":"edit_todos","session":"ID","edits":[{"edit":"add","text":"Write docs","parent":"t1"},{"edit":"change","id":"t2","status":"done"},{"edit":"move","id":"t3","parent":null,"before":"t1"},{"edit":"remove","id":"t4"}]}
```

## Specs, goals and children

Specifications carry a revision, text, acceptance criteria and an accepted
revision. Every text revision invalidates acceptance and enters spec mode.
Accepting a stale revision fails. A criterion has `id`, `text`, `status` and
`evidence`; statuses are `pending`, `in_progress`, `done`, `blocked`. Completed
criteria and todos require evidence. The agent can update criterion status
without changing the accepted text. `complete_goal` requires every tracked item
to be done and the spec to be accepted. It records the agent's evidence; it is
not an independent proof that the code is correct. When blocked, `pause_work`
records the specific reason, marks the goal blocked and stops the run without
spending further keep-going turns. Resume or a new user message reactivates it.

Child agents are off by default. When enabled, `spawn_agent` creates a durable
child session with its own model, effort, context, status and usage. The optional
`workspace` selects an existing subfolder or Git worktree inside the parent
workspace; omitting it inherits the parent directory. Missing folders, files,
parent traversal and symlinks outside the workspace are rejected before creation. `model_config` overrides every model field, including thinking budgets,
service tier, output limit, cache/compaction settings and extra provider fields. Children default to read-only and inherit the parent's approval and
sandbox policy. They cannot loosen a read-only parent or spec mode. Defaults
allow three active children per parent and two levels; both limits are configurable.
Admitted children reserve their slots immediately, including while waiting to
start or finish cancellation. Resuming an existing child uses the same limit.
Use `agent_status`, `wait_agent` (event-driven), `message_agent`, and `stop_agent`
to coordinate. A child's finish is journaled on its parent. All child sessions
can also be opened directly in either interface. Stopping a parent cancels its
running descendants. Forking an idle session is distinct from delegating work.
The native Agent menu offers **Start Subagent…**, with a task, subfolder, model
and effort. It starts read-only; blank optional fields inherit the parent’s
settings. The same operation is available in both composers with `/agent JSON`,
or through the `spawn_agent` tool/API. For isolated writable work:

```text
/git branch {"branch":"agent-review"}
/git worktree_add {"branch":"agent-review","path":"worktrees/review"}
/agents on
/agent {"task":"Implement the accepted change and run its tests","workspace":"worktrees/review","read_only":false,"model":"gpt-6.1-terra","effort":"high"}
```

Wait for each repository operation to finish before the next command. The child
inherits the parent’s approval restrictions even when writable. Coder does not
automatically merge or discard branches. Unrelated workspaces can be opened as
separate root sessions and coordinated with session messages.

## Approval and confinement

`ask` permits native workspace reads and tracking operations, and asks for
workspace edits, developer processes and MCP tool calls. `auto` asks a separate
configurable reviewer model to judge the exact action against
`approval_guidance`. Only a parsed affirmative verdict approves it; a refusal,
malformed verdict or reviewer failure goes to the person. Decisions are recorded.
`allow`, the default, approves tool effects. `denied_tools` always wins over
`allowed_tools`, and spec/read-only restrictions win over approval. An approval
is tied to its session and tool call ID, not a blanket approval for future calls.
If saving a decision fails, its pending approval remains available for retry;
the tool cannot proceed without the committed decision. Completed reviewer
requests are audited and charged to cumulative usage even when their verdict
is malformed. A review's context size never replaces the coding model's active
context count.

By default (`sandbox: "unrestricted"`) programs the agent runs may write
anywhere, as builds with shared caches need; with the default `allow` nothing
stops them, so choose `ask` or `auto` to be asked. `sandbox: "workspace"` (Settings → Safety → Files: Only the workspace
folder) uses Linux Landlock ABI 3 to confine **process writes** to the
workspace and Coder's artifact directory, plus `/dev/null`, and fails closed if
the kernel cannot enforce it. Either way host reads and networking are
available; neither is a container or a credential-isolation boundary. Native file tools reject parent and symlink escapes and direct `.git`
access. External programs are explicit developer commands with argv, never
implementations of native file/search tools. They run without the person's GUI
display variables; graphical work should launch an Agent Screen deliberately.
Timeout, Stop and cancellation kill the process group, including descendants.
`fetch_url` reads HTTP(S) documentation natively with a 30-second timeout, bounded
response size and cancellation. It uses action approval, including in spec mode.
Provider, fetch and GitHub requests check cancellation before dispatch. Stopping
a GitHub operation also stops waiting for credential lookup; a lookup already
running on its worker may finish, but cannot dispatch the cancelled request.
Returned source is untrusted text; it does not execute scripts or forward credentials.

User-configured MCP servers are trusted host programs: starting one is authorized
by configuring it, its startup is not a model tool action. Their subsequent tool
calls use the normal approval path. Similarly, configured hooks are explicitly
authorized programs, run within the configured process sandbox; spec and
read-only sessions do not run effect hooks.

## Configuration and extensions

`/config` shows the exact effective configuration; unknown fields and invalid
limits are rejected. Examples:

```text
/set model.effort "high"
/set model.compact_at 300000
/set max_turns 400
/set token_budget 4000000
/set command_timeout_secs 300
/set reviewer.model "gpt-6.1-sol"
/set approval "auto"
/set subagents true
/set model.thinking {"type":"adaptive"}
```

Defaults: no limit on model requests (`max_turns: 0`) or input/output tokens
(`token_budget: 0`) per explicit run; 0 is no limit for either. Compaction at
500,000 active-context tokens, keeping the latest 50,000 verbatim, in 100,000-token
chunks with 16,000-token summaries; 128,000 output tokens per response; an
872,000-token window for Codex models (the most the pool takes); 100 KB tool
results, 10-minute commands, 30-minute provider requests, five retries of
transient failures. Every action is allowed (`approval: "allow"`). The
supervisor, when on, has 400,000 tokens of its own.
Resume explicitly begins a new run budget; automatic keep-going does not.
Cached input and cache writes are reported separately; model/context accounting follows provider
usage, with a conservative serialized-size estimate for new tool output.
The active context, cumulative usage, compaction count and goal remain visible.

`model.extra` preserves optional provider fields for new capabilities (for example provider-specific context settings). Required
protocol fields, model, input, function tools, streaming and `store:false` are
owned by the adapter and cannot be overwritten by this map. Invalid provider
options are surfaced as errors instead of silently downgrading the model.

Root and directory-specific `AGENTS.md` files accompany native reads. Workspace
`.agents/skills/*/SKILL.md` and configured `skill_dirs` are cataloged by description.
The agent uses `read_skill` to load relevant instructions and supporting text,
with bounded sizes, so unused skills do not fill the context. Review which directories you configure: skills are prompts,
not an approval bypass.

MCP stdio and Streamable HTTP servers use JSON-RPC initialization, paginated discovery and tool calls:

```json
{"name":"project","program":"/path/to/server","args":["--stdio"]}
```

Put these entries in `mcp`. Tools are named `mcp__project__TOOL`. Requests have
bounded timeouts and can be cancelled. Server-initiated sampling and elicitation
are not advertised and are explicitly rejected. For Streamable HTTP, use an HTTPS URL instead of a program:

```json
{"name":"remote","url":"https://example.org/mcp","header_env":{"Authorization":"PROJECT_MCP_AUTHORIZATION"}}
```

Header values come from the service environment, never the model context. HTTP
connections negotiate the protocol version, preserve session IDs, accept JSON or
SSE replies, and cancel without replaying POST effects. Redirects are disabled.
Empty SSE priming events are accepted. Responses require JSON-RPC 2.0 and exactly
one result or error; malformed replies cannot be recorded as successful tools.
A completed server error leaves the connection reusable and does not send a
cancellation notice. A pre-cancelled call is never dispatched.

Stop and tool deadlines stop waiting locally without waiting for a cancellation
acknowledgement. HTTP cancellation notices run as bounded best-effort cleanup
(five seconds); session deletion follows that cleanup, with its own five-second
bound. This does not guarantee that an external effect stopped: inspect external
state before retrying an interrupted call. No request is automatically replayed.
This follows the [MCP cancellation lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/cancellation)
and [Streamable HTTP transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports).

Interactive OAuth setup, server sampling, elicitation, and resumable GET streams
are not advertised. Configure a supported credential header for protected servers.

`hooks` entries have `event` (`before_tool` or `after_tool`), `program` and `args`.
Arguments can contain `{session}`, `{tool}`, `{arguments}`; substitution produces
argv strings, never shell code. A before-hook failure prevents execution; an
after-hook failure is reported after the tool result has been committed. Hooks
have a 30-second timeout. No hooks run until configured by the person.

## Durable context and recovery

SQLite WAL with FULL synchronization commits session snapshots and their audit
events together. Tool intent is saved **before** an effect, its result afterward.
After a service crash, active sessions become interrupted. An in-flight effect
has an unknown outcome and is never automatically replayed. Inspect the workspace,
then Resume supplies an explicit unknown-outcome tool result so the model can
reconcile what happened. Partially received provider replies never execute tools.
A dropped provider stream fails the turn without appending partial native output.
Custom streams must supply a valid completion reason and `[DONE]` before
admission. Tool calls require unique, nonempty IDs and object arguments, so an
unfinished argument stream cannot become a call with placeholder arguments.

A snapshot doesn't repeat the conversation: each context item is a transcript
record written once, and the snapshot keeps their IDs (`context_refs`). Reading
a session rebuilds its context from them, reading only records added since it
was last read (the service keeps the last eight sessions' contexts built). A
730 KB session's per-event save costs 1.7 ms and writes 150 KB, not 6.3 ms and
1.2 MB (`tests/store_cost.rs`). `get` doesn't build the context at all. The
database is version 7 from this change; an older Coder refuses it rather
than read snapshots as having no context.

The full audit history is paged and searchable through a persistent SQLite FTS5
trigram index, independently of the active context. Views retain at most 256 KB / 2,000 recent lines; reconnect initially
replays 500 events and then follows new ones. Events have monotonic sequence
numbers for gap-free reconnect. Slow subscribers disconnect with a replay cursor.
Idle clients and service workers sleep on sockets/channels, without polling.
On Olo's upgrade signal, the service drains active turns and pending approvals,
rejects new turns temporarily, then re-executes. It marks itself as waiting so
the desktop does not kill an in-progress operation. Clients reconnect and replay
from their cursors. A service whose program file was replaced by a newer build
that olo-session doesn't upgrade (a worktree's `target/release`) is sent that
signal by the first client of the new build to connect: otherwise a new window
would talk to an old service, which refuses settings it doesn't know. Initial connection failures retry with bounded backoff while
retaining the requested session and composer draft. Reconnection appears as a
temporary status rather than repeated transcript entries. The GUI and TUI preserve the attached session and draft;
the GUI also restores its selected pane, follow setting, and unfinished forms
(including the original session and expected commit head). Compositor reloads
use the kit's state restoration.
Streaming display updates are batched without splitting UTF-8 characters. Genuine
invalid bytes display as replacement characters; the artifact keeps the raw bytes.
Truncated tails start at a character boundary. Tool output goes to bounded disk
artifacts (up to 64 MiB per stream); results contain tails and an artifact ID for
ranged `read_artifact` retrieval. `stdout_bytes`, `stderr_bytes` and their
`*_truncated` flags distinguish complete output from a tail; `artifact_bytes`
and `artifact_truncated` describe the saved raw output separately.

Capture is prepared before a process starts, with owner-only permissions. A
failed start removes its unused capture file. Output errors stop waiting for
the process immediately and terminate its group; errors identify the partial
artifact. Before a successful capture returns, buffered writes finish and the
artifact data and containing directory are synced. These filesystem operations
run on workers and remain within the command's cancellation/timeout boundary.
Artifacts and history are retained until the store is
removed; total lifetime disk retention is not automatically pruned.

Codex requests ask for `reasoning.encrypted_content` and replay the complete native
items, including unknown fields. Explicit and automatic compaction use a final
`compaction_trigger` input item on the subscription Responses endpoint. The
returned encrypted window replaces the old active window; no client summary is
substituted. Inline compaction items in normal responses are also preserved.
An incomplete response, empty compaction payload or unexecuted tool call rejects
explicit compaction and leaves the previous durable context intact.
Custom endpoints use [client-side checkpoint compaction](CODER-COMPACTION.md).
The immutable native transcript remains available through `read_transcript` and
`search_transcript`, independently of the smaller active context. `/compactions`
inspects checkpoint evidence and `/restore-context ID` restores the current
checkpoint while idle. Server and client compaction retain newer steering messages.
Preflight context estimates traverse borrowed native values without making a
serialized copy. Image encodings have a fixed conservative allowance instead
of being counted as millions of text tokens; actual provider usage takes precedence.

Stable instructions and tools precede the conversation. Mutable goals, todos,
accepted specs, and handoff notes are supplied at the end of each request, so a
routine tracking update does not invalidate the stable prompt prefix. Provider
JSON and streams are bounded by `max_response_bytes` (64 MB by default).

Protocol references: [Responses streaming and compaction triggers](https://developers.openai.com/api/reference/resources/responses/streaming-events),
[OpenAI compaction](https://developers.openai.com/api/docs/guides/compaction).

## Rich output, Git and GitHub

Both clients render the same parsed Markdown document: headings, emphasis,
links, quotes, task lists, tables, fenced code and diffs. Syntax captures use
Olo's language grammars, cached on a presentation worker. The GUI uses the shared
virtual text editor for selection and copy; the TUI wraps styled graphemes and
caches lines by document and width. Neither parses or highlights on the UI
thread. The agent's `present` tool also emits structured tables, checklists,
progress, charts and code; the original component data remains in the API audit.
Tool results have concise presentations with links to the complete action.
Terminal escape controls and HTML are never executed by the renderer.

The Changes pane shows native Git status, opens highlighted diffs, stages or
unstages selected files, and offers commit, branch and worktree actions.
Git operations use Rust `gix` in process, without invoking Git, hooks, external
filters or credential helpers. Commands include:

```text
/git init
/git status
/git diff {"path":"src/lib.rs","staged":false}
/git stage {"paths":["src/lib.rs"]}
/git unstage {"paths":["src/lib.rs"]}
/git commit {"message":"Fix parser","expected_head":"EXACT_CURRENT_HEAD"}
/git log {"limit":20}
/git branches
/git branch {"branch":"feature/parser"}
/git worktrees
/git worktree_add {"branch":"feature/parser","path":"worktrees/parser"}
```

Read-only inspection runs independently of an active model turn. Mutations are
manual tools with the same approval, confinement and durable effect journal as
agent tools. Commit requires the expected head (empty for an unborn repository)
and uses the repository's configured author. Index writes use Git's lock protocol.
CRLF conversion and LFS clean storage are native; unsupported external filters
fail explicitly. Worktrees must live within the session's workspace, and an
already checked-out branch is rejected. This version does not provide native
fetch/push, branch switching, commit signing, or worktree removal.

GitHub uses Olo's existing GitHub credentials through the native system/keyring
integration and the REST API. Credentials are never supplied to the model.
Read operations include repositories, pulls, issues, comments, changed files,
reviews and commit checks. Write operations include draft PRs, issues, comments,
reviews, PR updates and merges; they all require action approval. Merge requires
the expected head SHA. Writes are never automatically retried. Pages are bounded
and expose `has_next_page`; pass `page` to continue.

```text
/github pulls {"repository":"owner/repo"}
/github checks {"repository":"owner/repo","revision":"COMMIT_SHA"}
/github create_pull {"repository":"owner/repo","body":{"title":"Fix parser","head":"feature/parser","base":"main","body":"Description and validation","draft":true}}
```

Repository menu forms provide the same operations without editing JSON. A draft
PR uses a branch that is already on GitHub. The `repository_read`,
`repository_write`, `github_read` and `github_write` tool schemas document all
arguments. GitHub endpoint scope is fixed to the selected `owner/repository`.

## Automation API

`olo-coder --rpc JSON` runs a command and prints its result. `--rpc '{"op":"schema"}'`
returns the complete JSON Schema; `--rpc '{"op":"tools"}'` returns native tool
schemas. An agent can operate every session workflow without pixels or keyboard
simulation. The native GUI also exposes the shared kit's widget and document tree.

The owner-only `service.sock` in the state directory accepts newline-delimited
JSON. One command per connection:

```json
{"version":1,"id":1,"command":{"op":"create","workspace":"/path/to/repo"}}
{"version":1,"id":1,"result":{"id":"..."}}
```

Errors have `error` instead of `result`. `get` returns session state without its
potentially large provider context; `export` includes that exact native context.
`events` accepts `after` and `limit` (up to 1,000); `search` searches durable events.
Subscriptions register before replaying history, then stream live events:

```json
{"subscribe":true,"session":"SESSION_ID","after":123}
{"seq":124,"session":"SESSION_ID","time":1790000000,"kind":"assistant","data":{}}
{"ready":true,"after":124}
```

Durable notifications are published under the journal's write lock, in commit
order. Concurrent commands cannot advance a client's cursor past an event that
is still waiting to be published. This adds no idle polling or worker.

`seq:0` means a transient streaming update, not a durable replay position. A
`reconnect:true` record asks the client to reopen with the last durable sequence.
Omitted/negative `after` replays recent history; zero replays all events.

`olo-coder --exec PROMPT` is a streaming JSON client suitable for scripts. It stops
printing when its own message has been admitted and the run becomes idle, or
when work fails, pauses, stops or needs approval; an approval
is never implicitly granted. Failure, pause, and pending approval return a nonzero
exit status. A disconnected stream or replay-gap notification also returns a
nonzero status; neither implies completion. Resolve it through the API and
attach again, using the last durable sequence to resume an event stream.
`olo-coder --events ID --after N` follows events; `N` must be nonnegative.
To deliberately stop the service
and checkpoint cancellation, use `--rpc '{"op":"shutdown"}'`.

## Validation

See [the release QA report](CODER-QA.md) for measured results, screenshots,
live-provider coverage and known limits.

`cargo test -p olo-coder-core` covers both native protocols, split UTF-8 SSE,
opaque reasoning round trips, signed compaction, approval/denial, spec gates,
run budgets, recovery, native edits/undo, scoped instructions, ignore rules,
symlink escapes, process cancellation and Linux write confinement.
`cargo test -p olo-coder-core --test live -- --ignored --nocapture` uses the real
pool for a scratch Rust coding task and provider reasoning/compaction replay.
It requires usable subscription quota. Live GUI checks use isolated stores on
Agent Screens or `tsession`, at light/dark/night and fractional scales;
`tools/dev/ui-lint coder` and `tools/dev/bench --apps coder` cover the native window.
