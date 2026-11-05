//! HTML table insertion modes and the shared foster insertion location.
//! DOM and layout tables are different: these rules repair malformed
//! markup before CSS/layout sees it. Scripts remain inert.
use super::{Builder, Insertion, ParseError, Result, Scope, Token, Tokenizer, dom, synthetic, whitespace};
use crate::dom::NodeId;
use std::borrow::Cow;

impl<'a> Builder<'a> {
    // Keep the optional reference-child and its larger return value out
    // of ordinary text/element insertion. Release measurements caught
    // register spills and a larger stack frame when these paths merged.
    #[inline(never)]
    pub(super) fn insert_adjusted_text(&mut self, current: NodeId, text: Cow<'a, str>) -> Result<()> {
        let (parent, before) = self.insertion_location(current)?;
        let adjacent = if let Some(before) = before { self.document.previous_sibling(before)? } else { self.document.last_child(parent)? };
        if let Some(last) = adjacent
            && self.document.node_type(last)? == crate::dom::NodeType::Text
        {
            self.document.append_value(last, text)?;
            return Ok(());
        }
        let text = self.document.create_text(text)?;
        self.document.insert_before(parent, text, before)?;
        Ok(())
    }
    #[inline(never)]
    pub(super) fn insert_adjusted_node(&mut self, current: NodeId, node: NodeId) -> Result<()> {
        let (parent, before) = self.insertion_location(current)?;
        self.document.insert_before(parent, node, before)?;
        Ok(())
    }

    #[inline]
    pub(super) fn insertion_location(&self, target: NodeId) -> Result<(NodeId, Option<NodeId>)> {
        let (parent, before) = if self.insertion_flags & super::FOSTER_INSERTION != 0 { self.foster_insertion_location(target)? } else { (target, None) };
        if !self.template_modes.is_empty()
            && let Some(contents) = self.document.template_contents(parent)?
        {
            return Ok((contents, None));
        }
        Ok((parent, before))
    }

    fn foster_insertion_location(&self, target: NodeId) -> Result<(NodeId, Option<NodeId>)> {
        if self.document.namespace(target)? != Some(super::Namespace::Html) || !matches!(self.name(target), "table" | "tbody" | "tfoot" | "thead" | "tr") {
            return Ok((target, None));
        }
        let Some((at, &table)) = self.stack.iter().enumerate().rev().find(|(_, node)| self.is(**node, "table") || self.is(**node, "template")) else { return Ok((self.stack[0], None)) };
        if self.is(table, "template") {
            return Ok((table, None));
        }
        if let Some(parent) = self.document.parent(table)? {
            return Ok((parent, Some(table)));
        }
        Ok((self.stack[at.saturating_sub(1)], None))
    }

    fn clear_table_context(&mut self, names: &[&str]) {
        while self.stack.len() > 1 && !names.iter().any(|name| self.is(self.current(), name)) {
            self.stack.pop();
        }
    }

    pub(super) fn reset_insertion_mode(&mut self) {
        for &node in self.stack.iter().rev() {
            if self.document.namespace(node).unwrap() != Some(super::Namespace::Html) {
                continue;
            }
            self.mode = match self.name(node) {
                "td" | "th" => Insertion::Cell,
                "tr" => Insertion::Row,
                "tbody" | "tfoot" | "thead" => Insertion::TableBody,
                "caption" => Insertion::Caption,
                "colgroup" => Insertion::ColumnGroup,
                "table" => Insertion::Table,
                "template" => *self.template_modes.last().expect("template on stack has a mode"),
                "head" => Insertion::Head,
                "body" => Insertion::Body,
                "frameset" => Insertion::Frameset,
                "html" => {
                    if self.head.is_some() {
                        Insertion::AfterHead
                    } else {
                        Insertion::BeforeHead
                    }
                }
                _ => continue,
            };
            return;
        }
        self.mode = Insertion::Body;
    }

    fn table_fallback(&mut self, token: &mut Option<Token<'a>>, tokenizer: &mut Tokenizer<'a>) -> Result<bool> {
        let previous = self.insertion_flags & super::FOSTER_INSERTION;
        self.insertion_flags |= super::FOSTER_INSERTION;
        let result = self.in_body(token, tokenizer);
        self.insertion_flags = (self.insertion_flags & !super::FOSTER_INSERTION) | previous;
        result
    }

    fn in_table(&mut self, token: &mut Option<Token<'a>>, tokenizer: &mut Tokenizer<'a>) -> Result<bool> {
        match token {
            Some(Token::Text(_))
                if self.document.namespace(self.current())? == Some(super::Namespace::Html) && matches!(self.name(self.current()), "table" | "tbody" | "template" | "tfoot" | "thead" | "tr") =>
            {
                self.pending_table_text = Cow::Borrowed("");
                self.original_mode = self.mode;
                self.mode = Insertion::TableText;
                return Ok(true);
            }
            Some(Token::Doctype(_)) => {}
            Some(Token::StartTag(tag)) => match tag.name.as_ref() {
                "caption" => {
                    self.clear_table_context(&["table", "template", "html"]);
                    self.formatting_marker()?;
                    self.insert(tag)?;
                    self.mode = Insertion::Caption;
                }
                "colgroup" => {
                    self.clear_table_context(&["table", "template", "html"]);
                    self.insert(tag)?;
                    self.mode = Insertion::ColumnGroup;
                }
                "col" => {
                    self.clear_table_context(&["table", "template", "html"]);
                    self.insert(&mut synthetic("colgroup"))?;
                    self.mode = Insertion::ColumnGroup;
                    return Ok(true);
                }
                "tbody" | "tfoot" | "thead" => {
                    self.clear_table_context(&["table", "template", "html"]);
                    self.insert(tag)?;
                    self.mode = Insertion::TableBody;
                }
                "td" | "th" | "tr" => {
                    self.clear_table_context(&["table", "template", "html"]);
                    self.insert(&mut synthetic("tbody"))?;
                    self.mode = Insertion::TableBody;
                    return Ok(true);
                }
                "table" => {
                    if let Some(at) = self.scope(&["table"], Scope::Table) {
                        self.stack.truncate(at);
                        self.reset_insertion_mode();
                        return Ok(true);
                    }
                }
                "style" | "script" | "template" => return self.in_head(token, tokenizer),
                "input" if tag.attributes.iter().any(|a| a.name == "type" && a.value.eq_ignore_ascii_case("hidden")) => {
                    self.insert(tag)?;
                    self.stack.pop();
                }
                "form" => {
                    if self.form.is_none() || !self.template_modes.is_empty() {
                        let form = self.insert(tag)?;
                        if self.template_modes.is_empty() {
                            self.form = Some(form);
                        }
                        self.stack.pop();
                    }
                }
                _ => return self.table_fallback(token, tokenizer),
            },
            Some(Token::EndTag(tag)) => match tag.name.as_ref() {
                "table" => {
                    if let Some(at) = self.scope(&["table"], Scope::Table) {
                        self.stack.truncate(at);
                        self.reset_insertion_mode();
                    }
                }
                "body" | "caption" | "col" | "colgroup" | "html" | "tbody" | "td" | "tfoot" | "th" | "thead" | "tr" => {}
                "template" => return self.in_head(token, tokenizer),
                _ => return self.table_fallback(token, tokenizer),
            },
            None => return self.in_body(token, tokenizer),
            _ => return self.table_fallback(token, tokenizer),
        }
        Ok(false)
    }

    // Keep contiguous source runs borrowed; retain at most one growing
    // buffer for decoded/discontiguous runs, charged against the text quota.
    fn buffer_table_text(&mut self, text: Cow<'a, str>) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let available = self.options.document_limits.owned_text_bytes.saturating_sub(self.document.memory().owned_text_bytes);
        if self.pending_table_text.is_empty() {
            if let Cow::Owned(value) = &text
                && value.capacity() > available
            {
                return Err(ParseError::Document(dom::Error::TextLimit));
            }
            self.pending_table_text = text;
            return Ok(());
        }
        if let (Cow::Borrowed(old), Cow::Borrowed(next)) = (&self.pending_table_text, &text)
            && old.as_ptr() as usize + old.len() == next.as_ptr() as usize
            && let Some(start) = (old.as_ptr() as usize).checked_sub(self.source_text.as_ptr() as usize)
            && let Some(combined) = self.source_text.get(start..start + old.len() + next.len())
        {
            self.pending_table_text = Cow::Borrowed(combined);
            return Ok(());
        }
        let required = self.pending_table_text.len().checked_add(text.len()).ok_or(ParseError::Document(dom::Error::TextLimit))?;
        if required > available {
            return Err(ParseError::Document(dom::Error::TextLimit));
        }
        let capacity = required.max(self.pending_table_text.len().saturating_mul(2)).min(available);
        match &mut self.pending_table_text {
            Cow::Owned(value) => {
                if value.capacity() < required {
                    value.reserve_exact(capacity - value.len());
                }
                value.push_str(&text);
            }
            Cow::Borrowed(old) => {
                let mut value = String::with_capacity(capacity);
                value.push_str(old);
                value.push_str(&text);
                self.pending_table_text = Cow::Owned(value);
            }
        }
        Ok(())
    }

    fn in_table_text(&mut self, token: &mut Option<Token<'a>>, tokenizer: &mut Tokenizer<'a>) -> Result<bool> {
        if let Some(Token::Text(text)) = token {
            let mut text = std::mem::replace(text, Cow::Borrowed(""));
            if text.contains('\0') {
                text.to_mut().retain(|c| c != '\0');
            }
            self.buffer_table_text(text)?;
            return Ok(false);
        }
        let text = std::mem::replace(&mut self.pending_table_text, Cow::Borrowed(""));
        if text.bytes().all(whitespace) {
            self.insert_text(text)?;
        } else {
            self.table_fallback(&mut Some(Token::Text(text)), tokenizer)?;
        }
        self.mode = self.original_mode;
        Ok(true)
    }

    fn close_caption(&mut self) -> bool {
        let Some(at) = self.scope(&["caption"], Scope::Table) else { return false };
        self.implied("");
        self.stack.truncate(at);
        self.clear_formatting();
        self.mode = Insertion::Table;
        true
    }
    fn in_caption(&mut self, token: &mut Option<Token<'a>>, tokenizer: &mut Tokenizer<'a>) -> Result<bool> {
        match token {
            Some(Token::EndTag(tag)) if tag.name == "caption" => {
                self.close_caption();
                Ok(false)
            }
            Some(Token::StartTag(tag)) if matches!(tag.name.as_ref(), "caption" | "col" | "colgroup" | "tbody" | "td" | "tfoot" | "th" | "thead" | "tr") => Ok(self.close_caption()),
            Some(Token::EndTag(tag)) if tag.name == "table" => Ok(self.close_caption()),
            Some(Token::EndTag(tag)) if matches!(tag.name.as_ref(), "body" | "col" | "colgroup" | "html" | "tbody" | "td" | "tfoot" | "th" | "thead" | "tr") => Ok(false),
            _ => self.in_body(token, tokenizer),
        }
    }

    fn in_column_group(&mut self, token: &mut Option<Token<'a>>, tokenizer: &mut Tokenizer<'a>) -> Result<bool> {
        match token {
            Some(Token::Doctype(_)) => {}
            Some(Token::StartTag(tag)) if tag.name == "html" => return self.in_body(token, tokenizer),
            Some(Token::StartTag(tag)) if tag.name == "col" => {
                self.insert(tag)?;
                self.stack.pop();
            }
            Some(Token::EndTag(tag)) if tag.name == "colgroup" => {
                if self.is(self.current(), "colgroup") {
                    self.stack.pop();
                    self.mode = Insertion::Table;
                }
            }
            Some(Token::EndTag(tag)) if tag.name == "col" => {}
            Some(Token::StartTag(tag) | Token::EndTag(tag)) if tag.name == "template" => return self.in_head(token, tokenizer),
            None => return self.in_body(token, tokenizer),
            _ => {
                if self.is(self.current(), "colgroup") {
                    self.stack.pop();
                    self.mode = Insertion::Table;
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn in_table_body(&mut self, token: &mut Option<Token<'a>>, tokenizer: &mut Tokenizer<'a>) -> Result<bool> {
        match token {
            Some(Token::StartTag(tag)) if tag.name == "tr" => {
                self.clear_table_context(&["tbody", "tfoot", "thead", "template", "html"]);
                self.insert(tag)?;
                self.mode = Insertion::Row;
            }
            Some(Token::StartTag(tag)) if matches!(tag.name.as_ref(), "td" | "th") => {
                self.clear_table_context(&["tbody", "tfoot", "thead", "template", "html"]);
                self.insert(&mut synthetic("tr"))?;
                self.mode = Insertion::Row;
                return Ok(true);
            }
            Some(Token::EndTag(tag)) if matches!(tag.name.as_ref(), "tbody" | "tfoot" | "thead") => {
                if self.scope(&[&tag.name], Scope::Table).is_some() {
                    self.clear_table_context(&["tbody", "tfoot", "thead", "template", "html"]);
                    self.stack.pop();
                    self.mode = Insertion::Table;
                }
            }
            Some(Token::StartTag(tag)) if matches!(tag.name.as_ref(), "caption" | "col" | "colgroup" | "tbody" | "tfoot" | "thead") => return Ok(self.close_table_body()),
            Some(Token::EndTag(tag)) if tag.name == "table" => return Ok(self.close_table_body()),
            Some(Token::EndTag(tag)) if matches!(tag.name.as_ref(), "body" | "caption" | "col" | "colgroup" | "html" | "td" | "th" | "tr") => {}
            _ => return self.in_table(token, tokenizer),
        }
        Ok(false)
    }
    fn close_table_body(&mut self) -> bool {
        if self.scope(&["tbody", "thead", "tfoot"], Scope::Table).is_none() {
            return false;
        }
        self.clear_table_context(&["tbody", "tfoot", "thead", "template", "html"]);
        self.stack.pop();
        self.mode = Insertion::Table;
        true
    }
    fn close_row(&mut self) -> bool {
        if self.scope(&["tr"], Scope::Table).is_none() {
            return false;
        }
        self.clear_table_context(&["tr", "template", "html"]);
        self.stack.pop();
        self.mode = Insertion::TableBody;
        true
    }
    fn in_row(&mut self, token: &mut Option<Token<'a>>, tokenizer: &mut Tokenizer<'a>) -> Result<bool> {
        match token {
            Some(Token::StartTag(tag)) if matches!(tag.name.as_ref(), "td" | "th") => {
                self.clear_table_context(&["tr", "template", "html"]);
                self.insert(tag)?;
                self.mode = Insertion::Cell;
                self.formatting_marker()?;
            }
            Some(Token::EndTag(tag)) if tag.name == "tr" => {
                self.close_row();
            }
            Some(Token::StartTag(tag)) if matches!(tag.name.as_ref(), "caption" | "col" | "colgroup" | "tbody" | "tfoot" | "thead" | "tr") => return Ok(self.close_row()),
            Some(Token::EndTag(tag)) if tag.name == "table" => return Ok(self.close_row()),
            Some(Token::EndTag(tag)) if matches!(tag.name.as_ref(), "tbody" | "tfoot" | "thead") => return Ok(self.scope(&[&tag.name], Scope::Table).is_some() && self.close_row()),
            Some(Token::EndTag(tag)) if matches!(tag.name.as_ref(), "body" | "caption" | "col" | "colgroup" | "html" | "td" | "th") => {}
            _ => return self.in_table(token, tokenizer),
        }
        Ok(false)
    }
    fn close_cell(&mut self) -> bool {
        let Some(at) = self.scope(&["td", "th"], Scope::Table) else { return false };
        self.implied("");
        self.stack.truncate(at);
        self.clear_formatting();
        self.mode = Insertion::Row;
        true
    }
    fn in_cell(&mut self, token: &mut Option<Token<'a>>, tokenizer: &mut Tokenizer<'a>) -> Result<bool> {
        match token {
            Some(Token::EndTag(tag)) if matches!(tag.name.as_ref(), "td" | "th") => {
                if self.scope(&[&tag.name], Scope::Table).is_some() {
                    self.close_cell();
                }
            }
            Some(Token::StartTag(tag)) if matches!(tag.name.as_ref(), "caption" | "col" | "colgroup" | "tbody" | "td" | "tfoot" | "th" | "thead" | "tr") => return Ok(self.close_cell()),
            Some(Token::EndTag(tag)) if matches!(tag.name.as_ref(), "body" | "caption" | "col" | "colgroup" | "html") => {}
            Some(Token::EndTag(tag)) if matches!(tag.name.as_ref(), "table" | "tbody" | "tfoot" | "thead" | "tr") => return Ok(self.scope(&[&tag.name], Scope::Table).is_some() && self.close_cell()),
            _ => return self.in_body(token, tokenizer),
        }
        Ok(false)
    }

    // Otherwise LLVM folds the table handlers into process(), which then
    // stops inlining into the token loop: an extra large call per token
    // on ordinary pages. Keep this family of insertion modes outlined.
    #[inline(never)]
    pub(super) fn table_dispatch(&mut self, token: &mut Option<Token<'a>>, tokenizer: &mut Tokenizer<'a>) -> Result<bool> {
        match self.mode {
            Insertion::Table => self.in_table(token, tokenizer),
            Insertion::TableText => self.in_table_text(token, tokenizer),
            Insertion::Caption => self.in_caption(token, tokenizer),
            Insertion::ColumnGroup => self.in_column_group(token, tokenizer),
            Insertion::TableBody => self.in_table_body(token, tokenizer),
            Insertion::Row => self.in_row(token, tokenizer),
            Insertion::Cell => self.in_cell(token, tokenizer),
            _ => unreachable!("table dispatch only called for table insertion modes"),
        }
    }
}
