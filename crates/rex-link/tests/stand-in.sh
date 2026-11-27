#!/bin/bash
# A stand-in for an agent (or any program) in a child tab, for the
# agents service's tests: no agent, no account, nothing decided by
# itself. It is run under the agent's own name (its file is `claude`,
# `agy` or `codex` in a folder put first on PATH), draws what the test
# tells it to and writes down every byte it is given.
#
# The test speaks to it through $STAND_IN_DIR/$STAND_IN_NAME.fifo, a
# line a time:
#   show FILE      clear the screen and draw FILE
#   run COMMAND    run COMMAND (sh), its output to NAME.out; NAME.ran when over
#   exit N         end with status N
# What is typed into it is in NAME.keys, as it came. NAME.ready is
# there once it listens.
dir=$STAND_IN_DIR
name=$STAND_IN_NAME
# (Keys as they are typed, none echoed; lines it writes still end as lines.)
stty -icanon -echo -isig -icrnl -ixon 2>/dev/null
# The alternate screen and bracketed paste, as the agents have them.
printf '\033[?1049h\033[?2004h\033[2J\033[H'
# (Its input given to the recorder by name: a command put in the
# background by a script reads nothing otherwise.)
exec 3<&0
cat <&3 >> "$dir/$name.keys" &
keys=$!
# (The pipe it is told through, held open for as long as it runs: opened
# anew for each line, a second line written while the first was being
# read went into that opening and was lost with it. A test that showed a
# screen and told it to exit at once then waited for ever on a stand-in
# that never heard the second.)
exec 4<> "$dir/$name.fifo"
: > "$dir/$name.ready"
while :; do
	IFS= read -r line <&4 || continue
	case $line in
	"show "*)
		# (Cleared and drawn in one write, as an agent draws a frame:
		# cleared and then drawn, a reading between the two was of an
		# empty screen, and a question read so had "disappeared".)
		frame=$(cat "${line#show }"; printf x)
		printf '\033[2J\033[H%s' "${frame%x}"
		;;
	"run "*)
		sh -c "${line#run }" > "$dir/$name.out" 2>&1 < /dev/null
		: > "$dir/$name.ran"
		;;
	"exit "*)
		kill "$keys" 2>/dev/null
		printf '\033[?1049l'
		exit "${line#exit }"
		;;
	esac
done
