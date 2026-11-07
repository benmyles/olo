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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    rules: css::cascade::RuleSet,
    /// Every stylesheet in document order (the UA sheet first) with its URL,
    /// kept so a changed medium (viewport, scheme) can recompile the `@media` rules.
    sheets: Vec<(css::Syntax, Origin, Option<Arc<str>>)>,
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
    rules: css::cascade::RuleSet,
    media: css::media::Media,
    media_dependent: bool,
    images: Option<Arc<crate::image::Set>>,
    title: String,
    scripts: usize,
    base: Option<String>,
    resource_issues: usize,
    quirks: bool,
    scope: Scope,
}
impl Prepared {
    pub fn scope(&self) -> &Scope {
        &self.scope
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
    prepare_scripted(source, scope, media, fonts, cancel, progress, resource, None)
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
    scripts: Option<&mut dyn FnMut(Document, bool) -> Result<Document>>,
) -> Result<Prepared> {
    progress(Phase::Html, 0);
    let parsed = html::parse(source, html::ParseOptions { scripting: scripts.is_some(), ..Default::default() }, cancel)?;
    let mut doc = parsed.document;
    if let Some(run) = scripts {
        progress(Phase::Scripts, 0);
        doc = run(doc, parsed.quirks == html::QuirksMode::Quirks)?;
    }
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
    // The ordered small list avoids a second walk of all text/elements. Base
    // discovery is complete before any resource is fetched.
    for node in style_nodes {
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
                    if let Some(resource) = resource(ResourceKind::Stylesheet, base.as_deref(), href)? {
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
    let (rules, media_dependent, faces, background_urls) = compile(&sheets, media, cancel)?;
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
    let mut font_bytes = 0usize;
    if fonts {
        for face in faces.iter().take(FONT_FACES) {
            if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
                return Err(paint::Error::Cancelled.into());
            }
            let Some(css::fonts::Source::Url { url, .. }) = face.sources.iter().find(|s| s.decodable()) else {
                resource_issues += 1;
                continue;
            };
            match resource(ResourceKind::Font, face.base.as_deref().or(base.as_deref()), url)? {
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
    }
    let scope = if loaded.is_empty() { scope.clone() } else { scope.with_faces(loaded, cancel)? };
    let quirks = parsed.quirks == html::QuirksMode::Quirks;
    Ok(Prepared { document: doc, sheets, rules, media, media_dependent, images, title, scripts, base, resource_issues, quirks, scope })
}
/// Style, lay out and paint a prepared document for a host made for its scope.
pub fn finish(
    prepared: Prepared,
    host: &impl Host,
    shaper: &mut shaping::Engine,
    painter: &mut paint::Engine,
    viewport: layout::Viewport,
    cancel: Option<&AtomicBool>,
    mut progress: impl FnMut(Phase, usize),
) -> Result<Page> {
    let Prepared { document: doc, sheets, rules, media, media_dependent, images, title, scripts, base, resource_issues, quirks, scope } = prepared;
    if host.font_context() != scope.context() {
        return Err(style::Error::Stale.into());
    }
    let interaction_mask = rules.state_mask();
    let (scene, stylesheet_issues, fallbacks) = render(&doc, images.as_ref(), &rules, quirks, resource_issues, Interaction::default(), host, &scope, shaper, painter, viewport, cancel, &mut progress)?;
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
    })
}
impl Page {
    /// The fonts this page is styled and shaped with.
    pub fn scope(&self) -> &Scope {
        &self.scope
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
            self.rules = rules;
            self.media_dependent = dependent;
            self.interaction_mask = self.rules.state_mask();
            self.media = media;
        }
        let scope = &self.scope;
        let (scene, issues, fallbacks) =
            render(&self.document, self.images.as_ref(), &self.rules, self.quirks, self.resource_issues, self.interaction, host, scope, shaper, painter, viewport, cancel, &mut progress)?;
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
    mut progress: impl FnMut(Phase, usize),
) -> Result<(Arc<paint::Scene>, usize, layout::Fallbacks)> {
    let mut fallbacks = layout::Fallbacks::default();
    let defaults = Defaults::new(host, Default::default(), cancel)?;
    let snapshot = interaction::Snapshot::new(doc, input, cancel);
    let mut job = style::Job::new(doc, rules, defaults, style::JobOptions { quirks, states: Some(&snapshot), ..Default::default() }, Default::default(), cancel)?;
    while !job.step(128, |_| host)?.finished {
        progress(Phase::Styles, job.progress().styled)
    }
    let styles = Arc::new(job.finish()?);
    for _ in 0..styles.report().value_fallbacks.min(u32::MAX as usize) {
        fallbacks.note("declaration dropped at computed-value time", None);
    }
    let stylesheet_issues = resource_issues
        + styles.report().stylesheet_issues
        + styles.report().inline_unsupported
        + styles.report().inline_invalid
        + styles.report().presentation_invalid
        + styles.report().presentation_unsupported;
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
    progress(Phase::Text, 0);
    let mut job = layout::PrepareJob::new(shaper, scope, tree, Default::default(), cancel)?;
    while !job.step(1)?.finished {
        progress(Phase::Text, job.progress().texts)
    }
    fallbacks.merge(job.fallbacks());
    let cache = job.finish()?;
    progress(Phase::Layout, 0);
    let mut job = layout::Job::new(shaper, scope, cache, viewport, Default::default(), cancel)?;
    while !job.step(64)?.finished {
        progress(Phase::Layout, job.progress().boxes)
    }
    fallbacks.merge(job.fallbacks());
    let layout = Arc::new(job.finish()?);
    progress(Phase::Paint, 0);
    let canvas = host.system_color(css::color::SystemColor::Canvas, host.scheme())?.srgb8_clipped();
    let mut job = paint::BuildJob::new(doc, layout, painter, paint::Options { canvas }, Default::default(), cancel)?;
    while !job.step(32)?.finished {
        progress(Phase::Paint, job.progress().boxes)
    }
    fallbacks.merge(job.fallbacks());
    let scene = job.finish()?;
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
