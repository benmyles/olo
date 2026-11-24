//! The fuzzers' sandbox is sealed: a program in it sees nothing of the
//! person's home, runtime directory, environment, processes or network,
//! can't write to the build or the system, and can write its results.

#[test]
fn the_sandbox_is_sealed() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_olo-fuzz")).arg("sandbox-probe").output().expect("olo-fuzz runs");
    let text = String::from_utf8_lossy(&out.stdout);
    let err = String::from_utf8_lossy(&out.stderr);
    // (Where the kernel refuses a user namespace to whoever asks, as
    // Ubuntu's AppArmor does (`apparmor_restrict_unprivileged_userns`),
    // there is no sandbox to look at, and the fuzzers refuse to run
    // without one: said, not failed. Any other refusal fails.)
    if !out.status.success() && err.contains("no sandbox:") && (err.contains("setgroups: Permission denied") || err.contains("unshare: Operation not permitted")) {
        eprintln!("no user namespace here ({}): the sandbox not tested", err.trim());
        return;
    }
    assert!(out.status.success() && !text.contains("LEAK") && text.lines().count() >= 10, "{text}{err}");
}
