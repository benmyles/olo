# Supervisor and integrations

Both interfaces and the JSON API share these controls. The GUI has an
**Integrations** menu and **Settings → Supervisor**. In the TUI, **Ctrl+O**
opens settings; type to filter menus, then use arrows and Enter to select.
Both composers also accept `/supervisor` and `/integrations` commands.

## Model profiles and settings

**Settings → Model Profiles** manages a service-wide catalog. Give a profile a
name, choose **Codex · Token Pool** or **OpenAI-compatible endpoint**, and fetch
the provider's models. Custom endpoints take an API root and an optional
environment-variable name for their key. Filter the catalog, select a model,
choose reasoning and token limits, then save. **Use Saved Profile** copies the
saved configuration into the primary, supervisor, approval reviewer or
compaction role. Editing or deleting a profile preserves configurations already
selected in sessions. A running session must stop before its model changes.

Changing the endpoint invalidates its fetched model list. Unfinished text,
including invalid numeric input, survives an app upgrade. Profiles and catalogs
contain credential references, never credential values. Endpoints without a
models catalog can still be configured with `/profiles save` or the JSON API;
the normal editor requires discovery so it does not invent model identifiers.

The TUI profile editor uses ↑/↓ to move between fields, ←/→ for choices,
**Enter on Model** for a searchable picker, **F5** to fetch models, **F2** to save
and **Ctrl+U** to clear a text field. Escape closes a picker or editor.
**Ctrl+O → MCP connections** includes local/remote MCP setup, Slack, Logfire
and Passwords field selection. These forms use **F2** to continue.

**Settings → Session Settings** provides typed forms for budgets, timeouts,
compaction, subagent limits, safety/collaboration, supervisor limits and
instructions. Booleans and fixed choices use selectors. Invalid input stays
in the form with its error. Updates merge just the edited fields into the
latest stored configuration, preserving unrelated edits from another window.
The same sections are in the TUI settings menu. Advanced hooks, tool-policy
lists and provider-specific JSON remain available through `/config` and the
schema-documented API.

```text
/profiles
/profiles save {"profile":{"id":"NEW_UUID","name":"Fast endpoint","model":{"provider":"openai","model":"provider/model-id","endpoint":{"base_url":"https://provider.example/v1","api_key_env":"MODEL_API_KEY"}}}}
/profiles apply {"id":"PROFILE_UUID","role":"supervisor"}
```

API operation `model_profiles` supports `list`, `discover`, `save`, `remove`
and `apply`; roles are `primary`, `supervisor`, `reviewer` and `compaction`.
`configure_fields` takes a map of dotted setting paths to typed JSON values.
`subscribe_sessions` publishes profile changes alongside session catalogs.

## Supervisor

Off by default. `/supervisor on` enables it; `/supervisor off` also invalidates
an outstanding review. **Supervisor Guidance & Limits** edits instructions,
model profile, reasoning, cadence, intervention count and token budget.
**Inherit primary** follows the primary model. Profiles include their provider
and endpoint, so choosing a custom model also selects its connection.

| Setting | Default |
| --- | --- |
| Periodic review | Every 8 primary turns |
| Reviews per explicit run | 20 |
| Interventions per explicit run | 3 |
| Separate input + output token budget | 40,000 |
| Stalled model request timeout | 300 seconds |
| Recover model errors | Enabled |

Reviews run at safe tool boundaries, when the primary finishes an answer, and
after recoverable failures. Stalled model requests are cancelled before review.
The supervisor reads the original task, recent context, goal, spec, todos,
policy and durable supervisory notes. Read-only tools retrieve original
transcript records, search history, inspect compaction checkpoints and read
result artifacts. Decisions are **observe**, **steer** or **pause**. Advice is
attributed in both interfaces and retained in the original transcript.

It cannot approve tools, write files, change policy, grant secrets, declare
completion or override Stop. It does not resume uncertain in-flight effects,
approvals, intentional pauses or exhausted primary budgets. Limits survive
automatic recovery; explicit new runs reset counters while retaining notes.
Child agents inherit neither enabled supervision nor password grants.

Each review has at most four model turns, 4,000 output tokens per request and a
120-second request timeout. Input estimates reserve room in the remaining
budget; actual usage is tracked separately. Context, tool reads, decisions and
usage are journalled. Supervisors can see previously shared values in the
transcript: choose their provider accordingly.

There is no idle polling or perpetual model loop. An existing idle session
starts supervision on its next explicit run. Long tools retain their own
cancellation/timeouts; reviews wait for a safe boundary.

```text
/supervisor {"enabled":true,"guidance":"Check the accepted spec and test evidence. Steer toward unfinished criteria. Pause when a decision needs me.","every_turns":6,"max_interventions":4}
```

Omitted fields use defaults. Inspect `/config` before replacing an object.

## MCP

Native stdio and Streamable HTTP support tool discovery/calls, resource
listing/templates/reads and prompt listing/retrieval. Tool effects follow the
ordinary approval policy; server read-only annotations do not grant permission.
Server-initiated sampling and elicitation are not advertised or executed.
Legacy SSE-only servers need a Streamable HTTP endpoint; no bridge is installed.

**Add MCP Server** accepts an executable or URL, JSON arguments, and none,
environment-token, Passwords-reference or OAuth authentication.
**Manage Connection** signs in, tests, inspects, signs out or removes a server.

```text
/integrations configure {"server":{"name":"local","program":"/path/to/server","args":["--stdio"],"env":{"API_TOKEN":"PROJECT_API_TOKEN"}}}
/integrations configure {"server":{"name":"remote","url":"https://example.org/mcp","auth":{"kind":"bearer_env","variable":"PROJECT_MCP_TOKEN"}}}
/integrations configure {"server":{"name":"oauth_server","url":"https://example.org/mcp","auth":{"kind":"oauth","scopes":["read"]}}}
/integrations login {"name":"oauth_server"}
/integrations test {"name":"remote"}
/integrations request {"name":"remote","method":"resources/list","params":{}}
/tool mcp_read {"server":"remote","method":"prompts/get","params":{"name":"review"}}
/integrations logout {"name":"oauth_server"}
/integrations remove {"name":"remote"}
```

Environment maps contain variable names, never secret values. For stdio, each
entry maps a target variable to an inherited source variable. Remote
`header_env` supports other authentication schemes; do not combine an
Authorization header with `auth`. `optional:true` servers report discovery or
connection failure and let work continue; required servers stop the run.

OAuth uses protected-resource and authorization-server discovery, S256 PKCE,
random state, issuer checks and loopback callbacks. Dynamic registration and
pre-registered public/confidential clients are supported. Registered clients
configure `client_id`, `client_secret_env`, `scopes` and optional fixed
`callback_port`. Resource identifiers may name the endpoint or an ancestor path
on the same origin. Sign-in opens through Olo's URL handler and also supplies a
clickable link. Completion arrives as an event, without polling.
`/integrations status` lists flows;
`/integrations cancel_login {"flow":"FLOW_ID"}` cancels one.

Tokens and registered client secrets are sealed using XChaCha20-Poly1305 in the
service store; the encryption key lives in Olo Passwords. Unlock Passwords to
save/restore sign-ins. Refresh rotation is serialized and saved durably.
Long-running MCP connections get a fresh token before the next call. Tokens do
not enter session configuration or authentication audit events. Credentials
never follow redirects; interrupted effectful requests are never replayed.

Sign-out deletes local credentials and cancels pending sign-ins for that
connection identity. Provider account settings control remote revocation.
Sessions in one service with identical URL/auth configuration share a sign-in.
Removing a connection preserves its sign-in for other sessions.

## Slack and Logfire

**Connect Logfire (US/EU)** installs the official endpoint and starts browser
sign-in in one action. Default scope is `project:read`; add scopes explicitly
for administrative tools. Token auth is also available through the custom
connection form, for example using `LOGFIRE_MCP_TOKEN`.

```text
/integrations setup {"preset":"logfire"}
/integrations setup {"preset":"logfire-eu"}
```

**Add Slack** installs the official endpoint and starts sign-in after receiving
your registered app's client ID and secret environment-variable name. Register
`http://127.0.0.1:8787/callback` as that app's redirect URI. Slack does not offer
dynamic client registration; Coder cannot create or approve your Slack app.
The preset requests Slack's advertised scopes; customize `auth.scopes` for
narrower access. Sending messages still follows ordinary action approvals.

```text
/integrations setup_slack {"client_id":"REGISTERED_CLIENT","client_secret_env":"SLACK_CLIENT_SECRET"}
```

`/integrations install {"preset":"slack"}` installs without signing in.
Presets: `slack`, `logfire`, `logfire-us`, `logfire-eu`, `terminal`.

Provider references checked 2026-11-10:
[Slack MCP](https://docs.slack.dev/ai/slack-mcp-server/),
[Slack user token exchange](https://docs.slack.dev/reference/methods/oauth.v2.user.access/),
[Logfire MCP](https://pydantic.dev/docs/logfire/guides/mcp-server/),
[MCP authorization](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization).
Slack's documented `user` token type is accepted only for its official issuer
and token endpoint. Tokens without expiry are not assigned an invented expiry.

## Native Olo

**Terminal titles** are enabled by default. `set_terminal_title` talks to Olo
Terminal's native MCP on loopback port 17893 (`OLO_TERMINAL_MCP_PORT` overrides
it). Terminal's agent-title preference still applies. No terminal is launched
and no global settings are changed.

**Agent Screens** are opt-in. Each session owns a private screen named
`coder-SESSION_ID_WITHOUT_DASHES`. The native library shares Agent Screens'
accessibility tree, input, lifecycle, history and person-takeover checks in
process, without wrapping its CLI.

```text
/set olo.screens true
/tool screen_control {"operation":"start","args":["--size","1280x800"]}
/tool screen_control {"operation":"run","args":["olo-styled-edit"]}
/tool screen_read {"operation":"tree"}
/tool screen_capture {}
/tool screen_control {"operation":"stop"}
```

`screen_read` provides windows, trees, menus and review. `screen_control`
provides native input, window operations and menu invocation. Arguments follow
Agent Screens syntax; the target screen is fixed by Coder. `screen_capture`
writes an artifact and attaches its PNG after the complete tool-result batch,
keeping provider ordering valid. Optional `window` selects one numeric window
ID. It cannot write a caller-selected path or capture the person's desktop.
Input stops during person takeover. A cancelled native operation may have
finished atomically; inspect before retrying.

**Passwords** uses explicit field grants. **Share Password Field** gives an
agent-facing alias to one `op://vault/item/field` reference. Passwords still
controls vault access; grants do not unlock vaults or authorize other fields.
An agent cannot grant itself access.

```text
/integrations password_search {"query":"deployment"}
/integrations grant_secret {"name":"deployment_token","reference":"op://Work/Deploy/token"}
/tool read_granted_secret {"name":"deployment_token"}
/integrations revoke_secret {"name":"deployment_token"}
```

Search returns metadata, not secret values. Reading a grant sends its value to
the model and retains it in the transcript. Revocation prevents future reads;
it cannot retract delivered values. MCP auth can instead use
`{"kind":"password","reference":"op://Work/Service/token"}`: its value becomes
an authentication header without entering model context.

The Passwords picker searches item metadata, then asks for field labels and
stable references through `CliQuery::Fields`. It never requests item values or
generates OTPs during selection. Granting a field does not read it; the agent's
later `read_granted_secret` call does. Passwords retains its unlock and terminal
permission checks. Field choices are also available to automation through
`integrations/password_fields` with `vault` and `item` IDs.
