//! Initial static page pipeline. Run entirely on workers. Scripts are inert;
//! network resources, complete UA defaults and browser runtime remain required.
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
    /// A `@font-face` source: WOFF2, WOFF, TrueType or OpenType bytes.
    Font,
}
pub enum Resource {
    /// A stylesheet and the absolute URL it was fetched from, which its
    /// relative `url()`s resolve against.
    Stylesheet(css::Syntax, Option<Arc<str>>),
    Image(Arc<crate::image::Image>),
    Font(Arc<[u8]>),
}
pub struct Page {
    images: Option<Arc<crate::image::Set>>,
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
    /// The document and revision the sheets were last looked for at.
    sheets_checked: Option<(u64, u64)>,
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
pub struct Prepared {
    document: Document,
    sheets: Vec<Sheet>,
    rules: Arc<css::cascade::RuleSet>,
    media: css::media::Media,
    media_dependent: bool,
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
            images: self.images.clone(),
            scope: self.scope.clone(),
            base_scope: self.base_scope.clone(),
            quirks: self.quirks,
            sheet_ids: self.sheet_ids.clone(),
            base: self.base.clone(),
            checked: None,
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
    media: css::media::Media,
    sheets: Vec<(Origin, Option<Arc<str>>, css::SourceHandle, Option<u64>)>,
    compiled: Compiled,
}
thread_local! {
    static COMPILED: std::cell::RefCell<Vec<CompiledEntry>> = const { std::cell::RefCell::new(Vec::new()) };
}
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
fn compile(sheets: &[Sheet], media: css::media::Media, cancel: Option<&AtomicBool>) -> Result<Compiled> {
    let same = |e: &CompiledEntry| {
        e.media == media
            && e.sheets.len() == sheets.len()
            && e.sheets.iter().zip(sheets).all(|((o, u, s, k), (syntax, origin, url, scope))| o == origin && u == url && k == scope && s.len() == syntax.source().len() && **s == *syntax.source())
    };
    if let Some(hit) = COMPILED.with(|c| {
        let mut c = c.borrow_mut();
        let i = c.iter().position(same)?;
        let entry = c.remove(i);
        let compiled = entry.compiled.clone();
        c.insert(0, entry);
        Some(compiled)
    }) {
        return Ok(hit);
    }
    let mut builder = SheetBuilder::new(Default::default()).with_media(media);
    for (syntax, origin, base, scope) in sheets {
        builder.add_scoped_sheet_from(syntax, *origin, base.clone(), *scope, cancel)?;
    }
    let dependent = builder.media_dependent();
    let faces = builder.font_faces().to_vec();
    let backgrounds = builder.background_urls().to_vec();
    let compiled: Compiled = (Arc::new(builder.finish(cancel)?), dependent, faces, backgrounds);
    COMPILED.with(|c| {
        let mut c = c.borrow_mut();
        c.insert(0, CompiledEntry { media, sheets: sheets.iter().map(|(s, o, u, k)| (*o, u.clone(), s.source_handle(), *k)).collect(), compiled: compiled.clone() });
        c.truncate(COMPILED_KEPT);
    });
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
    // With scripting, <noscript> holds its markup as raw text (HTML §13.2):
    // never shown.
    "@media (scripting:enabled){noscript{display:none!important}}",
    // Form controls, as Chrome draws them (HTML §15.5): text fields and
    // buttons in 13.33px Arial, fields white with an inset border, buttons
    // grey with an outset one, text areas monospace; one line where a
    // field is, its text vertically centred by the field's layout.
    "video{object-fit:contain}",
    "input,textarea,select,button{display:inline-block;font:13.3333px Arial,sans-serif;letter-spacing:normal;word-spacing:normal;text-align:start;color:#000;box-sizing:content-box}",
    "input{padding:1px 2px;border:2px inset #767676;background-color:#fff;width:177px;white-space:pre;overflow:clip}",
    "input::placeholder,textarea::placeholder{color:#757575}",
    "textarea{padding:2px;border:1px solid #767676;background-color:#fff;font-family:monospace;width:195px;height:36px;white-space:pre-wrap;overflow:auto}",
    "select{border:1px solid #767676;border-radius:0;background-color:#efefef;padding:0 20px 0 4px;white-space:pre;color:#000}",
    "input[type=submit],input[type=reset],input[type=button],button{padding:1px 6px;border:2px outset #000;background-color:#efefef;color:#000;width:auto;text-align:center;box-sizing:border-box}",
    "input[type=checkbox],input[type=radio]{width:13px;height:13px;padding:0;border:0;margin:3px 3px 3px 4px;background-color:transparent;box-sizing:border-box}",
    "input[type=hidden]{display:none!important}",
    "li{display:list-item}ul,menu,dir{list-style-type:disc;padding-inline-start:40px;margin-block:1em}ol{list-style-type:decimal;padding-inline-start:40px;margin-block:1em}",
    ":is(ul,ol,menu,dir) :is(ul,ol,menu,dir){margin-block:0}:is(ol,ul,menu,dir) ul{list-style-type:circle}:is(ol,ul,menu,dir) :is(ol,ul,menu,dir) ul{list-style-type:square}",
    "table{display:table;border-spacing:2px;box-sizing:border-box}caption{display:table-caption;text-align:center}colgroup{display:table-column-group}col{display:table-column}",
    "thead{display:table-header-group;vertical-align:middle}tbody{display:table-row-group;vertical-align:middle}tfoot{display:table-footer-group;vertical-align:middle}",
    "tr{display:table-row;vertical-align:inherit}td,th{display:table-cell;vertical-align:inherit;padding:1px}th{font-weight:bold;text-align:center}",
    "body{margin:8px}html{color:CanvasText}",
    ":any-link{color:#0000ee;text-decoration:underline}u,ins{text-decoration:underline}s,strike,del{text-decoration:line-through}abbr[title]{text-decoration:underline dotted}",
    "blockquote,figure,dl,ol,ul,menu,dir,p{margin-top:1em;margin-bottom:1em}blockquote,figure{margin-left:40px;margin-right:40px}",
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
        ResourceKind::Stylesheet => resource(base, href).map(|s| s.map(|s| Resource::Stylesheet(s, None))),
        ResourceKind::Image | ResourceKind::Font => Ok(None),
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
    let mut load = |kind: ResourceKind, base: Option<&str>, href: &str| memo.load(kind, base, href, cancel, &mut resource);
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
    Stylesheet(Arc<str>, Option<Arc<str>>),
    Image(Arc<crate::image::Image>),
    Font(Arc<[u8]>),
}
struct Memo {
    entries: std::collections::HashMap<(ResourceKind, Option<String>, String), Memoed>,
    active: bool,
}
impl Memo {
    fn load(
        &mut self,
        kind: ResourceKind,
        base: Option<&str>,
        href: &str,
        cancel: Option<&AtomicBool>,
        resource: &mut impl FnMut(ResourceKind, Option<&str>, &str) -> Result<Option<Resource>>,
    ) -> Result<Option<Resource>> {
        if !self.active {
            return resource(kind, base, href);
        }
        let key = (kind, base.map(str::to_owned), href.to_owned());
        if let Some(m) = self.entries.get(&key) {
            return Ok(match m {
                Memoed::Missing => None,
                Memoed::Stylesheet(source, url) => Some(Resource::Stylesheet(css::Syntax::parse(source.clone(), Default::default(), cancel)?, url.clone())),
                Memoed::Image(image) => Some(Resource::Image(image.clone())),
                Memoed::Font(bytes) => Some(Resource::Font(bytes.clone())),
            });
        }
        let r = resource(kind, base, href)?;
        let m = match &r {
            None => Memoed::Missing,
            Some(Resource::Stylesheet(syntax, url)) => Memoed::Stylesheet(Arc::from(syntax.source()), url.clone()),
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
    images: Option<Arc<crate::image::Set>>,
    scope: Scope,
    base_scope: Scope,
    quirks: bool,
    sheet_ids: Vec<(crate::dom::NodeId, u64)>,
    base: Option<String>,
    /// The document and revision the sheets were last looked for at.
    checked: Option<(u64, u64)>,
}
impl Measurer {
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
        let mut sheets = vec![(css::Syntax::parse(Arc::from(UA), Default::default(), cancel)?, Origin::UserAgent, None, None)];
        let (loaded, _) = load_sheets(doc, &nodes, base.as_deref(), resource, cancel)?;
        sheets.extend(loaded);
        let loaded_at = started.elapsed();
        let (rules, _, faces, _) = compile(&sheets, self.media, cancel)?;
        let compiled_at = started.elapsed();
        let (fonts, _) = load_fonts(&faces, base.as_deref(), resource, cancel)?;
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
        let options = || style::JobOptions { quirks: self.quirks, states: Some(&snapshot), previous: None, generation: self.rules.generation(), animated: None, containers: None, resized: &[] };
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
                let mut job = style::Job::new(doc, &self.rules, defaults.clone(), style::JobOptions { previous: previous_styles, ..options() }, Default::default(), cancel)?;
                while !job.step(256, |_| host)?.finished {}
                Arc::new(job.finish()?)
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
        let kept = previous.as_ref().filter(|p| styles.geometry_kept() && !relayout && p.viewport == viewport);
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
            let mut job = layout::BuildJob::new(doc, styles, Default::default(), cancel)?;
            // The images were bound to the document before the scripts changed
            // it; while they still fit, boxes take their sizes.
            if let Some(images) = &self.images
                && images.validate(doc).is_ok()
            {
                job = job.with_images(images.clone())?;
            }
            while !job.step(256)?.finished {}
            found[1].merge(job.fallbacks());
            let tree = job.finish()?;
            phases[1] = started.elapsed().as_secs_f64() * 1000. - phases[0];
            let mut job = layout::PrepareJob::new(shaper, &self.scope, tree, Default::default(), cancel)?;
            if let Some(p) = previous {
                job = job.with_previous(p.layout.cache());
                redone[2] = layout::Redone::Part;
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
        Ok(Measured { layout, document_size: Default::default(), index, revision: doc.revision(), viewport, phases, defaults, path })
    }
}
/// One layout of the document, queried by node.
pub struct Measured {
    layout: Arc<layout::Layout>,
    document_size: std::sync::OnceLock<(f64, f64)>,
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
    /// The border box in page coordinates (getBoundingClientRect at scroll 0).
    pub fn border_box(&self, node: crate::dom::NodeId) -> Option<layout::Rect> {
        self.geometry(node).map(|(_, r, _)| r)
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
            let mut out = vec![false; n];
            let mut clips = vec![false; n];
            let (mut right, mut bottom) = (self.viewport.width, self.viewport.height);
            for (id, b) in tree.iter() {
                let i = id.index();
                let style = tree.style(id);
                let fixed = style.is_some_and(|s| matches!(s.value(P::Position), Value::Keyword(K::Fixed)));
                out[i] = fixed || b.parent.is_some_and(|p| out[p.index()] || clips[p.index()]);
                // (The root element's and the body's overflow is the
                // viewport's: theirs clips nothing of the page.)
                let top = b.parent.is_none_or(|p| tree.get(p).is_none_or(|pb| pb.parent.is_none() || pb.parent.and_then(|g| tree.get(g)).is_some_and(|gb| gb.parent.is_none())));
                clips[i] = !top && style.is_some_and(|s| !matches!(s.value(P::OverflowX), Value::Keyword(K::Visible)) || !matches!(s.value(P::OverflowY), Value::Keyword(K::Visible)));
                if out[i] || b.parent.is_none() {
                    continue;
                }
                if let (Some(r), Some(g)) = (self.layout.bounds(id), self.layout.geometry(id)) {
                    right = right.max(r.x + r.width + g.margin.right.max(0.));
                    bottom = bottom.max(r.y + r.height + g.margin.bottom.max(0.));
                }
            }
            (right, bottom)
        })
    }
    /// A computed (resolved) value as getComputedStyle gives it, for the
    /// properties scripts read: keywords, lengths in px, colours, display,
    /// and the box's used sizes and edges.
    pub fn computed(&self, node: crate::dom::NodeId, name: &str) -> Option<String> {
        use css::properties::{Keyword as K, Name, Property as P};
        use css::style::Value;
        let tree = self.layout.cache().tree();
        let style = tree.styles().snapshot(node)?;
        // A custom property: its computed value as spelled, substituted
        // (a page's gradient reads its colours from `--gradientColorOne`).
        if name.starts_with("--") {
            return style.custom(name).and_then(|c| c.value().map(|v| v.source_spelling().trim().to_owned()));
        }
        let Name::Longhand(p) = Name::parse(name)? else { return None };
        let g = self.geometry(node);
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
pub type Sheet = (css::Syntax, Origin, Option<Arc<str>>, Option<u64>);
// Within the cascade's work budget (16 M, a unit a source byte), with room
// for the rules and declarations it ticks too.
const CSS_BYTES: usize = 8 * 1024 * 1024;
fn load_sheets(
    doc: &Document,
    style_nodes: &[crate::dom::NodeId],
    base: Option<&str>,
    resource: &mut impl FnMut(ResourceKind, Option<&str>, &str) -> Result<Option<Resource>>,
    cancel: Option<&AtomicBool>,
) -> Result<(Vec<Sheet>, usize)> {
    enum Item<'a> {
        Inline(crate::dom::NodeId),
        Link(&'a str),
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
                if doc.attribute(node, "media")?.is_some_and(|m| !m.trim().is_empty() && !m.eq_ignore_ascii_case("all")) {
                    resource_issues += 1;
                    continue;
                }
                items.push(Item::Inline(node));
            }
            Some("link") if doc.attribute(node, "rel")?.is_some_and(|r| r.split_ascii_whitespace().any(|p| p.eq_ignore_ascii_case("stylesheet"))) => {
                let rel = doc.attribute(node, "rel")?.unwrap_or("");
                let media = doc.attribute(node, "media")?.unwrap_or("").trim();
                let kind = doc.attribute(node, "type")?.unwrap_or("").trim();
                if rel.split_ascii_whitespace().any(|p| p.eq_ignore_ascii_case("alternate")) || doc.attribute(node, "disabled")?.is_some() {
                    // Alternate and disabled sheets are not initially active.
                } else if doc.attribute(node, "integrity")?.is_some_and(|v| !v.trim().is_empty()) {
                    resource_issues += 1; // SRI validation is not implemented yet.
                } else if !media.is_empty() && !media.eq_ignore_ascii_case("all") || !kind.is_empty() && !kind.eq_ignore_ascii_case("text/css") {
                    resource_issues += 1;
                } else if let Some(href) = doc.attribute(node, "href")? {
                    items.push(Item::Link(href));
                }
            }
            _ => {}
        }
    }
    // Where each href is linked last.
    let mut last = std::collections::HashMap::new();
    for (i, item) in items.iter().enumerate() {
        if let Item::Link(href) = item {
            last.insert(href.trim(), i);
        }
    }
    // The sheet at an href's last place, loaded first so an earlier copy
    // can be checked for layers without loading it again.
    let mut loaded: Vec<Option<Option<Resource>>> = (0..items.len()).map(|_| None).collect();
    // A repeated sheet's layer order, by href, for its first place.
    let mut layered = std::collections::HashMap::new();
    for (i, item) in items.iter().enumerate() {
        if let Item::Link(href) = item
            && last.get(href.trim()) == Some(&i)
            && items[..i].iter().any(|e| matches!(e, Item::Link(h) if h.trim() == href.trim()))
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
    for (i, item) in items.iter().enumerate() {
        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
            return Err(paint::Error::Cancelled.into());
        }
        match item {
            Item::Inline(node) => {
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
                sheets.push((css::Syntax::parse(Arc::from(text), Default::default(), cancel)?, Origin::Author, None, None));
            }
            Item::Link(href) => {
                if last.get(href.trim()) != Some(&i) {
                    // Linked again later, where it wins: here only the layers
                    // it names, at their first place.
                    if let Some((statement, url)) = layered.get(href.trim())
                        && first_seen.insert(href.trim())
                    {
                        sheets.push((css::Syntax::parse(Arc::from(statement.as_str()), Default::default(), cancel)?, Origin::Author, url.clone(), None));
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
                        css_bytes += bytes;
                        sheets.push((syntax, Origin::Author, url, None));
                    }
                    Some(_) => return Err(paint::Error::Invalid.into()),
                    None => resource_issues += 1,
                }
            }
        }
    }
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
                if css_bytes.saturating_add(text.len()) > CSS_BYTES {
                    resource_issues += 1;
                    continue;
                }
                css_bytes += text.len();
                sheets.push((css::Syntax::parse(Arc::from(text), Default::default(), cancel)?, Origin::Author, None, scope));
            }
            Some("link") if doc.attribute(node, "rel")?.is_some_and(|r| r.split_ascii_whitespace().any(|p| p.eq_ignore_ascii_case("stylesheet"))) => {
                let Some(href) = doc.attribute(node, "href")? else { continue };
                match resource(ResourceKind::Stylesheet, base, href)? {
                    Some(Resource::Stylesheet(syntax, url)) if css_bytes.saturating_add(syntax.source().len()) <= CSS_BYTES => {
                        css_bytes += syntax.source().len();
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
            resource_issues += 1;
            continue;
        };
        match resource(ResourceKind::Font, face.base.as_deref().or(base), url)? {
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
    let mut sheets = vec![(css::Syntax::parse(Arc::from(UA), Default::default(), cancel)?, Origin::UserAgent, None, None)];
    let mut next = Some(doc.root());
    let mut title = String::new();
    let mut scripts = 0;
    let mut resource_issues = 0;
    let mut base = None;
    let mut style_nodes = Vec::new();
    let mut image_nodes = Vec::new();
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
                Some("img") => {
                    // Past the bindings an image is left out (an issue below).
                    if image_nodes.len() < PAGE_IMAGE_LIMITS.bindings {
                        image_nodes.push(node);
                    } else {
                        resource_issues += 1;
                    }
                }
                Some("script") => scripts += 1,
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
        prefetch(ResourceKind::Stylesheet, &links);
        let images: Vec<(Option<String>, String)> = image_nodes.iter().filter_map(|&n| crate::srcset::choose(&doc, n, &media, cancel).map(|c| (base.clone(), c.url))).collect();
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
        let Some(choice) = crate::srcset::choose(&doc, node, &media, cancel) else {
            resource_issues += 1;
            continue;
        };
        match resource(ResourceKind::Image, base.as_deref(), &choice.url)? {
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
        match resource(ResourceKind::Image, sheet_base.as_deref().or(base.as_deref()), url)? {
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
    Ok(Prepared {
        document: doc,
        sheets,
        rules,
        media,
        media_dependent,
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
    let Prepared { document: doc, sheets, rules, media, media_dependent, images, title, scripts, base, resource_issues, quirks, scope, rules_reused, base_scope, sheet_ids, .. } = prepared;
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
        &mut progress,
    )?;
    Ok(Page {
        images,
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
    })
}
impl Page {
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
        let canvas = host.system_color(css::color::SystemColor::Canvas, host.scheme())?.srgb8_clipped();
        let mut job = paint::BuildJob::new(&self.document, self.scene.layout().clone(), painter, paint::Options { canvas }, Default::default(), cancel)?.with_animated(self.animated.clone());
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
    pub fn refresh(&mut self, doc: &Document, cancel: Option<&AtomicBool>, resource: &mut impl FnMut(ResourceKind, Option<&str>, &str) -> Result<Option<Resource>>) -> Result<bool> {
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
            return Ok(false);
        }
        let mut sheets = vec![(css::Syntax::parse(Arc::from(UA), Default::default(), cancel)?, Origin::UserAgent, None, None)];
        let (loaded, _) = load_sheets(doc, &nodes, base.as_deref(), resource, cancel)?;
        sheets.extend(loaded);
        let (rules, dependent, faces, _) = compile(&sheets, self.media, cancel)?;
        let (fonts, _) = load_fonts(&faces, base.as_deref(), resource, cancel)?;
        self.rules = rules;
        self.media_dependent = dependent;
        self.interaction_mask = self.rules.state_mask();
        self.scope = if fonts.is_empty() { self.base_scope.clone() } else { scope_with_faces(&self.base_scope, fonts, cancel)? };
        self.sheets = sheets;
        self.sheet_ids = ids;
        self.base_href = base;
        Ok(true)
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
            &mut progress,
        )?;
        self.rendered_interaction = self.interaction;
        self.scene = scene;
        self.stylesheet_issues = issues;
        self.fallbacks = fallbacks;
        Ok(())
    }
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
                    return Err("a :has() tests [style]".to_owned());
                }
                // No selector tests the style attribute (the usual case):
                // its change matches nothing anew, whatever keyless
                // compounds (`* + *`, `:first-child`) reach from any change.
                if invalidation.tests_attribute("style") {
                    let (down, beside) = invalidation.reach_of(std::iter::once("[style".to_owned()));
                    if !down.is_empty() || !beside.is_empty() {
                        return Err("a selector reaches from [style]".to_owned());
                    }
                }
                add(*n, false);
            }
            crate::dom::Change::Attribute { node, name: attribute, old, new } => {
                let keys = style::attribute_keys(attribute, old.as_deref(), new.as_deref());
                if has_keys.attribute_flips(attribute, old.as_deref(), new.as_deref()) || keys.iter().skip(1).any(|k| has_keys.has_key(k)) {
                    return Err(format!("<{}> {attribute}: a :has() tests it", name(*node)));
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
    mut progress: impl FnMut(Phase, usize),
) -> Result<(Arc<paint::Scene>, usize, layout::Fallbacks)> {
    // The previous layout's styles stand in only while they were computed
    // against this host (its scope, with the fonts); otherwise they hold
    // fonts from another scope.
    let timing = std::env::var_os("OLO_WEB_TIMING").map(|_| std::time::Instant::now());
    let previous = previous.filter(|p| p.cache().tree().styles().defaults().is_some_and(|d| d.is_current(host)));
    let previous_styles = previous.map(|p| p.cache().tree().styles().as_ref());
    let previous_revision = previous_styles.map(|p| p.document_revision());
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
        patched = Some(style::Patched { geometry_kept: true, changed: 0, nodes: Vec::new(), pseudos: 0 });
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
        let options = style::JobOptions { quirks, states: Some(&snapshot), previous: None, generation: rules.generation(), animated: Some(&animation_values), containers: Some(containers), resized };
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
            let mut job = style::Job::new(
                doc,
                rules,
                defaults.clone(),
                style::JobOptions {
                    quirks,
                    states: Some(&snapshot),
                    previous: previous_styles,
                    generation: rules.generation(),
                    animated: Some(&animation_values),
                    containers: Some(containers),
                    resized,
                },
                Default::default(),
                cancel,
            )?;
            while !job.step(128, |_| host)?.finished {
                progress(Phase::Styles, job.progress().styled)
            }
            Arc::new(job.finish()?)
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
        let options = style::JobOptions { quirks, states: Some(&snapshot), previous: None, generation: rules.generation(), animated: Some(&animation_values), containers: Some(containers), resized };
        let patching = std::time::Instant::now();
        let patch = style::Job::patch(doc, rules, defaults.clone(), options, Default::default(), cancel, &mut tree, &nodes, |_| host)?;
        if timing.is_some() {
            eprintln!("  animation style: copy {copied_ms:.2} ms, patch {:.2} ms, {} nodes", patching.elapsed().as_secs_f64() * 1000., nodes.len());
        }
        if let Some(result) = patch {
            styles = Arc::new(tree);
            patched = match patched {
                Some(mut p) => {
                    p.geometry_kept &= result.geometry_kept;
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
        let mut job = layout::BuildJob::new(doc, styles, Default::default(), cancel)?;
        if let Some(images) = images {
            job = job.with_images(images.clone())?;
        }
        while !job.step(128)?.finished {
            progress(Phase::Boxes, job.progress().boxes)
        }
        found[1].merge(job.fallbacks());
        let tree = job.finish()?;
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
            let canvas = host.system_color(css::color::SystemColor::Canvas, host.scheme())?.srgb8_clipped();
            let mut job = paint::BuildJob::new(doc, layout, painter, paint::Options { canvas }, Default::default(), cancel)?.with_animated(animated.clone());
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
