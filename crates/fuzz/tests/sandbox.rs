//! The fuzzers' sandbox is sealed: a program in it sees nothing of the
//! person's home, runtime directory, environment, processes or network,
//! can't write to the build or the system, and can write its results.

#[test]
fn the_sandbox_is_sealed() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_olo-fuzz")).arg("sandbox-probe").output().expect("olo-fuzz runs");
    let text = String::from_utf8_lossy(&out.stdout);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success() && !text.contains("LEAK") && text.lines().count() >= 10, "{text}{err}");
}
