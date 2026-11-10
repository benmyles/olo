//! Initial static page pipeline. Run entirely on workers. Scripts are inert;
//! network resources, complete UA defaults and browser runtime remain required.
mod background;
mod interaction;
use crate::{
    css::{
        self,
        cascade::{Origin, SheetBuilder},
        style::{self, Defaults, Host, StyleTree},
    },
    dom::{Document, NodeType},
    font::Scope,
    html, layout, paint,
    text::shaping,
};
pub use interaction::Interaction;
use std::sync::{Arc, atomic::AtomicBool};
pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Html,
    /// The page's scripts running (the browser's, not the engine's).
    Scripts,
    Styles,
    Boxes,
    Text,
    Layout,
    Paint,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    Stylesheet,
    Image,
    /// A script (classic or module) or a `<link rel=preload>` for one: only
    /// ever fetched ahead (`prefetch`), its load the host's.
    Script,
    /// A `@font-face` source: WOFF2, WOFF, TrueType or OpenType bytes.
    Font,
}
pub enum Resource {
    /// A stylesheet and the absolute URL it was fetched from, which its
    /// relative `url()`s resolve against.
    Stylesheet(Arc<css::Syntax>, Option<Arc<str>>),
    Image(Arc<crate::image::Image>),
    Font(Arc<[u8]>),
}
pub struct Page {
    images: Option<Arc<crate::image::Set>>,
    /// Pictures not here when it was prepared (it showed without them).
    late: Vec<Late>,
    /// The interaction the scene was rendered with.
    rendered_interaction: Interaction,
    /// Elements whose `transform` has animated: painted as moved layers.
    animated: std::collections::HashSet<crate::dom::NodeId>,
    /// Query containers' sizes as the last layout had them (`@container`).
    containers: crate::dom::NodeMap<[f64; 2]>,
    /// Its CSS animations: started when, at which moment (see
    /// `set_time`), still changing.
    timeline: css::animation::Timeline,
    pub document: Document,
    pub scene: Arc<paint::Scene>,
    pub title: String,
    pub scripts: usize,
    pub base_href: Option<String>,
    pub interaction: Interaction,
    pub interaction_mask: u64,
    pub stylesheet_issues: usize,
    /// What the engine substituted while rendering this page: structured for
    /// agents and the eval, so a rendered page never passes as complete.
    pub fallbacks: layout::Fallbacks,
    rules: Arc<css::cascade::RuleSet>,
    /// Every stylesheet in document order (the UA sheet first) with its URL,
    /// kept so a changed medium (viewport, scheme) can recompile the `@media` rules.
    sheets: Vec<Sheet>,
    base_scope: Scope,
    sheet_ids: Vec<(crate::dom::NodeId, u64)>,
    /// A requested compile stays here until used, even if other pages
    /// have evicted its rules from the shared cache in the meantime.
    pending_compile: Option<PendingCompile>,
    /// How its rules changed, compilation to compilation.
    rule_changes: RuleChanges,
    /// The document and revision the sheets were last looked for at.
    sheets_checked: Option<(u64, u64)>,
    /// The style tree its pictures were last looked for in
    /// (`note_pictures`): the next looks only where a tree differs from it.
    pictures_scanned: Option<Arc<StyleTree>>,
    media: css::media::Media,
    /// Whether any stylesheet has `@media` rules, so only such pages recompile.
    pub media_dependent: bool,
    /// The fonts this page styles and shapes with: the base scope plus its
    /// web faces. Hosts for this page are made for this scope.
    scope: Scope,
    quirks: bool,
    resource_issues: usize,
}
/// A document parsed, with its stylesheets, images and fonts loaded, ready
/// to style for a host made for its `scope()`.
/// The picture an `<img>` shows (its `<picture>`, srcset and sizes'
/// choice) or a `<video>` (its poster), at the density it is for.
fn picture_source(doc: &Document, node: crate::dom::NodeId, media: &css::media::Media, cancel: Option<&AtomicBool>) -> Option<crate::srcset::Choice> {
    if doc.name(node).ok().flatten() == Some("video") {
        let poster = doc.attribute(node, "poster").ok().flatten()?.trim();
        return (!poster.is_empty()).then(|| crate::srcset::Choice { url: poster.to_owned(), density: 1. });
    }
    crate::srcset::choose(doc, node, media, cancel)
}
/// A picture the page showed before it arrived (its host answered
/// `PENDING`): bound when it does (`Page::bind_late`).
#[derive(Clone, Debug)]
enum Late {
    /// An `<img>`, its source chosen again then.
    Element(crate::dom::NodeId),
    /// A `background-image`, by the URL as written, and what it resolves
    /// against.
    Background(Arc<str>, Option<String>),
}
pub struct Prepared {
    late: Vec<Late>,
    document: Document,
    sheets: Vec<Sheet>,
    rules: Arc<css::cascade::RuleSet>,
    media: css::media::Media,
    media_dependent: bool,
    font_faces: Vec<css::fonts::FontFace>,
    images: Option<Arc<crate::image::Set>>,
    title: String,
    scripts: usize,
    base: Option<String>,
    resource_issues: usize,
    quirks: bool,
    scope: Scope,
    /// The scope without the page's web fonts, to add them to again when
    /// the sheets change.
    base_scope: Scope,
    /// The style/link elements the sheets came from, hashed.
    sheet_ids: Vec<(crate::dom::NodeId, u64)>,
    /// The `background-image` URLs the sheets name (relative to each sheet).
    backgrounds: Vec<(Arc<str>, Option<Arc<str>>)>,
    /// Whether the rules (and fonts) are an earlier preparation's: styles
    /// computed against them then can be kept.
    rules_reused: bool,
}
impl Prepared {
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
    pub fn document(&self) -> &Document {
        &self.document
    }
    /// The document, to change and measure again (benchmarks).
    pub fn into_document(self) -> Document {
        self.document
    }
    /// Bind more images (canvases the scripts drew) before finishing.
    pub fn bind_images(&mut self, extra: Vec<(crate::dom::NodeId, Arc<crate::image::Image>)>, cancel: Option<&AtomicBool>) -> Result<()> {
        if extra.is_empty() {
            return Ok(());
        }
        self.images = Some(match &self.images {
            Some(set) => set.with_extra(&self.document, extra, SCRIPTED_IMAGE_LIMITS, cancel)?,
            None => crate::image::Set::from_bindings(&self.document, extra, SCRIPTED_IMAGE_LIMITS, cancel)?,
        });
        Ok(())
    }
    /// A measurer for this page as prepared: styles and lays it out on
    /// demand (scripts' measurements, benchmarks).
    pub fn measurer(&self) -> Measurer {
        Measurer {
            rules: self.rules.clone(),
            media: self.media,
            media_dependent: self.media_dependent,
            font_faces: self.font_faces.clone(),
            images: self.images.clone(),
            scope: self.scope.clone(),
            base_scope: self.base_scope.clone(),
            quirks: self.quirks,
            sheet_ids: self.sheet_ids.clone(),
            sheets: self.sheets.clone(),
            base: self.base.clone(),
            checked: None,
            late: self.late.len(),
            changes: RuleChanges::default(),
            own_images: self.images.clone(),
            bound: (0, 0),
        }
    }
    /// What the stylesheets declared that the engine dropped.
    pub fn issues(&self) -> &[css::cascade::Issue] {
        self.rules.issues()
    }
}
fn media_for(host: &impl Host, viewport: layout::Viewport) -> css::media::Media {
    css::media::Media::screen(viewport.width, viewport.height, viewport.scale, host.scheme() == css::color::Scheme::Dark)
}
/// Compile every sheet for one medium, with the `@font-face` rules met.
type Compiled = (Arc<css::cascade::RuleSet>, bool, Vec<css::fonts::FontFace>, Vec<(Arc<str>, Option<Arc<str>>)>);
/// Rules compiled lately, by the sheets (origin, URL, text) and medium
/// they came from: a page's scripts measure with the rules its scripted
/// render then needs, and a reload brings the same sheets again (GitHub
/// compiled its 2.5 MB of CSS three times on one load, 50 ms each).
struct CompiledEntry {
    key: CompileKey,
    compiled: Compiled,
}
/// What rules are compiled from: the sheets (origin, URL, text, shadow
/// tree) and the medium.
struct CompileKey {
    media: css::media::Media,
    sheets: Vec<(Origin, Option<Arc<str>>, css::SourceHandle, Option<u64>)>,
}
impl CompileKey {
    fn of(sheets: &[Sheet], media: css::media::Media) -> Self {
        Self { media, sheets: sheets.iter().map(|(s, o, u, k)| (*o, u.clone(), s.source_handle(), *k)).collect() }
    }
    fn is(&self, sheets: &[Sheet], media: css::media::Media) -> bool {
        self.media == media
            && self.sheets.len() == sheets.len()
            && self.sheets.iter().zip(sheets).all(|((o, u, s, k), (syntax, origin, url, scope))| o == origin && u == url && k == scope && s.len() == syntax.source().len() && **s == *syntax.source())
    }
}
/// (Shared by the pages' threads and the compiles in the background.)
static COMPILED: std::sync::Mutex<Vec<CompiledEntry>> = std::sync::Mutex::new(Vec::new());
/// A compile under way in the background: its rules once there.
#[derive(Default)]
struct Compiling {
    state: std::sync::Mutex<CompileState>,
    done: std::sync::Condvar,
    cancel: AtomicBool,
}
#[derive(Clone, Debug)]
struct CompileFailure(Arc<Error>);
impl std::fmt::Display for CompileFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for CompileFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref().as_ref())
    }
}
#[derive(Default)]
struct CompileState {
    result: Option<std::result::Result<Compiled, CompileFailure>>,
    wakes: Vec<std::sync::Weak<std::sync::Mutex<Wake>>>,
    users: usize,
}
struct PendingCompile {
    key: CompileKey,
    compiling: CompileUse,
    wake: Arc<std::sync::Mutex<Wake>>,
}
/// Both asynchronous pages and synchronous waiters own a use. The last
/// one leaving cancels unfinished work; the registry/queue are not users.
struct CompileUse {
    job: Arc<Compiling>,
}
impl std::ops::Deref for CompileUse {
    type Target = Compiling;
    fn deref(&self) -> &Compiling {
        &self.job
    }
}
impl Drop for CompileUse {
    fn drop(&mut self) {
        let abandoned = {
            let mut state = self.state.lock().unwrap();
            state.users -= 1;
            let abandoned = state.users == 0 && state.result.is_none();
            if abandoned {
                self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            abandoned
        };
        if abandoned {
            COMPILING.lock().unwrap().retain(|(_, c)| !Arc::ptr_eq(c, &self.job));
            if let Some(queue) = COMPILE_QUEUE.get() {
                queue.remove(Arc::as_ptr(&self.job) as usize);
            }
        }
    }
}
static COMPILE_QUEUE: std::sync::OnceLock<Arc<background::Queue>> = std::sync::OnceLock::new();
impl Compiling {
    fn claim(self: &Arc<Self>) -> Option<CompileUse> {
        let mut state = self.state.lock().unwrap();
        if self.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return None;
        }
        state.users += 1;
        Some(CompileUse { job: self.clone() })
    }
    fn result(&self) -> Option<Result<Compiled>> {
        self.state.lock().unwrap().result.clone().map(|result| result.map_err(|e| e.into()))
    }
    fn finish(self: &Arc<Self>, compiled: Result<Compiled>) {
        let wakes = {
            let mut state = self.state.lock().unwrap();
            state.result = Some(compiled.map_err(|e| CompileFailure(Arc::new(e))));
            std::mem::take(&mut state.wakes)
        };
        self.done.notify_all();
        COMPILING.lock().unwrap().retain(|(_, c)| !Arc::ptr_eq(c, self));
        // Every page waiting for these rules hears, once. Dropped or
        // superseded requests do not keep callbacks alive. Call outside
        // all locks: a callback may consume the result or request rules.
        for wake in wakes.into_iter().filter_map(|w| w.upgrade()) {
            let wake = wake.lock().unwrap().clone();
            // An embedding callback failing cannot strand other pages.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| wake()));
        }
    }
}
static COMPILING: std::sync::Mutex<Vec<(CompileKey, Arc<Compiling>)>> = std::sync::Mutex::new(Vec::new());
/// Told when rules compiled in the background are ready (the page renders
/// again, and its refresh takes them).
pub type Wake = Arc<dyn Fn() + Send + Sync>;
/// What a live page's refresh answers while its new rules compile in the
/// background: the page keeps what it shows until they are ready.
pub const COMPILING_RULES: &str = "Rules still compiling";
/// Scopes made lately from a base scope and a page's web fonts: the same
/// faces again (the scripts' measurements, then the scripted render) give
/// the same scope, so styles and texts made with one serve the other.
struct FacesEntry {
    base: u64,
    faces: Vec<(Arc<[u8]>, String)>,
    scope: Scope,
}
thread_local! {
    static SCOPES: std::cell::RefCell<Vec<FacesEntry>> = const { std::cell::RefCell::new(Vec::new()) };
}
fn scope_with_faces(base: &Scope, faces: Vec<(Arc<[u8]>, crate::font::Descriptor)>, cancel: Option<&AtomicBool>) -> Result<Scope> {
    let key: Vec<(Arc<[u8]>, String)> = faces.iter().map(|(d, f)| (d.clone(), format!("{f:?}"))).collect();
    let same = |e: &FacesEntry| e.base == base.context() && e.faces.len() == key.len() && e.faces.iter().zip(&key).all(|((a, x), (b, y))| x == y && (Arc::ptr_eq(a, b) || a[..] == b[..]));
    if let Some(scope) = SCOPES.with(|c| c.borrow().iter().find(|e| same(e)).map(|e| e.scope.clone())) {
        return Ok(scope);
    }
    let scope = base.with_faces(faces, cancel)?;
    SCOPES.with(|c| {
        let mut c = c.borrow_mut();
        c.insert(0, FacesEntry { base: base.context(), faces: key, scope: scope.clone() });
        c.truncate(COMPILED_KEPT);
    });
    Ok(scope)
}
/// How many compiled rule sets are kept (the last ones compiled or used).
const COMPILED_KEPT: usize = 2;
/// How many are kept for all pages together.
const COMPILED_SHARED: usize = 6;
fn compiled_already(sheets: &[Sheet], media: css::media::Media) -> Option<Compiled> {
    let mut c = COMPILED.lock().unwrap();
    let i = c.iter().position(|e| e.key.is(sheets, media))?;
    let entry = c.remove(i);
    let compiled = entry.compiled.clone();
    c.insert(0, entry);
    Some(compiled)
}
/// The rules for these sheets: compiled already, else compiled now (or,
/// when the same compile is under way in the background, waited for).
fn compile(sheets: &[Sheet], media: css::media::Media, cancel: Option<&AtomicBool>) -> Result<Compiled> {
    if let Some(hit) = compiled_already(sheets, media) {
        return Ok(hit);
    }
    let running = COMPILING.lock().unwrap().iter().find_map(|(k, c)| k.is(sheets, media).then(|| c.claim()).flatten());
    if let Some(running) = running {
        let mut state = running.state.lock().unwrap();
        while state.result.is_none() {
            if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
                return Err(paint::Error::Cancelled.into());
            }
            state = running.done.wait_timeout(state, std::time::Duration::from_millis(50)).unwrap().0;
        }
        if let Some(result) = state.result.clone() {
            return result.map_err(|e| e.into());
        }
    }
    compile_now(sheets, media, cancel)
}
/// The rules for these sheets when they are compiled already; else their
/// compile queued on a bounded worker (`wake` told when it is done)
/// and `COMPILING_RULES` answered.
fn compile_in_background(sheets: &[Sheet], media: css::media::Media, wake: &Wake, pending: &mut Option<PendingCompile>) -> Result<Compiled> {
    if let Some(request) = pending.as_ref().filter(|p| p.key.is(sheets, media)) {
        *request.wake.lock().unwrap() = wake.clone();
        return request.compiling.result().unwrap_or_else(|| Err(COMPILING_RULES.into()));
    }
    *pending = None;
    if let Some(hit) = compiled_already(sheets, media) {
        return Ok(hit);
    }
    let (compiling, start) = {
        let mut running = COMPILING.lock().unwrap();
        if let Some(compiling) = running.iter().find_map(|(k, c)| k.is(sheets, media).then(|| c.claim()).flatten()) {
            (compiling, false)
        } else {
            let compiling = Arc::new(Compiling::default());
            running.push((CompileKey::of(sheets, media), compiling.clone()));
            (compiling.claim().unwrap(), true)
        }
    };
    let wake = Arc::new(std::sync::Mutex::new(wake.clone()));
    let mine = compiling.job.clone();
    *pending = Some(PendingCompile { key: CompileKey::of(sheets, media), compiling, wake: wake.clone() });
    {
        // Subscribe and check completion under one lock: finishing
        // between the registry lookup and this point cannot lose a wake.
        let mut state = mine.state.lock().unwrap();
        if let Some(result) = state.result.clone() {
            drop(state);
            return result.map_err(|e| e.into());
        }
        state.wakes.push(Arc::downgrade(&wake));
    }
    if !start {
        return Err(COMPILING_RULES.into());
    }
    let (owned, compiling) = (sheets.to_vec(), mine.clone());
    // Leave CPU for page scripts, layout and the desktop. A burst shares
    // at most two workers; queued obsolete revisions are removed on drop.
    let queue = COMPILE_QUEUE.get_or_init(|| background::Queue::new(std::thread::available_parallelism().map_or(1, |n| n.get().min(2))));
    let started = queue.submit(
        Arc::as_ptr(&mine) as usize,
        Box::new(move || {
            let compiled =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compile_now(&owned, media, Some(&compiling.cancel)))).unwrap_or_else(|_| Err("Stylesheet compiler panicked".into()));
            compiling.finish(compiled);
        }),
    );
    match started {
        Ok(_) => Err(COMPILING_RULES.into()),
        // No worker can run this request: report the failure to every
        // waiter, without moving expensive work onto the caller's loop.
        Err(error) => {
            mine.finish(Err(error.into()));
            mine.result().unwrap()
        }
    }
}
fn compile_now(sheets: &[Sheet], media: css::media::Media, cancel: Option<&AtomicBool>) -> Result<Compiled> {
    let check = || -> Result<()> { if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) { Err(paint::Error::Cancelled.into()) } else { Ok(()) } };
    check()?;
    let mut builder = SheetBuilder::new(Default::default()).with_media(media);
    // Shadow trees with the same sheets in the same order (a component's
    // every instance): compiled once, the others' rules that tree's.
    let mut trees: Vec<(u64, Vec<usize>)> = Vec::new();
    for (i, (_, _, _, scope)) in sheets.iter().enumerate() {
        check()?;
        let Some(scope) = scope else { continue };
        match trees.iter_mut().find(|(k, _)| k == scope) {
            Some((_, list)) => list.push(i),
            None => trees.push((*scope, vec![i])),
        }
    }
    let mut first: std::collections::HashMap<Vec<(&str, u8, Option<&str>)>, u64> = std::collections::HashMap::new();
    let mut aliased = std::collections::HashSet::new();
    for (scope, list) in &trees {
        check()?;
        let identity: Vec<(&str, u8, Option<&str>)> = list.iter().map(|&i| (sheets[i].0.source(), sheets[i].1 as u8, sheets[i].2.as_deref())).collect();
        if crate::env_on!("OLO_ALIAS_DEBUG") {
            eprintln!(
                "  tree {scope:x}: {} sheets, first {:?}{}",
                list.len(),
                sheets[list[0]].0.source().chars().take(60).collect::<String>(),
                first.get(&identity).map(|s| format!(" -> {s:x}")).unwrap_or_default()
            );
        }
        match first.get(&identity) {
            Some(&same) => {
                builder.alias_scope(*scope, same);
                aliased.insert(*scope);
            }
            None => {
                first.insert(identity, *scope);
            }
        }
    }
    for (syntax, origin, base, scope) in sheets {
        if scope.is_some_and(|k| aliased.contains(&k)) {
            continue;
        }
        builder.add_scoped_sheet_from(syntax, *origin, base.clone(), *scope, cancel)?;
    }
    if crate::env_on!("OLO_SHEET_DEBUG") && !aliased.is_empty() {
        eprintln!("  compile: {} shadow trees' sheets compiled as {} trees'", trees.len(), trees.len() - aliased.len());
    }
    let dependent = builder.media_dependent();
    let faces = builder.font_faces().to_vec();
    let backgrounds = builder.background_urls().to_vec();
    let compiled: Compiled = (Arc::new(builder.finish(cancel)?), dependent, faces, backgrounds);
    check()?;
    let mut c = COMPILED.lock().unwrap();
    c.insert(0, CompiledEntry { key: CompileKey::of(sheets, media), compiled: compiled.clone() });
    c.truncate(COMPILED_SHARED);
    Ok(compiled)
}
/// At most this many `background-image` files per page.
const BACKGROUND_IMAGES: usize = 256;
/// At most this many web fonts, and this many decoded bytes, per page.
const FONT_FACES: usize = 64;
const FONT_BYTES: usize = 64 * 1024 * 1024;
/// The user-agent stylesheet: HTML's rendering section (§15), for the
/// properties the engine knows. Form controls and list markers remain.
pub const UA: &str = concat!(
    "svg{overflow:hidden}",
    "html,body,address,article,aside,blockquote,center,dialog,div,figure,figcaption,footer,form,header,hgroup,hr,legend,listing,main,nav,p,plaintext,pre,search,section,xmp,details,summary,fieldset,dl,dd,dt,ul,ol,menu,dir,li,optgroup,option,h1,h2,h3,h4,h5,h6{display:block}",
    "head,link,meta,script,style,title,template,area,base,basefont,datalist,noembed,noframes,param,rp,[hidden]{display:none}",
    // Dialogs (HTML §15.3.3): closed ones hidden; open ones centred across
    // their containing block, framed; a modal one over the viewport (the
    // top layer, as near as stacking gets).
    "dialog:not([open]){display:none}dialog{position:absolute;left:0;right:0;width:fit-content;height:fit-content;margin:auto;border:solid;padding:1em;background-color:Canvas;color:CanvasText}dialog:modal{position:fixed;overflow:auto;top:0;bottom:0;max-width:calc(100% - 6px - 2em);max-height:calc(100% - 6px - 2em);z-index:2147483647}",
    // With scripting, <noscript> holds its markup as raw text (HTML §13.2):
    // never shown.
    "@media (scripting:enabled){noscript{display:none!important}}",
    // Form controls, as Chrome draws them (HTML §15.5): text fields and
    // buttons in 13.33px Arial, fields white with an inset border, buttons
    // grey with an outset one, text areas monospace; one line where a
    // field is, its text vertically centred by the field's layout.
    "video{object-fit:contain}",
    "input,textarea,select,button{display:inline-block;font:13.3333px Arial,sans-serif;letter-spacing:normal;word-spacing:normal;text-align:start;color:FieldText;box-sizing:content-box}",
    "input{padding:1px 2px;border:2px inset light-dark(#767676,#858585);background-color:Field;width:177px;white-space:pre;overflow:clip}",
    "input::placeholder,textarea::placeholder{color:#757575}",
    "textarea{padding:2px;border:1px solid light-dark(#767676,#858585);background-color:Field;font-family:monospace;width:195px;height:36px;white-space:pre-wrap;overflow:auto}",
    "select{border:1px solid light-dark(#767676,#858585);border-radius:0;background-color:ButtonFace;padding:1px 16px 1px 4px;white-space:pre;color:FieldText;box-sizing:border-box;overflow:clip}",
    "input[type=submit],input[type=reset],input[type=button],button{padding:1px 6px;border:2px outset #000;background-color:ButtonFace;color:ButtonText;width:auto;text-align:center;box-sizing:border-box}",
    "input[type=checkbox],input[type=radio]{width:13px;height:13px;padding:0;border:0;margin:3px 3px 3px 4px;background-color:transparent;box-sizing:border-box}input[type=radio]{margin:3px 3px 0 5px}",
    "input[type=hidden]{display:none!important}",
    // Disabled controls, faded as Chrome's.
    ":is(input,select,textarea,button):disabled{color:light-dark(rgba(16,16,16,.3),rgba(255,255,255,.3))}",
    ":is(input,select,textarea):disabled{background-color:light-dark(rgba(239,239,239,.3),rgba(59,59,59,.3));border-color:light-dark(rgba(118,118,118,.3),rgba(133,133,133,.3))}",
    // Controls the engine draws itself, as Chrome's (see paint::control).
    "input,textarea,select,button,meter,progress{appearance:auto}",
    "li{display:list-item}ul,menu,dir{list-style-type:disc;padding-inline-start:40px;margin-block:1em}ol{list-style-type:decimal;padding-inline-start:40px;margin-block:1em}",
    ":is(ul,ol,menu,dir) :is(ul,ol,menu,dir){margin-block:0}:is(ol,ul,menu,dir) ul{list-style-type:circle}:is(ol,ul,menu,dir) :is(ol,ul,menu,dir) ul{list-style-type:square}",
    "table{display:table;border-spacing:2px;box-sizing:border-box}caption{display:table-caption;text-align:center}colgroup{display:table-column-group}col{display:table-column}",
    "thead{display:table-header-group;vertical-align:middle}tbody{display:table-row-group;vertical-align:middle}tfoot{display:table-footer-group;vertical-align:middle}",
    "tr{display:table-row;vertical-align:inherit}td,th{display:table-cell;vertical-align:inherit;padding:1px}th{font-weight:bold;text-align:center}",
    "body{margin:8px}html{color:CanvasText}",
    ":any-link{color:LinkText;text-decoration:underline}u,ins{text-decoration:underline}s,strike,del{text-decoration:line-through}abbr[title]{text-decoration:underline dotted}",
    "blockquote,figure,dl,ol,ul,menu,dir,p{margin-top:1em;margin-bottom:1em}blockquote,figure{margin-left:40px;margin-right:40px}",
    // <q>: its quotes from `quotes` (HTML §15.3.3).
    "q::before{content:open-quote}q::after{content:close-quote}",
    "dd{margin-left:40px}ol,ul,menu,dir{padding-left:40px}",
    "h1{font-size:2em;font-weight:bold;margin-top:.67em;margin-bottom:.67em}h2{font-size:1.5em;font-weight:bold;margin-top:.83em;margin-bottom:.83em}h3{font-size:1.17em;font-weight:bold;margin-top:1em;margin-bottom:1em}h4{font-weight:bold;margin-top:1.33em;margin-bottom:1.33em}h5{font-size:.83em;font-weight:bold;margin-top:1.67em;margin-bottom:1.67em}h6{font-size:.67em;font-weight:bold;margin-top:2.33em;margin-bottom:2.33em}",
    "pre,listing,plaintext,xmp{font-family:monospace;white-space:pre;margin-top:1em;margin-bottom:1em}code,kbd,samp,tt{font-family:monospace}",
    "b,strong{font-weight:bolder}i,cite,em,var,address,dfn{font-style:italic}small{font-size:smaller}big{font-size:larger}",
    "sub{vertical-align:sub;font-size:smaller}sup{vertical-align:super;font-size:smaller}",
    "hr{color:gray;border-style:inset;border-width:1px;margin-top:.5em;margin-bottom:.5em;margin-left:auto;margin-right:auto;overflow:hidden}",
    "a{color:LinkText}mark{background-color:yellow;color:black}",
    "fieldset{margin-left:2px;margin-right:2px;border:2px groove;padding-top:.35em;padding-bottom:.625em;padding-left:.75em;padding-right:.75em}legend{padding-left:2px;padding-right:2px}",
    "button,input,select,textarea{font-family:sans-serif;font-size:13.3333px;margin:0}button{display:inline-block;padding:1px 6px;border:2px outset;background-color:ButtonFace;color:ButtonText;text-align:center;white-space:nowrap}",
    "iframe{border:2px inset}center{text-align:-webkit-center}",
    "input[type=hidden i]{display:none}",
);
/// What the scene is painted with: the canvas in the root element's colour
/// scheme (dark under a dark `color-scheme`, as Chrome paints it), and the
/// person's preferred scheme.
fn paint_options(host: &impl Host, doc: &Document, layout: &layout::Layout) -> Result<paint::Options> {
    let preferred = host.scheme();
    let root = doc.children(doc.root())?.find(|&n| doc.node_type(n) == Ok(crate::dom::NodeType::Element));
    let allowed = root.and_then(|r| layout.cache().tree().styles().snapshot(r)).map_or(css::properties::Keyword::Normal, |s| match s.value(css::properties::Property::ColorScheme) {
        css::style::Value::Keyword(k) => *k,
        _ => css::properties::Keyword::Normal,
    });
    let canvas = host.system_color(css::color::SystemColor::Canvas, css::style::used_scheme(allowed, preferred))?.srgb8_clipped();
    Ok(paint::Options { canvas, dark: preferred == css::color::Scheme::Dark })
}
#[allow(clippy::too_many_arguments)] // Explicit worker resources, viewport, cancellation and progress.
/// Room for what scripts draw and load on top of the page's own images.
const SCRIPTED_IMAGE_LIMITS: crate::image::Limits = PAGE_IMAGE_LIMITS;
/// What a page's images may take, decoded: a photograph-heavy page (Apple's
/// homepage) holds a few hundred megabytes of pixels; past that an image is
/// left out (an issue), the page still shown.
pub const PAGE_IMAGE_LIMITS: crate::image::Limits = crate::image::Limits { pixels: 64 << 20, bytes: 256 << 20, bindings: 2048 };
pub fn load(
    source: Arc<str>,
    host: &impl Host,
    scope: &Scope,
    shaper: &mut shaping::Engine,
    painter: &mut paint::Engine,
    viewport: layout::Viewport,
    cancel: Option<&AtomicBool>,
    progress: impl FnMut(Phase, usize),
) -> Result<Page> {
    load_resources(source, host, scope, shaper, painter, viewport, cancel, progress, |_, _| Err(paint::Error::Unsupported("external stylesheet loading").into()))
}
/// Resolve external sheets on the worker, in document order. The first base
/// href applies even to links appearing before it. None means a failed/blocked
/// resource; the page records the issue and continues with remaining sheets.
#[allow(clippy::too_many_arguments)]
pub fn load_resources(
    source: Arc<str>,
    host: &impl Host,
    scope: &Scope,
    shaper: &mut shaping::Engine,
    painter: &mut paint::Engine,
    viewport: layout::Viewport,
    cancel: Option<&AtomicBool>,
    progress: impl FnMut(Phase, usize),
    mut resource: impl FnMut(Option<&str>, &str) -> Result<Option<css::Syntax>>,
) -> Result<Page> {
    load_assets(source, host, scope, shaper, painter, viewport, cancel, progress, |kind, base, href| match kind {
        ResourceKind::Stylesheet => resource(base, href).map(|s| s.map(|s| Resource::Stylesheet(Arc::new(s), None))),
        ResourceKind::Image | ResourceKind::Font | ResourceKind::Script => Ok(None),
    })
}
/// Typed worker resources. Decoded image allocations are retained across restyle;
/// the caller owns fetch/decode policy and can share repeated URLs. Web fonts
/// are not fetched here (the host is already bound to `scope`); a browser
/// uses `prepare` and `finish` with a host made for the prepared scope.
#[allow(clippy::too_many_arguments)]
pub fn load_assets(
    source: Arc<str>,
    host: &impl Host,
    scope: &Scope,
    shaper: &mut shaping::Engine,
    painter: &mut paint::Engine,
    viewport: layout::Viewport,
    cancel: Option<&AtomicBool>,
    mut progress: impl FnMut(Phase, usize),
    resource: impl FnMut(ResourceKind, Option<&str>, &str) -> Result<Option<Resource>>,
) -> Result<Page> {
    let prepared = prepare(source, scope, media_for(host, viewport), false, cancel, &mut progress, resource)?;
    finish(prepared, host, shaper, painter, viewport, cancel, progress)
}
/// Parse the document and load its stylesheets, images and (when `fonts`)
/// web fonts, through the resource callback. The result's scope has the
/// web faces; make the host for it, then `finish`.
#[allow(clippy::too_many_arguments)]
pub fn prepare(
    source: Arc<str>,
    scope: &Scope,
    media: css::media::Media,
    fonts: bool,
    cancel: Option<&AtomicBool>,
    progress: impl FnMut(Phase, usize),
    resource: impl FnMut(ResourceKind, Option<&str>, &str) -> Result<Option<Resource>>,
) -> Result<Prepared> {
    prepare_scripted(source, scope, media, fonts, cancel, progress, resource, |_, _| {}, None)
}
/// `prepare`, with the page's scripts run between parsing and loading its
/// resources: `scripts` takes the parsed document (and whether it is in
/// quirks mode) and returns it as the scripts left it, so what they added
/// (stylesheets, images, classes) is loaded and styled like the rest. With
/// `None`, `<noscript>` content renders, as in a browser without scripting.
#[allow(clippy::too_many_arguments)] // The page's inputs, each distinct.
pub fn prepare_scripted(
    source: Arc<str>,
    scope: &Scope,
    media: css::media::Media,
    fonts: bool,
    cancel: Option<&AtomicBool>,
    mut progress: impl FnMut(Phase, usize),
    mut resource: impl FnMut(ResourceKind, Option<&str>, &str) -> Result<Option<Resource>>,
    mut prefetch: impl FnMut(ResourceKind, &[(Option<String>, String)]),
    scripts: Option<&mut dyn FnMut(Document, bool, Measurer) -> Result<Document>>,
) -> Result<Prepared> {
    progress(Phase::Html, 0);
    // A page whose scripts run matches `@media (scripting: enabled)` (and
    // the user-agent sheet hides its <noscript>).
    let media = css::media::Media { scripting: scripts.is_some(), ..media };
    let parsed = html::parse(source, html::ParseOptions { scripting: scripts.is_some(), ..Default::default() }, cancel)?;
    let quirks = parsed.quirks == html::QuirksMode::Quirks;
    let mut doc = parsed.document;
    // With scripts, resources are gathered twice (before the scripts, so
    // they can measure a styled page; after, for what they added): the
    // memo hands the second pass what the first fetched.
    let mut memo = Memo { entries: Default::default(), active: scripts.is_some() };
    let mut load = |kind: ResourceKind, base: Option<&str>, href: &str| memo.load(kind, base, href, &mut resource);
    if let Some(run) = scripts {
        let mut first = gather(doc, quirks, scope, media, fonts, cancel, &mut progress, &mut load, &mut prefetch, None)?;
        let measurer = first.measurer();
        progress(Phase::Scripts, 0);
        let first_doc = std::mem::replace(&mut first.document, Document::new(Arc::from(""), Default::default())?);
        doc = run(first_doc, quirks, measurer)?;
        let started = std::time::Instant::now();
        let second = gather(doc, quirks, scope, media, fonts, cancel, &mut progress, &mut load, &mut prefetch, Some(&first))?;
        if crate::env_on!("OLO_WEB_TIMING") {
            let differing = first.sheets.iter().zip(&second.sheets).filter(|(x, y)| x.1 != y.1 || x.2 != y.2 || x.0.source() != y.0.source()).count();
            eprintln!(
                "scripted gather: {:.0} ms; sheets {} before, {} after, {} differ; rules {}",
                started.elapsed().as_secs_f64() * 1000.,
                first.sheets.len(),
                second.sheets.len(),
                differing,
                if second.rules_reused { "reused" } else { "recompiled" }
            );
        }
        if crate::env_on!("OLO_SHEET_DEBUG") {
            for (i, (x, o, u, _)) in second.sheets.iter().enumerate() {
                let was = first.sheets.iter().position(|(y, p, v, _)| p == o && v == u && y.source() == x.source());
                eprintln!("  sheet {i}: {:?} {} bytes, before at {was:?}", u.as_deref().map(|u| u.rsplit('/').next().unwrap_or(u)), x.source().len());
            }
        }
        return Ok(second);
    }
    gather(doc, quirks, scope, media, fonts, cancel, &mut progress, &mut load, &mut prefetch, None)
}
/// Whether two sheet lists are the same sheets (origin, URL and text).
fn same_sheets(a: &[Sheet], b: &[Sheet]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.1 == y.1 && x.2 == y.2 && x.3 == y.3 && x.0.source() == y.0.source())
}
/// What a resource request returned, for a second gathering pass.
enum Memoed {
    Missing,
    Stylesheet(Arc<css::Syntax>, Option<Arc<str>>),
    Image(Arc<crate::image::Image>),
    Font(Arc<[u8]>),
}
struct Memo {
    entries: std::collections::HashMap<(ResourceKind, Option<String>, String), Memoed>,
    active: bool,
}
impl Memo {
    fn load(&mut self, kind: ResourceKind, base: Option<&str>, href: &str, resource: &mut impl FnMut(ResourceKind, Option<&str>, &str) -> Result<Option<Resource>>) -> Result<Option<Resource>> {
        if !self.active {
            return resource(kind, base, href);
        }
        let key = (kind, base.map(str::to_owned), href.to_owned());
        if let Some(m) = self.entries.get(&key) {
            return Ok(match m {
                Memoed::Missing => None,
                Memoed::Stylesheet(syntax, url) => Some(Resource::Stylesheet(syntax.clone(), url.clone())),
                Memoed::Image(image) => Some(Resource::Image(image.clone())),
                Memoed::Font(bytes) => Some(Resource::Font(bytes.clone())),
            });
        }
        let r = resource(kind, base, href)?;
        let m = match &r {
            None => Memoed::Missing,
            Some(Resource::Stylesheet(syntax, url)) => Memoed::Stylesheet(syntax.clone(), url.clone()),
            Some(Resource::Image(image)) => Memoed::Image(image.clone()),
            Some(Resource::Font(bytes)) => Memoed::Font(bytes.clone()),
        };
        self.entries.insert(key, m);
        Ok(r)
    }
}
/// Lays out the document as scripts see it, for what they measure
/// (getBoundingClientRect, offsetWidth, getComputedStyle): the styles and
/// fonts gathered before the scripts ran.
#[derive(Clone)]
pub struct Measurer {
    rules: Arc<css::cascade::RuleSet>,
    media: css::media::Media,
    media_dependent: bool,
    font_faces: Vec<css::fonts::FontFace>,
    images: Option<Arc<crate::image::Set>>,
    scope: Scope,
    base_scope: Scope,
    quirks: bool,
    sheet_ids: Vec<(crate::dom::NodeId, u64)>,
    base: Option<String>,
    /// The document and revision the sheets were last looked for at.
    checked: Option<(u64, u64)>,
    /// How many pictures the page it measures for was prepared without.
    late: usize,
    /// How its rules changed, compilation to compilation.
    changes: RuleChanges,
    /// The sheets its rules were compiled from (what a change added or
    /// took away, see `document_sheets_reach`).
    sheets: Vec<Sheet>,
    /// The page's own pictures, before any scripts' were bound over them.
    own_images: Option<Arc<crate::image::Set>>,
    /// The scripts' pictures last bound (how many, who they are).
    bound: (usize, usize),
}
impl Measurer {
    /// Follow the live viewport and preferred scheme without reloading the
    /// document's sheets. Media-filtered rules and font faces are selected
    /// again only when their environment changes. Returns whether the host
    /// needs rebuilding for a new font scope.
    pub fn set_environment(
        &mut self,
        viewport: layout::Viewport,
        dark: bool,
        cancel: Option<&AtomicBool>,
        resource: &mut impl FnMut(ResourceKind, Option<&str>, &str) -> Result<Option<Resource>>,
    ) -> Result<bool> {
        let media = css::media::Media { scripting: self.media.scripting, ..css::media::Media::screen(viewport.width, viewport.height, viewport.scale, dark) };
        if media == self.media {
            return Ok(false);
        }
        if !self.media_dependent {
            self.media = media;
            return Ok(false);
        }
        let (rules, dependent, faces, _) = compile(&self.sheets, media, cancel)?;
        let scope = if faces == self.font_faces {
            self.scope.clone()
        } else {
            let (fonts, _) = load_fonts(&faces, self.base.as_deref(), resource, cancel)?;
            if fonts.is_empty() { self.base_scope.clone() } else { scope_with_faces(&self.base_scope, fonts, cancel)? }
        };
        self.changes.record(self.rules.generation(), rules.generation(), None);
        self.rules = rules;
        self.scope = scope;
        self.media = media;
        self.media_dependent = dependent;
        self.font_faces = faces;
        Ok(true)
    }
    /// The pictures scripts loaded into `img` elements, for its layouts to
    /// size their images by (bound again only when they changed). Without
    /// them, an image a script loaded measured as its placeholder: IMDb's
    /// carousel read its slides' pictures 0 tall.
    pub fn bind_pictures(&mut self, doc: &Document, pictures: &[(crate::dom::NodeId, Arc<crate::image::Image>)], cancel: Option<&AtomicBool>) {
        let key = pictures.iter().fold((pictures.len(), 0usize), |(n, h), (node, i)| (n, h.wrapping_add((Arc::as_ptr(i) as usize) ^ node.slot() as usize)));
        // A set is the document's at one revision: after any change the
        // page's own pictures, bound at its preparing, went unused too, and
        // every image measured as its placeholder. Bound again to the
        // document as it is.
        let current = self.images.as_ref().is_none_or(|i| i.validate(doc).is_ok());
        if key == self.bound && current {
            return;
        }
        self.bound = key;
        let base = self.own_images.clone();
        self.images = match base {
            Some(set) if !pictures.is_empty() => set.with_extra(doc, pictures.to_vec(), SCRIPTED_IMAGE_LIMITS, cancel).ok().or(Some(set)),
            Some(set) => set.with_extra(doc, Vec::new(), SCRIPTED_IMAGE_LIMITS, cancel).ok().or(Some(set)),
            None if !pictures.is_empty() => crate::image::Set::new(doc, pictures.to_vec(), SCRIPTED_IMAGE_LIMITS, cancel).ok(),
            None => None,
        };
    }
    /// How many pictures the page was prepared without (still on their
    /// way): a page with none to run still goes live for them.
    pub fn late_images(&self) -> usize {
        self.late
    }
    /// Follow the document's stylesheets: when the style/link elements
    /// changed since (a sheet a script added, removed or rewrote), load
    /// them, compile the rules and fonts again. Returns whether they did
    /// (then a host made for the old scope is stale).
    pub fn refresh(&mut self, doc: &Document, cancel: Option<&AtomicBool>, resource: &mut impl FnMut(ResourceKind, Option<&str>, &str) -> Result<Option<Resource>>) -> Result<bool> {
        // Since the last look, only inline styles and attributes no sheet
        // follows changed: the sheets are as they were, without a walk.
        if sheets_unchanged(doc, self.checked) {
            self.checked = Some((doc.id(), doc.revision()));
            return Ok(false);
        }
        let (ids, base, nodes) = sheet_identities(doc)?;
        self.checked = Some((doc.id(), doc.revision()));
        if crate::env_on!("OLO_SHEET_DEBUG") {
            eprintln!(
                "  refresh: {} sheet elements ({} in shadow trees), {}",
                nodes.len(),
                nodes.iter().filter(|&&n| doc.shadow_scope(n).is_some()).count(),
                if ids == self.sheet_ids { "as before" } else { "changed" }
            );
        }
        if ids == self.sheet_ids {
            return Ok(false);
        }
        let started = std::time::Instant::now();
        let mut sheets = ua_sheets(doc, self.quirks, cancel)?;
        let (loaded, _) = load_sheets(doc, &nodes, base.as_deref(), resource, cancel).inspect_err(|_| self.checked = None)?;
        sheets.extend(loaded);
        let loaded_at = started.elapsed();
        let (rules, dependent, faces, _) = compile(&sheets, self.media, cancel)?;
        let media = self.media;
        self.changes.record(self.rules.generation(), rules.generation(), sheets_changed_within(doc, &self.sheet_ids, &ids, || document_sheets_reach(doc, &self.sheets, &sheets, media)));
        self.sheets = sheets.clone();
        let compiled_at = started.elapsed();
        let (fonts, _) = load_fonts(&faces, base.as_deref(), resource, cancel).inspect_err(|_| self.checked = None)?;
        if crate::env_on!("OLO_WEB_TIMING") {
            eprintln!(
                "  measurer sheets: {} ({} KB) loaded in {:.0} ms, compiled in {:.0} ms, fonts {:.0} ms",
                sheets.len(),
                sheets.iter().map(|s| s.0.source().len()).sum::<usize>() / 1024,
                loaded_at.as_secs_f64() * 1000.,
                (compiled_at - loaded_at).as_secs_f64() * 1000.,
                (started.elapsed() - compiled_at).as_secs_f64() * 1000.
            );
        }
        self.rules = rules;
        self.media_dependent = dependent;
        self.font_faces = faces;
        self.scope = if fonts.is_empty() { self.base_scope.clone() } else { scope_with_faces(&self.base_scope, fonts, cancel)? };
        self.sheet_ids = ids;
        self.base = base;
        Ok(true)
    }
    /// The fonts the page is styled with; a host for measuring is made for it.
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
    pub fn media(&self) -> css::media::Media {
        self.media
    }
    /// The changes since the previous measurement, when they are a few
    /// elements' own declarations (inline styles, attributes no selector
    /// reaches from or tests in a `:has()`): those elements, each with
    /// whether its rules must be matched again. None when the changes
    /// are anything else (or too many): the full incremental job's.
    fn patchable(&self, doc: &Document, revision: u64) -> std::result::Result<Patchable, String> {
        patchable(doc, &self.rules, revision)
    }
    /// Style and lay out `doc` as it is now. The previous measurement is
    /// consumed: when a few elements' own declarations changed and
    /// nothing else, its styles are patched in place and its layout kept
    /// (nothing visited, nothing copied); else its styles seed the
    /// incremental job and its texts the preparation.
    pub fn measure(&self, doc: &Document, host: &impl Host, shaper: &mut shaping::Engine, viewport: layout::Viewport, previous: Option<Measured>, cancel: Option<&AtomicBool>) -> Result<Measured> {
        if host.font_context() != self.scope.context() {
            return Err(style::Error::Stale.into());
        }
        let started = std::time::Instant::now();
        let mut phases = [0f64; 6];
        // The previous measurement's defaults, while the host is the same:
        // styles compare equal only against the same defaults.
        // (Another host, another scope: the previous styles and layout
        // hold fonts from the old one and are not reused.)
        let mut previous = previous.filter(|m| m.defaults.is_current(host));
        // The document the previous layout was of, read before its styles
        // are patched to this one (the patch advances their revision).
        let previous_revision = previous.as_ref().map(|m| m.layout.cache().tree().styles().document_revision());
        // What the previous measurement found (read before a patch in place).
        let previous_record = previous.as_ref().map(|m| m.layout.recorded());
        let mut found: [layout::Fallbacks; 4] = Default::default();
        let mut redone = [layout::Redone::All; 4];
        let defaults = match &previous {
            Some(m) => m.defaults.clone(),
            None => Defaults::new(host, Default::default(), cancel)?,
        };
        let snapshot = interaction::Snapshot::new(doc, Interaction::default(), cancel);
        let options = || style::JobOptions {
            quirks: self.quirks,
            states: Some(&snapshot),
            previous: None,
            generation: self.rules.generation(),
            animated: None,
            containers: None,
            resized: &[],
            changed_within: None,
        };
        // A patch over the previous styles: in place when nothing else
        // holds them, else over a copy.
        let mut patched: Option<(style::Patched, Option<Arc<StyleTree>>)> = None;
        let mut path = String::new();
        // The patch said the text changed: laid out again whatever the styles say.
        let mut relayout = false;
        if let Some(m) = previous.as_mut() {
            match self.patchable(doc, m.layout.cache().tree().styles().document_revision()) {
                Ok(Patchable { nodes, relayout: text_changed }) => {
                    relayout = text_changed;
                    let patch = |tree: &mut StyleTree| style::Job::patch(doc, &self.rules, defaults.clone(), options(), Default::default(), cancel, tree, &nodes, |_| host);
                    patched = match Arc::get_mut(&mut m.layout).and_then(|l| l.patch_styles(doc, patch)) {
                        Some(result) => {
                            path = format!("patched in place, {} elements", nodes.len());
                            result?.map(|p| (p, None))
                        }
                        None => {
                            let mut tree = (**m.layout.cache().tree().styles()).clone();
                            path = format!("patched a copy, {} elements", nodes.len());
                            style::Job::patch(doc, &self.rules, defaults.clone(), options(), Default::default(), cancel, &mut tree, &nodes, |_| host)?.map(|p| (p, Some(Arc::new(tree))))
                        }
                    };
                    if patched.is_none() {
                        path = format!("full job: the patch over {} elements could not stand", nodes.len());
                    }
                }
                Err(why) => path = format!("full job: {why}"),
            }
        } else {
            path = "full job: nothing before".to_owned();
        }
        let styles = match &patched {
            Some((_, Some(copied))) => copied.clone(),
            Some((_, None)) => previous.as_ref().map(|m| m.layout.cache().tree().styles().clone()).ok_or(style::Error::Stale)?,
            None => {
                let previous_styles = previous.as_ref().map(|m| m.layout.cache().tree().styles().as_ref());
                // Rules that differ from the previous styles' in some shadow
                // trees' sheets alone: kept outside them.
                let generation = previous_styles.map(|s| s.generation());
                let hosts = generation.and_then(|g| self.changes.since(g, self.rules.generation()));
                let changed_within = generation.zip(hosts.as_deref());
                let mut job = style::Job::new(doc, &self.rules, defaults.clone(), style::JobOptions { previous: previous_styles, changed_within, ..options() }, Default::default(), cancel)?;
                while !job.step(256, |_| host)?.finished {}
                let styles = job.finish()?;
                check_styles(doc, &self.rules, defaults.clone(), style::JobOptions { previous: previous_styles, changed_within, ..options() }, &styles, host, cancel)?;
                Arc::new(styles)
            }
        };
        phases[0] = started.elapsed().as_secs_f64() * 1000.;
        for _ in 0..styles.report().value_fallbacks.min(u32::MAX as usize) {
            found[0].note("declaration dropped at computed-value time", None);
        }
        if previous.is_some() {
            redone[0] = layout::Redone::Part;
        }
        let issues =
            styles.report().stylesheet_issues + styles.report().inline_unsupported + styles.report().inline_invalid + styles.report().presentation_invalid + styles.report().presentation_unsupported;
        // (Kept only with the same pictures: one a script loaded since
        // changes its image's size.)
        let same_pictures = |p: &Measured| match (p.layout.cache().tree().images(), self.images.as_ref().filter(|i| i.validate(doc).is_ok())) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same_layout(b),
            _ => false,
        };
        let kept = previous.as_ref().filter(|p| styles.geometry_kept() && !relayout && p.viewport == viewport && same_pictures(p));
        let layout = if let Some(p) = kept {
            redone[1] = layout::Redone::Not;
            redone[2] = layout::Redone::Not;
            redone[3] = layout::Redone::Not;
            match &patched {
                // Patched in place: the layout holds the patched styles.
                Some((_, None)) => p.layout.clone(),
                _ => Arc::new(p.layout.with_styles(styles, doc, None)),
            }
        } else {
            let previous = previous.as_ref();
            // The nodes changed since the previous layout, and the elements
            // the patch restyled (in place, the styles compare equal to
            // themselves), with their ancestors.
            let dirty = previous_revision.and_then(|revision| dirty_nodes(doc, revision)).map(|mut d| {
                if let Some((p, _)) = &patched {
                    dirty_ancestors(doc, &p.nodes, &mut d);
                }
                d
            });
            let bound = self.images.as_ref().filter(|images| images.validate(doc).is_ok());
            let in_place = match &patched {
                Some((p, None)) => Some(p.boxes_kept),
                _ => None,
            };
            let kept = match (previous, previous_revision) {
                (Some(p), Some(revision)) => kept_boxes(&p.layout, doc, revision, &styles, in_place, bound),
                _ => None,
            };
            let tree = match kept {
                Some(tree) => {
                    redone[1] = layout::Redone::Not;
                    Arc::new(tree)
                }
                None => {
                    let mut job = layout::BuildJob::new(doc, styles, Default::default(), cancel)?;
                    // The images were bound to the document before the scripts changed
                    // it; while they still fit, boxes take their sizes.
                    if let Some(images) = bound {
                        job = job.with_images(images.clone())?;
                    }
                    while !job.step(256)?.finished {}
                    found[1].merge(job.fallbacks());
                    job.finish()?
                }
            };
            phases[1] = started.elapsed().as_secs_f64() * 1000. - phases[0];
            let mut job = layout::PrepareJob::new(shaper, &self.scope, tree, Default::default(), cancel)?;
            if let Some(p) = previous {
                job = job.with_previous(p.layout.cache());
                redone[2] = layout::Redone::Part;
            }
            if let Some((p, None)) = &patched {
                job = job.with_restyled(&p.nodes);
            }
            while !job.step(4)?.finished {}
            found[2].merge(job.fallbacks());
            let cache = job.finish()?;
            phases[2] = started.elapsed().as_secs_f64() * 1000. - phases[0] - phases[1];
            let checked = cache.clone();
            let mut job = layout::Job::new(shaper, &self.scope, cache, viewport, Default::default(), cancel)?;
            if let (Some(p), Some(dirty)) = (previous, &dirty) {
                job = job.with_previous(&p.layout, dirty);
                redone[3] = layout::Redone::Part;
            }
            while !job.step(256)?.finished {}
            found[3].merge(job.fallbacks());
            let reused = job.progress().reused;
            let layout = job.finish()?;
            check_reuse(shaper, &self.scope, checked, viewport, &layout, doc, reused, cancel)?;
            Arc::new(layout)
        };
        phases[3] = started.elapsed().as_secs_f64() * 1000. - phases[0] - phases[1] - phases[2];
        let stages = std::array::from_fn(|i| (std::mem::take(&mut found[i]), redone[i]));
        layout.set_recorded(layout::Recorded::update(previous_record.as_deref(), stages, (issues, redone[0])));
        // The index stands with the boxes: the same layout, or one with new
        // styles over the same boxes (a paint-only restyle keeps the
        // geometry's identity); apple.com's scrolling rebuilt it 40 times
        // a second.
        let same_boxes = |p: &Measured| {
            Arc::ptr_eq(&p.layout, &layout) || p.layout.geometry_identity() == layout.geometry_identity() && p.layout.cache().tree().boxes().len() == layout.cache().tree().boxes().len()
        };
        let index = match previous.as_mut().filter(|p| same_boxes(p)) {
            Some(p) => std::mem::take(&mut p.index),
            None => {
                let mut index = crate::dom::NodeMap::with_capacity_and_hasher(layout.cache().tree().boxes().len(), Default::default());
                for (id, b) in layout.cache().tree().iter() {
                    if b.marker || b.style_node.pseudo.is_some() {
                        continue;
                    }
                    index.entry(b.node).or_insert(id);
                }
                index
            }
        };
        phases[4] = started.elapsed().as_secs_f64() * 1000. - phases[..4].iter().sum::<f64>();
        phases[5] = started.elapsed().as_secs_f64() * 1000.;
        Ok(Measured { layout, document_size: Default::default(), inlines: Default::default(), text_spans: Default::default(), index, revision: doc.revision(), viewport, phases, defaults, path })
    }
}
/// One layout of the document, queried by node.
pub struct Measured {
    layout: Arc<layout::Layout>,
    document_size: std::sync::OnceLock<(f64, f64)>,
    /// Inline elements' pieces in the paragraphs (text source, source range),
    /// by element: made the first time one is measured.
    inlines: std::sync::OnceLock<(crate::dom::NodeMap<Vec<(u32, std::ops::Range<u32>, bool, bool)>>, Vec<Option<layout::BoxId>>)>,
    /// Each text node's spans in the text sources (source, byte range),
    /// found once (Range.getClientRects).
    text_spans: std::sync::OnceLock<crate::dom::NodeMap<Vec<(u32, std::ops::Range<u32>)>>>,
    index: crate::dom::NodeMap<layout::BoxId>,
    revision: u64,
    viewport: layout::Viewport,
    phases: [f64; 6],
    defaults: Arc<Defaults>,
    /// How it was made (diagnostics): patched, or the full job and why.
    path: String,
}
impl Measured {
    pub fn path(&self) -> &str {
        &self.path
    }
    /// Milliseconds spent styling, building boxes, preparing text, laying
    /// out, indexing, and in all.
    pub fn phases_ms(&self) -> [f64; 6] {
        self.phases
    }
    /// The document revision this was laid out from.
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn layout(&self) -> &Arc<layout::Layout> {
        &self.layout
    }
    pub fn viewport(&self) -> layout::Viewport {
        self.viewport
    }
    fn geometry(&self, node: crate::dom::NodeId) -> Option<(layout::BoxId, layout::Rect, &layout::Geometry)> {
        let id = *self.index.get(&node)?;
        let bounds = self.layout.bounds(id)?;
        Some((id, bounds, self.layout.geometry(id)?))
    }
    /// The border box in page coordinates (getBoundingClientRect at scroll 0);
    /// an inline element's, the union of its pieces on the lines.
    pub fn border_box(&self, node: crate::dom::NodeId) -> Option<layout::Rect> {
        self.geometry(node).map(|(_, r, _)| r).or_else(|| self.inline_box(node))
    }
    /// `border_box`, or for a rendered element with nothing to measure (an
    /// empty inline, a `display: contents` one) the empty rectangle where
    /// it sits, as Chrome gives one: after the nearest earlier sibling with
    /// a box (at its bottom left), else at its parent's content box.
    /// Reddit's feed waits for its next page's (empty) partial to come
    /// near the viewport, which one at (0, 0) and empty never did.
    pub fn placed_box(&self, doc: &Document, node: crate::dom::NodeId) -> Option<layout::Rect> {
        if let Some(r) = self.border_box(node) {
            return Some(r);
        }
        let tree = self.layout.cache().tree();
        if doc.node_type(node).ok()? != NodeType::Element || !doc.is_connected(node).unwrap_or(false) || !displayed(doc, tree.styles(), node) {
            return None;
        }
        self.place_empty(doc, node, 0)
    }
    /// The composed children of `node`, in order.
    fn composed_children(doc: &Document, node: crate::dom::NodeId) -> Vec<crate::dom::NodeId> {
        let mut out = Vec::new();
        let mut at = doc.composed_first_child(node).ok().flatten();
        while let Some(c) = at.filter(|_| out.len() < 100_000) {
            out.push(c);
            at = doc.composed_next_sibling(c).ok().flatten();
        }
        out
    }
    /// The bottom of what `node` shows: its box's, or its last composed
    /// child's with one (through `display: contents`).
    fn shown_bottom(&self, doc: &Document, node: crate::dom::NodeId, depth: usize) -> Option<layout::Rect> {
        if let Some(r) = self.border_box(node) {
            return Some(layout::Rect { x: r.x, y: r.y + r.height, width: 0., height: 0. });
        }
        if depth > 32 || doc.node_type(node).ok()? != NodeType::Element {
            return None;
        }
        Self::composed_children(doc, node).into_iter().rev().find_map(|c| self.shown_bottom(doc, c, depth + 1))
    }
    fn place_empty(&self, doc: &Document, node: crate::dom::NodeId, depth: usize) -> Option<layout::Rect> {
        let parent = doc.composed_parent(node).ok().flatten().or_else(|| doc.shadow_host(node))?;
        let siblings = Self::composed_children(doc, parent);
        let at = siblings.iter().position(|&s| s == node).unwrap_or(siblings.len());
        if let Some(r) = siblings[..at].iter().rev().find_map(|&s| self.shown_bottom(doc, s, depth)) {
            return Some(r);
        }
        if let Some((_, _, g)) = self.geometry(parent) {
            let mut c = g.content_box();
            if let Some(r) = self.border_box(parent) {
                c.x += r.x - g.border_box.x;
                c.y += r.y - g.border_box.y;
            }
            return Some(layout::Rect { x: c.x, y: c.y, width: 0., height: 0. });
        }
        if depth > 32 {
            return None;
        }
        self.place_empty(doc, parent, depth + 1)
    }
    /// An inline element (one with no box of its own): the union of its
    /// text's fragments on their lines, each a line's ascent and descent
    /// tall, as Chrome measures one's content area.
    /// The inline elements' pieces of each text source, and each source's
    /// text box (what its lines are laid out in), found once.
    #[allow(clippy::type_complexity)]
    fn inline_index(&self) -> &(crate::dom::NodeMap<Vec<(u32, std::ops::Range<u32>, bool, bool)>>, Vec<Option<layout::BoxId>>) {
        let tree = self.layout.cache().tree();
        self.inlines.get_or_init(|| {
            let mut map: crate::dom::NodeMap<Vec<(u32, std::ops::Range<u32>, bool, bool)>> = Default::default();
            let mut source = 0u32;
            while tree.text_source(source).is_some() {
                for b in tree.inline_boxes(source).unwrap_or(&[]) {
                    map.entry(b.node).or_default().push((source, b.range.clone(), b.first, b.last));
                }
                source += 1;
            }
            // Each source's text box (what its lines are laid out in).
            let mut boxes = vec![None; source as usize];
            for (id, b) in tree.iter() {
                if let layout::BoxKind::InlineContent { source } = b.kind
                    && let Some(slot) = boxes.get_mut(source as usize)
                {
                    *slot = Some(id);
                }
            }
            (map, boxes)
        })
    }
    fn inline_box(&self, node: crate::dom::NodeId) -> Option<layout::Rect> {
        let mut union: Option<layout::Rect> = None;
        for r in self.inline_rects(node) {
            union = Some(match union {
                None => r,
                Some(u) => {
                    let (x, y) = (u.x.min(r.x), u.y.min(r.y));
                    layout::Rect { x, y, width: (u.x + u.width).max(r.x + r.width) - x, height: (u.y + u.height).max(r.y + r.height) - y }
                }
            });
        }
        union
    }
    /// An inline element's border box on each line it is laid out on, in
    /// page coordinates (getClientRects): its text's content area there,
    /// grown by its padding and borders (at its sides only where it starts
    /// and ends), as Chrome gives them.
    pub fn inline_rects(&self, node: crate::dom::NodeId) -> Vec<layout::Rect> {
        let mut out = Vec::new();
        let (inlines, text_boxes) = self.inline_index();
        let Some(pieces) = inlines.get(&node) else { return out };
        let tree = self.layout.cache().tree();
        let edges = tree.styles().snapshot(node).map_or([0.; 4], |s| {
            let pad = |p| s.value(p).scalar().unwrap_or(0.).max(0.);
            let border = |p| s.used_border_width(p).unwrap_or(0.);
            [
                pad(css::properties::Property::PaddingTop) + border(css::properties::Property::BorderTopWidth),
                pad(css::properties::Property::PaddingRight) + border(css::properties::Property::BorderRightWidth),
                pad(css::properties::Property::PaddingBottom) + border(css::properties::Property::BorderBottomWidth),
                pad(css::properties::Property::PaddingLeft) + border(css::properties::Property::BorderLeftWidth),
            ]
        });
        for (source, range, first, last) in pieces {
            let Some(container) = text_boxes.get(*source as usize).copied().flatten() else { continue };
            let (Some(text), Some(origin)) = (self.layout.text_layout(container), self.layout.bounds(container)) else { continue };
            let content = self.layout.geometry(container).map_or((0., 0.), |g| {
                let c = g.content_box();
                (c.x - g.border_box.x, c.y - g.border_box.y)
            });
            for (i, line) in text.lines().iter().enumerate() {
                let Some(baseline) = text.baseline(i) else { continue };
                let (mut x0, mut x1, mut top, mut bottom) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
                let (mut starts, mut ends) = (false, false);
                for f in &text.fragments()[line.fragments.start as usize..line.fragments.end as usize] {
                    if f.source.end <= range.start || f.source.start >= range.end {
                        continue;
                    }
                    let (l, r) = fragment_part(text, f, range.start, range.end);
                    x0 = x0.min(f.x + l);
                    x1 = x1.max(f.x + r);
                    // (Its font's content area, as Chrome measures an inline.)
                    let (above, below) = text.content_area(f);
                    let y = baseline - text.shift(f) - above;
                    top = top.min(y);
                    bottom = bottom.max(y + above + below);
                    starts |= f.source.start <= range.start;
                    ends |= f.source.end >= range.end;
                }
                if x0 > x1 {
                    continue;
                }
                if starts && *first {
                    x0 -= edges[3];
                }
                if ends && *last {
                    x1 += edges[1];
                }
                out.push(layout::Rect { x: origin.x + content.0 + x0, y: origin.y + content.1 + top - edges[0], width: x1 - x0, height: bottom - top + edges[0] + edges[2] });
            }
        }
        out
    }
    /// Where a text node's characters `start..end` (UTF-16 offsets, as a
    /// Range has them) are laid out: a rectangle a line, in page
    /// coordinates (Range.getClientRects). Where whitespace was collapsed
    /// the node's offsets are placed in proportion along what it became.
    pub fn text_rects(&self, doc: &Document, node: crate::dom::NodeId, start: u32, end: u32) -> Vec<layout::Rect> {
        let mut out = Vec::new();
        let Some(raw) = doc.value(node).ok().flatten() else { return out };
        // UTF-16 offsets to bytes of the node's text.
        let byte_at = |unit: u32| {
            let mut units = 0u32;
            for (i, c) in raw.char_indices() {
                if units >= unit {
                    return i as u32;
                }
                units += c.len_utf16() as u32;
            }
            raw.len() as u32
        };
        let (from, to) = (byte_at(start.min(end)), byte_at(end.max(start)));
        let spans = self.text_spans.get_or_init(|| {
            let tree = self.layout.cache().tree();
            let mut map: crate::dom::NodeMap<Vec<(u32, std::ops::Range<u32>)>> = Default::default();
            let mut source = 0u32;
            while let Some(text_source) = tree.text_source(source) {
                for span in text_source.spans.iter() {
                    map.entry(span.node).or_default().push((source, span.range.clone()));
                }
                source += 1;
            }
            map
        });
        for (source, range) in spans.get(&node).map(Vec::as_slice).unwrap_or(&[]) {
            let Some(container) = self.inline_index().1.get(*source as usize).copied().flatten() else { continue };
            let (Some(text), Some(origin)) = (self.layout.text_layout(container), self.layout.bounds(container)) else { continue };
            let content = self.layout.geometry(container).map_or((0., 0.), |g| {
                let c = g.content_box();
                (c.x - g.border_box.x, c.y - g.border_box.y)
            });
            {
                let span_range = range;
                let len = span_range.end - span_range.start;
                let scale = |b: u32| if raw.len() as u32 == len || raw.is_empty() { b.min(len) } else { ((u64::from(b) * u64::from(len)) / raw.len() as u64) as u32 };
                let (a, z) = (span_range.start + scale(from), span_range.start + scale(to));
                for (i, line) in text.lines().iter().enumerate() {
                    let Some(baseline) = text.baseline(i) else { continue };
                    for f in &text.fragments()[line.fragments.start as usize..line.fragments.end as usize] {
                        let collapsed = a == z;
                        if (!collapsed && (f.source.end <= a || f.source.start >= z)) || (collapsed && (a < f.source.start || a > f.source.end)) {
                            continue;
                        }
                        let (left, right) = fragment_part(text, f, a, z);
                        // (Its font's content area, as Chrome measures text.)
                        let (above, below) = text.content_area(f);
                        let y = origin.y + content.1 + baseline - text.shift(f) - above;
                        out.push(layout::Rect { x: origin.x + content.0 + f.x + left, y, width: right - left, height: above + below });
                    }
                }
            }
        }
        out
    }
    /// An element's rendered text (HTML's innerText): its text as shown,
    /// whitespace collapsed as its style says, text-transform applied, a
    /// line break around each block (two around a paragraph), a tab
    /// between table cells, a newline between rows and for each `<br>`,
    /// nothing of what isn't displayed or is hidden. None when the element
    /// itself isn't rendered (then it is its textContent).
    pub fn inner_text(&self, doc: &Document, node: crate::dom::NodeId) -> Option<String> {
        use css::properties::{Display, Internal, Keyword as K, Outer, Property as P};
        use css::style::Value;
        let styles = self.layout.cache().tree().styles();
        let own = styles.snapshot(node)?;
        if matches!(own.value(P::Display), Value::Display(Display::None)) || !displayed(doc, styles, node) {
            return None;
        }
        enum Item {
            Text(String),
            Break(u8),
        }
        struct Walk<'a> {
            doc: &'a Document,
            styles: &'a style::StyleTree,
            items: Vec<Item>,
            budget: usize,
        }
        impl Walk<'_> {
            fn visit(&mut self, node: crate::dom::NodeId, depth: usize) {
                if depth > 512 || self.budget == 0 {
                    return;
                }
                self.budget -= 1;
                match self.doc.node_type(node) {
                    Ok(crate::dom::NodeType::Text) => {
                        let Some(parent) = self.doc.parent(node).ok().flatten() else { return };
                        let Some(s) = self.styles.snapshot(parent) else { return };
                        if !matches!(s.value(P::Visibility), Value::Keyword(K::Visible)) {
                            return;
                        }
                        let raw = self.doc.value(node).ok().flatten().unwrap_or_default();
                        let mut text = match s.value(P::WhiteSpaceCollapse) {
                            Value::Keyword(K::Preserve | K::BreakSpaces) => format!("\u{1}{raw}"),
                            Value::Keyword(K::PreserveBreaks) => format!("\u{2}{raw}"),
                            _ => format!("\u{3}{raw}"),
                        };
                        match s.value(P::TextTransform) {
                            Value::Keyword(K::Uppercase) => text = text.to_uppercase(),
                            Value::Keyword(K::Lowercase) => text = text.to_lowercase(),
                            _ => {}
                        }
                        self.items.push(Item::Text(text));
                    }
                    Ok(crate::dom::NodeType::Element) => {
                        let Some(s) = self.styles.snapshot(node) else { return };
                        let display = match s.value(P::Display) {
                            Value::Display(d) => *d,
                            _ => return,
                        };
                        if matches!(display, Display::None) {
                            return;
                        }
                        let name = self.doc.name(node).ok().flatten().unwrap_or_default().to_ascii_lowercase();
                        if name == "br" {
                            self.items.push(Item::Text("\u{1}\n".into()));
                            return;
                        }
                        let (before, after) = match display {
                            _ if name == "p" => (2, 2),
                            Display::Box { outer: Outer::Block, .. } | Display::Internal(Internal::TableCaption) => (1, 1),
                            Display::Box { list_item: true, .. } => (1, 1),
                            _ => (0, 0),
                        };
                        if before > 0 {
                            self.items.push(Item::Break(before));
                        }
                        let mut child = self.doc.composed_first_child(node).ok().flatten();
                        while let Some(c) = child {
                            self.visit(c, depth + 1);
                            child = self.doc.composed_next_sibling(c).ok().flatten();
                        }
                        // A cell before another in its row, a row before
                        // another (in its table).
                        let next_element = |mut n: crate::dom::NodeId| loop {
                            match self.doc.next_sibling(n).ok().flatten() {
                                Some(x) if self.doc.node_type(x).ok() == Some(crate::dom::NodeType::Element) => break Some(x),
                                Some(x) => n = x,
                                None => break None,
                            }
                        };
                        match display {
                            Display::Internal(Internal::TableCell) if next_element(node).is_some() => self.items.push(Item::Text("\u{1}\t".into())),
                            Display::Internal(Internal::TableRow) if next_element(node).is_some() => self.items.push(Item::Break(1)),
                            _ => {}
                        }
                        if after > 0 {
                            self.items.push(Item::Break(after));
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut walk = Walk { doc, styles, items: Vec::new(), budget: 200_000 };
        let mut child = doc.composed_first_child(node).ok().flatten();
        while let Some(c) = child {
            walk.visit(c, 1);
            child = doc.composed_next_sibling(c).ok().flatten();
        }
        // Whitespace collapsed across the text (a space kept once, none at a
        // line's start or end), and runs of breaks as their largest.
        let mut out = String::new();
        let (mut pending, mut breaks, mut line_start) = (false, 0u8, true);
        let flush_breaks = |out: &mut String, breaks: &mut u8| {
            if *breaks > 0 && !out.is_empty() {
                for _ in 0..*breaks {
                    out.push('\n');
                }
            }
            *breaks = 0;
        };
        for item in walk.items {
            match item {
                Item::Break(n) => {
                    pending = false;
                    breaks = breaks.max(n);
                    line_start = true;
                }
                Item::Text(text) => {
                    let mut chars = text.chars();
                    let mode = chars.next().unwrap_or('\u{3}');
                    for c in chars {
                        let preserved = mode == '\u{1}' || (mode == '\u{2}' && c == '\n');
                        if preserved {
                            flush_breaks(&mut out, &mut breaks);
                            if pending {
                                out.push(' ');
                                pending = false;
                            }
                            out.push(c);
                            line_start = c == '\n';
                        } else if c.is_ascii_whitespace() {
                            if !line_start {
                                pending = true;
                            }
                        } else {
                            flush_breaks(&mut out, &mut breaks);
                            if pending {
                                out.push(' ');
                                pending = false;
                            }
                            out.push(c);
                            line_start = false;
                        }
                    }
                }
            }
        }
        Some(out)
    }
    /// `rect` (of `node`, in the document's space) where the transforms of
    /// the node and the boxes around it show it: the bounds of its corners
    /// through each (getBoundingClientRect, getClientRects; CSSOM View
    /// §6.1). Unchanged where none is transformed.
    pub fn transformed(&self, doc: &Document, node: crate::dom::NodeId, rect: layout::Rect) -> layout::Rect {
        let styles = self.layout.cache().tree().styles();
        let mut matrix: Option<[f64; 6]> = None;
        let mut at = Some(node);
        for _ in 0..1024 {
            let Some(n) = at else { break };
            if doc.node_type(n).ok() == Some(NodeType::Element)
                && let Some(s) = styles.snapshot(n)
                && !matches!(s.value(css::properties::Property::Transform), css::style::Value::Transforms(t) if t.0.is_empty())
                && let Some(outer) = self.border_box(n)
                && let Some(about) = paint::about_of(s, outer)
            {
                // (Its own first, then each around it: the outermost last.)
                matrix = Some(match matrix {
                    None => about,
                    Some(m) => css::properties::multiply_transforms(about, m),
                });
            }
            at = doc.composed_parent(n).ok().flatten().or_else(|| doc.shadow_host(n));
        }
        let Some(m) = matrix else { return rect };
        let point = |x: f64, y: f64| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
        let corners = [point(rect.x, rect.y), point(rect.x + rect.width, rect.y), point(rect.x, rect.y + rect.height), point(rect.x + rect.width, rect.y + rect.height)];
        let (x0, x1) = corners.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), c| (a.min(c.0), b.max(c.0)));
        let (y0, y1) = corners.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), c| (a.min(c.1), b.max(c.1)));
        if !(x0.is_finite() && x1.is_finite() && y0.is_finite() && y1.is_finite()) {
            return rect;
        }
        layout::Rect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 }
    }
    /// The padding box (clientWidth/clientHeight, clientTop/clientLeft).
    pub fn padding_box(&self, node: crate::dom::NodeId) -> Option<layout::Rect> {
        let (_, r, g) = self.geometry(node)?;
        Some(layout::Rect { x: r.x + g.border_width.left, y: r.y + g.border_width.top, width: (r.width - g.border_width.horizontal()).max(0.), height: (r.height - g.border_width.vertical()).max(0.) })
    }
    /// How far the box's content reaches (scrollWidth/scrollHeight): the
    /// padding box, extended by descendants that overflow it.
    pub fn scroll_size(&self, node: crate::dom::NodeId) -> Option<(f64, f64)> {
        let (id, r, g) = self.geometry(node)?;
        let padding = self.padding_box(node)?;
        let mut right = padding.x + padding.width;
        let mut bottom = padding.y + padding.height;
        let tree = self.layout.cache().tree();
        let mut stack = vec![id];
        let mut seen = 0usize;
        while let Some(at) = stack.pop() {
            seen += 1;
            if seen > 1 << 16 {
                break;
            }
            let Some(b) = tree.get(at) else { continue };
            if at != id
                && let Some(bounds) = self.layout.bounds(at)
            {
                right = right.max(bounds.x + bounds.width);
                bottom = bottom.max(bounds.y + bounds.height);
            }
            let mut child = b.first_child;
            while let Some(c) = child {
                stack.push(c);
                child = tree.get(c).and_then(|n| n.next_sibling);
            }
        }
        let _ = (r, g);
        Some(((right - padding.x).max(0.), (bottom - padding.y).max(0.)))
    }
    /// The page's scrollable size (`documentElement.scrollWidth/Height`):
    /// as far as its boxes reach that no ancestor clips and that aren't
    /// fixed, margins included, painted or not, and at least the viewport;
    /// worked out once per layout.
    pub fn document_size(&self) -> (f64, f64) {
        *self.document_size.get_or_init(|| {
            use css::{
                properties::{Keyword as K, Property as P},
                style::Value,
            };
            let tree = self.layout.cache().tree();
            let n = tree.boxes().len();
            // Per box: whether it is out of the page's overflow (an ancestor
            // clips it, or it or one is fixed); and whether it clips what it
            // holds.
            let mut out = vec![0u8; n];
            let mut clips = vec![0u8; n];
            let viewport_source = tree.viewport_overflow().source;
            // Each box's coordinate origin (its parent's content box on the
            // page), carried down: boxes are in preorder, parents first, so
            // one pass places them all (`bounds` walks the ancestors of one).
            let mut origin = vec![(0f64, 0f64); n];
            let (mut right, mut bottom) = (self.viewport.width, self.viewport.height);
            for (id, b) in tree.iter() {
                let i = id.index();
                if let Some(p) = b.parent
                    && let Some(pg) = self.layout.geometry(p)
                {
                    let c = pg.content_box();
                    let at = origin[p.index()];
                    origin[i] = (at.0 + c.x, at.1 + c.y);
                }
                let style = tree.style(id);
                let fixed = style.is_some_and(|s| matches!(s.value(P::Position), Value::Keyword(K::Fixed)));
                out[i] = if fixed { 3 } else { b.parent.map_or(0, |p| out[p.index()] | clips[p.index()]) };
                // Only the element whose overflow propagates to the
                // viewport is exempt. A body's own scroll container clips
                // normally when the root already owns viewport overflow.
                // `clip visible` clips one axis without clipping the other.
                clips[i] = if viewport_source == Some(b.node) {
                    0
                } else {
                    style.map_or(0, |s| u8::from(!matches!(s.value(P::OverflowX), Value::Keyword(K::Visible))) | (u8::from(!matches!(s.value(P::OverflowY), Value::Keyword(K::Visible))) << 1))
                };
                if out[i] == 3 || b.parent.is_none() {
                    continue;
                }
                if let Some(g) = self.layout.geometry(id) {
                    let r = g.border_box;
                    if out[i] & 1 == 0 {
                        right = right.max(origin[i].0 + r.x + r.width + g.margin.right.max(0.));
                    }
                    if out[i] & 2 == 0 {
                        bottom = bottom.max(origin[i].1 + r.y + r.height + g.margin.bottom.max(0.));
                    }
                }
            }
            (right, bottom)
        })
    }
    /// A computed (resolved) value as getComputedStyle gives it, for the
    /// properties scripts read: keywords, lengths in px, colours, display,
    /// and the box's used sizes and edges.
    pub fn computed(&self, node: crate::dom::NodeId, name: &str) -> Option<String> {
        self.computed_pseudo(node, None, name)
    }
    /// Whether `node` has the pseudo-element `pseudo` styled (one that
    /// generates content: `::before` with a `content`).
    pub fn has_pseudo(&self, node: crate::dom::NodeId, pseudo: css::selectors::PseudoElement) -> bool {
        self.layout.cache().tree().styles().snapshot(css::style::StyleKey::pseudo(node, pseudo)).is_some()
    }
    /// `computed` for a pseudo-element of `node` (`getComputedStyle(el,
    /// '::before')`): its own style, and its box's sizes when it has one.
    pub fn computed_pseudo(&self, node: crate::dom::NodeId, pseudo: Option<css::selectors::PseudoElement>, name: &str) -> Option<String> {
        use css::properties::{Keyword as K, Name, Property as P};
        use css::style::Value;
        let tree = self.layout.cache().tree();
        let Some(style) = tree.styles().snapshot(css::style::StyleKey { node, pseudo }) else {
            // A pseudo-element nothing styles (no `content`): what it
            // inherits from its element, as Chrome answers (the rest its
            // initial values, which the caller knows).
            let Name::Longhand(p) = Name::parse(name)? else { return None };
            return (pseudo.is_some() && p.inherited()).then(|| computed_value(tree.styles().snapshot(node)?, p)).flatten();
        };
        // A custom property: its computed value as spelled, substituted
        // (a page's gradient reads its colours from `--gradientColorOne`).
        if name.starts_with("--") {
            return style.custom(name).and_then(|c| c.value().map(|v| v.source_spelling().trim().to_owned()));
        }
        let Name::Longhand(p) = Name::parse(name)? else { return None };
        let g = match pseudo {
            None => self.geometry(node),
            // A pseudo-element's box: a child of its element's.
            Some(pseudo) => self.index.get(&node).and_then(|&id| {
                let key = css::style::StyleKey::pseudo(node, pseudo);
                let id = std::iter::successors(tree.get(id)?.first_child, |&c| tree.get(c)?.next_sibling).find(|&c| tree.get(c).is_some_and(|b| b.style_node == key))?;
                Some((id, self.layout.bounds(id)?, self.layout.geometry(id)?))
            }),
        };
        let px = |v: f64| format!("{}px", trim(v));
        let used = match p {
            P::Width | P::Height => g.map(|(_, r, g)| {
                let border_box = matches!(style.value(P::BoxSizing), Value::Keyword(K::BorderBox));
                let c = g.content_box();
                if p == P::Width {
                    if border_box { r.width } else { c.width }
                } else if border_box {
                    r.height
                } else {
                    c.height
                }
            }),
            // A margin given as a length is that length, as Chrome reports
            // it: not the room an over-constrained block's right margin
            // takes up (a 300px canvas in a 600px box has a 0px right
            // margin, not 300px; Chart.js sized its charts down by it).
            // `auto` and percentages, the used value.
            P::MarginTop | P::MarginRight | P::MarginBottom | P::MarginLeft
                if let Value::Numeric(n) = style.value(p)
                    && let Some(q) = n.expression().constant()
                    && q.ty.kind() == Some(css::math::Dimension::Length) =>
            {
                Some(q.value)
            }
            P::MarginTop => g.map(|(_, _, g)| g.margin.top),
            P::MarginRight => g.map(|(_, _, g)| g.margin.right),
            P::MarginBottom => g.map(|(_, _, g)| g.margin.bottom),
            P::MarginLeft => g.map(|(_, _, g)| g.margin.left),
            P::PaddingTop => g.map(|(_, _, g)| g.padding.top),
            P::PaddingRight => g.map(|(_, _, g)| g.padding.right),
            P::PaddingBottom => g.map(|(_, _, g)| g.padding.bottom),
            P::PaddingLeft => g.map(|(_, _, g)| g.padding.left),
            P::BorderTopWidth => g.map(|(_, _, g)| g.border_width.top),
            P::BorderRightWidth => g.map(|(_, _, g)| g.border_width.right),
            P::BorderBottomWidth => g.map(|(_, _, g)| g.border_width.bottom),
            P::BorderLeftWidth => g.map(|(_, _, g)| g.border_width.left),
            _ => None,
        };
        if let Some(v) = used {
            return Some(px(v));
        }
        // A transform resolves to its matrix, its percentages against the
        // border box (CSSOM's resolved value; Chrome reports matrix(...),
        // and apple.com's scripts read the numbers out of it: they made
        // matrix(NaN, ...) of Olo's translate(30px, 0px)).
        if p == P::Transform
            && let Value::Transforms(t) = style.value(P::Transform)
        {
            if t.0.is_empty() {
                return Some("none".into());
            }
            let (w, h) = g.map_or((0., 0.), |(_, r, _)| (r.width, r.height));
            let m = t.matrix(w, h);
            return Some(format!("matrix({})", m.iter().map(|v| trim(if v.abs() < 1e-12 { 0. } else { *v })).collect::<Vec<_>>().join(", ")));
        }
        computed_value(style, p)
    }
}
/// A property's computed value as getComputedStyle serialises it, from the
/// style alone (no box sizes): keywords, lengths in px, colours, families
/// and display.
pub fn computed_value(style: &css::style::Style, p: css::properties::Property) -> Option<String> {
    use css::properties::{Display, Inner, Outer, Property as P};
    use css::style::Value;
    let px = |v: f64| format!("{}px", trim(v));
    {
        Some(match style.value(p) {
            Value::Keyword(k) => k.as_str().to_owned(),
            Value::Numeric(n) if n.expression().constant().is_none_or(|q| q.ty.kind() == Some(css::math::Dimension::Percentage)) => numeric_text(n)?,
            Value::Content(c) => c.css_text(),
            Value::Transforms(t) if t.0.is_empty() => "none".into(),
            Value::Transforms(t) => {
                t.0.iter()
                    .map(|f| {
                        use css::properties::TransformFunction as F;
                        Some(match f {
                            F::Translate(x, y) => format!("translate({}, {})", numeric_text(x)?, numeric_text(y)?),
                            F::Scale(x, y) => format!("scale({}, {})", trim(*x), trim(*y)),
                            F::Rotate(a) => format!("rotate({}deg)", trim(a.expression().constant()?.value)),
                            F::Skew(x, y) => format!("skew({}deg, {}deg)", trim(x.expression().constant()?.value), trim(y.expression().constant()?.value)),
                            F::Matrix(m) => format!("matrix({})", m.iter().map(|v| trim(*v)).collect::<Vec<_>>().join(", ")),
                        })
                    })
                    .collect::<Option<Vec<_>>>()?
                    .join(" ")
            }
            value @ Value::Numeric(_) => {
                let v = value.scalar()?;
                match p {
                    P::Opacity | P::ZIndex | P::FlexGrow | P::FlexShrink | P::Order | P::FontWeight | P::FillOpacity | P::StrokeOpacity | P::StopOpacity | P::FloodOpacity => trim(v),
                    // A number (1.5) resolves against the font size, as Chrome reports it.
                    P::LineHeight => px(style.line_height()),
                    _ => px(v),
                }
            }
            // clip's rect(), as Chrome spells it: commas, auto kept.
            Value::Inset(inset) if inset.clip_rect().is_some() => {
                let autos = inset.clip_rect()?;
                let sides = inset.offsets().into_iter().zip(autos).map(|(n, auto)| if auto { Some("auto".to_owned()) } else { numeric_text(n) }).collect::<Option<Vec<_>>>()?;
                format!("rect({})", sides.join(", "))
            }
            Value::Color(_) => {
                let c = style.used_color(p).ok()?.srgb8_clipped();
                if c[3] == 255 { format!("rgb({}, {}, {})", c[0], c[1], c[2]) } else { format!("rgba({}, {}, {}, {})", c[0], c[1], c[2], trim(f64::from(c[3]) / 255.)) }
            }
            // Families as Chrome spells them: names with spaces quoted.
            Value::Families(f) => f
                .as_slice()
                .iter()
                .map(|f| match f {
                    css::properties::Family::Generic(g) => g.as_str().to_owned(),
                    css::properties::Family::Named(n) if n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') && !n.starts_with(|c: char| c.is_ascii_digit()) => n.to_string(),
                    css::properties::Family::Named(n) => format!("\"{}\"", n.replace('\\', "\\\\").replace('"', "\\\"")),
                })
                .collect::<Vec<_>>()
                .join(", "),
            Value::Display(d) => match d {
                Display::None => "none".into(),
                Display::Contents => "contents".into(),
                Display::Internal(i) => format!("{i:?}").chars().fold(String::new(), |mut out, ch| {
                    if ch.is_ascii_uppercase() {
                        if !out.is_empty() {
                            out.push('-');
                        }
                        out.push(ch.to_ascii_lowercase());
                    } else {
                        out.push(ch);
                    }
                    out
                }),
                Display::Box { list_item: true, .. } => "list-item".into(),
                Display::Box { outer, inner, .. } => match (outer, inner) {
                    (Outer::Block, Inner::Flow) => "block",
                    (Outer::Block, Inner::FlowRoot) => "flow-root",
                    (Outer::Block, Inner::Flex) => "flex",
                    (Outer::Block, Inner::Grid) => "grid",
                    (Outer::Block, Inner::Table) => "table",
                    (Outer::Block, Inner::Ruby) => "block ruby",
                    (Outer::Inline, Inner::Flow) => "inline",
                    (Outer::Inline, Inner::FlowRoot) => "inline-block",
                    (Outer::Inline, Inner::Flex) => "inline-flex",
                    (Outer::Inline, Inner::Grid) => "inline-grid",
                    (Outer::Inline, Inner::Table) => "inline-table",
                    (Outer::Inline, Inner::Ruby) => "ruby",
                    (Outer::RunIn, _) => "run-in",
                }
                .into(),
            },
            _ => return None,
        })
    }
}
/// A computed length-percentage as text: `12px`, `50%`, or
/// `calc(50% + 12px)` (its percentage and length parts found by resolving
/// it against two bases: it is linear in the percentage).
fn numeric_text(n: &css::properties::Numeric) -> Option<String> {
    struct Basis(f64);
    impl css::math::Context for Basis {
        fn factor(&self, unit: css::math::RelativeUnit) -> css::math::Result<f64> {
            if unit == css::math::RelativeUnit::Percentage { Ok(self.0 / 100.) } else { Err(css::math::Error::ContextUnavailable) }
        }
    }
    if let Some(q) = n.expression().constant() {
        return Some(match q.ty.kind() {
            Some(css::math::Dimension::Percentage) => format!("{}%", trim(q.value)),
            Some(css::math::Dimension::Number) => trim(q.value),
            _ => format!("{}px", trim(q.value)),
        });
    }
    let mut scratch = css::math::Scratch::default();
    let at = |b: f64, scratch: &mut css::math::Scratch| n.resolve(&Basis(b), scratch, Default::default(), None).ok();
    let length = at(0., &mut scratch)?;
    let percent = at(100., &mut scratch)? - length;
    Some(match (percent == 0., length == 0.) {
        (true, _) => format!("{}px", trim(length)),
        (false, true) => format!("{}%", trim(percent)),
        (false, false) => format!("calc({}% + {}px)", trim(percent), trim(length)),
    })
}
/// A number as CSS serialises it: no trailing zeros, at most 3 decimals.
fn trim(v: f64) -> String {
    let r = (v * 1000.).round() / 1000.;
    if r == r.trunc() { format!("{}", r as i64) } else { format!("{r}") }
}
/// The sheets of these `style` and `link` elements, in order, loaded
/// through `resource` (an external one by its href against `base`).
///
/// A sheet linked again later (React apps insert the same one several
/// times) is kept only at its last place, where it wins anyway; when it
/// names cascade layers, its first place keeps just those names (one
/// `@layer` statement), whose order that place sets. The sheets are
/// held to `CSS_BYTES` in all: one past it is left out, counted as an
/// issue, and the page renders with the rest, as a browser renders a page
/// whose sheet failed to load.
/// A sheet, its origin, the URL it came from, and the shadow tree it
/// belongs to (`selectors::scope_key`; None: the document's).
pub type Sheet = (Arc<css::Syntax>, Origin, Option<Arc<str>>, Option<u64>);
// Within the cascade's work budget (16 M, a unit a source byte), with room
// for the rules and declarations it ticks too.
const CSS_BYTES: usize = 8 * 1024 * 1024;
/// What the user agent's sheet adds in quirks mode (HTML §15.3.6 and the
/// rendering section's quirks): a form's bottom margin, tables taking
/// their fonts afresh, fields sized by their border box, aligned images'
/// margins.
const UA_QUIRKS: &str = concat!(
    "form{margin-block-end:1em}",
    "table{font-weight:initial;font-style:initial;font-size:initial;line-height:initial;white-space:initial;text-align:initial}",
    "input:not([type=image i]),textarea{box-sizing:border-box}",
    // (Their default sizes, as Chrome's intrinsic ones, still hold their
    // padding and border inside: the UA's widths are of the content box.)
    "input:not([type=checkbox i],[type=radio i],[type=image i],[type=submit i],[type=button i],[type=reset i],[type=hidden i]){width:185px}textarea{width:201px;height:42px}",
    "img[align=left i]{margin-right:3px}img[align=right i]{margin-left:3px}",
);
/// What a host's resource answers for one it is still fetching (without
/// waiting for it): the sheets are looked at again at the next render.
pub const PENDING: &str = "Resource still loading";
/// A style sheet's text parsed, once: the same text again (a `<style>`
/// or linked sheet at the next refresh, the user agent's, the second
/// gather's) is the syntax already made for it, while anything holds it.
pub fn parsed(text: Arc<str>, cancel: Option<&AtomicBool>) -> Result<Arc<css::Syntax>> {
    use std::{
        hash::BuildHasher,
        sync::{Mutex, Weak},
    };
    // By the text's hash (checked against the text): the sheets, and how
    // many entries before the dead ones are dropped.
    static PARSED: Mutex<Option<(std::collections::HashMap<u64, Weak<css::Syntax>>, usize)>> = Mutex::new(None);
    let key = foldhash::quality::FixedState::with_seed(0x01_0c55).hash_one(&*text);
    if let Some((map, _)) = PARSED.lock().unwrap().as_ref()
        && let Some(syntax) = map.get(&key).and_then(Weak::upgrade)
        && syntax.source() == &*text
    {
        return Ok(syntax);
    }
    let syntax = Arc::new(css::Syntax::parse(text, Default::default(), cancel)?);
    let mut guard = PARSED.lock().unwrap();
    let (map, limit) = guard.get_or_insert_with(|| (Default::default(), 256));
    if map.len() >= *limit {
        map.retain(|_, w| w.strong_count() > 0);
        *limit = (map.len() * 2).max(256);
    }
    map.insert(key, Arc::downgrade(&syntax));
    Ok(syntax)
}
/// The user agent's sheet, and the page's `<meta name=color-scheme>` as
/// its root's `color-scheme` when its own sheets give none (HTML §4.2.5.4).
fn ua_sheets(doc: &Document, quirks: bool, cancel: Option<&AtomicBool>) -> Result<Vec<Sheet>> {
    let mut sheets = vec![(parsed(Arc::from(UA), cancel)?, Origin::UserAgent, None, None)];
    if quirks {
        sheets.push((parsed(Arc::from(UA_QUIRKS), cancel)?, Origin::UserAgent, None, None));
    }
    // The first such meta in the head (the document's first children).
    let mut stack: Vec<crate::dom::NodeId> = doc.children(doc.root())?.collect();
    let mut seen = 0;
    while let Some(n) = stack.pop() {
        seen += 1;
        if seen > 4096 {
            break;
        }
        match doc.name(n)? {
            Some("meta") if doc.attribute(n, "name")?.is_some_and(|v| v.trim().eq_ignore_ascii_case("color-scheme")) => {
                let content: String = doc.attribute(n, "content")?.unwrap_or("").chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_')).collect();
                if !content.trim().is_empty() {
                    sheets.push((parsed(Arc::from(format!(":root{{color-scheme:{content}}}")), cancel)?, Origin::UserAgent, None, None));
                }
                break;
            }
            Some("html" | "head") => {
                let mut kids: Vec<_> = doc.children(n)?.collect();
                kids.reverse();
                stack.extend(kids);
            }
            _ => {}
        }
    }
    Ok(sheets)
}
/// The sheets a sheet `@import`s (CSS Cascade 5 §2), each before what
/// imports it and under the import's `layer`, `supports()` and media
/// conditions, fetched relative to the importing sheet: at most 4 deep and
/// 32 in all, within the page's CSS bytes.
#[allow(clippy::too_many_arguments)]
fn imported(
    syntax: &css::Syntax,
    url: Option<&str>,
    base: Option<&str>,
    resource: &mut impl FnMut(ResourceKind, Option<&str>, &str) -> Result<Option<Resource>>,
    depth: usize,
    budget: &mut (usize, usize, usize),
    cancel: Option<&AtomicBool>,
) -> Result<Vec<(Arc<css::Syntax>, Option<Arc<str>>)>> {
    let mut out = Vec::new();
    if depth >= 4 || !syntax.source().contains("@import") {
        return Ok(out);
    }
    let Ok(rules) = syntax.stylesheet(cancel) else { return Ok(out) };
    for rule in rules {
        let name = rule.name().unwrap_or("");
        if rule.kind() != css::StatementKind::AtRule {
            break;
        }
        if name.eq_ignore_ascii_case("charset") || name.eq_ignore_ascii_case("layer") && rule.block().is_none() {
            continue;
        }
        if !name.eq_ignore_ascii_case("import") {
            break;
        }
        let parts: Vec<_> = rule.value().filter(|c| c.kind() != css::Kind::Whitespace).collect();
        let Some(first) = parts.first() else { continue };
        let href = match first.kind() {
            css::Kind::Url | css::Kind::String => first.text(),
            css::Kind::Function if first.text().is_some_and(|t| t.eq_ignore_ascii_case("url")) => first.children().find(|c| c.kind() == css::Kind::String).and_then(|c| c.text()),
            _ => None,
        };
        let Some(href) = href else { continue };
        // The conditions after the URL, as written.
        let (mut layer, mut supports, mut media) = (None::<String>, None::<String>, String::new());
        let source = syntax.source();
        for c in &parts[1..] {
            let text = &source[c.span().start as usize..c.span().end as usize];
            match (c.kind(), c.text()) {
                (css::Kind::Ident, Some(t)) if t.eq_ignore_ascii_case("layer") && media.is_empty() => layer = Some(String::new()),
                (css::Kind::Function, Some(t)) if t.eq_ignore_ascii_case("layer") && media.is_empty() => layer = Some(text[6..text.len().saturating_sub(1)].trim().to_owned()),
                (css::Kind::Function, Some(t)) if t.eq_ignore_ascii_case("supports") && media.is_empty() => supports = Some(text[9..text.len().saturating_sub(1)].trim().to_owned()),
                _ => {
                    if !media.is_empty() {
                        media.push(' ');
                    }
                    media.push_str(text);
                }
            }
        }
        if let Some(condition) = &supports {
            // `supports(display: grid)` is `@supports (display: grid)`.
            let condition = if condition.contains(':') && !condition.starts_with('(') { format!("({condition})") } else { condition.clone() };
            if !css::media::supports(css::Syntax::parse(Arc::from(condition.as_str()), Default::default(), cancel)?.components(), css::properties::Limits::default(), cancel) {
                continue;
            }
        }
        if budget.0 >= 32 {
            budget.2 += 1;
            continue;
        }
        budget.0 += 1;
        let Some(Resource::Stylesheet(sheet, sheet_url)) = resource(ResourceKind::Stylesheet, url.or(base), href)? else {
            budget.2 += 1;
            continue;
        };
        if budget.1.saturating_add(sheet.source().len()) > CSS_BYTES {
            budget.2 += 1;
            continue;
        }
        budget.1 += sheet.source().len();
        let mut nested = imported(&sheet, sheet_url.as_deref(), base, resource, depth + 1, budget, cancel)?;
        nested.push((sheet, sheet_url));
        for (sheet, sheet_url) in nested {
            let mut text = sheet.source().to_string();
            if let Some(name) = &layer {
                text = format!("@layer {name} {{\n{text}\n}}");
            }
            if !media.is_empty() && !media.eq_ignore_ascii_case("all") {
                text = under_media(&media, &text);
            }
            let sheet = if text.len() == sheet.source().len() { sheet } else { parsed(Arc::from(text), cancel)? };
            out.push((sheet, sheet_url));
        }
    }
    Ok(out)
}
/// A sheet's rules under `@media <media>`: what a `media` attribute means.
/// (Its `@import` and `@namespace` rules can't be nested; it has none.)
fn under_media(media: &str, text: &str) -> String {
    let mut out = String::with_capacity(text.len() + media.len() + 12);
    out.push_str("@media ");
    // A media list can't close the block early: braces leave it.
    out.extend(media.chars().filter(|c| !matches!(c, '{' | '}' | ';')));
    out.push_str(" {\n");
    out.push_str(text);
    out.push_str("\n}");
    out
}
/// Whether a sheet's text has a digest its `integrity` names (SRI §3.3.5):
/// of the strongest algorithm listed, any one matching.
fn integrity_matches(text: &str, metadata: &str) -> bool {
    use sha2::Digest;
    let listed: Vec<(u8, &str)> = metadata
        .split_ascii_whitespace()
        .filter_map(|item| {
            let (alg, rest) = item.split_once('-')?;
            let digest = rest.split('?').next()?;
            let strength = match alg.to_ascii_lowercase().as_str() {
                "sha256" => 1,
                "sha384" => 2,
                "sha512" => 3,
                _ => return None,
            };
            Some((strength, digest))
        })
        .collect();
    // Nothing it can check: as if there were no metadata.
    let Some(strongest) = listed.iter().map(|(s, _)| *s).max() else { return true };
    // The bytes as served: the decoded text, with and without a byte order mark.
    let bom = format!("\u{feff}{text}");
    [text.as_bytes(), bom.as_bytes()].iter().any(|bytes| {
        let digest = match strongest {
            1 => sha2::Sha256::digest(bytes).to_vec(),
            2 => sha2::Sha384::digest(bytes).to_vec(),
            _ => sha2::Sha512::digest(bytes).to_vec(),
        };
        // (base64 or its URL-safe form, padded or not)
        let plain = |d: &str| d.trim_end_matches('=').replace('-', "+").replace('_', "/");
        let encoded = plain(&base64(&digest));
        listed.iter().any(|(s, d)| *s == strongest && plain(d) == encoded)
    })
}
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}
fn load_sheets(
    doc: &Document,
    style_nodes: &[crate::dom::NodeId],
    base: Option<&str>,
    resource: &mut impl FnMut(ResourceKind, Option<&str>, &str) -> Result<Option<Resource>>,
    cancel: Option<&AtomicBool>,
) -> Result<(Vec<Sheet>, usize)> {
    enum Item<'a> {
        /// A `<style>`, and its `media` when it has one.
        Inline(crate::dom::NodeId, Option<&'a str>),
        /// A linked sheet: its href, `media` and `integrity`.
        Link(&'a str, Option<&'a str>, Option<&'a str>),
    }
    // A sheet for some media only (`media="screen"`, a query): its rules
    // under that `@media`, so they hold where it does, as the medium changes.
    fn conditioned(media: Option<&str>) -> Option<&str> {
        media.map(str::trim).filter(|m| !m.is_empty() && !m.eq_ignore_ascii_case("all"))
    }
    let mut items = Vec::with_capacity(style_nodes.len());
    let mut resource_issues = 0usize;
    // A shadow tree's sheets (after the document's, each where it is):
    // its rules for its own tree, its host and what is slotted into it.
    let (shadow_nodes, style_nodes): (Vec<crate::dom::NodeId>, Vec<crate::dom::NodeId>) = style_nodes.iter().partition(|&&n| doc.shadow_scope(n).is_some());
    for &node in &style_nodes {
        match doc.name(node)? {
            Some("style") => {
                if doc.attribute(node, "type")?.is_some_and(|t| !t.trim().is_empty() && !t.trim().eq_ignore_ascii_case("text/css")) {
                    continue;
                }
                items.push(Item::Inline(node, conditioned(doc.attribute(node, "media")?)));
            }
            Some("link") if doc.attribute(node, "rel")?.is_some_and(|r| r.split_ascii_whitespace().any(|p| p.eq_ignore_ascii_case("stylesheet"))) => {
                let rel = doc.attribute(node, "rel")?.unwrap_or("");
                let kind = doc.attribute(node, "type")?.unwrap_or("").trim();
                if rel.split_ascii_whitespace().any(|p| p.eq_ignore_ascii_case("alternate")) || doc.attribute(node, "disabled")?.is_some() {
                    // Alternate and disabled sheets are not initially active.
                } else if !kind.is_empty() && !kind.eq_ignore_ascii_case("text/css") {
                    // Not CSS: not a style sheet, as in browsers.
                } else if let Some(href) = doc.attribute(node, "href")? {
                    let integrity = doc.attribute(node, "integrity")?.map(str::trim).filter(|v| !v.is_empty());
                    items.push(Item::Link(href, conditioned(doc.attribute(node, "media")?), integrity));
                }
            }
            _ => {}
        }
    }
    // Where each href is linked last.
    let mut last = std::collections::HashMap::new();
    for (i, item) in items.iter().enumerate() {
        if let Item::Link(href, ..) = item {
            last.insert(href.trim(), i);
        }
    }
    // The sheet at an href's last place, loaded first so an earlier copy
    // can be checked for layers without loading it again.
    let mut loaded: Vec<Option<Option<Resource>>> = (0..items.len()).map(|_| None).collect();
    // A repeated sheet's layer order, by href, for its first place.
    let mut layered = std::collections::HashMap::new();
    for (i, item) in items.iter().enumerate() {
        if let Item::Link(href, ..) = item
            && last.get(href.trim()) == Some(&i)
            && items[..i].iter().any(|e| matches!(e, Item::Link(h, ..) if h.trim() == href.trim()))
        {
            let r = resource(ResourceKind::Stylesheet, base, href)?;
            if let Some(Resource::Stylesheet(syntax, url)) = &r
                && syntax.source().contains("@layer")
                && let Some(statement) = css::cascade::layer_statement(syntax, cancel)
            {
                layered.insert(href.trim(), (statement, url.clone()));
            }
            loaded[i] = Some(r);
        }
    }
    let mut first_seen = std::collections::HashSet::new();
    let mut sheets = Vec::new();
    let mut css_bytes = 0usize;
    let mut imports = 0usize;
    for (i, item) in items.iter().enumerate() {
        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
            return Err(paint::Error::Cancelled.into());
        }
        match item {
            Item::Inline(node, media) => {
                let mut bytes = 0usize;
                for child in doc.children(*node)? {
                    bytes = bytes.saturating_add(doc.value(child)?.map_or(0, str::len));
                }
                if css_bytes.saturating_add(bytes) > CSS_BYTES {
                    resource_issues += 1;
                    continue;
                }
                css_bytes += bytes;
                let mut text = String::with_capacity(bytes);
                for child in doc.children(*node)? {
                    if let Some(value) = doc.value(child)? {
                        text.push_str(value)
                    }
                }
                let syntax = parsed(Arc::from(text), cancel)?;
                let mut budget = (imports, css_bytes, 0);
                let mut list = imported(&syntax, None, base, resource, 0, &mut budget, cancel)?;
                (imports, css_bytes) = (budget.0, budget.1);
                resource_issues += budget.2;
                list.push((syntax, None));
                for (syntax, url) in list {
                    let syntax = match media {
                        Some(m) => parsed(Arc::from(under_media(m, syntax.source())), cancel)?,
                        None => syntax,
                    };
                    sheets.push((syntax, Origin::Author, url, None));
                }
            }
            Item::Link(href, media, integrity) => {
                if last.get(href.trim()) != Some(&i) {
                    // Linked again later, where it wins: here only the layers
                    // it names, at their first place.
                    if let Some((statement, url)) = layered.get(href.trim())
                        && first_seen.insert(href.trim())
                    {
                        sheets.push((parsed(Arc::from(statement.as_str()), cancel)?, Origin::Author, url.clone(), None));
                    }
                    continue;
                }
                let r = match loaded[i].take() {
                    Some(r) => r,
                    None => resource(ResourceKind::Stylesheet, base, href)?,
                };
                match r {
                    Some(Resource::Stylesheet(syntax, url)) => {
                        let bytes = syntax.source().len();
                        if css_bytes.saturating_add(bytes) > CSS_BYTES {
                            resource_issues += 1;
                            continue;
                        }
                        // Subresource Integrity: a sheet whose digest is not
                        // one the page named is not applied, as in browsers.
                        if let Some(metadata) = integrity
                            && !integrity_matches(syntax.source(), metadata)
                        {
                            eprintln!("Stylesheet {href}: its integrity digest does not match; not applied");
                            resource_issues += 1;
                            continue;
                        }
                        css_bytes += bytes;
                        let mut budget = (imports, css_bytes, 0);
                        let mut list = imported(&syntax, url.as_deref(), base, resource, 0, &mut budget, cancel)?;
                        (imports, css_bytes) = (budget.0, budget.1);
                        resource_issues += budget.2;
                        list.push((syntax, url));
                        for (syntax, url) in list {
                            let syntax = match media {
                                Some(m) => parsed(Arc::from(under_media(m, syntax.source())), cancel)?,
                                None => syntax,
                            };
                            sheets.push((syntax, Origin::Author, url, None));
                        }
                    }
                    Some(_) => return Err(paint::Error::Invalid.into()),
                    None => resource_issues += 1,
                }
            }
        }
    }
    // A component's every instance has the same sheets in its shadow root:
    // each text parsed once, and held to the budget once (Reddit's copies
    // passed 8 MB, and the later trees' sheets were left out). They
    // compile once too (`compile_now`).
    let mut shadow_texts: std::collections::HashMap<Arc<str>, Arc<css::Syntax>> = std::collections::HashMap::new();
    for node in shadow_nodes {
        let scope = doc.shadow_scope(node).map(crate::css::selectors::scope_key);
        match doc.name(node)? {
            Some("style") => {
                let mut text = String::new();
                for child in doc.children(node)? {
                    if let Some(value) = doc.value(child)? {
                        text.push_str(value)
                    }
                }
                if let Some(syntax) = shadow_texts.get(text.as_str()) {
                    sheets.push((syntax.clone(), Origin::Author, None, scope));
                    continue;
                }
                if css_bytes.saturating_add(text.len()) > CSS_BYTES {
                    resource_issues += 1;
                    continue;
                }
                css_bytes += text.len();
                let text: Arc<str> = Arc::from(text);
                let syntax = parsed(text.clone(), cancel)?;
                shadow_texts.insert(text, syntax.clone());
                sheets.push((syntax, Origin::Author, None, scope));
            }
            Some("link") if doc.attribute(node, "rel")?.is_some_and(|r| r.split_ascii_whitespace().any(|p| p.eq_ignore_ascii_case("stylesheet"))) => {
                let Some(href) = doc.attribute(node, "href")? else { continue };
                match resource(ResourceKind::Stylesheet, base, href)? {
                    Some(Resource::Stylesheet(syntax, url)) => {
                        let seen = shadow_texts.contains_key(syntax.source());
                        if !seen && css_bytes.saturating_add(syntax.source().len()) > CSS_BYTES {
                            resource_issues += 1;
                            continue;
                        }
                        if !seen {
                            css_bytes += syntax.source().len();
                            shadow_texts.insert(Arc::from(syntax.source()), syntax.clone());
                        }
                        sheets.push((syntax, Origin::Author, url, scope));
                    }
                    _ => resource_issues += 1,
                }
            }
            _ => {}
        }
    }
    Ok((sheets, resource_issues))
}
/// What identifies the document's stylesheets without loading them: each
/// style/link element with a hash of what matters (its attributes, its
/// text), and the base URL. Changed identities mean the rules must be
/// made again.
/// Whether nothing since `checked` (a document and revision the sheets
/// were looked for at) could change the stylesheets: only inline styles,
/// attributes no sheet follows, text outside sheets and children that
/// can't be one changed. (GitHub's scripts change text every frame: the
/// walk of the page and the hash of all its CSS ran each frame.)
fn sheets_unchanged(doc: &Document, checked: Option<(u64, u64)>) -> bool {
    if let Some((id, revision)) = checked
        && id == doc.id()
        && doc.changes_since(revision).is_some_and(|changes| {
            // A sheet's element is a <style> or <link>: only changes to
            // one, to the text in one, or children that may be one
            // (elements put in) can change the sheets. (GitHub's
            // scripts change text every frame: the walk of the page
            // and the hash of all its CSS ran each frame.)
            let sheet = |n: crate::dom::NodeId| matches!(doc.name(n), Ok(Some("style" | "link")));
            changes.iter().all(|(_, c)| match c {
                crate::dom::Change::Style(_) | crate::dom::Change::Control(_) => true,
                crate::dom::Change::Attribute { name, .. } => !matches!(&**name, "href" | "rel" | "media" | "type" | "disabled" | "integrity"),
                crate::dom::Change::Text { node, .. } => !doc.parent(*node).ok().flatten().is_some_and(sheet),
                crate::dom::Change::Children { node, elements: false } => !sheet(*node),
                crate::dom::Change::Node(node) => !sheet(*node),
                _ => false,
            })
        })
    {
        return true;
    }
    false
}
/// How a page's rules changed from one compilation to the next: by each
/// step, the hosts of the shadow trees whose sheets alone changed (none:
/// something else did), so styles of the rules before can be kept outside
/// those trees (`JobOptions::changed_within`). MDN's components add their
/// styles to their shadow roots one after another while it loads; each
/// restyled the whole page from nothing.
#[derive(Clone, Default)]
struct RuleChanges(Vec<(u64, u64, Option<Arc<[crate::dom::NodeId]>>)>);
impl RuleChanges {
    fn record(&mut self, from: u64, to: u64, hosts: Option<Vec<crate::dom::NodeId>>) {
        if from == to {
            return;
        }
        if crate::env_on!("OLO_SHEET_DEBUG") {
            eprintln!("  rules {from} -> {to}: {}", hosts.as_ref().map_or("the document's sheets changed".to_owned(), |h| format!("{} hosts' trees", h.len())));
        }
        self.0.push((from, to, hosts.map(Into::into)));
        if self.0.len() > 16 {
            self.0.remove(0);
        }
    }
    /// The hosts whose trees' sheets made the difference from generation
    /// `from` to `to`, step by recorded step; none when a step changed more
    /// or the way isn't known.
    fn since(&self, from: u64, to: u64) -> Option<Vec<crate::dom::NodeId>> {
        let mut hosts = Vec::new();
        let mut at = from;
        let mut steps = self.0.iter();
        while at != to {
            let (_, next, changed) = steps.find(|(a, _, _)| *a == at)?;
            hosts.extend(changed.as_deref()?.iter().copied());
            at = *next;
        }
        hosts.sort_unstable_by_key(|n| n.slot());
        hosts.dedup();
        Some(hosts)
    }
}
/// The hosts of the shadow trees whose sheets differ between `old` and
/// `new` (sheet identities, `sheet_identities`), when the document's own
/// sheets are as they were; none otherwise (a sheet now outside the
/// document counts as the document's).
fn sheets_changed_within(
    doc: &Document,
    old: &[(crate::dom::NodeId, u64)],
    new: &[(crate::dom::NodeId, u64)],
    document_reach: impl FnOnce() -> Option<Vec<crate::dom::NodeId>>,
) -> Option<Vec<crate::dom::NodeId>> {
    let by_tree = |ids: &[(crate::dom::NodeId, u64)]| {
        let mut trees: std::collections::HashMap<Option<crate::dom::NodeId>, Vec<(crate::dom::NodeId, u64)>> = std::collections::HashMap::new();
        for &(node, id) in ids {
            trees.entry(doc.shadow_scope(node)).or_default().push((node, id));
        }
        trees
    };
    let (old, new) = (by_tree(old), by_tree(new));
    let mut hosts = Vec::new();
    // The document's own sheets: what their rules added or taken away
    // reach, else everything.
    if old.get(&None) != new.get(&None) {
        let reach = document_reach();
        if crate::env_on!("OLO_SHEET_DEBUG") {
            eprintln!(
                "  document sheets {} -> {}: {}",
                old.get(&None).map_or(0, Vec::len),
                new.get(&None).map_or(0, Vec::len),
                reach.as_ref().map_or("everything restyled".to_owned(), |r| format!("{} elements reached", r.len()))
            );
        }
        hosts.extend(reach?);
    }
    for tree in old.keys().chain(new.keys()) {
        let Some(root) = tree else { continue };
        if old.get(tree) != new.get(tree) {
            // (A tree whose host is gone (a feed's post let go) has nothing
            // left to restyle; that gave the whole page up.)
            if let Some(host) = doc.shadow_host(*root).filter(|h| doc.is_connected(*h).unwrap_or(false)) {
                hosts.push(host);
            }
        }
    }
    hosts.sort_unstable_by_key(|n| n.slot());
    hosts.dedup();
    Some(hosts)
}
/// The elements (each with its subtree) the rules of the document's sheets
/// added or taken away (`old` to `new`) can restyle: those carrying a key
/// their subjects address (an id, class, type or attribute; the root).
/// None when it can't be told: a rule that can match any element, the
/// sheets only reordered, at-rules whose reach is the page's (fonts,
/// keyframes, layers, imports), or too many. (Reddit's feed adds and
/// takes away <link>ed sheets as it loads posts: the page restyled whole,
/// 105-120 ms, each time.)
fn document_sheets_reach(doc: &Document, old: &[Sheet], new: &[Sheet], media: css::media::Media) -> Option<Vec<crate::dom::NodeId>> {
    let documents = |list: &[Sheet]| -> Vec<Sheet> { list.iter().filter(|s| s.3.is_none() && s.1 == Origin::Author).cloned().collect() };
    let (old, new) = (documents(old), documents(new));
    let same = |a: &Sheet, b: &Sheet| (Arc::ptr_eq(&a.0, &b.0) || a.0.source() == b.0.source()) && a.2 == b.2;
    let added: Vec<&Sheet> = new.iter().filter(|s| !old.iter().any(|t| same(s, t))).collect();
    let removed: Vec<&Sheet> = old.iter().filter(|s| !new.iter().any(|t| same(s, t))).collect();
    if added.is_empty() && removed.is_empty() {
        return None;
    }
    let mut builder = SheetBuilder::new(Default::default()).with_media(media);
    for s in added.iter().chain(&removed) {
        let text = s.0.source();
        if let Some(at) = ["@font-face", "@keyframes", "@layer", "@import", "@property", "@counter-style", "@scope", "@container", "@font-feature-values"].iter().find(|a| text.contains(*a)) {
            if crate::env_on!("OLO_SHEET_DEBUG") {
                eprintln!("  document sheet reach: {at} in a sheet added or taken away ({} bytes)", text.len());
            }
            return None;
        }
        builder.add_scoped_sheet_from(&s.0, s.1, s.2.clone(), None, None).ok()?;
    }
    let Some(keys) = builder.finish(None).ok()?.subject_keys() else {
        if crate::env_on!("OLO_SHEET_DEBUG") {
            eprintln!("  document sheet reach: a rule that can match any element");
        }
        return None;
    };
    let mut out = Vec::new();
    let mut stack = vec![doc.root()];
    while let Some(n) = stack.pop() {
        if let Ok(children) = doc.children(n) {
            stack.extend(children);
        }
        if doc.node_type(n).ok() != Some(NodeType::Element) {
            continue;
        }
        let name = doc.name(n).ok().flatten().unwrap_or("");
        let hit = keys.types.iter().any(|t| t.eq_ignore_ascii_case(name))
            || (keys.root && doc.parent(n).ok().flatten() == Some(doc.root()))
            || doc.attribute(n, "id").ok().flatten().is_some_and(|id| keys.ids.contains(id))
            || doc.attribute(n, "class").ok().flatten().is_some_and(|c| c.split_ascii_whitespace().any(|c| keys.classes.contains(c)))
            || (!keys.attributes.is_empty() && keys.attributes.iter().any(|a| doc.attribute(n, a).ok().flatten().is_some()));
        if hit {
            out.push(n);
            if out.len() > 2000 {
                return None;
            }
        }
    }
    Some(out)
}
fn sheet_identities(doc: &Document) -> Result<(Vec<(crate::dom::NodeId, u64)>, Option<String>, Vec<crate::dom::NodeId>)> {
    use std::hash::{Hash, Hasher};
    let mut ids = Vec::new();
    let mut nodes = Vec::new();
    let mut base = None;
    let mut next = Some(doc.root());
    // Shadow roots met on the way, walked after the document (theirs are
    // scoped sheets), and those inside them in turn.
    let mut roots: Vec<crate::dom::NodeId> = Vec::new();
    let mut walking: Vec<crate::dom::NodeId> = Vec::new();
    // Past the document, in shadow trees.
    let mut in_shadow = false;
    loop {
        let Some(node) = next.or_else(|| {
            let root = roots.pop()?;
            in_shadow = true;
            walking = doc.children(root).map(|c| c.collect::<Vec<_>>()).unwrap_or_default();
            walking.reverse();
            walking.pop()
        }) else {
            break;
        };
        if let Some(root) = doc.shadow_root(node) {
            roots.push(root);
        }
        if doc.node_type(node)? == NodeType::Element {
            match doc.name(node)? {
                Some("base") if base.is_none() => {
                    if let Some(href) = doc.attribute(node, "href")? {
                        base = Some(href.to_owned());
                    }
                }
                Some(name @ ("style" | "link")) => {
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    name.hash(&mut h);
                    for a in ["href", "rel", "media", "type", "disabled", "integrity"] {
                        doc.attribute(node, a)?.hash(&mut h);
                    }
                    for child in doc.children(node)? {
                        doc.value(child)?.hash(&mut h);
                    }
                    ids.push((node, h.finish()));
                    nodes.push(node);
                }
                _ => {}
            }
        }
        // In the document, its order; in a shadow tree, depth first.
        if !in_shadow {
            next = next_node(doc, node)?;
        } else {
            if let Ok(children) = doc.children(node) {
                let mut children: Vec<_> = children.collect();
                children.reverse();
                walking.extend(children);
            }
            next = walking.pop();
        }
    }
    Ok((ids, base, nodes))
}
/// The web fonts the faces name: the first decodable source of each, loaded
/// and decoded (WOFF unpacked); a face that fails is unavailable.
fn load_fonts(
    faces: &[css::fonts::FontFace],
    base: Option<&str>,
    resource: &mut impl FnMut(ResourceKind, Option<&str>, &str) -> Result<Option<Resource>>,
    cancel: Option<&AtomicBool>,
) -> Result<(Vec<(Arc<[u8]>, crate::font::Descriptor)>, usize)> {
    let mut loaded = Vec::new();
    let mut font_bytes = 0usize;
    let mut resource_issues = 0usize;
    for face in faces.iter().take(FONT_FACES) {
        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
            return Err(paint::Error::Cancelled.into());
        }
        let Some(css::fonts::Source::Url { url, .. }) = face.sources.iter().find(|s| s.decodable()) else {
            if crate::env_on!("OLO_FONT_DEBUG") {
                eprintln!("  font {}: no source it can decode", face.descriptor.family);
            }
            resource_issues += 1;
            continue;
        };
        let got = resource(ResourceKind::Font, face.base.as_deref().or(base), url)?;
        if crate::env_on!("OLO_FONT_DEBUG") {
            eprintln!("  font {} from {url}: {}", face.descriptor.family, if matches!(got, Some(Resource::Font(_))) { "loaded" } else { "not loaded" });
        }
        match got {
            Some(Resource::Font(bytes)) => {
                font_bytes = font_bytes.checked_add(bytes.len()).ok_or(paint::Error::Limit)?;
                if font_bytes > FONT_BYTES {
                    return Err(paint::Error::Limit.into());
                }
                let data: Arc<[u8]> = if crate::font::woff::is_woff(&bytes) {
                    match crate::font::woff::decode(&bytes, cancel) {
                        Ok(sfnt) => {
                            font_bytes = font_bytes.checked_add(sfnt.len()).ok_or(paint::Error::Limit)?;
                            if font_bytes > FONT_BYTES {
                                return Err(paint::Error::Limit.into());
                            }
                            Arc::from(sfnt)
                        }
                        Err(crate::font::Error::Cancelled) => return Err(paint::Error::Cancelled.into()),
                        Err(crate::font::Error::Limit) => return Err(paint::Error::Limit.into()),
                        Err(_) => {
                            resource_issues += 1;
                            continue;
                        }
                    }
                } else {
                    bytes
                };
                loaded.push((data, face.descriptor.clone()));
            }
            None => resource_issues += 1,
            _ => return Err(paint::Error::Invalid.into()),
        }
    }
    resource_issues += faces.len().saturating_sub(FONT_FACES);
    Ok((loaded, resource_issues))
}
/// Everything after parsing: the document's stylesheets, images and fonts
/// loaded and the sheets compiled.
#[allow(clippy::too_many_arguments)] // The page's inputs, each distinct.
fn gather(
    doc: Document,
    quirks: bool,
    base_scope: &Scope,
    media: css::media::Media,
    fonts: bool,
    cancel: Option<&AtomicBool>,
    mut progress: impl FnMut(Phase, usize),
    mut resource: impl FnMut(ResourceKind, Option<&str>, &str) -> Result<Option<Resource>>,
    prefetch: &mut impl FnMut(ResourceKind, &[(Option<String>, String)]),
    reuse: Option<&Prepared>,
) -> Result<Prepared> {
    progress(Phase::Styles, 0);
    let mut sheets = ua_sheets(&doc, quirks, cancel)?;
    let mut next = Some(doc.root());
    let mut title = String::new();
    let mut scripts = 0;
    let mut resource_issues = 0;
    let mut base = None;
    let mut style_nodes = Vec::new();
    let mut image_nodes = Vec::new();
    // Scripts and preloads, fetched ahead with the sheets (by kind).
    let mut ahead: Vec<(ResourceKind, String)> = Vec::new();
    let mut inline_backgrounds: Vec<Arc<str>> = Vec::new();
    while let Some(node) = next {
        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
            return Err(paint::Error::Cancelled.into());
        }
        if doc.node_type(node)? == NodeType::Element {
            // Inline `style="background-image: url(…)"`: its images are
            // fetched with the sheets'.
            if let Some(style) = doc.attribute(node, "style")?
                && style.contains("url(")
                && inline_backgrounds.len() < BACKGROUND_IMAGES
                && let Ok(syntax) = css::Syntax::parse(Arc::from(style), Default::default(), cancel)
                && let Ok(declarations) = syntax.declarations(cancel)
            {
                for d in declarations {
                    if let Ok(css::properties::Declaration::Known { values, .. }) = css::properties::Declaration::compile(&d, Default::default(), cancel) {
                        for a in values.iter() {
                            if let css::properties::Value::Images(images) = &a.value {
                                for image in images.0.iter().flatten() {
                                    if let css::properties::Image::Url(url) = image
                                        && !inline_backgrounds.contains(url)
                                    {
                                        inline_backgrounds.push(url.clone());
                                    }
                                }
                            }
                        }
                    }
                }
            }
            match doc.name(node)? {
                Some("base") if base.is_none() => {
                    if let Some(href) = doc.attribute(node, "href")? {
                        base = Some(href.to_owned());
                    }
                }
                Some("style" | "link") => style_nodes.push(node),
                // A video's poster: its picture until a frame shows.
                Some("video") if doc.attribute(node, "poster")?.is_some_and(|p| !p.trim().is_empty()) => {
                    if image_nodes.len() < PAGE_IMAGE_LIMITS.bindings {
                        image_nodes.push(node);
                    }
                }
                Some("img") => {
                    // Past the bindings an image is left out (an issue below).
                    if image_nodes.len() < PAGE_IMAGE_LIMITS.bindings {
                        image_nodes.push(node);
                    } else {
                        resource_issues += 1;
                    }
                }
                Some("script") => {
                    scripts += 1;
                    if let Some(src) = doc.attribute(node, "src")?.map(str::trim).filter(|s| !s.is_empty())
                        && doc.attribute(node, "nomodule")?.is_none()
                    {
                        ahead.push((ResourceKind::Script, src.to_owned()));
                    }
                }
                Some("title") => {
                    for child in doc.children(node)? {
                        if let Some(value) = doc.value(child)? {
                            title.push_str(value)
                        }
                    }
                }
                _ => {}
            }
        }
        next = next_node(&doc, node)?;
    }
    // Everything the page links is fetched together before the loads
    // below, which then find it fetched: stylesheets, images; later fonts
    // and background images once the sheets say which.
    {
        let mut links = Vec::new();
        for &node in &style_nodes {
            if doc.name(node)? == Some("link")
                && doc.attribute(node, "rel")?.is_some_and(|r| r.split_ascii_whitespace().any(|p| p.eq_ignore_ascii_case("stylesheet")))
                && let Some(href) = doc.attribute(node, "href")?
            {
                links.push((base.clone(), href.to_owned()));
            }
        }
        // Preloads by what they are (`as`), and module preloads.
        for &node in &style_nodes {
            if doc.name(node)? != Some("link") {
                continue;
            }
            let rel = doc.attribute(node, "rel")?.unwrap_or("").to_ascii_lowercase();
            let Some(href) = doc.attribute(node, "href")? else { continue };
            let kind = if rel.split_ascii_whitespace().any(|r| r == "modulepreload") {
                Some(ResourceKind::Script)
            } else if rel.split_ascii_whitespace().any(|r| r == "preload") {
                match doc.attribute(node, "as")?.map(|a| a.trim().to_ascii_lowercase()).as_deref() {
                    Some("script") => Some(ResourceKind::Script),
                    Some("style") => Some(ResourceKind::Stylesheet),
                    Some("font") => Some(ResourceKind::Font),
                    Some("image") => Some(ResourceKind::Image),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(kind) = kind {
                ahead.push((kind, href.to_owned()));
            }
        }
        prefetch(ResourceKind::Stylesheet, &links);
        for kind in [ResourceKind::Script, ResourceKind::Stylesheet, ResourceKind::Font, ResourceKind::Image] {
            let items: Vec<(Option<String>, String)> = ahead.iter().filter(|(k, _)| *k == kind).map(|(_, h)| (base.clone(), h.clone())).collect();
            if !items.is_empty() {
                prefetch(kind, &items);
            }
        }
        let images: Vec<(Option<String>, String)> = image_nodes.iter().filter_map(|&n| picture_source(&doc, n, &media, cancel).map(|c| (base.clone(), c.url))).collect();
        prefetch(ResourceKind::Image, &images);
    }
    // The ordered small list avoids a second walk of all text/elements. Base
    // discovery is complete before any resource is fetched.
    // With those in shadow trees (a scripted document's components), as
    // `sheet_identities` finds them: the page's identities and its sheets
    // must be of the same elements (else a later refresh finds nothing
    // changed while the shadow trees' sheets were never loaded).
    if doc.has_shadows() {
        style_nodes = sheet_identities(&doc)?.2;
    }
    let (loaded_sheets, sheet_issues) = load_sheets(&doc, &style_nodes, base.as_deref(), &mut resource, cancel)?;
    sheets.extend(loaded_sheets);
    resource_issues += sheet_issues;
    let mut image_bindings = Vec::new();
    let mut late = Vec::new();
    // An image at a srcset density, once per image and density.
    let mut dense: std::collections::HashMap<(usize, u64), Arc<crate::image::Image>> = Default::default();
    let mut image_unique = std::collections::HashSet::new();
    let mut image_bytes = 0usize;
    for node in image_nodes {
        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
            return Err(paint::Error::Cancelled.into());
        }
        // The source its <picture>, srcset and sizes choose for this
        // display, at the density it was chosen for.
        let Some(choice) = picture_source(&doc, node, &media, cancel) else {
            resource_issues += 1;
            continue;
        };
        let got = match resource(ResourceKind::Image, base.as_deref(), &choice.url) {
            // Not here yet: the page shows without it, and binds it later.
            Err(e) if e.to_string() == PENDING => {
                late.push(Late::Element(node));
                continue;
            }
            got => got?,
        };
        match got {
            Some(Resource::Image(image)) => {
                let bound = if (choice.density - 1.).abs() > 1e-9 && image.density() == 1. {
                    dense.entry((Arc::as_ptr(&image) as usize, choice.density.to_bits())).or_insert_with(|| image.with_density(choice.density)).clone()
                } else {
                    image.clone()
                };
                if image_unique.contains(&Arc::as_ptr(&image)) {
                    image_bindings.push((node, bound));
                } else if image_bytes.saturating_add(image.payload_bytes()) > PAGE_IMAGE_LIMITS.bytes || image_bindings.len() >= PAGE_IMAGE_LIMITS.bindings {
                    resource_issues += 1;
                } else {
                    image_unique.insert(Arc::as_ptr(&image));
                    image_bytes += image.payload_bytes();
                    image_bindings.push((node, bound));
                }
            }
            None => resource_issues += 1,
            _ => return Err(paint::Error::Invalid.into()),
        }
    }
    // The same sheets as an earlier preparation compile to the same rules,
    // and load the same fonts: both are kept.
    let reused = reuse.filter(|p| same_sheets(&p.sheets, &sheets));
    let (rules, media_dependent, faces, background_urls) = match reused {
        Some(p) => (p.rules.clone(), p.media_dependent, Vec::new(), p.backgrounds.clone()),
        None => compile(&sheets, media, cancel)?,
    };
    let rules_reused = reused.is_some();
    {
        let backgrounds: Vec<(Option<String>, String)> = background_urls.iter().take(BACKGROUND_IMAGES).map(|(u, b)| (b.as_deref().or(base.as_deref()).map(str::to_owned), u.to_string())).collect();
        prefetch(ResourceKind::Image, &backgrounds);
        if fonts {
            let font_urls: Vec<(Option<String>, String)> = faces
                .iter()
                .take(FONT_FACES)
                .filter_map(|face| {
                    face.sources
                        .iter()
                        .find(|s| s.decodable())
                        .and_then(|s| if let css::fonts::Source::Url { url, .. } = s { Some((face.base.as_deref().or(base.as_deref()).map(str::to_owned), url.to_string())) } else { None })
                })
                .collect();
            prefetch(ResourceKind::Font, &font_urls);
        }
    }
    // background-image files, from the sheets (relative to each sheet) and
    // inline styles (relative to the document), keyed by the URL as written.
    let mut backgrounds: std::collections::HashMap<Arc<str>, Arc<crate::image::Image>> = Default::default();
    let inline: Vec<(Arc<str>, Option<Arc<str>>)> = inline_backgrounds.into_iter().map(|u| (u, None)).collect();
    for (url, sheet_base) in background_urls.iter().chain(inline.iter()).take(BACKGROUND_IMAGES) {
        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
            return Err(paint::Error::Cancelled.into());
        }
        if backgrounds.contains_key(url) {
            continue;
        }
        let resolved = sheet_base.as_deref().or(base.as_deref());
        let got = match resource(ResourceKind::Image, resolved, url) {
            Err(e) if e.to_string() == PENDING => {
                late.push(Late::Background(url.clone(), resolved.map(str::to_owned)));
                continue;
            }
            got => got?,
        };
        match got {
            Some(Resource::Image(image)) => {
                if image_unique.contains(&Arc::as_ptr(&image)) {
                    backgrounds.insert(url.clone(), image);
                } else if image_bytes.saturating_add(image.payload_bytes()) > PAGE_IMAGE_LIMITS.bytes {
                    resource_issues += 1;
                } else {
                    image_unique.insert(Arc::as_ptr(&image));
                    image_bytes += image.payload_bytes();
                    backgrounds.insert(url.clone(), image);
                }
            }
            None => resource_issues += 1,
            _ => return Err(paint::Error::Invalid.into()),
        }
    }
    let images = if image_bindings.is_empty() && backgrounds.is_empty() { None } else { Some(crate::image::Set::with_backgrounds(&doc, image_bindings, backgrounds, PAGE_IMAGE_LIMITS, cancel)?) };
    // Web fonts: the first decodable source of each face. A face whose file
    // fails to load or decode is simply unavailable, as in other browsers.
    let mut loaded = Vec::new();
    if fonts {
        let (fonts_loaded, font_issues) = load_fonts(&faces, base.as_deref(), &mut resource, cancel)?;
        loaded = fonts_loaded;
        resource_issues += font_issues;
    }
    let scope = match reused {
        Some(p) => p.scope.clone(),
        None if loaded.is_empty() => base_scope.clone(),
        None => scope_with_faces(base_scope, loaded, cancel)?,
    };
    let (sheet_ids, _, _) = sheet_identities(&doc)?;
    let font_faces = reused.map_or(faces, |p| p.font_faces.clone());
    Ok(Prepared {
        late,
        document: doc,
        sheets,
        rules,
        media,
        media_dependent,
        font_faces,
        images,
        title,
        scripts,
        base,
        resource_issues,
        quirks,
        scope,
        base_scope: base_scope.clone(),
        sheet_ids,
        backgrounds: background_urls,
        rules_reused,
    })
}
/// Style, lay out and paint a prepared document for a host made for its scope.
pub fn finish(
    prepared: Prepared,
    host: &impl Host,
    shaper: &mut shaping::Engine,
    painter: &mut paint::Engine,
    viewport: layout::Viewport,
    cancel: Option<&AtomicBool>,
    progress: impl FnMut(Phase, usize),
) -> Result<Page> {
    finish_with(prepared, host, shaper, painter, viewport, cancel, None, progress)
}
/// `finish`, keeping what a layout of the same document under the same
/// rules (a script measurement's) already computed: styles, prepared texts.
#[allow(clippy::too_many_arguments)]
pub fn finish_with(
    prepared: Prepared,
    host: &impl Host,
    shaper: &mut shaping::Engine,
    painter: &mut paint::Engine,
    viewport: layout::Viewport,
    cancel: Option<&AtomicBool>,
    previous: Option<&layout::Layout>,
    mut progress: impl FnMut(Phase, usize),
) -> Result<Page> {
    let Prepared { document: doc, sheets, rules, media, media_dependent, images, title, scripts, base, resource_issues, quirks, scope, rules_reused, base_scope, sheet_ids, late, .. } = prepared;
    if host.font_context() != scope.context() {
        return Err(style::Error::Stale.into());
    }
    let interaction_mask = rules.state_mask();
    // The scripts' last measurement serves when it was styled with these
    // very rules (an earlier preparation's, or the same compilation again,
    // from the compiled-rules cache).
    if crate::env_on!("OLO_WEB_TIMING")
        && let Some(p) = previous
    {
        eprintln!(
            "  finish: previous layout {}, its rules {}, its fonts {}",
            p.cache().tree().styles().document_revision(),
            if rules_reused || p.cache().tree().styles().generation() == rules.generation() { "the same" } else { "others" },
            if p.cache().tree().styles().defaults().is_some_and(|d| d.is_current(host)) { "the same" } else { "others" }
        );
    }
    let previous = previous.filter(|p| rules_reused || p.cache().tree().styles().generation() == rules.generation());
    let mut timeline = css::animation::Timeline::default();
    let mut containers = crate::dom::NodeMap::default();
    let (scene, stylesheet_issues, fallbacks) = render(
        &doc,
        images.as_ref(),
        &rules,
        quirks,
        resource_issues,
        Interaction::default(),
        host,
        &scope,
        shaper,
        painter,
        viewport,
        cancel,
        previous,
        None,
        &mut Default::default(),
        &mut timeline,
        &mut containers,
        &[],
        &[],
        None,
        &mut progress,
    )?;
    Ok(Page {
        images,
        late,
        rendered_interaction: Interaction::default(),
        animated: Default::default(),
        containers,
        timeline,
        document: doc,
        scene,
        title,
        scripts,
        base_href: base,
        interaction: Interaction::default(),
        interaction_mask,
        stylesheet_issues,
        fallbacks,
        rules,
        sheets,
        media,
        media_dependent,
        scope,
        quirks,
        resource_issues,
        base_scope,
        sheet_ids,
        sheets_checked: None,
        pending_compile: None,
        rule_changes: RuleChanges::default(),
        pictures_scanned: None,
    })
}
impl Page {
    /// The pictures its styles name (`background-image`, `mask-image`, as
    /// computed: through `var()`, inline styles, rules scripts added) that
    /// it has none of, taken as late ones (`bind_late` fetches and binds
    /// them): whether any came. Looked for once per style tree, in the
    /// styles it doesn't share with the tree looked in before (GitHub's
    /// animations make a new tree each frame, 9 of 2,578 styles patched:
    /// the whole scan was a fifth of its page thread while scrolling).
    pub fn note_pictures(&mut self) -> bool {
        let styles = self.scene.layout().cache().tree().styles().clone();
        if self.pictures_scanned.as_ref().is_some_and(|s| Arc::ptr_eq(s, &styles)) {
            return false;
        }
        let before = self.pictures_scanned.replace(styles.clone());
        let mut added = false;
        for (_, style) in styles.styles_unshared(before.as_deref()) {
            for property in [css::properties::Property::BackgroundImage, css::properties::Property::MaskImage] {
                let style::Value::Images(images) = style.value(property) else { continue };
                for image in images.0.iter().flatten() {
                    let css::properties::Image::Url(url) = image else { continue };
                    if self.images.as_ref().is_some_and(|set| set.background(url).is_some()) || self.late.iter().any(|l| matches!(l, Late::Background(u, _) if u == url)) {
                        continue;
                    }
                    if self.late.len() >= BACKGROUND_IMAGES {
                        return added;
                    }
                    self.late.push(Late::Background(url.clone(), self.base_href.clone()));
                    added = true;
                }
            }
        }
        added
    }
    /// Whether it shows without pictures still on their way.
    pub fn has_late_images(&self) -> bool {
        !self.late.is_empty()
    }
    /// How many pictures it shows without, still on their way.
    pub fn late_images(&self) -> usize {
        self.late.len()
    }
    /// The pictures it showed without that are here now, bound (those still
    /// on their way kept for later): whether any was, so it renders again.
    /// `doc` is the document as it is now (scripts may have moved on).
    pub fn bind_late(&mut self, doc: &Document, cancel: Option<&AtomicBool>, resource: &mut impl FnMut(ResourceKind, Option<&str>, &str) -> Result<Option<Resource>>) -> Result<bool> {
        let mut bindings = Vec::new();
        let mut backgrounds = std::collections::HashMap::new();
        let mut waiting = Vec::new();
        for late in std::mem::take(&mut self.late) {
            let (base, href) = match &late {
                Late::Element(node) => {
                    // Gone, or no longer a picture: nothing to bind.
                    if !doc.is_connected(*node).unwrap_or(false) || !matches!(doc.name(*node).ok().flatten(), Some("img" | "video")) {
                        continue;
                    }
                    match picture_source(doc, *node, &self.media, cancel) {
                        Some(choice) => (self.base_href.clone(), choice),
                        None => continue,
                    }
                }
                Late::Background(url, base) => (base.clone(), crate::srcset::Choice { url: url.to_string(), density: 1. }),
            };
            match resource(ResourceKind::Image, base.as_deref(), &href.url) {
                Err(e) if e.to_string() == PENDING => waiting.push(late),
                Ok(Some(Resource::Image(image))) => match late {
                    Late::Element(node) => {
                        let image = if (href.density - 1.).abs() > 1e-9 && image.density() == 1. { image.with_density(href.density) } else { image };
                        bindings.push((node, image));
                    }
                    Late::Background(url, _) => {
                        backgrounds.insert(url, image);
                    }
                },
                Ok(_) => self.resource_issues += 1,
                Err(e) if e.to_string().ends_with("Cancelled") => return Err(e),
                Err(_) => self.resource_issues += 1,
            }
        }
        self.late = waiting;
        if bindings.is_empty() && backgrounds.is_empty() {
            return Ok(false);
        }
        let set = match &self.images {
            Some(set) => set.with_more(doc, bindings, backgrounds, PAGE_IMAGE_LIMITS, cancel),
            None => crate::image::Set::with_backgrounds(doc, bindings, backgrounds, PAGE_IMAGE_LIMITS, cancel),
        };
        match set {
            Ok(set) => self.images = Some(set),
            // Past the page's budget: left out, as at its preparation.
            Err(crate::image::Error::Limit) => self.resource_issues += 1,
            Err(e) => return Err(e.into()),
        }
        Ok(true)
    }
    /// The moment (ms on the page's clock) its next render shows its CSS
    /// animations at.
    pub fn set_time(&mut self, now_ms: f64) {
        self.timeline.now_ms = now_ms;
    }
    /// Show new pictures for elements already showing one of the same size
    /// (a canvas's or a video's next frame) without a render: the scene and
    /// its layout as they are, over an image set with those pictures.
    /// False when that can't be (a size changed, an element isn't bound):
    /// the caller renders.
    pub fn swap_pictures(&mut self, pictures: &[(crate::dom::NodeId, Arc<crate::image::Image>)], cancel: Option<&AtomicBool>) -> bool {
        let why = |w: &str| {
            if crate::env_on!("OLO_WEB_TIMING") {
                eprintln!("swap pictures: no, {w}");
            }
            false
        };
        if pictures.is_empty() {
            return why("no pictures");
        }
        if self.timeline.running {
            return why("CSS animations running");
        }
        let Some(images) = self.images.as_ref() else { return why("no image set") };
        let layout = self.scene.layout();
        let tree = layout.cache().tree();
        // The layout must be of this image set (no bindings came or went since).
        if tree.images_identity() != Arc::as_ptr(images) as *const u8 as usize {
            return why("the layout is of another image set");
        }
        let Some(set) = images.with_replaced(pictures) else { return why("a picture isn't bound or changed size") };
        let styles = tree.styles().clone();
        let next = Arc::new(layout.with_styles(styles, &self.document, Some(set.clone())));
        match self.scene.with_transforms(next, &[], cancel) {
            Ok(Some(scene)) => {
                self.scene = scene;
                self.images = Some(set);
                true
            }
            _ => false,
        }
    }
    /// Scroll a box (a scroll container) by `delta` (CSS px), clamped to
    /// what it holds; the new offset when it moved. The scene is built
    /// again over the same layout (`repaint`): nothing restyles or lays out.
    pub fn scroll_box(&mut self, node: crate::dom::NodeId, delta: [f64; 2]) -> Option<[f64; 2]> {
        let sc = self.scene.scrollers().iter().find(|s| s.node == node).copied()?;
        let max = sc.max();
        // From where the document has it (scrolls since the scene was built count).
        let at = self.document.scroll_offset(node);
        let to = [(at[0] + delta[0]).clamp(0., max[0]), (at[1] + delta[1]).clamp(0., max[1])];
        self.document.set_scroll_offset(node, to).ok()?.then_some(to)
    }
    /// The scene built again over the same layout, as the document's
    /// scroll offsets have it now.
    pub fn repaint(&mut self, host: &impl Host, painter: &mut paint::Engine, cancel: Option<&AtomicBool>) -> Result<()> {
        let options = paint_options(host, &self.document, self.scene.layout())?;
        let mut job = paint::BuildJob::new(&self.document, self.scene.layout().clone(), painter, options, Default::default(), cancel)?.with_animated(self.animated.clone());
        while !job.step(256)?.finished {}
        self.scene = job.finish()?;
        Ok(())
    }
    /// Some CSS animation still changes: render again at the next frame.
    pub fn animating(&self) -> bool {
        self.timeline.running
    }
    /// The CSS animation events since the last call: element, type
    /// (animationstart, animationend), name, elapsed seconds.
    pub fn take_animation_events(&mut self) -> Vec<(crate::dom::NodeId, &'static str, Arc<str>, f64)> {
        self.timeline.take_events()
    }
    /// Follow a later revision's stylesheets (see `Measurer::refresh`):
    /// returns whether the rules were made again.
    /// With `wake`, new rules compile in the background: `COMPILING_RULES`
    /// is answered meanwhile (the page shows what it showed, its event loop
    /// going on), and `wake` told when they are ready.
    pub fn refresh(
        &mut self,
        doc: &Document,
        cancel: Option<&AtomicBool>,
        resource: &mut impl FnMut(ResourceKind, Option<&str>, &str) -> Result<Option<Resource>>,
        wake: Option<&Wake>,
    ) -> Result<bool> {
        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
            self.cancel_stylesheet_refresh();
            self.sheets_checked = None;
            return Err(paint::Error::Cancelled.into());
        }
        // Nothing that could change the sheets since the last look: no
        // walk of the document (it ran on every render while apple.com
        // loaded).
        if sheets_unchanged(doc, self.sheets_checked) {
            self.sheets_checked = Some((doc.id(), doc.revision()));
            return Ok(false);
        }
        self.sheets_checked = Some((doc.id(), doc.revision()));
        let (ids, base, nodes) = sheet_identities(doc)?;
        if crate::env_on!("OLO_SHEET_DEBUG") {
            eprintln!("  page refresh: {} sheet elements, {}", nodes.len(), if ids == self.sheet_ids { "as before" } else { "changed" });
        }
        if ids == self.sheet_ids {
            self.pending_compile = None;
            return Ok(false);
        }
        let mut sheets = ua_sheets(doc, self.quirks, cancel)?;
        // A sheet or font still loading (the host fetches it without
        // waiting, see `PENDING`): looked at again at the next render.
        let (loaded, _) = load_sheets(doc, &nodes, base.as_deref(), resource, cancel).inspect_err(|_| self.sheets_checked = None)?;
        sheets.extend(loaded);
        let (rules, dependent, faces, backgrounds) = match wake {
            Some(wake) => compile_in_background(&sheets, self.media, wake, &mut self.pending_compile).inspect_err(|_| self.sheets_checked = None)?,
            None => compile(&sheets, self.media, cancel)?,
        };
        // The pictures the new rules name (backgrounds, masks) that it has
        // none of: fetched and bound as late ones are (`bind_late`).
        for (url, sheet_base) in backgrounds.iter().take(BACKGROUND_IMAGES) {
            let had = self.images.as_ref().is_some_and(|set| set.background(url).is_some());
            if !had && !self.late.iter().any(|l| matches!(l, Late::Background(u, _) if u == url)) {
                self.late.push(Late::Background(url.clone(), sheet_base.as_deref().map(str::to_owned).or_else(|| base.clone())));
            }
        }
        let (fonts, _) = load_fonts(&faces, base.as_deref(), resource, cancel).inspect_err(|_| self.sheets_checked = None)?;
        let media = self.media;
        self.rule_changes.record(self.rules.generation(), rules.generation(), sheets_changed_within(doc, &self.sheet_ids, &ids, || document_sheets_reach(doc, &self.sheets, &sheets, media)));
        self.rules = rules;
        self.media_dependent = dependent;
        self.interaction_mask = self.rules.state_mask();
        self.scope = if fonts.is_empty() { self.base_scope.clone() } else { scope_with_faces(&self.base_scope, fonts, cancel)? };
        self.sheets = sheets;
        self.sheet_ids = ids;
        self.base_href = base;
        self.pending_compile = None;
        Ok(true)
    }
    /// Release this page's pending stylesheet request, keeping its current
    /// rules. Called when a host stops a load or leaves the document. Other
    /// pages sharing the job keep it alive; a later refresh can request it
    /// again. This also releases ready rules not yet applied by this page.
    pub fn cancel_stylesheet_refresh(&mut self) {
        if self.pending_compile.take().is_some() {
            self.sheets_checked = None;
        }
    }
    /// The style sheets it was styled with, parsed (with their compiled
    /// declarations): held, the next pass over the same document takes
    /// them as they are.
    pub fn parsed_sheets(&self) -> Vec<Arc<css::Syntax>> {
        self.sheets.iter().map(|s| s.0.clone()).collect()
    }
    /// The fonts this page is styled and shaped with.
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
    /// The computed styles this page was laid out from.
    pub fn styles(&self) -> &Arc<style::StyleTree> {
        self.scene.layout().cache().tree().styles()
    }
    /// This page rendered again from `doc`, a later revision of its document
    /// (as scripts left it): styles kept where nothing changed, the images
    /// rebound, everything laid out and painted. The previous page's
    /// sheets, rules and scope carry over (new stylesheets and images the
    /// scripts added are not loaded here).
    pub fn rerender(
        &mut self,
        doc: Document,
        canvases: Vec<(crate::dom::NodeId, Arc<crate::image::Image>)>,
        host: &impl Host,
        shaper: &mut shaping::Engine,
        painter: &mut paint::Engine,
        viewport: layout::Viewport,
        cancel: Option<&AtomicBool>,
        mut progress: impl FnMut(Phase, usize),
    ) -> Result<()> {
        if host.font_context() != self.scope.context() {
            return Err(style::Error::Stale.into());
        }
        let timing = std::env::var_os("OLO_WEB_TIMING").map(|_| std::time::Instant::now());
        if let Some(images) = &self.images
            && let Some(rebound) = images.rebind(&doc)
        {
            self.images = Some(rebound);
        }
        if !canvases.is_empty() {
            self.images = Some(match &self.images {
                Some(set) => set.with_extra(&doc, canvases, SCRIPTED_IMAGE_LIMITS, cancel)?,
                None => crate::image::Set::from_bindings(&doc, canvases, SCRIPTED_IMAGE_LIMITS, cancel)?,
            });
        }
        let rebind_ms = timing.map_or(0., |t| t.elapsed().as_secs_f64() * 1000.);
        let media = css::media::Media { scripting: self.media.scripting, ..media_for(host, viewport) };
        if self.media_dependent && media != self.media {
            let (rules, dependent, _, _) = compile(&self.sheets, media, cancel)?;
            self.rules = rules;
            self.media_dependent = dependent;
            self.interaction_mask = self.rules.state_mask();
            self.media = media;
        }
        let previous_scene = self.scene.clone();
        let previous = previous_scene.layout().clone();
        // Another hover, focus or press: the elements whose states flipped
        // matched again, with what their selectors reach (see
        // `interaction_rematch`); where that can't be followed, every
        // element (geometry and texts kept where they can be), the previous
        // scene then no longer standing for patching.
        let interaction_changed = self.rendered_interaction != self.interaction;
        let rematch = if interaction_changed { interaction_rematch(&doc, &self.rules, self.rendered_interaction, self.interaction, self.interaction_mask) } else { Some(Vec::new()) };
        let patchable_scene = rematch.is_some().then_some(&previous_scene);
        let before_render = std::time::Instant::now();
        let restyle: Vec<crate::dom::NodeId> = if rematch.is_none() { vec![doc.root()] } else { Vec::new() };
        if crate::env_on!("OLO_WEB_TIMING") && interaction_changed {
            match &rematch {
                Some(r) => eprintln!("rerender: the interaction changed: {} elements matched again", r.len()),
                None => eprintln!("rerender: the interaction changed ({:?} -> {:?}): every element matched again", self.rendered_interaction, self.interaction),
            }
        }
        let rematch = rematch.unwrap_or_default();
        let incremental = render(
            &doc,
            self.images.as_ref(),
            &self.rules,
            self.quirks,
            self.resource_issues,
            self.interaction,
            host,
            &self.scope,
            shaper,
            painter,
            viewport,
            cancel,
            Some(&previous),
            patchable_scene,
            &mut self.animated,
            &mut self.timeline,
            &mut self.containers,
            &restyle,
            &rematch,
            Some(&self.rule_changes),
            &mut progress,
        );
        // What it kept from before failed it (a case the incremental paths
        // missed): rendered whole instead, the page going on, and said.
        let (scene, stylesheet_issues, fallbacks) = match incremental {
            Ok(done) => done,
            Err(error) if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) => return Err(error),
            Err(error) => {
                eprintln!("page: an incremental render failed ({error}): rendered whole");
                render(
                    &doc,
                    self.images.as_ref(),
                    &self.rules,
                    self.quirks,
                    self.resource_issues,
                    self.interaction,
                    host,
                    &self.scope,
                    shaper,
                    painter,
                    viewport,
                    cancel,
                    None,
                    None,
                    &mut self.animated,
                    &mut self.timeline,
                    &mut self.containers,
                    &[],
                    &[],
                    None,
                    &mut progress,
                )?
            }
        };
        self.rendered_interaction = self.interaction;
        let after_render = std::time::Instant::now();
        let mut title = String::new();
        let mut next = Some(doc.root());
        while let Some(node) = next {
            if doc.node_type(node)? == NodeType::Element && doc.name(node)? == Some("title") {
                for child in doc.children(node)? {
                    if let Some(value) = doc.value(child)? {
                        title.push_str(value);
                    }
                }
                break;
            }
            next = next_node(&doc, node)?;
        }
        let after_title = std::time::Instant::now();
        self.document = doc;
        self.scene = scene;
        self.stylesheet_issues = stylesheet_issues;
        self.fallbacks = fallbacks;
        self.title = title;
        if let Some(t) = timing {
            let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.;
            eprintln!(
                "rerender: rebind {rebind_ms:.1} ms, before render {:.1} ms, render {:.1} ms, title {:.1} ms, replaced {:.1} ms",
                ms(before_render - t),
                ms(after_render - before_render),
                ms(after_title - after_render),
                ms(after_title.elapsed())
            );
        }
        Ok(())
    }
    /// Restyle for a new interaction (`interaction` set, the viewport,
    /// media and fonts as the scene was made with): only the elements whose
    /// states flipped and what their selectors reach are matched again,
    /// over the previous styles, layout and scene; `restyle` where that
    /// can't be followed.
    pub fn restyle_interaction(
        &mut self,
        host: &impl Host,
        shaper: &mut shaping::Engine,
        painter: &mut paint::Engine,
        viewport: layout::Viewport,
        cancel: Option<&AtomicBool>,
        mut progress: impl FnMut(Phase, usize),
    ) -> Result<()> {
        for node in [self.interaction.hover, self.interaction.active, self.interaction.focus, self.interaction.target].into_iter().flatten() {
            self.document.node_type(node)?;
        }
        let Some(rematch) = interaction_rematch(&self.document, &self.rules, self.rendered_interaction, self.interaction, self.interaction_mask) else {
            return self.restyle(host, shaper, painter, viewport, cancel, progress);
        };
        if crate::env_on!("OLO_WEB_TIMING") {
            eprintln!("restyle: the interaction changed: {} elements matched again", rematch.len());
        }
        let previous_scene = self.scene.clone();
        let previous = previous_scene.layout().clone();
        let incremental = render(
            &self.document,
            self.images.as_ref(),
            &self.rules,
            self.quirks,
            self.resource_issues,
            self.interaction,
            host,
            &self.scope,
            shaper,
            painter,
            viewport,
            cancel,
            Some(&previous),
            Some(&previous_scene),
            &mut self.animated,
            &mut self.timeline,
            &mut self.containers,
            &[],
            &rematch,
            Some(&self.rule_changes),
            &mut progress,
        );
        let (scene, issues, fallbacks) = match incremental {
            Ok(done) => done,
            Err(error) if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) => return Err(error),
            Err(error) => {
                eprintln!("page: an incremental restyle failed ({error}): restyled whole");
                return self.restyle(host, shaper, painter, viewport, cancel, progress);
            }
        };
        self.rendered_interaction = self.interaction;
        self.scene = scene;
        self.stylesheet_issues = issues;
        self.fallbacks = fallbacks;
        Ok(())
    }
    /// Recompute viewport-dependent styles without network I/O or HTML parsing.
    pub fn restyle(
        &mut self,
        host: &impl Host,
        shaper: &mut shaping::Engine,
        painter: &mut paint::Engine,
        viewport: layout::Viewport,
        cancel: Option<&AtomicBool>,
        mut progress: impl FnMut(Phase, usize),
    ) -> Result<()> {
        for node in [self.interaction.hover, self.interaction.active, self.interaction.focus, self.interaction.target].into_iter().flatten() {
            self.document.node_type(node)?;
        }
        let media = css::media::Media { scripting: self.media.scripting, ..media_for(host, viewport) };
        if self.media_dependent && media != self.media {
            let (rules, dependent, _, _) = compile(&self.sheets, media, cancel)?;
            self.rules = rules;
            self.media_dependent = dependent;
            self.interaction_mask = self.rules.state_mask();
            self.media = media;
        }
        let scope = &self.scope;
        let (scene, issues, fallbacks) = render(
            &self.document,
            self.images.as_ref(),
            &self.rules,
            self.quirks,
            self.resource_issues,
            self.interaction,
            host,
            scope,
            shaper,
            painter,
            viewport,
            cancel,
            None,
            None,
            &mut self.animated,
            &mut self.timeline,
            &mut self.containers,
            &[],
            &[],
            None,
            &mut progress,
        )?;
        self.rendered_interaction = self.interaction;
        self.scene = scene;
        self.stylesheet_issues = issues;
        self.fallbacks = fallbacks;
        Ok(())
    }
}
/// OLO_STYLE_CHECK: style `doc` again from nothing and compare every
/// element's and pseudo-element's style with `styles` (made from an
/// earlier tree), printing the first differences: what an incremental
/// restyle kept must be what a fresh one makes.
fn check_styles<H: Host>(
    doc: &Document,
    rules: &css::cascade::RuleSet,
    defaults: Arc<Defaults>,
    options: style::JobOptions<'_>,
    styles: &StyleTree,
    host: &H,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    if options.previous.is_none() || std::env::var_os("OLO_STYLE_CHECK").is_none() {
        return Ok(());
    }
    let mut job = style::Job::new(doc, rules, defaults, style::JobOptions { previous: None, changed_within: None, ..options }, Default::default(), cancel)?;
    while !job.step(256, |_| host)?.finished {}
    let fresh = job.finish()?;
    let (kept, made): (Vec<_>, Vec<_>) = (styles.iter().collect(), fresh.iter().collect());
    let name =
        |n: crate::dom::NodeId| format!("<{} class={:?}>", doc.name(n).ok().flatten().unwrap_or("?"), doc.attribute(n, "class").ok().flatten().unwrap_or("").chars().take(40).collect::<String>());
    let mut differing = 0usize;
    let path = |n: crate::dom::NodeId| {
        let mut out = Vec::new();
        let mut at = Some(n);
        while let Some(a) = at.filter(|_| out.len() < 12) {
            out.push(format!("{}{}", doc.name(a).ok().flatten().unwrap_or("#"), if doc.shadow_root(a).is_some() { "(host)" } else { "" }));
            at = doc.parent(a).ok().flatten().or_else(|| doc.shadow_host(a));
        }
        out.join(" < ")
    };
    for (i, ((a, x), (b, y))) in kept.iter().zip(&made).enumerate() {
        if a != b {
            eprintln!("style check: entry {i} is {} kept, {} fresh: the trees part here; fresh's is {}", name(*a), name(*b), path(*b));
            differing += 1;
            break;
        }
        // Animated elements take their values after (the animation patch).
        if options.animated.is_some_and(|m| m.contains_key(a)) {
            continue;
        }
        if !x.same_as(y) && x.differs_in_value(y) {
            if differing < 5 {
                let attrs: Vec<String> = doc.attributes_ns(*a).map(|a| a.map(|a| format!("{}={:?}", a.local_name, a.value.chars().take(30).collect::<String>())).collect()).unwrap_or_default();
                eprintln!("style check: {} at {} [{}]", name(*a), path(*a), attrs.join(" "));
                let rules_of = |t: &StyleTree| t.matched(*a).map(|m| m.iter().map(|(id, _)| rules.describe_rule(*id)).collect::<Vec<_>>().join(" | ")).unwrap_or_else(|| "not recorded".into());
                eprintln!("style check:   fresh matched: {}", rules_of(&fresh));
                eprintln!("style check:   kept matched: {}", if options.changed_within.is_some() { "(older rules)".to_owned() } else { rules_of(styles) });
                use css::properties::Property as P;
                let summary = |t: &StyleTree, n: crate::dom::NodeId| {
                    let parent = doc.composed_parent(n).ok().flatten();
                    let s = t.snapshot(style::StyleKey::from(n));
                    let ps = parent.and_then(|p| t.snapshot(style::StyleKey::from(p)));
                    format!(
                        "own position {:?} float {:?}; parent <{}> display {:?}",
                        s.map(|s| s.value(P::Position)),
                        s.map(|s| s.value(P::Float)),
                        parent.and_then(|p| doc.name(p).ok().flatten()).unwrap_or("?"),
                        ps.map(|s| s.value(P::Display))
                    )
                };
                eprintln!("style check:   kept: {}", summary(styles, *a));
                eprintln!("style check:   fresh: {}", summary(&fresh, *a));
            }
            differing += 1;
            if differing <= 5 {
                eprintln!("style check: {} differs: {}", name(*a), x.difference(y));
            }
        }
    }
    eprintln!(
        "style check ({}): {} of {} entries differ{}",
        if options.changed_within.is_some() { "rules changed in shadow trees" } else { "same rules" },
        differing,
        kept.len(),
        if kept.len() != made.len() { format!("; {} entries fresh", made.len()) } else { String::new() }
    );
    Ok(())
}
/// OLO_LAYOUT_CHECK: lay `cache` out again without the previous layout
/// and compare every box's geometry with `layout`'s (which took what it
/// could from the previous one), printing the first differences: the
/// reuse must be invisible.
fn check_reuse(
    shaper: &mut shaping::Engine,
    scope: &Scope,
    cache: Arc<layout::Cache>,
    viewport: layout::Viewport,
    layout: &layout::Layout,
    doc: &Document,
    reused: usize,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    if reused == 0 || std::env::var_os("OLO_LAYOUT_CHECK").is_none() {
        return Ok(());
    }
    let mut job = layout::Job::new(shaper, scope, cache.clone(), viewport, Default::default(), cancel)?;
    while !job.step(256)?.finished {}
    let full = job.finish()?;
    // Twice more: once again as is, once with nothing remembered, to tell
    // a difference of the reuse from one of the layout's own state.
    let mut job = layout::Job::new(shaper, scope, cache.clone(), viewport, Default::default(), cancel)?;
    while !job.step(256)?.finished {}
    let again = job.finish()?;
    let mut job = layout::Job::new(shaper, scope, cache.without_intrinsics(), viewport, Default::default(), cancel)?;
    while !job.step(256)?.finished {}
    let fresh = job.finish()?;
    let tree = layout.cache().tree();
    let count_differing = |x: &layout::Layout, y: &layout::Layout| {
        tree.iter()
            .filter(|(id, _)| match (x.geometry(*id), y.geometry(*id)) {
                (Some(a), Some(c)) => {
                    (a.border_box.x - c.border_box.x).abs() >= 1e-6
                        || (a.border_box.y - c.border_box.y).abs() >= 1e-6
                        || (a.border_box.width - c.border_box.width).abs() >= 1e-6
                        || (a.border_box.height - c.border_box.height).abs() >= 1e-6
                }
                _ => false,
            })
            .count()
    };
    eprintln!(
        "layout check: full vs full again {} differ, full vs fresh (no carried intrinsics) {} differ, reused vs fresh {} differ",
        count_differing(&full, &again),
        count_differing(&full, &fresh),
        count_differing(layout, &fresh)
    );
    let mut differing = std::collections::HashSet::new();
    for (id, _) in tree.iter() {
        let (Some(a), Some(c)) = (layout.geometry(id), full.geometry(id)) else { continue };
        let close = |x: f64, y: f64| (x - y).abs() < 1e-6;
        let same = close(a.border_box.x, c.border_box.x) && close(a.border_box.y, c.border_box.y) && close(a.border_box.width, c.border_box.width) && close(a.border_box.height, c.border_box.height);
        if !same {
            differing.insert(id);
        }
    }
    // The roots of the differences: boxes differing under ones that do not,
    // each with the reused block it lies under.
    let mut shown = 0usize;
    for (id, b) in tree.iter() {
        if !differing.contains(&id) || b.parent.is_some_and(|p| differing.contains(&p)) {
            continue;
        }
        if shown < 6 {
            shown += 1;
            let (Some(a), Some(c)) = (layout.geometry(id), full.geometry(id)) else { continue };
            let mut at = Some(id);
            let mut under = String::from("no reused block");
            while let Some(n) = at {
                if layout.reuse_roots().contains(&n) {
                    let node = tree.get(n).map(|b| b.node);
                    under = format!(
                        "under reused <{} class={:?}> {:?}",
                        node.and_then(|n| doc.name(n).ok().flatten()).unwrap_or("?"),
                        node.and_then(|n| doc.attribute(n, "class").ok().flatten()).unwrap_or("").chars().take(40).collect::<String>(),
                        layout.geometry(n).map(|g| g.border_box)
                    );
                    break;
                }
                at = tree.get(n).and_then(|b| b.parent);
            }
            eprintln!("  layout check: {under}");
            eprintln!(
                "  layout check: <{} class={:?}> {:?} reused {:?} full {:?}",
                doc.name(b.node).ok().flatten().unwrap_or("?"),
                doc.attribute(b.node, "class").ok().flatten().unwrap_or("").chars().take(50).collect::<String>(),
                b.kind,
                a.border_box,
                c.border_box
            );
        }
    }
    // And everything that differs under the first root, with its parent's
    // size: what in there was laid out another way.
    let mut listed = 0usize;
    for (id, b) in tree.iter() {
        if !differing.contains(&id) || listed >= 14 {
            continue;
        }
        listed += 1;
        let (Some(a), Some(c)) = (layout.geometry(id), full.geometry(id)) else { continue };
        eprintln!(
            "    differs: <{} class={:?}> {:?} absolute={} reused {:?} full {:?}",
            doc.name(b.node).ok().flatten().unwrap_or("?"),
            doc.attribute(b.node, "class").ok().flatten().unwrap_or("").chars().take(40).collect::<String>(),
            b.kind,
            b.absolute,
            a.border_box,
            c.border_box
        );
    }
    eprintln!("layout check: {} of {} boxes differ ({reused} reused)", differing.len(), tree.boxes().len());
    Ok(())
}
/// The nodes whose layout the changes to `doc` since `revision` can have
/// altered, with their ancestors (a block subtree under none of them lays
/// out as before, see `layout::Job::with_previous`); None when the
/// changes are not known.
fn dirty_nodes(doc: &Document, revision: u64) -> Option<crate::dom::NodeSet> {
    use crate::dom::Change;
    let changes = doc.changes_since(revision)?;
    if changes.len() > 4096 {
        return None;
    }
    let mut dirty = crate::dom::NodeSet::default();
    let mut nodes = Vec::with_capacity(changes.len());
    for (_, change) in changes {
        nodes.push(match change {
            Change::Node(n) | Change::Attribute { node: n, .. } | Change::Style(n) | Change::Text { node: n, .. } | Change::Children { node: n, .. } | Change::Control(n) => *n,
            Change::Unknown => return None,
        });
    }
    dirty_ancestors(doc, &nodes, &mut dirty);
    Some(dirty)
}
/// The previous layout's box tree with `styles`, when it still stands: the
/// document changed only in styles and in attributes the box tree never
/// reads (no text, no children, no controls), and no style decides other
/// boxes (`Style::keeps_boxes`). Laid out again over it, not built anew
/// (GitHub rebuilt its 2578 boxes, 4.5 ms, for a class flipped as it
/// scrolls). `patched_in_place`: the patch's own verdict, its old styles
/// gone.
fn kept_boxes(previous: &layout::Layout, doc: &Document, revision: u64, styles: &Arc<StyleTree>, patched_in_place: Option<bool>, images: Option<&Arc<crate::image::Set>>) -> Option<layout::Tree> {
    use crate::dom::Change;
    let changes = doc.changes_since(revision)?;
    if changes.len() > 1024 {
        return None;
    }
    let unread = |name: &str| {
        matches!(name, "class" | "id" | "title" | "role" | "tabindex" | "slot" | "part" | "lang" | "translate" | "draggable" | "nonce")
            || name.starts_with("data-")
            || name.starts_with("aria-")
            || name.starts_with("on")
    };
    let why = |what: String| {
        if crate::env_on!("OLO_WEB_TIMING") {
            eprintln!("  boxes built again: {what}");
        }
    };
    let mut texts: Vec<crate::dom::NodeId> = Vec::new();
    let mut holders: Vec<crate::dom::NodeId> = Vec::new();
    for (_, change) in changes {
        match change {
            Change::Style(_) => {}
            Change::Attribute { name, .. } if unread(name) => {}
            // A text's data, not appearing or going: its paragraph's source
            // rewritten in place (`Tree::with_text`), if it can be.
            Change::Text { node, emptiness: false } => {
                if !texts.contains(node) {
                    texts.push(*node);
                }
            }
            // Text children of an element, come or gone (a typed line).
            Change::Children { node, elements: false } => {
                if !holders.contains(node) {
                    holders.push(*node);
                }
            }
            other => {
                why(match other {
                    Change::Attribute { name, .. } => format!("attribute {name}"),
                    Change::Text { .. } => "text".into(),
                    Change::Children { node, elements } => format!("children of <{}>{}", doc.name(*node).ok().flatten().unwrap_or("?"), if *elements { " (elements)" } else { " (text)" }),
                    Change::Control(_) => "a control's value".into(),
                    _ => "a change of another kind".into(),
                });
                return None;
            }
        }
    }
    let old = previous.cache().tree();
    let same_images = match (old.images(), images) {
        (None, None) => true,
        (Some(a), Some(b)) => a.same_layout(b),
        _ => false,
    };
    if !same_images {
        return None;
    }
    let kept = if Arc::ptr_eq(old.styles(), styles) { patched_in_place? } else { styles.keeps_boxes_of(old.styles()) };
    if !kept {
        why("a style decides other boxes".into());
        return None;
    }
    let tree = old.with_styles(styles.clone(), doc, images.cloned());
    let tree = if texts.is_empty() && holders.is_empty() {
        Some(tree)
    } else {
        // (A text node whose holder is rewritten whole needs nothing more.)
        texts.retain(|t| !doc.parent(*t).ok().flatten().is_some_and(|p| holders.contains(&p)));
        let tree = tree.with_text(doc, &texts, &holders);
        if tree.is_none() {
            why("a text not rewritten in place".into());
        }
        tree
    };
    // OLO_LAYOUT_CHECK: the tree as kept or patched against one built anew.
    if let Some(t) = &tree
        && std::env::var_os("OLO_LAYOUT_CHECK").is_some()
        && let Ok(mut job) = layout::BuildJob::new(doc, styles.clone(), Default::default(), None).and_then(|j| match images {
            Some(i) => j.with_images(i.clone()),
            None => Ok(j),
        })
    {
        while job.step(256).is_ok_and(|p| !p.finished) {}
        if let Ok(fresh) = job.finish() {
            eprintln!("box tree check: {}", t.differs_from(&fresh).unwrap_or_else(|| "the same".into()));
        }
    }
    tree
}
/// `nodes` and their ancestors into `dirty`.
fn dirty_ancestors(doc: &Document, nodes: &[crate::dom::NodeId], dirty: &mut crate::dom::NodeSet) {
    for &node in nodes {
        let mut at = Some(node);
        while let Some(n) = at {
            if !dirty.insert(n) {
                break;
            }
            // Out of a shadow tree to its host.
            at = doc.parent(n).ok().flatten().or_else(|| doc.shadow_host(n));
        }
    }
}
/// The elements whose styles the changes to `doc` since `revision` (the
/// styles' document revision) can be patched over, each with whether its
/// rules must be matched again; or why the whole tree must be styled.
/// What a style patch covers: the elements to compute again (each with
/// whether its rules must be matched again), and whether the document
/// must be laid out again regardless (its text changed).
struct Patchable {
    nodes: Vec<(crate::dom::NodeId, bool)>,
    relayout: bool,
}
/// What an interaction change (`before` as styled, `now`) restyles: the
/// elements whose states flipped, what selectors testing those states
/// reach from them (`.card:hover .title`), and the ancestors a `:has()`
/// testing a state can match; each matched again. None where that can't
/// be followed (a `:has()` testing a state beside its subject): every
/// element then. (A hover moving as the page scrolls under the pointer
/// restyled BBC's page whole, 40-60 ms, many times a second.)
/// The elements `down` reaches below `node`, and `beside` reaches among
/// its later siblings and below them: what selectors with a changed key in
/// a compound left of the subject can now match (see `Invalidation`).
fn reached_from(doc: &Document, node: crate::dom::NodeId, down: &css::selectors::Reach, beside: &css::selectors::Reach, out: &mut Vec<crate::dom::NodeId>) {
    let is_element = |n: crate::dom::NodeId| doc.node_type(n).ok() == Some(NodeType::Element);
    let below = |root: crate::dom::NodeId, reach: &css::selectors::Reach, out: &mut Vec<crate::dom::NodeId>| {
        if reach.is_empty() {
            return;
        }
        let mut stack: Vec<crate::dom::NodeId> = doc.children(root).map(|c| c.collect()).unwrap_or_default();
        while let Some(n) = stack.pop() {
            if !is_element(n) {
                continue;
            }
            if reach.reaches(doc, n) {
                out.push(n);
            }
            if let Ok(children) = doc.children(n) {
                stack.extend(children);
            }
        }
    };
    below(node, down, out);
    if !beside.is_empty() {
        let mut sibling = doc.next_sibling(node).ok().flatten();
        while let Some(e) = sibling {
            if is_element(e) {
                if beside.reaches(doc, e) {
                    out.push(e);
                }
                below(e, beside, out);
            }
            sibling = doc.next_sibling(e).ok().flatten();
        }
    }
}
fn interaction_rematch(doc: &Document, rules: &css::cascade::RuleSet, before: Interaction, now: Interaction, mask: u64) -> Option<Vec<(crate::dom::NodeId, bool)>> {
    let has = rules.has_keys();
    if has.interaction_siblings {
        return None;
    }
    let invalidation = rules.invalidation();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut push = |n: crate::dom::NodeId, out: &mut Vec<(crate::dom::NodeId, bool)>| {
        if seen.insert(n) {
            out.push((n, true));
        }
    };
    let is_element = |n: crate::dom::NodeId| doc.node_type(n).ok() == Some(NodeType::Element);
    for (node, key) in before.changed_elements(now, doc, mask) {
        if !is_element(node) || !doc.is_connected(node).unwrap_or(false) {
            continue;
        }
        push(node, &mut out);
        let (down, beside) = invalidation.reach_of(std::iter::once(key.to_owned()));
        let mut reached = Vec::new();
        reached_from(doc, node, &down, &beside, &mut reached);
        // An ancestor's `:has()` testing the state may flip: the ancestor
        // matched again, and what a compound with a `:has()` reaches from
        // it (`body:has(.card:hover) .note`).
        if has.interaction {
            let (has_down, has_beside) = invalidation.reach_of(std::iter::once(":has".to_owned()));
            let mut at = doc.parent(node).ok().flatten();
            while let Some(a) = at {
                if is_element(a) && has.interaction_subjects.reaches(doc, a) {
                    reached.push(a);
                    reached_from(doc, a, &has_down, &has_beside, &mut reached);
                }
                at = doc.parent(a).ok().flatten();
            }
        }
        for n in reached {
            push(n, &mut out);
        }
    }
    Some(out)
}
fn patchable(doc: &Document, rules: &css::cascade::RuleSet, revision: u64) -> std::result::Result<Patchable, String> {
    let name = |n: crate::dom::NodeId| doc.name(n).ok().flatten().unwrap_or("?").to_owned();
    let changes = doc.changes_since(revision).ok_or_else(|| "the changes since are gone".to_owned())?;
    if changes.is_empty() {
        return Err("no changes".to_owned());
    }
    if changes.len() > 256 {
        return Err(format!("{} changes", changes.len()));
    }
    let has_keys = rules.has_keys();
    let invalidation = rules.invalidation();
    let mut relayout = false;
    let mut nodes: Vec<(crate::dom::NodeId, bool)> = Vec::new();
    let mut add = |node, rematch: bool| match nodes.iter_mut().find(|(n, _)| *n == node) {
        Some(entry) => entry.1 |= rematch,
        None => nodes.push((node, rematch)),
    };
    // A change inside an element a `:has()` tests: only its ancestors'
    // `:has()` can flip (no argument has a sibling combinator, else the
    // elements beside them too: not patched), so they are matched again,
    // with what selectors reach from a `:has()` there. (YouTube's style
    // attributes, which a `:has([style])` tests, restyled its page whole,
    // 129 times as it loaded, 365 ms.)
    let (has_down, has_beside) = invalidation.reach_of(std::iter::once(":has".to_owned()));
    // With a sibling combinator in an argument (`:has(+ .x)`, `:has(~ .x
    // .y)`), the elements before it and before each ancestor too: a
    // `:has()` there reaches it through them.
    let has_ancestors = |node: crate::dom::NodeId, reached: &mut Vec<crate::dom::NodeId>| -> bool {
        let element = |n: crate::dom::NodeId| doc.node_type(n).ok() == Some(crate::dom::NodeType::Element);
        let mut subject = |a: crate::dom::NodeId, reached: &mut Vec<crate::dom::NodeId>| {
            reached.push(a);
            reached_from(doc, a, &has_down, &has_beside, reached);
        };
        let mut at = Some(node);
        while let Some(a) = at {
            if has_keys.siblings {
                let mut before = doc.previous_sibling(a).ok().flatten();
                while let Some(b) = before {
                    if element(b) {
                        subject(b, reached);
                    }
                    if reached.len() > 512 {
                        return false;
                    }
                    before = doc.previous_sibling(b).ok().flatten();
                }
            }
            let parent = doc.parent(a).ok().flatten();
            if let Some(p) = parent
                && element(p)
            {
                subject(p, reached);
            }
            if reached.len() > 512 {
                return false;
            }
            at = parent;
        }
        true
    };
    for (_, change) in changes {
        // A detached element (one a script is making) has no style to
        // patch, and nothing in the document follows it.
        if let crate::dom::Change::Style(n) | crate::dom::Change::Attribute { node: n, .. } = change
            && !doc.is_connected(*n).unwrap_or(false)
        {
            continue;
        }
        match change {
            crate::dom::Change::Style(n) => {
                if has_keys.attributes.contains("style") {
                    let mut reached = Vec::new();
                    if !has_ancestors(*n, &mut reached) {
                        return Err(format!("a :has() tests [style]: over 512 elements to match again ({} before giving up)", reached.len()));
                    }
                    for a in reached {
                        add(a, true);
                    }
                }
                // No selector tests the style attribute (the usual case):
                // its change matches nothing anew, whatever keyless
                // compounds (`* + *`, `:first-child`) reach from any change.
                // One that does (`[style*=width] .bar`): what it reaches
                // matched again, where few enough (Reddit restyled whole at
                // each style a script set, 110 ms).
                if invalidation.tests_attribute("style") {
                    let (down, beside) = invalidation.reach_of(std::iter::once("[style".to_owned()));
                    if !down.is_empty() || !beside.is_empty() {
                        let mut reached = Vec::new();
                        reached_from(doc, *n, &down, &beside, &mut reached);
                        if reached.len() > 512 {
                            return Err(format!("<{}> style: selectors reach {} elements from it", name(*n), reached.len()));
                        }
                        for r in reached {
                            add(r, true);
                        }
                        add(*n, true);
                        continue;
                    }
                }
                add(*n, false);
            }
            crate::dom::Change::Attribute { node, name: attribute, old, new } => {
                relayout |= doc.attribute_changes_list_numbering(*node, attribute);
                let keys = style::attribute_keys(attribute, old.as_deref(), new.as_deref());
                if has_keys.attribute_flips(attribute, old.as_deref(), new.as_deref()) || keys.iter().skip(1).any(|k| has_keys.has_key(k)) {
                    let mut reached = Vec::new();
                    if !has_ancestors(*node, &mut reached) {
                        return Err(format!("<{}> {attribute}: a :has() tests it", name(*node)));
                    }
                    for a in reached {
                        add(a, true);
                    }
                }
                let (down, beside) = invalidation.reach_of(keys.into_iter());
                add(*node, true);
                // What selectors reach from it (`.open .item`, `.on + p`):
                // matched again too, where they are few enough to patch
                // (apple.com's menu items' classes, flipped as it loads,
                // restyled the page whole, 10-24 ms each).
                if !down.is_empty() || !beside.is_empty() {
                    let mut reached = Vec::new();
                    reached_from(doc, *node, &down, &beside, &mut reached);
                    if reached.len() > 512 {
                        return Err(format!("<{}> {attribute}: selectors reach {} elements from it", name(*node), reached.len()));
                    }
                    for n in reached {
                        add(n, true);
                    }
                }
            }
            crate::dom::Change::Node(n) => return Err(format!("<{}> attributes", name(*n))),
            // A control's value: its own style matched again (its states), and
            // what it shows laid out again.
            crate::dom::Change::Control(n) => {
                if has_keys.any {
                    return Err(format!("<{}>'s value: a :has() may test it", name(*n)));
                }
                add(*n, true);
                relayout = true;
            }
            // Text alone changes no element's style, bar `:empty` on its
            // parent (matched again) and a `:has()` testing that (not
            // patched), and those only when text appeared or went.
            crate::dom::Change::Text { node: n, emptiness } => {
                if has_keys.empty && *emptiness {
                    return Err(format!("text in <{}>: a :has() tests :empty", doc.parent(*n).ok().flatten().map(name).unwrap_or_default()));
                }
                if *emptiness
                    && rules.uses_empty()
                    && let Some(parent) = doc.parent(*n).ok().flatten()
                {
                    add(parent, true);
                }
                relayout = true;
            }
            crate::dom::Change::Children { node, elements } => {
                if *elements {
                    return Err(format!("children of <{}> (elements)", name(*node)));
                }
                // Text alone: `:empty` on the element may flip, and with it
                // a `:has()` testing that on an ancestor: the element and
                // its ancestors matched again (what a whole job restyles
                // for it; GitHub's ticking counters restyled the page
                // whole, 5-9 ms, every tick).
                if has_keys.empty {
                    if !doc.is_connected(*node).unwrap_or(false) {
                        continue;
                    }
                    // Only those a `:has()` testing `:empty` can match: rematching
                    // <html> and <body> recomputes GitHub's thousands of
                    // custom properties (2 ms each).
                    if rules.uses_empty() {
                        add(*node, true);
                    }
                    let mut at = Some(*node);
                    while let Some(a) = at {
                        if doc.node_type(a).ok() == Some(NodeType::Element) && has_keys.empty_subjects.reaches(doc, a) {
                            add(a, true);
                        }
                        at = doc.composed_parent(a).ok().flatten();
                    }
                } else if rules.uses_empty() {
                    add(*node, true);
                }
                relayout = true;
            }
            crate::dom::Change::Unknown => return Err("an unknown change".to_owned()),
        }
    }
    Ok(Patchable { nodes, relayout })
}
/// Query containers' content-box sizes in a layout (CSS px), by element.
fn container_sizes(layout: &layout::Layout) -> crate::dom::NodeMap<[f64; 2]> {
    (*layout.container_sizes()).clone()
}
/// `render_once`, again when a query container's size came out other than
/// the styles were matched with (`@container` rules then hold elsewhere):
/// styled and laid out once more with the sizes this layout gave, at most
/// twice (a size that flips what holds settles on the second).
#[allow(clippy::too_many_arguments)]
fn render(
    doc: &Document,
    images: Option<&Arc<crate::image::Set>>,
    rules: &css::cascade::RuleSet,
    quirks: bool,
    resource_issues: usize,
    input: Interaction,
    host: &impl Host,
    scope: &Scope,
    shaper: &mut shaping::Engine,
    painter: &mut paint::Engine,
    viewport: layout::Viewport,
    cancel: Option<&AtomicBool>,
    previous: Option<&layout::Layout>,
    previous_scene: Option<&Arc<paint::Scene>>,
    animated: &mut std::collections::HashSet<crate::dom::NodeId>,
    timeline: &mut css::animation::Timeline,
    containers: &mut crate::dom::NodeMap<[f64; 2]>,
    restyle: &[crate::dom::NodeId],
    rematch: &[(crate::dom::NodeId, bool)],
    changes: Option<&RuleChanges>,
    mut progress: impl FnMut(Phase, usize),
) -> Result<(Arc<paint::Scene>, usize, layout::Fallbacks)> {
    let mut result = render_once(
        doc,
        images,
        rules,
        quirks,
        resource_issues,
        input,
        host,
        scope,
        shaper,
        painter,
        viewport,
        cancel,
        previous,
        previous_scene,
        animated,
        timeline,
        containers,
        restyle,
        rematch,
        changes,
        &mut progress,
    )?;
    if !rules.has_containers() {
        return Ok(result);
    }
    for _ in 0..2 {
        let sizes = container_sizes(result.0.layout());
        if sizes == *containers {
            break;
        }
        // What the containers whose size changed hold is matched again; the
        // rest of this pass's styles and boxes are kept.
        let resized: Vec<crate::dom::NodeId> = sizes.iter().filter(|(n, s)| containers.get(n) != Some(s)).map(|(n, _)| *n).collect();
        if crate::env_on!("OLO_WEB_TIMING") {
            eprintln!("render: {} of {} query containers sized anew: what they hold styled again", resized.len(), sizes.len());
        }
        *containers = sizes;
        let first = result.0.clone();
        result = render_once(
            doc,
            images,
            rules,
            quirks,
            resource_issues,
            input,
            host,
            scope,
            shaper,
            painter,
            viewport,
            cancel,
            Some(first.layout()),
            None,
            animated,
            timeline,
            containers,
            &resized,
            &[],
            None,
            &mut progress,
        )?;
    }
    Ok(result)
}
#[allow(clippy::too_many_arguments)]
fn render_once(
    doc: &Document,
    images: Option<&Arc<crate::image::Set>>,
    rules: &css::cascade::RuleSet,
    quirks: bool,
    resource_issues: usize,
    input: Interaction,
    host: &impl Host,
    scope: &Scope,
    shaper: &mut shaping::Engine,
    painter: &mut paint::Engine,
    viewport: layout::Viewport,
    cancel: Option<&AtomicBool>,
    previous: Option<&layout::Layout>,
    previous_scene: Option<&Arc<paint::Scene>>,
    animated: &mut std::collections::HashSet<crate::dom::NodeId>,
    timeline: &mut css::animation::Timeline,
    containers: &crate::dom::NodeMap<[f64; 2]>,
    resized: &[crate::dom::NodeId],
    rematch: &[(crate::dom::NodeId, bool)],
    changes: Option<&RuleChanges>,
    mut progress: impl FnMut(Phase, usize),
) -> Result<(Arc<paint::Scene>, usize, layout::Fallbacks)> {
    // The previous layout's styles stand in only while they were computed
    // against this host (its scope, with the fonts); otherwise they hold
    // fonts from another scope.
    let timing = std::env::var_os("OLO_WEB_TIMING").map(|_| std::time::Instant::now());
    let previous = previous.filter(|p| p.cache().tree().styles().defaults().is_some_and(|d| d.is_current(host)));
    let previous_styles = previous.map(|p| p.cache().tree().styles().as_ref());
    let previous_revision = previous_styles.map(|p| p.document_revision());
    // Rules that differ from the previous styles' in some shadow trees'
    // sheets alone: those styles kept outside them.
    let changed_hosts = previous_styles.zip(changes).and_then(|(p, c)| c.since(p.generation(), rules.generation()));
    let changed_within = previous_styles.map(|p| p.generation()).zip(changed_hosts.as_deref());
    // What each stage substituted (styles, boxes, texts, layout), and
    // whether it did all its work again (see `layout::Recorded`).
    let mut found: [layout::Fallbacks; 4] = Default::default();
    let mut redone = [layout::Redone::All; 4];
    let mut stamps = [0f64; 6];
    let defaults = match previous_styles.and_then(|p| p.defaults()) {
        Some(d) => d,
        None => Defaults::new(host, Default::default(), cancel)?,
    };
    let snapshot = interaction::Snapshot::new(doc, input, cancel);
    if let Some(t) = timing {
        stamps[5] = t.elapsed().as_secs_f64() * 1000.;
    }
    // The changes since the previous scene patched over its styles (as a
    // measurement's are) when they can be; else every element styled anew,
    // taking the previous styles where nothing changed.
    let mut patched: Option<style::Patched> = None;
    let mut styles = None;
    let mut relayout = false;
    // CSS animations at the timeline's moment, from the previous styles'
    // animation lists (a new animation starts the frame after it appears).
    let (mut animation_values, mut animation_nodes) = match previous_styles {
        Some(p) if !rules.keyframes().is_empty() => timeline.sample(rules.keyframes(), animated_elements(doc, p), cancel),
        _ => {
            timeline.running = false;
            (Default::default(), Vec::new())
        }
    };
    let unchanged = previous_revision.is_some_and(|r| doc.changes_since(r).is_some_and(|c| c.is_empty()));
    // Nothing changed in the document, the interaction (a previous scene
    // is passed only for the same one) nor the containers' sizes: the
    // previous styles as they are (an animation frame; its animated
    // elements are patched below).
    if unchanged
        && resized.is_empty()
        && rematch.is_empty()
        && previous_scene.is_some()
        && let Some(tree) = previous_styles.and_then(|p| p.unchanged(doc, rules.generation()))
    {
        styles = Some(Arc::new(tree));
        patched = Some(style::Patched { geometry_kept: true, changed: 0, nodes: Vec::new(), pseudos: 0, boxes_kept: true });
    } else if timing.is_some()
        && let (Some(p), Some(_)) = (previous_styles, previous_scene)
        && let Err(why) = patchable(doc, rules, p.document_revision())
    {
        eprintln!("  render style: not patched: {why}");
    }
    // The document's changes, and the elements an interaction change
    // flipped (`rematch`), patched over the previous styles.
    if styles.is_none()
        && let (Some(p), Some(_)) = (previous_styles, previous_scene)
        && let Some(Patchable { mut nodes, relayout: text_changed }) = match patchable(doc, rules, p.document_revision()) {
            Ok(found) => Some(found),
            Err(_) if unchanged && !rematch.is_empty() => Some(Patchable { nodes: Vec::new(), relayout: false }),
            Err(_) => None,
        }
    {
        for &(n, again) in rematch {
            match nodes.iter_mut().find(|(m, _)| *m == n) {
                Some(entry) => entry.1 |= again,
                None => nodes.push((n, again)),
            }
        }
        relayout = text_changed;
        let copying = std::time::Instant::now();
        let mut tree = p.clone();
        let copied_ms = copying.elapsed().as_secs_f64() * 1000.;
        let options = style::JobOptions {
            quirks,
            states: Some(&snapshot),
            previous: None,
            generation: rules.generation(),
            animated: Some(&animation_values),
            containers: Some(containers),
            resized,
            changed_within: None,
        };
        if let Some(result) = style::Job::patch(doc, rules, defaults.clone(), options, Default::default(), cancel, &mut tree, &nodes, |_| host)? {
            patched = Some(result);
            styles = Some(Arc::new(tree));
        }
        if timing.is_some() {
            eprintln!("  render style: copy {copied_ms:.2} ms, patch {:.2} ms, {} nodes", copying.elapsed().as_secs_f64() * 1000. - copied_ms, nodes.len());
        }
    }
    // An interaction change not patched: every element matched again (the
    // previous styles are of the other interaction).
    let whole = [doc.root()];
    let resized = if styles.is_none() && !rematch.is_empty() { &whole[..] } else { resized };
    let mut styles = match styles {
        Some(styles) => styles,
        None => {
            let options = style::JobOptions {
                quirks,
                states: Some(&snapshot),
                previous: previous_styles,
                generation: rules.generation(),
                animated: Some(&animation_values),
                containers: Some(containers),
                resized,
                changed_within,
            };
            let mut job = style::Job::new(doc, rules, defaults.clone(), options, Default::default(), cancel)?;
            while !job.step(128, |_| host)?.finished {
                progress(Phase::Styles, job.progress().styled)
            }
            let styles = job.finish()?;
            check_styles(doc, rules, defaults.clone(), options, &styles, host, cancel)?;
            Arc::new(styles)
        }
    };
    // CSS transitions (CSS Transitions 1 §3): an element restyled whose
    // transitioned property's value changed goes from the value it showed
    // (its previous style's, mid-transition included) to its new one.
    if let Some(p) = previous_styles {
        let all;
        let nodes: &[crate::dom::NodeId] = match &patched {
            Some(patch) => &patch.nodes,
            None if unchanged => &[],
            None => {
                all = styled_elements(doc, &styles).map(|(n, _)| n).collect::<Vec<_>>();
                &all
            }
        };
        let before = timeline.transition_count();
        start_transitions(timeline, nodes, p, &styles);
        if timing.is_some() && (before > 0 || timeline.transition_count() > 0) {
            eprintln!("  transitions: {} running, {} restyled elements looked at", timeline.transition_count(), nodes.len());
        }
        let (values, touched) = timeline.sample_transitions(cancel);
        for (node, mut declarations) in values {
            // Animations win over transitions: their declarations later.
            let entry = animation_values.entry(node).or_default();
            declarations.append(entry);
            *entry = declarations;
        }
        for n in touched {
            if !animation_nodes.contains(&n) {
                animation_nodes.push(n);
            }
        }
    }
    // Whether the restyle so far left the geometry as it was: the animation
    // patch below says so of its own change only (BBC's menu section,
    // children added in a whole restyle, then a chevron's transition,
    // kept the old layout and stayed shut).
    let restyle_kept_geometry = styles.geometry_kept();
    // A first render (no styles before): its animations from its own styles.
    if previous_styles.is_none() && !rules.keyframes().is_empty() {
        (animation_values, animation_nodes) = timeline.sample(rules.keyframes(), animated_elements(doc, &styles), cancel);
    } else if !rules.keyframes().is_empty() && !unchanged {
        // The document changed: an animation it started shows from the
        // next frame (sampled from these styles).
        timeline.running = true;
    }
    // Scripts' animations (Web Animations: `element.animate()`) over the
    // CSS ones: their declarations come after.
    if !doc.script_animations().is_empty() || timeline.script_applied() {
        let (values, touched) = timeline.sample_script(doc.script_animations(), cancel);
        for (node, mut declarations) in values {
            animation_values.entry(node).or_default().append(&mut declarations);
        }
        for n in touched {
            if !animation_nodes.contains(&n) {
                animation_nodes.push(n);
            }
        }
    }
    // The animated elements styled with their values now (the patch above
    // left those the document did not change as they were).
    if !animation_nodes.is_empty() {
        let copying = std::time::Instant::now();
        let mut tree = (*styles).clone();
        let copied_ms = copying.elapsed().as_secs_f64() * 1000.;
        // Their recorded rules with the animated values over them; matched
        // again only with pseudo-elements, whose rules aren't recorded (on
        // GitHub's 2.5 MB of CSS, matching nine animated elements again
        // each frame took most of a millisecond).
        let nodes: Vec<(crate::dom::NodeId, bool)> = animation_nodes.iter().map(|&n| (n, tree.has_pseudos(n))).collect();
        let options = style::JobOptions {
            quirks,
            states: Some(&snapshot),
            previous: None,
            generation: rules.generation(),
            animated: Some(&animation_values),
            containers: Some(containers),
            resized,
            changed_within: None,
        };
        let patching = std::time::Instant::now();
        let patch = style::Job::patch(doc, rules, defaults.clone(), options, Default::default(), cancel, &mut tree, &nodes, |_| host)?;
        if timing.is_some() {
            let names: Vec<String> = nodes
                .iter()
                .take(4)
                .map(|(n, _)| {
                    format!(
                        "<{}{}>",
                        doc.name(*n).ok().flatten().unwrap_or("?"),
                        doc.attribute(*n, "class").ok().flatten().map(|c| format!(" .{}", c.split_whitespace().take(3).collect::<Vec<_>>().join("."))).unwrap_or_default()
                    )
                })
                .collect();
            eprintln!("  animation style: copy {copied_ms:.2} ms, patch {:.2} ms, {} nodes: {}", patching.elapsed().as_secs_f64() * 1000., nodes.len(), names.join(" "));
        }
        if let Some(result) = patch {
            styles = Arc::new(tree);
            patched = match patched {
                Some(mut p) => {
                    p.geometry_kept &= result.geometry_kept;
                    p.boxes_kept &= result.boxes_kept;
                    p.changed += result.changed;
                    p.pseudos += result.pseudos;
                    for n in result.nodes {
                        if !p.nodes.contains(&n) {
                            p.nodes.push(n);
                        }
                    }
                    Some(p)
                }
                // Nothing else changed: the animation is the whole patch.
                None if unchanged => Some(result),
                None => None,
            };
        }
    }
    if let Some(t) = timing {
        stamps[0] = t.elapsed().as_secs_f64() * 1000.;
    }
    for _ in 0..styles.report().value_fallbacks.min(u32::MAX as usize) {
        found[0].note("declaration dropped at computed-value time", None);
    }
    if previous_styles.is_some() {
        redone[0] = layout::Redone::Part;
    }
    let stylesheet_issues = resource_issues
        + styles.report().stylesheet_issues
        + styles.report().inline_unsupported
        + styles.report().inline_invalid
        + styles.report().presentation_invalid
        + styles.report().presentation_unsupported;
    // A restyle that changed only what is painted, over the same boxes:
    // the previous layout stands, with the new styles.
    // The same elements bound to images (a redrawn canvas is the same box).
    let same_bindings = |p: &layout::Layout| {
        let (a, b) = (p.cache().tree().images(), images);
        match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same_layout(b),
            _ => false,
        }
    };
    let kept = previous.filter(|p| restyle_kept_geometry && styles.geometry_kept() && !relayout && p.device_scale() == viewport.scale && same_bindings(p));
    let kept_layout = kept.is_some();
    // What changed since the previous layout (and what the patch restyled),
    // for taking the rest from it: only when laid out again.
    let dirty = if kept_layout {
        None
    } else {
        previous_revision.and_then(|revision| dirty_nodes(doc, revision)).map(|mut d| {
            if let Some(p) = &patched {
                dirty_ancestors(doc, &p.nodes, &mut d);
            }
            d
        })
    };
    let why_not_kept = match previous {
        _ if kept_layout => "",
        None => "nothing before",
        Some(_) if !styles.geometry_kept() || !restyle_kept_geometry => "geometry changed",
        Some(_) if relayout => "text changed",
        Some(p) if p.device_scale() != viewport.scale => "another scale",
        Some(p) => {
            if crate::env_on!("OLO_WEB_TIMING") {
                let count = |s: Option<&Arc<crate::image::Set>>| s.map(|s| s.bound_nodes().count() as i64).unwrap_or(-1);
                eprintln!("  layout not kept: {} images bound before, {} now", count(p.cache().tree().images()), count(images));
            }
            "other images bound"
        }
    };
    let layout = if let Some(p) = kept {
        redone[1] = layout::Redone::Not;
        redone[2] = layout::Redone::Not;
        redone[3] = layout::Redone::Not;
        if let Some(t) = timing {
            stamps[1] = t.elapsed().as_secs_f64() * 1000.;
            stamps[2] = stamps[1];
        }
        Arc::new(p.with_styles(styles, doc, images.cloned()))
    } else {
        progress(Phase::Boxes, 0);
        let kept = match (previous, previous_revision) {
            (Some(p), Some(revision)) => kept_boxes(p, doc, revision, &styles, None, images),
            _ => None,
        };
        let tree = match kept {
            Some(tree) => {
                redone[1] = layout::Redone::Not;
                Arc::new(tree)
            }
            None => {
                let mut job = layout::BuildJob::new(doc, styles, Default::default(), cancel)?;
                if let Some(images) = images {
                    job = job.with_images(images.clone())?;
                }
                while !job.step(128)?.finished {
                    progress(Phase::Boxes, job.progress().boxes)
                }
                found[1].merge(job.fallbacks());
                job.finish()?
            }
        };
        if let Some(t) = timing {
            stamps[1] = t.elapsed().as_secs_f64() * 1000.;
        }
        progress(Phase::Text, 0);
        let mut job = layout::PrepareJob::new(shaper, scope, tree, Default::default(), cancel)?;
        if let Some(p) = previous {
            job = job.with_previous(p.cache());
            redone[2] = layout::Redone::Part;
        }
        while !job.step(1)?.finished {
            progress(Phase::Text, job.progress().texts)
        }
        found[2].merge(job.fallbacks());
        let texts = (job.progress().reused_texts, job.progress().texts);
        let cache = job.finish()?;
        if let Some(t) = timing {
            stamps[2] = t.elapsed().as_secs_f64() * 1000.;
            eprintln!("  texts: {} of {} reused", texts.0, texts.1);
        }
        progress(Phase::Layout, 0);
        let checked = cache.clone();
        let mut job = layout::Job::new(shaper, scope, cache, viewport, Default::default(), cancel)?;
        if let (Some(p), Some(dirty)) = (previous, &dirty) {
            job = job.with_previous(p, dirty);
            redone[3] = layout::Redone::Part;
        }
        while !job.step(64)?.finished {
            progress(Phase::Layout, job.progress().boxes)
        }
        found[3].merge(job.fallbacks());
        let reused = job.progress().reused;
        let layout = job.finish()?;
        check_reuse(shaper, scope, checked, viewport, &layout, doc, reused, cancel)?;
        Arc::new(layout)
    };
    if let Some(t) = timing {
        stamps[3] = t.elapsed().as_secs_f64() * 1000.;
    }
    // The page's record: this render's findings over the previous one's.
    let stages = std::array::from_fn(|i| (std::mem::take(&mut found[i]), redone[i]));
    let recorded = layout::Recorded::update(previous.map(|p| p.recorded()).as_deref(), stages, (stylesheet_issues, redone[0]));
    layout.set_recorded(recorded.clone());
    let box_count = layout.cache().tree().boxes().len();
    progress(Phase::Paint, 0);
    // Only transforms changed, over the same boxes: the previous scene
    // with its matrices patched, not built again (a carousel sliding, a
    // card growing on hover).
    let mut scene = None;
    let mut transforms_patched = false;
    if let (Some(patched), Some(previous_scene), true) = (&patched, previous_scene, kept_layout)
        && patched.pseudos == 0
    {
        let old = previous_scene.layout().cache().tree();
        let new = layout.cache().tree();
        let mut boxes = Vec::new();
        let only_transforms = patched.nodes.iter().all(|&node| match (old.styles().snapshot(node), new.styles().snapshot(node)) {
            (Some(a), Some(b)) if a.differs_only_in_transform_or_opacity(b) => {
                let before = boxes.len();
                boxes.extend(new.boxes_of(node));
                // An inline element (no box of its own): its opacity is in
                // its glyphs' colours, painted again.
                boxes.len() > before || a.value(css::properties::Property::Opacity).scalar() == b.value(css::properties::Property::Opacity).scalar()
            }
            _ => false,
        });
        // A box whose transform starts animating gets a layer of its own
        // (moved by its translation, nothing repainted): the scene is built
        // once more to give it one; later frames only move it.
        let newly = patched.nodes.iter().filter(|n| !animated.contains(n)).count();
        // The previous scene's layers stand only while it shows every live
        // canvas as a surface (a canvas that became one needs the scene
        // built: a shared frame had no layer to show in, and was refused).
        let live_now = images.map(|i| i.live_nodes().collect::<Vec<_>>()).unwrap_or_default();
        let live_before = previous_scene.layout().cache().tree().images().map(|i| i.live_nodes().collect::<Vec<_>>()).unwrap_or_default();
        let surfaces_stand = live_now == live_before;
        if only_transforms {
            animated.extend(patched.nodes.iter().copied());
            if newly == 0 && surfaces_stand {
                scene = previous_scene.with_transforms(layout.clone(), &boxes, cancel)?;
                transforms_patched = scene.is_some();
            }
        }
        if timing.is_some() && !transforms_patched {
            eprintln!("  scene not patched: only transforms or opacity {only_transforms}, newly animated {newly}, surfaces stand {surfaces_stand}");
        }
    }
    let mut built_ms = timing.map_or(0., |t| t.elapsed().as_secs_f64() * 1000.);
    let scene = match scene {
        Some(scene) => scene,
        None => {
            let options = paint_options(host, doc, &layout)?;
            let mut job = paint::BuildJob::new(doc, layout, painter, options, Default::default(), cancel)?.with_animated(animated.clone());
            while !job.step(32)?.finished {
                progress(Phase::Paint, job.progress().boxes)
            }
            built_ms = timing.map_or(0., |t| t.elapsed().as_secs_f64() * 1000.);
            job.finish()?
        }
    };
    if let Some(t) = timing {
        stamps[4] = t.elapsed().as_secs_f64() * 1000.;
        eprintln!(
            "render: before style {:.1} ms, style {:.1} ms{}, boxes {:.1} ms, text {:.1} ms, layout {:.1} ms, paint {:.1} ms ({} build {:.1}, finish {:.1}; {} boxes)",
            stamps[5],
            stamps[0] - stamps[5],
            match &patched {
                Some(p) => format!(" (patched, {} changed, {} pseudos)", p.changed, p.pseudos),
                None => String::new(),
            },
            stamps[1] - stamps[0],
            stamps[2] - stamps[1],
            stamps[3] - stamps[2],
            stamps[4] - stamps[3],
            if transforms_patched {
                "transforms patched;"
            } else if kept_layout {
                "layout kept;"
            } else {
                why_not_kept
            },
            built_ms - stamps[3],
            stamps[4] - built_ms,
            box_count
        );
    }
    let mut fallbacks = recorded.total();
    fallbacks.merge(scene.fallbacks());
    Ok((scene, recorded.issues, fallbacks))
}

/// The document's elements with their styles in `tree`, in tree order.
/// Transitions for these restyled elements: each transitioned property
/// whose computed value differs from the previous style's starts one (see
/// `Timeline::transition`); `transition-property: all` compares every
/// longhand. Elements without transitions cost one lookup.
fn start_transitions(timeline: &mut css::animation::Timeline, nodes: &[crate::dom::NodeId], before: &style::StyleTree, after: &style::StyleTree) {
    use css::properties::{Display, Property as P};
    use css::style::Value;
    for &node in nodes {
        let (Some(old), Some(new)) = (before.snapshot(node), after.snapshot(node)) else { continue };
        if matches!(new.value(P::Display), Value::Display(Display::None)) || matches!(old.value(P::Display), Value::Display(Display::None)) {
            continue;
        }
        let Value::Animation(names) = new.value(P::TransitionProperty) else { continue };
        if matches!(new.value(P::TransitionDuration), Value::Animation(d) if d.items().iter().all(|d| matches!(d, css::properties::animation::Item::Time(t) if *t <= 0.))) {
            continue;
        }
        let every = names.items().iter().any(|n| matches!(n, css::properties::animation::Item::Name(n) if &**n == "all"));
        let listed: Vec<P> = if every {
            P::ALL.iter().copied().filter(|&p| !old.value(p).same(new.value(p))).collect()
        } else {
            names
                .items()
                .iter()
                .filter_map(|n| if let css::properties::animation::Item::Name(n) = n { css::properties::Name::parse(n) } else { None })
                .flat_map(|n| n.properties().iter().copied())
                .collect()
        };
        for property in listed {
            if matches!(property, P::TransitionProperty | P::TransitionDuration | P::TransitionDelay | P::TransitionTimingFunction | P::Display) {
                continue;
            }
            let Some((duration, delay, easing)) = css::animation::transition_of(new, property) else { continue };
            let (Some(mut from), Some(mut to)) = (computed_value(old, property), computed_value(new, property)) else { continue };
            // `none` as a transform list is the other list's identity (CSS
            // Transforms 1 §9): translate(0, 0), scale(1, 1)...
            if property == P::Transform {
                let identity = |s: &style::Style| match s.value(P::Transform) {
                    Value::Transforms(t) => identity_of(t),
                    _ => None,
                };
                if from == "none" && to != "none" {
                    from = identity(new).unwrap_or(from);
                } else if to == "none" && from != "none" {
                    to = identity(old).unwrap_or(to);
                }
            }
            if timeline.transition_end(node, property) == Some(to.as_str()) || from == to && !timeline.transitioning(node, property) {
                continue;
            }
            timeline.transition(node, property, from, to, duration, delay, easing);
        }
    }
}
/// A transform list's identity, function by function, as text.
fn identity_of(t: &css::properties::Transforms) -> Option<String> {
    use css::properties::TransformFunction as F;
    (!t.0.is_empty()).then(|| {
        t.0.iter()
            .map(|f| match f {
                F::Translate(..) => "translate(0px, 0px)",
                F::Scale(..) => "scale(1, 1)",
                F::Rotate(_) => "rotate(0deg)",
                F::Skew(..) => "skew(0deg, 0deg)",
                F::Matrix(_) => "matrix(1, 0, 0, 1, 0, 0)",
            })
            .collect::<Vec<_>>()
            .join(" ")
    })
}
/// The elements of `tree` with animations that are still in `doc` (the
/// tree may be the previous render's).
fn animated_elements<'t>(doc: &'t Document, tree: &'t style::StyleTree) -> impl Iterator<Item = (crate::dom::NodeId, &'t style::Style)> + 't {
    tree.animation_candidates().filter(move |(node, _)| doc.node_type(*node).ok() == Some(NodeType::Element) && doc.is_connected(*node).unwrap_or(false) && displayed(doc, tree, *node))
}
/// Whether an element is rendered: neither it nor a composed ancestor is
/// `display: none`, and a shadow host's child is taken by a slot. An
/// animation on one that isn't runs no frames, as in Chrome (YouTube's
/// hidden player spinners asked for 60 frames a second while idle).
fn displayed(doc: &Document, tree: &style::StyleTree, node: crate::dom::NodeId) -> bool {
    use css::{
        properties::{Display, Property},
        style::Value,
    };
    let mut at = node;
    for _ in 0..4096 {
        if tree.snapshot(at).is_some_and(|s| matches!(s.value(Property::Display), Value::Display(Display::None))) {
            return false;
        }
        let Some(parent) = doc.parent(at).ok().flatten() else { return true };
        at = if let Some(host) = doc.shadow_host(parent) {
            host
        } else if doc.shadow_root(parent).is_some() {
            match doc.assigned_slot(at).ok().flatten() {
                Some(slot) => slot,
                None => return false,
            }
        } else {
            parent
        };
    }
    true
}
fn styled_elements<'t>(doc: &'t Document, tree: &'t style::StyleTree) -> impl Iterator<Item = (crate::dom::NodeId, &'t style::Style)> + 't {
    let mut next = Some(doc.root());
    std::iter::from_fn(move || {
        while let Some(node) = next {
            next = next_node(doc, node).ok().flatten();
            if doc.node_type(node).ok() == Some(NodeType::Element)
                && let Some(style) = tree.snapshot(node)
            {
                return Some((node, style));
            }
        }
        None
    })
}
pub(crate) fn next_node(doc: &Document, node: crate::dom::NodeId) -> Result<Option<crate::dom::NodeId>> {
    if let Some(child) = doc.first_child(node)? {
        return Ok(Some(child));
    }
    let mut at = node;
    loop {
        if let Some(sibling) = doc.next_sibling(at)? {
            return Ok(Some(sibling));
        }
        if let Some(parent) = doc.parent(at)? {
            at = parent;
        } else {
            return Ok(None);
        }
    }
}
#[cfg(test)]
mod sheet_tests {
    use super::*;
    fn held_compile(name: &str) -> (Vec<Sheet>, css::media::Media, Arc<Compiling>) {
        // Register a job before its worker finishes, without depending
        // on scheduler timing or making a large stylesheet to slow it.
        let sheets = vec![(Arc::new(css::Syntax::parse(Arc::from(format!("body{{height:37px}}/* {name} */")), Default::default(), None).unwrap()), Origin::Author, None, None)];
        let media = css::media::Media::screen(100., 100., 1., false);
        let job = Arc::new(Compiling::default());
        COMPILING.lock().unwrap().push((CompileKey::of(&sheets, media), job.clone()));
        (sheets, media, job)
    }
    #[test]
    fn background_rules_notify_each_live_request_once() {
        let (sheets, media, job) = held_compile("background_rules_notify_each_live_request_once");
        let (tx, rx) = std::sync::mpsc::channel();
        let wake = |id| -> Wake {
            let (tx, job) = (tx.clone(), job.clone());
            Arc::new(move || {
                // A reentrant callback can read the completed result;
                // it never runs with the job's state locked.
                assert!(job.state.try_lock().unwrap().result.is_some());
                tx.send(id).unwrap();
            })
        };
        let mut first = None;
        let mut second = None;
        let mut dropped = None;
        for (id, request) in [(1, &mut first), (2, &mut second), (3, &mut dropped)] {
            assert_eq!(compile_in_background(&sheets, media, &wake(id), request).err().unwrap().to_string(), COMPILING_RULES);
        }
        drop(dropped);
        for _ in 0..20 {
            // Refreshes while waiting replace the callback, without
            // adding twenty subscriptions to one page's request.
            assert_eq!(compile_in_background(&sheets, media, &wake(4), &mut first).err().unwrap().to_string(), COMPILING_RULES);
        }
        let compiled = compile_now(&sheets, media, None).unwrap();
        job.finish(Ok(compiled.clone()));
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), [4, 2]);
        for request in [&mut first, &mut second] {
            let result = compile_in_background(&sheets, media, &wake(5), request).unwrap();
            assert!(Arc::ptr_eq(&result.0, &compiled.0));
        }
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn background_rules_discard_a_superseded_request() {
        let (old, media, old_job) = held_compile("background_rules_discard_a_superseded_request old");
        let (new, _, new_job) = held_compile("background_rules_discard_a_superseded_request new");
        let (tx, rx) = std::sync::mpsc::channel();
        let wake: Wake = Arc::new(move || tx.send(()).unwrap());
        let mut pending = None;
        for sheets in [&old, &new] {
            assert_eq!(compile_in_background(sheets, media, &wake, &mut pending).err().unwrap().to_string(), COMPILING_RULES);
        }
        old_job.finish(compile_now(&old, media, None));
        assert!(rx.try_recv().is_err(), "a superseded request must not wake the page");
        let compiled = compile_now(&new, media, None).unwrap();
        new_job.finish(Ok(compiled.clone()));
        assert_eq!(rx.try_iter().count(), 1);
        assert!(Arc::ptr_eq(&compile_in_background(&new, media, &wake, &mut pending).unwrap().0, &compiled.0));
    }
    #[test]
    fn background_rules_ready_before_subscription_are_taken_immediately() {
        let (sheets, media, job) = held_compile("background_rules_ready_before_subscription_are_taken_immediately");
        let compiled = compile_now(&sheets, media, None).unwrap();
        // The worker has published, but has not yet removed the registry
        // entry. Its cache entry may already have been evicted elsewhere.
        job.state.lock().unwrap().result = Some(Ok(compiled.clone()));
        COMPILED.lock().unwrap().retain(|entry| !entry.key.is(&sheets, media));
        let wake: Wake = Arc::new(|| panic!("a completed request needs no wake"));
        let result = compile_in_background(&sheets, media, &wake, &mut None).unwrap();
        assert!(Arc::ptr_eq(&result.0, &compiled.0));
        job.finish(Ok(compiled));
    }
    #[test]
    fn background_rules_cancel_only_after_the_last_user_leaves() {
        let (sheets, media, job) = held_compile("background_rules_cancel_only_after_the_last_user_leaves");
        let wake: Wake = Arc::new(|| panic!("abandoned requests must not wake"));
        let mut first = None;
        let mut second = None;
        for pending in [&mut first, &mut second] {
            assert_eq!(compile_in_background(&sheets, media, &wake, pending).err().unwrap().to_string(), COMPILING_RULES);
        }
        // Synchronous measurement also owns a use while it waits.
        let measurement = job.claim().unwrap();
        drop(first);
        drop(second);
        assert!(!job.cancel.load(std::sync::atomic::Ordering::Relaxed));
        drop(measurement);
        assert!(job.cancel.load(std::sync::atomic::Ordering::Relaxed));
        assert!(job.claim().is_none(), "an abandoned job cannot be revived halfway through cancellation");
        assert!(!COMPILING.lock().unwrap().iter().any(|(_, c)| Arc::ptr_eq(c, &job)));
        assert!(compile_now(&sheets, media, Some(&job.cancel)).err().unwrap().to_string().ends_with("Cancelled"));
        assert!(compiled_already(&sheets, media).is_none(), "cancelled work must not populate the cache");
        let (tx, rx) = std::sync::mpsc::channel();
        let wake: Wake = Arc::new(move || drop(tx.send(())));
        let mut fresh = None;
        assert_eq!(compile_in_background(&sheets, media, &wake, &mut fresh).err().unwrap().to_string(), COMPILING_RULES);
        assert!(!Arc::ptr_eq(&fresh.as_ref().unwrap().compiling.job, &job));
        rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert!(compile_in_background(&sheets, media, &wake, &mut fresh).is_ok());
    }
    #[test]
    fn background_rules_share_failures_without_retrying_on_the_page() {
        let (sheets, media, job) = held_compile("background_rules_share_failures_without_retrying_on_the_page");
        let wake: Wake = Arc::new(|| {});
        let mut pending = None;
        assert!(compile_in_background(&sheets, media, &wake, &mut pending).is_err());
        job.finish(Err("controlled compilation failure".into()));
        for _ in 0..2 {
            assert_eq!(compile_in_background(&sheets, media, &wake, &mut pending).err().unwrap().to_string(), "controlled compilation failure");
        }
        assert!(compiled_already(&sheets, media).is_none(), "valid sheets were not recompiled synchronously");
    }
    #[test]
    fn a_failed_callback_does_not_strand_other_rule_waiters() {
        let (sheets, media, job) = held_compile("a_failed_callback_does_not_strand_other_rule_waiters");
        let bad: Wake = Arc::new(|| panic!("controlled embedding callback failure"));
        let (tx, rx) = std::sync::mpsc::channel();
        let good: Wake = Arc::new(move || drop(tx.send(())));
        let mut first = None;
        let mut second = None;
        assert!(compile_in_background(&sheets, media, &bad, &mut first).is_err());
        assert!(compile_in_background(&sheets, media, &good, &mut second).is_err());
        job.finish(compile_now(&sheets, media, None));
        rx.try_recv().unwrap();
        assert!(compile_in_background(&sheets, media, &good, &mut second).is_ok());
    }
    #[test]
    fn integrity_takes_the_strongest_digest_and_both_base64_forms() {
        let css = "body{color:red}";
        let sha384 = "8U9HYzsHbf55cFZyiWIE29+QPYQ9WO+U5uT/ViFw0TOwM2Fbbb74ZegzRV/nvwrD";
        let sha256 = "FcQqt3aNlV7AZnGV4zkQRVeCeJOxbMPnQSx258L803E=";
        assert!(integrity_matches(css, &format!("sha384-{sha384}")));
        assert!(integrity_matches(css, &format!("sha256-{sha256}")));
        // URL-safe, unpadded, with options.
        assert!(integrity_matches(css, &format!("sha384-{}?ct=text/css", sha384.replace('+', "-").replace('/', "_"))));
        // The strongest listed decides: a wrong sha384 fails though sha256 is right.
        assert!(!integrity_matches(css, &format!("sha256-{sha256} sha384-AAAA")));
        assert!(!integrity_matches("body{color:blue}", &format!("sha384-{sha384}")));
        // Nothing it knows: as if there were none.
        assert!(integrity_matches(css, "md5-abc"));
        // With a byte order mark as served.
        assert!(integrity_matches(css, &format!("sha256-{sha256}")));
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(under_media("screen and (min-width: 600px)", "a{}"), "@media screen and (min-width: 600px) {\na{}\n}");
        assert_eq!(under_media("x{}", "a{}"), "@media x {\na{}\n}");
    }
}

/// The part of a fragment from source byte `a` to `z` (x from its left, to
/// its right), by its glyphs' clusters and advances, the fragment's
/// advance shared out over them; mirrored in a right-to-left one.
fn fragment_part(text: &crate::text::layout::Layout, f: &crate::text::layout::Fragment, a: u32, z: u32) -> (f64, f64) {
    let Some(run) = text.run(f) else { return (0., f.advance) };
    let unit = run.size / f64::from(run.font.units_per_em());
    let glyphs = run.glyphs.get(f.glyphs.start as usize..f.glyphs.end as usize).unwrap_or(&[]);
    let total: f64 = glyphs.iter().map(|g| f64::from(g.x_advance) * unit).sum();
    let stretch = if total > 0. { f.advance / total } else { 1. };
    let mut x = 0.;
    let (mut l, mut r) = (None, None);
    for g in glyphs {
        if l.is_none() && g.cluster >= a {
            l = Some(x);
        }
        if r.is_none() && g.cluster >= z {
            r = Some(x);
        }
        x += f64::from(g.x_advance) * unit * stretch;
    }
    let (left, right) = (l.unwrap_or(x), r.unwrap_or(x));
    let right = right.max(left);
    if f.level % 2 == 1 { (f.advance - right, f.advance - left) } else { (left, right) }
}
