//! Initial static page pipeline. Run entirely on workers. Scripts are inert;
//! network resources, complete UA defaults and browser runtime remain required.
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
pub struct Page {
    pub document: Document,
    pub scene: Arc<paint::Scene>,
    pub title: String,
    pub scripts: usize,
    pub stylesheet_issues: usize,
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
    mut progress: impl FnMut(Phase, usize),
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
    while let Some(node) = next {
        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
            return Err(paint::Error::Cancelled.into());
        }
        if doc.node_type(node)? == NodeType::Element {
            match doc.name(node)? {
                Some("style") => {
                    if doc.attribute(node, "media")?.is_some_and(|m| !m.trim().is_empty() && !m.eq_ignore_ascii_case("all")) {
                        return Err(paint::Error::Unsupported("stylesheet media contexts").into());
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
                    return Err(paint::Error::Unsupported("external stylesheet loading").into());
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
        if let Some(child) = doc.first_child(node)? {
            next = Some(child)
        } else {
            let mut at = node;
            loop {
                if let Some(sibling) = doc.next_sibling(at)? {
                    next = Some(sibling);
                    break;
                }
                if let Some(parent) = doc.parent(at)? {
                    at = parent
                } else {
                    next = None;
                    break;
                }
            }
        }
    }
    let rules = sheets.finish(cancel)?;
    let defaults = Defaults::new(host, Default::default(), cancel)?;
    let mut job = style::Job::new(&doc, &rules, defaults, style::JobOptions { quirks: parsed.quirks == html::QuirksMode::Quirks, ..Default::default() }, Default::default(), cancel)?;
    while !job.step(128, |_| host)?.finished {
        progress(Phase::Styles, job.progress().styled)
    }
    let styles = Arc::new(job.finish()?);
    let stylesheet_issues = styles.report().stylesheet_issues + styles.report().inline_unsupported + styles.report().inline_invalid;
    progress(Phase::Boxes, 0);
    let mut job = layout::BuildJob::new(&doc, styles, Default::default(), cancel)?;
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
    let mut job = paint::BuildJob::new(&doc, layout, painter, paint::Options { canvas }, Default::default(), cancel)?;
    while !job.step(32)?.finished {
        progress(Phase::Paint, job.progress().boxes)
    }
    let scene = job.finish()?;
    Ok(Page { document: doc, scene, title, scripts, stylesheet_issues })
}
