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
