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
    FontFamily,
    ClipPath,
    FontStyle,
    FontWidth,
    Overflow,
    Keywords(&'static [Keyword]),
    Numeric(NumericKind, Range, &'static [Keyword]),
    Color(&'static [Keyword]),
    Size(SizeKind),
    Display,
    Tracks,
    GridLine,
    GridAutoFlow,
    Radius,
    Images,
    Sizes,
    Positions,
    Repeats,
    Shadows,
    Transforms,
    Features,
    Ratio,
    Content,
    Animation(super::animation::Field),
    /// `text-box-edge`: an over edge and an under edge, as one keyword.
    TextBoxEdge,
    /// `container-name`: `none` or names (kept as a list of names).
    ContainerName,
    /// `backdrop-filter`: `none` or filter functions.
    Filters,
}
#[derive(Clone, Copy)]
pub(super) enum Expansion {
    WideOnly,
    WhiteSpace,
    Flex,
    Border,
    Quad,
    Repeat,
    Unordered,
    Grid,
    Radius,
    Background,
    TextWrap,
    TextBox,
    TextDecoration,
    ListStyle,
    Font,
    Animation,
    Transition,
    Container,
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
    Alpha => "alpha",
    Alternate => "alternate",
    AlternateReverse => "alternate-reverse",
    Backwards => "backwards",
    Forwards => "forwards",
    Paused => "paused",
    Reverse => "reverse",
    Running => "running",
    TrimStart => "trim-start",
    TrimEnd => "trim-end",
    TrimBoth => "trim-both",
    Cap => "cap text",
    Ex => "ex text",
    TextAlphabetic => "text alphabetic",
    CapAlphabetic => "cap alphabetic",
    ExAlphabetic => "ex alphabetic",
    Capitalize => "capitalize",
    Uppercase => "uppercase",
    Lowercase => "lowercase",
    FullWidth => "full-width",
    FullSizeKana => "full-size-kana",
    Anywhere => "anywhere",
    Auto => "auto",
    AutoPhrase => "auto-phrase",
    Balance => "balance",
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
    BreakSpaces => "break-spaces",
    Center => "center",
    Circle => "circle",
    Clip => "clip",
    Ellipsis => "ellipsis",
    Collapse => "collapse",
    Column => "column",
    ColumnDense => "column dense",
    ColumnReverse => "column-reverse",
    Contain => "contain",
    Content => "content",
    ContentBox => "content-box",
    Cover => "cover",
    Dashed => "dashed",
    Decimal => "decimal",
    DecimalLeadingZero => "decimal-leading-zero",
    Disc => "disc",
    Dotted => "dotted",
    Double => "double",
    End => "end",
    Evenodd => "evenodd",
    Inside => "inside",
    LineThrough => "line-through",
    LowerAlpha => "lower-alpha",
    LowerLatin => "lower-latin",
    LowerRoman => "lower-roman",
    Luminance => "luminance",
    MatchSource => "match-source",
    Nonzero => "nonzero",
    Fill => "fill",
    FitContent => "fit-content",
    Fixed => "fixed",
    FlexStart => "flex-start",
    FlexEnd => "flex-end",
    Outside => "outside",
    Overline => "overline",
    Pretty => "pretty",
    SpaceBetween => "space-between",
    SpaceAround => "space-around",
    SpaceEvenly => "space-evenly",
    Groove => "groove",
    Hidden => "hidden",
    Horizontal => "horizontal",
    HorizontalTb => "horizontal-tb",
    Inline => "inline",
    InlineSize => "inline-size",
    InlineEnd => "inline-end",
    InlineStart => "inline-start",
    Italic => "italic",
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
    Square => "square",
    Stable => "stable",
    Underline => "underline",
    UpperAlpha => "upper-alpha",
    UpperLatin => "upper-latin",
    UpperRoman => "upper-roman",
    Wavy => "wavy",
    WebkitCenter => "-webkit-center",
    WebkitLeft => "-webkit-left",
    WebkitRight => "-webkit-right",
    Math => "math",
    MaxContent => "max-content",
    Medium => "medium",
    Middle => "middle",
    Mixed => "mixed",
    MinContent => "min-content",
    None => "none",
    Normal => "normal",
    Oblique => "oblique",
    Nowrap => "nowrap",
    Outset => "outset",
    Painted => "painted",
    Preserve => "preserve",
    PreserveBreaks => "preserve-breaks",
    PreserveSpaces => "preserve-spaces",
    Relative => "relative",
    Ridge => "ridge",
    Right => "right",
    PaddingBox => "padding-box",
    Row => "row",
    RowDense => "row dense",
    Dense => "dense",
    RowReverse => "row-reverse",
    Rtl => "rtl",
    ScaleDown => "scale-down",
    Scroll => "scroll",
    Sideways => "sideways",
    SidewaysLr => "sideways-lr",
    SidewaysRl => "sideways-rl",
    Small => "small",
    Size => "size",
    Smaller => "smaller",
    Solid => "solid",
    Start => "start",
    Static => "static",
    Sticky => "sticky",
    Stretch => "stretch",
    Text => "text",
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
    AlignContent, "align-content", Grammar::Keywords(&[Keyword::Normal,Keyword::Stretch,Keyword::FlexStart,Keyword::FlexEnd,Keyword::Center,Keyword::Start,Keyword::End,Keyword::SpaceBetween,Keyword::SpaceAround,Keyword::SpaceEvenly,Keyword::Baseline]), "normal", false;
    AlignItems, "align-items", Grammar::Keywords(&[Keyword::Normal,Keyword::Stretch,Keyword::FlexStart,Keyword::FlexEnd,Keyword::Center,Keyword::Start,Keyword::End,Keyword::Baseline]), "normal", false;
    AlignSelf, "align-self", Grammar::Keywords(&[Keyword::Auto,Keyword::Normal,Keyword::Stretch,Keyword::FlexStart,Keyword::FlexEnd,Keyword::Center,Keyword::Start,Keyword::End,Keyword::Baseline]), "auto", false;
    AnimationDelay, "animation-delay", Grammar::Animation(super::animation::Field::Delay), "0s", false;
    AnimationDirection, "animation-direction", Grammar::Animation(super::animation::Field::Direction), "normal", false;
    AnimationDuration, "animation-duration", Grammar::Animation(super::animation::Field::Duration), "0s", false;
    AnimationFillMode, "animation-fill-mode", Grammar::Animation(super::animation::Field::Fill), "none", false;
    AnimationIterationCount, "animation-iteration-count", Grammar::Animation(super::animation::Field::Count), "1", false;
    AnimationName, "animation-name", Grammar::Animation(super::animation::Field::Name), "none", false;
    AnimationPlayState, "animation-play-state", Grammar::Animation(super::animation::Field::PlayState), "running", false;
    AnimationTimingFunction, "animation-timing-function", Grammar::Animation(super::animation::Field::Easing), "ease", false;
    AspectRatio, "aspect-ratio", Grammar::Ratio, "auto", false;
    BackdropFilter, "backdrop-filter", Grammar::Filters, "none", false;
    BackgroundClip, "background-clip", Grammar::Keywords(&[Keyword::BorderBox,Keyword::PaddingBox,Keyword::ContentBox,Keyword::Text]), "border-box", false;
    BackgroundColor, "background-color", Grammar::Color(&[]), "transparent", false;
    BackgroundImage, "background-image", Grammar::Images, "none", false;
    BackgroundOrigin, "background-origin", Grammar::Keywords(&[Keyword::BorderBox,Keyword::PaddingBox,Keyword::ContentBox]), "padding-box", false;
    BackgroundPosition, "background-position", Grammar::Positions, "0% 0%", false;
    BackgroundRepeat, "background-repeat", Grammar::Repeats, "repeat", false;
    BackgroundSize, "background-size", Grammar::Sizes, "auto", false;
    BlockSize, "block-size", Grammar::Size(SizeKind::Preferred), "auto", false;
    BorderBottomColor, "border-bottom-color", Grammar::Color(&[]), "currentcolor", false;
    BorderBottomLeftRadius, "border-bottom-left-radius", Grammar::Radius, "0", false;
    BorderBottomRightRadius, "border-bottom-right-radius", Grammar::Radius, "0", false;
    BorderBottomStyle, "border-bottom-style", Grammar::Keywords(&[Keyword::None,Keyword::Hidden,Keyword::Dotted,Keyword::Dashed,Keyword::Solid,Keyword::Double,Keyword::Groove,Keyword::Ridge,Keyword::Inset,Keyword::Outset]), "none", false;
    BorderBottomWidth, "border-bottom-width", Grammar::Numeric(NumericKind::Length, Range::Nonnegative, &[Keyword::Thin,Keyword::Medium,Keyword::Thick]), "medium", false;
    BorderLeftColor, "border-left-color", Grammar::Color(&[]), "currentcolor", false;
    BorderLeftStyle, "border-left-style", Grammar::Keywords(&[Keyword::None,Keyword::Hidden,Keyword::Dotted,Keyword::Dashed,Keyword::Solid,Keyword::Double,Keyword::Groove,Keyword::Ridge,Keyword::Inset,Keyword::Outset]), "none", false;
    BorderLeftWidth, "border-left-width", Grammar::Numeric(NumericKind::Length, Range::Nonnegative, &[Keyword::Thin,Keyword::Medium,Keyword::Thick]), "medium", false;
    BorderRightColor, "border-right-color", Grammar::Color(&[]), "currentcolor", false;
    BorderRightStyle, "border-right-style", Grammar::Keywords(&[Keyword::None,Keyword::Hidden,Keyword::Dotted,Keyword::Dashed,Keyword::Solid,Keyword::Double,Keyword::Groove,Keyword::Ridge,Keyword::Inset,Keyword::Outset]), "none", false;
    BorderRightWidth, "border-right-width", Grammar::Numeric(NumericKind::Length, Range::Nonnegative, &[Keyword::Thin,Keyword::Medium,Keyword::Thick]), "medium", false;
    BorderSpacing, "border-spacing", Grammar::Radius, "0", true;
    BorderTopColor, "border-top-color", Grammar::Color(&[]), "currentcolor", false;
    BorderTopLeftRadius, "border-top-left-radius", Grammar::Radius, "0", false;
    BorderTopRightRadius, "border-top-right-radius", Grammar::Radius, "0", false;
    BorderTopStyle, "border-top-style", Grammar::Keywords(&[Keyword::None,Keyword::Hidden,Keyword::Dotted,Keyword::Dashed,Keyword::Solid,Keyword::Double,Keyword::Groove,Keyword::Ridge,Keyword::Inset,Keyword::Outset]), "none", false;
    BorderTopWidth, "border-top-width", Grammar::Numeric(NumericKind::Length, Range::Nonnegative, &[Keyword::Thin,Keyword::Medium,Keyword::Thick]), "medium", false;
    Bottom, "bottom", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "auto", false;
    BoxShadow, "box-shadow", Grammar::Shadows, "none", false;
    BoxSizing, "box-sizing", Grammar::Keywords(&[Keyword::ContentBox,Keyword::BorderBox]), "content-box", false;
    CaretColor, "caret-color", Grammar::Color(&[Keyword::Auto]), "auto", true;
    Clear, "clear", Grammar::Keywords(&[Keyword::None,Keyword::Left,Keyword::Right,Keyword::Both,Keyword::InlineStart,Keyword::InlineEnd]), "none", false;
    ClipPath, "clip-path", Grammar::ClipPath, "none", false;
    ClipRule, "clip-rule", Grammar::Keywords(&[Keyword::Nonzero,Keyword::Evenodd]), "nonzero", true;
    Color, "color", Grammar::Color(&[]), "canvastext", true;
    ColumnGap, "column-gap", Grammar::Numeric(NumericKind::LengthPercentage, Range::Nonnegative, &[Keyword::Normal]), "normal", false;
    ContainerName, "container-name", Grammar::ContainerName, "none", false;
    ContainerType, "container-type", Grammar::Keywords(&[Keyword::Normal,Keyword::InlineSize,Keyword::Size]), "normal", false;
    Content, "content", Grammar::Content, "normal", false;
    Direction, "direction", Grammar::Keywords(&[Keyword::Ltr,Keyword::Rtl]), "ltr", true;
    Display, "display", Grammar::Display, "inline", false;
    Fill, "fill", Grammar::Color(&[Keyword::None]), "black", true;
    FillOpacity, "fill-opacity", Grammar::Numeric(NumericKind::NumberPercentage, Range::Unit, &[]), "1", true;
    FillRule, "fill-rule", Grammar::Keywords(&[Keyword::Nonzero,Keyword::Evenodd]), "nonzero", true;
    FlexBasis, "flex-basis", Grammar::Size(SizeKind::Basis), "auto", false;
    FlexDirection, "flex-direction", Grammar::Keywords(&[Keyword::Row,Keyword::RowReverse,Keyword::Column,Keyword::ColumnReverse]), "row", false;
    FlexGrow, "flex-grow", Grammar::Numeric(NumericKind::Number, Range::Nonnegative, &[]), "0", false;
    FlexShrink, "flex-shrink", Grammar::Numeric(NumericKind::Number, Range::Nonnegative, &[]), "1", false;
    FlexWrap, "flex-wrap", Grammar::Keywords(&[Keyword::Nowrap,Keyword::Wrap,Keyword::WrapReverse]), "nowrap", false;
    Float, "float", Grammar::Keywords(&[Keyword::None,Keyword::Left,Keyword::Right,Keyword::InlineStart,Keyword::InlineEnd]), "none", false;
    FloodColor, "flood-color", Grammar::Color(&[]), "black", false;
    FloodOpacity, "flood-opacity", Grammar::Numeric(NumericKind::NumberPercentage, Range::Unit, &[]), "1", false;
    FontFamily, "font-family", Grammar::FontFamily, "serif", true;
    FontFeatureSettings, "font-feature-settings", Grammar::Features, "normal", true;
    FontSize, "font-size", Grammar::Numeric(NumericKind::LengthPercentage, Range::Nonnegative, &[Keyword::XxSmall,Keyword::XSmall,Keyword::Small,Keyword::Medium,Keyword::Large,Keyword::XLarge,Keyword::XxLarge,Keyword::XxxLarge,Keyword::Larger,Keyword::Smaller,Keyword::Math]), "medium", true;
    FontStyle, "font-style", Grammar::FontStyle, "normal", true;
    FontWeight, "font-weight", Grammar::Numeric(NumericKind::Number, Range::FontWeight, &[Keyword::Normal,Keyword::Bold,Keyword::Bolder,Keyword::Lighter]), "normal", true;
    FontWidth, "font-width", Grammar::FontWidth, "normal", true;
    GridAutoColumns, "grid-auto-columns", Grammar::Tracks, "auto", false;
    GridAutoFlow, "grid-auto-flow", Grammar::GridAutoFlow, "row", false;
    GridAutoRows, "grid-auto-rows", Grammar::Tracks, "auto", false;
    GridColumnEnd, "grid-column-end", Grammar::GridLine, "auto", false;
    GridColumnStart, "grid-column-start", Grammar::GridLine, "auto", false;
    GridRowEnd, "grid-row-end", Grammar::GridLine, "auto", false;
    GridRowStart, "grid-row-start", Grammar::GridLine, "auto", false;
    GridTemplateColumns, "grid-template-columns", Grammar::Tracks, "none", false;
    GridTemplateRows, "grid-template-rows", Grammar::Tracks, "none", false;
    Height, "height", Grammar::Size(SizeKind::Preferred), "auto", false;
    InlineSize, "inline-size", Grammar::Size(SizeKind::Preferred), "auto", false;
    InsetBlockEnd, "inset-block-end", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "auto", false;
    InsetBlockStart, "inset-block-start", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "auto", false;
    InsetInlineEnd, "inset-inline-end", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "auto", false;
    InsetInlineStart, "inset-inline-start", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "auto", false;
    Isolation, "isolation", Grammar::Keywords(&[Keyword::Auto,Keyword::Isolate]), "auto", false;
    JustifyContent, "justify-content", Grammar::Keywords(&[Keyword::Normal,Keyword::Stretch,Keyword::FlexStart,Keyword::FlexEnd,Keyword::Center,Keyword::Start,Keyword::End,Keyword::Left,Keyword::Right,Keyword::SpaceBetween,Keyword::SpaceAround,Keyword::SpaceEvenly]), "normal", false;
    JustifyItems, "justify-items", Grammar::Keywords(&[Keyword::Normal,Keyword::Stretch,Keyword::FlexStart,Keyword::FlexEnd,Keyword::Center,Keyword::Start,Keyword::End,Keyword::Left,Keyword::Right,Keyword::Baseline]), "normal", false;
    JustifySelf, "justify-self", Grammar::Keywords(&[Keyword::Auto,Keyword::Normal,Keyword::Stretch,Keyword::FlexStart,Keyword::FlexEnd,Keyword::Center,Keyword::Start,Keyword::End,Keyword::Left,Keyword::Right,Keyword::Baseline]), "auto", false;
    Left, "left", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "auto", false;
    LetterSpacing, "letter-spacing", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Normal]), "normal", true;
    LineHeight, "line-height", Grammar::Numeric(NumericKind::LineHeight, Range::Nonnegative, &[Keyword::Normal]), "normal", true;
    ListStyleImage, "list-style-image", Grammar::Images, "none", true;
    ListStylePosition, "list-style-position", Grammar::Keywords(&[Keyword::Outside,Keyword::Inside]), "outside", true;
    ListStyleType, "list-style-type", Grammar::Keywords(&[Keyword::Disc,Keyword::Circle,Keyword::Square,Keyword::Decimal,Keyword::DecimalLeadingZero,Keyword::LowerAlpha,Keyword::UpperAlpha,Keyword::LowerLatin,Keyword::UpperLatin,Keyword::LowerRoman,Keyword::UpperRoman,Keyword::None]), "disc", true;
    MarginBlockEnd, "margin-block-end", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "0", false;
    MarginBlockStart, "margin-block-start", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "0", false;
    MarginBottom, "margin-bottom", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "0", false;
    MarginInlineEnd, "margin-inline-end", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "0", false;
    MarginInlineStart, "margin-inline-start", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "0", false;
    MarginLeft, "margin-left", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "0", false;
    MarginRight, "margin-right", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "0", false;
    MarginTop, "margin-top", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "0", false;
    MaskImage, "mask-image", Grammar::Images, "none", false;
    MaskMode, "mask-mode", Grammar::Keywords(&[Keyword::MatchSource,Keyword::Alpha,Keyword::Luminance]), "match-source", false;
    MaskPosition, "mask-position", Grammar::Positions, "0% 0%", false;
    MaskRepeat, "mask-repeat", Grammar::Repeats, "repeat", false;
    MaskSize, "mask-size", Grammar::Sizes, "auto", false;
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
    OverflowBlock, "overflow-block", Grammar::Overflow, "visible", false;
    OverflowInline, "overflow-inline", Grammar::Overflow, "visible", false;
    OverflowWrap, "overflow-wrap", Grammar::Keywords(&[Keyword::Normal,Keyword::BreakWord,Keyword::Anywhere]), "normal", true;
    OverflowX, "overflow-x", Grammar::Overflow, "visible", false;
    OverflowY, "overflow-y", Grammar::Overflow, "visible", false;
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
    Rx, "rx", Grammar::Numeric(NumericKind::LengthPercentage, Range::Nonnegative, &[Keyword::Auto]), "auto", false;
    Ry, "ry", Grammar::Numeric(NumericKind::LengthPercentage, Range::Nonnegative, &[Keyword::Auto]), "auto", false;
    StopColor, "stop-color", Grammar::Color(&[]), "black", false;
    StopOpacity, "stop-opacity", Grammar::Numeric(NumericKind::NumberPercentage, Range::Unit, &[]), "1", false;
    Stroke, "stroke", Grammar::Color(&[Keyword::None]), "none", true;
    StrokeOpacity, "stroke-opacity", Grammar::Numeric(NumericKind::NumberPercentage, Range::Unit, &[]), "1", true;
    TableLayout, "table-layout", Grammar::Keywords(&[Keyword::Auto,Keyword::Fixed]), "auto", false;
    TextAlign, "text-align", Grammar::Keywords(&[Keyword::Start,Keyword::End,Keyword::Left,Keyword::Right,Keyword::Center,Keyword::Justify,Keyword::MatchParent,Keyword::JustifyAll,Keyword::WebkitCenter,Keyword::WebkitLeft,Keyword::WebkitRight]), "start", true;
    TextAlignLast, "text-align-last", Grammar::Keywords(&[Keyword::Auto,Keyword::Start,Keyword::End,Keyword::Left,Keyword::Right,Keyword::Center,Keyword::Justify]), "auto", true;
    TextBoxEdge, "text-box-edge", Grammar::TextBoxEdge, "auto", true;
    TextBoxTrim, "text-box-trim", Grammar::Keywords(&[Keyword::None,Keyword::TrimStart,Keyword::TrimEnd,Keyword::TrimBoth]), "none", false;
    TextDecorationColor, "text-decoration-color", Grammar::Color(&[]), "currentcolor", false;
    // Decorations propagate to in-flow descendants (CSS Text Decoration 3
    // §2.1); inheriting the line approximates that.
    TextDecorationLine, "text-decoration-line", Grammar::Keywords(&[Keyword::None,Keyword::Underline,Keyword::Overline,Keyword::LineThrough]), "none", true;
    TextDecorationStyle, "text-decoration-style", Grammar::Keywords(&[Keyword::Solid,Keyword::Double,Keyword::Dotted,Keyword::Dashed,Keyword::Wavy]), "solid", false;
    TextOrientation, "text-orientation", Grammar::Keywords(&[Keyword::Mixed,Keyword::Upright,Keyword::Sideways]), "mixed", true;
    TextOverflow, "text-overflow", Grammar::Keywords(&[Keyword::Clip,Keyword::Ellipsis]), "clip", false;
    TextTransform, "text-transform", Grammar::Keywords(&[Keyword::None,Keyword::Capitalize,Keyword::Uppercase,Keyword::Lowercase,Keyword::FullWidth,Keyword::FullSizeKana]), "none", true;
    TextWrapMode, "text-wrap-mode", Grammar::Keywords(&[Keyword::Wrap,Keyword::Nowrap]), "wrap", true;
    TextWrapStyle, "text-wrap-style", Grammar::Keywords(&[Keyword::Auto,Keyword::Balance,Keyword::Stable,Keyword::Pretty]), "auto", true;
    Top, "top", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Auto]), "auto", false;
    Transform, "transform", Grammar::Transforms, "none", false;
    TransformOrigin, "transform-origin", Grammar::Positions, "50% 50%", false;
    TransitionDelay, "transition-delay", Grammar::Animation(super::animation::Field::Delay), "0s", false;
    TransitionDuration, "transition-duration", Grammar::Animation(super::animation::Field::Duration), "0s", false;
    TransitionProperty, "transition-property", Grammar::Animation(super::animation::Field::Property), "all", false;
    TransitionTimingFunction, "transition-timing-function", Grammar::Animation(super::animation::Field::Easing), "ease", false;
    VerticalAlign, "vertical-align", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Baseline,Keyword::Sub,Keyword::Super,Keyword::TextTop,Keyword::TextBottom,Keyword::Middle,Keyword::Top,Keyword::Bottom]), "baseline", false;
    Visibility, "visibility", Grammar::Keywords(&[Keyword::Visible,Keyword::Hidden,Keyword::Collapse]), "visible", true;
    WhiteSpaceCollapse, "white-space-collapse", Grammar::Keywords(&[Keyword::Collapse,Keyword::Preserve,Keyword::PreserveBreaks,Keyword::PreserveSpaces,Keyword::BreakSpaces]), "collapse", true;
    Width, "width", Grammar::Size(SizeKind::Preferred), "auto", false;
    WordBreak, "word-break", Grammar::Keywords(&[Keyword::Normal,Keyword::BreakAll,Keyword::KeepAll,Keyword::AutoPhrase,Keyword::BreakWord]), "normal", true;
    WordSpacing, "word-spacing", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[Keyword::Normal]), "normal", true;
    WritingMode, "writing-mode", Grammar::Keywords(&[Keyword::HorizontalTb,Keyword::VerticalRl,Keyword::VerticalLr,Keyword::SidewaysRl,Keyword::SidewaysLr]), "horizontal-tb", true;
    X, "x", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[]), "0px", false;
    Y, "y", Grammar::Numeric(NumericKind::LengthPercentage, Range::All, &[]), "0px", false;
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
    WebkitMask, "-webkit-mask", [MaskImage,MaskPosition,MaskSize,MaskRepeat], Background;
    WebkitTextDecoration, "-webkit-text-decoration", [TextDecorationLine,TextDecorationStyle,TextDecorationColor], TextDecoration;
    All, "all", [AlignContent,AlignItems,AlignSelf,AnimationDelay,AnimationDirection,AnimationDuration,AnimationFillMode,AnimationIterationCount,AnimationName,AnimationPlayState,AnimationTimingFunction,AspectRatio,BackdropFilter,BackgroundClip,BackgroundColor,BackgroundImage,BackgroundOrigin,BackgroundPosition,BackgroundRepeat,BackgroundSize,BlockSize,BorderBottomColor,BorderBottomLeftRadius,BorderBottomRightRadius,BorderBottomStyle,BorderBottomWidth,BorderLeftColor,BorderLeftStyle,BorderLeftWidth,BorderRightColor,BorderRightStyle,BorderRightWidth,BorderSpacing,BorderTopColor,BorderTopLeftRadius,BorderTopRightRadius,BorderTopStyle,BorderTopWidth,Bottom,BoxShadow,BoxSizing,CaretColor,Clear,ClipPath,ClipRule,Color,ColumnGap,ContainerName,ContainerType,Content,Display,Fill,FillOpacity,FillRule,FlexBasis,FlexDirection,FlexGrow,FlexShrink,FlexWrap,Float,FloodColor,FloodOpacity,FontFamily,FontFeatureSettings,FontSize,FontStyle,FontWeight,FontWidth,GridAutoColumns,GridAutoFlow,GridAutoRows,GridColumnEnd,GridColumnStart,GridRowEnd,GridRowStart,GridTemplateColumns,GridTemplateRows,Height,InlineSize,InsetBlockEnd,InsetBlockStart,InsetInlineEnd,InsetInlineStart,Isolation,JustifyContent,JustifyItems,JustifySelf,Left,LetterSpacing,LineHeight,ListStyleImage,ListStylePosition,ListStyleType,MarginBlockEnd,MarginBlockStart,MarginBottom,MarginInlineEnd,MarginInlineStart,MarginLeft,MarginRight,MarginTop,MaskImage,MaskMode,MaskPosition,MaskRepeat,MaskSize,MaxBlockSize,MaxHeight,MaxInlineSize,MaxWidth,MinBlockSize,MinHeight,MinInlineSize,MinWidth,ObjectFit,Opacity,Order,OverflowBlock,OverflowInline,OverflowWrap,OverflowX,OverflowY,PaddingBlockEnd,PaddingBlockStart,PaddingBottom,PaddingInlineEnd,PaddingInlineStart,PaddingLeft,PaddingRight,PaddingTop,PointerEvents,Position,Resize,Right,RowGap,StopColor,StopOpacity,Stroke,StrokeOpacity,TableLayout,TextAlign,TextAlignLast,TextDecorationColor,TextDecorationLine,TextDecorationStyle,TextBoxEdge,TextBoxTrim,TextOrientation,TextOverflow,TextTransform,TextWrapMode,TextWrapStyle,Top,Transform,TransformOrigin,TransitionDelay,TransitionDuration,TransitionProperty,TransitionTimingFunction,VerticalAlign,Visibility,WhiteSpaceCollapse,Width,WordBreak,WordSpacing,WritingMode,X,Y,Rx,Ry,ZIndex], WideOnly;
    Animation, "animation", [AnimationName,AnimationDuration,AnimationTimingFunction,AnimationDelay,AnimationIterationCount,AnimationDirection,AnimationFillMode,AnimationPlayState], Animation;
    Background, "background", [BackgroundColor,BackgroundImage,BackgroundPosition,BackgroundSize,BackgroundRepeat,BackgroundClip,BackgroundOrigin], Background;
    Border, "border", [BorderTopWidth,BorderRightWidth,BorderBottomWidth,BorderLeftWidth,BorderTopStyle,BorderRightStyle,BorderBottomStyle,BorderLeftStyle,BorderTopColor,BorderRightColor,BorderBottomColor,BorderLeftColor], Border;
    // Logical border shorthands map to physical sides for horizontal-tb,
    // left-to-right, as the logical insets and margins do.
    BorderBlock, "border-block", [BorderTopWidth,BorderBottomWidth,BorderTopStyle,BorderBottomStyle,BorderTopColor,BorderBottomColor], Border;
    BorderBlockEnd, "border-block-end", [BorderBottomWidth,BorderBottomStyle,BorderBottomColor], Border;
    BorderBlockStart, "border-block-start", [BorderTopWidth,BorderTopStyle,BorderTopColor], Border;
    BorderBottom, "border-bottom", [BorderBottomWidth,BorderBottomStyle,BorderBottomColor], Border;
    BorderColor, "border-color", [BorderTopColor,BorderRightColor,BorderBottomColor,BorderLeftColor], Quad;
    BorderInline, "border-inline", [BorderLeftWidth,BorderRightWidth,BorderLeftStyle,BorderRightStyle,BorderLeftColor,BorderRightColor], Border;
    BorderInlineEnd, "border-inline-end", [BorderRightWidth,BorderRightStyle,BorderRightColor], Border;
    BorderInlineStart, "border-inline-start", [BorderLeftWidth,BorderLeftStyle,BorderLeftColor], Border;
    BorderLeft, "border-left", [BorderLeftWidth,BorderLeftStyle,BorderLeftColor], Border;
    BorderRadius, "border-radius", [BorderTopLeftRadius,BorderTopRightRadius,BorderBottomRightRadius,BorderBottomLeftRadius], Radius;
    BorderRight, "border-right", [BorderRightWidth,BorderRightStyle,BorderRightColor], Border;
    BorderStyle, "border-style", [BorderTopStyle,BorderRightStyle,BorderBottomStyle,BorderLeftStyle], Quad;
    BorderTop, "border-top", [BorderTopWidth,BorderTopStyle,BorderTopColor], Border;
    BorderWidth, "border-width", [BorderTopWidth,BorderRightWidth,BorderBottomWidth,BorderLeftWidth], Quad;
    Container, "container", [ContainerName,ContainerType], Container;
    Flex, "flex", [FlexGrow,FlexShrink,FlexBasis], Flex;
    FlexFlow, "flex-flow", [FlexDirection,FlexWrap], Unordered;
    // font resets its longhands and the feature settings (font-variant,
    // font-kerning and the rest are not implemented).
    Font, "font", [FontStyle,FontWeight,FontWidth,FontSize,LineHeight,FontFamily,FontFeatureSettings], Font;
    Gap, "gap", [RowGap,ColumnGap], Repeat;
    Grid, "grid", [GridTemplateRows,GridTemplateColumns,GridAutoRows,GridAutoColumns,GridAutoFlow], Grid;
    GridArea, "grid-area", [GridRowStart,GridColumnStart,GridRowEnd,GridColumnEnd], Grid;
    GridColumn, "grid-column", [GridColumnStart,GridColumnEnd], Grid;
    GridGap, "grid-gap", [RowGap,ColumnGap], Repeat;
    GridRow, "grid-row", [GridRowStart,GridRowEnd], Grid;
    GridTemplate, "grid-template", [GridTemplateRows,GridTemplateColumns], Grid;
    Inset, "inset", [Top,Right,Bottom,Left], Quad;
    InsetBlock, "inset-block", [InsetBlockStart,InsetBlockEnd], Repeat;
    InsetInline, "inset-inline", [InsetInlineStart,InsetInlineEnd], Repeat;
    ListStyle, "list-style", [ListStyleType,ListStylePosition,ListStyleImage], ListStyle;
    Margin, "margin", [MarginTop,MarginRight,MarginBottom,MarginLeft], Quad;
    MarginBlock, "margin-block", [MarginBlockStart,MarginBlockEnd], Repeat;
    MarginInline, "margin-inline", [MarginInlineStart,MarginInlineEnd], Repeat;
    Mask, "mask", [MaskImage,MaskPosition,MaskSize,MaskRepeat], Background;
    Overflow, "overflow", [OverflowX,OverflowY], Repeat;
    Padding, "padding", [PaddingTop,PaddingRight,PaddingBottom,PaddingLeft], Quad;
    PaddingBlock, "padding-block", [PaddingBlockStart,PaddingBlockEnd], Repeat;
    PaddingInline, "padding-inline", [PaddingInlineStart,PaddingInlineEnd], Repeat;
    PlaceContent, "place-content", [AlignContent,JustifyContent], Repeat;
    PlaceItems, "place-items", [AlignItems,JustifyItems], Repeat;
    PlaceSelf, "place-self", [AlignSelf,JustifySelf], Repeat;
    TextBox, "text-box", [TextBoxTrim,TextBoxEdge], TextBox;
    TextDecoration, "text-decoration", [TextDecorationLine,TextDecorationStyle,TextDecorationColor], TextDecoration;
    TextWrap, "text-wrap", [TextWrapMode,TextWrapStyle], TextWrap;
    Transition, "transition", [TransitionProperty,TransitionDuration,TransitionTimingFunction,TransitionDelay], Transition;
    WhiteSpace, "white-space", [WhiteSpaceCollapse,TextWrapMode], WhiteSpace;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Name {
    Longhand(Property),
    Shorthand(Shorthand),
}
impl Name {
    pub fn parse(name: &str) -> Option<Self> {
        if name.eq_ignore_ascii_case("font-stretch") {
            return Some(Self::Longhand(Property::FontWidth));
        }
        if name.eq_ignore_ascii_case("grid-row-gap") {
            return Some(Self::Longhand(Property::RowGap));
        }
        if name.eq_ignore_ascii_case("grid-column-gap") {
            return Some(Self::Longhand(Property::ColumnGap));
        }
        if let Some(rest) = name.strip_prefix("-webkit-mask-").or_else(|| name.strip_prefix("-WEBKIT-MASK-"))
            && let Some(p) = [("image", Property::MaskImage), ("mode", Property::MaskMode), ("position", Property::MaskPosition), ("repeat", Property::MaskRepeat), ("size", Property::MaskSize)]
                .into_iter()
                .find(|(s, _)| rest.eq_ignore_ascii_case(s))
        {
            return Some(Self::Longhand(p.1));
        }
        // -webkit-animation, -webkit-transition, -webkit-backdrop-filter
        // and their longhands: the standard ones.
        if name.len() > 8 && name[..8].eq_ignore_ascii_case("-webkit-") && {
            let rest = name[8..].to_ascii_lowercase();
            rest.starts_with("animation") || rest.starts_with("transition") || rest == "backdrop-filter"
        } {
            return Self::parse(&name[8..]);
        }
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
