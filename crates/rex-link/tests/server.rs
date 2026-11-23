//! The helper itself in front of a real Rex server (a scratch one of the
//! test's own, `crates/rex/tests/scratch`), spoken to as the app speaks
//! to it: what the helper says unasked of a terminal is what the server
//! says when asked, and comes when it should.
//!
//! `through.rs` has the helper against a stand-in, which answers as the
//! helper's author thought the server does. That is how the helper came
//! to look for a block event's name under `event` where the server
//! writes `name`, and never saw a command end (2026-11-23): what the
//! helper takes from the server's own messages is held to the server
//! here.

#[path = "../../rex/tests/scratch/mod.rs"]
mod scratch;

use std::process::Stdio;
use std::time::{Duration, Instant};

use olo_rex_client::{ClientIdentity, ControlClient, ControlEvent, find_rex_binary};
use scratch::{Events, Result, Scratch};

/// The helper's next word about the terminal, within `within`: how it
/// reads, and after what typed.
async fn word(events: &mut Events, block: &str, within: Duration) -> Option<(serde_json::Value, Duration)> {
    let from = Instant::now();
    loop {
        let envelope = tokio::time::timeout_at((from + within).into(), events.recv()).await.ok()??;
        if let ControlEvent::BlockEvent { block_id, name, payload, .. } = envelope.event
            && block_id == block
            && name == "input_mode_changed"
        {
            assert_eq!(envelope.seq, 0, "the helper's own event has no place among the server's");
            return Some((payload, from.elapsed()));
        }
    }
}

#[tokio::test]
async fn what_the_helper_says_of_a_real_terminal_is_what_the_server_says_asked() -> Result {
    let Some(server) = Scratch::start().await? else { return Ok(()) };
    let rex = find_rex_binary();
    let mut helper = tokio::process::Command::new(env!("CARGO_BIN_EXE_olo-rex-link"))
        .arg("--")
        .arg(&rex)
        .args(["-S", &format!("unix://{}", server.sock.display()), "--autostart=false", "__internal", "dial-stdio"])
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", server.dir.join("home"))
        .env("XDG_RUNTIME_DIR", server.dir.join("run"))
        .env("XDG_STATE_HOME", server.dir.join("state"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stream = tokio::io::join(helper.stdout.take().unwrap(), helper.stdin.take().unwrap());
    let (ctrl, mut events) = ControlClient::connect(stream, "localhost", ClientIdentity::olo_rex(None), None).await?;
    assert!(ctrl.keeps_typing_in_order(), "the connection goes through the helper");
    let (sid, _, bid) = server.session(&ctrl, "says").await?;
    ctrl.session_bootstrap("all", Some(&sid), None, &[]).await?;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    // What the server says when asked, to hold the helper's word to.
    let asked = |ctrl: ControlClient, sid: String, bid: String| async move {
        let (mode, process) = tokio::try_join!(ctrl.terminal_input_mode(&sid, &bid), ctrl.terminal_process(&sid, &bid))?;
        let (mode, process) = (mode.payload, process.payload);
        let shell = matches!((&process.child, &process.foreground), (Some(child), Some(foreground)) if child.pid == foreground.pid);
        Ok::<_, olo_rex_client::RexError>((mode.echo, mode.canonical, shell))
    };
    let said = |word: &serde_json::Value| (word["echo"].as_bool().unwrap(), word["canonical"].as_bool().unwrap(), word["shell"].as_bool().unwrap());

    // A line ended that is over at once: the shell is at its prompt
    // again, and the helper says so in a moment, of after that line.
    ctrl.send_terminal_write(&sid, &bid, "true\r")?.await?;
    let (first, took) = word(&mut events, &bid, Duration::from_secs(3)).await.expect("a word after a line");
    assert_eq!(said(&first), (false, false, true), "the shell reads at its line editor: {first}");
    assert_eq!(said(&first), asked(ctrl.clone(), sid.clone(), bid.clone()).await?);
    assert_eq!(first["after"], ctrl.last_typed().to_string());
    assert!(took < Duration::from_millis(1500), "said {took:?} after the line was taken");
    eprintln!("a line over at once: the shell said to read {} ms after the server took the line", took.as_millis());

    // Letters: nothing said (the shell reads as it did).
    for letter in ["s", "l"] {
        ctrl.send_terminal_write(&sid, &bid, letter)?.await?;
    }
    assert!(word(&mut events, &bid, Duration::from_millis(400)).await.is_none(), "letters say nothing");

    // A command that takes its time: said not to read while it runs
    // (once), and to read again when it is over, of after the same line:
    // from the helper's own looks, or from the server's word that the
    // foreground changed, whichever comes first.
    ctrl.send_terminal_write(&sid, &bid, "eep 2\r")?.await?;
    let begun = Instant::now();
    let (runs, _) = word(&mut events, &bid, Duration::from_secs(2)).await.expect("a word while the command runs");
    assert!(!said(&runs).2, "another program is in the foreground: {runs}");
    let (over, _) = word(&mut events, &bid, Duration::from_secs(5)).await.expect("a word when the command is over");
    let after = begun.elapsed();
    assert_eq!(said(&over), (false, false, true), "{over}");
    assert_eq!(over["after"], ctrl.last_typed().to_string());
    assert!(after >= Duration::from_millis(1900) && after < Duration::from_millis(3500), "said {after:?} after a command of two seconds began");
    eprintln!("a command of 2 s: the shell said to read again {} ms after its line was taken", after.as_millis());

    // A secret read by the shell with the terminal's own line editing:
    // said as the server says it, which the app shows as a secret asked for.
    ctrl.send_terminal_write(&sid, &bid, "read -s x\r")?.await?;
    let (secret, _) = word(&mut events, &bid, Duration::from_secs(3)).await.expect("a word at a secret");
    assert_eq!((secret["echo"].as_bool(), secret["canonical"].as_bool()), (Some(false), Some(true)), "{secret}");
    assert_eq!(said(&secret), asked(ctrl.clone(), sid.clone(), bid.clone()).await?);
    ctrl.send_terminal_write(&sid, &bid, "\r")?.await?;
    let (back, _) = word(&mut events, &bid, Duration::from_secs(3)).await.expect("a word after the secret");
    assert_eq!(said(&back), (false, false, true), "{back}");
    Ok(())
}
