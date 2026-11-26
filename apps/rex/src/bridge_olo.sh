#!/bin/sh
# olo COMMAND [ARGUMENT…]: a command of the Olo desktop this Rex host is
# used from, asked through Rex (olo.lua beside this file, run by
# `rex do`): olo open URL, olo get rex://local/PATH [DEST], olo ls
# rex://local/PATH, olo op …, olo screen …, olo forward …, olo send PATH…,
# olo sync …, olo run [--cwd DIR] -- COMMAND…, olo notify TITLE [BODY],
# olo clipboard TEXT, olo paste.
# Part of Olo's skill for agents, and of the desktop commands (xdg-open
# and the rest, beside it). Its status is 69 where the desktop couldn't
# be asked.
self=$0
if command -v readlink >/dev/null 2>&1; then
	resolved=$(readlink -f "$self" 2>/dev/null) && [ -n "$resolved" ] && self=$resolved
fi
here=$(dirname "$self")
OLO_BRIDGE_CMD=${1:-}
[ $# -gt 0 ] && shift
i=0
for a in "$@"; do i=$((i + 1)); export "OLO_BRIDGE_ARG_$i=$a"; done
OLO_BRIDGE_ARGC=$i
OLO_BRIDGE_DIR="${TMPDIR:-/var/tmp}/olo-rex-bridge"
export OLO_BRIDGE_CMD OLO_BRIDGE_ARGC OLO_BRIDGE_DIR
mkdir -p "$OLO_BRIDGE_DIR" 2> /dev/null || true
# op's standard input, kept for olo.lua to send where the command takes
# it (`-`: `op item create -`, `op item delete -`; `op inject` without
# -i; a file given to `op item edit`, never a pipe: an agent's shell may
# leave one open, and it would be waited on for ever) and it is a pipe
# or a file, never a terminal. At most 1 MiB.
takes=no
if [ "$OLO_BRIDGE_CMD" = op ]; then
	case " $* " in *" - "*) takes=yes ;; esac
	if [ "${1:-}" = inject ]; then
		case " $* " in *" -i "* | *" --in-file "* | *" -i="* | *" --in-file="*) ;; *) takes=yes ;; esac
	fi
	[ "${1:-} ${2:-}" = "item edit" ] && [ -f /dev/stdin ] && takes=yes
fi
if [ "$takes" = yes ] && [ ! -t 0 ] && { [ -p /dev/stdin ] || [ -f /dev/stdin ]; }; then
	OLO_BRIDGE_STDIN=$(umask 077 && mktemp "$OLO_BRIDGE_DIR/stdin.XXXXXX") || exit 1
	head -c 1048577 > "$OLO_BRIDGE_STDIN"
	if [ "$(wc -c < "$OLO_BRIDGE_STDIN")" -gt 1048576 ]; then
		rm -f "$OLO_BRIDGE_STDIN"
		echo "olo: op's standard input is more than 1 MiB: too much to send to the Olo desktop" >&2
		exit 1
	fi
	export OLO_BRIDGE_STDIN
fi
# `olo run`: give the run a known id and trap INT/TERM/HUP so ^C cancels
# the desktop command (or dismisses its approval alert) immediately.
if [ "$OLO_BRIDGE_CMD" = run ]; then
	OLO_RUN_ID="run-$$-$(date +%s 2>/dev/null || echo 0)"
	export OLO_RUN_ID
	cancel_run() {
		trap - INT TERM HUP
		[ -n "${child_pid:-}" ] && kill "$child_pid" 2>/dev/null || true
		OLO_BRIDGE_CMD=run_cancel OLO_BRIDGE_ARGC=0 ${OLO_BRIDGE_REX:-rex} --timeout 5s do "$here/olo.lua" >/dev/null 2>&1 || true
		exit 130
	}
	trap cancel_run INT TERM HUP
	${OLO_BRIDGE_REX:-rex} --timeout 3600s do "$here/olo.lua" &
	child_pid=$!
	wait "$child_pid"
	rc=$?
	trap - INT TERM HUP
	exit "$rc"
fi
# `olo agent watch`: event-driven wait that may span minutes; allow up to 3600s.
if [ "$OLO_BRIDGE_CMD" = agent ] && { [ "${1:-}" = watch ] || [ "${1:-}" = wait ]; }; then
	exec ${OLO_BRIDGE_REX:-rex} --timeout 3600s do "$here/olo.lua"
fi
# (OLO_BRIDGE_REX: another way to say `rex`, for Olo's own tests.)
exec ${OLO_BRIDGE_REX:-rex} --timeout 60s do "$here/olo.lua"
