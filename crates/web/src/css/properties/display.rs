use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outer {
    Block,
    Inline,
    RunIn,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inner {
    Flow,
    FlowRoot,
    Table,
    Flex,
    Grid,
    Ruby,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Internal {
    TableRowGroup,
    TableHeaderGroup,
    TableFooterGroup,
    TableRow,
    TableCell,
    TableColumnGroup,
    TableColumn,
    TableCaption,
    RubyBase,
    RubyText,
    RubyBaseContainer,
    RubyTextContainer,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Display {
    None,
    Contents,
    Internal(Internal),
    Box { outer: Outer, inner: Inner, list_item: bool },
}
impl Display {
    pub(super) fn parse(values: &[Component<'_>]) -> Result<Self> {
        if values.len() == 1 {
            let s = ident(values[0])?;
            for (name, value) in [
                ("none", Self::None),
                ("contents", Self::Contents),
                ("inline-block", Self::Box { outer: Outer::Inline, inner: Inner::FlowRoot, list_item: false }),
                ("inline-table", Self::Box { outer: Outer::Inline, inner: Inner::Table, list_item: false }),
                ("inline-flex", Self::Box { outer: Outer::Inline, inner: Inner::Flex, list_item: false }),
                ("inline-grid", Self::Box { outer: Outer::Inline, inner: Inner::Grid, list_item: false }),
                // The old flexbox's boxes, which pages use now only to clamp
                // lines (-webkit-line-clamp): blocks of their own.
                ("-webkit-box", Self::Box { outer: Outer::Block, inner: Inner::FlowRoot, list_item: false }),
                ("-webkit-inline-box", Self::Box { outer: Outer::Inline, inner: Inner::FlowRoot, list_item: false }),
            ] {
                if s.eq_ignore_ascii_case(name) {
                    return Ok(value);
                }
            }
            for (name, value) in [
                ("table-row-group", Internal::TableRowGroup),
                ("table-header-group", Internal::TableHeaderGroup),
                ("table-footer-group", Internal::TableFooterGroup),
                ("table-row", Internal::TableRow),
                ("table-cell", Internal::TableCell),
                ("table-column-group", Internal::TableColumnGroup),
                ("table-column", Internal::TableColumn),
                ("table-caption", Internal::TableCaption),
                ("ruby-base", Internal::RubyBase),
                ("ruby-text", Internal::RubyText),
                ("ruby-base-container", Internal::RubyBaseContainer),
                ("ruby-text-container", Internal::RubyTextContainer),
            ] {
                if s.eq_ignore_ascii_case(name) {
                    return Ok(Self::Internal(value));
                }
            }
            if s.eq_ignore_ascii_case("math") {
                return Err(Error::Unsupported("math display"));
            }
        }
        let (mut outer, mut inner, mut list_item) = (None, None, false);
        for &c in values {
            let s = ident(c)?;
            let o = [("block", Outer::Block), ("inline", Outer::Inline), ("run-in", Outer::RunIn)].into_iter().find_map(|(n, v)| s.eq_ignore_ascii_case(n).then_some(v));
            if let Some(o) = o {
                if outer.replace(o).is_some() {
                    return Err(Error::Invalid);
                }
                continue;
            }
            let i = [("flow", Inner::Flow), ("flow-root", Inner::FlowRoot), ("table", Inner::Table), ("flex", Inner::Flex), ("grid", Inner::Grid), ("ruby", Inner::Ruby)]
                .into_iter()
                .find_map(|(n, v)| s.eq_ignore_ascii_case(n).then_some(v));
            if let Some(i) = i {
                if inner.replace(i).is_some() {
                    return Err(Error::Invalid);
                }
                continue;
            }
            if s.eq_ignore_ascii_case("list-item") && !list_item {
                list_item = true;
            } else {
                return Err(Error::Invalid);
            }
        }
        let inner = inner.unwrap_or(Inner::Flow);
        if list_item && !matches!(inner, Inner::Flow | Inner::FlowRoot) {
            return Err(Error::Invalid);
        }
        Ok(Self::Box { outer: outer.unwrap_or(if inner == Inner::Ruby { Outer::Inline } else { Outer::Block }), inner, list_item })
    }
}
fn ident(c: Component<'_>) -> Result<&str> {
    if c.kind() == Kind::Ident { Ok(c.text().unwrap()) } else { Err(Error::Invalid) }
}
