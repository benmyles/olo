//! Static template insertion modes. Declarative shadow roots/content
//! patching, custom-element reactions and script execution remain separate.
use super::{Builder, Insertion, ParseError, Result, Tag, Token, Tokenizer};

impl<'a> Builder<'a> {
    pub(super) fn start_template(&mut self, tag: &mut Tag<'a>) -> Result<()> {
        if self.fragment_context.is_none() && tag.attributes.iter().any(|a| a.name == "shadowrootmode" && (a.value.eq_ignore_ascii_case("open") || a.value.eq_ignore_ascii_case("closed"))) {
            return Err(ParseError::Unsupported("declarative shadow roots"));
        }
        if tag.attributes.iter().any(|a| a.name == "for") {
            return Err(ParseError::Unsupported("template content patching"));
        }
        self.formatting_marker()?;
        self.insert(tag)?;
        self.frameset_ok = false;
        self.mode = Insertion::Template;
        self.template_modes.push(Insertion::Template);
        self.insertion_flags |= super::TEMPLATE_INSERTION;
        Ok(())
    }
    pub(super) fn close_template(&mut self) {
        if let Some(at) = self.stack.iter().rposition(|&n| self.is(n, "template")) {
            // Thorough implied end tags have the same final tree here:
            // no script runs between them and truncation of this stack.
            self.stack.truncate(at);
            self.clear_formatting();
            self.template_modes.pop();
            if self.template_modes.is_empty() {
                self.insertion_flags &= !super::TEMPLATE_INSERTION;
            }
            self.reset_insertion_mode();
        }
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
