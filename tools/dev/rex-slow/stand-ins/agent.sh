#!/bin/bash
# A stand-in for a coding agent on a test host (never the real one): named
# claude, codex or agy by its link's name, it keeps a session where the real
# one keeps its sessions, says which it is in, takes the real one's resume
# arguments, and draws a prompt whose input line shows what is typed into
# it (bash's readline), writing each line sent to its session.
#   claude [FLAGS] [--resume ID]        ~/.claude/projects/PROJECT/ID.jsonl (PROJECT: the folder, each character but a letter or digit as -)
#   codex [FLAGS] [resume ID]           ~/.codex/sessions/YYYY/MM/DD/rollout-STAMP-ID.jsonl, kept open
#   agy [FLAGS] [--resume ID]           ~/.gemini/antigravity-cli/conversations/ID.db, kept open
# Its arguments are in its session's start line, so a test sees the flags came back.
set -u
me=$(basename "$0")
id=""
# (The resume among the agent's own flags, wherever it is: `claude
# --dangerously-skip-permissions --resume ID`, `codex --yolo resume ID`.)
args=("$@")
for ((i = 0; i < ${#args[@]}; i++)); do
  next=${args[$((i + 1))]:-}
  case "$me:${args[$i]}" in
    claude:--resume | agy:--resume) id=$next ;;
    codex:resume) [ "$next" != --last ] && id=$next ;;
  esac
done
[ -n "$id" ] || id=$(cat /proc/sys/kernel/random/uuid)
case "$me" in
  claude) d="$HOME/.claude/projects/$(pwd | sed 's/[^A-Za-z0-9]/-/g')"; f="$d/$id.jsonl" ;;
  codex) d="$HOME/.codex/sessions/$(date +%Y/%m/%d)"; f=$(ls "$d"/rollout-*-"$id".jsonl 2> /dev/null | head -n 1); [ -n "$f" ] || f="$d/rollout-$(date +%Y-%m-%dT%H-%M-%S)-$id.jsonl" ;;
  agy) d="$HOME/.gemini/antigravity-cli/conversations"; f="$d/$id.db" ;;
  *) echo "a stand-in for claude, codex or agy, not $me" >&2; exit 2 ;;
esac
mkdir -p "$d"
printf '{"type":"start","agent":"%s","session":"%s","cwd":"%s","pid":%s,"args":"%s"}\n' "$me" "$id" "$(pwd)" "$$" "$*" >> "$f"
# (Codex and agy keep theirs open, as the real ones do.)
case "$me" in codex|agy) exec 9>> "$f" ;; esac
echo "stand-in $me, session $id, in $(pwd)"
while IFS= read -r -e -p "$me> " line; do
  printf '{"type":"user","text":"%s"}\n' "$(printf '%s' "$line" | sed 's/\\/\\\\/g; s/"/\\"/g')" >> "$f"
  echo "($me heard: $line)"
done
