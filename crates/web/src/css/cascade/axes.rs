use super::{Defaulted, Error, Resolver, Result};
use crate::css::properties::{Keyword, Property as P, Value};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WritingMode {
    #[default]
    HorizontalTb,
    VerticalRl,
    VerticalLr,
    SidewaysRl,
    SidewaysLr,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Direction {
    #[default]
    Ltr,
    Rtl,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextOrientation {
    #[default]
    Mixed,
    Upright,
    Sideways,
}
/// Computed prerequisites for logical-property projection. Keep the computed
/// direction for inheritance; upright vertical text changes its used value.
/// This does not implement text shaping, bidi or root/body layout propagation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Axes {
    pub writing_mode: WritingMode,
    pub direction: Direction,
    pub text_orientation: TextOrientation,
}
impl Axes {
    pub fn used_direction(self) -> Direction {
        if matches!(self.writing_mode, self::WritingMode::VerticalRl | self::WritingMode::VerticalLr) && self.text_orientation == TextOrientation::Upright {
            self::Direction::Ltr
        } else {
            self.direction
        }
    }
    /// Equivalent implemented longhands, physical first. Other properties have
    /// no pair. The initial/defaulted stage still needs a computed-value host.
    pub fn pair(self, property: P) -> Option<[P; 2]> {
        use P::*;
        let vertical = self.writing_mode != self::WritingMode::HorizontalTb;
        let sizes = |x, y, block, inline| {
            let (px, py) = if vertical { (block, inline) } else { (inline, block) };
            if property == x || property == px { [x, px] } else { [y, py] }
        };
        let sides = |physical: [P; 4], logical: [P; 4]| {
            // Physical order top/right/bottom/left; logical order
            // block-start/block-end/inline-start/inline-end.
            let block_start = match self.writing_mode {
                self::WritingMode::HorizontalTb => 0,
                self::WritingMode::VerticalRl | self::WritingMode::SidewaysRl => 1,
                self::WritingMode::VerticalLr | self::WritingMode::SidewaysLr => 3,
            };
            let mut inline_start = if !vertical {
                3
            } else if self.writing_mode == self::WritingMode::SidewaysLr {
                2
            } else {
                0
            };
            if self.used_direction() == self::Direction::Rtl {
                inline_start = (inline_start + 2) % 4;
            }
            let positions = [block_start, (block_start + 2) % 4, inline_start, (inline_start + 2) % 4];
            for i in 0..4 {
                if property == logical[i] || property == physical[positions[i]] {
                    return [physical[positions[i]], logical[i]];
                }
            }
            unreachable!()
        };
        Some(match property {
            Width | Height | BlockSize | InlineSize => sizes(Width, Height, BlockSize, InlineSize),
            MinWidth | MinHeight | MinBlockSize | MinInlineSize => sizes(MinWidth, MinHeight, MinBlockSize, MinInlineSize),
            MaxWidth | MaxHeight | MaxBlockSize | MaxInlineSize => sizes(MaxWidth, MaxHeight, MaxBlockSize, MaxInlineSize),
            OverflowX | OverflowY | OverflowBlock | OverflowInline => sizes(OverflowX, OverflowY, OverflowBlock, OverflowInline),
            Top | Right | Bottom | Left | InsetBlockStart | InsetBlockEnd | InsetInlineStart | InsetInlineEnd => {
                sides([Top, Right, Bottom, Left], [InsetBlockStart, InsetBlockEnd, InsetInlineStart, InsetInlineEnd])
            }
            MarginTop | MarginRight | MarginBottom | MarginLeft | MarginBlockStart | MarginBlockEnd | MarginInlineStart | MarginInlineEnd => {
                sides([MarginTop, MarginRight, MarginBottom, MarginLeft], [MarginBlockStart, MarginBlockEnd, MarginInlineStart, MarginInlineEnd])
            }
            PaddingTop | PaddingRight | PaddingBottom | PaddingLeft | PaddingBlockStart | PaddingBlockEnd | PaddingInlineStart | PaddingInlineEnd => {
                sides([PaddingTop, PaddingRight, PaddingBottom, PaddingLeft], [PaddingBlockStart, PaddingBlockEnd, PaddingInlineStart, PaddingInlineEnd])
            }
            _ => return None,
        })
    }
    pub fn physical(self, property: P) -> P {
        self.pair(property).map_or(property, |pair| pair[0])
    }
}
impl Resolver<'_> {
    /// Resolve prerequisites before querying paired properties. The host must
    /// supply the coherent parent's computed axes, including its raw direction.
    pub fn axes(&mut self, parent: Option<Axes>, allow_animation_tainted: bool) -> Result<Axes> {
        let keyword = |value, inherited: Option<Keyword>, initial| -> Result<Keyword> {
            match value {
                Defaulted::Inherit => inherited.ok_or(Error::Unsupported("computed parent axes")),
                Defaulted::Initial => Ok(initial),
                Defaulted::Specified(Value::Keyword(k)) => Ok(k),
                _ => Err(Error::Invalid),
            }
        };
        let wm = |m| match m {
            self::WritingMode::HorizontalTb => Keyword::HorizontalTb,
            self::WritingMode::VerticalRl => Keyword::VerticalRl,
            self::WritingMode::VerticalLr => Keyword::VerticalLr,
            self::WritingMode::SidewaysRl => Keyword::SidewaysRl,
            self::WritingMode::SidewaysLr => Keyword::SidewaysLr,
        };
        let writing_mode = match keyword(self.property(P::WritingMode, allow_animation_tainted)?, parent.map(|p| wm(p.writing_mode)), Keyword::HorizontalTb)? {
            Keyword::HorizontalTb => self::WritingMode::HorizontalTb,
            Keyword::VerticalRl => self::WritingMode::VerticalRl,
            Keyword::VerticalLr => self::WritingMode::VerticalLr,
            Keyword::SidewaysRl => self::WritingMode::SidewaysRl,
            Keyword::SidewaysLr => self::WritingMode::SidewaysLr,
            _ => return Err(Error::Invalid),
        };
        let direction =
            match keyword(self.property(P::Direction, allow_animation_tainted)?, parent.map(|p| if p.direction == self::Direction::Ltr { Keyword::Ltr } else { Keyword::Rtl }), Keyword::Ltr)? {
                Keyword::Ltr => self::Direction::Ltr,
                Keyword::Rtl => self::Direction::Rtl,
                _ => return Err(Error::Invalid),
            };
        let orientation = |o| match o {
            TextOrientation::Mixed => Keyword::Mixed,
            TextOrientation::Upright => Keyword::Upright,
            TextOrientation::Sideways => Keyword::Sideways,
        };
        let text_orientation = match keyword(self.property(P::TextOrientation, allow_animation_tainted)?, parent.map(|p| orientation(p.text_orientation)), Keyword::Mixed)? {
            Keyword::Mixed => TextOrientation::Mixed,
            Keyword::Upright => TextOrientation::Upright,
            Keyword::Sideways => TextOrientation::Sideways,
            _ => return Err(Error::Invalid),
        };
        Ok(Axes { writing_mode, direction, text_orientation })
    }
}
