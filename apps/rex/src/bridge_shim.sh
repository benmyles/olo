#!/bin/sh
# A desktop command on this host done on the Olo desktop it is used from,
# through Rex: xdg-open (and $BROWSER), notify-send, wl-copy, wl-paste,
# xclip, xsel, op and olo-screen, under each of those names beside `olo` in this
# folder. Only in a Rex session (REX_SESSION) shown in Olo's Rex, and only
# what the host may do there (Preferences ▸ Rex); otherwise, or where the
# desktop can't be asked, the host's own command of the name, as if this
# weren't here. Put here by Olo's Rex and taken off from its Host window
# ("Desktop commands").
me=${0##*/}
here=${0%/*}

# The host's own command of this name: the first on PATH past this folder.
real() {
	old_ifs=$IFS
	IFS=:
	for d in $PATH; do
		[ "$d" = "$here" ] && continue
		if [ -f "$d/$me" ] && [ -x "$d/$me" ]; then
			IFS=$old_ifs
			exec "$d/$me" "$@"
		fi
	done
	IFS=$old_ifs
	echo "$me: not on this host, and not done by the Olo desktop either" >&2
	exit 127
}

# Asked of the desktop: its answer and status are the command's, unless
# it couldn't be asked (69: no Olo desktop connected, the Rex server not
# answering, or not something this host may do), when the caller goes on
# to the host's own.
asked() {
	"$here/olo" "$@"
	status=$?
	[ "$status" -eq 69 ] && return 69
	exit "$status"
}

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

[ -n "${REX_SESSION:-}" ] || real "$@"

case $me in
xdg-open | x-www-browser | sensible-browser | www-browser)
	[ $# -eq 1 ] || real "$@"
	target=$1
	case $target in
	*:*) ;;
	*)
		# A file of this host's, by its whole path: the desktop copies
		# it and opens it in its app.
		if [ -e "$target" ]; then
			dir=$(cd -- "$(dirname -- "$target")" && pwd -P) && target=$dir/$(basename -- "$target")
		fi
		;;
	esac
	asked open "$target"
	real "$@"
	;;
notify-send)
	# Its title and body; its options (urgency, icon, how long) are the
	# desktop's own to choose.
	summary= body= n=0 skip=0
	for a in "$@"; do
		if [ "$skip" -eq 1 ]; then
			skip=0
			continue
		fi
		case $a in
		-u | -t | -i | -a | -c | -h | -r | -A | --urgency | --expire-time | --icon | --app-name | --category | --hint | --replace-id | --action) skip=1 ;;
		-*) ;;
		*)
			n=$((n + 1))
			if [ "$n" -eq 1 ]; then summary=$a; elif [ "$n" -eq 2 ]; then body=$a; fi
			;;
		esac
	done
	[ -n "$summary" ] || real "$@"
	asked notify "$summary" "$body"
	real "$@"
	;;
wl-copy | xclip | xsel)
	# Copying or pasting? wl-copy copies; xclip pastes with -o; xsel
	# pastes with -o, or when nothing comes on its input.
	paste=0 trim=0 skip=0 have=0 text=
	for a in "$@"; do
		if [ "$skip" -eq 1 ]; then
			skip=0
			continue
		fi
		case $me:$a in
		wl-copy:-n | wl-copy:--trim-newline) trim=1 ;;
		wl-copy:-t | wl-copy:--type | wl-copy:-s | wl-copy:--seat | xclip:-selection | xclip:-sel | xclip:-se | xclip:-t | xclip:-target | xclip:-d | xclip:-display | xclip:-l | xclip:-loops) skip=1 ;;
		wl-copy:--clear | wl-copy:-c) real "$@" ;;
		xclip:-o | xclip:-out | xsel:-o | xsel:--output) paste=1 ;;
		xsel:-*o*) case $a in --*) ;; *) paste=1 ;; esac ;;
		*:-*) ;;
		wl-copy:*)
			have=1
			text="${text:+$text }$a"
			;;
		# (Files to copy from: the host's own does that.)
		xclip:*) real "$@" ;;
		*) ;;
		esac
	done
	if [ "$me" = xsel ] && [ "$paste" -eq 0 ]; then
		case " $* " in
		*" -i "* | *" --input "* | *" -a "* | *" --append "*) ;;
		*) [ -t 0 ] && paste=1 ;;
		esac
	fi
	if [ "$paste" -eq 1 ]; then
		asked paste
		real "$@"
	fi
	if [ "$have" -eq 0 ]; then
		# (What comes in, whole: its last newlines kept.)
		text=$(
			cat
			printf x
		)
		text=${text%x}
	fi
	if [ "$trim" -eq 1 ]; then
		text=${text%"
"}
	fi
	"$here/olo" clipboard "$text"
	status=$?
	[ "$status" -eq 69 ] || exit "$status"
	# (The host's own copier, given what came in.)
	printf '%s' "$text" | real "$@"
	;;
wl-paste)
	newline=1
	for a in "$@"; do
		case $a in
		-n | --no-newline) newline=0 ;;
		-l | --list-types | -w | --watch) real "$@" ;;
		esac
	done
	text=$(
		"$here/olo" paste
		s=$?
		printf x
		exit $s
	)
	status=$?
	if [ "$status" -eq 0 ]; then
		text=${text%x}
		printf '%s' "$text"
		if [ "$newline" -eq 1 ]; then
			case $text in *"
") ;; *) echo ;; esac
		fi
		exit 0
	fi
	[ "$status" -eq 69 ] || exit "$status"
	real "$@"
	;;
op)
	# What op is given on its input, where it reads any, is kept here
	# first, so that whichever op takes it, the desktop's or, where the
	# desktop can't be asked or may not do it (69), the host's own, gets
	# it whole: `olo` reads it to send it, and the host's own op then
	# found it read and gone (a piped `op inject` got an empty template).
	# Kept in a file with no name: made in the bridge's folder, which is
	# this account's alone, opened and unlinked before a byte is written,
	# and read again through its descriptor. Nothing of it is on disk
	# under a name at any moment, whatever ends this; none of it is ever
	# another run's; and the host's own op gets it as its input and no
	# other descriptor. (Two code reviews, 2026-11-27: it was a file
	# named by this shell's pid, then one removed by a trap.)
	op_takes_input "$@"
	if [ "$takes" = yes ] && [ -t 0 ]; then
		# (Typed at a terminal: the host's own op reads it there. The
		# desktop has none of this terminal to read, and was sent an
		# empty input.)
		real "$@"
	fi
	if [ "$takes" = yes ] && { [ -p /dev/stdin ] || [ -f /dev/stdin ]; }; then
		keep="${XDG_RUNTIME_DIR:-${TMPDIR:-/var/tmp}}/olo-rex-bridge-$(id -u 2> /dev/null || echo 0)"
		if [ ! -L "$keep" ]; then
			mkdir -p -m 0700 "$keep" 2> /dev/null || true
			chmod 0700 "$keep" 2> /dev/null || true
		fi
		case $(ls -ld "$keep" 2> /dev/null) in
		drwx------*) [ -O "$keep" ] || keep= ;;
		*) keep= ;;
		esac
		if [ -z "$keep" ]; then
			echo "$me: no folder of this account's own to keep its input in (${XDG_RUNTIME_DIR:-${TMPDIR:-/var/tmp}}/olo-rex-bridge-…)" >&2
			exit 1
		fi
		kept=$(umask 077 && mktemp "$keep/op-input.XXXXXX") || exit 1
		if ! exec 3<> "$kept" || ! rm -f "$kept"; then
			rm -f "$kept"
			echo "$me: its input couldn't be kept ($keep)" >&2
			exit 1
		fi
		if ! head -c 1048577 >&3; then
			echo "$me: its input couldn't be kept ($keep)" >&2
			exit 1
		fi
		if [ "$(wc -c < /dev/fd/3)" -gt 1048576 ]; then
			# More than the desktop is sent: the host's own op, given
			# all of it (what was read, then the rest as it comes).
			{
				cat /dev/fd/3
				cat
			} | {
				exec 3<&-
				real "$@"
			}
			exit $?
		fi
		"$here/olo" op "$@" < /dev/fd/3 3<&-
		status=$?
		[ "$status" -eq 69 ] || exit "$status"
		exec < /dev/fd/3 3<&-
		real "$@"
	fi
	asked op "$@"
	real "$@"
	;;
olo-screen)
	# Agent Screens on the desktop, by their own command line (the
	# olo-screens skill's), as `olo screen` asks them; pictures saved here.
	asked screen "$@"
	real "$@"
	;;
*)
	real "$@"
	;;
esac
