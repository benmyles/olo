//! Static template insertion modes, and declarative shadow roots
//! (`<template shadowrootmode>`). Content patching, custom-element
//! reactions and script execution remain separate.
use super::{Builder, Insertion, Result, Tag, Token, Tokenizer};

impl<'a> Builder<'a> {
    pub(super) fn start_template(&mut self, tag: &mut Tag<'a>) -> Result<()> {
        let declarative =
            self.fragment_context.is_none() && tag.attributes.iter().any(|a| a.name == "shadowrootmode" && (a.value.eq_ignore_ascii_case("open") || a.value.eq_ignore_ascii_case("closed")));
        // (`<template for>`, the proposed content patching, is an ordinary
        // template here, as in Chrome: Reddit's pages have it.)
        self.formatting_marker()?;
        self.insert(tag)?;
        // A declarative shadow root (HTML §13.2.6.4.7): parsed as a
        // template, made its parent's shadow root once it closes.
        if declarative && let Some(&template) = self.stack.last() {
            self.declarative_shadows.push(template);
        }
        self.frameset_ok = false;
        self.mode = Insertion::Template;
        self.template_modes.push(Insertion::Template);
        self.insertion_flags |= super::TEMPLATE_INSERTION;
        Ok(())
    }
    pub(super) fn close_template(&mut self) {
        if let Some(at) = self.stack.iter().rposition(|&n| self.is(n, "template")) {
            let template = self.stack[at];
            // Thorough implied end tags have the same final tree here:
            // no script runs between them and truncation of this stack.
            self.stack.truncate(at);
            if let Some(i) = self.declarative_shadows.iter().position(|&t| t == template) {
                self.declarative_shadows.swap_remove(i);
                self.attach_declarative_shadow(template);
            }
            self.clear_formatting();
            self.template_modes.pop();
            if self.template_modes.is_empty() {
                self.insertion_flags &= !super::TEMPLATE_INSERTION;
            }
            self.reset_insertion_mode();
        }
    }
    /// The closed `template`'s contents as its parent's shadow root (the
    /// first one asked for; a host has one), and the template gone.
    fn attach_declarative_shadow(&mut self, template: crate::dom::NodeId) {
        let doc = &mut self.document;
        let Ok(Some(host)) = doc.parent(template) else { return };
        if doc.node_type(host).ok() != Some(crate::dom::NodeType::Element) || doc.shadow_root(host).is_some() {
            return;
        }
        let Ok(Some(contents)) = doc.template_contents(template) else { return };
        let Ok(root) = doc.attach_shadow(host) else { return };
        while let Ok(Some(child)) = doc.first_child(contents) {
            if doc.detach(child).is_err() || doc.append_child(root, child).is_err() {
                break;
            }
        }
        let _ = doc.detach(template);
    }
    #[inline(never)]
    pub(super) fn in_template(&mut self, token: &mut Option<Token<'a>>, tokenizer: &mut Tokenizer<'a>) -> Result<bool> {
        let next = match token {
            Some(Token::Text(_) | Token::Doctype(_)) => return self.in_body(token, tokenizer),
            Some(Token::Comment(_) | Token::ProcessingInstruction { .. }) => unreachable!("miscellaneous already handled"),
            Some(Token::StartTag(tag)) => match tag.name.as_ref() {
                "base" | "basefont" | "bgsound" | "link" | "meta" | "noframes" | "script" | "style" | "template" | "title" => return self.in_head(token, tokenizer),
                "caption" | "colgroup" | "tbody" | "tfoot" | "thead" => Insertion::Table,
                "col" => Insertion::ColumnGroup,
                "tr" => Insertion::TableBody,
                "td" | "th" => Insertion::Row,
                _ => Insertion::Body,
            },
            Some(Token::EndTag(tag)) if tag.name == "template" => return self.in_head(token, tokenizer),
            Some(Token::EndTag(_)) => return Ok(false),
            None => {
                if self.stack.iter().any(|&n| self.is(n, "template")) {
                    self.close_template();
                    return Ok(true);
                }
                return Ok(false);
            }
        };
        *self.template_modes.last_mut().expect("in template has a template mode") = next;
        self.mode = next;
        Ok(true)
    }
}
