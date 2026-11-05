//! Standalone inert fragment construction. Import/adoption into an
//! existing live document and its Web IDL hooks remain separate work.
use super::{Builder, Insertion, Mode, ParseOptions, QuirksMode, Result, TEMPLATE_INSERTION, Tokenizer, parse_tokens};
use crate::dom::{self, AttributeRef, Document, Namespace, NodeId};
use std::{
    borrow::Cow,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Immutable context metadata. Its strings borrow the source document;
/// parse_fragment copies/interns what its independent arena needs. Build
/// and parse on a worker; neither operation modifies the source document.
#[derive(Clone)]
pub struct FragmentContext<'a> {
    local_name: &'a str,
    namespace: Namespace,
    attributes: Vec<AttributeRef<'a>>,
    quirks: QuirksMode,
    form_ancestor: bool,
}
impl std::fmt::Debug for FragmentContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FragmentContext").field("namespace", &self.namespace).field("attribute_count", &self.attributes.len()).field("quirks", &self.quirks).finish_non_exhaustive()
    }
}
impl<'a> FragmentContext<'a> {
    /// An unattached, attribute-free context in no-quirks mode.
    pub fn new(local_name: &'a str, namespace: Namespace) -> Self {
        Self { local_name, namespace, attributes: Vec::new(), quirks: QuirksMode::NoQuirks, form_ancestor: false }
    }
    pub fn with_quirks(mut self, quirks: QuirksMode) -> Self {
        self.quirks = quirks;
        self
    }
    pub fn from_element(document: &'a Document, element: NodeId, quirks: QuirksMode, cancelled: Option<&AtomicBool>) -> dom::Result<Self> {
        let namespace = document.namespace(element)?.ok_or(dom::Error::Hierarchy)?;
        let mut context = Self::new(document.name(element)?.expect("element has a name"), namespace).with_quirks(quirks);
        for attribute in document.attributes_ns(element)? {
            if cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Err(dom::Error::Cancelled);
            }
            context.attributes.push(attribute);
        }
        let mut ancestor = Some(element);
        while let Some(node) = ancestor {
            if cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Err(dom::Error::Cancelled);
            }
            if document.namespace(node)? == Some(Namespace::Html) && document.name(node)? == Some("form") {
                context.form_ancestor = true;
                break;
            }
            ancestor = document.parent(node)?;
        }
        Ok(context)
    }
}

#[derive(Debug)]
pub struct ParsedFragment {
    pub document: Document,
    pub fragment: NodeId,
    pub quirks: QuirksMode,
}

/// Complete UTF-8 fragment, scoped to an independent arena. Scripts are
/// inert. The context's namespace, attributes, form ancestry and document
/// mode control tree construction; its actual tree stays untouched.
pub fn parse_fragment(source: Arc<str>, context: &FragmentContext<'_>, options: ParseOptions, cancelled: Option<&AtomicBool>) -> Result<ParsedFragment> {
    let mut tokenizer = Tokenizer::new(&source, options.tokenizer_limits)?;
    if let Some(cancelled) = cancelled {
        tokenizer = tokenizer.with_cancellation(cancelled);
    }
    let mut builder = Builder::new(source.clone(), &source, options, cancelled)?;
    builder.check_cancelled()?;
    builder.quirks = context.quirks;
    let context_node = builder.document.create_element(context.local_name, context.namespace)?;
    builder.document.initial_attributes_ns(
        context_node,
        context.attributes.iter().take_while(|_| !cancelled.is_some_and(|c| c.load(Ordering::Relaxed))).map(|a| (Cow::Borrowed(a.qualified_name), Cow::Borrowed(a.value), a.namespace)),
    )?;
    builder.check_cancelled()?;
    builder.fragment_context = Some(std::num::NonZeroU32::new(context_node.slot() + 1).unwrap());
    builder.foreign_seen = context.namespace != Namespace::Html;
    if context.namespace == Namespace::Html {
        let name = builder.name(context_node);
        tokenizer.set_mode(match name {
            "title" | "textarea" => Mode::Rcdata(String::new()),
            // No appropriate end tag exists in the initial fragment
            // state. HTML explicitly permits this faster equivalent
            // while parse-error diagnostics are not being collected.
            "style" | "xmp" | "iframe" | "noembed" | "noframes" | "script" | "plaintext" => Mode::Plaintext,
            "noscript" if builder.options.scripting => Mode::Plaintext,
            _ => Mode::Data,
        });
        if name == "template" {
            builder.template_modes.push(Insertion::Template);
            builder.insertion_flags |= TEMPLATE_INSERTION;
        }
    }
    let root = builder.document.create_element("html", Namespace::Html)?;
    builder.document.append_child(builder.document.root(), root)?;
    builder.push(root)?;
    builder.reset_insertion_mode();
    let form = if builder.is(context_node, "form") {
        Some(context_node)
    } else if context.form_ancestor {
        Some(builder.document.create_element("form", Namespace::Html)?)
    } else {
        None
    };
    builder.form = form;
    tokenizer.set_allow_cdata(context.namespace != Namespace::Html);
    parse_tokens(&mut builder, &mut tokenizer)?;
    let fragment = builder.document.create_fragment()?;
    while let Some(child) = builder.document.first_child(root)? {
        builder.check_cancelled()?;
        builder.document.append_child(fragment, child)?;
    }
    builder.document.destroy_subtree(root)?;
    builder.document.destroy_subtree(context_node)?;
    if let Some(form) = form
        && form != context_node
    {
        builder.document.destroy_subtree(form)?;
    }
    Ok(ParsedFragment { document: builder.document, fragment, quirks: builder.quirks })
}
