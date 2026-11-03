//! brush, the bash-compatible shell written in Rust
//! (github.com/reubeno/brush), built with Olo: installed with Olo's
//! programs and upgraded with them, and what Olo Suggestions makes the
//! login shell (copied to /usr/local/bin/brush, listed in /etc/shells).
//! Nothing of Olo's own: brush's entry as its own `brush` binary has it.

fn main() {
    brush_shell::entry::run();
}
