# How the agents' stand-ins read what reaches them, as the real programs do (sourced by each: `. reads.sh`).
#
# Claude Code (2.1.332) and Codex ask their terminal for the mouse: presses, drags and every move of the pointer
# (DEC modes 1000, 1002, 1003), reported in SGR form (1006), and for their keys by the kitty keyboard protocol;
# Antigravity asks for neither. A stand-in that asks for none of it hid a bug for three days: in Rex the pointer
# crossing such a pane made its waiting question "the person's" (2026-11-27). So a stand-in asks for what its
# agent asks (`asks_for_the_mouse`), and reads as it reads: a report of the mouse is read away (counted in
# RECORD.mouse), a key that is no answer is written down ("key:[…]") and the question stays, Return answers
# ("answer:[]"), Escape refuses ("answer:[esc]").
asks_for_the_mouse() { printf '\033[?1000h\033[?1002h\033[?1003h\033[?1006h\033[>1u'; }
# All three draw on a screen of their own (the alternate screen: nothing of them in the terminal's history).
full_screen() { printf '\033[?1049h'; trap 'printf "\033[<u\033[?1006l\033[?1003l\033[?1002l\033[?1000l\033[?1049l"' EXIT; }

# One key or report, whole: into `got` (an escape sequence without its ESC, with `esc` true).
one() {
	esc=false got=
	IFS= read -rsn1 got || return 1
	[ "$got" = $'\e' ] || return 0
	esc=true got=
	local c
	while IFS= read -rsn1 -t 0.2 c; do
		got=$got$c
		case $c in [A-Za-z~]) break ;; esac
	done
}

# A question's answer: waits for one, writing it into RECORD.
answer() {
	while one; do
		case $esc$got in
		'true[<'*) echo "$got" >> "$out.mouse" ;;
		true | 'true[27u' | 'true[27;'*) echo 'answer:[esc]' >> "$out"; return ;;
		true*) printf 'key:[esc%s]\n' "$got" >> "$out" ;;
		false) echo 'answer:[]' >> "$out"; return ;;
		false[[:cntrl:]]) printf 'key:[%s]\n' "$(printf '%s' "$got" | od -An -tx1 | tr -d ' ')" >> "$out" ;;
		*) printf 'answer:[%s]\n' "$got" >> "$out"; return ;;
		esac
	done
}

# A line typed at its prompt, into `line` (reports of the mouse read away).
told() {
	line=
	while one; do
		case $esc$got in
		'true[<'*) echo "$got" >> "$out.mouse" ;;
		true*) ;;
		false) return ;;
		*) line=$line$got ;;
		esac
	done
}
