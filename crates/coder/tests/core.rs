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
fn claude_thinking_redacted_and_signed_compaction_are_lossless() {
    let blocks = json!([{"type":"compaction","content":"summary","signature":"signature"},{"type":"thinking","thinking":"reasoning","signature":"sig"},{"type":"redacted_thinking","data":"encrypted"},{"type":"tool_use","id":"a","name":"read_file","input":{"path":"x"}}]);
    let reply =
        provider::parse(Provider::Claude, json!({"content":blocks,"stop_reason":"tool_use","usage":{"input_tokens":3,"output_tokens":4,"cache_read_input_tokens":5,"cache_creation_input_tokens":6}}))
            .unwrap();
    let mut s = Session::new(PathBuf::from("/"), Config::default());
    s.config.model.provider = Provider::Claude;
    s.context = reply.items;
    s.result(&reply.calls[0], "result", false);
    let body = provider::body(&s, "system", &[]);
    assert_eq!(body["messages"][0]["content"], blocks);
    assert_eq!(body["messages"][1]["content"][0]["tool_use_id"], "a");
    assert_eq!(reply.usage.context, 18);
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
    assert!(provider::parse(Provider::Claude, json!({"stop_reason":"max_tokens","content":[]})).is_err());
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
    assert_eq!(s.config.approval, ApprovalMode::Ask);
    assert_eq!(s.config.sandbox, SandboxMode::Workspace);
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
    let s = Session::new(dir.0.clone(), Config::default());
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
    let created = e.command(Command::Create { workspace: dir.0.clone(), config: None }).await.unwrap();
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
async fn claude_server_compaction_is_signed_and_replayed() {
    let (fake, base, server) = server().await;
    let client = provider::Client::at(&base).unwrap();
    let dir = Scratch::new();
    let mut s = Session::new(dir.0.clone(), Config::default());
    s.config.model.provider = Provider::Claude;
    s.config.model.model = "claude-custom".into();
    s.user("remember this");
    let block = json!({"type":"compaction","content":"summary","signature":"do-not-change"});
    fake.replies.lock().unwrap().push_back(json!({"content":[block],"stop_reason":"compaction"}));
    let r = client.compact(&s, "system", &tools::definitions(), &CancellationToken::new()).await.unwrap();
    s.context = r.items;
    s.user("continue");
    fake.replies.lock().unwrap().push_back(json!({"content":[{"type":"text","text":"yes"}],"stop_reason":"end_turn"}));
    client.turn(&s, "system", &tools::definitions(), &CancellationToken::new(), |_, _| {}).await.unwrap();
    let req = fake.requests.lock().unwrap();
    assert_eq!(req[0].1["compaction"]["type"], "summarize");
    assert_eq!(req[1].1["messages"][0]["content"][0], block);
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
    let app = Router::new()
        .fallback(any(|State(seen): State<Arc<Mutex<Vec<Value>>>>, method: Method, headers: HeaderMap, body: String| async move {
            if method == Method::DELETE {
                return axum::http::StatusCode::NO_CONTENT.into_response();
            }
            let v: Value = serde_json::from_str(&body).unwrap();
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
        .with_state(seen.clone());
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
fn cache_accounting_and_mutable_state_do_not_change_stable_prefixes() {
    let mut s = Session::new(PathBuf::from("/"), Config::default());
    s.config.model.provider = Provider::Claude;
    let signed = json!({"role":"assistant","content":[{"type":"thinking","thinking":"opaque","signature":"never-touch"}]});
    s.context.push(signed.clone());
    let before = provider::body(&s, "stable", &tools::definitions());
    s.memory.push_str("remember this");
    let after = provider::body(&s, "stable", &tools::definitions());
    assert_eq!(before["system"], after["system"]);
    assert_eq!(before["tools"], after["tools"]);
    assert_eq!(after["messages"][0], signed);
    assert!(after["messages"][1].to_string().contains("remember this"));
    assert_eq!(after["cache_control"]["ttl"], "1h");
    s.config.model.cache_ttl = None;
    let disabled = provider::body(&s, "stable", &[]);
    assert!(disabled.get("cache_control").is_none());
    let usage = provider::usage(Provider::Claude, &json!({"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":100,"cache_creation_input_tokens":50}));
    assert_eq!(usage.input, 160);
    assert_eq!(usage.cached, 100);
    assert_eq!(usage.cache_write, 50);
    assert_eq!(usage.context, 180);
    let codex = provider::usage(Provider::Codex, &json!({"input_tokens":160,"output_tokens":20,"input_tokens_details":{"cached_tokens":100}}));
    assert_eq!(codex.input, 160);
    assert_eq!(codex.context, 180);
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
    old.execute_batch("DROP TRIGGER event_search_insert; DROP TABLE event_search; ALTER TABLE edits DROP COLUMN deleted; PRAGMA user_version=1;").unwrap();
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
    let sid = request(&path, json!({"op":"create","workspace":dir.0})).await["result"]["id"].as_str().unwrap().to_owned();
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
    let child = parent.child(false, &json!({"provider":"claude","read_only":false,"model_config":{"model":"future-claude","thinking":{"type":"enabled","budget_tokens":4096},"max_output_tokens":8192,"service_tier":"priority","extra":{"new_option":true}}})).unwrap();
    assert_eq!(child.model.provider, Provider::Claude);
    assert_eq!(child.model.model, "future-claude");
    assert_eq!(child.model.thinking.unwrap()["budget_tokens"], 4096);
    assert_eq!(child.model.max_output_tokens, 8192);
    assert_eq!(child.model.extra["new_option"], true);
    assert_eq!(child.approval, ApprovalMode::Ask);
    assert_eq!(child.denied_tools, parent.denied_tools);
    assert!(child.read_only);
    let defaults = parent.child(false, &json!({"provider":"claude"})).unwrap();
    assert_eq!(defaults.model.model, "claude-sonnet-5-5");
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
    let sid = e.command(Command::Create { workspace: dir.0.clone(), config: None }).await.unwrap()["id"].as_str().unwrap().to_owned();
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
    db.execute_batch("DROP TABLE messages; PRAGMA user_version=3;").unwrap();
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
