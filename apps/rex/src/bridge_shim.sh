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
