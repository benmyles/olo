---
name: olo-worklist
description: Keep the person an up-to-date list of the work in a pane to the right of yours - one short line an item, checked off as you go - with tools/dev/worklist (a Rex split pane that draws the list again the moment its file changes). Use at the start of every session (CLAUDE.md, "A work list beside you"), whenever the person asks for a todo or work list beside you, and at each step after: an item started, done, added or dropped.
---

# A work list beside you

Ben, 2026-11-24: "open a split pane to the right of this and keep in
there for me an up to date todo list of the work (just a brief 1 line
entry for each one and check them off as you go"; then "add a skill for
doing that in the future and a rule asking you to do it by default on
new sessions" (CLAUDE.md, "A work list beside you").

## Open it

At the start of a session, once you know what the work is (the person's
message, `docs/TODO.md`'s open items, what is blocked):

```sh
tools/dev/worklist open
```

It splits a pane to the right of yours (`rex split` beside `$REX_BLOCK`,
in `$REX_SESSION`) running `tools/dev/worklist show` on the list's file
(`/var/tmp/olo-worklist/list.txt`; `worklist path` says it). Write the
list first, or right after: the pane draws whatever the file holds. If
a list is there from an earlier session, bring it up to date rather than
starting over. Not in a Rex pane (no `REX_SESSION`): say so to the
person, keep the file anyway, and tell them its path.

## Keep it

The file, one line an item, short (a few words: what, not how):

```
# Now
[>] olo send: a host's files to the desktop
[ ] Preferences box: pictures and reviews
# Done today
[x] Sync engine
# Blocked
Snap pads (a branch on the laptop)
```

`[>]` the one you are on (one at a time), `[ ]` to do, `[x]` done, `#` a
heading, any other line dim (blocked, waiting on the person). Order:
what you're on and what's next first, done after (or a "Done today"
heading), blocked last.

Update it at each step, not at the end: an item started, finished
(checked, not removed: the person sees the day's work), added (the
person's new asks the moment they come, in their order), dropped or
blocked. Write it whole and rename it into place, so the pane never
draws half a file:

```sh
cat > /var/tmp/olo-worklist/list.new <<'LIST'
…
LIST
mv /var/tmp/olo-worklist/list.new /var/tmp/olo-worklist/list.txt
```

Nothing private in it beyond what the person's own screen shows: no
other people's names, accounts or hosts (it is their pane, but it may
be in a picture).

## How it works

`worklist show` watches the list's folder with inotify and draws the
file again when it is written or renamed into place: nothing is done
while it doesn't change. It hides the cursor; the pane is the person's
to close (its process ends with it). Tried in a Rex pane: opened beside
the agent's, drawn, redrawn as the file was replaced.
