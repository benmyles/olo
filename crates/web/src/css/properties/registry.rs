use super::*;
use std::cmp::Ordering;
#[derive(Clone, Copy)]
pub(super) enum SizeKind {
    Preferred,
    Minimum,
    Maximum,
    Basis,
}
#[derive(Clone, Copy)]
pub(super) enum Grammar {
    Keywords(&'static [Keyword]),
    Numeric(NumericKind, Range, &'static [Keyword]),
    Color(&'static [Keyword]),
    Size(SizeKind),
    Display,
}
#[derive(Clone, Copy)]
pub(super) enum Expansion {
    WideOnly,
    Quad,
    Repeat,
    Unordered,
}
macro_rules! keyword_registry {
    ($($id:ident => $name:literal),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Keyword { $($id),* }
        impl Keyword {
            pub fn as_str(self) -> &'static str { match self { $(Self::$id => $name),* } }
        }
    }
}
keyword_registry! {
    Absolute => "absolute",
    All => "all",
    Anywhere => "anywhere",
    Auto => "auto",
    AutoPhrase => "auto-phrase",
    Baseline => "baseline",
    Block => "block",
    Bold => "bold",
    Bolder => "bolder",
    BorderBox => "border-box",
    Both => "both",
    Bottom => "bottom",
    BoundingBox => "bounding-box",
    BreakAll => "break-all",
    BreakWord => "break-word",
    Center => "center",
    Clip => "clip",
    Collapse => "collapse",
    Column => "column",
    ColumnReverse => "column-reverse",
    Contain => "contain",
    Content => "content",
    ContentBox => "content-box",
    Cover => "cover",
    Dashed => "dashed",
    Dotted => "dotted",
    Double => "double",
    End => "end",
    Fill => "fill",
    FitContent => "fit-content",
    Fixed => "fixed",
    Groove => "groove",
    Hidden => "hidden",
    Horizontal => "horizontal",
    HorizontalTb => "horizontal-tb",
    Inline => "inline",
    InlineEnd => "inline-end",
    InlineStart => "inline-start",
    Inset => "inset",
    Isolate => "isolate",
    Justify => "justify",
    JustifyAll => "justify-all",
    KeepAll => "keep-all",
    Large => "large",
    Larger => "larger",
    Left => "left",
    Lighter => "lighter",
    Ltr => "ltr",
    MatchParent => "match-parent",
    Math => "math",
    MaxContent => "max-content",
    Medium => "medium",
    Middle => "middle",
    Mixed => "mixed",
    MinContent => "min-content",
    None => "none",
    Normal => "normal",
    Nowrap => "nowrap",
    Outset => "outset",
    Painted => "painted",
    Relative => "relative",
    Ridge => "ridge",
    Right => "right",
    Row => "row",
    RowReverse => "row-reverse",
    Rtl => "rtl",
    ScaleDown => "scale-down",
    Scroll => "scroll",
    Sideways => "sideways",
    SidewaysLr => "sideways-lr",
    SidewaysRl => "sideways-rl",
    Small => "small",
    Smaller => "smaller",
    Solid => "solid",
    Start => "start",
    Static => "static",
    Sticky => "sticky",
    Stretch => "stretch",
    Stroke => "stroke",
    Sub => "sub",
    Super => "super",
    TextBottom => "text-bottom",
    TextTop => "text-top",
    Thick => "thick",
    Thin => "thin",
    Top => "top",
    Upright => "upright",
    Vertical => "vertical",
    VerticalLr => "vertical-lr",
    VerticalRl => "vertical-rl",
    Visible => "visible",
    Visiblefill => "visiblefill",
    Visiblepainted => "visiblepainted",
    Visiblestroke => "visiblestroke",
    Wrap => "wrap",
    WrapReverse => "wrap-reverse",
    XLarge => "x-large",
    XSmall => "x-small",
    XxLarge => "xx-large",
    XxSmall => "xx-small",
    XxxLarge => "xxx-large",
}
macro_rules! property_registry {
    ($( $id:ident, $name:literal, $grammar:expr, $initial:literal, $inherited:literal; )*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[repr(u16)]
        pub enum Property { $($id),* }
        impl Property {
            pub const ALL: &'static [Self] = &[$(Self::$id),*];
            pub fn as_str(self) -> &'static str { match self { $(Self::$id => $name),* } }
            pub fn inherited(self) -> bool { match self { $(Self::$id => $inherited),* } }
            pub fn initial_spelling(self) -> &'static str { match self { $(Self::$id => $initial),* } }
            pub(super) fn grammar(self) -> Grammar { match self { $(Self::$id => $grammar),* } }
            pub(super) fn singleton(self) -> &'static [Self] { match self { $(Self::$id => &[Self::$id]),* } }
        }
    }
}
property_registry! {
    BackgroundColor, "background-color", Grammar::Color(&[]), "transparent", false;
    BlockSize, "block-size", Grammar::Size(SizeKind::Preferred), "auto", false;
    BorderBottomColor, "border-bottom-color", Grammar::Color(&[]), "currentcolor", false;
    BorderBottomStyle, "border-bottom-style", Grammar::Keywords(&[Keyword::None,Keyword::Hidden,Keyword::Dotted,Keyword::Dashed,Keyword::Solid,Keyword::Double,Keyword::Groove,Keyword::Ridge,Keyword::Inset,Keyword::Outset]), "none", false;
    BorderBottomWidth, "border-bottom-width", Grammar::Numeric(NumericKind::Length, Range::Nonnegative, &[Keyword::Thin,Keyword::Medium,Keyword::Thick]), "medium", false;
    BorderLeftColor, "border-left-color", Grammar::Color(&[]), "currentcolor", false;
    BorderLeftStyle, "border-left-style", Grammar::Keywords(&[Keyword::None,Keyword::Hidden,Keyword::Dotted,Keyword::Dashed,Keyword::Solid,Keyword::Double,Keyword::Groove,Keyword::Ridge,Keyword::Inset,Keyword::Outset]), "none", false;
    BorderLeftWidth, "border-left-width", Grammar::Numeric(NumericKind::Length, Range::Nonnegative, &[Keyword::Thin,Keyword::Medium,Keyword::Thick]), "medium", false;
    BorderRightColor, "border-right-color", Grammar::Color(&[]), "currentcolor", false;
    BorderRightStyle, "border-right-style", Grammar::Keywords(&[Keyword::None,Keyword::Hidden,Keyword::Dotted,Keyword::Dashed,Keyword::Solid,Keyword::Double,Keyword::Groove,Keyword::Ridge,Keyword::Inset,Keyword::Outset]), "none", false;
    BorderRightWidth, "border-right-width", Grammar::Numeric(NumericKind::Length, Range::Nonnegative, &[Keyword::Thin,Keyword::Medium,Keyword::Thick]), "medium", false;
    BorderTopColor, "border-top-color", Grammar::Color(&[]), "currentcolor", false;
    BorderTopStyle, "border-top-style", Grammar::Keywords(&[Keyword::None,Keyword::Hidden,Keyword::Dotted,Keyword::Dashed,Keyword::Solid,Keyword::Double,Keyword::Groove,Keyword::Ridge,Keyword::Inset,Keyword::Outset]), "none", false;
    BorderTopWidth, "border-top-width", Grammar::Numeric(NumericKind::Length, Range::Nonnegative, &[Keyword::Thin,Keyword::Medium,Keyword::Thick]), "medium", false;
    Bottom, "bottom", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "auto", false;
    BoxSizing, "box-sizing", Grammar::Keywords(&[Keyword::ContentBox,Keyword::BorderBox]), "content-box", false;
    CaretColor, "caret-color", Grammar::Color(&[Keyword::Auto]), "auto", true;
    Clear, "clear", Grammar::Keywords(&[Keyword::None,Keyword::Left,Keyword::Right,Keyword::Both,Keyword::InlineStart,Keyword::InlineEnd]), "none", false;
    Color, "color", Grammar::Color(&[]), "canvastext", true;
    ColumnGap, "column-gap", Grammar::Numeric(NumericKind::LengthPercentage, Range::Nonnegative, &[Keyword::Normal]), "normal", false;
    Direction, "direction", Grammar::Keywords(&[Keyword::Ltr,Keyword::Rtl]), "ltr", true;
    Display, "display", Grammar::Display, "inline", false;
    FlexBasis, "flex-basis", Grammar::Size(SizeKind::Basis), "auto", false;
    FlexDirection, "flex-direction", Grammar::Keywords(&[Keyword::Row,Keyword::RowReverse,Keyword::Column,Keyword::ColumnReverse]), "row", false;
    FlexGrow, "flex-grow", Grammar::Numeric(NumericKind::Number, Range::Nonnegative, &[]), "0", false;
    FlexShrink, "flex-shrink", Grammar::Numeric(NumericKind::Number, Range::Nonnegative, &[]), "1", false;
    FlexWrap, "flex-wrap", Grammar::Keywords(&[Keyword::Nowrap,Keyword::Wrap,Keyword::WrapReverse]), "nowrap", false;
    Float, "float", Grammar::Keywords(&[Keyword::None,Keyword::Left,Keyword::Right,Keyword::InlineStart,Keyword::InlineEnd]), "none", false;
    FontSize, "font-size", Grammar::Numeric(NumericKind::LengthPercentage, Range::Nonnegative, &[Keyword::XxSmall,Keyword::XSmall,Keyword::Small,Keyword::Medium,Keyword::Large,Keyword::XLarge,Keyword::XxLarge,Keyword::XxxLarge,Keyword::Larger,Keyword::Smaller,Keyword::Math]), "medium", true;
    FontWeight, "font-weight", Grammar::Numeric(NumericKind::Number, Range::FontWeight, &[Keyword::Normal,Keyword::Bold,Keyword::Bolder,Keyword::Lighter]), "normal", true;
    Height, "height", Grammar::Size(SizeKind::Preferred), "auto", false;
    InlineSize, "inline-size", Grammar::Size(SizeKind::Preferred), "auto", false;
    InsetBlockEnd, "inset-block-end", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "auto", false;
    InsetBlockStart, "inset-block-start", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "auto", false;
    InsetInlineEnd, "inset-inline-end", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "auto", false;
    InsetInlineStart, "inset-inline-start", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "auto", false;
    Isolation, "isolation", Grammar::Keywords(&[Keyword::Auto,Keyword::Isolate]), "auto", false;
    Left, "left", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "auto", false;
    LetterSpacing, "letter-spacing", Grammar::Numeric(NumericKind::Length, Range::All, &[Keyword::Normal]), "normal", true;
    LineHeight, "line-height", Grammar::Numeric(NumericKind::LineHeight, Range::Nonnegative, &[Keyword::Normal]), "normal", true;
    MarginBlockEnd, "margin-block-end", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "0", false;
    MarginBlockStart, "margin-block-start", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "0", false;
    MarginBottom, "margin-bottom", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "0", false;
    MarginInlineEnd, "margin-inline-end", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "0", false;
    MarginInlineStart, "margin-inline-start", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "0", false;
    MarginLeft, "margin-left", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "0", false;
    MarginRight, "margin-right", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "0", false;
    MarginTop, "margin-top", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "0", false;
    MaxBlockSize, "max-block-size", Grammar::Size(SizeKind::Maximum), "none", false;
    MaxHeight, "max-height", Grammar::Size(SizeKind::Maximum), "none", false;
    MaxInlineSize, "max-inline-size", Grammar::Size(SizeKind::Maximum), "none", false;
    MaxWidth, "max-width", Grammar::Size(SizeKind::Maximum), "none", false;
    MinBlockSize, "min-block-size", Grammar::Size(SizeKind::Minimum), "auto", false;
    MinHeight, "min-height", Grammar::Size(SizeKind::Minimum), "auto", false;
    MinInlineSize, "min-inline-size", Grammar::Size(SizeKind::Minimum), "auto", false;
    MinWidth, "min-width", Grammar::Size(SizeKind::Minimum), "auto", false;
    ObjectFit, "object-fit", Grammar::Keywords(&[Keyword::Fill,Keyword::Contain,Keyword::Cover,Keyword::None,Keyword::ScaleDown]), "fill", false;
    Opacity, "opacity", Grammar::Numeric(NumericKind::NumberPercentage, Range::Unit, &[]), "1", false;
    Order, "order", Grammar::Numeric(NumericKind::Integer, Range::All, &[]), "0", false;
    OverflowBlock, "overflow-block", Grammar::Keywords(&[Keyword::Visible, Keyword::Hidden, Keyword::Clip, Keyword::Scroll, Keyword::Auto]), "visible", false;
    OverflowInline, "overflow-inline", Grammar::Keywords(&[Keyword::Visible, Keyword::Hidden, Keyword::Clip, Keyword::Scroll, Keyword::Auto]), "visible", false;
    OverflowWrap, "overflow-wrap", Grammar::Keywords(&[Keyword::Normal,Keyword::BreakWord,Keyword::Anywhere]), "normal", true;
    OverflowX, "overflow-x", Grammar::Keywords(&[Keyword::Visible,Keyword::Hidden,Keyword::Clip,Keyword::Scroll,Keyword::Auto]), "visible", false;
    OverflowY, "overflow-y", Grammar::Keywords(&[Keyword::Visible,Keyword::Hidden,Keyword::Clip,Keyword::Scroll,Keyword::Auto]), "visible", false;
    PaddingBlockEnd, "padding-block-end", Grammar::Numeric(NumericKind::LengthPercentage, Range::Nonnegative, &[]), "0", false;
    PaddingBlockStart, "padding-block-start", Grammar::Numeric(NumericKind::LengthPercentage, Range::Nonnegative, &[]), "0", false;
    PaddingBottom, "padding-bottom", Grammar::Numeric(NumericKind::LengthPercentage, Range::Nonnegative, &[]), "0", false;
    PaddingInlineEnd, "padding-inline-end", Grammar::Numeric(NumericKind::LengthPercentage, Range::Nonnegative, &[]), "0", false;
    PaddingInlineStart, "padding-inline-start", Grammar::Numeric(NumericKind::LengthPercentage, Range::Nonnegative, &[]), "0", false;
    PaddingLeft, "padding-left", Grammar::Numeric(NumericKind::LengthPercentage, Range::Nonnegative, &[]), "0", false;
    PaddingRight, "padding-right", Grammar::Numeric(NumericKind::LengthPercentage, Range::Nonnegative, &[]), "0", false;
    PaddingTop, "padding-top", Grammar::Numeric(NumericKind::LengthPercentage, Range::Nonnegative, &[]), "0", false;
    PointerEvents, "pointer-events", Grammar::Keywords(&[Keyword::Auto,Keyword::None,Keyword::Visiblepainted,Keyword::Visiblefill,Keyword::Visiblestroke,Keyword::Visible,Keyword::Painted,Keyword::Fill,Keyword::Stroke,Keyword::BoundingBox,Keyword::All]), "auto", true;
    Position, "position", Grammar::Keywords(&[Keyword::Static,Keyword::Relative,Keyword::Absolute,Keyword::Fixed,Keyword::Sticky]), "static", false;
    Resize, "resize", Grammar::Keywords(&[Keyword::None,Keyword::Both,Keyword::Horizontal,Keyword::Vertical,Keyword::Block,Keyword::Inline]), "none", false;
    Right, "right", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "auto", false;
    RowGap, "row-gap", Grammar::Numeric(NumericKind::LengthPercentage, Range::Nonnegative, &[Keyword::Normal]), "normal", false;
    TextAlign, "text-align", Grammar::Keywords(&[Keyword::Start,Keyword::End,Keyword::Left,Keyword::Right,Keyword::Center,Keyword::Justify,Keyword::MatchParent,Keyword::JustifyAll]), "start", true;
    TextAlignLast, "text-align-last", Grammar::Keywords(&[Keyword::Auto,Keyword::Start,Keyword::End,Keyword::Left,Keyword::Right,Keyword::Center,Keyword::Justify]), "auto", true;
    TextDecorationColor, "text-decoration-color", Grammar::Color(&[]), "currentcolor", false;
    TextOrientation, "text-orientation", Grammar::Keywords(&[Keyword::Mixed,Keyword::Upright,Keyword::Sideways]), "mixed", true;
    Top, "top", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "auto", false;
    VerticalAlign, "vertical-align", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Baseline,Keyword::Sub,Keyword::Super,Keyword::TextTop,Keyword::TextBottom,Keyword::Middle,Keyword::Top,Keyword::Bottom]), "baseline", false;
    Visibility, "visibility", Grammar::Keywords(&[Keyword::Visible,Keyword::Hidden,Keyword::Collapse]), "visible", true;
    Width, "width", Grammar::Size(SizeKind::Preferred), "auto", false;
    WordBreak, "word-break", Grammar::Keywords(&[Keyword::Normal,Keyword::BreakAll,Keyword::KeepAll,Keyword::AutoPhrase,Keyword::BreakWord]), "normal", true;
    WordSpacing, "word-spacing", Grammar::Numeric(NumericKind::Length, Range::All, &[Keyword::Normal]), "normal", true;
    WritingMode, "writing-mode", Grammar::Keywords(&[Keyword::HorizontalTb,Keyword::VerticalRl,Keyword::VerticalLr,Keyword::SidewaysRl,Keyword::SidewaysLr]), "horizontal-tb", true;
    ZIndex, "z-index", Grammar::Numeric(NumericKind::Integer, Range::All, &[Keyword::Auto]), "auto", false;
}
macro_rules! shorthand_registry {
    ($( $id:ident, $name:literal, [$($property:ident),+], $mode:ident; )*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Shorthand { $($id),* }
        impl Shorthand {
            pub const ALL: &'static [Self] = &[$(Self::$id),*];
            pub fn as_str(self) -> &'static str { match self { $(Self::$id => $name),* } }
            pub fn properties(self) -> &'static [Property] { match self { $(Self::$id => &[$(Property::$property),+]),* } }
            pub(super) fn expansion(self) -> Expansion { match self { $(Self::$id => Expansion::$mode),* } }
        }
    }
}
shorthand_registry! {
    // all resets every implemented longhand except direction. Custom properties
    // and the not-yet-implemented unicode-bidi are excluded by definition.
    All, "all", [BackgroundColor,BlockSize,BorderBottomColor,BorderBottomStyle,BorderBottomWidth,BorderLeftColor,BorderLeftStyle,BorderLeftWidth,BorderRightColor,BorderRightStyle,BorderRightWidth,BorderTopColor,BorderTopStyle,BorderTopWidth,Bottom,BoxSizing,CaretColor,Clear,Color,ColumnGap,Display,FlexBasis,FlexDirection,FlexGrow,FlexShrink,FlexWrap,Float,FontSize,FontWeight,Height,InlineSize,InsetBlockEnd,InsetBlockStart,InsetInlineEnd,InsetInlineStart,Isolation,Left,LetterSpacing,LineHeight,MarginBlockEnd,MarginBlockStart,MarginBottom,MarginInlineEnd,MarginInlineStart,MarginLeft,MarginRight,MarginTop,MaxBlockSize,MaxHeight,MaxInlineSize,MaxWidth,MinBlockSize,MinHeight,MinInlineSize,MinWidth,ObjectFit,Opacity,Order,OverflowBlock,OverflowInline,OverflowWrap,OverflowX,OverflowY,PaddingBlockEnd,PaddingBlockStart,PaddingBottom,PaddingInlineEnd,PaddingInlineStart,PaddingLeft,PaddingRight,PaddingTop,PointerEvents,Position,Resize,Right,RowGap,TextAlign,TextAlignLast,TextDecorationColor,TextOrientation,Top,VerticalAlign,Visibility,Width,WordBreak,WordSpacing,WritingMode,ZIndex], WideOnly;
    BorderColor, "border-color", [BorderTopColor,BorderRightColor,BorderBottomColor,BorderLeftColor], Quad;
    BorderStyle, "border-style", [BorderTopStyle,BorderRightStyle,BorderBottomStyle,BorderLeftStyle], Quad;
    BorderWidth, "border-width", [BorderTopWidth,BorderRightWidth,BorderBottomWidth,BorderLeftWidth], Quad;
    FlexFlow, "flex-flow", [FlexDirection,FlexWrap], Unordered;
    Gap, "gap", [RowGap,ColumnGap], Repeat;
    Inset, "inset", [Top,Right,Bottom,Left], Quad;
    InsetBlock, "inset-block", [InsetBlockStart,InsetBlockEnd], Repeat;
    InsetInline, "inset-inline", [InsetInlineStart,InsetInlineEnd], Repeat;
    Margin, "margin", [MarginTop,MarginRight,MarginBottom,MarginLeft], Quad;
    MarginBlock, "margin-block", [MarginBlockStart,MarginBlockEnd], Repeat;
    MarginInline, "margin-inline", [MarginInlineStart,MarginInlineEnd], Repeat;
    Overflow, "overflow", [OverflowX,OverflowY], Repeat;
    Padding, "padding", [PaddingTop,PaddingRight,PaddingBottom,PaddingLeft], Quad;
    PaddingBlock, "padding-block", [PaddingBlockStart,PaddingBlockEnd], Repeat;
    PaddingInline, "padding-inline", [PaddingInlineStart,PaddingInlineEnd], Repeat;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Name {
    Longhand(Property),
    Shorthand(Shorthand),
}
impl Name {
    pub fn parse(name: &str) -> Option<Self> {
        if let Ok(i) = Property::ALL.binary_search_by(|p| ascii_cmp(p.as_str(), name)) {
            return Some(Self::Longhand(Property::ALL[i]));
        }
        Shorthand::ALL.binary_search_by(|p| ascii_cmp(p.as_str(), name)).ok().map(|i| Self::Shorthand(Shorthand::ALL[i]))
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Longhand(p) => p.as_str(),
            Self::Shorthand(p) => p.as_str(),
        }
    }
    pub fn properties(self) -> &'static [Property] {
        match self {
            Self::Longhand(p) => p.singleton(),
            Self::Shorthand(p) => p.properties(),
        }
    }
    pub(super) fn wide(self, wide: variables::Wide) -> Assignments {
        self.properties().iter().map(|&property| Assignment { property, value: Value::Wide(wide) }).collect()
    }
}
fn ascii_cmp(a: &str, b: &str) -> Ordering {
    a.bytes().map(|c| c.to_ascii_lowercase()).cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
}
