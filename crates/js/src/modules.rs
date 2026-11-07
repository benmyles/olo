//! ES modules (ECMA-262 §16.2; HTML's module scripts): a module's static
//! imports are resolved against its URL, fetched and compiled before it
//! is instantiated and run, `import.meta.url` is its URL, and a dynamic
//! `import()` loads the same way and answers a promise. Each module is
//! loaded once per realm, by URL.
use crate::{State, exception_text, string};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

thread_local! {
    /// The realm on this thread, for V8's module callbacks (plain
    /// functions without state of their own).
    static CURRENT: RefCell<Option<Rc<RefCell<State>>>> = const { RefCell::new(None) };
}
/// A realm's module map.
#[derive(Default)]
pub(crate) struct Modules {
    by_url: HashMap<String, v8::Global<v8::Module>>,
    /// Each module's URL, by its identity (the referrer of a resolution).
    urls: HashMap<i32, String>,
}
/// A scope that catches: what the loader works in. Handles made in it
/// live for `'obj` (the try-catch's handle scope).
pub(crate) type Catching<'a, 'scope, 'obj, 'iso> = v8::PinnedRef<'a, v8::TryCatch<'scope, 'obj, v8::HandleScope<'iso>>>;

pub(crate) fn enter(state: &Rc<RefCell<State>>) {
    CURRENT.with(|c| *c.borrow_mut() = Some(state.clone()));
}
pub(crate) fn leave() {
    CURRENT.with(|c| *c.borrow_mut() = None);
}
fn current() -> Option<Rc<RefCell<State>>> {
    CURRENT.with(|c| c.borrow().clone())
}
pub(crate) fn install(isolate: &mut v8::Isolate) {
    isolate.set_host_initialize_import_meta_object_callback(import_meta);
    isolate.set_host_import_module_dynamically_callback(dynamic_import);
}
/// A specifier resolved against its referrer's URL: an absolute URL, or
/// one starting with `/`, `./` or `../` (HTML "resolve a module
/// specifier"); a bare name has no import map to go by yet.
pub(crate) fn resolve(base: &str, specifier: &str) -> Result<String, String> {
    if let Ok(u) = url::Url::parse(specifier) {
        return Ok(u.to_string());
    }
    if specifier.starts_with('/') || specifier.starts_with("./") || specifier.starts_with("../") {
        let base = url::Url::parse(base).map_err(|_| format!("no base URL to resolve '{specifier}' against"))?;
        return base.join(specifier).map(|u| u.to_string()).map_err(|e| format!("'{specifier}': {e}"));
    }
    Err(format!("bare module specifier '{specifier}' (import maps are not supported yet)"))
}
/// A module's source: fetched now, on this thread.
fn fetch(state: &Rc<RefCell<State>>, url: &str) -> Result<String, String> {
    let fetched = {
        let mut s = state.borrow_mut();
        match s.fetcher.as_mut() {
            Some(f) => f(url, "GET", "")?,
            None => return Err("no network for modules".into()),
        }
    };
    if fetched.status >= 400 {
        return Err(format!("{url} answered {}", fetched.status));
    }
    Ok(String::from_utf8_lossy(&fetched.body).into_owned())
}
/// Several modules' sources, fetched together on threads of their own
/// when the realm has a fetcher that allows it (a graph's imports load
/// side by side, not one after another), else in turn.
fn fetch_all(state: &Rc<RefCell<State>>, urls: &[String]) -> Vec<Result<String, String>> {
    let shared = state.borrow().worker_fetcher.clone();
    let Some(shared) = shared.filter(|_| urls.len() > 1) else {
        return urls.iter().map(|u| fetch(state, u)).collect();
    };
    std::thread::scope(|scope| {
        let handles: Vec<_> = urls
            .iter()
            .map(|url| {
                let shared = shared.clone();
                scope.spawn(move || -> Result<String, String> {
                    let fetched = shared(url, "GET", "")?;
                    if fetched.status >= 400 {
                        return Err(format!("{url} answered {}", fetched.status));
                    }
                    Ok(String::from_utf8_lossy(&fetched.body).into_owned())
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("a module fetch panicked".into()))).collect()
    })
}
/// `text` compiled as the module at `url` and put in the map; its
/// imports' URLs, resolved.
fn compile<'obj>(tc: &mut Catching<'_, '_, 'obj, '_>, state: &Rc<RefCell<State>>, url: &str, text: &str) -> Result<(v8::Local<'obj, v8::Module>, Vec<String>), String> {
    let code = v8::String::new(tc, text).ok_or("module source too long")?;
    let resource = string(tc, url);
    let origin = v8::ScriptOrigin::new(tc, resource, 0, 0, false, 0, None, false, false, true, None);
    let mut src = v8::script_compiler::Source::new(code, Some(&origin));
    let Some(module) = v8::script_compiler::compile_module(tc, &mut src) else {
        let error = exception_text(tc, url);
        tc.reset();
        return Err(error);
    };
    {
        let mut s = state.borrow_mut();
        s.modules.by_url.insert(url.to_owned(), v8::Global::new(tc, module));
        s.modules.urls.insert(module.get_identity_hash().get(), url.to_owned());
    }
    let requests = module.get_module_requests();
    let mut imports = Vec::new();
    for i in 0..requests.length() {
        let Some(request) = requests.get(tc, i).and_then(|d| v8::Local::<v8::ModuleRequest>::try_from(d).ok()) else { continue };
        let specifier = request.get_specifier().to_rust_string_lossy(tc);
        imports.push(resolve(url, &specifier).map_err(|e| format!("{url}: {e}"))?);
    }
    Ok((module, imports))
}
/// The module at `url` (its source given, or fetched now), compiled, and
/// its whole import graph with it, level by level (each level's sources
/// fetched together): all in the map afterwards.
fn load<'obj>(tc: &mut Catching<'_, '_, 'obj, '_>, state: &Rc<RefCell<State>>, url: &str, source: Option<&str>) -> Result<v8::Local<'obj, v8::Module>, String> {
    if let Some(global) = state.borrow().modules.by_url.get(url) {
        return Ok(v8::Local::new(tc, global));
    }
    let text = match source {
        Some(s) => s.to_owned(),
        None => fetch(state, url)?,
    };
    let started = std::time::Instant::now();
    let (module, imports) = compile(tc, state, url, &text)?;
    let mut frontier = imports;
    let mut loaded = 1usize;
    let mut levels = 0usize;
    let mut fetching = 0f64;
    while !frontier.is_empty() {
        frontier.sort();
        frontier.dedup();
        let missing: Vec<String> = {
            let s = state.borrow();
            frontier.into_iter().filter(|u| !s.modules.by_url.contains_key(u)).collect()
        };
        if missing.is_empty() {
            break;
        }
        loaded += missing.len();
        if loaded > 4096 {
            return Err(format!("too many modules under {url}"));
        }
        let before = std::time::Instant::now();
        let sources = fetch_all(state, &missing);
        fetching += before.elapsed().as_secs_f64();
        levels += 1;
        frontier = Vec::new();
        for (u, text) in missing.iter().zip(sources) {
            if state.borrow().modules.by_url.contains_key(u) {
                continue;
            }
            let (_, imports) = compile(tc, state, u, &text?)?;
            frontier.extend(imports);
        }
    }
    if std::env::var_os("OLO_BROWSER_TIMING").is_some() && loaded > 1 {
        eprintln!("  modules: {loaded} under {url} in {} levels, {:.0} ms ({:.0} ms fetching)", levels, started.elapsed().as_secs_f64() * 1000., fetching * 1000.);
    }
    Ok(module)
}
/// The module at `url` loaded with its graph, instantiated and run.
pub(crate) fn run<'obj>(tc: &mut Catching<'_, '_, 'obj, '_>, state: &Rc<RefCell<State>>, url: &str, source: Option<&str>) -> Result<v8::Local<'obj, v8::Module>, String> {
    let module = load(tc, state, url, source)?;
    match module.get_status() {
        v8::ModuleStatus::Evaluated => return Ok(module),
        v8::ModuleStatus::Errored => {
            let e = module.get_exception();
            return Err(format!("{} ({url})", e.to_rust_string_lossy(tc)));
        }
        _ => {}
    }
    if module.get_status() == v8::ModuleStatus::Uninstantiated && module.instantiate_module(tc, resolve_callback).is_none() {
        let error = exception_text(tc, url);
        tc.reset();
        return Err(error);
    }
    let Some(value) = module.evaluate(tc) else {
        let error = exception_text(tc, url);
        tc.reset();
        return Err(error);
    };
    // Top-level awaits settle through the microtasks.
    tc.perform_microtask_checkpoint();
    if module.get_status() == v8::ModuleStatus::Errored {
        let e = module.get_exception();
        return Err(format!("{} ({url})", e.to_rust_string_lossy(tc)));
    }
    if let Ok(promise) = v8::Local::<v8::Promise>::try_from(value)
        && promise.state() == v8::PromiseState::Rejected
    {
        let e = promise.result(tc);
        return Err(format!("{} ({url})", e.to_rust_string_lossy(tc)));
    }
    Ok(module)
}
/// A static import resolved while instantiating: the module was loaded
/// into the map beforehand.
#[allow(clippy::unnecessary_wraps)]
fn resolve_callback<'s>(
    context: v8::Local<'s, v8::Context>,
    specifier: v8::Local<'s, v8::String>,
    _attributes: v8::Local<'s, v8::FixedArray>,
    referrer: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Module>> {
    v8::callback_scope!(unsafe scope, context);
    let state = current()?;
    let specifier = specifier.to_rust_string_lossy(scope);
    let base = state.borrow().modules.urls.get(&referrer.get_identity_hash().get()).cloned()?;
    let module = match resolve(&base, &specifier) {
        Ok(url) => state.borrow().modules.by_url.get(&url).cloned(),
        Err(_) => None,
    };
    match module {
        Some(global) => Some(v8::Local::new(scope, &global)),
        None => {
            let message = string(scope, &format!("cannot resolve module '{specifier}' from {base}"));
            let error = v8::Exception::error(scope, message.try_into().ok()?);
            scope.throw_exception(error);
            None
        }
    }
}
/// `import.meta`: its `url`.
extern "C" fn import_meta(context: v8::Local<v8::Context>, module: v8::Local<v8::Module>, meta: v8::Local<v8::Object>) {
    v8::callback_scope!(unsafe scope, context);
    v8::scope!(let scope, scope);
    let Some(state) = current() else { return };
    let Some(url) = state.borrow().modules.urls.get(&module.get_identity_hash().get()).cloned() else { return };
    // An inline module's URL is its document's (its map key carries a
    // fragment to tell it apart).
    let url = match url.find("#inline-module-") {
        Some(at) => url[..at].to_owned(),
        None => url,
    };
    let key = v8::String::new(scope, "url").unwrap();
    let value = string(scope, &url);
    meta.create_data_property(scope, key.into(), value);
}
/// `import(specifier)`: loaded and run now (its fetches synchronous, on
/// the scripts' thread), the promise answered with the namespace.
fn dynamic_import<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _host_defined_options: v8::Local<'s, v8::Data>,
    resource_name: v8::Local<'s, v8::Value>,
    specifier: v8::Local<'s, v8::String>,
    _attributes: v8::Local<'s, v8::FixedArray>,
) -> Option<v8::Local<'s, v8::Promise>> {
    let resolver = v8::PromiseResolver::new(scope)?;
    let promise = resolver.get_promise(scope);
    let specifier = specifier.to_rust_string_lossy(scope);
    let referrer = resource_name.to_rust_string_lossy(scope);
    let outcome = match current() {
        None => Err("no realm for import()".to_owned()),
        Some(state) => {
            // A classic script's name is not a URL: the page's is the base.
            let base = if url::Url::parse(&referrer).is_ok() { referrer.clone() } else { state.borrow().config.url.clone() };
            resolve(&base, &specifier).and_then(|url| {
                v8::tc_scope!(let tc, scope);
                run(tc, &state, &url, None).map(|m| v8::Global::new(tc, m))
            })
        }
    };
    match outcome {
        Ok(module) => {
            let module = v8::Local::new(scope, &module);
            let namespace = module.get_module_namespace();
            resolver.resolve(scope, namespace);
        }
        Err(e) => {
            let message = string(scope, &e);
            let error = v8::Exception::error(scope, message.try_into().ok()?);
            resolver.reject(scope, error);
        }
    }
    Some(promise)
}
