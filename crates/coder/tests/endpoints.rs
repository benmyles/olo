//! Protocol and long-session compaction regressions against real local HTTP.
use olo_coder_core::{compaction, provider, store::Store, *};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = PathBuf::from("/var/tmp").join(format!("coder-context-{}", id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn custom(base: &str) -> Config {
    let mut c = Config { model: ModelConfig::for_provider(Provider::Openai), ..Config::default() };
    c.model.endpoint.as_mut().unwrap().base_url = base.into();
    c.model.auto_compact = false;
    c.compaction.keep_recent_exchanges = 2;
    c.compaction.keep_recent_tokens = 256;
    c.compaction.summary_tokens = 512;
    c.retries = 0;
    c
}
fn completion(text: &str) -> Value {
    json!({"choices":[{"index":0,"message":{"role":"assistant","content":text},"finish_reason":"stop"}],"usage":{"prompt_tokens":120,"completion_tokens":25,"prompt_tokens_details":{"cached_tokens":50}}})
}
#[derive(Default)]
struct Fixture {
    requests: Mutex<Vec<(String, Value)>>,
    malformed: bool,
    judge_reject: bool,
    delay: Option<Arc<tokio::sync::Notify>>,
    block_second: bool,
}
async fn endpoint(f: Arc<Fixture>) -> (String, tokio::task::JoinHandle<()>) {
    let router = axum::Router::new().fallback(axum::routing::post(move |uri: axum::http::Uri, axum::Json(body): axum::Json<Value>| {
        let f = f.clone();
        async move {
            f.requests.lock().unwrap().push((uri.path().into(), body.clone()));
            let system = body["messages"][0]["content"].as_str().unwrap_or("");
            let text = if system.contains("evidence-backed working memory") {
                if let Some(delay) = &f.delay {
                    let count = f.requests.lock().unwrap().iter().filter(|(_, b)| b["messages"][0]["content"].as_str().unwrap_or("").contains("evidence-backed working memory")).count();
                    if !f.block_second || count == 2 {
                        delay.notified().await;
                    }
                }
                if f.malformed {
                    "not JSON".into()
                } else {
                    let payload: Value = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
                    let source = &payload["records"][0];
                    let quote = source["text"].as_str().unwrap().chars().take(60).collect::<String>();
                    json!({"entries":[
                        {"section":"constraints","text":"Preserve cedar-47; proposed changes still require checking.","evidence":[{"record":source["record"],"quote":quote}]},
                        {"section":"progress","text":"Invented passing test result","evidence":[{"record":source["record"],"quote":"not in the source at all"}]}
                    ]})
                    .to_string()
                }
            } else if system.contains("Check each coding-memory entry") {
                json!({"supported":if f.judge_reject {vec![]}else{vec![0]}}).to_string()
            } else {
                "Foreground answer".into()
            };
            axum::Json(completion(&text))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let job = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (base, job)
}
fn seed(engine: &Engine, root: &std::path::Path, config: Config) -> Session {
    let mut s = Session::new(root.into(), config);
    for i in 0..10 {
        s.user(&format!("Keep cedar-47. User constraint {i}: {}", "context λ🌿 ".repeat(180)));
        s.context.push(json!({"role":"assistant","content":format!("Step {i}: proposal, not executed. {}", "detail ".repeat(180)),"reasoning_content":format!("reasoning-{i}")}));
    }
    engine.store.lock().unwrap().save(&mut s, "created", json!({})).unwrap();
    s
}
async fn finish(engine: &Engine) {
    tokio::time::timeout(std::time::Duration::from_secs(15), engine.quiesce()).await.unwrap();
}
async fn compact(engine: &Arc<Engine>, sid: &str) {
    engine.command(Command::Compact { session: sid.into() }).await.unwrap();
    finish(engine).await;
    let s = engine.get(sid).unwrap();
    assert_eq!(s.status, Status::Idle, "{:?}", s.error);
}
#[tokio::test]
async fn checkpoints_preserve_exact_sources_across_repeated_compaction_restore_fork_and_restart() {
    let dir = Scratch::new();
    let f = Arc::new(Fixture::default());
    let (base, server) = endpoint(f.clone()).await;
    let engine = Engine::open(dir.0.join("state")).unwrap();
    let original = seed(&engine, &dir.0, custom(&base));
    compact(&engine, &original.id).await;
    let first = engine.get(&original.id).unwrap();
    assert!(first.context.len() < original.context.len());
    let cp = engine.store.lock().unwrap().compactions(&original.id, 0, first.active_checkpoint.as_deref()).unwrap();
    assert_eq!(cp["status"], "applied");
    assert_eq!(cp["entries"][0]["verified"], true);
    assert!(!cp.to_string().contains("Invented passing"));
    assert!(first.context[0]["content"].as_str().unwrap().contains("transcript:"));
    assert_eq!(engine.store.lock().unwrap().transcript_items(&original.id, &original.context_refs).unwrap(), original.context);
    assert!(!engine.store.lock().unwrap().search_transcript(&original.id, "constraint 3").unwrap().as_array().unwrap().is_empty());
    let mut grown = first.clone();
    for i in 0..8 {
        grown.user(&format!("Correction {i}: {}", "new constraint ".repeat(200)));
    }
    engine.store.lock().unwrap().save(&mut grown, "user", json!({})).unwrap();
    compact(&engine, &original.id).await;
    let second = engine.get(&original.id).unwrap();
    let second_cp = engine.store.lock().unwrap().compactions(&original.id, 0, second.active_checkpoint.as_deref()).unwrap();
    assert_eq!(second_cp["parent"], first.active_checkpoint.clone().unwrap());
    // A fork keeps bounded context and inherits immutable source ranges.
    let fork = engine.command(Command::Fork { session: original.id.clone(), title: None }).await.unwrap();
    let fork = engine.get(fork["id"].as_str().unwrap()).unwrap();
    assert_eq!(fork.active_checkpoint, second.active_checkpoint);
    assert_eq!(fork.context, second.context);
    assert_eq!(engine.store.lock().unwrap().transcript_items(&fork.id, &original.context_refs).unwrap(), original.context);
    assert_eq!(engine.store.lock().unwrap().transcript_items(&fork.id, &fork.context_refs).unwrap(), fork.context);
    // Context restore retains new suffix messages and never replays their effects.
    let mut later = second.clone();
    later.user("A new steering message after compaction");
    engine.store.lock().unwrap().save(&mut later, "user", json!({})).unwrap();
    assert!(engine.store.lock().unwrap().search_transcript(&fork.id, "A new steering message").unwrap().as_array().unwrap().is_empty());
    let mut fork_growth = engine.get(&fork.id).unwrap();
    for i in 0..6 {
        fork_growth.user(&format!("Fork-only work {i}: {}", "new source ".repeat(200)));
    }
    engine.store.lock().unwrap().save(&mut fork_growth, "user", json!({})).unwrap();
    compact(&engine, &fork.id).await;
    let fork_checkpoint = engine.get(&fork.id).unwrap().active_checkpoint.unwrap();
    engine.command(Command::RestoreCompaction { session: fork.id.clone(), checkpoint: fork_checkpoint }).await.unwrap();
    let inherited = second.active_checkpoint.clone().unwrap();
    engine.command(Command::RestoreCompaction { session: fork.id.clone(), checkpoint: inherited.clone() }).await.unwrap();
    assert_eq!(engine.store.lock().unwrap().compactions(&fork.id, 0, Some(&inherited)).unwrap()["status"], "restored");
    assert_eq!(engine.store.lock().unwrap().compactions(&original.id, 0, Some(&inherited)).unwrap()["status"], "applied");
    let descendant = engine.command(Command::Fork { session: fork.id.clone(), title: None }).await.unwrap();
    let descendant = descendant["id"].as_str().unwrap();
    assert_eq!(engine.store.lock().unwrap().transcript_items(descendant, &original.context_refs).unwrap(), original.context);

    engine.command(Command::RestoreCompaction { session: original.id.clone(), checkpoint: second.active_checkpoint.unwrap() }).await.unwrap();
    engine.command(Command::RestoreCompaction { session: original.id.clone(), checkpoint: first.active_checkpoint.unwrap() }).await.unwrap();
    let restored = engine.get(&original.id).unwrap();
    assert_eq!(&restored.context[..original.context.len()], original.context.as_slice());
    assert_eq!(restored.context.last().unwrap()["content"], "A new steering message after compaction");
    let reads = f.requests.lock().unwrap();
    assert!(reads.iter().all(|(path, _)| path == "/v1/chat/completions"));
    drop(reads);
    drop(engine);
    let reopened = Engine::open(dir.0.join("state")).unwrap();
    assert_eq!(reopened.get(&original.id).unwrap().context_refs, restored.context_refs);
    assert_eq!(reopened.store.lock().unwrap().transcript_items(&original.id, &original.context_refs).unwrap(), original.context);
    server.abort();
}
#[tokio::test]
async fn malformed_or_rejected_summaries_fall_back_to_labeled_original_excerpts() {
    for (malformed, judge_reject) in [(true, false), (false, true)] {
        let dir = Scratch::new();
        let f = Arc::new(Fixture { malformed, judge_reject, ..Default::default() });
        let (base, server) = endpoint(f).await;
        let e = Engine::open(dir.0.join("state")).unwrap();
        let s = seed(&e, &dir.0, custom(&base));
        compact(&e, &s.id).await;
        let state = e.get(&s.id).unwrap();
        let cp = e.store.lock().unwrap().compactions(&s.id, 0, state.active_checkpoint.as_deref()).unwrap();
        assert!(cp["entries"].as_array().unwrap().iter().all(|e| e["verified"] == false && e["text"].as_str().unwrap().starts_with("Source excerpt")));
        assert_eq!(e.store.lock().unwrap().transcript_items(&s.id, &s.context_refs).unwrap(), s.context);
        server.abort();
    }
}
#[tokio::test]
async fn background_compaction_allows_foreground_progress_and_stop_cancels_it() {
    let dir = Scratch::new();
    let f = Arc::new(Fixture { delay: Some(Arc::new(tokio::sync::Notify::new())), ..Default::default() });
    let (base, server) = endpoint(f.clone()).await;
    let e = Engine::open(dir.0.join("state")).unwrap();
    let mut config = custom(&base);
    config.model.auto_compact = true;
    config.model.compact_at = 30000;
    config.compaction.prepare_percent = 25;
    let s = seed(&e, &dir.0, config);
    let mut events = e.events.subscribe();
    e.command(Command::Send { session: s.id.clone(), text: "Continue".into() }).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.kind == "assistant" {
                break;
            }
        }
        loop {
            if f.requests.lock().unwrap().iter().any(|(_, b)| b["messages"][0]["content"].as_str().unwrap_or("").contains("evidence-backed")) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let started = std::time::Instant::now();
    e.command(Command::Stop { session: s.id.clone() }).await.unwrap();
    finish(&e).await;
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    let after = e.get(&s.id).unwrap();
    assert!(after.active_checkpoint.is_none());
    assert_eq!(e.store.lock().unwrap().transcript_items(&s.id, &s.context_refs).unwrap(), s.context);
    let history = e.store.lock().unwrap().compactions(&s.id, 0, None).unwrap();
    assert_eq!(history["checkpoints"][0]["status"], "interrupted");
    server.abort();
}
#[test]
fn transcript_pagination_round_trips_every_unicode_byte_and_is_session_scoped() {
    let dir = Scratch::new();
    let mut store = Store::open(&dir.0).unwrap();
    let mut s = Session::new(dir.0.clone(), Config::default());
    s.context.push(json!({"role":"assistant","content":"🌿λ中".repeat(100),"encrypted_content":"opaque+/=","unknown":{"keep":true}}));
    store.save(&mut s, "test", json!({})).unwrap();
    let mut bytes = String::new();
    let mut offset = 0;
    loop {
        let page = store.transcript(&s.id, 0, 1, Some(s.context_refs[0]), offset).unwrap();
        bytes.push_str(page["text"].as_str().unwrap());
        match page["next_offset"].as_u64() {
            Some(next) => {
                assert!(next as usize > offset);
                offset = next as usize
            }
            None => break,
        }
    }
    assert_eq!(serde_json::from_str::<Value>(&bytes).unwrap(), s.context[0]);
    let middle = bytes.find('🌿').unwrap() + 1;
    assert!(store.transcript(&s.id, 0, 16, Some(s.context_refs[0]), middle).is_err());
    assert!(store.transcript(&s.id, 0, 16, Some(s.context_refs[0]), bytes.len() + 1).is_err());
    let eof = store.transcript(&s.id, 0, 16, Some(s.context_refs[0]), bytes.len()).unwrap();
    assert_eq!(eof["text"], "");
    assert!(eof["next_offset"].is_null());
    assert!(store.transcript("another-session", 0, 0, Some(s.context_refs[0]), 0).is_err());
    assert!(store.search_transcript(&s.id, "🌿λ中").unwrap().as_array().unwrap().len() == 1);
}
#[test]
fn compaction_boundaries_never_split_parallel_calls_from_results() {
    let mut s = Session::new("/var/tmp".into(), custom("http://127.0.0.1:1/v1"));
    s.config.compaction.keep_recent_tokens = 0;
    s.config.compaction.keep_recent_exchanges = 1;
    s.user("old".repeat(10000).as_str());
    s.context.push(json!({"role":"assistant","tool_calls":[{"id":"a"},{"id":"b"}]}));
    s.context.push(json!({"role":"tool","tool_call_id":"a","content":"first"}));
    s.context.push(json!({"role":"tool","tool_call_id":"b","content":"second"}));
    s.user("next");
    assert_eq!(compaction::cut(&s), Some(4));
    s.context.remove(3);
    assert_eq!(compaction::cut(&s), None);
}
#[test]
fn source_validation_rejects_fabricated_ids_quotes_and_claim_shapes() {
    use compaction::{Entry, Quote};
    let good = Entry { section: "constraints".into(), text: "Keep marker".into(), evidence: vec![Quote { record: 7, quote: "cedar-47".into() }], verified: false };
    let mut bad_id = good.clone();
    bad_id.evidence[0].record = 8;
    let mut bad_quote = good.clone();
    bad_quote.evidence[0].quote = "invented".into();
    let mut bad_section = good.clone();
    bad_section.section = "approval".into();
    let mut entries = vec![good, bad_id, bad_quote, bad_section];
    compaction::validate_entries(&mut entries, &[(7, "Keep cedar-47 intact".into())].into());
    assert_eq!(entries.len(), 1);
}
#[test]
fn custom_reasoning_tool_results_and_usage_survive_native_replay() {
    let message = json!({"role":"assistant","content":null,"reasoning_content":"original reasoning λ","tool_calls":[{"id":"c1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"main.rs\"}"}}],"future_reasoning":{"opaque":"retain"}});
    let reply = provider::parse(
        Provider::Openai,
        json!({"choices":[{"message":message,"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":20,"prompt_tokens_details":{"cached_tokens":70}}}),
    )
    .unwrap();
    let mut s = Session::new("/var/tmp".into(), custom("http://127.0.0.1:1/v1"));
    s.context = reply.items;
    s.result(&reply.calls[0], "source", false);
    let before = provider::body(&s, "stable", &[]);
    s.memory = "current correction".into();
    let after = provider::body(&s, "stable", &[]);
    assert_eq!(before["messages"][1], message);
    assert_eq!(after["messages"][1], message);
    assert_eq!(after["messages"][2]["tool_call_id"], "c1");
    assert_eq!(reply.usage.cached, 70);
    assert_eq!(reply.usage.context, 120);
    assert_eq!(after["n"], 1);
    assert_eq!(after["max_tokens"], s.config.model.max_output_tokens);
}
#[tokio::test]
async fn custom_stream_preserves_reasoning_and_requires_completion_before_tools() {
    let chunks = [
        json!({"choices":[{"index":0,"delta":{"role":"assistant","reasoning_content":"think λ"}}]}),
        json!({"choices":[{"index":0,"delta":{"reasoning_content":" 🌿","tool_calls":[{"index":0,"id":"a","type":"function","function":{"name":"read_file","arguments":"{\"path\":"}}]}}]}),
        json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"file.rs\"}"}}]},"finish_reason":"tool_calls"}]}),
        json!({"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5}}),
    ];
    for (finish, done, valid) in [(true, true, true), (true, false, false), (false, true, false)] {
        let wire = chunks.iter().enumerate().filter(|(i, _)| finish || *i != 2).map(|(_, v)| format!("data: {v}\r\n\r\n")).collect::<String>() + if done { "data: [DONE]\n\n" } else { "" };
        let router = axum::Router::new().fallback(axum::routing::post(move || {
            let wire = wire.clone();
            async move { ([("content-type", "text/event-stream")], wire) }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let job = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let s = Session::new("/var/tmp".into(), custom(&base));
        let result = provider::Client::new().unwrap().turn(&s, "test", &[], &CancellationToken::new(), |_, _| {}).await;
        assert_eq!(result.is_ok(), valid, "{result:?}");
        if let Ok(reply) = result {
            assert_eq!(reply.items[0]["reasoning_content"], "think λ 🌿");
            assert_eq!(reply.calls[0].arguments["path"], "file.rs");
            assert_eq!(reply.usage.input, 10);
        }
        job.abort();
    }
}
#[test]
fn endpoint_validation_rejects_credential_urls_and_ambiguous_authentication() {
    for url in ["http://example.com/v1", "https://user:password@example.com/v1", "https://example.com/v1?key=secret", "https://example.com/v1#fragment"] {
        assert!(custom(url).validate().is_err(), "{url}");
    }
    let mut c = custom("https://api.fireworks.ai/inference/v1");
    assert!(c.validate().is_ok());
    let e = c.model.endpoint.as_mut().unwrap();
    e.api_key_env = Some("FIREWORKS_API_KEY".into());
    e.header_env.insert("Authorization".into(), "OTHER_KEY".into());
    assert!(c.validate().is_err());
    let mut c = custom("http://[::1]:8000/v1");
    assert!(c.validate().is_ok());
    c.model.endpoint.as_mut().unwrap().api_key_env = Some("actual secret value".into());
    assert!(c.validate().is_err());
    assert!(serde_json::from_value::<Provider>(json!("claude")).is_err());
}

#[test]
fn removed_provider_migration_preserves_originals_and_pauses_instead_of_translating_them() {
    let dir = Scratch::new();
    let mut store = Store::open(&dir.0).unwrap();
    let mut s = Session::new(dir.0.clone(), Config::default());
    s.user("keep my requirement");
    s.memory = "durable note".into();
    store.save(&mut s, "created", json!({})).unwrap();
    drop(store);
    let db = rusqlite::Connection::open(dir.0.join("sessions.sqlite3")).unwrap();
    let mut old = serde_json::to_value(&s).unwrap();
    old["config"]["model"]["provider"] = "claude".into();
    old["config"]["model"]["cache_ttl"] = "1h".into();
    old["context"] = json!([{"role":"assistant","content":[{"type":"thinking","signature":"original-signature","thinking":"old reason"}]}]);
    db.execute("UPDATE sessions SET body=? WHERE id=?", rusqlite::params![old.to_string(), s.id]).unwrap();
    db.execute_batch("DROP TRIGGER transcript_insert; DROP TABLE transcript_search; DROP TABLE transcript; DROP TABLE compactions; PRAGMA user_version=4;").unwrap();
    drop(db);
    let e = Engine::open(dir.0.clone()).unwrap();
    let migrated = e.get(&s.id).unwrap();
    assert_eq!(migrated.config.model.provider, Provider::Codex);
    assert_eq!(migrated.status, Status::Paused);
    assert!(migrated.context.is_empty());
    assert_eq!(migrated.memory, "durable note");
    let events = e.store.lock().unwrap().events(&s.id, 0, 100).unwrap();
    assert!(events.iter().any(|e| e.kind == "provider_removed" && e.data["previous"]["context"] == old["context"]));
    let list = e.store.lock().unwrap().transcript(&s.id, 0, 0, None, 0).unwrap();
    let record = list["records"][0]["id"].as_i64().unwrap();
    assert_eq!(e.store.lock().unwrap().transcript_items(&s.id, &[record]).unwrap(), old["context"].as_array().unwrap().clone());
}

#[tokio::test]
async fn custom_endpoint_auth_catalog_and_full_tool_loop_use_the_configured_route() {
    let dir = Scratch::new();
    std::fs::write(dir.0.join("source.rs"), "fn main() {}\n").unwrap();
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let received = calls.clone();
    // Existing non-secret environment values exercise header resolution without
    // modifying the test process environment while parallel tests are running.
    let expected = std::env::var("PATH").unwrap();
    let app=axum::Router::new().fallback(axum::routing::any(move |method:axum::http::Method,uri:axum::http::Uri,headers:axum::http::HeaderMap,body:axum::body::Bytes|{
        let calls=received.clone();let expected=expected.clone();
        async move {
            assert!(headers.get("authorization").is_some_and(|v|v.as_bytes()==format!("Bearer {expected}").as_bytes()));
            if method==axum::http::Method::GET {assert_eq!(uri.path(),"/prefix/v1/models");return axum::Json(json!({"data":[{"id":"custom-deployment"}]}));}
            assert_eq!(uri.path(),"/prefix/v1/chat/completions");
            let body:Value=serde_json::from_slice(&body).unwrap();let n={let mut c=calls.lock().unwrap();c.push(body.clone());c.len()};
            if n==1 {axum::Json(json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"reasoning_content":"original reasoning","tool_calls":[{"id":"read","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"source.rs\"}"}}]}}]}))}
            else {assert!(body["messages"].as_array().unwrap().iter().any(|v|v["role"]=="tool"&&v["tool_call_id"]=="read"));assert!(body["messages"].as_array().unwrap().iter().any(|v|v["reasoning_content"]=="original reasoning"));axum::Json(completion("File inspected"))}
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/prefix/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let e = Engine::open(dir.0.join("state")).unwrap();
    let mut config = custom(&base);
    config.model.endpoint.as_mut().unwrap().api_key_env = Some("PATH".into());
    let sid = e.command(Command::Create { workspace: dir.0.clone(), config: Some(config) }).await.unwrap()["id"].as_str().unwrap().to_owned();
    let catalog = e.command(Command::Models { provider: Provider::Openai, session: Some(sid.clone()) }).await.unwrap();
    assert_eq!(catalog["data"][0]["id"], "custom-deployment");
    e.command(Command::Send { session: sid.clone(), text: "Read the file".into() }).await.unwrap();
    finish(&e).await;
    let s = e.get(&sid).unwrap();
    assert_eq!(s.status, Status::Idle, "{:?}", s.error);
    assert_eq!(s.turns, 2);
    let history = e.store.lock().unwrap().events(&sid, 0, 100).unwrap();
    assert_eq!(history.iter().filter(|v| v.kind == "run_context").count(), 1);
    let requests = history.iter().filter(|v| v.kind == "model_request").collect::<Vec<_>>();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].data["context_refs"], json!([s.context_refs[0]]));
    assert_eq!(s.context.len(), s.context_refs.len());
    assert_eq!(e.store.lock().unwrap().transcript_items(&sid, &s.context_refs).unwrap(), s.context);
    assert!(!serde_json::to_string(&s.config).unwrap().contains(&std::env::var("PATH").unwrap()));
    server.abort();
}

#[test]
#[ignore = "release performance measurement; run with --ignored --nocapture"]
fn transcript_archive_release_cost() {
    let dir = Scratch::new();
    let mut db = Store::open(&dir.0).unwrap();
    let mut s = Session::new(dir.0.clone(), Config::default());
    let start = std::time::Instant::now();
    for i in 0..100 {
        s.user(&format!("record {i}: {}", "source ".repeat(200)));
        db.save(&mut s, "user", json!({"index":i})).unwrap();
    }
    let archive = start.elapsed();
    let start = std::time::Instant::now();
    for _ in 0..100 {
        db.save(&mut s, "boundary", json!({})).unwrap();
    }
    let unchanged = start.elapsed();
    let start = std::time::Instant::now();
    for _ in 0..100 {
        assert_eq!(db.search_transcript(&s.id, "record 57").unwrap().as_array().unwrap().len(), 1);
    }
    println!("100 growing archive commits: {archive:?}; 100 unchanged-context commits: {unchanged:?}; 100 indexed searches: {:?}", start.elapsed());
    assert_eq!(db.transcript(&s.id, 0, 100, None, 0).unwrap()["records"].as_array().unwrap().len(), 100);
    s.user(&"large native record ".repeat(100_000));
    db.save(&mut s, "user", json!({})).unwrap();
    let record = *s.context_refs.last().unwrap();
    let connection = rusqlite::Connection::open(dir.0.join("sessions.sqlite3")).unwrap();
    let start = std::time::Instant::now();
    for _ in 0..100 {
        let item: String = connection.query_row("SELECT item FROM transcript WHERE id=?", [record], |r| r.get(0)).unwrap();
        std::hint::black_box(json!({"record":record,"text":&item[..16000],"total_bytes":item.len(),"next_offset":16000}));
    }
    let before = start.elapsed();
    let start = std::time::Instant::now();
    for _ in 0..100 {
        std::hint::black_box(db.transcript(&s.id, 0, 16000, Some(record), 0).unwrap());
    }
    println!("100 first-page reads of a 2 MB record: whole-record baseline {before:?}; incremental pages {:?}", start.elapsed());
}

#[tokio::test]
async fn interrupted_preparation_resumes_durable_chunks_after_restart() {
    let dir = Scratch::new();
    let f = Arc::new(Fixture { delay: Some(Arc::new(tokio::sync::Notify::new())), block_second: true, ..Default::default() });
    let (base, server) = endpoint(f.clone()).await;
    let e = Engine::open(dir.0.join("state")).unwrap();
    let mut config = custom(&base);
    config.compaction.chunk_tokens = 1024;
    let s = seed(&e, &dir.0, config);
    let mut events = e.events.subscribe();
    e.command(Command::Compact { session: s.id.clone() }).await.unwrap();
    let checkpoint = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.kind == "compaction_chunk" {
                break event.data["checkpoint"]["id"].as_str().unwrap().to_owned();
            }
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if f.requests.lock().unwrap().iter().filter(|(_, b)| b["messages"][0]["content"].as_str().unwrap_or("").contains("evidence-backed working memory")).count() >= 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    e.command(Command::Stop { session: s.id.clone() }).await.unwrap();
    finish(&e).await;
    let partial = e.store.lock().unwrap().compactions(&s.id, 0, Some(&checkpoint)).unwrap();
    assert_eq!(partial["completed_chunks"], 1);
    assert_eq!(partial["status"], "interrupted");
    drop(e);
    let e = Engine::open(dir.0.join("state")).unwrap();
    compact(&e, &s.id).await;
    let state = e.get(&s.id).unwrap();
    assert_eq!(state.active_checkpoint.as_deref(), Some(checkpoint.as_str()));
    let ready = e.store.lock().unwrap().compactions(&s.id, 0, Some(&checkpoint)).unwrap();
    assert_eq!(ready["completed_chunks"], ready["chunk_count"]);
    assert_eq!(e.store.lock().unwrap().transcript_items(&s.id, &s.context_refs).unwrap(), s.context);
    server.abort();
}

#[test]
#[ignore = "release performance measurement; run with --ignored --nocapture"]
fn fork_large_archive_release_cost() {
    let dir = Scratch::new();
    let mut db = Store::open(&dir.0).unwrap();
    let mut source = Session::new(dir.0.clone(), Config::default());
    for i in 0..10000 {
        source.user(&format!("Original record {i}: {}", "source ".repeat(180)));
    }
    db.save(&mut source, "created", json!({})).unwrap();
    let first = source.context_refs[0];
    source.context.drain(..9994);
    source.context_refs.drain(..9994);
    db.save(&mut source, "compacted", json!({})).unwrap();
    let started = std::time::Instant::now();
    for _ in 0..10 {
        let mut fork = source.clone();
        fork.parent = Some(source.id.clone());
        fork.id = id();
        db.save(&mut fork, "forked", json!({})).unwrap();
        assert_eq!(fork.context.len(), 6);
        assert_eq!(db.transcript_items(&fork.id, &[first]).unwrap()[0]["content"].as_str().unwrap().split(':').next().unwrap(), "Original record 0");
    }
    println!("10 forks of a 10,000-record archive with six active items: {:?}", started.elapsed());
    let inspect = rusqlite::Connection::open(dir.0.join("sessions.sqlite3")).unwrap();
    let records: i64 = inspect.query_row("SELECT count(*) FROM transcript", [], |r| r.get(0)).unwrap();
    assert_eq!(records, 10000, "forks share immutable source ranges without copying transcript payloads");
}

#[tokio::test]
async fn agent_retrieves_exact_transcript_and_large_checkpoint_with_a_small_tool_budget() {
    let dir = Scratch::new();
    let e = Engine::open(dir.0.join("state")).unwrap();
    let mut config = custom("http://127.0.0.1:1/v1");
    config.max_tool_bytes = 1024;
    config.max_turns = 200;
    let mut s = Session::new(dir.0.clone(), config);
    s.user(&"Exact source \0\n\"\\🌿λ ".repeat(80));
    e.store.lock().unwrap().save(&mut s, "created", json!({})).unwrap();
    let original = s.context[0].to_string();
    let record = s.context_refs[0];
    let checkpoint = id();
    let detail = json!({"id":checkpoint,"created":now(),"status":"ready","entries":[{"text":"Evidence with escapes \0\n\"\\🌿λ ".repeat(120)}],"prefix_refs":[record]});
    e.store.lock().unwrap().save(&mut s, "compaction_ready", json!({"checkpoint":detail})).unwrap();
    let recovered = Arc::new(Mutex::new((String::new(), String::new(), 0usize)));
    let captured = recovered.clone();
    let router = axum::Router::new().fallback(axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
        let captured = captured.clone();
        let checkpoint = checkpoint.clone();
        async move {
            let messages = body["messages"].as_array().unwrap();
            let last = messages.last().unwrap();
            let mut memory = captured.lock().unwrap();
            let (name, args) = if last["role"] != "tool" {
                ("read_transcript", json!({"record":record,"limit":64000}))
            } else {
                let content = last["content"].as_str().unwrap();
                assert!(content.len() <= 1024, "encoded result exceeds configured budget");
                let page: Value = serde_json::from_str(content).expect("model must receive complete JSON");
                let call = &messages[messages.len()-2]["tool_calls"][0]["function"];
                let args: Value = serde_json::from_str(call["arguments"].as_str().unwrap()).unwrap();
                match call["name"].as_str().unwrap() {
                    "read_transcript" => {
                        memory.0.push_str(page["text"].as_str().expect("transcript pages should fit directly"));
                        if let Some(offset) = page["next_offset"].as_u64() {
                            ("read_transcript", json!({"record":record,"limit":64000,"offset":offset}))
                        } else {
                            ("compaction_history", json!({"checkpoint":checkpoint}))
                        }
                    }
                    "compaction_history" => {
                        assert_eq!(page["truncated"], true);
                        assert_eq!(page["read_with"], "read_artifact");
                        ("read_artifact", json!({"id":page["artifact"],"limit":64000}))
                    }
                    "read_artifact" => {
                        memory.1.push_str(page["text"].as_str().unwrap());
                        if let Some(offset) = page["next_offset"].as_u64() {
                            ("read_artifact", json!({"id":args["id"],"limit":64000,"offset":offset}))
                        } else {
                            return axum::Json(completion("Retrieved every source byte."));
                        }
                    }
                    other => panic!("unexpected tool {other}"),
                }
            };
            memory.2 += 1;
            axum::Json(json!({"choices":[{"index":0,"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":format!("read-{}",memory.2),"type":"function","function":{"name":name,"arguments":args.to_string()}}]}}]}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    s.config.model.endpoint.as_mut().unwrap().base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    e.store.lock().unwrap().save(&mut s, "configured", json!({})).unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    e.command(Command::Send { session: s.id.clone(), text: "Retrieve the source and checkpoint exactly.".into() }).await.unwrap();
    finish(&e).await;
    let state = e.get(&s.id).unwrap();
    assert_eq!(state.status, Status::Idle, "{:?}", state.error);
    let memory = recovered.lock().unwrap();
    assert_eq!(memory.0, original);
    assert_eq!(serde_json::from_str::<Value>(&memory.1).unwrap(), detail);
    assert!(memory.2 > 2);
    server.abort();
}
