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
}
pub enum Resource {
    Stylesheet(css::Syntax),
    Image(Arc<crate::image::Image>),
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
    rules: css::cascade::RuleSet,
    quirks: bool,
    resource_issues: usize,
}
/// Initial normal-flow defaults, not the complete HTML rendering stylesheet.
pub const UA: &str = "html,body,article,section,nav,aside,header,footer,main,div,p,blockquote,pre,h1,h2,h3,h4,h5,h6{display:block}head,title,base,link,meta,style,script,template,[hidden]{display:none}body{margin:8px}html{color:CanvasText}p,blockquote,pre{margin-top:1em;margin-bottom:1em}h1{font-size:2em;font-weight:bold;margin-top:.67em;margin-bottom:.67em}h2{font-size:1.5em;font-weight:bold;margin-top:.83em;margin-bottom:.83em}a{color:LinkText}pre{white-space:pre}";
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
        ResourceKind::Stylesheet => resource(base, href).map(|s| s.map(Resource::Stylesheet)),
        ResourceKind::Image => Ok(None),
    })
}
/// Typed worker resources. Decoded image allocations are retained across restyle;
/// the caller owns fetch/decode policy and can share repeated URLs.
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
    mut resource: impl FnMut(ResourceKind, Option<&str>, &str) -> Result<Option<Resource>>,
) -> Result<Page> {
    progress(Phase::Html, 0);
    let parsed = html::parse(source, html::ParseOptions { scripting: false, ..Default::default() }, cancel)?;
    let doc = parsed.document;
    progress(Phase::Styles, 0);
    let mut sheets = SheetBuilder::new(Default::default());
    let ua = css::Syntax::parse(Arc::from(UA), Default::default(), cancel)?;
    sheets.add_sheet(&ua, Origin::UserAgent, cancel)?;
    let mut next = Some(doc.root());
    let mut title = String::new();
    let mut scripts = 0;
    let mut css_bytes = 0usize;
    let mut resource_issues = 0;
    let mut base = None;
    let mut style_nodes = Vec::new();
    let mut image_nodes = Vec::new();
    while let Some(node) = next {
        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
            return Err(paint::Error::Cancelled.into());
        }
        if doc.node_type(node)? == NodeType::Element {
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
                let syntax = css::Syntax::parse(Arc::from(text), Default::default(), cancel)?;
                sheets.add_sheet(&syntax, Origin::Author, cancel)?;
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
                        let Resource::Stylesheet(syntax) = resource else { return Err(paint::Error::Invalid.into()) };
                        css_bytes = css_bytes.checked_add(syntax.source().len()).ok_or(paint::Error::Limit)?;
                        if css_bytes > 4 * 1024 * 1024 {
                            return Err(paint::Error::Limit.into());
                        }
                        sheets.add_sheet(&syntax, Origin::Author, cancel)?;
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
    let images = if image_bindings.is_empty() { None } else { Some(crate::image::Set::new(&doc, image_bindings, Default::default(), cancel)?) };
    let rules = sheets.finish(cancel)?;
    let interaction_mask = rules.state_mask();
    let quirks = parsed.quirks == html::QuirksMode::Quirks;
    let (scene, stylesheet_issues) = render(&doc, images.as_ref(), &rules, quirks, resource_issues, Interaction::default(), host, scope, shaper, painter, viewport, cancel, &mut progress)?;
    Ok(Page { images, document: doc, scene, title, scripts, base_href: base, interaction: Interaction::default(), interaction_mask, stylesheet_issues, rules, quirks, resource_issues })
}
impl Page {
    /// Recompute viewport-dependent styles without network I/O or HTML parsing.
    #[allow(clippy::too_many_arguments)]
    pub fn restyle(
        &mut self,
        host: &impl Host,
        scope: &Scope,
        shaper: &mut shaping::Engine,
        painter: &mut paint::Engine,
        viewport: layout::Viewport,
        cancel: Option<&AtomicBool>,
        mut progress: impl FnMut(Phase, usize),
    ) -> Result<()> {
        for node in [self.interaction.hover, self.interaction.active, self.interaction.focus, self.interaction.target].into_iter().flatten() {
            self.document.node_type(node)?;
        }
        let (scene, issues) =
            render(&self.document, self.images.as_ref(), &self.rules, self.quirks, self.resource_issues, self.interaction, host, scope, shaper, painter, viewport, cancel, &mut progress)?;
        self.scene = scene;
        self.stylesheet_issues = issues;
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
) -> Result<(Arc<paint::Scene>, usize)> {
    let defaults = Defaults::new(host, Default::default(), cancel)?;
    let snapshot = interaction::Snapshot::new(doc, input, cancel);
    let mut job = style::Job::new(doc, rules, defaults, style::JobOptions { quirks, states: Some(&snapshot), ..Default::default() }, Default::default(), cancel)?;
    while !job.step(128, |_| host)?.finished {
        progress(Phase::Styles, job.progress().styled)
    }
    let styles = Arc::new(job.finish()?);
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
    let tree = job.finish()?;
    progress(Phase::Text, 0);
    let mut job = layout::PrepareJob::new(shaper, scope, tree, Default::default(), cancel)?;
    while !job.step(1)?.finished {
        progress(Phase::Text, job.progress().texts)
    }
    let cache = job.finish()?;
    progress(Phase::Layout, 0);
    let mut job = layout::Job::new(shaper, scope, cache, viewport, Default::default(), cancel)?;
    while !job.step(64)?.finished {
        progress(Phase::Layout, job.progress().boxes)
    }
    let layout = Arc::new(job.finish()?);
    progress(Phase::Paint, 0);
    let canvas = host.system_color(css::color::SystemColor::Canvas, host.scheme())?.srgb8_clipped();
    let mut job = paint::BuildJob::new(doc, layout, painter, paint::Options { canvas }, Default::default(), cancel)?;
    while !job.step(32)?.finished {
        progress(Phase::Paint, job.progress().boxes)
    }
    let scene = job.finish()?;
    Ok((scene, stylesheet_issues))
}

fn next_node(doc: &Document, node: crate::dom::NodeId) -> Result<Option<crate::dom::NodeId>> {
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
