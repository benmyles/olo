#!/bin/sh
# olo COMMAND [ARGUMENT…]: Olo's command on a Rex host.
#
# olo agent …: the children of this tab (agents, and commands of your
# own, each a Rex tab under it). This host's own: asked of the agents
# service here (Olo's helper, `olo-rex-link agent`), which starts and
# keeps them through this host's Rex server. No Olo desktop is asked or
# need be connected.
#
# olo screen …: a screen of your own (Olo Agent Screens: a headless Olo
# desktop). On this host itself, drawn here, with no desktop asked:
# `olo screen --on host …`, once Olo is put here (`olo screen install`;
# `olo screen status` says what is). Or on the desktop this host is used
# from, asked through Rex: `olo screen --on desktop …`. Not said, it is
# OLO_SCREENS_ON's, else this host's default (`olo screen default
# [host|desktop]`), else the desktop's.
#
# The rest are commands of the Olo desktop this host is used from, asked
# through Rex (olo.lua beside this file, run by `rex do`): olo open URL,
# olo get rex://local/PATH [DEST], olo ls rex://local/PATH, olo op …,
# olo screen …, olo forward …, olo send PATH…, olo sync …, olo run
# [--cwd DIR] -- COMMAND…, olo notify TITLE [BODY], olo clipboard TEXT,
# olo paste. Their status is 69 where the desktop couldn't be asked.
#
# Part of Olo's skill for agents, and of the desktop commands (xdg-open
# and the rest, beside it).
self=$0
if command -v readlink >/dev/null 2>&1; then
	resolved=$(readlink -f "$self" 2>/dev/null) && [ -n "$resolved" ] && self=$resolved
fi
here=$(dirname "$self")
OLO_BRIDGE_CMD=${1:-}
[ $# -gt 0 ] && shift
# `olo agent`: the helper beside this file (the desktop's own computer),
# else the newest build of it the app has put in this account for this
# machine. (OLO_REX_LINK: another, for Olo's own tests.) Only one that
# has the agents service is used (`agent --protocol`); where none is
# here yet the desktop is asked below, which puts the helper here and
# asks its service: from then on this goes straight to it.
helper() {
	link=${OLO_REX_LINK:-}
	if [ -z "$link" ] && [ -x "$here/olo-rex-link" ]; then
		link="$here/olo-rex-link"
	fi
	if [ -z "$link" ]; then
		link=$(ls -t "$HOME"/.local/share/olo/rex-link/olo-rex-link-*-"$(uname -m)" 2>/dev/null | head -n 1)
	fi
	[ -n "$link" ] && [ -x "$link" ]
}
if [ "$OLO_BRIDGE_CMD" = agent ]; then
	if helper && "$link" agent --protocol >/dev/null 2>&1; then
		exec "$link" agent "$@"
	fi
fi
# `olo screen`: this host's own (the helper's `screen`: Olo put here and
# its olo-screen run), or the desktop's, below with the rest. What puts
# Olo here and says what is here is the host's own whichever is asked.
OLO_SCREEN_ON=
if [ "$OLO_BRIDGE_CMD" = screen ]; then
	on=${OLO_SCREENS_ON:-}
	if [ "${1:-}" = --on ]; then
		if [ $# -lt 2 ]; then
			echo "olo screen: --on takes host or desktop" >&2
			exit 2
		fi
		on=$2
		shift 2
	fi
	case "${1:-}" in install | status | default | remove) on=host ;; esac
	if [ -z "$on" ]; then
		conf="${XDG_CONFIG_HOME:-$HOME/.config}/olo/screens-on"
		if [ -r "$conf" ]; then read -r on < "$conf" || true; fi
	fi
	case "$on" in
	host)
		# (Only a helper that has `screen`: one from before takes the
		# word for a program to run in front of the Rex server, and a
		# host may well have a program called screen. Known by what it
		# says of itself, not by whether the asking succeeded.)
		if helper; then
			case "$("$link" screen --help 2>/dev/null)" in
			"usage: olo screen"*) exec "$link" screen "$@" ;;
			esac
		fi
		echo "olo screen: this host's helper has no screens of its own yet (it comes with Olo's next upgrade of this host: connect from an upgraded Olo desktop); \`olo screen --on desktop …\` asks the desktop's" >&2
		exit 69
		;;
	desktop | "") OLO_SCREEN_ON=desktop ;;
	*)
		echo "olo screen: --on takes host or desktop, not \`$on\`" >&2
		exit 2
		;;
	esac
fi
i=0
for a in "$@"; do i=$((i + 1)); export "OLO_BRIDGE_ARG_$i=$a"; done
OLO_BRIDGE_ARGC=$i
uid=$(id -u 2>/dev/null || echo 0)
OLO_BRIDGE_DIR="${XDG_RUNTIME_DIR:-${TMPDIR:-/var/tmp}}/olo-rex-bridge-${uid}"
export OLO_BRIDGE_CMD OLO_BRIDGE_ARGC OLO_BRIDGE_DIR
if [ -L "$OLO_BRIDGE_DIR" ]; then
	echo "olo: refusing symlink bridge dir $OLO_BRIDGE_DIR" >&2
	exit 1
fi
mkdir -p -m 0700 "$OLO_BRIDGE_DIR" 2> /dev/null || true
chmod 0700 "$OLO_BRIDGE_DIR" 2> /dev/null || true
if [ ! -d "$OLO_BRIDGE_DIR" ] || [ ! -O "$OLO_BRIDGE_DIR" ]; then
	echo "olo: bridge dir $OLO_BRIDGE_DIR is not owned by uid $uid" >&2
	exit 1
fi
# (And this account's alone: a folder found there open to a group stays
# so where it can't be closed, and nothing is kept in it then.)
case $(ls -ld "$OLO_BRIDGE_DIR" 2> /dev/null) in
drwx------*) ;;
*)
	echo "olo: bridge dir $OLO_BRIDGE_DIR is open to others and couldn't be closed to them" >&2
	exit 1
	;;
esac
# Whether this `op` reads its standard input (`takes`), told as `op`
# itself reads its arguments (apps/op/src/main.rs, `Args::parse`: an
# option that is no switch takes the next argument as its value): a `-`
# among its words (`op item create -`), `inject` with no `--in-file`, or
# `item edit`. By each argument, never by the line joined: a title with
# " - " in it ("Production - DB") is not a `-`, and an option before the
# command (`op --account work inject`) doesn't hide it. (Kept the same
# in `olo` and in the stand-in for `op`.)
op_takes_input() {
	takes=no
	op_w1=
	op_w2=
	op_dash=no
	op_in=no
	op_skip=no
	for op_a in "$@"; do
		if [ "$op_skip" = yes ]; then
			op_skip=no
			continue
		fi
		case $op_a in
		--) break ;;
		-) op_dash=yes ;;
		-i | --in-file)
			op_in=yes
			op_skip=yes
			;;
		-i=* | --in-file=*) op_in=yes ;;
		-*=*) ;;
		--reveal | --otp | --force | -f | --no-newline | -n | --no-masking | --raw | --help | -h | --version | -v | --debug | --no-color | --iso-timestamps | --cache | --include-archive | --all | --favorite | --long | --dry-run | --archive | --generate-password | --ssh-generate-key) ;;
		-*) op_skip=yes ;;
		*)
			if [ -z "$op_w1" ]; then
				op_w1=$op_a
			elif [ -z "$op_w2" ]; then
				op_w2=$op_a
			fi
			;;
		esac
	done
	[ "$op_dash" = yes ] && takes=yes
	[ "$op_w1" = inject ] && [ "$op_in" = no ] && takes=yes
	[ "$op_w1 $op_w2" = "item edit" ] && [ -f /dev/stdin ] && takes=yes
	return 0
}
# op's standard input, copied for olo.lua to send where the command
# takes it and it is a pipe or a file. At most 1 MiB. The copy has no
# name from the moment it is made (opened, then unlinked, and read again
# through its descriptor): nothing of a secret is left on disk whatever
# ends this, a kill that no trap sees included, and nothing of it can
# be another run's. (Its name was passed about, and removed by a trap,
# until a code review, 2026-11-27.)
unset OLO_BRIDGE_STDIN OLO_BRIDGE_STDIN_KEPT
takes=no
[ "$OLO_BRIDGE_CMD" = op ] && op_takes_input "$@"
if [ "$takes" = yes ] && [ -t 0 ]; then
	echo "olo op: this reads its input, and the Olo desktop can't read this terminal: give it a pipe or a file (\`olo op ${1:-inject} … < FILE\`)" >&2
	exit 2
fi
if [ "$takes" = yes ] && { [ -p /dev/stdin ] || [ -f /dev/stdin ]; }; then
	olo_copy=$(umask 077 && mktemp "$OLO_BRIDGE_DIR/stdin.XXXXXX") || exit 1
	if ! exec 3<> "$olo_copy" || ! rm -f "$olo_copy"; then
		rm -f "$olo_copy"
		echo "olo: op's standard input couldn't be kept to send to the Olo desktop ($OLO_BRIDGE_DIR)" >&2
		exit 1
	fi
	if ! head -c 1048577 >&3; then
		echo "olo: op's standard input couldn't be kept to send to the Olo desktop ($OLO_BRIDGE_DIR)" >&2
		exit 1
	fi
	if [ "$(wc -c < /dev/fd/3)" -gt 1048576 ]; then
		echo "olo: op's standard input is more than 1 MiB: too much to send to the Olo desktop" >&2
		exit 1
	fi
	# (Said to olo.lua: nothing there for it to remove. An olo.lua beside
	# an `olo` from before removes the copy itself, as it did.)
	OLO_BRIDGE_STDIN=/dev/fd/3
	OLO_BRIDGE_STDIN_KEPT=1
	export OLO_BRIDGE_STDIN OLO_BRIDGE_STDIN_KEPT
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
# `olo agent watch`: event-driven wait up to 3600s; give outer rex timeout a 60s margin so inner timeout JSON always flushes.
if [ "$OLO_BRIDGE_CMD" = agent ] && { [ "${1:-}" = watch ] || [ "${1:-}" = wait ]; }; then
	exec ${OLO_BRIDGE_REX:-rex} --timeout 3660s do "$here/olo.lua"
fi
# `olo agent stop`: allow up to 300s so `--save` with large scrollback completes cleanly.
if [ "$OLO_BRIDGE_CMD" = agent ] && { [ "${1:-}" = stop ] || [ "${1:-}" = close ] || [ "${1:-}" = kill ]; }; then
	exec ${OLO_BRIDGE_REX:-rex} --timeout 300s do "$here/olo.lua"
fi
# A screen on the desktop: where the desktop can't be asked, said with
# the other way.
if [ "$OLO_SCREEN_ON" = desktop ]; then
	${OLO_BRIDGE_REX:-rex} --timeout 60s do "$here/olo.lua"
	rc=$?
	if [ "$rc" = 69 ]; then
		echo "olo screen: a screen on this host itself needs no desktop: \`olo screen --on host …\` (\`olo screen status\` says whether Olo is here; \`olo screen install\` puts it)" >&2
	fi
	exit "$rc"
fi
# (OLO_BRIDGE_REX: another way to say `rex`, for Olo's own tests.)
exec ${OLO_BRIDGE_REX:-rex} --timeout 60s do "$here/olo.lua"
