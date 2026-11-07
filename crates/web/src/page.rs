//! Initial static page pipeline. Run entirely on workers. Scripts are inert;
//! network resources, complete UA defaults and browser runtime remain required.
mod interaction;
use crate::{
    css::{
        self,
        cascade::{Origin, SheetBuilder},
        style::{self, Defaults, Host},
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
    sheets: Vec<(css::Syntax, Origin, Option<Arc<str>>)>,
    base_scope: Scope,
    sheet_ids: Vec<(crate::dom::NodeId, u64)>,
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
    sheets: Vec<(css::Syntax, Origin, Option<Arc<str>>)>,
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
type Compiled = (css::cascade::RuleSet, bool, Vec<css::fonts::FontFace>, Vec<(Arc<str>, Option<Arc<str>>)>);
fn compile(sheets: &[(css::Syntax, Origin, Option<Arc<str>>)], media: css::media::Media, cancel: Option<&AtomicBool>) -> Result<Compiled> {
    let mut builder = SheetBuilder::new(Default::default()).with_media(media);
    for (syntax, origin, base) in sheets {
        builder.add_sheet_from(syntax, *origin, base.clone(), cancel)?;
    }
    let dependent = builder.media_dependent();
    let faces = builder.font_faces().to_vec();
    let backgrounds = builder.background_urls().to_vec();
    Ok((builder.finish(cancel)?, dependent, faces, backgrounds))
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
        if std::env::var_os("OLO_WEB_TIMING").is_some() {
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
        return Ok(second);
    }
    gather(doc, quirks, scope, media, fonts, cancel, &mut progress, &mut load, &mut prefetch, None)
}
/// Whether two sheet lists are the same sheets (origin, URL and text).
fn same_sheets(a: &[(css::Syntax, Origin, Option<Arc<str>>)], b: &[(css::Syntax, Origin, Option<Arc<str>>)]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.1 == y.1 && x.2 == y.2 && x.0.source() == y.0.source())
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
}
impl Measurer {
    /// Follow the document's stylesheets: when the style/link elements
    /// changed since (a sheet a script added, removed or rewrote), load
    /// them, compile the rules and fonts again. Returns whether they did
    /// (then a host made for the old scope is stale).
    pub fn refresh(&mut self, doc: &Document, cancel: Option<&AtomicBool>, resource: &mut impl FnMut(ResourceKind, Option<&str>, &str) -> Result<Option<Resource>>) -> Result<bool> {
        let (ids, base, nodes) = sheet_identities(doc)?;
        if ids == self.sheet_ids {
            return Ok(false);
        }
        let mut sheets = vec![(css::Syntax::parse(Arc::from(UA), Default::default(), cancel)?, Origin::UserAgent, None)];
        let (loaded, _) = load_sheets(doc, &nodes, base.as_deref(), resource, cancel)?;
        sheets.extend(loaded);
        let (rules, _, faces, _) = compile(&sheets, self.media, cancel)?;
        let (fonts, _) = load_fonts(&faces, base.as_deref(), resource, cancel)?;
        self.rules = Arc::new(rules);
        self.scope = if fonts.is_empty() { self.base_scope.clone() } else { self.base_scope.with_faces(fonts, cancel)? };
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
    /// Style and lay out `doc` as it is now.
    pub fn measure(&self, doc: &Document, host: &impl Host, shaper: &mut shaping::Engine, viewport: layout::Viewport, previous: Option<&Measured>, cancel: Option<&AtomicBool>) -> Result<Measured> {
        if host.font_context() != self.scope.context() {
            return Err(style::Error::Stale.into());
        }
        let started = std::time::Instant::now();
        let mut phases = [0f64; 6];
        // The previous measurement's defaults, while the host is the same:
        // styles compare equal only against the same defaults.
        let defaults = match previous.filter(|m| m.defaults.is_current(host)) {
            Some(m) => m.defaults.clone(),
            None => Defaults::new(host, Default::default(), cancel)?,
        };
        let snapshot = interaction::Snapshot::new(doc, Interaction::default(), cancel);
        let previous_styles = previous.map(|m| m.layout.cache().tree().styles().as_ref());
        let mut job = style::Job::new(
            doc,
            &self.rules,
            defaults.clone(),
            style::JobOptions { quirks: self.quirks, states: Some(&snapshot), previous: previous_styles, ..Default::default() },
            Default::default(),
            cancel,
        )?;
        while !job.step(256, |_| host)?.finished {}
        let styles = Arc::new(job.finish()?);
        phases[0] = started.elapsed().as_secs_f64() * 1000.;
        let kept = previous.filter(|p| styles.geometry_kept() && p.viewport == viewport);
        let layout = if let Some(p) = kept {
            Arc::new(p.layout.with_styles(styles))
        } else {
            let mut job = layout::BuildJob::new(doc, styles, Default::default(), cancel)?;
            // The images were bound to the document before the scripts changed
            // it; while they still fit, boxes take their sizes.
            if let Some(images) = &self.images
                && images.validate(doc).is_ok()
            {
                job = job.with_images(images.clone())?;
            }
            while !job.step(256)?.finished {}
            let tree = job.finish()?;
            phases[1] = started.elapsed().as_secs_f64() * 1000. - phases[0];
            let mut job = layout::PrepareJob::new(shaper, &self.scope, tree, Default::default(), cancel)?;
            if let Some(p) = previous {
                job = job.with_previous(p.layout.cache());
            }
            while !job.step(4)?.finished {}
            let cache = job.finish()?;
            phases[2] = started.elapsed().as_secs_f64() * 1000. - phases[0] - phases[1];
            let mut job = layout::Job::new(shaper, &self.scope, cache, viewport, Default::default(), cancel)?;
            while !job.step(256)?.finished {}
            Arc::new(job.finish()?)
        };
        phases[3] = started.elapsed().as_secs_f64() * 1000. - phases[0] - phases[1] - phases[2];
        let mut index = std::collections::HashMap::with_capacity(layout.cache().tree().boxes().len());
        for (id, b) in layout.cache().tree().iter() {
            if b.marker || b.style_node.pseudo.is_some() {
                continue;
            }
            index.entry(b.node).or_insert(id);
        }
        phases[4] = started.elapsed().as_secs_f64() * 1000. - phases[..4].iter().sum::<f64>();
        phases[5] = started.elapsed().as_secs_f64() * 1000.;
        Ok(Measured { layout, index, revision: doc.revision(), viewport, phases, defaults })
    }
}
/// One layout of the document, queried by node.
pub struct Measured {
    layout: Arc<layout::Layout>,
    index: std::collections::HashMap<crate::dom::NodeId, layout::BoxId>,
    revision: u64,
    viewport: layout::Viewport,
    phases: [f64; 6],
    defaults: Arc<Defaults>,
}
impl Measured {
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
    /// The page's height: the root box's.
    pub fn document_size(&self) -> (f64, f64) {
        let root = self.layout.cache().tree().root();
        let r = self.layout.bounds(root).unwrap_or_default();
        (r.width.max(self.viewport.width), r.height.max(self.viewport.height))
    }
    /// A computed (resolved) value as getComputedStyle gives it, for the
    /// properties scripts read: keywords, lengths in px, colours, display,
    /// and the box's used sizes and edges.
    pub fn computed(&self, node: crate::dom::NodeId, name: &str) -> Option<String> {
        use css::properties::{Display, Inner, Keyword as K, Name, Outer, Property as P};
        use css::style::Value;
        let tree = self.layout.cache().tree();
        let style = tree.styles().snapshot(node)?;
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
        Some(match style.value(p) {
            Value::Keyword(k) => k.as_str().to_owned(),
            value @ Value::Numeric(_) => {
                let v = value.scalar()?;
                match p {
                    P::Opacity | P::ZIndex | P::FlexGrow | P::FlexShrink | P::Order | P::FontWeight | P::FillOpacity | P::StrokeOpacity | P::StopOpacity | P::FloodOpacity => trim(v),
                    P::LineHeight => px(v),
                    _ => px(v),
                }
            }
            Value::Color(_) => {
                let c = style.used_color(p).ok()?.srgb8_clipped();
                if c[3] == 255 { format!("rgb({}, {}, {})", c[0], c[1], c[2]) } else { format!("rgba({}, {}, {}, {})", c[0], c[1], c[2], trim(f64::from(c[3]) / 255.)) }
            }
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
/// A number as CSS serialises it: no trailing zeros, at most 3 decimals.
fn trim(v: f64) -> String {
    let r = (v * 1000.).round() / 1000.;
    if r == r.trunc() { format!("{}", r as i64) } else { format!("{r}") }
}
/// The sheets of these `style` and `link` elements, in order, loaded
/// through `resource` (an external one by its href against `base`).
pub type Sheet = (css::Syntax, Origin, Option<Arc<str>>);
fn load_sheets(
    doc: &Document,
    style_nodes: &[crate::dom::NodeId],
    base: Option<&str>,
    resource: &mut impl FnMut(ResourceKind, Option<&str>, &str) -> Result<Option<Resource>>,
    cancel: Option<&AtomicBool>,
) -> Result<(Vec<Sheet>, usize)> {
    let mut sheets = Vec::new();
    let mut css_bytes = 0usize;
    let mut resource_issues = 0usize;
    for &node in style_nodes {
        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
            return Err(paint::Error::Cancelled.into());
        }
        match doc.name(node)? {
            Some("style") => {
                if doc.attribute(node, "type")?.is_some_and(|t| !t.trim().is_empty() && !t.trim().eq_ignore_ascii_case("text/css")) {
                    continue;
                }
                if doc.attribute(node, "media")?.is_some_and(|m| !m.trim().is_empty() && !m.eq_ignore_ascii_case("all")) {
                    resource_issues += 1;
                    continue;
                }
                let mut text = String::new();
                for child in doc.children(node)? {
                    if let Some(value) = doc.value(child)? {
                        css_bytes = css_bytes.checked_add(value.len()).ok_or(paint::Error::Limit)?;
                        if css_bytes > 4 * 1024 * 1024 {
                            return Err(paint::Error::Limit.into());
                        }
                        text.push_str(value)
                    }
                }
                sheets.push((css::Syntax::parse(Arc::from(text), Default::default(), cancel)?, Origin::Author, None));
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
                    if let Some(resource) = resource(ResourceKind::Stylesheet, base, href)? {
                        let Resource::Stylesheet(syntax, url) = resource else { return Err(paint::Error::Invalid.into()) };
                        css_bytes = css_bytes.checked_add(syntax.source().len()).ok_or(paint::Error::Limit)?;
                        if css_bytes > 4 * 1024 * 1024 {
                            return Err(paint::Error::Limit.into());
                        }
                        sheets.push((syntax, Origin::Author, url));
                    } else {
                        resource_issues += 1;
                    }
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
fn sheet_identities(doc: &Document) -> Result<(Vec<(crate::dom::NodeId, u64)>, Option<String>, Vec<crate::dom::NodeId>)> {
    use std::hash::{Hash, Hasher};
    let mut ids = Vec::new();
    let mut nodes = Vec::new();
    let mut base = None;
    let mut next = Some(doc.root());
    while let Some(node) = next {
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
        next = next_node(doc, node)?;
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
    let mut sheets = vec![(css::Syntax::parse(Arc::from(UA), Default::default(), cancel)?, Origin::UserAgent, None)];
    let mut next = Some(doc.root());
    let mut title = String::new();
    let mut scripts = 0;
    let mut css_bytes = 0usize;
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
                    if image_nodes.len() == crate::image::Limits::default().bindings {
                        return Err(paint::Error::Limit.into());
                    }
                    image_nodes.push(node);
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
        let images: Vec<(Option<String>, String)> =
            image_nodes.iter().filter_map(|&n| doc.attribute(n, "src").ok().flatten().filter(|s| !s.trim().is_empty()).map(|s| (base.clone(), s.to_owned()))).collect();
        prefetch(ResourceKind::Image, &images);
    }
    // The ordered small list avoids a second walk of all text/elements. Base
    // discovery is complete before any resource is fetched.
    let (loaded_sheets, sheet_issues) = load_sheets(&doc, &style_nodes, base.as_deref(), &mut resource, cancel)?;
    sheets.extend(loaded_sheets);
    resource_issues += sheet_issues;
    let mut image_bindings = Vec::new();
    let mut image_unique = std::collections::HashSet::new();
    let mut image_bytes = 0usize;
    for node in image_nodes {
        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
            return Err(paint::Error::Cancelled.into());
        }
        if doc.attribute(node, "srcset")?.is_some() || doc.attribute(node, "sizes")?.is_some() {
            resource_issues += 1;
        }
        let Some(src) = doc.attribute(node, "src")?.filter(|s| !s.trim().is_empty()) else {
            resource_issues += 1;
            continue;
        };
        match resource(ResourceKind::Image, base.as_deref(), src)? {
            Some(Resource::Image(image)) => {
                if image_unique.insert(Arc::as_ptr(&image)) {
                    image_bytes = image_bytes.checked_add(image.payload_bytes()).ok_or(paint::Error::Limit)?;
                    if image_bytes > crate::image::Limits::default().bytes {
                        return Err(paint::Error::Limit.into());
                    }
                }
                image_bindings.push((node, image));
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
        None => {
            let (rules, media_dependent, faces, background_urls) = compile(&sheets, media, cancel)?;
            (Arc::new(rules), media_dependent, faces, background_urls)
        }
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
                if image_unique.insert(Arc::as_ptr(&image)) {
                    image_bytes = image_bytes.checked_add(image.payload_bytes()).ok_or(paint::Error::Limit)?;
                    if image_bytes > crate::image::Limits::default().bytes {
                        return Err(paint::Error::Limit.into());
                    }
                }
                backgrounds.insert(url.clone(), image);
            }
            None => resource_issues += 1,
            _ => return Err(paint::Error::Invalid.into()),
        }
    }
    let images = if image_bindings.is_empty() && backgrounds.is_empty() { None } else { Some(crate::image::Set::with_backgrounds(&doc, image_bindings, backgrounds, Default::default(), cancel)?) };
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
        None => base_scope.with_faces(loaded, cancel)?,
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
    let previous = if rules_reused { previous } else { None };
    let (scene, stylesheet_issues, fallbacks) =
        render(&doc, images.as_ref(), &rules, quirks, resource_issues, Interaction::default(), host, &scope, shaper, painter, viewport, cancel, previous, &mut progress)?;
    Ok(Page {
        images,
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
    })
}
impl Page {
    /// Follow a later revision's stylesheets (see `Measurer::refresh`):
    /// returns whether the rules were made again.
    pub fn refresh(&mut self, doc: &Document, cancel: Option<&AtomicBool>, resource: &mut impl FnMut(ResourceKind, Option<&str>, &str) -> Result<Option<Resource>>) -> Result<bool> {
        let (ids, base, nodes) = sheet_identities(doc)?;
        if ids == self.sheet_ids {
            return Ok(false);
        }
        let mut sheets = vec![(css::Syntax::parse(Arc::from(UA), Default::default(), cancel)?, Origin::UserAgent, None)];
        let (loaded, _) = load_sheets(doc, &nodes, base.as_deref(), resource, cancel)?;
        sheets.extend(loaded);
        let (rules, dependent, faces, _) = compile(&sheets, self.media, cancel)?;
        let (fonts, _) = load_fonts(&faces, base.as_deref(), resource, cancel)?;
        self.rules = Arc::new(rules);
        self.media_dependent = dependent;
        self.interaction_mask = self.rules.state_mask();
        self.scope = if fonts.is_empty() { self.base_scope.clone() } else { self.base_scope.with_faces(fonts, cancel)? };
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
        if let Some(images) = &self.images
            && let Some(rebound) = images.rebind(&doc)
        {
            self.images = Some(rebound);
        }
        let media = media_for(host, viewport);
        if self.media_dependent && media != self.media {
            let (rules, dependent, _, _) = compile(&self.sheets, media, cancel)?;
            self.rules = Arc::new(rules);
            self.media_dependent = dependent;
            self.interaction_mask = self.rules.state_mask();
            self.media = media;
        }
        let previous = self.scene.layout().clone();
        let (scene, stylesheet_issues, fallbacks) =
            render(&doc, self.images.as_ref(), &self.rules, self.quirks, self.resource_issues, self.interaction, host, &self.scope, shaper, painter, viewport, cancel, Some(&previous), &mut progress)?;
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
        self.document = doc;
        self.scene = scene;
        self.stylesheet_issues = stylesheet_issues;
        self.fallbacks = fallbacks;
        self.title = title;
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
        let media = media_for(host, viewport);
        if self.media_dependent && media != self.media {
            let (rules, dependent, _, _) = compile(&self.sheets, media, cancel)?;
            self.rules = Arc::new(rules);
            self.media_dependent = dependent;
            self.interaction_mask = self.rules.state_mask();
            self.media = media;
        }
        let scope = &self.scope;
        let (scene, issues, fallbacks) =
            render(&self.document, self.images.as_ref(), &self.rules, self.quirks, self.resource_issues, self.interaction, host, scope, shaper, painter, viewport, cancel, None, &mut progress)?;
        self.scene = scene;
        self.stylesheet_issues = issues;
        self.fallbacks = fallbacks;
        Ok(())
    }
}
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
    mut progress: impl FnMut(Phase, usize),
) -> Result<(Arc<paint::Scene>, usize, layout::Fallbacks)> {
    let previous_styles = previous.map(|p| p.cache().tree().styles().as_ref());
    let mut fallbacks = layout::Fallbacks::default();
    let timing = std::env::var_os("OLO_WEB_TIMING").map(|_| std::time::Instant::now());
    let mut stamps = [0f64; 5];
    let defaults = match previous_styles.and_then(|p| p.defaults()).filter(|d| d.is_current(host)) {
        Some(d) => d,
        None => Defaults::new(host, Default::default(), cancel)?,
    };
    let snapshot = interaction::Snapshot::new(doc, input, cancel);
    let mut job = style::Job::new(doc, rules, defaults, style::JobOptions { quirks, states: Some(&snapshot), previous: previous_styles, ..Default::default() }, Default::default(), cancel)?;
    while !job.step(128, |_| host)?.finished {
        progress(Phase::Styles, job.progress().styled)
    }
    let styles = Arc::new(job.finish()?);
    if let Some(t) = timing {
        stamps[0] = t.elapsed().as_secs_f64() * 1000.;
    }
    for _ in 0..styles.report().value_fallbacks.min(u32::MAX as usize) {
        fallbacks.note("declaration dropped at computed-value time", None);
    }
    let stylesheet_issues = resource_issues
        + styles.report().stylesheet_issues
        + styles.report().inline_unsupported
        + styles.report().inline_invalid
        + styles.report().presentation_invalid
        + styles.report().presentation_unsupported;
    // A restyle that changed only what is painted, over the same boxes:
    // the previous layout stands, with the new styles.
    let kept = previous.filter(|p| styles.geometry_kept() && p.device_scale() == viewport.scale && p.cache().tree().images().map(Arc::as_ptr) == images.map(Arc::as_ptr));
    let layout = if let Some(p) = kept {
        fallbacks.merge(&p.cache().fallbacks());
        Arc::new(p.with_styles(styles))
    } else {
        progress(Phase::Boxes, 0);
        let mut job = layout::BuildJob::new(doc, styles, Default::default(), cancel)?;
        if let Some(images) = images {
            job = job.with_images(images.clone())?;
        }
        while !job.step(128)?.finished {
            progress(Phase::Boxes, job.progress().boxes)
        }
        fallbacks.merge(job.fallbacks());
        let tree = job.finish()?;
        if let Some(t) = timing {
            stamps[1] = t.elapsed().as_secs_f64() * 1000.;
        }
        progress(Phase::Text, 0);
        let mut job = layout::PrepareJob::new(shaper, scope, tree, Default::default(), cancel)?;
        if let Some(p) = previous {
            job = job.with_previous(p.cache());
        }
        while !job.step(1)?.finished {
            progress(Phase::Text, job.progress().texts)
        }
        fallbacks.merge(job.fallbacks());
        let cache = job.finish()?;
        if let Some(t) = timing {
            stamps[2] = t.elapsed().as_secs_f64() * 1000.;
        }
        progress(Phase::Layout, 0);
        let mut job = layout::Job::new(shaper, scope, cache, viewport, Default::default(), cancel)?;
        while !job.step(64)?.finished {
            progress(Phase::Layout, job.progress().boxes)
        }
        fallbacks.merge(job.fallbacks());
        Arc::new(job.finish()?)
    };
    if let Some(t) = timing {
        stamps[3] = t.elapsed().as_secs_f64() * 1000.;
    }
    let box_count = layout.cache().tree().boxes().len();
    progress(Phase::Paint, 0);
    let canvas = host.system_color(css::color::SystemColor::Canvas, host.scheme())?.srgb8_clipped();
    let mut job = paint::BuildJob::new(doc, layout, painter, paint::Options { canvas }, Default::default(), cancel)?;
    while !job.step(32)?.finished {
        progress(Phase::Paint, job.progress().boxes)
    }
    fallbacks.merge(job.fallbacks());
    let scene = job.finish()?;
    if let Some(t) = timing {
        stamps[4] = t.elapsed().as_secs_f64() * 1000.;
        eprintln!(
            "render: style {:.1} ms, boxes {:.1} ms, text {:.1} ms, layout {:.1} ms, paint {:.1} ms ({} boxes)",
            stamps[0],
            stamps[1] - stamps[0],
            stamps[2] - stamps[1],
            stamps[3] - stamps[2],
            stamps[4] - stamps[3],
            box_count
        );
    }
    Ok((scene, stylesheet_issues, fallbacks))
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
