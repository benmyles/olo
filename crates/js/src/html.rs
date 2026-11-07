//! Serialising and parsing HTML for scripts: `innerHTML`/`outerHTML`
//! (HTML §13.3) and `textContent`, over the engine's document.
use olo_web::{
    dom::{Document, Namespace, NodeId, NodeType},
    html,
};
use std::sync::Arc;
/// Elements whose end tag is never written (HTML §13.1.2).
fn is_void(name: &str) -> bool {
    matches!(name, "area" | "base" | "br" | "col" | "embed" | "hr" | "img" | "input" | "link" | "meta" | "source" | "track" | "wbr")
}
fn escape_text(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\u{a0}' => out.push_str("&nbsp;"),
            c => out.push(c),
        }
    }
}
fn escape_attribute(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\u{a0}' => out.push_str("&nbsp;"),
            c => out.push(c),
        }
    }
}
/// The HTML fragment serialisation of a node's children (`innerHTML`).
pub fn inner_html(doc: &Document, node: NodeId, out: &mut String, budget: &mut usize) -> olo_web::dom::Result<()> {
    let mut child = doc.first_child(node)?;
    while let Some(c) = child {
        serialize(doc, c, out, budget)?;
        child = doc.next_sibling(c)?;
    }
    Ok(())
}
/// A node and its descendants as HTML (`outerHTML`).
pub fn serialize(doc: &Document, node: NodeId, out: &mut String, budget: &mut usize) -> olo_web::dom::Result<()> {
    if out.len() > *budget {
        return Ok(());
    }
    match doc.node_type(node)? {
        NodeType::Element => {
            let name = doc.name(node)?.unwrap_or("");
            out.push('<');
            out.push_str(name);
            for a in doc.attributes_ns(node)? {
                out.push(' ');
                out.push_str(a.qualified_name);
                out.push_str("=\"");
                escape_attribute(a.value, out);
                out.push('"');
            }
            out.push('>');
            let html = doc.namespace(node)? == Some(Namespace::Html);
            if html && is_void(name) {
                return Ok(());
            }
            let raw = html && matches!(name, "style" | "script" | "xmp" | "iframe" | "noembed" | "noframes" | "plaintext" | "noscript");
            // A template's children are its contents fragment's.
            let container = doc.template_contents(node)?.unwrap_or(node);
            let mut child = doc.first_child(container)?;
            while let Some(c) = child {
                if raw && doc.node_type(c)? == NodeType::Text {
                    out.push_str(doc.value(c)?.unwrap_or(""));
                } else {
                    serialize(doc, c, out, budget)?;
                }
                child = doc.next_sibling(c)?;
            }
            out.push_str("</");
            out.push_str(name);
            out.push('>');
        }
        NodeType::Text => escape_text(doc.value(node)?.unwrap_or(""), out),
        NodeType::Comment => {
            out.push_str("<!--");
            out.push_str(doc.value(node)?.unwrap_or(""));
            out.push_str("-->");
        }
        NodeType::ProcessingInstruction => {
            out.push_str("<?");
            out.push_str(doc.name(node)?.unwrap_or(""));
            out.push(' ');
            out.push_str(doc.value(node)?.unwrap_or(""));
            out.push('>');
        }
        NodeType::Doctype => {
            out.push_str("<!DOCTYPE ");
            out.push_str(doc.doctype(node)?.map(|d| d.name).unwrap_or("html"));
            out.push('>');
        }
        NodeType::Document | NodeType::Fragment => inner_html(doc, node, out, budget)?,
    }
    Ok(())
}
/// The concatenated text of a node's descendants (`textContent`).
pub fn text_content(doc: &Document, node: NodeId, out: &mut String, budget: usize) -> olo_web::dom::Result<()> {
    match doc.node_type(node)? {
        NodeType::Text | NodeType::Comment | NodeType::ProcessingInstruction => out.push_str(doc.value(node)?.unwrap_or("")),
        NodeType::Element | NodeType::Fragment | NodeType::Document => {
            let mut child = doc.first_child(node)?;
            while let Some(c) = child {
                if out.len() > budget {
                    break;
                }
                if matches!(doc.node_type(c)?, NodeType::Text | NodeType::Element | NodeType::Fragment) {
                    text_content(doc, c, out, budget)?;
                }
                child = doc.next_sibling(c)?;
            }
        }
        NodeType::Doctype => {}
    }
    Ok(())
}
/// Parse `markup` in the context of `context` (an element of `doc`) and copy
/// the result's nodes into `doc` as children of a new fragment, which is
/// returned (the parser builds a standalone document).
pub fn parse_fragment_into(doc: &mut Document, context: NodeId, markup: &str, quirks: html::QuirksMode) -> olo_web::dom::Result<NodeId> {
    let ctx = html::FragmentContext::from_element(doc, context, quirks, None)?;
    let parsed = html::parse_fragment(Arc::from(markup), &ctx, html::ParseOptions { scripting: true, ..Default::default() }, None).map_err(|_| olo_web::dom::Error::Hierarchy)?;
    let fragment = doc.create_fragment()?;
    let mut child = parsed.document.first_child(parsed.fragment)?;
    while let Some(c) = child {
        copy_subtree(&parsed.document, c, doc, fragment)?;
        child = parsed.document.next_sibling(c)?;
    }
    Ok(fragment)
}
/// Copy `node` from `from` as the last child of `parent` in `to`, recursively.
pub fn copy_subtree(from: &Document, node: NodeId, to: &mut Document, parent: NodeId) -> olo_web::dom::Result<()> {
    let copy = match from.node_type(node)? {
        NodeType::Element => {
            let e = to.create_element(from.name(node)?.unwrap_or("div"), from.namespace(node)?.unwrap_or(Namespace::Html))?;
            for a in from.attributes_ns(node)? {
                to.set_attribute(e, a.qualified_name, a.value)?;
            }
            Some(e)
        }
        NodeType::Text => Some(to.create_text(from.value(node)?.unwrap_or(""))?),
        NodeType::Comment => Some(to.create_comment(from.value(node)?.unwrap_or(""))?),
        _ => None,
    };
    let Some(copy) = copy else { return Ok(()) };
    to.append_child(parent, copy)?;
    let mut child = from.first_child(node)?;
    while let Some(c) = child {
        copy_subtree(from, c, to, copy)?;
        child = from.next_sibling(c)?;
    }
    Ok(())
}
