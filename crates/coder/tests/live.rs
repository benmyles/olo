//! Opt-in, real token-pool tests. They never touch the user's workspace.
//! TMPDIR=/var/tmp/... cargo test -p olo-coder-core --test live -- --ignored --nocapture
use olo_coder_core::{provider, *};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
#[ignore = "uses real token-pool quota"]
async fn pool_codex_reasoning_compaction_and_replay() {
    reasoning_compaction_and_replay(Provider::Codex, "gpt-6.1-terra").await;
}
#[tokio::test]
#[ignore = "uses real token-pool quota"]
async fn pool_claude_reasoning_compaction_and_replay() {
    reasoning_compaction_and_replay(Provider::Claude, "claude-sonnet-5-5").await;
}
async fn reasoning_compaction_and_replay(p: Provider, model: &str) {
    let client = provider::Client::new().unwrap();
    {
        let mut s = Session::new("/var/tmp".into(), Config::default());
        s.config.model.provider = p;
        s.config.model.model = model.into();
        s.config.model.effort = "high".into();
        if p == Provider::Claude {
            s.config.model.thinking = Some(json!({"type":"adaptive"}));
        }
        s.user("Remember the marker cedar-47 and that 19 * 31 = 589. Solve without tools: count permutations of 1 through 9 where no adjacent pair sums to 10, with 1 before 9. Check the combinatorial argument carefully. End with the marker and multiplication result; keep the final answer short.");
        let stop = CancellationToken::new();
        let first = client.turn(&s, "You are a concise test assistant. Follow the user.", &[], &stop, |_, _| {}).await.unwrap();
        println!("{model}: {} ({} output tokens)", first.text, first.usage.output);
        assert!(first.text.contains("589"));
        if p == Provider::Codex {
            assert!(first.items.iter().any(|v| v["type"] == "reasoning" && v["encrypted_content"].is_string()), "encrypted reasoning must be returned");
        }
        s.context.extend(first.items);
        s.user("What marker did I ask you to remember?");
        let second = client.turn(&s, "You are a concise test assistant. Follow the user.", &[], &stop, |_, _| {}).await.unwrap();
        assert!(second.text.contains("cedar-47"));
        s.context.extend(second.items);
        let compact = client.compact(&s, "You are a concise test assistant. Follow the user.", &[], &stop).await.unwrap();
        println!("{model}: compacted {} native items to {}", s.context.len(), compact.items.len());
        s.context = compact.items;
        s.user("Repeat the remembered marker and the multiplication result.");
        let third = client.turn(&s, "You are a concise test assistant. Follow the user.", &[], &stop, |_, _| {}).await.unwrap();
        assert!(third.text.contains("cedar-47") && third.text.contains("589"), "{}", third.text);
        println!("{model}: compacted context replay succeeded");
    }
}
#[tokio::test]
#[ignore = "uses real token-pool quota and compiles a scratch Rust program"]
async fn pool_agent_writes_and_verifies_rust() {
    let dir = std::path::PathBuf::from("/var/tmp").join(format!("olo-coder-live-{}", id()));
    std::fs::create_dir_all(&dir).unwrap();
    let e = Engine::open(dir.join("state")).unwrap();
    let mut config = Config::default();
    config.approval = ApprovalMode::Allow;
    config.max_turns = 12;
    config.token_budget = 60000;
    config.model.effort = "low".into();
    let s = e.command(Command::Create { workspace: dir.clone(), config: Some(config) }).await.unwrap();
    let sid = s["id"].as_str().unwrap().to_owned();
    e.command(Command::Send{session:sid.clone(),text:"Create solution.rs with a public Rust gcd(a:u64,b:u64)->u64 function and at least three unit tests including zero input. Compile it with rustc --test solution.rs -o solution-test and run ./solution-test. Use the provided native file tools and execute tool. Do not install anything. End with the exact test result.".into()}).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(300), e.quiesce()).await.unwrap();
    let s = e.get(&sid).unwrap();
    assert_eq!(s.status, Status::Idle, "{:?}", s.error);
    assert!(dir.join("solution.rs").exists());
    let events = e.store.lock().unwrap().events(&sid, 0, 1000).unwrap();
    assert!(events.iter().any(|e| e.kind == "tool_result" && e.data["name"] == "execute" && e.data["output"]["success"] == true));
    println!("live code task: {} model turns, {} input + {} output tokens", s.turns, s.usage.input, s.usage.output);
    drop(e);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
#[ignore = "uses real token-pool quota for an agent and approval reviewer"]
async fn pool_automatic_review_approves_a_scoped_edit() {
    let dir = std::path::PathBuf::from("/var/tmp").join(format!("olo-coder-live-{}", id()));
    std::fs::create_dir_all(&dir).unwrap();
    let e = Engine::open(dir.join("state")).unwrap();
    let mut config = Config::default();
    config.approval = ApprovalMode::Auto;
    config.approval_guidance =
        "Approve creating the workspace file scratch.txt containing hello. This is an explicitly authorized reversible test. Deny other writes and all process executions.".into();
    config.max_turns = 6;
    config.model.effort = "low".into();
    let sid = e.command(Command::Create { workspace: dir.clone(), config: Some(config) }).await.unwrap()["id"].as_str().unwrap().to_owned();
    let mut events = e.events.subscribe();
    e.command(Command::Send {
        session: sid.clone(),
        text: "Use write_file to create scratch.txt containing exactly hello. Then report completion. Do not run processes or change other files.".into(),
    })
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(120), async {
        loop {
            let event = events.recv().await.unwrap();
            if matches!(event.kind.as_str(), "idle" | "error" | "approval") {
                break;
            }
        }
    })
    .await
    .unwrap();
    let s = e.get(&sid).unwrap();
    assert_eq!(s.status, Status::Idle, "{:?} {:?}", s.error, s.pending);
    assert_eq!(std::fs::read_to_string(dir.join("scratch.txt")).unwrap(), "hello");
    let history = e.store.lock().unwrap().events(&sid, 0, 1000).unwrap();
    assert!(history.iter().any(|v| v.kind == "auto_review" && v.data["allow"] == true));
    println!("automatic reviewer approved the scoped edit; {} model turns", s.turns);
    e.quiesce().await;
    drop(e);
    std::fs::remove_dir_all(dir).unwrap();
}
