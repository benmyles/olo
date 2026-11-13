use olo_coder_core::{
    provider::{self, Sse},
    store::Store,
    tools, *,
};
use olo_sync::Mutex;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let p = PathBuf::from("/var/tmp").join(format!("olo-coder-test-{}", id()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn call(name: &str, a: Value) -> ToolCall {
    ToolCall { id: id(), name: name.into(), arguments: a }
}
#[test]
fn context_estimate_counts_images_without_tokenizing_their_encoding() {
    let encoded = "a".repeat(20 * 1024 * 1024);
    for block in [json!({"type":"input_image","image_url":encoded}), json!({"type":"image","source":{"type":"base64","data":encoded}})] {
        let context = vec![json!({"role":"user","content":[block,{"type":"text","text":"Inspect this image"}]})];
        assert!((8192..8300).contains(&provider::estimate_context(&context)));
    }
    assert_eq!(provider::estimate_context(&[json!("a".repeat(3000))]), 1000);
    assert!(provider::estimate_context(&[json!({"type":"reasoning","encrypted_content":"a".repeat(3000)})]) >= 1000);
}
#[test]
fn opaque_reasoning_and_compaction_survive_disk_and_request() {
    let scratch = Scratch::new();
    let mut db = Store::open(&scratch.0).unwrap();
    let mut s = Session::new(scratch.0.clone(), Config::default());
    let opaque = json!({"id":"reason1","type":"reasoning","summary":[],"encrypted_content":"opaque+/=abc","future_field":{"keep":true}});
    let compact = json!({"type":"compaction","encrypted_content":"compact+/=xyz","id":"cmp1"});
    s.context = vec![compact.clone(), opaque.clone()];
    db.save(&mut s, "test", json!({})).unwrap();
    drop(db);
    let db = Store::open(&scratch.0).unwrap();
    let s = db.get(&s.id).unwrap();
    let body = provider::body(&s, "system", &tools::definitions());
    assert_eq!(body["input"], json!([compact, opaque]));
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(body["store"], false);
}
#[test]
fn sse_split_every_byte_unicode_crlf_multiline() {
    let wire = "event: hello\r\ndata: {\"text\":\r\ndata: \"héllo 🌿\"}\r\n\r\n: ping\n\ndata: [DONE]\n\n";
    let mut decoder = Sse::default();
    let mut data = vec![];
    for b in wire.bytes() {
        data.extend(decoder.push(&[b]).unwrap());
    }
    assert_eq!(data.len(), 2);
    assert_eq!(serde_json::from_str::<Value>(&data[0]).unwrap()["text"], "héllo 🌿");
    assert_eq!(data[1], "[DONE]");
}
#[test]
fn incomplete_replies_never_produce_tools() {
    assert!(provider::parse(Provider::Codex, json!({"status":"incomplete","output":[{"type":"function_call","call_id":"x","name":"execute","arguments":"{}"}]})).is_err());
    assert!(provider::parse(Provider::Openai, json!({"stop_reason":"max_tokens","content":[]})).is_err());
}
#[test]
fn file_edits_are_checked_undoable_and_conflict_safe() {
    let dir = Scratch::new();
    let db = Arc::new(Mutex::new(Store::open(&dir.0.join("state")).unwrap()));
    let mut s = Session::new(dir.0.clone(), Config::default());
    db.lock().unwrap().save(&mut s, "created", json!({})).unwrap();
    let token = CancellationToken::new();
    let created = tools::file_tool(&s, &call("write_file", json!({"path":"hello.rs","content":"one\ntwo\n"})), &db, &token).unwrap();
    assert!(created["diff"].as_str().unwrap().contains("+one"));
    assert!(tools::file_tool(&s, &call("write_file", json!({"path":"hello.rs","content":"oops"})), &db, &token).is_err());
    let changed = tools::file_tool(&s, &call("edit_file", json!({"path":"hello.rs","old":"two","new":"three"})), &db, &token).unwrap();
    db.lock().unwrap().undo(&s.id, changed["edit"].as_i64().unwrap()).unwrap();
    assert_eq!(std::fs::read_to_string(dir.0.join("hello.rs")).unwrap(), "one\ntwo\n");
    std::fs::write(dir.0.join("hello.rs"), "person's newer work").unwrap();
    assert!(db.lock().unwrap().undo(&s.id, created["edit"].as_i64().unwrap()).is_err());
}
#[test]
fn symlink_and_parent_escape_are_rejected() {
    let dir = Scratch::new();
    std::os::unix::fs::symlink("/etc", dir.0.join("escape")).unwrap();
    assert!(tools::path(&dir.0, "escape/passwd", false).is_err());
    assert!(tools::path(&dir.0, "../outside", true).is_err());
    assert!(tools::path(&dir.0, "escape/new", true).is_err());
}
#[test]
fn scoped_instructions_and_gitignored_search() {
    let dir = Scratch::new();
    std::fs::create_dir(dir.0.join("sub")).unwrap();
    std::fs::create_dir(dir.0.join(".git")).unwrap();
    std::fs::write(dir.0.join("AGENTS.md"), "root rules").unwrap();
    std::fs::write(dir.0.join("sub/AGENTS.md"), "sub rules").unwrap();
    std::fs::write(dir.0.join(".gitignore"), "secret.txt\n").unwrap();
    std::fs::write(dir.0.join("secret.txt"), "needle secret").unwrap();
    std::fs::write(dir.0.join("sub/code.rs"), "needle code").unwrap();
    let instructions = tools::instructions(&dir.0, &dir.0.join("sub/code.rs")).unwrap();
    assert!(instructions.contains("root rules") && instructions.contains("sub rules"));
    let db = Arc::new(Mutex::new(Store::open(&dir.0.join("state")).unwrap()));
    let s = Session::new(dir.0.clone(), Config::default());
    let found = tools::file_tool(&s, &call("search", json!({"pattern":"needle"})), &db, &CancellationToken::new()).unwrap();
    assert_eq!(found["results"].as_array().unwrap().len(), 1);
}
#[test]
fn journal_is_paged_ordered_and_searchable() {
    let dir = Scratch::new();
    let mut db = Store::open(&dir.0).unwrap();
    let mut s = Session::new(dir.0.clone(), Config::default());
    for n in 0..250 {
        db.save(&mut s, "event", json!({"text":format!("entry {n}")})).unwrap();
    }
    let first = db.events(&s.id, 0, 100).unwrap();
    let second = db.events(&s.id, first.last().unwrap().seq, 100).unwrap();
    assert_eq!(second[0].seq, first[99].seq + 1);
    assert_eq!(db.search(&s.id, "entry 249").unwrap().len(), 1);
    assert_eq!(db.list().unwrap().len(), 1);
    assert!(db.tail_cursor(&s.id, 50).unwrap() > 0);
}
#[test]
fn interrupted_effect_is_preserved_and_not_automatically_replayed() {
    let dir = Scratch::new();
    let mut db = Store::open(&dir.0).unwrap();
    let mut s = Session::new(dir.0.clone(), Config::default());
    s.status = Status::Running;
    s.in_flight = Some(call("execute", json!({"program":"danger"})));
    db.save(&mut s, "tool_started", json!({})).unwrap();
    drop(db);
    let e = Engine::open(dir.0.clone()).unwrap();
    let recovered = e.get(&s.id).unwrap();
    assert_eq!(recovered.status, Status::Interrupted);
    assert!(recovered.in_flight.is_some());
    assert!(recovered.error.unwrap().contains("unknown"));
}
#[test]
fn defaults_and_shared_command_language() {
    let s = Session::new("/".into(), Config::default());
    assert!(!s.config.subagents);
    assert_eq!(s.config.approval, ApprovalMode::Allow);
    assert_eq!(s.config.sandbox, SandboxMode::Unrestricted);
    assert_eq!((s.config.max_turns, s.config.token_budget), (0, 0), "no run limits by default");
    assert!(s.config.within_turns(u32::MAX - 1) && s.config.within_tokens(u64::MAX - 1));
    assert_eq!((s.config.model.compact_at, s.config.model.max_output_tokens), (500_000, 128_000));
    for text in [
        "hello",
        "/stop",
        "/resume",
        "/goal build it",
        "/spec on",
        "/spec off",
        "/accept",
        "/model custom-model ultra",
        "/approval auto",
        "/guidance be careful",
        "/agents on",
        "/readonly on",
        "/keep on",
        "/compact",
        "/history test",
        "/config",
        "/help",
    ] {
        assert!(commands::parse(text, &s).is_ok(), "{text}");
    }
    assert!(commands::parse("/approval nonsense", &s).is_err());
    let mut c = s.config;
    c.max_turns = 0;
    c.validate().unwrap();
    c.max_turns = 3;
    assert!(c.within_turns(2) && !c.within_turns(3));
    c.token_budget = 10;
    assert!(c.within_tokens(9) && !c.within_tokens(10));
    assert_eq!((c.tokens_left(4), c.tokens_left(99)), (6, 1));
    c.compaction.summary_tokens = 1_000_000;
    assert!(c.validate().is_err());
}
#[tokio::test]
async fn processes_stream_and_cancel_immediately() {
    let dir = Scratch::new();
    let mut s = Session::new(dir.0.clone(), Config::default());
    s.config.sandbox = SandboxMode::Unrestricted;
    let token = CancellationToken::new();
    let t = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        t.cancel();
    });
    let started = std::time::Instant::now();
    let r = tools::execute(&s, &call("execute", json!({"program":"/bin/sleep","args":["30"]})), &dir.0, &token, Arc::new(|_, _| {})).await;
    assert!(r.unwrap_err().to_string().contains("cancelled"));
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    let r = tools::execute(&s, &call("execute", json!({"program":"/bin/echo","args":["héllo"]})), &dir.0, &CancellationToken::new(), Arc::new(|_, _| {})).await.unwrap();
    assert_eq!(r["exit_code"], 0);
    assert!(r["stdout"].as_str().unwrap().contains("héllo"));
}
#[tokio::test]
async fn workspace_sandbox_denies_outside_writes() {
    let dir = Scratch::new();
    let outside = Scratch::new();
    let s = Session::new(dir.0.clone(), Config { sandbox: SandboxMode::Workspace, ..Default::default() });
    let r = tools::execute(&s, &call("execute", json!({"program":"/usr/bin/touch","args":[outside.0.join("should-not-exist")]})), &dir.0, &CancellationToken::new(), Arc::new(|_, _| {})).await;
    match r {
        Ok(r) => assert_eq!(r["success"], false),
        Err(e) => assert!(e.to_string().contains("Landlock"), "{e:#}"),
    };
    assert!(!outside.0.join("should-not-exist").exists());
}

#[derive(Clone)]
struct Fake {
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    replies: Arc<Mutex<std::collections::VecDeque<Value>>>,
}
async fn fake(axum::extract::State(fake): axum::extract::State<Fake>, uri: axum::http::Uri, axum::Json(body): axum::Json<Value>) -> axum::response::Response {
    use axum::response::IntoResponse;
    let compacting = body["input"].as_array().is_some_and(|items| items.last().is_some_and(|v| v["type"] == "compaction_trigger"));
    fake.requests.lock().unwrap().push((uri.path().into(), body));
    let reply = fake.replies.lock().unwrap().pop_front().unwrap_or_else(|| {
        if compacting {
            json!({"status":"completed","output":[{"type":"compaction","encrypted_content":"opaque-checkpoint","id":"cmp"}],"usage":{"input_tokens":20,"output_tokens":10}})
        } else {
            json!({"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done."}]}],"usage":{"input_tokens":12,"output_tokens":4}})
        }
    });
    if uri.path().ends_with("/compact") {
        return axum::Json(reply).into_response();
    }
    let data = if reply.get("content").is_some() {
        let mut events = format!("data: {}\n\n", json!({"type":"message_start","message":{"content":[],"usage":{"input_tokens":10}}}));
        for (i, b) in reply["content"].as_array().unwrap().iter().enumerate() {
            events += &format!("data: {}\n\ndata: {}\n\n", json!({"type":"content_block_start","index":i,"content_block":b}), json!({"type":"content_block_stop","index":i}));
        }
        events += &format!("data: {}\n\ndata: {}\n\n", json!({"type":"message_delta","delta":{"stop_reason":reply["stop_reason"]},"usage":{"output_tokens":5}}), json!({"type":"message_stop"}));
        events
    } else {
        format!("data: {}\n\n", json!({"type":"response.completed","response":reply}))
    };
    ([("content-type", "text/event-stream")], data).into_response()
}
async fn server() -> (Fake, String, tokio::task::JoinHandle<()>) {
    let state = Fake { requests: Default::default(), replies: Default::default() };
    let app = axum::Router::new().route("/{*rest}", axum::routing::post(fake)).with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (state, base, task)
}
fn response(items: Vec<Value>) -> Value {
    json!({"status":"completed","output":items,"usage":{"input_tokens":10,"output_tokens":5}})
}
fn tool(name: &str, args: Value) -> Value {
    json!({"type":"function_call","call_id":"call1","name":name,"arguments":args.to_string()})
}
async fn finish(e: &Arc<Engine>) {
    tokio::time::timeout(std::time::Duration::from_secs(10), e.quiesce()).await.unwrap();
}
#[tokio::test]
async fn full_loop_approval_compaction_specs_and_budget() {
    let dir = Scratch::new();
    let (fake, base, server) = server().await;
    let e = Engine::open_at(dir.0.join("state"), Some(base.clone())).unwrap();
    let created = e.command(Command::Create { workspace: dir.0.clone(), config: Some(Config { approval: ApprovalMode::Ask, ..Default::default() }) }).await.unwrap();
    let sid = created["id"].as_str().unwrap().to_string();
    fake.replies
        .lock()
        .unwrap()
        .push_back(response(vec![json!({"type":"reasoning","id":"r1","encrypted_content":"secret"}), tool("write_file", json!({"path":"result.rs","content":"fn main() {}\n"}))]));
    let mut events = e.events.subscribe();
    e.command(Command::Send { session: sid.clone(), text: "Implement".into() }).await.unwrap();
    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv()).await.unwrap().unwrap();
        if event.kind == "approval" {
            break;
        }
    }
    assert!(!dir.0.join("result.rs").exists());
    assert_eq!(e.get(&sid).unwrap().status, Status::Approval);
    assert!(e.command(Command::Resume { session: sid.clone() }).await.is_err());
    assert!(e.command(Command::Approve { session: sid.clone(), call_id: "stale".into(), allow: true }).await.is_err());
    e.command(Command::Approve { session: sid.clone(), call_id: "call1".into(), allow: true }).await.unwrap();
    finish(&e).await;
    assert!(dir.0.join("result.rs").exists());
    let req = fake.requests.lock().unwrap().clone();
    assert_eq!(req[1].1["input"][1]["encrypted_content"], "secret");
    assert!(req[1].1["input"].as_array().unwrap().iter().any(|v| v["type"] == "function_call_output"));
    let compact = json!({"type":"compaction","encrypted_content":"compressed","id":"cmp"});
    fake.replies.lock().unwrap().push_back(json!({"output":[{"role":"user","content":"retained"},compact],"usage":{"input_tokens":100,"output_tokens":20}}));
    e.command(Command::Compact { session: sid.clone() }).await.unwrap();
    finish(&e).await;
    let s = e.get(&sid).unwrap();
    assert_eq!(s.context.len(), 2);
    assert_eq!(s.context[1], compact);
    e.command(Command::Send { session: sid.clone(), text: "continue".into() }).await.unwrap();
    finish(&e).await;
    assert_eq!(fake.requests.lock().unwrap().last().unwrap().1["input"][1], compact);
    // Spec mode cannot edit even if the person configured Allow.
    let mut config = s.config.clone();
    config.approval = ApprovalMode::Allow;
    e.command(Command::Configure { session: sid.clone(), config }).await.unwrap();
    e.command(Command::Spec { session: sid.clone(), text: "A detailed spec".into(), criteria: vec![] }).await.unwrap();
    assert!(e.command(Command::AcceptSpec { session: sid.clone(), revision: 9 }).await.is_err());
    fake.replies.lock().unwrap().push_back(response(vec![tool("write_file", json!({"path":"forbidden","content":"no"}))]));
    e.command(Command::Send { session: sid.clone(), text: "discuss spec".into() }).await.unwrap();
    finish(&e).await;
    assert!(!dir.0.join("forbidden").exists());
    e.command(Command::AcceptSpec { session: sid.clone(), revision: 1 }).await.unwrap();
    assert!(!e.get(&sid).unwrap().spec_mode);
    let mut config = e.get(&sid).unwrap().config;
    config.max_turns = 1;
    config.keep_going = true;
    e.command(Command::Configure { session: sid.clone(), config }).await.unwrap();
    e.command(Command::Goal { session: sid.clone(), objective: "Keep working".into() }).await.unwrap();
    e.command(Command::Send { session: sid.clone(), text: "go".into() }).await.unwrap();
    finish(&e).await;
    assert!(e.get(&sid).unwrap().error.unwrap().contains("budget"));
    server.abort();
}
#[tokio::test]
async fn automatic_review_fails_closed_and_hard_denials_win() {
    let dir = Scratch::new();
    let (fake, base, server) = server().await;
    let e = Engine::open_at(dir.0.join("state"), Some(base)).unwrap();
    let config = Config { approval: ApprovalMode::Auto, ..Config::default() };
    let sid = e.command(Command::Create { workspace: dir.0.clone(), config: Some(config) }).await.unwrap()["id"].as_str().unwrap().to_owned();
    fake.replies.lock().unwrap().extend([
        response(vec![tool("write_file", json!({"path":"unapproved","content":"no"}))]),
        response(vec![json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"not valid review JSON"}]})]),
    ]);
    let mut events = e.events.subscribe();
    e.command(Command::Send { session: sid.clone(), text: "test review".into() }).await.unwrap();
    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv()).await.unwrap().unwrap();
        if event.kind == "approval" {
            break;
        }
    }
    assert!(!dir.0.join("unapproved").exists());
    e.command(Command::Approve { session: sid.clone(), call_id: "call1".into(), allow: false }).await.unwrap();
    finish(&e).await;
    let mut config = e.get(&sid).unwrap().config;
    config.approval = ApprovalMode::Allow;
    config.allowed_tools.push("write_file".into());
    config.denied_tools.push("write_file".into());
    e.command(Command::Configure { session: sid.clone(), config }).await.unwrap();
    fake.replies.lock().unwrap().push_back(response(vec![tool("write_file", json!({"path":"denied","content":"no"}))]));
    e.command(Command::Send { session: sid.clone(), text: "test denial".into() }).await.unwrap();
    finish(&e).await;
    assert!(!dir.0.join("denied").exists());
    assert!(!dir.0.join("unapproved").exists());
    server.abort();
}

#[tokio::test]
async fn rpc_replays_live_events_and_survives_client_detach() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let dir = Scratch::new();
    let path = dir.0.join("state");
    let service = tokio::spawn(rpc::serve(path.clone()));
    for _ in 0..100 {
        if rpc::socket(&path).exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    async fn request(path: &std::path::Path, command: Value) -> Value {
        let mut s = tokio::net::UnixStream::connect(rpc::socket(path)).await.unwrap();
        s.write_all(format!("{}\n", json!({"version":1,"id":7,"command":command})).as_bytes()).await.unwrap();
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).await.unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], 7);
        assert!(v.get("error").is_none(), "{v}");
        v["result"].clone()
    }
    let s = request(&path, json!({"op":"create","workspace":dir.0})).await;
    let mut stream = tokio::net::UnixStream::connect(rpc::socket(&path)).await.unwrap();
    stream.write_all(format!("{}\n", json!({"subscribe":true,"session":s["id"],"after":0})).as_bytes()).await.unwrap();
    let mut reader = BufReader::new(stream);
    let mut cursor = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        if v["ready"] == true {
            break;
        }
        cursor = v["seq"].as_i64().unwrap();
    }
    request(&path, json!({"op":"goal","session":s["id"],"objective":"persistent work"})).await;
    let mut line = String::new();
    tokio::time::timeout(std::time::Duration::from_secs(2), reader.read_line(&mut line)).await.unwrap().unwrap();
    let event: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(event["kind"], "goal");
    assert!(event["seq"].as_i64().unwrap() > cursor);
    drop(reader);
    let saved = request(&path, json!({"op":"get","session":s["id"]})).await;
    assert_eq!(saved["goal"]["objective"], "persistent work");
    request(&path, json!({"op":"shutdown"})).await;
    tokio::time::timeout(std::time::Duration::from_secs(2), service).await.unwrap().unwrap().unwrap();
    assert!(!rpc::socket(&path).exists());
}

#[tokio::test]
async fn native_http_is_bounded_and_cancellable() {
    let app = axum::Router::new().route("/", axum::routing::get(|| async { "x".repeat(5000) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let r = tools::fetch_url(&call("fetch_url", json!({"url":url})), 1024, &CancellationToken::new()).await.unwrap();
    assert_eq!(r["text"].as_str().unwrap().len(), 1024);
    assert_eq!(r["truncated"], true);
    assert!(tools::fetch_url(&call("fetch_url", json!({"url":"file:///etc/passwd"})), 1024, &CancellationToken::new()).await.is_err());
    let token = CancellationToken::new();
    token.cancel();
    assert!(tools::fetch_url(&call("fetch_url", json!({"url":url})), 1024, &token).await.is_err());
    server.abort();
}

#[tokio::test]
async fn long_session_repeated_compaction_keeps_history_and_bounded_context() {
    let dir = Scratch::new();
    let (fake, base, server) = server().await;
    let e = Engine::open_at(dir.0.join("state"), Some(base)).unwrap();
    let mut config = Config { keep_going: true, max_turns: 500, ..Config::default() };
    config.model.compact_at = 160;
    let sid = e.command(Command::Create { workspace: dir.0.clone(), config: Some(config) }).await.unwrap()["id"].as_str().unwrap().to_owned();
    e.command(Command::Goal { session: sid.clone(), objective: "a long-running task".into() }).await.unwrap();
    e.command(Command::Memory { session: sid.clone(), text: "keep this decision across every compaction".into() }).await.unwrap();
    e.command(Command::Send { session: sid.clone(), text: "begin".into() }).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(60), e.quiesce()).await.unwrap();
    let s = e.get(&sid).unwrap();
    assert_eq!(s.turns, 500);
    assert!(s.compactions > 50);
    assert!(s.context.len() < 12, "{} items", s.context.len());
    assert!(s.error.unwrap().contains("budget"));
    assert!(fake.requests.lock().unwrap().iter().all(|(_, b)| b["input"].to_string().contains("keep this decision")));
    let mut cursor = 0;
    let mut count = 0;
    loop {
        let events = e.store.lock().unwrap().events(&sid, cursor, 100).unwrap();
        if events.is_empty() {
            break;
        }
        for event in events {
            assert!(event.seq > cursor);
            cursor = event.seq;
            count += 1;
        }
    }
    assert!(count > 1500);
    drop(e);
    let db = Store::open(&dir.0.join("state")).unwrap();
    let restored = db.get(&sid).unwrap();
    assert_eq!(restored.turns, 500);
    assert_eq!(restored.memory, "keep this decision across every compaction");
    assert!(db.tail_cursor(&sid, 500).unwrap() > 0);
    println!("long-session fixture: 500 turns, {} compactions, {count} durable events, {} active native items", restored.compactions, restored.context.len());
    server.abort();
}

#[test]
fn skills_load_on_demand_without_filling_the_prompt() {
    let dir = Scratch::new();
    let skills = Scratch::new();
    let skill = skills.0.join("rust");
    std::fs::create_dir(&skill).unwrap();
    std::fs::write(skill.join("SKILL.md"), format!("---\nname: rust\ndescription: Review Rust code\n---\n{}", "Detailed instructions. ".repeat(1000))).unwrap();
    let mut s = Session::new(dir.0.clone(), Config::default());
    s.config.skill_dirs.push(skills.0.clone());
    let catalog = tools::skill_catalog(&s);
    assert!(catalog.contains("Review Rust code"));
    assert!(!catalog.contains("Detailed instructions"));
    assert!(catalog.len() < 1000);
    assert!(tools::read_skill(&s, skill.join("SKILL.md").to_str().unwrap()).unwrap()["text"].as_str().unwrap().contains("Detailed instructions"));
    assert!(tools::read_skill(&s, "/etc/passwd").is_err());
    assert!(tools::definitions().iter().any(|d| d.name == "read_skill"));
}

#[test]
fn native_git_index_commit_diff_worktrees_and_conflicts() {
    use std::sync::atomic::AtomicBool;
    let scratch = Scratch::new();
    let repo = gix::init(&scratch.0).unwrap();
    std::fs::write(repo.git_dir().join("config"), "[core]\nrepositoryformatversion = 0\nbare = false\n[user]\nname = Test Author\nemail = test@example.invalid\n").unwrap();
    let run = |name: &str, args: Value| olo_coder_core::git::run(&scratch.0, &call(name, args), Arc::new(AtomicBool::new(false)));
    std::fs::write(scratch.0.join("hello.rs"), "fn main() {}\n").unwrap();
    let status = run("repository_read", json!({"operation":"status"})).unwrap();
    assert!(status["files"].as_array().unwrap().iter().any(|f| f["path"] == "hello.rs"));
    assert!(run("repository_read", json!({"operation":"stage","paths":["hello.rs"]})).is_err());
    run("repository_write", json!({"operation":"stage","paths":["hello.rs"]})).unwrap();
    let diff = run("repository_read", json!({"operation":"diff","path":"hello.rs","staged":true})).unwrap();
    assert!(diff["diff"].as_str().unwrap().contains("+fn main()"));
    let first = run("repository_write", json!({"operation":"commit","message":"Initial","expected_head":""})).unwrap();
    let head = first["commit"].as_str().unwrap();
    assert_eq!(run("repository_read", json!({"operation":"status"})).unwrap()["files"], json!([]));
    std::fs::write(scratch.0.join("hello.rs"), "fn main() { println!(\"Hi\"); }\n").unwrap();
    run("repository_write", json!({"operation":"stage","paths":["hello.rs"]})).unwrap();
    run("repository_write", json!({"operation":"unstage","paths":["hello.rs"]})).unwrap();
    assert_eq!(run("repository_read", json!({"operation":"diff","path":"hello.rs","staged":true})).unwrap()["diff"], "");
    run("repository_write", json!({"operation":"stage","paths":["hello.rs"]})).unwrap();
    assert!(run("repository_write", json!({"operation":"commit","message":"stale","expected_head":""})).is_err());
    run("repository_write", json!({"operation":"commit","message":"Greeting","expected_head":head})).unwrap();
    run("repository_write", json!({"operation":"branch","branch":"experiment"})).unwrap();
    run("repository_write", json!({"operation":"worktree_add","branch":"experiment","path":"experiment"})).unwrap();
    assert!(scratch.0.join("experiment/hello.rs").exists());
    assert_eq!(run("repository_read", json!({"operation":"worktrees"})).unwrap()["worktrees"].as_array().unwrap().len(), 2);
    assert!(run("repository_write", json!({"operation":"worktree_add","branch":"experiment","path":"other"})).is_err());
    assert!(!scratch.0.join("other").exists());
    std::fs::write(repo.git_dir().join("index.lock"), "other process").unwrap();
    assert!(run("repository_write", json!({"operation":"stage","paths":["hello.rs"]})).is_err());
    assert_eq!(std::fs::read_to_string(repo.git_dir().join("index.lock")).unwrap(), "other process");
    assert!(run("repository_write", json!({"operation":"stage","paths":["../outside"]})).is_err());
}

#[tokio::test]
async fn github_scoping_drafts_pagination_and_write_guards() {
    use axum::{
        Json, Router,
        extract::State,
        http::{HeaderMap, Method, Uri},
        routing::any,
    };
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let seen = requests.clone();
    let app = Router::new()
        .fallback(any(|State(seen): State<Arc<Mutex<Vec<Value>>>>, method: Method, uri: Uri, headers: HeaderMap, body: String| async move {
            seen.lock()
                .unwrap()
                .push(json!({"method":method.as_str(),"path":uri.path(),"auth":headers.get("authorization").unwrap().to_str().unwrap(),"body":serde_json::from_str::<Value>(&body).ok()}));
            ([("link", "<https://api.github.com/next>; rel=\"next\"")], Json(json!({"number":17,"html_url":"https://github.com/a/b/pull/17"})))
        }))
        .with_state(seen);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cancel = CancellationToken::new();
    let result = github::request(&base, Some("test-token"), &call("github_read", json!({"repository":"a/b","operation":"pulls"})), &cancel).await.unwrap();
    assert_eq!(result["has_next_page"], true);
    github::request(&base, Some("test-token"), &call("github_write", json!({"repository":"a/b","operation":"create_pull","body":{"title":"Test","head":"work","base":"main"}})), &cancel)
        .await
        .unwrap();
    assert_eq!(requests.lock().unwrap()[1]["body"]["draft"], true);
    assert!(github::request(&base, Some("test-token"), &call("github_read", json!({"repository":"a/b","operation":"comment","number":1,"body":{"body":"no"}})), &cancel).await.is_err());
    assert!(github::request(&base, Some("test-token"), &call("github_write", json!({"repository":"a/b","operation":"merge","number":1,"body":{}})), &cancel).await.is_err());
    assert!(github::request(&base, Some("test-token"), &call("github_read", json!({"repository":"a/../evil","operation":"pulls"})), &cancel).await.is_err());
    assert_eq!(requests.lock().unwrap().len(), 2);
    server.abort();
}

#[tokio::test]
async fn http_mcp_negotiates_sessions_streams_and_cancels_without_replay() {
    use axum::{
        Router,
        extract::State,
        http::{HeaderMap, Method},
        response::IntoResponse,
        routing::any,
    };
    let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
    let cancelled = Arc::new(tokio::sync::Notify::new());
    let app = Router::new()
        .fallback(any(|State((seen, cancelled)): State<(Arc<Mutex<Vec<Value>>>, Arc<tokio::sync::Notify>)>, method: Method, headers: HeaderMap, body: String| async move {
            if method == Method::DELETE {
                return axum::http::StatusCode::NO_CONTENT.into_response();
            }
            let v: Value = serde_json::from_str(&body).unwrap();
            if v["method"] == "notifications/cancelled" {
                cancelled.notify_one();
            }
            seen.lock()
                .unwrap()
                .push(json!({"request":v,"session":headers.get("mcp-session-id").and_then(|v|v.to_str().ok()),"version":headers.get("mcp-protocol-version").and_then(|v|v.to_str().ok())}));
            let result = match v["method"].as_str().unwrap() {
                "initialize" => json!({"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}),
                "tools/list" => json!({"tools":[{"name":"echo","description":"fixture","inputSchema":{"type":"object"}}]}),
                "tools/call" => {
                    if v["params"]["arguments"]["slow"] == true {
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    }
                    let response = json!({"jsonrpc":"2.0","id":v["id"],"result":{"content":[{"type":"text","text":"hello"}]}});
                    return ([("content-type", "text/event-stream")], format!("event: message\ndata: {response}\n\n")).into_response();
                }
                _ => return axum::http::StatusCode::ACCEPTED.into_response(),
            };
            ([("Mcp-Session-Id", "fixture-session")], axum::Json(json!({"jsonrpc":"2.0","id":v["id"],"result":result}))).into_response()
        }))
        .with_state((seen.clone(), cancelled.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let config: McpServer = serde_json::from_value(json!({"name":"fixture","url":url})).unwrap();
    let mut connection = mcp::Connection::start(&config, std::path::Path::new("/var/tmp")).await.unwrap();
    let defs = connection.tools("fixture").await.unwrap();
    assert_eq!(defs[0].name, "mcp__fixture__echo");
    let cancel = CancellationToken::new();
    let r = connection.invoke("echo", json!({}), &cancel, 10).await.unwrap();
    assert_eq!(r["content"][0]["text"], "hello");
    let stop = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        stop.cancel();
    });
    assert!(connection.invoke("echo", json!({"slow":true}), &cancel, 10).await.is_err());
    tokio::time::timeout(std::time::Duration::from_secs(1), cancelled.notified()).await.unwrap();
    let requests = seen.lock().unwrap();
    assert!(requests.iter().skip(1).all(|r| r["session"] == "fixture-session" && r["version"] == "2025-11-25"));
    assert_eq!(requests.iter().filter(|r| r["request"]["method"] == "tools/call").count(), 2);
    assert!(requests.iter().any(|r| r["request"]["method"] == "notifications/cancelled"));
    drop(requests);
    drop(connection);
    server.abort();
}

#[tokio::test]
async fn manual_operations_are_durable_without_orphan_provider_results() {
    let scratch = Scratch::new();
    let work = scratch.0.join("work");
    std::fs::create_dir(&work).unwrap();
    let e = Engine::open_at(scratch.0.join("state"), Some("http://127.0.0.1:1".into())).unwrap();
    let created = e.command(Command::Create { workspace: work.clone(), config: Some(Config { approval: ApprovalMode::Allow, ..Default::default() }) }).await.unwrap();
    let sid = created["id"].as_str().unwrap().to_string();
    e.command(Command::Tool { session: sid.clone(), name: "write_file".into(), arguments: json!({"path":"manual.txt","content":"durable"}) }).await.unwrap();
    e.quiesce().await;
    assert_eq!(std::fs::read_to_string(work.join("manual.txt")).unwrap(), "durable");
    let s = e.get(&sid).unwrap();
    assert!(!s.manual);
    assert!(s.in_flight.is_none());
    assert!(s.calls.is_empty());
    assert_eq!(s.context.len(), 1);
    assert_eq!(s.context[0]["role"], "user");
    assert_eq!(s.status, Status::Idle);
    let mut config = s.config;
    config.read_only = true;
    e.command(Command::Configure { session: sid.clone(), config }).await.unwrap();
    e.command(Command::Tool { session: sid.clone(), name: "write_file".into(), arguments: json!({"path":"denied.txt","content":"no"}) }).await.unwrap();
    e.quiesce().await;
    assert!(!work.join("denied.txt").exists());
    assert_eq!(e.get(&sid).unwrap().context.len(), 1);
}

#[test]
fn native_staging_honours_lfs_and_refuses_external_filters() {
    use std::sync::atomic::AtomicBool;
    let scratch = Scratch::new();
    gix::init(&scratch.0).unwrap();
    std::fs::write(scratch.0.join(".gitattributes"), "*.png filter=lfs -text\n*.danger filter=external\n*.txt text eol=lf\n").unwrap();
    std::fs::write(scratch.0.join("asset.png"), b"binary\0content").unwrap();
    std::fs::write(scratch.0.join("file.danger"), b"content").unwrap();
    std::fs::write(scratch.0.join("lines.txt"), b"one\r\ntwo\r\n").unwrap();
    let run = |paths: Value| git::run(&scratch.0, &call("repository_write", json!({"operation":"stage","paths":paths})), Arc::new(AtomicBool::new(false)));
    run(json!(["asset.png", "lines.txt"])).unwrap();
    let repo = gix::open(&scratch.0).unwrap();
    let index = repo.index().unwrap();
    use gix::bstr::ByteSlice;
    let asset = index.entry_by_path(b"asset.png".as_bstr()).unwrap();
    let object = repo.find_blob(asset.id).unwrap();
    let pointer = std::str::from_utf8(&object.data).unwrap();
    assert!(pointer.starts_with("version https://git-lfs.github.com/spec/v1\n"));
    assert!(pointer.contains("size 14"));
    let lines = index.entry_by_path(b"lines.txt".as_bstr()).unwrap();
    assert_eq!(repo.find_blob(lines.id).unwrap().data, b"one\ntwo\n");
    assert!(run(json!(["file.danger"])).unwrap_err().to_string().contains("external"));
    assert!(!scratch.0.join(".git/index.lock").exists());
}

#[test]
fn nested_files_deletion_checkpoints_and_recovery_preserve_newer_work() {
    let dir = Scratch::new();
    let db = Arc::new(Mutex::new(Store::open(&dir.0.join("state")).unwrap()));
    let mut s = Session::new(dir.0.clone(), Config::default());
    db.lock().unwrap().save(&mut s, "created", json!({})).unwrap();
    let cancel = CancellationToken::new();
    tools::file_tool(&s, &call("write_file", json!({"path":"src/deep/module.rs","content":"original\n"})), &db, &cancel).unwrap();
    assert!(tools::file_tool(&s, &call("delete_file", json!({"path":"src/deep/module.rs","expected":"stale"})), &db, &cancel).is_err());
    let deleted = tools::file_tool(&s, &call("delete_file", json!({"path":"src/deep/module.rs","expected":"original\n"})), &db, &cancel).unwrap();
    assert!(!dir.0.join("src/deep/module.rs").exists());
    let checkpoints = db.lock().unwrap().checkpoints(&s.id).unwrap();
    assert_eq!(checkpoints[0]["deleted"], true);
    assert_eq!(checkpoints[0]["id"], deleted["edit"]);
    let edit = deleted["edit"].as_i64().unwrap();
    std::fs::write(dir.0.join("src/deep/module.rs"), "newer work").unwrap();
    assert!(db.lock().unwrap().undo(&s.id, edit).is_err());
    std::fs::remove_file(dir.0.join("src/deep/module.rs")).unwrap();
    db.lock().unwrap().undo(&s.id, edit).unwrap();
    assert_eq!(std::fs::read_to_string(dir.0.join("src/deep/module.rs")).unwrap(), "original\n");
    assert!(db.lock().unwrap().undo(&s.id, edit).is_err());
    std::os::unix::fs::symlink("/var/tmp", dir.0.join("outside")).unwrap();
    assert!(tools::path(&dir.0, "outside/missing/new.rs", true).is_err());
    assert!(tools::path(&dir.0, "src/.git/objects/new", true).is_err());
    assert!(tools::path(&dir.0, "src/../escape/new", true).is_err());
}

#[test]
fn older_journal_migration_preserves_edits_and_indexes_history() {
    let dir = Scratch::new();
    let mut db = Store::open(&dir.0).unwrap();
    let mut s = Session::new(dir.0.clone(), Config::default());
    db.save(&mut s, "user", json!({"text":"migration marker"})).unwrap();
    let file = dir.0.join("existing");
    std::fs::write(&file, b"after").unwrap();
    let edit = db.edit(&s.id, &file, Some(b"before"), b"after").unwrap();
    drop(db);
    let old = rusqlite::Connection::open(dir.0.join("sessions.sqlite3")).unwrap();
    old.execute_batch("DROP TRIGGER transcript_insert; DROP TABLE transcript_search; DROP TABLE transcript; DROP TABLE compactions; DROP TRIGGER event_search_insert; DROP TABLE event_search; ALTER TABLE edits DROP COLUMN deleted; PRAGMA user_version=1;").unwrap();
    drop(old);
    let db = Store::open(&dir.0).unwrap();
    assert_eq!(db.search(&s.id, "migration marker").unwrap().len(), 1);
    db.undo(&s.id, edit).unwrap();
    assert_eq!(std::fs::read(file).unwrap(), b"before");
    drop(db);
    let db = Store::open(&dir.0).unwrap();
    assert_eq!(db.checkpoints(&s.id).unwrap()[0]["undone"], true);
}

#[tokio::test]
async fn upgrade_drains_approved_work_and_rejects_new_turns_without_cancelling() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    async fn request(path: &std::path::Path, command: Value) -> Value {
        let mut stream = tokio::net::UnixStream::connect(rpc::socket(path)).await.unwrap();
        stream.write_all(format!("{}\n", json!({"version":1,"command":command})).as_bytes()).await.unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).await.unwrap();
        serde_json::from_str(&line).unwrap()
    }
    let dir = Scratch::new();
    let path = dir.0.join("state");
    let (signal, upgrade) = tokio::sync::oneshot::channel();
    let service = tokio::spawn(rpc::serve_with_upgrade(path.clone(), Some(upgrade)));
    for _ in 0..100 {
        if rpc::socket(&path).exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let sid = request(&path, json!({"op":"create","workspace":dir.0,"config":{"approval":"ask"}})).await["result"]["id"].as_str().unwrap().to_owned();
    request(&path, json!({"op":"tool","session":sid,"name":"write_file","arguments":{"path":"survived","content":"exactly once"}})).await;
    let call = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let state = request(&path, json!({"op":"get","session":sid})).await;
            if let Some(call) = state["result"]["pending"]["call"]["id"].as_str() {
                break call.to_owned();
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    signal.send(()).unwrap();
    // A harmless read synchronizes with the service's upgrade notification.
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let rejected = request(&path, json!({"op":"send","session":sid,"text":"must not be queued"})).await;
    assert!(rejected["error"].as_str().unwrap().contains("upgrade"));
    assert!(!service.is_finished(), "pending approval must keep the service alive");
    request(&path, json!({"op":"approve","session":sid,"call_id":call,"allow":true})).await;
    assert!(tokio::time::timeout(std::time::Duration::from_secs(5), service).await.unwrap().unwrap().unwrap());
    assert_eq!(std::fs::read_to_string(dir.0.join("survived")).unwrap(), "exactly once");
    let db = Store::open(&path).unwrap();
    let s = db.get(&sid).unwrap();
    assert_eq!(s.status, Status::Idle);
    assert!(s.queued.is_empty());
    assert_eq!(db.checkpoints(&sid).unwrap().as_array().unwrap().len(), 1);
    assert!(!rpc::socket(&path).exists());
}

#[tokio::test]
async fn keep_going_can_pause_on_a_blocker_and_resume_with_evidence() {
    let dir = Scratch::new();
    let (fake, base, server) = server().await;
    let e = Engine::open_at(dir.0.join("state"), Some(base)).unwrap();
    let config = Config { keep_going: true, ..Config::default() };
    let sid = e.command(Command::Create { workspace: dir.0.clone(), config: Some(config) }).await.unwrap()["id"].as_str().unwrap().to_owned();
    e.command(Command::Goal { session: sid.clone(), objective: "Use the missing test fixture".into() }).await.unwrap();
    fake.replies.lock().unwrap().push_back(response(vec![tool("pause_work", json!({"reason":"The fixture is missing. Supply fixture.json to continue."}))]));
    e.command(Command::Send { session: sid.clone(), text: "Start".into() }).await.unwrap();
    finish(&e).await;
    let s = e.get(&sid).unwrap();
    assert_eq!(s.status, Status::Paused);
    assert_eq!(s.goal.unwrap().status, "blocked");
    assert_eq!(s.turns, 1, "a blocker must not burn the keep-going budget");
    assert!(s.in_flight.is_none());
    fake.replies.lock().unwrap().extend([
        response(vec![tool("complete_goal", json!({"evidence":"The person supplied the fixture; validation succeeded."}))]),
        response(vec![json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done."}]})]),
    ]);
    e.command(Command::Resume { session: sid.clone() }).await.unwrap();
    finish(&e).await;
    assert_eq!(e.get(&sid).unwrap().status, Status::Complete);
    server.abort();
}

#[test]
fn child_model_tuning_preserves_parent_restrictions() {
    let parent = Config { subagents: true, read_only: true, approval: ApprovalMode::Ask, denied_tools: vec!["execute".into()], ..Config::default() };
    let child = parent.child(false, &json!({"provider":"openai","read_only":false,"model_config":{"model":"custom-deployment","thinking":{"type":"enabled","budget_tokens":4096},"max_output_tokens":8192,"service_tier":"priority","extra":{"new_option":true}}})).unwrap();
    assert_eq!(child.model.provider, Provider::Openai);
    assert_eq!(child.model.model, "custom-deployment");
    assert_eq!(child.model.thinking.unwrap()["budget_tokens"], 4096);
    assert_eq!(child.model.max_output_tokens, 8192);
    assert_eq!(child.model.extra["new_option"], true);
    assert_eq!(child.approval, ApprovalMode::Ask);
    assert_eq!(child.denied_tools, parent.denied_tools);
    assert!(child.read_only);
    let defaults = parent.child(false, &json!({"provider":"openai"})).unwrap();
    assert_eq!(defaults.model.model, "custom-model");
    assert!(Config::default().child(true, &json!({"read_only":false})).unwrap().read_only);
    assert!(parent.child(false, &json!({"model_config":{"approval":"allow"}})).is_err());
}

#[test]
fn repository_status_exposes_stageable_files_inside_untracked_directories() {
    let dir = Scratch::new();
    gix::init(&dir.0).unwrap();
    std::fs::create_dir_all(dir.0.join("src/nested")).unwrap();
    std::fs::write(dir.0.join("src/nested/lib.rs"), "fn main() {}\n").unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let status = olo_coder_core::git::run(&dir.0, &call("repository_read", json!({"operation":"status"})), stop.clone()).unwrap();
    assert_eq!(status["files"][0]["path"], "src/nested/lib.rs");
    assert_eq!(status["truncated"], false);
    olo_coder_core::git::run(&dir.0, &call("repository_write", json!({"operation":"stage","paths":[status["files"][0]["path"]]})), stop).unwrap();
}

#[tokio::test]
async fn steering_modes_finish_skip_or_interrupt_the_correct_tool_batch() {
    for delivery in [Delivery::AfterTool, Delivery::Queue, Delivery::Interrupt] {
        let dir = Scratch::new();
        let (fake, base, server) = server().await;
        let e = Engine::open_at(dir.0.join("state"), Some(base)).unwrap();
        let sid =
            e.command(Command::Create { workspace: dir.0.clone(), config: Some(Config { approval: ApprovalMode::Allow, ..Default::default() }) }).await.unwrap()["id"].as_str().unwrap().to_owned();
        let mut second = tool("write_file", json!({"path":"stale.txt","content":"old plan"}));
        second["call_id"] = json!("call2");
        fake.replies.lock().unwrap().push_back(response(vec![tool("execute", json!({"program":"/usr/bin/sleep","args":["0.2"]})), second]));
        let mut events = e.events.subscribe();
        e.command(Command::Send { session: sid.clone(), text: "Initial task".into() }).await.unwrap();
        loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv()).await.unwrap().unwrap();
            if event.kind == "tool_started" {
                break;
            }
        }
        let receipt = e.command(Command::Steer { session: sid.clone(), text: "STEERING: use the new plan".into(), delivery, message_id: None }).await.unwrap();
        finish(&e).await;
        assert_eq!(dir.0.join("stale.txt").exists(), delivery == Delivery::Queue, "{delivery:?}");
        assert_eq!(e.command(Command::Receipt { message: receipt["message"]["id"].as_str().unwrap().into() }).await.unwrap()["state"], "delivered");
        let requests = fake.requests.lock().unwrap();
        let after = &requests[if delivery == Delivery::Queue { 2 } else { 1 }].1["input"];
        assert!(after.to_string().contains("STEERING"));
        for call in ["call1", "call2"] {
            assert!(after.as_array().unwrap().iter().any(|v| v["type"] == "function_call_output" && v["call_id"] == call));
        }
        if delivery == Delivery::Queue {
            assert!(!requests[1].1["input"].to_string().contains("STEERING"));
        }
        assert_eq!(e.get(&sid).unwrap().status, Status::Idle);
        server.abort();
    }
}

#[tokio::test]
async fn pinned_catalog_and_inbox_are_durable_and_idempotent() {
    let dir = Scratch::new();
    let e = Engine::open(dir.0.join("state")).unwrap();
    let mut ids = Vec::new();
    for _ in 0..2 {
        ids.push(e.command(Command::Create { workspace: dir.0.clone(), config: None }).await.unwrap()["id"].as_str().unwrap().to_owned());
    }
    e.command(Command::Organize { session: ids[0].clone(), title: Some("Compiler work".into()), pinned: Some(true), archived: None, tags: Some(vec!["rust".into()]) }).await.unwrap();
    let rows = e.command(Command::Browse { filter: SessionFilter { query: "rust".into(), ..Default::default() } }).await.unwrap();
    assert_eq!(rows["sessions"][0]["id"], ids[0]);
    let message_id = id();
    let command = Command::SendMessage {
        session: ids[1].clone(),
        target: ids[0].clone(),
        peer: None,
        text: "Please review the ABI".into(),
        delivery: Delivery::Queue,
        wake: true,
        message_id: Some(message_id.clone()),
    };
    e.command(command.clone()).await.unwrap();
    assert_eq!(e.get(&ids[0]).unwrap().status, Status::Idle, "incoming wake is off by default");
    e.command(Command::EditMessage { session: ids[0].clone(), message: message_id.clone(), text: Some("Review the ABI after tests".into()), delivery: None }).await.unwrap();
    e.command(command).await.unwrap();
    assert_eq!(e.get(&ids[0]).unwrap().inbox.len(), 1, "retry deduplicates the original request even after recipient edits it");
    assert!(e.command(Command::Organize { session: ids[0].clone(), title: None, pinned: None, archived: Some(true), tags: None }).await.is_err());
    e.command(Command::CancelMessage { session: ids[0].clone(), message: message_id.clone() }).await.unwrap();
    assert_eq!(e.command(Command::Receipt { message: message_id }).await.unwrap()["state"], "cancelled");
    e.command(Command::Organize { session: ids[0].clone(), title: None, pinned: None, archived: Some(true), tags: None }).await.unwrap();
    drop(e);
    let e = Engine::open(dir.0.join("state")).unwrap();
    assert!(e.get(&ids[0]).unwrap().pinned);
    assert_eq!(e.command(Command::Browse { filter: Default::default() }).await.unwrap()["sessions"].as_array().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn linked_instances_exchange_and_retry_messages_without_duplicate_delivery() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    async fn rpc_at(path: &std::path::Path, cmd: Value) -> Value {
        let mut stream = tokio::net::UnixStream::connect(rpc::socket(path)).await.unwrap();
        stream.write_all(format!("{cmd}\n").as_bytes()).await.unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).await.unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    }
    let dir = Scratch::new();
    let a = dir.0.join("a");
    let b = dir.0.join("b");
    let sa = tokio::spawn(rpc::serve(a.clone()));
    let sb = tokio::spawn(rpc::serve(b.clone()));
    for _ in 0..200 {
        if rpc::socket(&a).exists() && rpc::socket(&b).exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let from = rpc_at(&a, json!({"op":"create","workspace":dir.0})).await["result"]["id"].as_str().unwrap().to_owned();
    let to = rpc_at(&b, json!({"op":"create","workspace":dir.0})).await["result"]["id"].as_str().unwrap().to_owned();
    let linked = rpc_at(&a, json!({"op":"link_peer","name":"review","directory":b,"reciprocal":true})).await;
    assert!(linked.get("error").is_none(), "{linked}");
    let mid = id();
    let send = json!({"op":"send_message","session":from,"target":to,"peer":"review","text":"Review these findings","message_id":mid});
    let first = rpc_at(&a, send.clone()).await;
    assert_eq!(first["result"]["state"], "queued", "{first}");
    rpc_at(&a, json!({"op":"retry_message","session":from,"message":mid})).await;
    let state = rpc_at(&b, json!({"op":"get","session":to})).await;
    assert_eq!(state["result"]["inbox"].as_array().unwrap().len(), 1);
    assert_eq!(state["result"]["inbox"][0]["message"]["from"]["session"], from);
    assert_eq!(state["result"]["status"], "idle");
    rpc_at(&b, json!({"op":"shutdown"})).await;
    sb.await.unwrap().unwrap();
    let offline_id = id();
    let failed = rpc_at(&a, json!({"op":"send_message","session":from,"target":to,"peer":"review","text":"Retained while offline","message_id":offline_id})).await;
    assert!(failed.get("error").is_some());
    assert_eq!(rpc_at(&a, json!({"op":"receipt","message":offline_id})).await["result"]["state"], "uncertain");
    let sb = tokio::spawn(rpc::serve(b.clone()));
    for _ in 0..200 {
        if rpc::socket(&b).exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let retry = rpc_at(&a, json!({"op":"retry_message","session":from,"message":offline_id})).await;
    assert_eq!(retry["result"]["state"], "queued", "{retry}");
    assert_eq!(rpc_at(&b, json!({"op":"get","session":to})).await["result"]["inbox"].as_array().unwrap().len(), 2);
    rpc_at(&a, json!({"op":"shutdown"})).await;
    rpc_at(&b, json!({"op":"shutdown"})).await;
    sa.await.unwrap().unwrap();
    sb.await.unwrap().unwrap();
}

#[tokio::test]
async fn steering_during_manual_work_is_delivered_and_stop_preserves_the_inbox() {
    for delivery in [Delivery::AfterTool, Delivery::Queue, Delivery::Interrupt] {
        let dir = Scratch::new();
        let (fake, base, server) = server().await;
        let e = Engine::open_at(dir.0.join("state"), Some(base)).unwrap();
        let sid =
            e.command(Command::Create { workspace: dir.0.clone(), config: Some(Config { approval: ApprovalMode::Allow, ..Default::default() }) }).await.unwrap()["id"].as_str().unwrap().to_owned();
        let mut events = e.events.subscribe();
        e.command(Command::Tool { session: sid.clone(), name: "execute".into(), arguments: json!({"program":"/usr/bin/sleep","args":["0.2"]}) }).await.unwrap();
        loop {
            if events.recv().await.unwrap().kind == "tool_started" {
                break;
            }
        }
        e.command(Command::Steer { session: sid.clone(), text: "Follow up after manual work".into(), delivery, message_id: None }).await.unwrap();
        finish(&e).await;
        let s = e.get(&sid).unwrap();
        assert_eq!(s.status, Status::Idle, "{delivery:?}");
        assert!(s.inbox.is_empty());
        assert!(!s.manual);
        assert!(fake.requests.lock().unwrap().iter().any(|r| r.1["input"].to_string().contains("Follow up after manual work")));
        server.abort();
    }
}

#[tokio::test]
async fn inbox_reordering_limits_and_recipient_policy_are_enforced() {
    let dir = Scratch::new();
    let e = Engine::open(dir.0.join("state")).unwrap();
    let a = e.command(Command::Create { workspace: dir.0.clone(), config: None }).await.unwrap()["id"].as_str().unwrap().to_owned();
    let b = e.command(Command::Create { workspace: dir.0.clone(), config: None }).await.unwrap()["id"].as_str().unwrap().to_owned();
    let mut ids = Vec::new();
    for text in ["one", "two", "three"] {
        let receipt = e.command(Command::SendMessage { session: a.clone(), target: b.clone(), peer: None, text: text.into(), delivery: Delivery::Queue, wake: false, message_id: None }).await.unwrap();
        ids.push(receipt["message"]["id"].as_str().unwrap().to_owned());
    }
    e.command(Command::MoveMessage { session: b.clone(), message: ids[2].clone(), before: Some(ids[0].clone()) }).await.unwrap();
    assert_eq!(e.get(&b).unwrap().inbox.iter().map(|p| p.message.text.as_str()).collect::<Vec<_>>(), ["three", "one", "two"]);
    assert!(e.command(Command::MoveMessage { session: b.clone(), message: ids[2].clone(), before: Some("missing".into()) }).await.is_err());
    assert_eq!(e.get(&b).unwrap().inbox.len(), 3);
    let mut config = e.get(&b).unwrap().config;
    config.accept_messages = false;
    e.command(Command::Configure { session: b.clone(), config }).await.unwrap();
    assert!(e.command(Command::SendMessage { session: a, target: b.clone(), peer: None, text: "four".into(), delivery: Delivery::Queue, wake: false, message_id: None }).await.is_err());
    e.command(Command::Organize { session: b.clone(), title: None, pinned: Some(true), archived: None, tags: None }).await.unwrap();
    let rows = e.command(Command::Browse { filter: Default::default() }).await.unwrap();
    assert_eq!(rows["sessions"][0]["pinned"], true);
    assert!(e.command(Command::Browse { filter: SessionFilter { offset: usize::MAX, ..Default::default() } }).await.is_err());
}

#[tokio::test]
async fn stop_holds_queued_messages_until_explicit_resume() {
    let dir = Scratch::new();
    let (fake, base, server) = server().await;
    let e = Engine::open_at(dir.0.join("state"), Some(base)).unwrap();
    let sid = e.command(Command::Create { workspace: dir.0.clone(), config: Some(Config { approval: ApprovalMode::Allow, ..Default::default() }) }).await.unwrap()["id"].as_str().unwrap().to_owned();
    fake.replies.lock().unwrap().push_back(response(vec![tool("execute", json!({"program":"/usr/bin/sleep","args":["30"]}))]));
    let mut events = e.events.subscribe();
    e.command(Command::Send { session: sid.clone(), text: "Initial".into() }).await.unwrap();
    loop {
        if events.recv().await.unwrap().kind == "tool_started" {
            break;
        }
    }
    let receipt = e.command(Command::Steer { session: sid.clone(), text: "Saved for later".into(), delivery: Delivery::Queue, message_id: None }).await.unwrap();
    e.command(Command::Stop { session: sid.clone() }).await.unwrap();
    finish(&e).await;
    assert_eq!(e.get(&sid).unwrap().status, Status::Paused);
    assert_eq!(e.get(&sid).unwrap().inbox.len(), 1);
    assert_eq!(fake.requests.lock().unwrap().len(), 1);
    assert_eq!(e.command(Command::Receipt { message: receipt["message"]["id"].as_str().unwrap().into() }).await.unwrap()["state"], "queued");
    e.command(Command::Resume { session: sid.clone() }).await.unwrap();
    finish(&e).await;
    assert_eq!(e.get(&sid).unwrap().status, Status::Idle);
    assert!(e.get(&sid).unwrap().inbox.is_empty());
    assert!(fake.requests.lock().unwrap()[1].1["input"].to_string().contains("Saved for later"));
    server.abort();
}

#[tokio::test]
async fn interrupt_replaces_unapproved_calls_without_executing_them() {
    let dir = Scratch::new();
    let (fake, base, server) = server().await;
    let e = Engine::open_at(dir.0.join("state"), Some(base)).unwrap();
    let sid = e.command(Command::Create { workspace: dir.0.clone(), config: Some(Config { approval: ApprovalMode::Ask, ..Default::default() }) }).await.unwrap()["id"].as_str().unwrap().to_owned();
    fake.replies.lock().unwrap().push_back(response(vec![tool("write_file", json!({"path":"must-not-exist","content":"stale"}))]));
    let mut events = e.events.subscribe();
    e.command(Command::Send { session: sid.clone(), text: "Initial".into() }).await.unwrap();
    loop {
        if events.recv().await.unwrap().kind == "approval" {
            break;
        }
    }
    e.command(Command::Steer { session: sid.clone(), text: "Do something else".into(), delivery: Delivery::Interrupt, message_id: None }).await.unwrap();
    finish(&e).await;
    let s = e.get(&sid).unwrap();
    assert_eq!(s.status, Status::Idle);
    assert!(s.pending.is_none());
    assert!(!dir.0.join("must-not-exist").exists());
    assert!(s.calls.is_empty());
    let input = fake.requests.lock().unwrap()[1].1["input"].clone();
    assert!(input.to_string().contains("Skipped before execution"));
    assert!(input.to_string().contains("Do something else"));
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_an_unconfirmed_peer_send_is_prompt_and_retryable() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let dir = Scratch::new();
    let peer = dir.0.join("peer");
    std::fs::create_dir(&peer).unwrap();
    let listener = tokio::net::UnixListener::bind(peer.join("service.sock")).unwrap();
    let remote = id();
    let release = tokio_util::sync::CancellationToken::new();
    let close = release.clone();
    let server = tokio::spawn(async move {
        loop {
            let (stream, _) = tokio::select! {_ = close.cancelled()=>break,r=listener.accept()=>r.unwrap()};
            let remote = remote.clone();
            let close = close.clone();
            tokio::spawn(async move {
                let mut stream = BufReader::new(stream);
                let mut line = String::new();
                stream.read_line(&mut line).await.unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                if request["command"]["op"] == "info" {
                    stream.get_mut().write_all(format!("{}\n", json!({"result":{"instance":remote}})).as_bytes()).await.unwrap();
                } else {
                    close.cancelled().await;
                }
            });
        }
    });
    let e = Engine::open_at(dir.0.join("state"), Some("http://127.0.0.1:1".into())).unwrap();
    let sid = e.command(Command::Create { workspace: dir.0.clone(), config: Some(Config { approval: ApprovalMode::Allow, ..Default::default() }) }).await.unwrap()["id"].as_str().unwrap().to_owned();
    e.command(Command::LinkPeer { name: "slow".into(), directory: peer, reciprocal: false }).await.unwrap();
    let message = id();
    let mut events = e.events.subscribe();
    e.command(Command::Tool { session: sid.clone(), name: "send_message".into(), arguments: json!({"peer":"slow","target":id(),"text":"hello","message_id":message}) }).await.unwrap();
    loop {
        if events.recv().await.unwrap().kind == "message_outgoing" {
            break;
        }
    }
    e.command(Command::Stop { session: sid.clone() }).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), e.quiesce()).await.unwrap();
    assert_eq!(e.command(Command::Receipt { message }).await.unwrap()["state"], "uncertain");
    assert_eq!(e.get(&sid).unwrap().status, Status::Paused);
    release.cancel();
    server.await.unwrap();
}

#[test]
fn restart_marks_unconfirmed_outgoing_messages_for_safe_retry() {
    let dir = Scratch::new();
    let state = dir.0.join("state");
    let e = Engine::open(state.clone()).unwrap();
    drop(e);
    let instance: String = serde_json::from_slice(&std::fs::read(state.join("instance.json")).unwrap()).unwrap();
    let mut db = Store::open(&state).unwrap();
    let mut session = Session::new(dir.0.clone(), Default::default());
    db.save(&mut session, "created", json!({})).unwrap();
    let message = Message {
        id: id(),
        from: Some(Sender { instance, session: session.id.clone(), title: "Sender".into() }),
        to_instance: id(),
        to: id(),
        text: "Saved before transmission".into(),
        delivery: Delivery::Queue,
        created: now(),
        wake: false,
    };
    db.save(&mut session, "message_outgoing", json!({"message":message})).unwrap();
    drop(db);
    let e = Engine::open(state.clone()).unwrap();
    drop(e);
    let db = Store::open(&state).unwrap();
    assert_eq!(db.receipt(&message.id).unwrap().unwrap()["state"], "uncertain");
    assert_eq!(db.original_message(&message.id).unwrap().unwrap(), message);
}

#[tokio::test]
async fn a_manual_action_does_not_wake_older_collaborator_messages() {
    let dir = Scratch::new();
    let (fake, base, server) = server().await;
    let e = Engine::open_at(dir.0.join("state"), Some(base)).unwrap();
    let config = Config { approval: ApprovalMode::Allow, ..Default::default() };
    let a = e.command(Command::Create { workspace: dir.0.clone(), config: Some(config.clone()) }).await.unwrap()["id"].as_str().unwrap().to_owned();
    let b = e.command(Command::Create { workspace: dir.0.clone(), config: Some(config) }).await.unwrap()["id"].as_str().unwrap().to_owned();
    for delivery in [Delivery::Queue, Delivery::AfterTool, Delivery::Interrupt] {
        e.command(Command::SendMessage { session: a.clone(), target: b.clone(), peer: None, text: "Wait for an explicit model turn".into(), delivery, wake: false, message_id: None }).await.unwrap();
    }
    e.command(Command::Tool { session: b.clone(), name: "write_file".into(), arguments: json!({"path":"manual.txt","content":"manual action only"}) }).await.unwrap();
    finish(&e).await;
    let s = e.get(&b).unwrap();
    assert_eq!(s.status, Status::Idle);
    assert_eq!(s.inbox.len(), 3);
    assert!(fake.requests.lock().unwrap().is_empty());
    assert_eq!(std::fs::read_to_string(dir.0.join("manual.txt")).unwrap(), "manual action only");
    e.command(Command::Resume { session: b.clone() }).await.unwrap();
    finish(&e).await;
    assert_eq!(e.get(&b).unwrap().inbox.len(), 0);
    assert_eq!(fake.requests.lock().unwrap().len(), 1);
    server.abort();
}

#[test]
fn pre_coordination_stores_upgrade_without_losing_provider_context() {
    let dir = Scratch::new();
    let mut db = Store::open(&dir.0).unwrap();
    let mut s = Session::new(dir.0.clone(), Default::default());
    s.context = vec![json!({"type":"reasoning","encrypted_content":"opaque-preserved"})];
    db.save(&mut s, "created", json!({})).unwrap();
    drop(db);
    let mut old = serde_json::to_value(&s).unwrap();
    for key in ["pinned", "archived", "tags", "inbox", "tool_sequence", "interrupt_requested", "continue_requested"] {
        old.as_object_mut().unwrap().remove(key);
    }
    old["config"].as_object_mut().unwrap().remove("accept_messages");
    old["config"].as_object_mut().unwrap().remove("wake_on_message");
    let db = rusqlite::Connection::open(dir.0.join("sessions.sqlite3")).unwrap();
    db.execute("UPDATE sessions SET body=? WHERE id=?", rusqlite::params![old.to_string(), s.id]).unwrap();
    db.execute_batch("DROP TRIGGER transcript_insert; DROP TABLE transcript_search; DROP TABLE transcript; DROP TABLE compactions; DROP TABLE messages; PRAGMA user_version=3;").unwrap();
    drop(db);
    let e = Engine::open(dir.0.clone()).unwrap();
    let upgraded = e.get(&s.id).unwrap();
    assert_eq!(upgraded.context, s.context);
    assert!(!upgraded.continue_requested);
    assert!(!upgraded.pinned);
    assert!(upgraded.inbox.is_empty());
    assert!(upgraded.config.accept_messages);
    assert!(!upgraded.config.wake_on_message);
}

#[tokio::test]
async fn audit_cancelled_process_never_starts_or_creates_an_artifact() {
    let dir = Scratch::new();
    let s = Session::new(dir.0.clone(), Config::default());
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = tools::execute(&s, &call("execute", json!({"program":"/usr/bin/true"})), &dir.0, &cancel, Arc::new(|_, _| {})).await;
    assert!(result.is_err());
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0, "cancelled work must not spawn or allocate output artifacts");
}

#[test]
fn audit_event_stream_disconnect_is_not_success() {
    use std::io::{BufRead, Write};
    for frame in [json!({"ready":true,"after":0}), json!({"reconnect":true,"after":4})] {
        let dir = Scratch::new();
        let listener = std::os::unix::net::UnixListener::bind(rpc::socket(&dir.0)).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            std::io::BufReader::new(stream.try_clone().unwrap()).read_line(&mut request).unwrap();
            writeln!(stream, "{frame}").unwrap();
        });
        let result = rpc::follow(&dir.0, "session", 0, |_| true);
        server.join().unwrap();
        assert!(result.is_err(), "a lost subscription cannot imply task completion");
    }
}

#[tokio::test]
async fn audit_mcp_discovery_cancels_and_manual_tools_have_parity() {
    use axum::{extract::State, response::IntoResponse};
    let listing = Arc::new(tokio::sync::Notify::new());
    let observed = listing.clone();
    let app = axum::Router::new()
        .fallback(axum::routing::post(|State(listing): State<Arc<tokio::sync::Notify>>, axum::Json(v): axum::Json<Value>| async move {
            let result = match v["method"].as_str().unwrap() {
                "initialize" => json!({"protocolVersion":"2025-11-25","capabilities":{"tools":{}}}),
                "tools/list" => {
                    listing.notify_one();
                    std::future::pending::<()>().await;
                    unreachable!()
                }
                "tools/call" => json!({"content":[{"type":"text","text":"manual echo"}]}),
                _ => return axum::http::StatusCode::ACCEPTED.into_response(),
            };
            axum::Json(json!({"jsonrpc":"2.0","id":v["id"],"result":result})).into_response()
        }))
        .with_state(listing);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = Scratch::new();
    let e = Engine::open_at(dir.0.join("state"), Some("http://127.0.0.1:1".into())).unwrap();
    let config = Config { approval: ApprovalMode::Allow, mcp: vec![serde_json::from_value(json!({"name":"fixture","url":url})).unwrap()], ..Default::default() };
    let sid = e.command(Command::Create { workspace: dir.0.clone(), config: Some(config) }).await.unwrap()["id"].as_str().unwrap().to_owned();
    e.command(Command::Send { session: sid.clone(), text: "Start discovery".into() }).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), observed.notified()).await.unwrap();
    let began = std::time::Instant::now();
    e.command(Command::Stop { session: sid.clone() }).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), e.quiesce()).await.expect("Stop must cancel MCP discovery immediately");
    println!("MCP discovery cancellation: {:?}", began.elapsed());
    assert_eq!(e.get(&sid).unwrap().status, Status::Paused);
    // Direct manual calls need only initialization; they do not enumerate all
    // configured servers or start a model turn.
    e.command(Command::Tool { session: sid.clone(), name: "mcp__fixture__echo".into(), arguments: json!({}) }).await.unwrap();
    finish(&e).await;
    let events = e.store.lock().unwrap().events(&sid, 0, 100).unwrap();
    let result = events.iter().rev().find(|e| e.kind == "tool_result").unwrap();
    assert_eq!(result.data["error"], false, "{}", result.data);
    assert_eq!(result.data["output"]["content"][0]["text"], "manual echo");
    assert_eq!(e.get(&sid).unwrap().usage.input, 0);
    assert!(e.command(Command::Tool { session: sid, name: "mcp__absent__echo".into(), arguments: json!({}) }).await.is_err());
    server.abort();
}

#[tokio::test]
async fn audit_parent_stop_clears_descendant_restart_intent_and_shutdown_rejects_work() {
    let dir = Scratch::new();
    let e = Engine::open_at(dir.0.join("state"), Some("http://127.0.0.1:1".into())).unwrap();
    let root = e.command(Command::Create { workspace: dir.0.clone(), config: None }).await.unwrap()["id"].as_str().unwrap().to_owned();
    let mut child = Session::new(dir.0.clone(), Config::default());
    child.parent = Some(root.clone());
    child.interrupt_requested = true;
    child.continue_requested = true;
    let mut grandchild = child.clone();
    grandchild.id = id();
    grandchild.parent = Some(child.id.clone());
    let mut completed = Session::new(dir.0.clone(), Config::default());
    completed.parent = Some(root.clone());
    completed.status = Status::Complete;
    for s in [&mut child, &mut grandchild, &mut completed] {
        e.store.lock().unwrap().save(s, "created", json!({})).unwrap();
    }
    e.command(Command::Stop { session: root.clone() }).await.unwrap();
    for id in [&root, &child.id, &grandchild.id] {
        let s = e.get(id).unwrap();
        assert_eq!(s.status, Status::Paused);
        assert!(!s.interrupt_requested && !s.continue_requested);
    }
    assert_eq!(e.get(&completed.id).unwrap().status, Status::Complete);
    e.command(Command::Shutdown).await.unwrap();
    assert!(e.command(Command::Send { session: root.clone(), text: "Must not start".into() }).await.is_err());
    assert!(e.get(&root).unwrap().inbox.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_failed_checkpoints_do_not_spin_or_execute_pending_work() {
    let dir = Scratch::new();
    let e = Engine::open_at(dir.0.join("state"), Some("http://127.0.0.1:1".into())).unwrap();
    let sid = e.command(Command::Create { workspace: dir.0.clone(), config: None }).await.unwrap()["id"].as_str().unwrap().to_owned();
    let db = rusqlite::Connection::open(dir.0.join("state/sessions.sqlite3")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_checkpoints BEFORE INSERT ON events WHEN NEW.kind != 'message_queued' BEGIN SELECT RAISE(ABORT,'injected storage failure'); END;").unwrap();
    let mut events = e.events.subscribe();
    e.command(Command::Steer { session: sid.clone(), text: "Do not lose this input".into(), delivery: Delivery::Interrupt, message_id: None }).await.unwrap();
    let stopped = tokio::time::timeout(std::time::Duration::from_secs(1), e.quiesce()).await;
    // Ensure a broken supervisor can still be shut down by the fixture before
    // asserting, instead of leaving an unyielding task behind on test failure.
    e.shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(1), e.quiesce()).await.unwrap();
    assert!(stopped.is_ok(), "failed checkpoint writes must not cause an interrupt/restart loop");
    let session = e.get(&sid).unwrap();
    assert_eq!(session.inbox.len(), 1);
    assert!(session.context.is_empty());
    assert_eq!(session.turns, 0);
    let mut reported = false;
    while let Ok(event) = events.try_recv() {
        if event.kind == "error" && event.seq == 0 {
            reported = event.data["message"].as_str().unwrap().contains("checkpoint could not be saved");
        }
    }
    assert!(reported, "storage failures must be visible even if the journal cannot be written");
}

#[tokio::test]
async fn audit_stop_still_cancels_active_process_when_checkpoint_writes_fail() {
    let dir = Scratch::new();
    let e = Engine::open_at(dir.0.join("state"), Some("http://127.0.0.1:1".into())).unwrap();
    let sid = e.command(Command::Create { workspace: dir.0.clone(), config: Some(Config { approval: ApprovalMode::Allow, ..Default::default() }) }).await.unwrap()["id"].as_str().unwrap().to_owned();
    let mut events = e.events.subscribe();
    e.command(Command::Tool { session: sid.clone(), name: "execute".into(), arguments: json!({"program":"/usr/bin/sleep","args":["30"]}) }).await.unwrap();
    loop {
        if tokio::time::timeout(std::time::Duration::from_secs(2), events.recv()).await.unwrap().unwrap().kind == "tool_started" {
            break;
        }
    }
    let db = rusqlite::Connection::open(dir.0.join("state/sessions.sqlite3")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_stops BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT,'injected storage failure'); END;").unwrap();
    let stopped = e.command(Command::Stop { session: sid.clone() }).await;
    assert!(stopped.is_err());
    tokio::time::timeout(std::time::Duration::from_secs(1), e.quiesce()).await.expect("cancellation must not depend on journal writes succeeding");
    assert!(e.get(&sid).unwrap().in_flight.is_some(), "a failed result checkpoint must remain an explicitly unknown outcome");
}

#[test]
fn durable_receipt_confirmation_survives_a_late_failed_attempt() {
    let dir = Scratch::new();
    let mut db = Store::open(&dir.0).unwrap();
    let mut session = Session::new(dir.0.clone(), Config::default());
    let message = Message {
        id: id(),
        from: Some(Sender { instance: id(), session: session.id.clone(), title: "sender".into() }),
        to_instance: id(),
        to: id(),
        text: "Review".into(),
        delivery: Delivery::Queue,
        created: now(),
        wake: false,
    };
    db.save(&mut session, "message_outgoing", json!({"message":message})).unwrap();
    db.save(&mut session, "message_sent", json!({"id":message.id})).unwrap();
    db.save(&mut session, "message_uncertain", json!({"id":message.id,"error":"older request lost its connection"})).unwrap();
    assert_eq!(db.receipt(&message.id).unwrap().unwrap()["state"], "sent", "a late failed attempt cannot erase confirmed acceptance");
    drop(db);
    assert_eq!(Store::open(&dir.0).unwrap().receipt(&message.id).unwrap().unwrap()["state"], "sent");
}

#[tokio::test]
async fn durable_context_survives_incomplete_compaction() {
    let dir = Scratch::new();
    let (fake, base, server) = server().await;
    let e = Engine::open_at(dir.0.join("state"), Some(base)).unwrap();
    let sid = e.command(Command::Create { workspace: dir.0.clone(), config: None }).await.unwrap()["id"].as_str().unwrap().to_owned();
    let mut session = e.get(&sid).unwrap();
    session.context = vec![json!({"type":"reasoning","encrypted_content":"must-survive","future_field":17}), json!({"role":"user","content":"Keep the decision."})];
    e.store.lock().unwrap().save(&mut session, "fixture", json!({})).unwrap();
    for response in [
        json!({"status":"incomplete","output":[{"type":"compaction","encrypted_content":"unfinished-window"}]}),
        json!({"status":"completed","output":[{"type":"compaction","encrypted_content":""}]}),
        json!({"status":"completed","output":[{"type":"compaction","encrypted_content":"window"},{"type":"function_call","call_id":"unexecuted","name":"write_file","arguments":"{}"}]}),
    ] {
        fake.replies.lock().unwrap().push_back(response);
        e.command(Command::Compact { session: sid.clone() }).await.unwrap();
        finish(&e).await;
        let after = e.get(&sid).unwrap();
        assert_eq!(after.context, session.context, "failed compaction must never replace the saved provider context");
        assert_eq!(after.compactions, 0);
        assert_eq!(after.status, Status::Failed);
    }
    drop(e);
    assert_eq!(Engine::open(dir.0.join("state")).unwrap().get(&sid).unwrap().context, session.context);
    server.abort();
}

#[test]
#[ignore = "release performance measurement; fixed 4096-event provider batch"]
fn provider_stream_batch_benchmark() {
    let payload = format!("héllo {}", "x".repeat(180));
    let wire = format!("data: {}\r\n\r\n", json!({"text":payload})).repeat(4096);
    let mut samples = vec![];
    for _ in 0..9 {
        let began = std::time::Instant::now();
        let events = Sse::default().push(wire.as_bytes()).unwrap();
        samples.push(began.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(events.len(), 4096);
        assert_eq!(serde_json::from_str::<Value>(&events[4095]).unwrap()["text"], payload);
    }
    samples.sort_by(f64::total_cmp);
    println!("SSE batch: {} bytes, 4096 events; median {:.3} ms; min {:.3} ms; max {:.3} ms", wire.len(), samples[4], samples[0], samples[8]);
}

#[test]
fn fragmented_long_sse_lines_preserve_every_event() {
    let payload = "héllo 🌿".repeat(1024);
    let expected = json!({"text":payload}).to_string();
    let wire = format!(": heartbeat\r\nevent: delta\r\ndata: {expected}\r\n\r\n").repeat(3);
    for size in [1, 2, 3, 7, 31, 4096, wire.len()] {
        let mut sse = Sse::default();
        let mut events = vec![];
        for chunk in wire.as_bytes().chunks(size) {
            events.extend(sse.push(chunk).unwrap());
        }
        assert_eq!(events, vec![expected.clone(); 3], "chunk size {size}");
        assert_eq!(sse.push(b"data: next\n\n").unwrap(), vec!["next"]);
    }
}

#[test]
fn malformed_provider_calls_cannot_enter_the_tool_loop() {
    for p in [Provider::Codex, Provider::Openai] {
        for (ids, args) in [(vec!["duplicate", "duplicate"], json!({})), (vec![""], json!({})), (vec!["one"], json!([1, 2])), (vec!["one"], Value::Null)] {
            let v = match p {
                Provider::Codex => {
                    json!({"status":"completed","output":ids.iter().map(|id|json!({"type":"function_call","call_id":id,"name":"execute","arguments":args.to_string()})).collect::<Vec<_>>()})
                }
                Provider::Openai => {
                    json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","tool_calls":ids.iter().map(|id|json!({"type":"function","id":id,"function":{"name":"execute","arguments":args.to_string()}})).collect::<Vec<_>>()}}]})
                }
            };
            assert!(provider::parse(p, v).is_err());
        }
    }
    assert!(provider::parse(Provider::Openai, json!({"content":[]})).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_receipts_are_validated_before_delivery_is_confirmed() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let dir = Scratch::new();
    let peer = dir.0.join("peer");
    std::fs::create_dir(&peer).unwrap();
    let listener = tokio::net::UnixListener::bind(peer.join("service.sock")).unwrap();
    let remote = id();
    let mode = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let behavior = mode.clone();
    let server = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let remote = remote.clone();
            let mode = behavior.load(std::sync::atomic::Ordering::SeqCst);
            tokio::spawn(async move {
                let mut stream = BufReader::new(stream);
                let mut line = String::new();
                stream.read_line(&mut line).await.unwrap();
                let req: Value = serde_json::from_str(&line).unwrap();
                let result = if req["command"]["op"] == "info" {
                    json!({"instance":remote})
                } else {
                    let mut message = req["command"]["message"].clone();
                    match mode {
                        0 => message["id"] = json!(id()),
                        1 => message["to"] = json!(id()),
                        2 => message["from"]["session"] = json!(id()),
                        _ => message["text"] = json!("Edited at recipient"),
                    };
                    json!({"sequence":1,"message":message,"state":if mode==3{"sent"}else{"queued"}})
                };
                stream.get_mut().write_all(format!("{}\n", json!({"result":result})).as_bytes()).await.unwrap();
            });
        }
    });
    let e = Engine::open(dir.0.join("state")).unwrap();
    let sid = e.command(Command::Create { workspace: dir.0.clone(), config: None }).await.unwrap()["id"].as_str().unwrap().to_owned();
    e.command(Command::LinkPeer { name: "test".into(), directory: peer, reciprocal: false }).await.unwrap();
    for i in 0..5 {
        mode.store(i, std::sync::atomic::Ordering::SeqCst);
        let mid = id();
        let result = e
            .command(Command::SendMessage {
                session: sid.clone(),
                target: id(),
                peer: Some("test".into()),
                text: "Original message".into(),
                delivery: Delivery::Queue,
                wake: false,
                message_id: Some(mid.clone()),
            })
            .await;
        assert_eq!(result.is_ok(), i == 4, "{result:?}");
        assert_eq!(e.command(Command::Receipt { message: mid }).await.unwrap()["state"], if i == 4 { "sent" } else { "uncertain" });
    }
    server.abort();
}

#[tokio::test]
async fn approval_checkpoint_failure_can_be_retried_without_restarting_work() {
    for allow in [false, true] {
        let dir = Scratch::new();
        let (_, base, server) = server().await;
        let e = Engine::open_at(dir.0.join("state"), Some(base)).unwrap();
        let sid = e.command(Command::Create { workspace: dir.0.clone(), config: Some(Config { approval: ApprovalMode::Ask, ..Default::default() }) }).await.unwrap()["id"].as_str().unwrap().to_owned();
        let mut events = e.events.subscribe();
        e.command(Command::Tool { session: sid.clone(), name: "write_file".into(), arguments: json!({"path":"approved.txt","content":"one effect"}) }).await.unwrap();
        let call_id = loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv()).await.unwrap().unwrap();
            if event.kind == "approval" {
                break event.data["call"]["id"].as_str().unwrap().to_owned();
            }
        };
        let db = rusqlite::Connection::open(dir.0.join("state/sessions.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER fail_decision BEFORE INSERT ON events WHEN NEW.kind='approval_decision' BEGIN SELECT RAISE(ABORT,'injected decision checkpoint failure'); END;").unwrap();
        assert!(e.command(Command::Approve { session: sid.clone(), call_id: call_id.clone(), allow }).await.is_err());
        assert!(tokio::time::timeout(std::time::Duration::from_millis(50), e.quiesce()).await.is_err(), "a failed decision checkpoint must leave the approval waiting");
        assert_eq!(e.get(&sid).unwrap().status, Status::Approval);
        assert!(!dir.0.join("approved.txt").exists());
        db.execute_batch("DROP TRIGGER fail_decision;").unwrap();
        e.command(Command::Approve { session: sid.clone(), call_id: call_id.clone(), allow }).await.unwrap();
        finish(&e).await;
        assert_eq!(e.get(&sid).unwrap().status, Status::Idle);
        assert_eq!(dir.0.join("approved.txt").exists(), allow);
        assert_eq!(e.command(Command::Checkpoints { session: sid.clone() }).await.unwrap().as_array().unwrap().len(), usize::from(allow));
        assert!(e.command(Command::Approve { session: sid.clone(), call_id, allow }).await.is_err(), "a decision can be consumed only once");
        assert_eq!(e.store.lock().unwrap().events(&sid, 0, 100).unwrap().iter().filter(|v| v.kind == "approval_decision").count(), 1);
        server.abort();
    }
}

async fn concurrent_journal_batch(writers: usize, count: usize) -> (Vec<i64>, Vec<i64>, f64) {
    let dir = Scratch::new();
    let e = Engine::open(dir.0.join("state")).unwrap();
    let sid = e.command(Command::Create { workspace: dir.0.clone(), config: None }).await.unwrap()["id"].as_str().unwrap().to_owned();
    let cursor = e.store.lock().unwrap().tail_cursor(&sid, 0).unwrap();
    let mut events = e.events.subscribe();
    let runtime = tokio::runtime::Handle::current();
    let start = std::sync::Barrier::new(writers);
    let began = std::time::Instant::now();
    std::thread::scope(|scope| {
        for writer in 0..writers {
            let (e, sid, runtime, start) = (&e, &sid, &runtime, &start);
            scope.spawn(move || {
                start.wait();
                for n in 0..count {
                    runtime.block_on(e.command(Command::Goal { session: sid.clone(), objective: format!("writer {writer}, revision {n}") })).unwrap();
                }
            });
        }
    });
    let ms = began.elapsed().as_secs_f64() * 1000.0;
    let mut published = Vec::new();
    while let Ok(event) = events.try_recv() {
        published.push(event.seq);
    }
    let stored = e.store.lock().unwrap().events(&sid, cursor, 1000).unwrap().iter().map(|v| v.seq).collect::<Vec<_>>();
    assert_eq!(stored.len(), writers * count);
    assert_eq!(published.len(), stored.len(), "no live events may be lost");
    (published, stored, ms)
}

#[tokio::test]
async fn concurrent_journal_publication_matches_durable_order() {
    for _ in 0..4 {
        let (published, stored, _) = concurrent_journal_batch(16, 48).await;
        assert_eq!(published, stored, "live cursors must never skip an earlier committed event");
    }
}

#[tokio::test]
#[ignore = "release performance measurement; 768 durable updates from 16 writers"]
async fn journal_publication_benchmark() {
    let mut samples = Vec::new();
    for _ in 0..7 {
        let (_, _, ms) = concurrent_journal_batch(16, 48).await;
        samples.push(ms);
    }
    samples.sort_by(f64::total_cmp);
    println!("Journal batch: 768 updates, 16 writers; median {:.3} ms; min {:.3} ms; max {:.3} ms", samples[3], samples[0], samples[6]);
}

async fn review_accounting_case(malformed: bool) {
    let dir = Scratch::new();
    let (fake, base, server) = server().await;
    let e = Engine::open_at(dir.0.join("state"), Some(base)).unwrap();
    let sid = e.command(Command::Create { workspace: dir.0.clone(), config: Some(Config { approval: ApprovalMode::Auto, ..Default::default() }) }).await.unwrap()["id"].as_str().unwrap().to_owned();
    let mut s = e.get(&sid).unwrap();
    s.usage = Usage { input: 1000, output: 200, cached: 20, cache_write: 10, context: 123456, requests: 7 };
    e.store.lock().unwrap().save(&mut s, "fixture", json!({})).unwrap();
    let text = if malformed { "not a review decision".into() } else { json!({"allow":false,"reason":"Ask the person"}).to_string() };
    fake.replies.lock().unwrap().push_back(json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":text}]}],"usage":{"input_tokens":11,"output_tokens":3,"input_tokens_details":{"cached_tokens":5}}}));
    let mut events = e.events.subscribe();
    e.command(Command::Tool { session: sid.clone(), name: "write_file".into(), arguments: json!({"path":"needs-approval","content":"no"}) }).await.unwrap();
    let call_id = loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv()).await.unwrap().unwrap();
        if event.kind == "approval" {
            break event.data["call"]["id"].as_str().unwrap().to_owned();
        }
    };
    let usage = e.get(&sid).unwrap().usage;
    // Clean up the waiting task before assertions, including on regression.
    e.command(Command::Approve { session: sid.clone(), call_id, allow: false }).await.unwrap();
    finish(&e).await;
    server.abort();
    assert_eq!(usage.input, 1011, "successful review requests are charged even if their decision is malformed");
    assert_eq!(usage.output, 203);
    assert_eq!(usage.cached, 25);
    assert_eq!(usage.cache_write, 10);
    assert_eq!(usage.requests, 8);
    assert_eq!(usage.context, 123456, "reviewer context is independent of the coding session context");
    let history = e.store.lock().unwrap().events(&sid, 0, 100).unwrap();
    let reviewed = history.iter().find(|v| v.kind == "auto_review").expect("every completed reviewer request is audited");
    assert_eq!(reviewed.data["allow"], false);
    assert_eq!(reviewed.data["usage"]["input"], 11);
    assert!(!dir.0.join("needs-approval").exists());
}

#[tokio::test]
async fn automatic_review_preserves_the_main_context_usage() {
    review_accounting_case(false).await;
}

#[tokio::test]
async fn malformed_review_still_counts_toward_the_session_budget() {
    review_accounting_case(true).await;
}

#[derive(Clone, Default)]
struct McpAudit {
    seen: Arc<Mutex<Vec<Value>>>,
    started: Arc<tokio::sync::Notify>,
    cancelled: Arc<tokio::sync::Notify>,
    deleted: Arc<tokio::sync::Notify>,
    release: CancellationToken,
}
async fn mcp_audit_server() -> (McpAudit, McpServer, tokio::task::JoinHandle<()>) {
    use axum::{
        extract::State,
        http::{Method, StatusCode},
        response::IntoResponse,
    };
    let state = McpAudit::default();
    let app = axum::Router::new()
        .fallback(axum::routing::any(|State(s): State<McpAudit>, method: Method, body: String| async move {
            if method == Method::DELETE {
                s.seen.lock().unwrap().push(json!({"deleted":true}));
                s.deleted.notify_one();
                return StatusCode::NO_CONTENT.into_response();
            }
            let v: Value = serde_json::from_str(&body).unwrap();
            s.seen.lock().unwrap().push(v.clone());
            match v["method"].as_str().unwrap() {
                "initialize" => {
                    ([("Mcp-Session-Id", "audit-session")], axum::Json(json!({"jsonrpc":"2.0","id":v["id"],"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}}}}))).into_response()
                }
                "tools/call" => {
                    let mode = v["params"]["arguments"]["mode"].as_str().unwrap_or("ok");
                    if mode == "slow" {
                        s.started.notify_one();
                        s.release.cancelled().await;
                    }
                    let mut reply = json!({"jsonrpc":"2.0","id":v["id"],"result":{"content":[{"type":"text","text":"received"}]}});
                    match mode {
                        "error" => reply = json!({"jsonrpc":"2.0","id":v["id"],"error":{"code":-32602,"message":"invalid arguments"}}),
                        "both" => reply["error"] = json!({"code":-32602,"message":"ambiguous"}),
                        "version" => reply["jsonrpc"] = json!("1.0"),
                        "missing" => {
                            reply.as_object_mut().unwrap().remove("result");
                        }
                        "primed" => return ([("content-type", "text/event-stream")], format!("id: prime\ndata:\n\ndata: {reply}\n\n")).into_response(),
                        _ => {}
                    }
                    axum::Json(reply).into_response()
                }
                "notifications/cancelled" => {
                    s.cancelled.notify_one();
                    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
                    s.seen.lock().unwrap().push(json!({"cancel_reply":true}));
                    StatusCode::ACCEPTED.into_response()
                }
                _ => StatusCode::ACCEPTED.into_response(),
            }
        }))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = serde_json::from_value(json!({"name":"audit","url":format!("http://{}", listener.local_addr().unwrap())})).unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (state, config, server)
}

#[tokio::test]
async fn mcp_stop_does_not_wait_for_a_slow_cancellation_acknowledgement() {
    let (state, config, server) = mcp_audit_server().await;
    let dir = Scratch::new();
    let e = Engine::open_at(dir.0.join("state"), Some("http://127.0.0.1:1".into())).unwrap();
    let sid = e.command(Command::Create { workspace: dir.0.clone(), config: Some(Config { approval: ApprovalMode::Allow, mcp: vec![config], ..Default::default() }) }).await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    e.command(Command::Tool { session: sid.clone(), name: "mcp__audit__echo".into(), arguments: json!({"mode":"slow"}) }).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), state.started.notified()).await.unwrap();
    let began = std::time::Instant::now();
    e.command(Command::Stop { session: sid.clone() }).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(8), e.quiesce()).await.unwrap();
    let elapsed = began.elapsed();
    println!("MCP Stop with 1200 ms cancellation acknowledgement: {:.3} ms", elapsed.as_secs_f64() * 1000.0);
    assert_eq!(e.get(&sid).unwrap().status, Status::Paused);
    tokio::time::timeout(std::time::Duration::from_secs(2), state.cancelled.notified()).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), state.deleted.notified()).await.unwrap();
    let seen = state.seen.lock().unwrap().clone();
    let request = seen.iter().find(|v| v["method"] == "tools/call").unwrap();
    let notification = seen.iter().find(|v| v["method"] == "notifications/cancelled").unwrap();
    assert_eq!(notification["params"]["requestId"], request["id"]);
    assert_eq!(seen.iter().filter(|v| v["method"] == "tools/call").count(), 1, "an interrupted external effect cannot be replayed");
    assert!(seen.iter().position(|v| v["cancel_reply"] == true).unwrap() < seen.iter().position(|v| v["deleted"] == true).unwrap(), "session deletion must follow cancellation cleanup");
    state.release.cancel();
    server.abort();
    assert!(elapsed < std::time::Duration::from_millis(500), "local Stop must not wait for remote cancellation acknowledgement");
}

#[tokio::test]
async fn mcp_does_not_cancel_unissued_or_completed_requests() {
    let (state, config, server) = mcp_audit_server().await;
    let mut c = mcp::Connection::start(&config, std::path::Path::new("/var/tmp")).await.unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(c.invoke("echo", json!({}), &cancel, 10).await.is_err());
    assert!(c.invoke("echo", json!({"mode":"error"}), &CancellationToken::new(), 10).await.is_err());
    assert!(c.invoke("echo", json!({}), &CancellationToken::new(), 10).await.is_ok(), "a completed remote error must leave the connection reusable");
    drop(c);
    tokio::time::timeout(std::time::Duration::from_secs(3), state.deleted.notified()).await.unwrap();
    let seen = state.seen.lock().unwrap().clone();
    server.abort();
    assert_eq!(seen.iter().filter(|v| v["method"] == "tools/call").count(), 2);
    assert!(!seen.iter().any(|v| v["method"] == "notifications/cancelled"), "only an issued, unfinished request can be cancelled");
}

#[tokio::test]
async fn mcp_accepts_empty_sse_priming_events() {
    let (_, config, server) = mcp_audit_server().await;
    let mut c = mcp::Connection::start(&config, std::path::Path::new("/var/tmp")).await.unwrap();
    let result = c.invoke("echo", json!({"mode":"primed"}), &CancellationToken::new(), 10).await;
    server.abort();
    assert_eq!(result.unwrap()["content"][0]["text"], "received");
}

#[tokio::test]
async fn mcp_rejects_ambiguous_or_wrong_version_responses() {
    let (_, config, server) = mcp_audit_server().await;
    let mut c = mcp::Connection::start(&config, std::path::Path::new("/var/tmp")).await.unwrap();
    for mode in ["version", "both", "missing"] {
        assert!(c.invoke("echo", json!({"mode":mode}), &CancellationToken::new(), 10).await.is_err(), "{mode} must not be recorded as a successful effect");
    }
    server.abort();
}

#[tokio::test]
async fn mcp_timeout_cancels_once_without_extending_the_local_deadline() {
    let (state, config, server) = mcp_audit_server().await;
    let mut c = mcp::Connection::start(&config, std::path::Path::new("/var/tmp")).await.unwrap();
    let began = std::time::Instant::now();
    let error = c.invoke("echo", json!({"mode":"slow"}), &CancellationToken::new(), 1).await.unwrap_err();
    let elapsed = began.elapsed();
    assert!(error.to_string().contains("timed out; inspect external state"));
    tokio::time::timeout(std::time::Duration::from_secs(2), state.cancelled.notified()).await.unwrap();
    drop(c);
    tokio::time::timeout(std::time::Duration::from_secs(3), state.deleted.notified()).await.unwrap();
    let seen = state.seen.lock().unwrap();
    assert_eq!(seen.iter().filter(|v| v["method"] == "tools/call").count(), 1);
    assert_eq!(seen.iter().filter(|v| v["method"] == "notifications/cancelled").count(), 1);
    state.release.cancel();
    server.abort();
    println!("MCP 1000 ms timeout with 1200 ms cancellation acknowledgement: {:.3} ms", elapsed.as_secs_f64() * 1000.0);
    assert!(elapsed < std::time::Duration::from_millis(1500), "remote cleanup cannot extend the local request deadline");
}

#[tokio::test]
async fn cancelled_network_tools_do_not_dispatch_or_replay_writes() {
    use axum::{extract::State, response::IntoResponse};
    let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let started = Arc::new(tokio::sync::Notify::new());
    let release = CancellationToken::new();
    let app = axum::Router::new()
        .fallback(axum::routing::any(|State((seen, started, release)): State<(Arc<std::sync::atomic::AtomicUsize>, Arc<tokio::sync::Notify>, CancellationToken)>| async move {
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            started.notify_one();
            release.cancelled().await;
            axum::Json(json!({"accepted":true})).into_response()
        }))
        .with_state((seen.clone(), started.clone(), release.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cancel = CancellationToken::new();
    cancel.cancel();
    let write = call("github_write", json!({"repository":"a/b","operation":"comment","number":1,"body":{"body":"fixture"}}));
    assert!(github::run(&write, &cancel).await.unwrap_err().to_string().contains("before GitHub credential lookup"));
    assert!(github::request(&base, Some("fixture-token"), &write, &cancel).await.is_err());
    assert!(tools::fetch_url(&call("fetch_url", json!({"url":base})), 1024, &cancel).await.is_err());
    let s = Session::new("/var/tmp".into(), Config::default());
    assert!(provider::Client::at(&base).unwrap().turn(&s, "test", &[], &cancel, |_, _| {}).await.is_err());
    assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 0);
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let request = tokio::spawn(async move { github::request(&base, Some("fixture-token"), &write, &cancel).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), started.notified()).await.unwrap();
    let began = std::time::Instant::now();
    stop.cancel();
    let error = tokio::time::timeout(std::time::Duration::from_millis(500), request).await.unwrap().unwrap().unwrap_err();
    assert!(error.to_string().contains("inspect GitHub before retrying"));
    assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1, "publishing is never retried after uncertain cancellation");
    println!("GitHub cancellation while awaiting a response: {:.3} ms", began.elapsed().as_secs_f64() * 1000.0);
    release.cancel();
    server.abort();
}

#[tokio::test]
#[ignore = "release process capture benchmark; nine 22 KiB Unicode outputs"]
async fn process_capture_benchmark() {
    let dir = Scratch::new();
    let s = Session::new(dir.0.clone(), Config::default());
    let payload = "résultat 🌿 世界\n".repeat(1024);
    let command = call("execute", json!({"program":"/usr/bin/printf","args":["%s",payload]}));
    let mut samples = Vec::new();
    for _ in 0..9 {
        let began = std::time::Instant::now();
        let output = tools::execute(&s, &command, &dir.0, &CancellationToken::new(), Arc::new(|_, _| {})).await.unwrap();
        samples.push(began.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(output["stdout"], payload);
        assert_eq!(output["success"], true);
    }
    samples.sort_by(f64::total_cmp);
    println!("Process capture: {} bytes; median {:.3} ms; min {:.3} ms; max {:.3} ms", payload.len(), samples[4], samples[0], samples[8]);
}

#[tokio::test]
async fn process_capture_is_complete_private_and_reports_truncation() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Scratch::new();
    let s = Session::new(dir.0.clone(), Config { max_tool_bytes: 1024, ..Default::default() });
    let payload = format!("{}🌿end", "a".repeat(8191));
    let command = call("execute", json!({"program":"/usr/bin/printf","args":["%s",payload]}));
    let streamed = Arc::new(Mutex::new(String::new()));
    let observed = streamed.clone();
    let result = tools::execute(&s, &command, &dir.0, &CancellationToken::new(), Arc::new(move |_, v| observed.lock().unwrap().push_str(v["text"].as_str().unwrap()))).await.unwrap();
    assert_eq!(*streamed.lock().unwrap(), payload);
    assert_eq!(result["stdout"], payload[payload.len() - 1024..]);
    assert_eq!(result["stdout_bytes"], payload.len());
    assert_eq!(result["stderr_bytes"], 0);
    assert_eq!(result["stdout_truncated"], true);
    assert_eq!(result["stderr_truncated"], false);
    assert_eq!(result["artifact_bytes"], payload.len());
    assert_eq!(result["artifact_truncated"], false);
    let path = dir.0.join(result["artifact"].as_str().unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), payload.as_bytes());
    assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
}

#[tokio::test]
async fn failed_process_start_removes_unused_capture_files() {
    let dir = Scratch::new();
    let s = Session::new(dir.0.clone(), Config::default());
    let command = call("execute", json!({"program":dir.0.join("no-such-program"),"args":[]}));
    assert!(tools::execute(&s, &command, &dir.0, &CancellationToken::new(), Arc::new(|_, _| {})).await.is_err());
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
}

#[tokio::test]
async fn capture_allocation_failure_precedes_process_effects() {
    let dir = Scratch::new();
    let s = Session::new(dir.0.clone(), Config { sandbox: SandboxMode::Unrestricted, ..Default::default() });
    let artifact_path = dir.0.join("not-a-directory");
    std::fs::write(&artifact_path, b"existing file").unwrap();
    let effect = dir.0.join("should-not-exist");
    let command = call("execute", json!({"program":"/usr/bin/touch","args":[effect]}));
    assert!(tools::execute(&s, &command, &artifact_path, &CancellationToken::new(), Arc::new(|_, _| {})).await.is_err());
    assert!(!effect.exists());
    assert_eq!(std::fs::read(artifact_path).unwrap(), b"existing file");
}

#[tokio::test]
async fn child_concurrency_counts_admitted_work_before_it_runs() {
    let dir = Scratch::new();
    let (fake, base, server) = server().await;
    let e = Engine::open_at(dir.0.join("state"), Some(base)).unwrap();
    let config = Config { subagents: true, max_subagents: 1, ..Default::default() };
    let sid = e.command(Command::Create { workspace: dir.0.clone(), config: Some(config) }).await.unwrap()["id"].as_str().unwrap().to_owned();
    let mut second = tool("spawn_agent", json!({"task":"Second review"}));
    second["call_id"] = json!("call2");
    fake.replies.lock().unwrap().push_back(response(vec![tool("spawn_agent", json!({"task":"First review"})), second]));
    let began = std::time::Instant::now();
    e.command(Command::Send { session: sid.clone(), text: "Review using at most one child at a time".into() }).await.unwrap();
    finish(&e).await;
    let children = e.store.lock().unwrap().list().unwrap().into_iter().filter(|v| v["parent"] == sid).collect::<Vec<_>>();
    println!("Child admission batch: {:.3} ms; {} children", began.elapsed().as_secs_f64() * 1000.0, children.len());
    server.abort();
    assert_eq!(children.len(), 1, "admitted children must count immediately, before their run task is polled");
    let events = e.store.lock().unwrap().events(&sid, 0, 100).unwrap();
    assert!(events.iter().any(|v| v.kind == "tool_result" && v.data["call_id"] == "call2" && v.data["error"] == true));
}

#[tokio::test]
async fn child_workspace_selects_a_native_worktree_and_keeps_parent_files_separate() {
    use std::sync::atomic::AtomicBool;
    let dir = Scratch::new();
    let root = dir.0.join("repo");
    std::fs::create_dir(&root).unwrap();
    let repo = gix::init(&root).unwrap();
    std::fs::write(repo.git_dir().join("config"), "[core]\nrepositoryformatversion = 0\nbare = false\n[user]\nname = Test Author\nemail = test@example.invalid\n").unwrap();
    let run = |a: Value| git::run(&root, &call("repository_write", a), Arc::new(AtomicBool::new(false))).unwrap();
    std::fs::write(root.join("base.txt"), "baseline").unwrap();
    run(json!({"operation":"stage","paths":["base.txt"]}));
    run(json!({"operation":"commit","message":"Base","expected_head":""}));
    run(json!({"operation":"branch","branch":"child-review"}));
    run(json!({"operation":"worktree_add","branch":"child-review","path":"worktrees/review"}));
    let worktree = root.join("worktrees/review").canonicalize().unwrap();
    let (fake, base, server) = server().await;
    let e = Engine::open_at(dir.0.join("state"), Some(base)).unwrap();
    let config = Config { subagents: true, approval: ApprovalMode::Allow, ..Default::default() };
    let sid = e.command(Command::Create { workspace: root.clone(), config: Some(config) }).await.unwrap()["id"].as_str().unwrap().to_owned();
    fake.replies.lock().unwrap().push_back(response(vec![tool("write_file", json!({"path":"child-only.txt","content":"isolated change"}))]));
    e.command(Command::Tool { session: sid.clone(), name: "spawn_agent".into(), arguments: json!({"task":"Make the isolated fixture change","workspace":"worktrees/review","read_only":false}) })
        .await
        .unwrap();
    finish(&e).await;
    let child = e.store.lock().unwrap().list().unwrap().into_iter().find(|v| v["parent"] == sid).unwrap();
    let id = child["id"].as_str().unwrap();
    let s = e.get(id).unwrap();
    server.abort();
    assert_eq!(s.workspace, worktree);
    assert_eq!(std::fs::read_to_string(worktree.join("child-only.txt")).unwrap(), "isolated change");
    assert!(!root.join("child-only.txt").exists());
    assert_eq!(git::run(&worktree, &call("repository_read", json!({"operation":"status"})), Arc::new(AtomicBool::new(false))).unwrap()["branch"], "child-review");
    drop(e);
    assert_eq!(Engine::open(dir.0.join("state")).unwrap().get(id).unwrap().workspace, worktree);
}

#[tokio::test]
async fn child_workspace_rejects_escapes_and_invalid_tasks_without_creating_sessions() {
    let dir = Scratch::new();
    let outside = Scratch::new();
    std::fs::write(dir.0.join("file"), "not a folder").unwrap();
    std::fs::create_dir(dir.0.join(".git")).unwrap();
    std::os::unix::fs::symlink(&outside.0, dir.0.join("escape")).unwrap();
    let e = Engine::open_at(dir.0.join("state"), Some("http://127.0.0.1:1".into())).unwrap();
    let config = Config { subagents: true, ..Default::default() };
    let sid = e.command(Command::Create { workspace: dir.0.clone(), config: Some(config) }).await.unwrap()["id"].as_str().unwrap().to_owned();
    let mut invalid: Vec<Value> = [json!("../"), json!(outside.0), json!("escape"), json!("file"), json!(".git"), json!("missing"), json!(false), Value::Null]
        .into_iter()
        .map(|workspace| json!({"task":"Review","workspace":workspace}))
        .collect();
    invalid.extend([json!({"task":"  "}), json!({"task":42})]);
    for arguments in invalid {
        e.command(Command::Tool { session: sid.clone(), name: "spawn_agent".into(), arguments: arguments.clone() }).await.unwrap();
        finish(&e).await;
        assert_eq!(e.store.lock().unwrap().list().unwrap().len(), 1, "{arguments}");
        let events = e.store.lock().unwrap().events(&sid, 0, 1000).unwrap();
        assert_eq!(events.iter().rev().find(|v| v.kind == "tool_result").unwrap().data["error"], true);
    }
}

#[test]
fn agent_shortcut_preserves_workspace_and_model_controls() {
    let s = Session::new("/".into(), Config::default());
    let arguments =
        json!({"task":"Review independently","workspace":"worktrees/review","provider":"openai","model":"custom-model","effort":"high","read_only":false,"model_config":{"max_output_tokens":8000}});
    let commands::Input::Command(Command::Tool { session, name, arguments: actual }) = commands::parse(&format!("/agent {arguments}"), &s).unwrap() else { panic!("expected tool") };
    assert_eq!(session, s.id);
    assert_eq!(name, "spawn_agent");
    assert_eq!(actual, arguments);
    for invalid in ["/agent", "/agent []", "/agent null", "/agent invalid"] {
        assert!(commands::parse(invalid, &s).is_err());
    }
}

fn assistant_text(text: &str) -> Value {
    response(vec![json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]})])
}
#[tokio::test]
async fn supervisor_recovers_early_completion_and_preserves_policy_and_audit() {
    let dir = Scratch::new();
    let (fake, base, server) = server().await;
    let e = Engine::open_at(dir.0.join("state"), Some(base)).unwrap();
    let config = Config {
        supervisor: supervisor::Config { enabled: true, model: Some(ModelConfig { model: "supervisor-fixture".into(), ..Default::default() }), ..Default::default() },
        approval: ApprovalMode::Ask,
        ..Default::default()
    };
    let s: Session = serde_json::from_value(e.command(Command::Create { workspace: dir.0.clone(), config: Some(config) }).await.unwrap()).unwrap();
    fake.replies.lock().unwrap().extend([
        assistant_text("I will stop before checking the result."),
        assistant_text(r#"{"action":"steer","reason":"Requested verification is missing","message":"Run the requested verification","notes":"Check evidence"}"#),
        assistant_text("Verification complete."),
        assistant_text(r#"{"action":"observe","reason":"The requested verification is complete","notes":"Complete"}"#),
    ]);
    e.command(Command::Send { session: s.id.clone(), text: "Complete the work and verify it.".into() }).await.unwrap();
    finish(&e).await;
    let final_s = e.get(&s.id).unwrap();
    assert_eq!(final_s.status, Status::Idle);
    assert_eq!(final_s.supervisor.reviews, 2);
    assert_eq!(final_s.supervisor.interventions, 1);
    assert_eq!(final_s.config.approval, ApprovalMode::Ask);
    assert_eq!(final_s.supervisor.usage.input, 20);
    let requests = fake.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[1].1["model"], "supervisor-fixture");
    assert!(requests[2].1.to_string().contains("Supervisor advisory"));
    assert!(requests[1].1["tools"].as_array().unwrap().iter().all(|t| matches!(t["name"].as_str(), Some("read_transcript" | "search_transcript" | "compaction_history" | "read_artifact"))));
    drop(requests);
    let events = e.command(Command::Events { session: s.id.clone(), after: 0, limit: 200 }).await.unwrap();
    assert!(events.to_string().contains("supervisor_review"));
    drop(e);
    let reopened = Engine::open(dir.0.join("state")).unwrap();
    assert_eq!(reopened.get(&s.id).unwrap().supervisor.notes, "Complete");
    server.abort();
}
#[tokio::test]
async fn supervisor_pause_and_intervention_limits_do_not_spin() {
    for action in ["pause", "steer"] {
        let dir = Scratch::new();
        let (fake, base, server) = server().await;
        let e = Engine::open_at(dir.0.join("state"), Some(base)).unwrap();
        let config = Config { supervisor: supervisor::Config { enabled: true, max_interventions: 1, ..Default::default() }, ..Default::default() };
        let s: Session = serde_json::from_value(e.command(Command::Create { workspace: dir.0.clone(), config: Some(config) }).await.unwrap()).unwrap();
        fake.replies.lock().unwrap().extend([
            assistant_text("Initial answer"),
            assistant_text(&json!({"action":action,"reason":"fixture evidence","message":"next step"}).to_string()),
            assistant_text("Final answer"),
        ]);
        e.command(Command::Send { session: s.id.clone(), text: "Task".into() }).await.unwrap();
        finish(&e).await;
        let s = e.get(&s.id).unwrap();
        assert_eq!(s.status, if action == "pause" { Status::Paused } else { Status::Idle });
        assert_eq!(s.supervisor.interventions, 1);
        assert_eq!(fake.requests.lock().unwrap().len(), if action == "pause" { 2 } else { 3 });
        server.abort();
    }
}
#[tokio::test]
async fn integrations_validate_grants_presets_and_local_auth_without_secret_access() {
    let dir = Scratch::new();
    let e = Engine::open(dir.0.join("state")).unwrap();
    let s: Session = serde_json::from_value(e.command(Command::Create { workspace: dir.0.clone(), config: None }).await.unwrap()).unwrap();
    for preset in ["slack", "logfire", "logfire-eu", "terminal"] {
        let mut config = Config::default();
        config.mcp.push(integrations::preset(preset).unwrap());
        config.validate().unwrap();
    }
    e.command(Command::Integrations { session: s.id.clone(), action: "grant_secret".into(), arguments: json!({"name":"test","reference":"op://vault/item/password"}) }).await.unwrap();
    assert_eq!(e.get(&s.id).unwrap().config.olo.secret_grants.len(), 1);
    assert!(e.get(&s.id).unwrap().config.child(false, &json!({})).unwrap().olo.secret_grants.is_empty());
    e.command(Command::Integrations { session: s.id.clone(), action: "revoke_secret".into(), arguments: json!({"name":"test"}) }).await.unwrap();
    assert!(e.get(&s.id).unwrap().config.olo.secret_grants.is_empty());
    let mut config = Config::default();
    config.mcp = vec![McpServer { name: "bad".into(), program: "server".into(), auth: Some(oauth::Auth::BearerEnv { variable: "TOKEN".into() }), ..Default::default() }];
    assert!(config.validate().is_err());
}

#[tokio::test]
async fn stop_cancels_an_inflight_supervisor_without_steering() {
    use axum::{Json, Router, response::IntoResponse, routing::post};
    let waiting = Arc::new(tokio::sync::Notify::new());
    let signal = waiting.clone();
    let app = Router::new().fallback(post(move |Json(body): Json<Value>| {
        let signal = signal.clone();
        async move {
            if body["instructions"].as_str().is_some_and(|s| s.starts_with("You supervise")) {
                signal.notify_one();
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
            let reply = assistant_text("Primary stopped early.");
            ([("content-type", "text/event-stream")], format!("data: {}\n\n", json!({"type":"response.completed","response":reply}))).into_response()
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let dir = Scratch::new();
    let e = Engine::open_at(dir.0.join("state"), Some(base)).unwrap();
    let config = Config { supervisor: supervisor::Config { enabled: true, ..Default::default() }, ..Default::default() };
    let s: Session = serde_json::from_value(e.command(Command::Create { workspace: dir.0.clone(), config: Some(config) }).await.unwrap()).unwrap();
    e.command(Command::Send { session: s.id.clone(), text: "Task".into() }).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), waiting.notified()).await.unwrap();
    let start = std::time::Instant::now();
    e.command(Command::Stop { session: s.id.clone() }).await.unwrap();
    finish(&e).await;
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
    let s = e.get(&s.id).unwrap();
    assert_eq!(s.status, Status::Paused);
    assert_eq!(s.supervisor.interventions, 0);
    assert!(!serde_json::to_string(&s.context).unwrap().contains("Supervisor advisory"));
    server.abort();
}
#[tokio::test]
async fn supervisor_cannot_write_or_override_primary_budget() {
    let dir = Scratch::new();
    let (fake, base, server) = server().await;
    let e = Engine::open_at(dir.0.join("state"), Some(base)).unwrap();
    let config = Config { supervisor: supervisor::Config { enabled: true, ..Default::default() }, ..Default::default() };
    let s: Session = serde_json::from_value(e.command(Command::Create { workspace: dir.0.clone(), config: Some(config.clone()) }).await.unwrap()).unwrap();
    fake.replies.lock().unwrap().extend([
        assistant_text("Done"),
        response(vec![tool("write_file", json!({"path":"forbidden","content":"bad"}))]),
        assistant_text(r#"{"action":"observe","reason":"Cannot perform writes"}"#),
    ]);
    e.command(Command::Send { session: s.id.clone(), text: "Task".into() }).await.unwrap();
    finish(&e).await;
    assert!(!dir.0.join("forbidden").exists());
    assert!(fake.requests.lock().unwrap()[2].1.to_string().contains("no permission"));
    let mut config = config;
    config.max_turns = 1;
    let other: Session = serde_json::from_value(e.command(Command::Create { workspace: dir.0.clone(), config: Some(config) }).await.unwrap()).unwrap();
    fake.replies.lock().unwrap().push_back(response(vec![tool("set_todos", json!({"todos":[]}))]));
    e.command(Command::Send { session: other.id.clone(), text: "Task".into() }).await.unwrap();
    finish(&e).await;
    let other = e.get(&other.id).unwrap();
    assert_eq!(other.status, Status::Failed);
    assert!(other.error.unwrap().contains("budget reached"));
    assert_eq!(other.supervisor.reviews, 0);
    server.abort();
}

#[tokio::test]
async fn model_profiles_are_durable_named_snapshots_for_every_role() {
    let dir = Scratch::new();
    let e = Engine::open(dir.0.clone()).unwrap();
    let s: Session = serde_json::from_value(e.command(Command::Create { workspace: dir.0.clone(), config: None }).await.unwrap()).unwrap();
    let profile = profiles::ModelProfile { id: id(), name: "  Fast custom  ".into(), model: ModelConfig { model: "custom-model".into(), ..ModelConfig::for_provider(Provider::Openai) } };
    // API clients can create profiles without depending on a provider's discovery implementation.
    let saved = e.command(Command::ModelProfiles { action: "save".into(), profile: Some(profile.clone()), id: None, session: None, role: "primary".into() }).await.unwrap();
    assert!(saved["profiles"].as_array().unwrap().iter().any(|p| p["name"] == "Fast custom"));
    let mut duplicate = profile.clone();
    duplicate.id = id();
    duplicate.name = "fast CUSTOM".into();
    assert!(e.command(Command::ModelProfiles { action: "save".into(), profile: Some(duplicate), id: None, session: None, role: "primary".into() }).await.is_err());
    for role in ["primary", "supervisor", "reviewer", "compaction"] {
        e.command(Command::ModelProfiles { action: "apply".into(), profile: None, id: Some(profile.id.clone()), session: Some(s.id.clone()), role: role.into() }).await.unwrap();
    }
    let selected = e.get(&s.id).unwrap();
    assert_eq!(selected.config.model, profile.model);
    assert_eq!(selected.config.supervisor.model, Some(profile.model.clone()));
    assert_eq!(selected.config.reviewer, profile.model);
    assert_eq!(selected.config.compaction.model, Some(profile.model.clone()));
    e.command(Command::ModelProfiles { action: "remove".into(), profile: None, id: Some(profile.id.clone()), session: None, role: "primary".into() }).await.unwrap();
    assert_eq!(e.get(&s.id).unwrap().config.model, profile.model);
    drop(e);
    let restored = Engine::open(dir.0.clone()).unwrap();
    assert_eq!(restored.get(&s.id).unwrap().config.model, profile.model);
    let profiles = restored.command(Command::ModelProfiles { action: "list".into(), profile: None, id: None, session: None, role: "primary".into() }).await.unwrap();
    assert!(!profiles["profiles"].as_array().unwrap().iter().any(|p| p["id"] == profile.id));
}

#[tokio::test]
async fn field_settings_preserve_other_edits_and_reject_invalid_changes_atomically() {
    let dir = Scratch::new();
    let e = Engine::open(dir.0.clone()).unwrap();
    let s: Session = serde_json::from_value(e.command(Command::Create { workspace: dir.0.clone(), config: None }).await.unwrap()).unwrap();
    let edit = |pairs: Vec<(&str, Value)>| Command::ConfigureFields { session: s.id.clone(), values: pairs.into_iter().map(|(k, v)| (k.into(), v)).collect() };
    e.command(edit(vec![("instructions", json!("Keep this edit"))])).await.unwrap();
    e.command(edit(vec![("supervisor.enabled", json!(true))])).await.unwrap();
    assert_eq!(e.get(&s.id).unwrap().config.instructions, "Keep this edit");
    assert!(e.command(edit(vec![("instructions", json!("discard")), ("max_turns", json!(-1))])).await.is_err());
    assert!(e.command(edit(vec![("unknown", json!(true))])).await.is_err());
    let s = e.get(&s.id).unwrap();
    assert_eq!(s.config.instructions, "Keep this edit");
    assert!(s.config.supervisor.enabled);
}

#[tokio::test]
async fn model_discovery_normalizes_catalog_without_saving_drafts() {
    let app = axum::Router::new().route("/v1/models", axum::routing::get(|| async { axum::Json(json!({"data":[{"id":"vendor/z"},{"id":"vendor/a"},{"id":"vendor/z"},"vendor/b",{"ignored":true}]})) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = Scratch::new();
    let e = Engine::open(dir.0.clone()).unwrap();
    let mut model = ModelConfig::for_provider(Provider::Openai);
    model.endpoint.as_mut().unwrap().base_url = base;
    let profile = profiles::ModelProfile { id: id(), name: "Draft".into(), model };
    let result = e.command(Command::ModelProfiles { action: "discover".into(), profile: Some(profile), id: None, session: None, role: "primary".into() }).await.unwrap();
    assert_eq!(result["models"], json!(["vendor/a", "vendor/b", "vendor/z"]));
    let saved = e.command(Command::ModelProfiles { action: "list".into(), profile: None, id: None, session: None, role: "primary".into() }).await.unwrap();
    assert!(!saved["profiles"].as_array().unwrap().iter().any(|p| p["name"] == "Draft"));
    server.abort();
}
#[test]
fn a_session_is_named_in_a_few_words_until_the_agent_names_it() {
    assert_eq!(
        olo_coder_core::model::provisional_title("In calc.py: fix add and mul, add sub and div (div raises ZeroDivisionError with a clear message)"),
        "In calc.py: fix add and mul, add sub and div…"
    );
    assert_eq!(olo_coder_core::model::provisional_title("\n  Fix the build \nand more"), "Fix the build");
}
