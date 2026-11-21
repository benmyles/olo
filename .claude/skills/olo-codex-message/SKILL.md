---
name: olo-codex-message
description: Send a message to a Codex agent that is running on this computer (one of the person's own Codex sessions, in another terminal window or tab) - hand it to the turn it is working on right now, or queue it for when that turn ends - and check that it arrived, with tools/dev/codex-say. Use when the person asks you to tell, ask or report something to a Codex agent of theirs (they give its session id), for example findings about a tool it is building. Not for the Codex subagents you start yourself (olo-codex-agents, tools/dev/agents say) nor for another Claude session (the SendMessage tool).
---

# A message for a Codex agent of the person's

The person runs Codex agents of their own, in other windows, on other
projects. When they ask you to tell one of them something ("tell the
agent working on X what you found"), this is how it gets there, and how
you know it did (Ben, 2026-11-21: "if you have a reply for the codex
agent can you force send it now instead of queuing it?", then "add a
skill for sending messages to codex agents with the mechanism so you
know how to do it next time").

## When, and whose

- Only a session the person asked you to write to, by the id they gave
  (`/status` in its window shows it). Never one you found by listing:
  the others are their other work, and nothing of theirs is yours to
  read or steer.
- What you send goes to that agent's provider like anything else it
  reads: nothing private in it (no names, mail, accounts, window
  titles, nothing from the person's logs), as for a brief.
- It lands in the middle of the agent's work. One message that stands
  on its own, not a stream.

## How

```sh
tools/dev/codex-say SESSION look              # working or at rest; what waits in its queue
tools/dev/codex-say SESSION now  MESSAGE.txt  # into the turn it is working on, at its next step
tools/dev/codex-say SESSION later MESSAGE.txt # queued: taken when that turn ends
tools/dev/codex-say SESSION arrived A few words of it   # in its record? (exit 1: not there)
tools/dev/codex-say SESSION drop ID…          # queued ones taken out again
```

- **`now` is what "tell it" means.** It is what typing into the
  agent's window while it works does: the agent reads it at its next
  step and goes on. At rest, it starts a turn with it. It says whether
  the message is in the session's record a few seconds on.
- **`later` waits for the turn to end, however long that is.** The
  first message sent this way sat in the queue for eighty minutes
  behind one turn, and the person asked whether anything had been sent
  at all. Use it for what can wait until the agent has finished what
  it is doing.
- **Check, don't assume**: `arrived` with a few exact words of your
  message (it counts them in the session's record and prints nothing
  of the record). "Queued" is not "received".
- After a `now`, take out what you had queued before that says the
  same (`look` lists them, `drop` removes them), so the agent isn't
  told twice.

## The message

Write it to a file under your scratch folder in `/var/tmp` and send
the file. It should say, in this order:

1. Who is writing and why ("From the agent working in the Olo
   repository; its owner asked me to tell you …"), and that it needn't
   stop what it is doing.
2. The finding, with the figures and the exact commands, short enough
   to act on without opening anything.
3. Where the rest is: a report file under `/var/tmp` (the agent can
   read there) with the commands, counts and logs.
4. How to answer: the agent can't write back to you. Name a file for
   it to append to (`/var/tmp/NAME/replies.md`), and wait on that file
   in the background with a deadline (CLAUDE.md, "Waits that can't
   hang"); look again when it changes.

Then tell the person what was sent and that it arrived.

## How it works (codex-cli 0.170)

Every `codex` window on the computer is a client of one app server,
the daemon the first of them started, which runs the sessions
(`ps`: `… app-server --listen unix://`). Its control socket,
`~/.codex/app-server-control/app-server-control.sock`, speaks
WebSocket with JSON-RPC inside:

- `initialize` (`clientInfo`, and `capabilities.experimentalApi: true`
  for the queue's methods), then the `initialized` notification;
- `thread/read {threadId}`: `status.type` is `active`, `idle` or
  `notLoaded` (no window has it open);
- `thread/turns/list {threadId, limit, sortDirection: "desc",
  itemsView: "notLoaded"}`: the turn whose `status` is `inProgress`,
  without fetching what is in it;
- `turn/steer {threadId, expectedTurnId, input: [{type: "text",
  text}]}`: more input for the running turn;
- `turn/start {threadId, input}`: a new turn, when it is at rest;
- `thread/queue/list`, `thread/queue/delete {threadId,
  queuedSubmissionId}`: what waits, and taking it out.

`codex queue --thread ID --message TEXT` is the documented command and
is `later`: it writes `~/.codex/queue_1.sqlite`, and the app server
takes from it when the session's turn ends. `codex app-server proxy`
is the control socket's bytes on stdio (WebSocket too, not lines of
JSON: a client that writes JSON lines to it gets a broken pipe).
`codex app-server generate-json-schema --out DIR --experimental`
writes the whole protocol down, which is where to look when a newer
codex-cli answers differently: fix the tool, don't work round it.

The session's record is `~/.codex/sessions/YEAR/MONTH/DAY/rollout-…-ID.jsonl`.
It is the person's agent's conversation: count your own words in it
(`arrived`), don't read it.
