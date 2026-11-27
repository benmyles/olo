//! What agents on a host are told is kept by the host's own helper at
//! each connection, and by no other build of it (`context::keep`).
//!
//! The helper built here is run as a link (its server side a stand-in
//! that ends by itself) twice in a scratch home that has Claude Code's
//! folder and Olo's skill for agents: once from where Rex puts a host's
//! helper, once from anywhere else.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const HELPER: &str = env!("CARGO_BIN_EXE_olo-rex-link");

fn run_as_a_link(helper: &Path, home: &Path) {
    // (`-- sleep 1`: what it runs as the server's side; its input closed, it ends with it.)
    let status = Command::new(helper)
        .args(["--", "sleep", "1"])
        .env("HOME", home)
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.code().is_some(), "the helper was ended by a signal: {status:?}");
}

#[test]
fn the_hosts_own_helper_keeps_what_agents_are_told_and_no_other_build_does() {
    let home = PathBuf::from(format!("/var/tmp/olo-rex-link-keep-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let claude = home.join(".claude/CLAUDE.md");
    std::fs::create_dir_all(home.join(".claude/skills/olo-rex-bridge")).unwrap();
    std::fs::write(home.join(".claude/skills/olo-rex-bridge/SKILL.md"), "the skill\n").unwrap();
    std::fs::write(&claude, "# Mine\n").unwrap();
    let told = || std::fs::read_to_string(&claude).unwrap().contains("olo open");

    // A build run from anywhere else (this one, from the build's own folder): nothing written, the skill's stamp there or not.
    std::fs::write(home.join(".claude/skills/olo-rex-bridge/.olo-skill"), "0001\n").unwrap();
    run_as_a_link(Path::new(HELPER), &home);
    assert!(!told(), "a build that isn't the host's own wrote into the home it ran in");

    // The host's own, from where Rex puts it. Without Olo's skill on the host (no stamp): nothing.
    let own = home.join(".local/share/olo/rex-link/olo-rex-link-test-x86_64");
    std::fs::create_dir_all(own.parent().unwrap()).unwrap();
    std::fs::copy(HELPER, &own).unwrap();
    std::fs::remove_file(home.join(".claude/skills/olo-rex-bridge/.olo-skill")).unwrap();
    run_as_a_link(&own, &home);
    assert!(!told(), "written on a host that hasn't Olo's skill for agents");

    // With it: the section is there after a connection, the person's text before it; and again changes nothing.
    std::fs::write(home.join(".claude/skills/olo-rex-bridge/.olo-skill"), "0001\n").unwrap();
    run_as_a_link(&own, &home);
    let after = std::fs::read_to_string(&claude).unwrap();
    assert!(after.starts_with("# Mine\n") && told(), "{after}");
    run_as_a_link(&own, &home);
    assert_eq!(std::fs::read_to_string(&claude).unwrap(), after);

    // Turned off on this host (a file somebody put there): taken away at the next connection, the person's text as
    // it was, and not put back; and the command itself says so and puts nothing.
    std::fs::create_dir_all(home.join(".config/olo")).unwrap();
    std::fs::write(home.join(".config/olo/agent-context.off"), "").unwrap();
    run_as_a_link(&own, &home);
    assert_eq!(std::fs::read_to_string(&claude).unwrap(), "# Mine\n", "turned off, it is taken away");
    let said = Command::new(&own).args(["context", "install"]).env("HOME", &home).env_remove("CLAUDE_CONFIG_DIR").env_remove("CODEX_HOME").output().unwrap();
    assert!(said.status.success() && String::from_utf8_lossy(&said.stderr).contains("turned off on this host") && !told(), "{}", String::from_utf8_lossy(&said.stderr));
    std::fs::remove_file(home.join(".config/olo/agent-context.off")).unwrap();
    run_as_a_link(&own, &home);
    assert!(told(), "the file gone, it is kept again");
    let _ = std::fs::remove_dir_all(&home);
}
