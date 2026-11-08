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
    /// What bare names and URL prefixes map to.
    pub(crate) map: ImportMap,
}
/// A scope that catches: what the loader works in. Handles made in it
/// live for `'obj` (the try-catch's handle scope).
pub(crate) type Catching<'a, 'scope, 'obj, 'iso> = v8::PinnedRef<'a, v8::TryCatch<'scope, 'obj, v8::HandleScope<'iso>>>;

impl Modules {
    pub(crate) fn with_map(map: ImportMap) -> Self {
        Self { map, ..Default::default() }
    }
}
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
/// The page's import maps (HTML "import maps"): bare names such as
/// `react` and URL prefixes mapped to URLs, at the top level and within
/// scopes (the modules under a URL prefix), read from every
/// `<script type="importmap">` in the document before any module loads.
/// A later map adds only what the earlier ones did not map.
#[derive(Default, Debug)]
pub(crate) struct ImportMap {
    /// Specifier keys and what they map to (None: blocked), longest first.
    imports: Vec<(String, Option<url::Url>)>,
    /// Scope prefixes (serialised URLs) and their maps, most specific first.
    scopes: Vec<(String, Vec<(String, Option<url::Url>)>)>,
}
impl ImportMap {
    /// The maps in `doc`, their URLs resolved against `base` (the page's).
    pub(crate) fn of_document(doc: &olo_web::dom::Document, base: &str) -> Self {
        let mut map = Self::default();
        let base = url::Url::parse(base).ok();
        let mut stack = vec![doc.root()];
        while let Some(n) = stack.pop() {
            if let Ok(children) = doc.children(n) {
                let children: Vec<_> = children.collect();
                stack.extend(children.into_iter().rev());
            }
            if doc.name(n).ok().flatten() != Some("script") || doc.namespace(n).ok().flatten() != Some(olo_web::dom::Namespace::Html) {
                continue;
            }
            if !doc.attribute(n, "type").ok().flatten().is_some_and(|t| t.trim().eq_ignore_ascii_case("importmap")) {
                continue;
            }
            let mut text = String::new();
            let _ = crate::html::text_content(doc, n, &mut text, 4 << 20);
            map.merge(&text, base.as_ref());
        }
        map
    }
    /// One map's JSON added; malformed JSON adds nothing (as in a browser,
    /// which reports it and goes on).
    pub(crate) fn merge(&mut self, json: &str, base: Option<&url::Url>) {
        let Ok(serde_json::Value::Object(top)) = serde_json::from_str::<serde_json::Value>(json) else {
            eprintln!("Browser scripts: an import map is not a JSON object");
            return;
        };
        if let Some(serde_json::Value::Object(imports)) = top.get("imports") {
            for (key, value) in normalise(imports, base) {
                if !self.imports.iter().any(|(k, _)| *k == key) {
                    self.imports.push((key, value));
                }
            }
            sort(&mut self.imports);
        }
        if let Some(serde_json::Value::Object(scopes)) = top.get("scopes") {
            for (prefix, map) in scopes {
                let serde_json::Value::Object(map) = map else { continue };
                let Some(prefix) = base.and_then(|b| b.join(prefix).ok()).or_else(|| url::Url::parse(prefix).ok()) else { continue };
                let prefix = prefix.to_string();
                let entries = normalise(map, base);
                match self.scopes.iter_mut().find(|(p, _)| *p == prefix) {
                    Some((_, existing)) => {
                        for (key, value) in entries {
                            if !existing.iter().any(|(k, _)| *k == key) {
                                existing.push((key, value));
                            }
                        }
                        sort(existing);
                    }
                    None => {
                        let mut entries = entries;
                        sort(&mut entries);
                        self.scopes.push((prefix, entries));
                    }
                }
            }
            self.scopes.sort_by(|a, b| b.0.cmp(&a.0));
        }
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.imports.is_empty() && self.scopes.is_empty()
    }
}
/// A specifier map's entries: keys normalised (a URL-like key as its URL),
/// values parsed as URL-like specifiers against the map's base (None: a
/// value that is not, or a prefix key mapped to a URL not ending in `/`).
fn normalise(map: &serde_json::Map<String, serde_json::Value>, base: Option<&url::Url>) -> Vec<(String, Option<url::Url>)> {
    let mut out = Vec::new();
    for (key, value) in map {
        if key.is_empty() {
            continue;
        }
        let key = url_like(key, base).map_or_else(|| key.clone(), |u| u.to_string());
        let value = value.as_str().and_then(|v| url_like(v, base)).filter(|v| !key.ends_with('/') || v.as_str().ends_with('/'));
        out.push((key, value));
    }
    out
}
/// Keys in descending code-unit order: a longer prefix before the shorter
/// one it starts with.
fn sort(entries: &mut [(String, Option<url::Url>)]) {
    entries.sort_by(|a, b| b.0.cmp(&a.0));
}
/// A URL-like specifier (HTML "resolve a URL-like module specifier"): an
/// absolute URL, or one starting with `/`, `./` or `../` against `base`.
fn url_like(specifier: &str, base: Option<&url::Url>) -> Option<url::Url> {
    if specifier.starts_with('/') || specifier.starts_with("./") || specifier.starts_with("../") {
        return base?.join(specifier).ok();
    }
    url::Url::parse(specifier).ok()
}
/// A specifier resolved against its referrer's URL through the import
/// map (HTML "resolve a module specifier"): the most specific scope
/// containing the referrer first, then the top-level map, then the
/// specifier as a URL-like one; a bare name nothing maps is an error.
pub(crate) fn resolve(map: &ImportMap, base: &str, specifier: &str) -> Result<String, String> {
    // An inline module's name carries a fragment to tell it apart; its base
    // is its document's URL.
    let base = base.find("#inline-module-").map_or(base, |at| &base[..at]);
    let base_url = url::Url::parse(base).ok();
    let as_url = url_like(specifier, base_url.as_ref());
    if map.is_empty() {
        return as_url.map(|u| u.to_string()).ok_or_else(|| match &base_url {
            None if specifier.starts_with('/') || specifier.starts_with("./") || specifier.starts_with("../") => format!("no base URL to resolve '{specifier}' against"),
            _ => format!("bare module specifier '{specifier}' (no import map maps it)"),
        });
    }
    let normalised = as_url.as_ref().map_or_else(|| specifier.to_owned(), |u| u.to_string());
    for (prefix, imports) in &map.scopes {
        if prefix == base || prefix.ends_with('/') && base.starts_with(prefix.as_str()) {
            if let Some(url) = imports_match(&normalised, as_url.as_ref(), imports)? {
                return Ok(url);
            }
        }
    }
    if let Some(url) = imports_match(&normalised, as_url.as_ref(), &map.imports)? {
        return Ok(url);
    }
    as_url.map(|u| u.to_string()).ok_or_else(|| format!("bare module specifier '{specifier}' (no import map maps it)"))
}
/// HTML "resolve an imports match": an exact key, or the longest key
/// ending in `/` that the specifier starts with (the rest joined to its
/// URL, which must stay under it).
fn imports_match(normalised: &str, as_url: Option<&url::Url>, imports: &[(String, Option<url::Url>)]) -> Result<Option<String>, String> {
    for (key, value) in imports {
        if key == normalised {
            return value.as_ref().map(|v| Some(v.to_string())).ok_or_else(|| format!("'{normalised}' is blocked by the import map"));
        }
        if key.ends_with('/') && normalised.starts_with(key.as_str()) && as_url.is_none_or(|u| u.is_special()) {
            let value = value.as_ref().ok_or_else(|| format!("'{normalised}' is blocked by the import map"))?;
            let url = value.join(&normalised[key.len()..]).map_err(|_| format!("'{normalised}' maps to no URL"))?;
            if !url.as_str().starts_with(value.as_str()) {
                return Err(format!("'{normalised}' backtracks above its mapping"));
            }
            return Ok(Some(url.to_string()));
        }
    }
    Ok(None)
}
/// A module's source: fetched now, on this thread.
fn fetch(state: &Rc<RefCell<State>>, url: &str) -> Result<String, String> {
    let fetched = {
        let mut s = state.borrow_mut();
        match s.fetcher.as_mut() {
            Some(f) => f(url, "GET", &[], "")?,
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
                    let fetched = shared(url, "GET", &[], "")?;
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
        let resolved = resolve(&state.borrow().modules.map, url, &specifier);
        imports.push(resolved.map_err(|e| format!("{url}: {e}"))?);
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
            return Err(thrown(tc, e, url));
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
        return Err(thrown(tc, e, url));
    }
    if let Ok(promise) = v8::Local::<v8::Promise>::try_from(value)
        && promise.state() == v8::PromiseState::Rejected
    {
        let e = promise.result(tc);
        return Err(thrown(tc, e, url));
    }
    Ok(module)
}
/// What a module threw, with its stack when it has one (where it failed,
/// not only the module it failed in).
fn thrown(tc: &mut Catching<'_, '_, '_, '_>, e: v8::Local<'_, v8::Value>, url: &str) -> String {
    let message = e.to_rust_string_lossy(tc);
    let stack = e.to_object(tc).and_then(|o| {
        let key = v8::String::new(tc, "stack")?;
        o.get(tc, key.into())
    });
    match stack.filter(|s| s.is_string()).map(|s| s.to_rust_string_lossy(tc)) {
        Some(stack) if stack.len() > message.len() => {
            let stack: String = stack.lines().take(12).collect::<Vec<_>>().join("\n  ");
            format!("{stack}\n  ({url})")
        }
        _ => format!("{message} ({url})"),
    }
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
    let resolved = resolve(&state.borrow().modules.map, &base, &specifier);
    let module = match resolved {
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
            let resolved = resolve(&state.borrow().modules.map, &base, &specifier);
            resolved.and_then(|url| {
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
#[cfg(test)]
mod tests {
    use super::*;
    fn map(json: &str) -> ImportMap {
        let mut m = ImportMap::default();
        m.merge(json, url::Url::parse("https://site.test/app/page.html").ok().as_ref());
        m
    }
    #[test]
    fn bare_names_prefixes_and_scopes_resolve_as_html_says() {
        let m = map(r#"{"imports":{"react":"https://cdn.test/react.js","lodash/":"/vendor/lodash/","./old.js":"./new.js","blocked":null},
                "scopes":{"/app/legacy/":{"react":"/react-17.js"},"/app/":{"lodash/fp":"/fp.js"}}}"#);
        let page = "https://site.test/app/page.html";
        assert_eq!(resolve(&m, page, "react").unwrap(), "https://cdn.test/react.js");
        assert_eq!(resolve(&m, page, "lodash/map.js").unwrap(), "https://site.test/vendor/lodash/map.js");
        // A URL-like specifier maps by its URL.
        assert_eq!(resolve(&m, page, "./old.js").unwrap(), "https://site.test/app/new.js");
        assert_eq!(resolve(&m, "https://site.test/app/x/y.js", "../old.js").unwrap(), "https://site.test/app/new.js");
        // Scopes: the most specific containing the referrer wins, then the top.
        assert_eq!(resolve(&m, "https://site.test/app/legacy/a.js", "react").unwrap(), "https://site.test/react-17.js");
        assert_eq!(resolve(&m, "https://site.test/app/a.js", "lodash/fp").unwrap(), "https://site.test/fp.js");
        assert_eq!(resolve(&m, "https://site.test/other/a.js", "lodash/fp").unwrap(), "https://site.test/vendor/lodash/fp");
        // An inline module resolves against its document.
        assert_eq!(resolve(&m, "https://site.test/app/page.html#inline-module-3", "./a.js").unwrap(), "https://site.test/app/a.js");
        assert!(resolve(&m, page, "blocked").is_err());
        assert!(resolve(&m, page, "vue").is_err());
        // A prefix match may not climb out of what it maps to.
        assert!(resolve(&m, page, "lodash/../../etc").is_err());
        // Without a map, URL-like specifiers still resolve.
        assert_eq!(resolve(&ImportMap::default(), page, "/x.js").unwrap(), "https://site.test/x.js");
        assert!(resolve(&ImportMap::default(), page, "react").is_err());
    }
    #[test]
    fn a_later_map_adds_only_what_earlier_ones_left_unmapped() {
        let mut m = map(r#"{"imports":{"a":"/one.js"}}"#);
        m.merge(r#"{"imports":{"a":"/two.js","b":"/b.js"}}"#, url::Url::parse("https://site.test/").ok().as_ref());
        m.merge("not json", None);
        assert_eq!(resolve(&m, "https://site.test/", "a").unwrap(), "https://site.test/one.js");
        assert_eq!(resolve(&m, "https://site.test/", "b").unwrap(), "https://site.test/b.js");
    }
}
