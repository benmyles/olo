//! JavaScript for Olo's browser: V8 runs a page's scripts against the
//! engine's document. A realm holds one isolate, one context and the
//! document; a few native functions (`__olo.*`) read and change the
//! document, run selectors, serialise and parse HTML, log and fetch; the
//! prelude (`prelude.js`) builds the DOM and Web API surface on them:
//! Node, Element, Document, events, timers, storage, location, fetch.
//!
//! Scripts run after the document is parsed (in order, as if deferred);
//! `document.write` is not supported. Time is virtual: timers run in due
//! order up to a budget, without waiting. The document comes back changed
//! and the page styles, lays out and paints from it.
#![forbid(unsafe_op_in_unsafe_fn)]
pub mod html;
use olo_web::{
    css::{
        self,
        selectors::{CompileOptions, DomTree, MatchLimits, MatchOptions, Selectors},
    },
    dom::{Document, Namespace, NodeId, NodeType},
    html::QuirksMode,
};
use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::c_void,
    rc::Rc,
    sync::{Arc, OnceLock},
    time::Instant,
};
const PRELUDE: &str = include_str!("prelude.js");
/// What the page is, for `location`, `navigator` and the viewport.
#[derive(Clone, Debug)]
pub struct Config {
    pub url: String,
    pub viewport: (f64, f64),
    pub scale: f64,
    pub user_agent: String,
    pub language: String,
    pub quirks: bool,
    /// Virtual milliseconds of timers to run after the scripts and load.
    pub budget_ms: f64,
    /// Wall-clock seconds the whole run may take.
    pub wall_seconds: f64,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            url: "about:blank".into(),
            viewport: (1000., 700.),
            scale: 1.,
            user_agent: "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36 Olo/0.1".into(),
            language: "en-US".into(),
            quirks: false,
            budget_ms: 3000.,
            wall_seconds: 8.,
        }
    }
}
/// A fetched resource for `fetch` and `XMLHttpRequest`.
pub struct Fetched {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub url: String,
}
/// Fetches a URL (method, body) for scripts; the browser supplies one.
pub type Fetcher = Box<dyn FnMut(&str, &str, &str) -> Result<Fetched, String>>;
/// Lays out the document as it is now, for what scripts measure; the
/// browser supplies one (olo_web::page::Measurer behind a host).
pub type Measure = Box<dyn FnMut(&Document) -> Option<olo_web::page::Measured>>;
/// How often scripts may have the page laid out, and for how long in all:
/// past these, measurements come from the last layout.
const LAYOUTS: usize = 400;
const LAYOUT_SECONDS: f64 = 4.;
/// A script to run: its source and a name for errors.
pub struct Script {
    pub source: Arc<str>,
    pub name: String,
}
/// What happened.
#[derive(Debug, Default, Clone)]
pub struct Report {
    pub scripts: usize,
    pub errors: Vec<String>,
    pub console: Vec<String>,
    pub timers: usize,
    pub listeners: usize,
    /// Layouts run for measurements, and their time.
    pub layouts: usize,
    pub layout_ms: f64,
    pub revision_before: u64,
    pub revision_after: u64,
    pub wall_ms: f64,
}
struct Templates {
    event_target: v8::Global<v8::FunctionTemplate>,
    node: v8::Global<v8::FunctionTemplate>,
    element: v8::Global<v8::FunctionTemplate>,
    html_element: v8::Global<v8::FunctionTemplate>,
    svg_element: v8::Global<v8::FunctionTemplate>,
    text: v8::Global<v8::FunctionTemplate>,
    comment: v8::Global<v8::FunctionTemplate>,
    document: v8::Global<v8::FunctionTemplate>,
    fragment: v8::Global<v8::FunctionTemplate>,
}
struct State {
    document: Document,
    config: Config,
    quirks: QuirksMode,
    wrappers: HashMap<NodeId, v8::Global<v8::Object>>,
    /// The boxed ids wrappers point at; freed with the realm.
    ids: Vec<*mut NodeId>,
    templates: Option<Templates>,
    fetcher: Option<Fetcher>,
    measure: Option<Measure>,
    /// The last layout, good while the document's revision is its own.
    measured: Option<olo_web::page::Measured>,
    layout_seconds: f64,
    report: Report,
    started: Instant,
}
type Shared = Rc<RefCell<State>>;
fn runtime() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
    });
}
/// One page's scripting: its isolate, context and document.
pub struct Realm {
    isolate: v8::OwnedIsolate,
    context: v8::Global<v8::Context>,
    state: Shared,
}
impl Realm {
    pub fn new(document: Document, config: Config, fetcher: Option<Fetcher>, measure: Option<Measure>) -> Result<Self, String> {
        runtime();
        let quirks = if config.quirks { QuirksMode::Quirks } else { QuirksMode::NoQuirks };
        let revision = document.revision();
        let state: Shared = Rc::new(RefCell::new(State {
            document,
            config,
            quirks,
            wrappers: HashMap::new(),
            ids: Vec::new(),
            templates: None,
            fetcher,
            measure,
            measured: None,
            layout_seconds: 0.,
            report: Report { revision_before: revision, ..Default::default() },
            started: Instant::now(),
        }));
        let mut isolate = v8::Isolate::new(v8::CreateParams::default());
        isolate.set_slot(state.clone());
        let context = {
            v8::scope!(let scope, &mut isolate);
            let global = v8::ObjectTemplate::new(scope);
            // Named access on the window: an unknown global name is the
            // element with that id (or name, for a few elements), as in
            // browsers. Non-masking: only names nothing else defines.
            global.set_named_property_handler(
                v8::NamedPropertyHandlerConfiguration::new().getter(named_global).flags(v8::PropertyHandlerFlags::NON_MASKING | v8::PropertyHandlerFlags::ONLY_INTERCEPT_STRINGS),
            );
            let context = v8::Context::new(scope, v8::ContextOptions { global_template: Some(global), ..Default::default() });
            let scope = &mut v8::ContextScope::new(scope, context);
            install(scope, &state)?;
            v8::Global::new(scope, context)
        };
        let mut realm = Self { isolate, context, state };
        realm.run(PRELUDE, "prelude.js").map_err(|e| format!("prelude: {e}"))?;
        Ok(realm)
    }
    /// Run one script; an exception is recorded and returned, not fatal.
    pub fn run(&mut self, source: &str, name: &str) -> Result<(), String> {
        let state = self.state.clone();
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
            let error = exception_text(tc, name);
            state.borrow_mut().report.errors.push(error.clone());
            return Err(error);
        }
        Ok(())
    }
    /// Run the page's scripts in order, fire the document's lifecycle events
    /// and the timers due within the budget.
    pub fn run_page(&mut self, scripts: &[Script]) -> Report {
        for script in scripts {
            if self.over_time() {
                self.state.borrow_mut().report.errors.push("scripts stopped: out of time".into());
                break;
            }
            self.state.borrow_mut().report.scripts += 1;
            let _ = self.run(&script.source, &script.name);
        }
        let _ = self.call_prelude("__olo_lifecycle", &["DOMContentLoaded"]);
        self.pump();
        let _ = self.call_prelude("__olo_lifecycle", &["load"]);
        self.pump();
        self.finish_report()
    }
    fn over_time(&self) -> bool {
        let s = self.state.borrow();
        s.started.elapsed().as_secs_f64() > s.config.wall_seconds
    }
    /// Run timers due within the virtual budget, in order, without waiting.
    fn pump(&mut self) {
        let budget = self.state.borrow().config.budget_ms;
        let mut now = 0f64;
        for _ in 0..100_000 {
            if self.over_time() {
                break;
            }
            let Ok(next) = self.call_prelude_number("__olo_tick", now) else { break };
            if next < 0. {
                break;
            }
            now = now.max(next);
            if now > budget {
                break;
            }
        }
    }
    fn call_prelude(&mut self, function: &str, args: &[&str]) -> Result<(), String> {
        let state = self.state.clone();
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc, scope);
        let global = context.global(tc);
        let name = v8::String::new(tc, function).ok_or("string")?;
        let f = global.get(tc, name.into()).ok_or("missing")?;
        let f = v8::Local::<v8::Function>::try_from(f).map_err(|_| "not a function")?;
        let args: Vec<v8::Local<v8::Value>> = args.iter().map(|a| v8::String::new(tc, a).unwrap().into()).collect();
        if f.call(tc, global.into(), &args).is_none() {
            let error = exception_text(tc, function);
            state.borrow_mut().report.errors.push(error.clone());
            return Err(error);
        }
        Ok(())
    }
    fn call_prelude_number(&mut self, function: &str, arg: f64) -> Result<f64, String> {
        let state = self.state.clone();
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc, scope);
        let global = context.global(tc);
        let name = v8::String::new(tc, function).ok_or("string")?;
        let f = global.get(tc, name.into()).ok_or("missing")?;
        let f = v8::Local::<v8::Function>::try_from(f).map_err(|_| "not a function")?;
        let arg: v8::Local<v8::Value> = v8::Number::new(tc, arg).into();
        match f.call(tc, global.into(), &[arg]) {
            Some(v) => Ok(v.number_value(tc).unwrap_or(-1.)),
            None => {
                let error = exception_text(tc, function);
                state.borrow_mut().report.errors.push(error.clone());
                Err(error)
            }
        }
    }
    fn finish_report(&self) -> Report {
        let mut s = self.state.borrow_mut();
        s.report.revision_after = s.document.revision();
        s.report.wall_ms = s.started.elapsed().as_secs_f64() * 1000.;
        s.report.clone()
    }
    /// The document, as the scripts left it, and the report.
    pub fn finish(mut self) -> (Document, Report) {
        let report = self.finish_report();
        // Drop every handle before the isolate.
        {
            let mut s = self.state.borrow_mut();
            s.wrappers.clear();
            s.templates = None;
            s.measure = None;
            s.measured = None;
        }
        let state = std::mem::replace(&mut self.state, Rc::new(RefCell::new(State::empty())));
        drop(self);
        let state = Rc::try_unwrap(state).ok().expect("the realm's state is only its own").into_inner();
        for id in state.ids {
            // SAFETY: each pointer came from Box::into_raw in `wrap` and is
            // freed exactly once, here, after every V8 object that held it.
            drop(unsafe { Box::from_raw(id) });
        }
        (state.document, report)
    }
}
impl State {
    fn empty() -> Self {
        Self {
            document: Document::new(Arc::from(""), Default::default()).expect("an empty document"),
            config: Config::default(),
            quirks: QuirksMode::NoQuirks,
            wrappers: HashMap::new(),
            ids: Vec::new(),
            templates: None,
            fetcher: None,
            measure: None,
            measured: None,
            layout_seconds: 0.,
            report: Report::default(),
            started: Instant::now(),
        }
    }
    /// The layout to measure against: the last one while the document is
    /// unchanged, else a new one within the layout budget.
    fn measured(&mut self) -> Option<&olo_web::page::Measured> {
        let revision = self.document.revision();
        let stale = self.measured.as_ref().is_none_or(|m| m.revision() != revision);
        if stale && self.measure.is_some() && self.report.layouts < LAYOUTS && self.layout_seconds < LAYOUT_SECONDS {
            let started = Instant::now();
            let measure = self.measure.as_mut()?;
            if let Some(m) = measure(&self.document) {
                self.measured = Some(m);
            }
            let took = started.elapsed().as_secs_f64();
            self.layout_seconds += took;
            self.report.layouts += 1;
            self.report.layout_ms += took * 1000.;
        }
        self.measured.as_ref()
    }
}
fn exception_text(tc: &mut v8::PinnedRef<'_, v8::TryCatch<'_, '_, v8::HandleScope<'_>>>, name: &str) -> String {
    let mut text = String::new();
    if let Some(e) = tc.exception() {
        text.push_str(&e.to_rust_string_lossy(tc));
    }
    if let Some(m) = tc.message() {
        let line = m.get_line_number(tc).unwrap_or(0);
        text.push_str(&format!(" ({name}:{line})"));
    } else {
        text.push_str(&format!(" ({name})"));
    }
    if let Some(trace) = tc.stack_trace() {
        let trace = trace.to_rust_string_lossy(tc);
        if !trace.is_empty() && trace.len() < 2000 {
            text.push_str("\n  ");
            text.push_str(&trace.replace('\n', "\n  "));
        }
    }
    text
}
/// The DOM class of an element by its tag, for the prototype of its
/// wrapper. Sorted by tag for the lookup; tags not listed are HTMLElement
/// (or SVGElement) itself. Unknown HTML tags are HTMLUnknownElement.
const HTML_CLASSES: &[(&str, &str)] = &[
    ("a", "HTMLAnchorElement"),
    ("abbr", "HTMLElement"),
    ("address", "HTMLElement"),
    ("area", "HTMLAreaElement"),
    ("article", "HTMLElement"),
    ("aside", "HTMLElement"),
    ("audio", "HTMLAudioElement"),
    ("b", "HTMLElement"),
    ("base", "HTMLBaseElement"),
    ("bdi", "HTMLElement"),
    ("bdo", "HTMLElement"),
    ("blockquote", "HTMLQuoteElement"),
    ("body", "HTMLBodyElement"),
    ("br", "HTMLBRElement"),
    ("button", "HTMLButtonElement"),
    ("canvas", "HTMLCanvasElement"),
    ("caption", "HTMLTableCaptionElement"),
    ("cite", "HTMLElement"),
    ("code", "HTMLElement"),
    ("col", "HTMLTableColElement"),
    ("colgroup", "HTMLTableColElement"),
    ("data", "HTMLDataElement"),
    ("datalist", "HTMLDataListElement"),
    ("dd", "HTMLElement"),
    ("del", "HTMLModElement"),
    ("details", "HTMLDetailsElement"),
    ("dfn", "HTMLElement"),
    ("dialog", "HTMLDialogElement"),
    ("div", "HTMLDivElement"),
    ("dl", "HTMLDListElement"),
    ("dt", "HTMLElement"),
    ("em", "HTMLElement"),
    ("embed", "HTMLEmbedElement"),
    ("fieldset", "HTMLFieldSetElement"),
    ("figcaption", "HTMLElement"),
    ("figure", "HTMLElement"),
    ("font", "HTMLFontElement"),
    ("footer", "HTMLElement"),
    ("form", "HTMLFormElement"),
    ("frame", "HTMLFrameElement"),
    ("frameset", "HTMLFrameSetElement"),
    ("h1", "HTMLHeadingElement"),
    ("h2", "HTMLHeadingElement"),
    ("h3", "HTMLHeadingElement"),
    ("h4", "HTMLHeadingElement"),
    ("h5", "HTMLHeadingElement"),
    ("h6", "HTMLHeadingElement"),
    ("head", "HTMLHeadElement"),
    ("header", "HTMLElement"),
    ("hgroup", "HTMLElement"),
    ("hr", "HTMLHRElement"),
    ("html", "HTMLHtmlElement"),
    ("i", "HTMLElement"),
    ("iframe", "HTMLIFrameElement"),
    ("img", "HTMLImageElement"),
    ("input", "HTMLInputElement"),
    ("ins", "HTMLModElement"),
    ("kbd", "HTMLElement"),
    ("label", "HTMLLabelElement"),
    ("legend", "HTMLLegendElement"),
    ("li", "HTMLLIElement"),
    ("link", "HTMLLinkElement"),
    ("main", "HTMLElement"),
    ("map", "HTMLMapElement"),
    ("mark", "HTMLElement"),
    ("marquee", "HTMLMarqueeElement"),
    ("menu", "HTMLMenuElement"),
    ("meta", "HTMLMetaElement"),
    ("meter", "HTMLMeterElement"),
    ("nav", "HTMLElement"),
    ("noscript", "HTMLElement"),
    ("object", "HTMLObjectElement"),
    ("ol", "HTMLOListElement"),
    ("optgroup", "HTMLOptGroupElement"),
    ("option", "HTMLOptionElement"),
    ("output", "HTMLOutputElement"),
    ("p", "HTMLParagraphElement"),
    ("param", "HTMLParamElement"),
    ("picture", "HTMLPictureElement"),
    ("pre", "HTMLPreElement"),
    ("progress", "HTMLProgressElement"),
    ("q", "HTMLQuoteElement"),
    ("rp", "HTMLElement"),
    ("rt", "HTMLElement"),
    ("ruby", "HTMLElement"),
    ("s", "HTMLElement"),
    ("samp", "HTMLElement"),
    ("script", "HTMLScriptElement"),
    ("search", "HTMLElement"),
    ("section", "HTMLElement"),
    ("select", "HTMLSelectElement"),
    ("slot", "HTMLSlotElement"),
    ("small", "HTMLElement"),
    ("source", "HTMLSourceElement"),
    ("span", "HTMLSpanElement"),
    ("strong", "HTMLElement"),
    ("style", "HTMLStyleElement"),
    ("sub", "HTMLElement"),
    ("summary", "HTMLElement"),
    ("sup", "HTMLElement"),
    ("table", "HTMLTableElement"),
    ("tbody", "HTMLTableSectionElement"),
    ("td", "HTMLTableCellElement"),
    ("template", "HTMLTemplateElement"),
    ("textarea", "HTMLTextAreaElement"),
    ("tfoot", "HTMLTableSectionElement"),
    ("th", "HTMLTableCellElement"),
    ("thead", "HTMLTableSectionElement"),
    ("time", "HTMLTimeElement"),
    ("title", "HTMLTitleElement"),
    ("tr", "HTMLTableRowElement"),
    ("track", "HTMLTrackElement"),
    ("u", "HTMLElement"),
    ("ul", "HTMLUListElement"),
    ("var", "HTMLElement"),
    ("video", "HTMLVideoElement"),
    ("wbr", "HTMLElement"),
];
const SVG_CLASSES: &[(&str, &str)] = &[
    ("circle", "SVGCircleElement"),
    ("clipPath", "SVGClipPathElement"),
    ("defs", "SVGDefsElement"),
    ("ellipse", "SVGEllipseElement"),
    ("g", "SVGGElement"),
    ("image", "SVGImageElement"),
    ("line", "SVGLineElement"),
    ("linearGradient", "SVGLinearGradientElement"),
    ("mask", "SVGMaskElement"),
    ("path", "SVGPathElement"),
    ("pattern", "SVGPatternElement"),
    ("polygon", "SVGPolygonElement"),
    ("polyline", "SVGPolylineElement"),
    ("radialGradient", "SVGRadialGradientElement"),
    ("rect", "SVGRectElement"),
    ("stop", "SVGStopElement"),
    ("svg", "SVGSVGElement"),
    ("symbol", "SVGSymbolElement"),
    ("text", "SVGTextElement"),
    ("tspan", "SVGTSpanElement"),
    ("use", "SVGUseElement"),
];
fn tag_class(name: &str, namespace: Option<Namespace>) -> Option<&'static str> {
    match namespace {
        Some(Namespace::Html) => Some(match HTML_CLASSES.binary_search_by_key(&name, |(t, _)| t) {
            Ok(i) => HTML_CLASSES[i].1,
            // Custom elements (with a dash) are HTMLElement; the rest unknown.
            Err(_) if name.contains('-') => "HTMLElement",
            Err(_) => "HTMLUnknownElement",
        }),
        Some(Namespace::Svg) => SVG_CLASSES.binary_search_by_key(&name, |(t, _)| t).ok().map(|i| SVG_CLASSES[i].1),
        _ => None,
    }
}
/// `window.NAME` for an element with that id, or a form/img/iframe/embed/
/// object with that name; the first in document order. Names that scripts
/// probe for features (undefined checks) stay undefined when no element
/// has them.
fn named_global(scope: &mut v8::PinScope, key: v8::Local<v8::Name>, _args: v8::PropertyCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) -> v8::Intercepted {
    let Some(state) = scope.get_slot::<Shared>().cloned() else { return v8::Intercepted::kNo };
    let Ok(key) = v8::Local::<v8::String>::try_from(key) else { return v8::Intercepted::kNo };
    let name = key.to_rust_string_lossy(scope);
    if name.is_empty() || name.len() > 256 {
        return v8::Intercepted::kNo;
    }
    let found = {
        let s = state.borrow();
        find_by_id(&s.document, &name).or_else(|| find_by_name(&s.document, &name))
    };
    match found.and_then(|n| wrap(scope, &state, n)) {
        Some(obj) => {
            rv.set(obj.into());
            v8::Intercepted::kYes
        }
        None => v8::Intercepted::kNo,
    }
}
/// Attribute names are case-insensitive on HTML elements (stored in lower
/// case) and case-sensitive elsewhere (SVG's viewBox).
fn attribute_name(state: &Shared, node: NodeId, name: String) -> String {
    let html = state.borrow().document.namespace(node).ok().flatten() == Some(Namespace::Html);
    if html && name.bytes().any(|b| b.is_ascii_uppercase()) { name.to_ascii_lowercase() } else { name }
}
/// The first form, img, iframe, embed or object with this `name`.
fn find_by_name(doc: &Document, name: &str) -> Option<NodeId> {
    let mut stack = vec![doc.root()];
    let mut seen = 0usize;
    while let Some(n) = stack.pop() {
        seen += 1;
        if seen > 1 << 20 {
            return None;
        }
        if doc.node_type(n) == Ok(NodeType::Element) && matches!(doc.name(n), Ok(Some("form" | "img" | "iframe" | "embed" | "object"))) && doc.attribute(n, "name") == Ok(Some(name)) {
            return Some(n);
        }
        let Ok(children) = doc.children(n) else { continue };
        let children: Vec<NodeId> = children.collect();
        stack.extend(children.into_iter().rev());
    }
    None
}
/// One DOM class: a constructor nobody may call, with an internal field for
/// the node, installed on the global object.
fn class<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    global: v8::Local<'s, v8::Object>,
    name: &str,
    parent: Option<v8::Local<'s, v8::FunctionTemplate>>,
) -> Result<v8::Local<'s, v8::FunctionTemplate>, String> {
    let data: v8::Local<v8::Value> = v8::String::new(scope, name).ok_or("string")?.into();
    let t = v8::FunctionTemplate::builder(illegal_constructor).data(data).build(scope);
    let class_name = v8::String::new(scope, name).ok_or("string")?;
    t.set_class_name(class_name);
    t.instance_template(scope).set_internal_field_count(1);
    if let Some(p) = parent {
        t.inherit(p);
    }
    let f = t.get_function(scope).ok_or("function")?;
    let key = v8::String::new(scope, name).ok_or("string")?;
    global.set(scope, key.into(), f.into());
    Ok(t)
}
/// The native functions and the DOM classes, on the global object.
fn install(scope: &mut v8::PinScope<'_, '_>, state: &Shared) -> Result<(), String> {
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
    // The DOM classes: constructors whose prototypes the prelude fills.
    let event_target = class(scope, global, "EventTarget", None)?;
    let node = class(scope, global, "Node", Some(event_target))?;
    let element = class(scope, global, "Element", Some(node))?;
    let html_element = class(scope, global, "HTMLElement", Some(element))?;
    let svg_element = class(scope, global, "SVGElement", Some(element))?;
    let text = class(scope, global, "Text", Some(node))?;
    let comment = class(scope, global, "Comment", Some(node))?;
    let document = class(scope, global, "Document", Some(node))?;
    let fragment = class(scope, global, "DocumentFragment", Some(node))?;
    state.borrow_mut().templates = Some(Templates {
        event_target: v8::Global::new(scope, event_target),
        node: v8::Global::new(scope, node),
        element: v8::Global::new(scope, element),
        html_element: v8::Global::new(scope, html_element),
        svg_element: v8::Global::new(scope, svg_element),
        text: v8::Global::new(scope, text),
        comment: v8::Global::new(scope, comment),
        document: v8::Global::new(scope, document),
        fragment: v8::Global::new(scope, fragment),
    });
    Ok(())
}
fn illegal_constructor(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let message = v8::String::new(scope, "Illegal constructor").unwrap();
    let exception = v8::Exception::type_error(scope, message);
    scope.throw_exception(exception);
}
const NATIVES: &[&str] = &[
    "nodeType",
    "nodeName",
    "namespace",
    "parent",
    "firstChild",
    "lastChild",
    "nextSibling",
    "previousSibling",
    "childNodes",
    "children",
    "data",
    "setData",
    "textContent",
    "setTextContent",
    "attr",
    "setAttr",
    "removeAttr",
    "attrNames",
    "append",
    "insertBefore",
    "remove",
    "createElement",
    "createText",
    "createComment",
    "createFragment",
    "templateContents",
    "clone",
    "query",
    "matches",
    "innerHTML",
    "setInnerHTML",
    "outerHTML",
    "docRoot",
    "byId",
    "isConnected",
    "log",
    "fetchSync",
    "runScript",
    "rect",
    "paddingRect",
    "scrollSize",
    "computed",
    "docSize",
    "revision",
    "url",
    "viewport",
    "userAgent",
    "language",
    "count",
];
fn throw(scope: &mut v8::PinScope, message: &str) {
    let message = v8::String::new(scope, message).unwrap();
    let exception = v8::Exception::type_error(scope, message);
    scope.throw_exception(exception);
}
/// The node a wrapper object holds.
fn node_of(scope: &mut v8::PinScope, value: v8::Local<v8::Value>) -> Option<NodeId> {
    let object = value.try_cast::<v8::Object>().ok()?;
    if object.internal_field_count() < 1 {
        return None;
    }
    let field = object.get_internal_field(scope, 0)?;
    let external = field.try_cast::<v8::External>().ok()?;
    let pointer = external.value() as *const NodeId;
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the pointer was made in `wrap` from a Box the state keeps until
    // the realm ends, after every wrapper is dropped.
    Some(unsafe { *pointer })
}
/// The wrapper object of a node: one per node, for identity.
fn wrap<'s>(scope: &mut v8::PinScope<'s, '_>, state: &Shared, node: NodeId) -> Option<v8::Local<'s, v8::Object>> {
    if let Some(existing) = state.borrow().wrappers.get(&node) {
        return Some(v8::Local::new(scope, existing));
    }
    let (kind, name, namespace) = {
        let s = state.borrow();
        let doc = &s.document;
        let kind = doc.node_type(node).ok()?;
        (kind, doc.name(node).ok().flatten().map(|n| n.to_owned()), doc.namespace(node).ok().flatten())
    };
    let template = {
        let s = state.borrow();
        let t = s.templates.as_ref()?;
        let chosen = match kind {
            NodeType::Element => match namespace {
                Some(Namespace::Html) => &t.html_element,
                Some(Namespace::Svg) => &t.svg_element,
                _ => &t.element,
            },
            NodeType::Text => &t.text,
            NodeType::Comment | NodeType::ProcessingInstruction => &t.comment,
            NodeType::Document => &t.document,
            NodeType::Fragment => &t.fragment,
            NodeType::Doctype => &t.node,
        };
        v8::Local::new(scope, chosen)
    };
    let object = template.instance_template(scope).new_instance(scope)?;
    // HTML elements get their tag's class (HTMLDivElement, …) when the
    // prelude defined one: its prototype replaces HTMLElement's.
    if kind == NodeType::Element
        && let Some(name) = name.as_deref()
        && let Some(class) = tag_class(name, namespace)
    {
        let global = scope.get_current_context().global(scope);
        let class_key = v8::String::new(scope, class)?;
        if let Some(ctor) = global.get(scope, class_key.into()).and_then(|c| v8::Local::<v8::Object>::try_from(c).ok()) {
            let proto_key = v8::String::new(scope, "prototype")?;
            if let Some(proto) = ctor.get(scope, proto_key.into()) {
                object.set_prototype(scope, proto);
            }
        }
    }
    let boxed = Box::into_raw(Box::new(node));
    let external = v8::External::new(scope, boxed as *mut c_void);
    object.set_internal_field(0, external.into());
    let mut s = state.borrow_mut();
    s.ids.push(boxed);
    s.wrappers.insert(node, v8::Global::new(scope, object));
    Some(object)
}
fn string<'s>(scope: &mut v8::PinScope<'s, '_>, s: &str) -> v8::Local<'s, v8::Value> {
    v8::String::new(scope, s).map_or_else(|| v8::undefined(scope).into(), |v| v.into())
}
fn arg_string(scope: &mut v8::PinScope, args: &v8::FunctionCallbackArguments, i: i32) -> String {
    let v = args.get(i);
    if v.is_null_or_undefined() { String::new() } else { v.to_rust_string_lossy(scope) }
}
fn nodes_array<'s>(scope: &mut v8::PinScope<'s, '_>, state: &Shared, nodes: &[NodeId]) -> v8::Local<'s, v8::Value> {
    let items: Vec<v8::Local<v8::Value>> = nodes.iter().filter_map(|n| wrap(scope, state, *n).map(|o| o.into())).collect();
    v8::Array::new_with_elements(scope, &items).into()
}
/// Every `__olo.*` function: `args.data()` names it.
fn native(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let Some(state) = scope.get_slot::<Shared>().cloned() else { return };
    let name = args.data().to_rust_string_lossy(scope);
    let node = |scope: &mut v8::PinScope, i: i32| node_of(scope, args.get(i));
    match name.as_str() {
        "nodeType" => {
            if let Some(n) = node(scope, 0) {
                let t = state.borrow().document.node_type(n).ok();
                rv.set_int32(match t {
                    Some(NodeType::Element) => 1,
                    Some(NodeType::Text) => 3,
                    Some(NodeType::ProcessingInstruction) => 7,
                    Some(NodeType::Comment) => 8,
                    Some(NodeType::Document) => 9,
                    Some(NodeType::Doctype) => 10,
                    Some(NodeType::Fragment) => 11,
                    None => 0,
                });
            }
        }
        "nodeName" => {
            if let Some(n) = node(scope, 0) {
                let s = state.borrow();
                let doc = &s.document;
                let text = match doc.node_type(n) {
                    Ok(NodeType::Element) => {
                        let name = doc.name(n).ok().flatten().unwrap_or("");
                        if doc.namespace(n).ok().flatten() == Some(Namespace::Html) { name.to_ascii_uppercase() } else { name.to_owned() }
                    }
                    Ok(NodeType::Text) => "#text".into(),
                    Ok(NodeType::Comment) => "#comment".into(),
                    Ok(NodeType::Document) => "#document".into(),
                    Ok(NodeType::Fragment) => "#document-fragment".into(),
                    Ok(NodeType::Doctype) => doc.doctype(n).ok().flatten().map(|d| d.name.to_owned()).unwrap_or_default(),
                    Ok(NodeType::ProcessingInstruction) => doc.name(n).ok().flatten().unwrap_or("").to_owned(),
                    Err(_) => String::new(),
                };
                drop(s);
                rv.set(string(scope, &text));
            }
        }
        "namespace" => {
            if let Some(n) = node(scope, 0) {
                let ns = state.borrow().document.namespace(n).ok().flatten();
                let text = match ns {
                    Some(Namespace::Html) => "http://www.w3.org/1999/xhtml",
                    Some(Namespace::Svg) => "http://www.w3.org/2000/svg",
                    Some(Namespace::MathMl) => "http://www.w3.org/1998/Math/MathML",
                    None => "",
                };
                rv.set(string(scope, text));
            }
        }
        "parent" | "firstChild" | "lastChild" | "nextSibling" | "previousSibling" => {
            if let Some(n) = node(scope, 0) {
                let related = {
                    let s = state.borrow();
                    let doc = &s.document;
                    match name.as_str() {
                        "parent" => doc.parent(n),
                        "firstChild" => doc.first_child(n),
                        "lastChild" => doc.last_child(n),
                        "nextSibling" => doc.next_sibling(n),
                        _ => doc.previous_sibling(n),
                    }
                    .ok()
                    .flatten()
                };
                match related.and_then(|r| wrap(scope, &state, r)) {
                    Some(o) => rv.set(o.into()),
                    None => rv.set_null(),
                }
            }
        }
        "childNodes" | "children" => {
            if let Some(n) = node(scope, 0) {
                let elements_only = name == "children";
                let nodes: Vec<NodeId> = {
                    let s = state.borrow();
                    let doc = &s.document;
                    let mut out = Vec::new();
                    let mut child = doc.first_child(n).ok().flatten();
                    while let Some(c) = child {
                        if !elements_only || doc.node_type(c).ok() == Some(NodeType::Element) {
                            out.push(c);
                        }
                        child = doc.next_sibling(c).ok().flatten();
                    }
                    out
                };
                rv.set(nodes_array(scope, &state, &nodes));
            }
        }
        "data" => {
            if let Some(n) = node(scope, 0) {
                let text = state.borrow().document.value(n).ok().flatten().map(|t| t.to_owned()).unwrap_or_default();
                rv.set(string(scope, &text));
            }
        }
        "setData" => {
            if let Some(n) = node(scope, 0) {
                let text = arg_string(scope, &args, 1);
                let _ = state.borrow_mut().document.set_value(n, text);
            }
        }
        "textContent" => {
            if let Some(n) = node(scope, 0) {
                let mut out = String::new();
                let _ = html::text_content(&state.borrow().document, n, &mut out, 16 << 20);
                rv.set(string(scope, &out));
            }
        }
        "setTextContent" => {
            if let Some(n) = node(scope, 0) {
                let text = arg_string(scope, &args, 1);
                let mut s = state.borrow_mut();
                let doc = &mut s.document;
                let _ = (|| -> olo_web::dom::Result<()> {
                    match doc.node_type(n)? {
                        NodeType::Text | NodeType::Comment | NodeType::ProcessingInstruction => {
                            doc.set_value(n, text)?;
                        }
                        _ => {
                            while let Some(c) = doc.first_child(n)? {
                                doc.detach(c)?;
                            }
                            if !text.is_empty() {
                                let t = doc.create_text(text)?;
                                doc.append_child(n, t)?;
                            }
                        }
                    }
                    Ok(())
                })();
            }
        }
        "attr" => {
            if let Some(n) = node(scope, 0) {
                let attribute = attribute_name(&state, n, arg_string(scope, &args, 1));
                let value = state.borrow().document.attribute(n, &attribute).ok().flatten().map(|v| v.to_owned());
                match value {
                    Some(v) => rv.set(string(scope, &v)),
                    None => rv.set_null(),
                }
            }
        }
        "setAttr" => {
            if let Some(n) = node(scope, 0) {
                let attribute = attribute_name(&state, n, arg_string(scope, &args, 1));
                let value = arg_string(scope, &args, 2);
                if attribute.is_empty() {
                    throw(scope, "InvalidCharacterError: empty attribute name");
                    return;
                }
                let _ = state.borrow_mut().document.set_attribute(n, &attribute, value);
            }
        }
        "removeAttr" => {
            if let Some(n) = node(scope, 0) {
                let attribute = attribute_name(&state, n, arg_string(scope, &args, 1));
                let _ = state.borrow_mut().document.remove_attribute(n, &attribute);
            }
        }
        "attrNames" => {
            if let Some(n) = node(scope, 0) {
                let names: Vec<String> = state.borrow().document.attributes_ns(n).map(|a| a.map(|a| a.qualified_name.to_owned()).collect()).unwrap_or_default();
                let items: Vec<v8::Local<v8::Value>> = names.iter().map(|s| string(scope, s)).collect();
                rv.set(v8::Array::new_with_elements(scope, &items).into());
            }
        }
        "append" | "insertBefore" => {
            let (Some(parent), Some(child)) = (node(scope, 0), node(scope, 1)) else {
                throw(scope, "TypeError: not a node");
                return;
            };
            let before = if name == "insertBefore" { node(scope, 2) } else { None };
            let result = {
                let mut s = state.borrow_mut();
                let doc = &mut s.document;
                (|| -> olo_web::dom::Result<()> {
                    // A fragment's children move, the fragment stays empty.
                    if doc.node_type(child)? == NodeType::Fragment {
                        while let Some(c) = doc.first_child(child)? {
                            doc.detach(c)?;
                            if before.is_some() {
                                doc.insert_before(parent, c, before)?
                            } else {
                                doc.append_child(parent, c)?
                            };
                        }
                        return Ok(());
                    }
                    if doc.parent(child)?.is_some() {
                        doc.detach(child)?;
                    }
                    if before.is_some() {
                        doc.insert_before(parent, child, before)?
                    } else {
                        doc.append_child(parent, child)?
                    };
                    Ok(())
                })()
            };
            if let Err(e) = result {
                throw(scope, &format!("HierarchyRequestError: {e:?}"));
            }
        }
        "remove" => {
            if let Some(n) = node(scope, 0) {
                let _ = state.borrow_mut().document.detach(n);
            }
        }
        "createElement" => {
            let tag = arg_string(scope, &args, 0);
            let ns = arg_string(scope, &args, 1);
            let namespace = match ns.as_str() {
                "http://www.w3.org/2000/svg" => Namespace::Svg,
                "http://www.w3.org/1998/Math/MathML" => Namespace::MathMl,
                _ => Namespace::Html,
            };
            let tag = if namespace == Namespace::Html { tag.to_ascii_lowercase() } else { tag };
            if tag.is_empty() || !tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':' || c == '.') {
                throw(scope, "InvalidCharacterError: bad tag name");
                return;
            }
            let created = state.borrow_mut().document.create_element(&tag, namespace).ok();
            match created.and_then(|n| wrap(scope, &state, n)) {
                Some(o) => rv.set(o.into()),
                None => throw(scope, "could not create the element"),
            }
        }
        "templateContents" => {
            if let Some(n) = node(scope, 0) {
                let contents = state.borrow().document.template_contents(n).ok().flatten();
                if let Some(o) = contents.and_then(|c| wrap(scope, &state, c)) {
                    rv.set(o.into());
                }
            }
        }
        "createText" | "createComment" | "createFragment" => {
            let text = arg_string(scope, &args, 0);
            let created = {
                let mut s = state.borrow_mut();
                match name.as_str() {
                    "createText" => s.document.create_text(text).ok(),
                    "createComment" => s.document.create_comment(text).ok(),
                    _ => s.document.create_fragment().ok(),
                }
            };
            match created.and_then(|n| wrap(scope, &state, n)) {
                Some(o) => rv.set(o.into()),
                None => throw(scope, "could not create the node"),
            }
        }
        "clone" => {
            if let Some(n) = node(scope, 0) {
                let deep = args.get(1).boolean_value(scope);
                let copy = {
                    let mut s = state.borrow_mut();
                    let doc = &mut s.document;
                    if deep {
                        doc.clone_subtree(n).ok()
                    } else {
                        (|| -> olo_web::dom::Result<NodeId> {
                            match doc.node_type(n)? {
                                NodeType::Element => {
                                    let name = doc.name(n)?.unwrap_or("div").to_owned();
                                    let ns = doc.namespace(n)?.unwrap_or(Namespace::Html);
                                    let e = doc.create_element(&name, ns)?;
                                    let attributes: Vec<(String, String)> = doc.attributes_ns(n)?.map(|a| (a.qualified_name.to_owned(), a.value.to_owned())).collect();
                                    for (k, v) in attributes {
                                        doc.set_attribute(e, &k, v)?;
                                    }
                                    Ok(e)
                                }
                                NodeType::Text => doc.create_text(doc.value(n)?.unwrap_or("").to_owned()),
                                NodeType::Comment => doc.create_comment(doc.value(n)?.unwrap_or("").to_owned()),
                                _ => doc.create_fragment(),
                            }
                        })()
                        .ok()
                    }
                };
                match copy.and_then(|c| wrap(scope, &state, c)) {
                    Some(o) => rv.set(o.into()),
                    None => throw(scope, "could not clone the node"),
                }
            }
        }
        "query" | "matches" => {
            let Some(root) = node(scope, 0) else { return };
            let selector = arg_string(scope, &args, 1);
            let all = name == "query" && args.get(2).boolean_value(scope);
            let matching = name == "matches";
            let result = {
                let s = state.borrow();
                select(&s.document, root, &selector, s.quirks == QuirksMode::Quirks, all, matching)
            };
            match result {
                Err(e) => throw(scope, &format!("SyntaxError: '{selector}' is not a valid selector ({e})")),
                Ok(nodes) if matching => rv.set_bool(!nodes.is_empty()),
                Ok(nodes) if all => rv.set(nodes_array(scope, &state, &nodes)),
                Ok(nodes) => match nodes.first().and_then(|n| wrap(scope, &state, *n)) {
                    Some(o) => rv.set(o.into()),
                    None => rv.set_null(),
                },
            }
        }
        "innerHTML" | "outerHTML" => {
            if let Some(n) = node(scope, 0) {
                let mut out = String::new();
                let mut budget = 32 << 20;
                let s = state.borrow();
                let _ = if name == "innerHTML" { html::inner_html(&s.document, n, &mut out, &mut budget) } else { html::serialize(&s.document, n, &mut out, &mut budget) };
                drop(s);
                rv.set(string(scope, &out));
            }
        }
        "setInnerHTML" => {
            if let Some(n) = node(scope, 0) {
                let markup = arg_string(scope, &args, 1);
                let mut s = state.borrow_mut();
                let quirks = s.quirks;
                let doc = &mut s.document;
                let result = (|| -> olo_web::dom::Result<()> {
                    let context = if doc.node_type(n)? == NodeType::Element { n } else { doc.root() };
                    let fragment = html::parse_fragment_into(doc, context, &markup, quirks)?;
                    // A template's markup is its contents, not its children.
                    let target = doc.template_contents(n)?.unwrap_or(n);
                    while let Some(c) = doc.first_child(target)? {
                        doc.detach(c)?;
                    }
                    while let Some(c) = doc.first_child(fragment)? {
                        doc.detach(c)?;
                        doc.append_child(target, c)?;
                    }
                    Ok(())
                })();
                drop(s);
                if let Err(e) = result {
                    throw(scope, &format!("innerHTML: {e:?}"));
                }
            }
        }
        "docRoot" => {
            let root = state.borrow().document.root();
            if let Some(o) = wrap(scope, &state, root) {
                rv.set(o.into());
            }
        }
        "byId" => {
            let id = arg_string(scope, &args, 0);
            let found = {
                let s = state.borrow();
                find_by_id(&s.document, &id)
            };
            match found.and_then(|n| wrap(scope, &state, n)) {
                Some(o) => rv.set(o.into()),
                None => rv.set_null(),
            }
        }
        "isConnected" => {
            if let Some(n) = node(scope, 0) {
                let s = state.borrow();
                let doc = &s.document;
                let mut at = Some(n);
                let mut connected = false;
                while let Some(a) = at {
                    if a == doc.root() {
                        connected = true;
                        break;
                    }
                    at = doc.parent(a).ok().flatten();
                }
                rv.set_bool(connected);
            }
        }
        "log" => {
            let level = arg_string(scope, &args, 0);
            let text = arg_string(scope, &args, 1);
            let mut s = state.borrow_mut();
            if s.report.console.len() < 500 {
                let line = format!("[{level}] {}", text.chars().take(2000).collect::<String>());
                s.report.console.push(line);
            }
        }
        "fetchSync" => {
            let url = arg_string(scope, &args, 0);
            let method = arg_string(scope, &args, 1);
            let body = arg_string(scope, &args, 2);
            let result = {
                let mut s = state.borrow_mut();
                match s.fetcher.as_mut() {
                    Some(f) => f(&url, if method.is_empty() { "GET" } else { &method }, &body),
                    None => Err("no network for scripts".into()),
                }
            };
            match result {
                Ok(fetched) => {
                    let object = v8::Object::new(scope);
                    let pairs: [(&str, v8::Local<v8::Value>); 4] = [
                        ("status", v8::Integer::new(scope, i32::from(fetched.status)).into()),
                        ("contentType", string(scope, &fetched.content_type)),
                        ("body", string(scope, &String::from_utf8_lossy(&fetched.body))),
                        ("url", string(scope, &fetched.url)),
                    ];
                    for (k, v) in pairs {
                        let key = v8::String::new(scope, k).unwrap();
                        object.set(scope, key.into(), v);
                    }
                    rv.set(object.into());
                }
                Err(e) => throw(scope, &format!("TypeError: fetch failed: {e}")),
            }
        }
        // A script inserted by a script: compiled and run here, in the global
        // scope, its exception recorded like a page script's. Returns whether
        // it ran to the end.
        "runScript" => {
            let source = arg_string(scope, &args, 0);
            let name = arg_string(scope, &args, 1);
            state.borrow_mut().report.scripts += 1;
            v8::tc_scope!(let tc, scope);
            let ran = (|| {
                let code = v8::String::new(tc, &source)?;
                let resource = string(tc, &name);
                let origin = v8::ScriptOrigin::new(tc, resource, 0, 0, false, 0, None, false, false, false, None);
                let script = v8::Script::compile(tc, code, Some(&origin))?;
                script.run(tc)
            })()
            .is_some();
            if !ran {
                let error = exception_text(tc, &name);
                state.borrow_mut().report.errors.push(error);
            }
            rv.set_bool(ran);
        }
        // Geometry, from a layout of the document as it is now.
        "rect" | "paddingRect" | "scrollSize" => {
            if let Some(n) = node(scope, 0) {
                let mut s = state.borrow_mut();
                let kind = name.as_str();
                let values = s.measured().and_then(|m| match kind {
                    "rect" => m.border_box(n).map(|r| [r.x, r.y, r.width, r.height]),
                    "paddingRect" => m.padding_box(n).map(|r| [r.x, r.y, r.width, r.height]),
                    _ => m.scroll_size(n).map(|(w, h)| [0., 0., w, h]),
                });
                drop(s);
                match values {
                    Some(v) => {
                        let items: Vec<v8::Local<v8::Value>> = v.iter().map(|x| v8::Number::new(scope, *x).into()).collect();
                        rv.set(v8::Array::new_with_elements(scope, &items).into());
                    }
                    None => rv.set_null(),
                }
            }
        }
        "computed" => {
            if let Some(n) = node(scope, 0) {
                let property = arg_string(scope, &args, 1);
                let mut s = state.borrow_mut();
                let text = s.measured().and_then(|m| m.computed(n, &property));
                drop(s);
                match text {
                    Some(t) => rv.set(string(scope, &t)),
                    None => rv.set_null(),
                }
            }
        }
        "docSize" => {
            let mut s = state.borrow_mut();
            let size = s.measured().map(|m| m.document_size());
            drop(s);
            let (w, h) = size.unwrap_or_else(|| {
                let s = state.borrow();
                (s.config.viewport.0, s.config.viewport.1)
            });
            let items: Vec<v8::Local<v8::Value>> = vec![v8::Number::new(scope, w).into(), v8::Number::new(scope, h).into()];
            rv.set(v8::Array::new_with_elements(scope, &items).into());
        }
        "revision" => {
            let r = state.borrow().document.revision();
            rv.set(v8::Number::new(scope, r as f64).into());
        }
        "url" => {
            let url = state.borrow().config.url.clone();
            rv.set(string(scope, &url));
        }
        "viewport" => {
            let (w, h, scale) = {
                let s = state.borrow();
                (s.config.viewport.0, s.config.viewport.1, s.config.scale)
            };
            let items: Vec<v8::Local<v8::Value>> = vec![v8::Number::new(scope, w).into(), v8::Number::new(scope, h).into(), v8::Number::new(scope, scale).into()];
            rv.set(v8::Array::new_with_elements(scope, &items).into());
        }
        "userAgent" => {
            let ua = state.borrow().config.user_agent.clone();
            rv.set(string(scope, &ua));
        }
        "language" => {
            let l = state.borrow().config.language.clone();
            rv.set(string(scope, &l));
        }
        "count" => {
            // Timers and listeners the prelude accounts for, for the report.
            let kind = arg_string(scope, &args, 0);
            let mut s = state.borrow_mut();
            if kind == "timer" {
                s.report.timers += 1;
            } else {
                s.report.listeners += 1;
            }
        }
        _ => {}
    }
}
/// Elements under `root` (or `root` itself for `matching`) matching the
/// selector list, in document order; `all` keeps every one.
fn select(doc: &Document, root: NodeId, selector: &str, quirks: bool, all: bool, matching: bool) -> Result<Vec<NodeId>, String> {
    let syntax = css::Syntax::parse(Arc::from(selector), Default::default(), None).map_err(|e| format!("{e:?}"))?;
    let selectors = Selectors::compile(syntax.components(), CompileOptions { namespaces: &[], default_namespace: None }, Default::default(), None).map_err(|e| format!("{e:?}"))?;
    let tree = DomTree::new(doc);
    let options = MatchOptions { html_document: true, quirks, scope: Some(root), ..Default::default() };
    let hit = |node: NodeId| selectors.matches(&tree, node, options.clone(), MatchLimits::default(), None).ok().flatten().is_some();
    if matching {
        return Ok(if doc.node_type(root).map_err(|e| format!("{e:?}"))? == NodeType::Element && hit(root) { vec![root] } else { vec![] });
    }
    let mut out = Vec::new();
    let mut stack = vec![root];
    let mut first = true;
    let mut visited = 0usize;
    while let Some(n) = stack.pop() {
        visited += 1;
        if visited > 2_000_000 {
            break;
        }
        if !first && doc.node_type(n).ok() == Some(NodeType::Element) && hit(n) {
            out.push(n);
            if !all {
                break;
            }
        }
        first = false;
        // Push children in reverse so the first child is handled next.
        let mut children = Vec::new();
        let mut child = doc.first_child(n).ok().flatten();
        while let Some(c) = child {
            children.push(c);
            child = doc.next_sibling(c).ok().flatten();
        }
        stack.extend(children.into_iter().rev());
    }
    Ok(out)
}
fn find_by_id(doc: &Document, id: &str) -> Option<NodeId> {
    if id.is_empty() {
        return None;
    }
    let mut stack = vec![doc.root()];
    while let Some(n) = stack.pop() {
        if doc.node_type(n).ok() == Some(NodeType::Element) && doc.attribute(n, "id").ok().flatten() == Some(id) {
            return Some(n);
        }
        let mut children = Vec::new();
        let mut child = doc.first_child(n).ok().flatten();
        while let Some(c) = child {
            children.push(c);
            child = doc.next_sibling(c).ok().flatten();
        }
        stack.extend(children.into_iter().rev());
    }
    None
}
/// Where a script's source comes from: inline text, or a URL to fetch.
pub enum Source {
    Inline(Arc<str>),
    External(String),
}
/// The page's scripts in the order they run: classic scripts (deferred
/// ones included, since everything runs after parsing) in document order,
/// then async ones. Modules are not run (no `import` yet), so `nomodule`
/// scripts are, as in a browser without module support. Resolve each
/// external URL against `base` with `url::Url`.
pub fn page_scripts(doc: &Document, base: &str) -> Vec<(Source, String)> {
    let base = url::Url::parse(base).ok();
    let mut classic = Vec::new();
    let mut asynchronous = Vec::new();
    let mut stack = vec![doc.root()];
    while let Some(n) = stack.pop() {
        let Ok(children) = doc.children(n) else { continue };
        let children: Vec<NodeId> = children.collect();
        for c in children.into_iter().rev() {
            stack.push(c);
        }
        if doc.node_type(n) != Ok(NodeType::Element) || doc.namespace(n) != Ok(Some(Namespace::Html)) || doc.name(n) != Ok(Some("script")) {
            continue;
        }
        let kind = doc.attribute(n, "type").ok().flatten().unwrap_or("").trim().to_ascii_lowercase();
        let javascript = kind.is_empty()
            || matches!(kind.as_str(), "text/javascript" | "application/javascript" | "text/ecmascript" | "application/ecmascript" | "application/x-javascript" | "text/jscript" | "text/x-javascript");
        if !javascript {
            continue;
        }
        let entry = match doc.attribute(n, "src").ok().flatten().filter(|s| !s.trim().is_empty()) {
            Some(src) => {
                let resolved = match &base {
                    Some(b) => b.join(src.trim()).map(|u| u.to_string()).unwrap_or_else(|_| src.trim().to_owned()),
                    None => src.trim().to_owned(),
                };
                (Source::External(resolved.clone()), resolved)
            }
            None => {
                let mut text = String::new();
                let _ = html::text_content(doc, n, &mut text, 16 << 20);
                if text.trim().is_empty() {
                    continue;
                }
                (Source::Inline(Arc::from(text)), format!("inline script {}", classic.len() + asynchronous.len() + 1))
            }
        };
        let is_async = doc.attribute(n, "async").ok().flatten().is_some() && matches!(entry.0, Source::External(_));
        if is_async { asynchronous.push(entry) } else { classic.push(entry) }
    }
    classic.extend(asynchronous);
    classic
}
