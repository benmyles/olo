//! Web Workers: each `new Worker(url)` is a thread with its own isolate and
//! a worker global scope (`worker.js` over the shared Web API pieces),
//! fetching its script and what it imports through the browser's fetcher.
//! Messages are structured clones (bytes) over channels; the page's realm
//! drains them when it advances, and the browser is woken when one is
//! posted so that happens promptly.
use crate::{Fetched, clone, exception_text, string};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};

/// Fetches a URL (method, body) for a worker, on its own thread; the
/// browser supplies one.
/// (url, method, body, headers as `name: value` lines).
pub type SharedFetcher = Arc<dyn Fn(&str, &str, &[u8], &str) -> Result<Fetched, String> + Send + Sync>;
/// Starts a worker's fetch() without a thread waiting on it (the browser's
/// network runtime does the I/O): (url, method, body, headers), and what
/// to call with what came.
pub type FetchStarter = Arc<dyn Fn(String, String, Vec<u8>, String, Box<dyn FnOnce(Result<Fetched, String>) + Send>) + Send + Sync>;
/// Called from a worker's thread when it posted something, so the page's
/// loop runs soon.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

const PRELUDE: &str = concat!(include_str!("shared.js"), "\n", include_str!("worker.js"));

pub enum ToWorker {
    Message(Vec<u8>),
    Terminate,
    /// A fetch the worker's scripts started (`fetch()`), answered from the
    /// thread that did it: its number and what came.
    Fetched(u32, Result<Fetched, String>),
}
pub enum FromWorker {
    Message(Vec<u8>),
    Error(String),
    Console(String),
}
/// The page's end of a worker.
pub struct Handle {
    tx: Sender<ToWorker>,
    rx: Receiver<FromWorker>,
    isolate: Arc<Mutex<Option<v8::IsolateHandle>>>,
}
impl Handle {
    /// Start a worker for the script at `url` (or `source`, for a blob
    /// URL's script), fetching through `fetcher`.
    pub fn spawn(url: String, source: Option<String>, fetcher: SharedFetcher, starter: Option<FetchStarter>, waker: Option<Waker>, user_agent: String) -> Result<Self, String> {
        let (to_tx, to_rx) = mpsc::channel();
        let (from_tx, from_rx) = mpsc::channel();
        let isolate = Arc::new(Mutex::new(None));
        let slot = isolate.clone();
        let name = url.rsplit('/').next().unwrap_or("worker").chars().take(15).collect::<String>();
        let to_self = to_tx.clone();
        std::thread::Builder::new()
            .name(format!("worker-{name}"))
            .spawn(move || run(url, source, fetcher, starter, waker, user_agent, to_rx, to_self, from_tx, slot))
            .map_err(|e| format!("worker thread: {e}"))?;
        Ok(Self { tx: to_tx, rx: from_rx, isolate })
    }
    pub fn post(&self, bytes: Vec<u8>) {
        let _ = self.tx.send(ToWorker::Message(bytes));
    }
    pub fn try_recv(&self) -> Option<FromWorker> {
        self.rx.try_recv().ok()
    }
    /// Stop the worker: its loop ends, a running script is terminated.
    pub fn terminate(&self) {
        let _ = self.tx.send(ToWorker::Terminate);
        if let Some(handle) = self.isolate.lock().unwrap().as_ref() {
            handle.terminate_execution();
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.terminate();
    }
}
struct State {
    url: String,
    user_agent: String,
    fetcher: SharedFetcher,
    starter: Option<FetchStarter>,
    waker: Option<Waker>,
    tx: Sender<FromWorker>,
    /// The worker's own inbox: fetches answer through it.
    to_self: Sender<ToWorker>,
    next_fetch: u32,
    started: Instant,
    closed: bool,
}
type Shared = Rc<RefCell<State>>;
const NATIVES: &[&str] = &[
    "log",
    "postMessage",
    "fetchSync",
    "fetchStart",
    "runScript",
    "now",
    "url",
    "userAgent",
    "close",
    "digest",
    "hmac",
    "randomBytes",
    "aes",
    "pbkdf2",
    "hkdf",
    "machine",
    "brands",
    "chromeVersion",
    "isProxy",
    "builtin",
    "errorFrames",
];

#[allow(clippy::too_many_arguments)]
fn run(
    url: String,
    source: Option<String>,
    fetcher: SharedFetcher,
    starter: Option<FetchStarter>,
    waker: Option<Waker>,
    user_agent: String,
    rx: Receiver<ToWorker>,
    to_self: Sender<ToWorker>,
    tx: Sender<FromWorker>,
    slot: Arc<Mutex<Option<v8::IsolateHandle>>>,
) {
    crate::runtime();
    let state: Shared =
        Rc::new(RefCell::new(State { url: url.clone(), user_agent, fetcher: fetcher.clone(), starter, waker, tx: tx.clone(), to_self, next_fetch: 0, started: Instant::now(), closed: false }));
    // The script first: a worker that cannot load reports an error.
    let source = match source {
        Some(s) => s,
        None => match fetcher(&url, "GET", &[], "") {
            Ok(f) if f.status < 400 => String::from_utf8_lossy(&f.body).into_owned(),
            Ok(f) => {
                let _ = tx.send(FromWorker::Error(format!("worker script {url}: HTTP {}", f.status)));
                return;
            }
            Err(e) => {
                let _ = tx.send(FromWorker::Error(format!("worker script {url}: {e}")));
                return;
            }
        },
    };
    let mut isolate = v8::Isolate::new(v8::CreateParams::default());
    *slot.lock().unwrap() = Some(isolate.thread_safe_handle());
    isolate.set_slot(state.clone());
    let context = {
        v8::scope!(let scope, &mut isolate);
        let context = v8::Context::new(scope, Default::default());
        let scope = &mut v8::ContextScope::new(scope, context);
        if let Err(e) = install(scope) {
            let _ = tx.send(FromWorker::Error(format!("worker scope: {e}")));
            return;
        }
        v8::Global::new(scope, context)
    };
    let mut worker = Worker { isolate, context, state: state.clone() };
    if let Err(e) = worker.run(PRELUDE, "worker prelude") {
        let _ = tx.send(FromWorker::Error(format!("worker prelude: {e}")));
        return;
    }
    if let Err(e) = worker.run(&source, &url) {
        let _ = tx.send(FromWorker::Error(e));
    }
    // The event loop: timers on real time, messages as they come.
    loop {
        if state.borrow().closed {
            break;
        }
        let now = state.borrow().started.elapsed().as_secs_f64() * 1000.;
        let next = worker.tick(now);
        let message = match next {
            Some(due) if due > now => rx.recv_timeout(Duration::from_secs_f64((due - now) / 1000.)).ok(),
            Some(_) => rx.try_recv().ok(),
            None => match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => break,
            },
        };
        match message {
            Some(ToWorker::Message(bytes)) => worker.deliver(&bytes),
            Some(ToWorker::Fetched(id, result)) => worker.fetched(id, result),
            Some(ToWorker::Terminate) => break,
            None => {
                if next.is_none() {
                    break;
                }
            }
        }
    }
}
struct Worker {
    isolate: v8::OwnedIsolate,
    context: v8::Global<v8::Context>,
    state: Shared,
}
impl Worker {
    fn run(&mut self, source: &str, name: &str) -> Result<(), String> {
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc, scope);
        let result = (|| {
            let code = v8::String::new(tc, source)?;
            let resource = string(tc, name);
            let origin = v8::ScriptOrigin::new(tc, resource, 0, 0, false, 0, None, false, false, false, None);
            let script = v8::Script::compile(tc, code, Some(&origin))?;
            script.run(tc)
        })();
        if result.is_none() {
            return Err(exception_text(tc, name));
        }
        Ok(())
    }
    /// Run the timers due at `now` (ms); the next due time, if any.
    fn tick(&mut self, now: f64) -> Option<f64> {
        let state = self.state.clone();
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc, scope);
        let global = context.global(tc);
        let name = v8::String::new(tc, "__olo_wtick")?;
        let f = global.get(tc, name.into()).and_then(|f| v8::Local::<v8::Function>::try_from(f).ok())?;
        let arg: v8::Local<v8::Value> = v8::Number::new(tc, now).into();
        match f.call(tc, global.into(), &[arg]) {
            Some(v) => {
                let next = v.number_value(tc).unwrap_or(-1.);
                if next < 0. { None } else { Some(next) }
            }
            None => {
                let error = exception_text(tc, "worker timers");
                let _ = state.borrow().tx.send(FromWorker::Error(error));
                None
            }
        }
    }
    /// A fetch the worker's scripts started came back: its promise settled.
    fn fetched(&mut self, id: u32, result: Result<Fetched, String>) {
        let state = self.state.clone();
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc, scope);
        let global = context.global(tc);
        let Some(name) = v8::String::new(tc, "__olo_fetched") else { return };
        let Some(f) = global.get(tc, name.into()).and_then(|f| v8::Local::<v8::Function>::try_from(f).ok()) else { return };
        let id = v8::Integer::new(tc, id as i32).into();
        let args: [v8::Local<v8::Value>; 3] = match result {
            Ok(fetched) => [id, fetched_object(tc, fetched), v8::null(tc).into()],
            Err(e) => [id, v8::null(tc).into(), string(tc, &e)],
        };
        if f.call(tc, global.into(), &args).is_none() {
            let error = exception_text(tc, "worker fetch");
            let _ = state.borrow().tx.send(FromWorker::Error(error));
        }
    }
    /// A message from the page: a `message` event on the worker's scope.
    fn deliver(&mut self, bytes: &[u8]) {
        let state = self.state.clone();
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc, scope);
        let global = context.global(tc);
        let Some(name) = v8::String::new(tc, "__olo_wmessage") else { return };
        let Some(f) = global.get(tc, name.into()).and_then(|f| v8::Local::<v8::Function>::try_from(f).ok()) else { return };
        let Some(value) = clone::deserialize(tc, bytes) else {
            let _ = state.borrow().tx.send(FromWorker::Error("message could not be read".into()));
            return;
        };
        if f.call(tc, global.into(), &[value]).is_none() {
            let error = exception_text(tc, "worker message");
            let _ = state.borrow().tx.send(FromWorker::Error(error));
        }
    }
}
fn install(scope: &mut v8::PinScope<'_, '_>) -> Result<(), String> {
    let global = scope.get_current_context().global(scope);
    let natives = v8::Object::new(scope);
    for name in NATIVES {
        let data: v8::Local<v8::Value> = v8::String::new(scope, name).ok_or("string")?.into();
        let f = v8::FunctionTemplate::builder(native).data(data).build(scope);
        let f = f.get_function(scope).ok_or("function")?;
        let key = v8::String::new(scope, name).ok_or("string")?;
        natives.set(scope, key.into(), f.into());
    }
    let key = v8::String::new(scope, "__olo").ok_or("string")?;
    global.set(scope, key.into(), natives.into());
    Ok(())
}
/// A fetch's answer as the prelude takes it: { status, contentType, body, url, headers }.
fn fetched_object<'s>(scope: &mut v8::PinScope<'s, '_>, fetched: Fetched) -> v8::Local<'s, v8::Value> {
    let object = v8::Object::new(scope);
    let pairs: [(&str, v8::Local<v8::Value>); 5] = [
        ("status", v8::Integer::new(scope, i32::from(fetched.status)).into()),
        ("contentType", string(scope, &fetched.content_type)),
        ("body", crate::bytes_array(scope, fetched.body, crate::ByteKind::U8)),
        ("url", string(scope, &fetched.url)),
        ("headers", string(scope, &fetched.headers)),
    ];
    for (k, v) in pairs {
        let key = v8::String::new(scope, k).unwrap();
        object.set(scope, key.into(), v);
    }
    object.into()
}
fn arg_string(scope: &mut v8::PinScope, args: &v8::FunctionCallbackArguments, i: i32) -> String {
    let v = args.get(i);
    if v.is_null_or_undefined() { String::new() } else { v.to_rust_string_lossy(scope) }
}
/// Every `__olo.*` function of a worker's scope.
fn native(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let Some(state) = scope.get_slot::<Shared>().cloned() else { return };
    let name = args.data().to_rust_string_lossy(scope);
    match name.as_str() {
        // Web Crypto, as the page's (see the page's natives).
        "digest" | "hmac" | "randomBytes" | "aes" | "pbkdf2" | "hkdf" => {
            let out = match name.as_str() {
                "aes" => {
                    let mode = arg_string(scope, &args, 0);
                    let encrypt = args.get(1).boolean_value(scope);
                    let counter = args.get(6).number_value(scope).unwrap_or(128.) as usize;
                    crate::crypto::aes(
                        &mode,
                        encrypt,
                        &crate::arg_body(scope, &args, 2),
                        &crate::arg_body(scope, &args, 3),
                        &crate::arg_body(scope, &args, 4),
                        &crate::arg_body(scope, &args, 5),
                        counter,
                    )
                }
                "pbkdf2" => {
                    let hash = arg_string(scope, &args, 0);
                    let iterations = args.get(3).number_value(scope).unwrap_or(0.).clamp(0., f64::from(u32::MAX)) as u32;
                    let bits = args.get(4).number_value(scope).unwrap_or(0.).max(0.) as usize;
                    crate::crypto::pbkdf2(&hash, &crate::arg_body(scope, &args, 1), &crate::arg_body(scope, &args, 2), iterations, bits)
                }
                "hkdf" => {
                    let hash = arg_string(scope, &args, 0);
                    let bits = args.get(4).number_value(scope).unwrap_or(0.).max(0.) as usize;
                    crate::crypto::hkdf(&hash, &crate::arg_body(scope, &args, 1), &crate::arg_body(scope, &args, 2), &crate::arg_body(scope, &args, 3), bits)
                }
                "digest" => {
                    let algorithm = arg_string(scope, &args, 0);
                    crate::crypto::digest(&algorithm, &crate::arg_body(scope, &args, 1))
                }
                "hmac" => {
                    let algorithm = arg_string(scope, &args, 0);
                    crate::crypto::hmac(&algorithm, &crate::arg_body(scope, &args, 1), &crate::arg_body(scope, &args, 2))
                }
                _ => Some(crate::crypto::random(args.get(0).number_value(scope).unwrap_or(0.).max(0.) as usize)),
            };
            match out {
                Some(bytes) => rv.set(crate::bytes_array(scope, bytes, crate::ByteKind::U8)),
                None => rv.set_null(),
            }
        }
        "log" => {
            let level = arg_string(scope, &args, 0);
            let text = arg_string(scope, &args, 1);
            let _ = state.borrow().tx.send(FromWorker::Console(format!("[{level}] {text}")));
        }
        "postMessage" => {
            if let Some(bytes) = clone::serialize(scope, args.get(0)) {
                let s = state.borrow();
                let _ = s.tx.send(FromWorker::Message(bytes));
                if let Some(w) = &s.waker {
                    w();
                }
            }
        }
        // fetchStart(url, method, body, headers): started on the browser's
        // network runtime (else fetched on a thread of its own), the
        // worker's event loop going on; its number, answered as
        // ToWorker::Fetched.
        "fetchStart" => {
            let url = arg_string(scope, &args, 0);
            let method = arg_string(scope, &args, 1);
            let body = crate::arg_body(scope, &args, 2);
            let headers = if args.length() > 3 && args.get(3).is_string() { arg_string(scope, &args, 3) } else { String::new() };
            let (fetcher, starter, to_self, id) = {
                let mut s = state.borrow_mut();
                s.next_fetch = s.next_fetch.wrapping_add(1);
                (s.fetcher.clone(), s.starter.clone(), s.to_self.clone(), s.next_fetch)
            };
            if let Some(start) = starter {
                start(url, if method.is_empty() { "GET".into() } else { method }, body, headers, Box::new(move |result| drop(to_self.send(ToWorker::Fetched(id, result)))));
                rv.set(v8::Integer::new(scope, id as i32).into());
                return;
            }
            let spawned = std::thread::Builder::new().name("worker-fetch".into()).spawn(move || {
                let result = fetcher(&url, if method.is_empty() { "GET" } else { &method }, &body, &headers);
                let _ = to_self.send(ToWorker::Fetched(id, result));
            });
            if spawned.is_err() {
                let message = v8::String::new(scope, "no thread for the fetch").unwrap();
                let exception = v8::Exception::error(scope, message);
                scope.throw_exception(exception);
                return;
            }
            rv.set(v8::Integer::new(scope, id as i32).into());
        }
        "fetchSync" => {
            let url = arg_string(scope, &args, 0);
            let method = arg_string(scope, &args, 1);
            let body = crate::arg_body(scope, &args, 2);
            let headers = if args.length() > 3 && args.get(3).is_string() { arg_string(scope, &args, 3) } else { String::new() };
            let fetcher = state.borrow().fetcher.clone();
            match fetcher(&url, if method.is_empty() { "GET" } else { &method }, &body, &headers) {
                Ok(fetched) => {
                    let object = v8::Object::new(scope);
                    let pairs: [(&str, v8::Local<v8::Value>); 5] = [
                        ("status", v8::Integer::new(scope, i32::from(fetched.status)).into()),
                        ("contentType", string(scope, &fetched.content_type)),
                        ("body", crate::bytes_array(scope, fetched.body, crate::ByteKind::U8)),
                        ("url", string(scope, &fetched.url)),
                        ("headers", string(scope, &fetched.headers)),
                    ];
                    for (k, v) in pairs {
                        let key = v8::String::new(scope, k).unwrap();
                        object.set(scope, key.into(), v);
                    }
                    rv.set(object.into());
                }
                Err(e) => {
                    let message = v8::String::new(scope, &format!("fetch failed: {e}")).unwrap();
                    let exception = v8::Exception::type_error(scope, message);
                    scope.throw_exception(exception);
                }
            }
        }
        // importScripts: compile and run in the worker's global scope.
        "runScript" => {
            let source = arg_string(scope, &args, 0);
            let name = arg_string(scope, &args, 1);
            v8::tc_scope!(let tc, scope);
            let ran = (|| {
                let code = v8::String::new(tc, &source)?;
                let resource = string(tc, &name);
                let origin = v8::ScriptOrigin::new(tc, resource, 0, 0, false, 0, None, false, false, false, None);
                let script = v8::Script::compile(tc, code, Some(&origin))?;
                script.run(tc)
            })();
            if ran.is_none() {
                let error = exception_text(tc, &name);
                let _ = state.borrow().tx.send(FromWorker::Error(error));
                rv.set_bool(false);
            } else {
                rv.set_bool(true);
            }
        }
        "now" => {
            let ms = state.borrow().started.elapsed().as_secs_f64() * 1000.;
            rv.set(v8::Number::new(scope, ms).into());
        }
        "url" => {
            let url = state.borrow().url.clone();
            rv.set(string(scope, &url));
        }
        "userAgent" => {
            let ua = state.borrow().user_agent.clone();
            rv.set(string(scope, &ua));
        }
        "close" => state.borrow_mut().closed = true,
        // What the page's navigator says (a worker's says the same, as in
        // Chrome): [processors, memory], Chrome's brands, its version.
        "machine" => {
            let (threads, memory) = crate::machine();
            let items: Vec<v8::Local<v8::Value>> = vec![v8::Number::new(scope, threads as f64).into(), v8::Number::new(scope, memory).into()];
            rv.set(v8::Array::new_with_elements(scope, &items).into());
        }
        "brands" => {
            let items: Vec<v8::Local<v8::Value>> = crate::BRANDS
                .iter()
                .map(|(b, major, full)| {
                    let parts: Vec<v8::Local<v8::Value>> = [*b, *major, *full].iter().map(|t| string(scope, t)).collect();
                    v8::Array::new_with_elements(scope, &parts).into()
                })
                .collect();
            rv.set(v8::Array::new_with_elements(scope, &items).into());
        }
        "chromeVersion" => rv.set(string(scope, crate::CHROME_FULL)),
        "isProxy" => rv.set_bool(args.get(0).is_proxy()),
        "errorFrames" => {
            let text = crate::error_frames(scope, args.get(0));
            rv.set(string(scope, &text));
        }
        "builtin" => rv.set_bool(v8::Local::<v8::Function>::try_from(args.get(0)).is_ok_and(|f| f.script_id() <= 0)),
        _ => {}
    }
}
