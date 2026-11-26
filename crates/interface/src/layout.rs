//! Layout: where everything in a window goes, kept from frame to frame.
//!
//! BeOS's views were placed by hand (a frame and a resizing mode each);
//! Haiku added layouts. Olo's are flexbox and grid (taffy), with every
//! size in device pixels so that what's laid out lands on them. The tree
//! of what's to be laid out is kept: a view that changes marks what it
//! changed, and only that and what its size pushes about is worked out
//! again (taffy keeps each node's answers until it's marked).

use crate::{AbsoluteLength, Bounds, DefiniteLength, Edges, Length, Pixels, Point, Size, Style, point, size};
use std::{fmt::Debug, ops::Range};
use taffy::{
    TaffyTree,
    geometry::{Point as TaffyPoint, Rect as TaffyRect, Size as TaffySize},
    style::AvailableSpace as TaffyAvailableSpace,
};

pub(crate) use taffy::tree::NodeId as LayoutId;

const EXPECT_MESSAGE: &str = "layout errors are avoided by construction";

/// A window's layout tree. `C` says which of the window's nodes a leaf
/// that measures itself is (text wraps to the width it's given).
pub(crate) struct Layout<C> {
    taffy: TaffyTree<C>,
}

impl<C: Copy> Layout<C> {
    pub(crate) fn new() -> Self {
        let mut taffy = TaffyTree::new();
        taffy.enable_rounding();
        Layout { taffy }
    }

    /// A new node, measured by its content if `measured` says which it
    /// is.
    pub(crate) fn add(&mut self, style: &Style, rem_size: Pixels, scale_factor: f32, measured: Option<C>) -> LayoutId {
        let style = style.to_taffy(rem_size, scale_factor);
        match measured {
            Some(context) => self.taffy.new_leaf_with_context(style, context),
            None => self.taffy.new_leaf(style),
        }
        .expect(EXPECT_MESSAGE)
    }

    pub(crate) fn remove(&mut self, id: LayoutId) {
        let _ = self.taffy.remove(id);
    }

    /// Changes a node's style. Whether it changed anything layout goes by
    /// (it's marked to be laid out again only then).
    pub(crate) fn set_style(&mut self, id: LayoutId, style: &Style, rem_size: Pixels, scale_factor: f32) -> bool {
        let style = style.to_taffy(rem_size, scale_factor);
        if self.taffy.style(id).is_ok_and(|old| *old == style) {
            return false;
        }
        self.taffy.set_style(id, style).expect(EXPECT_MESSAGE);
        true
    }

    pub(crate) fn set_children(&mut self, id: LayoutId, children: &[LayoutId]) {
        self.taffy.set_children(id, children).expect(EXPECT_MESSAGE);
    }

    /// Whether a leaf measures itself, and as which node.
    pub(crate) fn set_measured(&mut self, id: LayoutId, measured: Option<C>) {
        self.taffy.set_node_context(id, measured).expect(EXPECT_MESSAGE);
    }

    /// What a node measures as changed (its text): it's laid out again.
    pub(crate) fn mark_dirty(&mut self, id: LayoutId) {
        let _ = self.taffy.mark_dirty(id);
    }

    /// Whether anything under `root` is to be laid out again.
    pub(crate) fn is_dirty(&self, root: LayoutId) -> bool {
        self.taffy.dirty(root).unwrap_or(true)
    }

    /// Lays out what's marked under `root`, in `available` space;
    /// `measure` sizes each leaf that measures itself, given what's known
    /// of its size and the room it has.
    pub(crate) fn compute(&mut self, root: LayoutId, available: Size<AvailableSpace>, scale_factor: f32, mut measure: impl FnMut(C, Size<Option<Pixels>>, Size<AvailableSpace>) -> Size<Pixels>) {
        let scale = |v: AvailableSpace| match v {
            AvailableSpace::Definite(pixels) => AvailableSpace::Definite(Pixels(pixels.0 * scale_factor)),
            other => other,
        };
        let unscale = |v: AvailableSpace| match v {
            AvailableSpace::Definite(pixels) => AvailableSpace::Definite(Pixels(pixels.0 / scale_factor)),
            other => other,
        };
        let available = size(scale(available.width), scale(available.height));
        self.taffy
            .compute_layout_with_measure(root, available.into(), |known, available, _id, context, _style| {
                let Some(context) = context else { return taffy::geometry::Size::default() };
                let known = Size { width: known.width.map(|e| Pixels(e / scale_factor)), height: known.height.map(|e| Pixels(e / scale_factor)) };
                let available: Size<AvailableSpace> = available.into();
                let measured = measure(*context, known, size(unscale(available.width), unscale(available.height)));
                size(measured.width.0 * scale_factor, measured.height.0 * scale_factor).into()
            })
            .expect(EXPECT_MESSAGE);
    }

    /// Where a node is in what holds it, and its size and its content's,
    /// in points.
    pub(crate) fn placed(&self, id: LayoutId, scale_factor: f32) -> (Bounds<Pixels>, Size<Pixels>) {
        let layout = self.taffy.layout(id).expect(EXPECT_MESSAGE);
        (
            Bounds {
                origin: point(Pixels(layout.location.x / scale_factor), Pixels(layout.location.y / scale_factor)),
                size: size(Pixels(layout.size.width / scale_factor), Pixels(layout.size.height / scale_factor)),
            },
            size(Pixels(layout.content_size.width / scale_factor), Pixels(layout.content_size.height / scale_factor)),
        )
    }
}

trait ToTaffy<Output> {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> Output;
}

impl ToTaffy<taffy::style::Style> for Style {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::Style {
        use taffy::style_helpers::{fr, length, minmax, repeat};

        fn to_grid_line(placement: &Range<crate::GridPlacement>) -> taffy::Line<taffy::GridPlacement> {
            taffy::Line { start: placement.start.into(), end: placement.end.into() }
        }

        fn to_grid_repeat<T: taffy::style::CheapCloneStr>(unit: &Option<u16>) -> Vec<taffy::GridTemplateComponent<T>> {
            // grid-template-columns: repeat(<number>, minmax(0, 1fr));
            unit.map(|count| vec![repeat(count, vec![minmax(length(0.0_f32), fr(1.0_f32))])]).unwrap_or_default()
        }

        taffy::style::Style {
            display: self.display.into(),
            overflow: self.overflow.into(),
            scrollbar_width: self.scrollbar_width.to_taffy(rem_size, scale_factor),
            position: self.position.into(),
            inset: self.inset.to_taffy(rem_size, scale_factor),
            size: self.size.to_taffy(rem_size, scale_factor),
            min_size: self.min_size.to_taffy(rem_size, scale_factor),
            max_size: self.max_size.to_taffy(rem_size, scale_factor),
            aspect_ratio: self.aspect_ratio,
            margin: self.margin.to_taffy(rem_size, scale_factor),
            padding: self.padding.to_taffy(rem_size, scale_factor),
            // Olo: whole device pixels, as it's drawn (`Style::border_pixels`).
            border: {
                let w = self.border_widths.to_pixels(rem_size);
                let on = |v: Pixels| taffy::style::LengthPercentage::length(crate::style::border_device_pixels(v, scale_factor));
                taffy::Rect { top: on(w.top), right: on(w.right), bottom: on(w.bottom), left: on(w.left) }
            },
            align_items: self.align_items.map(|x| x.into()),
            align_self: self.align_self.map(|x| x.into()),
            align_content: self.align_content.map(|x| x.into()),
            justify_content: self.justify_content.map(|x| x.into()),
            gap: self.gap.to_taffy(rem_size, scale_factor),
            flex_direction: self.flex_direction.into(),
            flex_wrap: self.flex_wrap.into(),
            flex_basis: self.flex_basis.to_taffy(rem_size, scale_factor),
            flex_grow: self.flex_grow,
            flex_shrink: self.flex_shrink,
            grid_template_rows: to_grid_repeat(&self.grid_rows),
            grid_template_columns: to_grid_repeat(&self.grid_cols),
            grid_row: self.grid_location.as_ref().map(|location| to_grid_line(&location.row)).unwrap_or_default(),
            grid_column: self.grid_location.as_ref().map(|location| to_grid_line(&location.column)).unwrap_or_default(),
            ..Default::default()
        }
    }
}

impl ToTaffy<f32> for AbsoluteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> f32 {
        match self {
            AbsoluteLength::Pixels(pixels) => {
                let pixels: f32 = pixels.into();
                pixels * scale_factor
            }
            AbsoluteLength::Rems(rems) => {
                let pixels: f32 = (*rems * rem_size).into();
                pixels * scale_factor
            }
        }
    }
}

impl ToTaffy<taffy::style::LengthPercentageAuto> for Length {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::prelude::LengthPercentageAuto {
        match self {
            Length::Definite(length) => length.to_taffy(rem_size, scale_factor),
            Length::Auto => taffy::prelude::LengthPercentageAuto::auto(),
        }
    }
}

impl ToTaffy<taffy::style::Dimension> for Length {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::prelude::Dimension {
        match self {
            Length::Definite(length) => length.to_taffy(rem_size, scale_factor),
            Length::Auto => taffy::prelude::Dimension::auto(),
        }
    }
}

impl ToTaffy<taffy::style::LengthPercentage> for DefiniteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::LengthPercentage {
        match self {
            DefiniteLength::Absolute(length) => match length {
                AbsoluteLength::Pixels(pixels) => {
                    let pixels: f32 = pixels.into();
                    taffy::style::LengthPercentage::length(pixels * scale_factor)
                }
                AbsoluteLength::Rems(rems) => {
                    let pixels: f32 = (*rems * rem_size).into();
                    taffy::style::LengthPercentage::length(pixels * scale_factor)
                }
            },
            DefiniteLength::Fraction(fraction) => taffy::style::LengthPercentage::percent(*fraction),
        }
    }
}

impl ToTaffy<taffy::style::LengthPercentageAuto> for DefiniteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::LengthPercentageAuto {
        match self {
            DefiniteLength::Absolute(length) => match length {
                AbsoluteLength::Pixels(pixels) => {
                    let pixels: f32 = pixels.into();
                    taffy::style::LengthPercentageAuto::length(pixels * scale_factor)
                }
                AbsoluteLength::Rems(rems) => {
                    let pixels: f32 = (*rems * rem_size).into();
                    taffy::style::LengthPercentageAuto::length(pixels * scale_factor)
                }
            },
            DefiniteLength::Fraction(fraction) => taffy::style::LengthPercentageAuto::percent(*fraction),
        }
    }
}

impl ToTaffy<taffy::style::Dimension> for DefiniteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::Dimension {
        match self {
            DefiniteLength::Absolute(length) => match length {
                AbsoluteLength::Pixels(pixels) => {
                    let pixels: f32 = pixels.into();
                    taffy::style::Dimension::length(pixels * scale_factor)
                }
                AbsoluteLength::Rems(rems) => taffy::style::Dimension::length((*rems * rem_size * scale_factor).into()),
            },
            DefiniteLength::Fraction(fraction) => taffy::style::Dimension::percent(*fraction),
        }
    }
}

impl ToTaffy<taffy::style::LengthPercentage> for AbsoluteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::LengthPercentage {
        match self {
            AbsoluteLength::Pixels(pixels) => {
                let pixels: f32 = pixels.into();
                taffy::style::LengthPercentage::length(pixels * scale_factor)
            }
            AbsoluteLength::Rems(rems) => {
                let pixels: f32 = (*rems * rem_size).into();
                taffy::style::LengthPercentage::length(pixels * scale_factor)
            }
        }
    }
}

impl<T, T2> From<TaffyPoint<T>> for Point<T2>
where
    T: Into<T2>,
    T2: Clone + Debug + Default + PartialEq,
{
    fn from(point: TaffyPoint<T>) -> Point<T2> {
        Point { x: point.x.into(), y: point.y.into() }
    }
}

impl<T, T2> From<Point<T>> for TaffyPoint<T2>
where
    T: Into<T2> + Clone + Debug + Default + PartialEq,
{
    fn from(val: Point<T>) -> Self {
        TaffyPoint { x: val.x.into(), y: val.y.into() }
    }
}

impl<T, U> ToTaffy<TaffySize<U>> for Size<T>
where
    T: ToTaffy<U> + Clone + Debug + Default + PartialEq,
{
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> TaffySize<U> {
        TaffySize { width: self.width.to_taffy(rem_size, scale_factor), height: self.height.to_taffy(rem_size, scale_factor) }
    }
}

impl<T, U> ToTaffy<TaffyRect<U>> for Edges<T>
where
    T: ToTaffy<U> + Clone + Debug + Default + PartialEq,
{
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> TaffyRect<U> {
        TaffyRect {
            top: self.top.to_taffy(rem_size, scale_factor),
            right: self.right.to_taffy(rem_size, scale_factor),
            bottom: self.bottom.to_taffy(rem_size, scale_factor),
            left: self.left.to_taffy(rem_size, scale_factor),
        }
    }
}

impl<T, U> From<TaffySize<T>> for Size<U>
where
    T: Into<U>,
    U: Clone + Debug + Default + PartialEq,
{
    fn from(taffy_size: TaffySize<T>) -> Self {
        Size { width: taffy_size.width.into(), height: taffy_size.height.into() }
    }
}

impl<T, U> From<Size<T>> for TaffySize<U>
where
    T: Into<U> + Clone + Debug + Default + PartialEq,
{
    fn from(size: Size<T>) -> Self {
        TaffySize { width: size.width.into(), height: size.height.into() }
    }
}

/// The space available for an element to be laid out in
#[derive(Copy, Clone, Default, Debug, Eq, PartialEq)]
pub enum AvailableSpace {
    /// The amount of space available is the specified number of pixels
    Definite(Pixels),
    /// The amount of space available is indefinite and the node should be laid out under a min-content constraint
    #[default]
    MinContent,
    /// The amount of space available is indefinite and the node should be laid out under a max-content constraint
    MaxContent,
}

impl AvailableSpace {
    /// Returns a `Size` with both width and height set to `AvailableSpace::MinContent`.
    ///
    /// This function is useful when you want to create a `Size` with the minimum content constraints
    /// for both dimensions.
    ///
    /// # Examples
    ///
    /// ```
    /// use olo_interface::AvailableSpace;
    /// let min_content_size = AvailableSpace::min_size();
    /// assert_eq!(min_content_size.width, AvailableSpace::MinContent);
    /// assert_eq!(min_content_size.height, AvailableSpace::MinContent);
    /// ```
    pub const fn min_size() -> Size<Self> {
        Size { width: Self::MinContent, height: Self::MinContent }
    }
}

impl From<AvailableSpace> for TaffyAvailableSpace {
    fn from(space: AvailableSpace) -> TaffyAvailableSpace {
        match space {
            AvailableSpace::Definite(Pixels(value)) => TaffyAvailableSpace::Definite(value),
            AvailableSpace::MinContent => TaffyAvailableSpace::MinContent,
            AvailableSpace::MaxContent => TaffyAvailableSpace::MaxContent,
        }
    }
}

impl From<TaffyAvailableSpace> for AvailableSpace {
    fn from(space: TaffyAvailableSpace) -> AvailableSpace {
        match space {
            TaffyAvailableSpace::Definite(value) => AvailableSpace::Definite(Pixels(value)),
            TaffyAvailableSpace::MinContent => AvailableSpace::MinContent,
            TaffyAvailableSpace::MaxContent => AvailableSpace::MaxContent,
        }
    }
}

impl From<Pixels> for AvailableSpace {
    fn from(pixels: Pixels) -> Self {
        AvailableSpace::Definite(pixels)
    }
}

impl From<Size<Pixels>> for Size<AvailableSpace> {
    fn from(size: Size<Pixels>) -> Self {
        Size { width: AvailableSpace::Definite(size.width), height: AvailableSpace::Definite(size.height) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AvailableSpace, Styled as _, div, px};

    fn style(element: crate::Element) -> Style {
        let mut style = Style::default();
        crate::Refineable::refine(&mut style, &element.style);
        style
    }

    /// A tree kept between layouts comes out as a new one would. A
    /// window's tool bar, and under it a side bar beside a column: a
    /// part that takes what's left, and a fixed-height block holding
    /// what fills it. The window is resized: the column is measured
    /// again by its content (the block with it, its children laid out
    /// as it goes with no size to fill), then placed, the block at the
    /// size it had. What fills it must still be its size (taffy 0.9.0
    /// took the block's layout from its cache and left its children as
    /// the measuring pass had: no width).
    #[test]
    fn a_kept_tree_lays_out_again_as_a_new_one_would() {
        let mut layout: Layout<u32> = Layout::new();
        let rem = px(16.0);
        let mut add = |element: crate::Element, children: &[LayoutId]| {
            let id = layout_add(&mut layout, &style(element), rem);
            layout.set_children(id, children);
            id
        };
        let within = add(div().size_full(), &[]);
        let over = add(div().absolute().inset_0(), &[]);
        let fills = add(div().relative().size_full(), &[within, over]);
        let fixed = add(div().h(px(90.0)), &[fills]);
        let grows = add(div().flex_1().min_h_0(), &[]);
        let leaf = add(div().w(px(70.0)).h(px(24.0)).flex_shrink_0(), &[]);
        let column = add(div().flex_1().min_w_0().p(px(16.0)).flex().flex_col().gap(px(16.0)), &[grows, fixed, leaf]);
        let side = add(div().w(px(37.0)).h_full().flex_shrink_0(), &[]);
        let row = add(div().flex_1().min_h_0().flex(), &[side, column]);
        let bar = add(div().w_full().h(px(67.0)).flex_shrink_0(), &[]);
        let root = add(div().size_full().flex().flex_col(), &[bar, row]);
        for height in [400.0, 353.0, 500.0, 353.0] {
            let available = size(AvailableSpace::Definite(px(420.0)), AvailableSpace::Definite(px(height)));
            layout.compute(root, available, 1.0, |_, _, _| Size::default());
            for node in [fills, within, over] {
                assert_eq!(layout.placed(node, 1.0).0.size, size(px(351.0), px(90.0)), "in a window {height} tall");
            }
            assert_eq!(layout.placed(grows, 1.0).0.size.height, px(height - 67.0 - 32.0 - 32.0 - 90.0 - 24.0));
        }
    }

    /// A child that doesn't shrink, with a negative top margin, makes its
    /// column that much shorter, no more: what follows the column comes
    /// right under it, at 1x and 2x (taffy 0.9.0 multiplied the margin
    /// by the child's height working out the column's: a 15-point line
    /// set up 2 points took 30 off, and the form's next row was laid over
    /// the column's last; Passwords' editor, 2026-11-24). And across a
    /// row, a negative left margin.
    #[test]
    fn a_negative_margin_takes_off_what_it_says() {
        for scale in [1.0, 2.0] {
            let mut layout: Layout<u32> = Layout::new();
            let rem = px(16.0);
            let mut add = |element: crate::Element, children: &[LayoutId]| {
                let id = layout.add(&style(element), rem, scale, None);
                layout.set_children(id, children);
                id
            };
            let field = add(div().h(px(28.0)).flex_shrink_0(), &[]);
            let line = add(div().w_full().h(px(15.0)).mt(px(-2.0)).flex_shrink_0(), &[]);
            let buttons = add(div().h(px(28.0)).mt(px(4.0)).flex_shrink_0(), &[]);
            let group = add(div().flex().flex_col(), &[field, line, buttons]);
            let next = add(div().h(px(24.0)).flex_shrink_0(), &[]);
            let form = add(div().size_full().flex().flex_col().gap(px(12.0)), &[group, next]);
            let wide = add(div().w(px(15.0)).h(px(10.0)).ml(px(-2.0)).flex_shrink_0(), &[]);
            let first = add(div().w(px(28.0)).h(px(10.0)).flex_shrink_0(), &[]);
            let pair = add(div().flex(), &[first, wide]);
            let after = add(div().w(px(10.0)).h(px(10.0)).flex_shrink_0(), &[]);
            let row = add(div().flex(), &[pair, after]);
            let root = add(div().size_full().flex().flex_col(), &[form, row]);
            let available = size(AvailableSpace::Definite(px(400.0)), AvailableSpace::Definite(px(400.0)));
            layout.compute(root, available, scale, |_, _, _| Size::default());
            let placed = |id| layout.placed(id, scale).0;
            assert_eq!(placed(group).size.height, px(28.0 + 13.0 + 32.0), "at {scale}x");
            assert_eq!(placed(next).origin.y, px(28.0 + 13.0 + 32.0 + 12.0), "at {scale}x");
            assert_eq!(placed(pair).size.width, px(28.0 + 13.0), "at {scale}x");
            assert_eq!(placed(after).origin.x, px(28.0 + 13.0), "at {scale}x");
        }
    }

    /// What follows a row begins where the row ends, to the device pixel,
    /// however deep each is and whatever the scale: a node's place summed
    /// down the tree is the edge its neighbour's size was cut to. Rex at
    /// 1.25x and 1.75x: a menu bar 26 points high, then a column whose tab
    /// strip is 34 points and whose pane follows it. The strip ended at
    /// device row 75 and the pane began at 76 (two offsets that each end
    /// on half a pixel, rounded each by itself), a row of the window's
    /// ground between them; the layout review's "seam".
    ///
    /// The same sizes as shares, too, which end on any part of a pixel,
    /// not only a half, as whatever layout divides can (shares of a
    /// little more than a window that is whole pixels itself, as windows
    /// are, so that none ends on half a pixel but for what a float loses,
    /// which no rounding puts the same way twice).
    #[test]
    fn what_follows_a_row_begins_where_it_ends_at_every_scale() {
        for (scale, shares) in [1.0, 1.25, 1.5, 1.75, 2.0].into_iter().flat_map(|scale| [(scale, false), (scale, true)]) {
            for (bar, strip) in [(26.0, 34.0), (25.0, 33.0), (26.0, 13.0), (31.0, 17.0)] {
                let mut layout: Layout<u32> = Layout::new();
                let rem = px(16.0);
                let mut add = |element: crate::Element, children: &[LayoutId]| {
                    let id = layout.add(&style(element), rem, scale, None);
                    layout.set_children(id, children);
                    id
                };
                let (wide, tall) = if shares { (800.0, 600.0) } else { (801.0, 601.0) };
                let high = |points: f32| if shares { crate::relative(points / 601.3) } else { px(points).into() };
                let menu = add(div().w_full().h(high(bar)).flex_shrink_0(), &[]);
                let tabs = add(div().w_full().h(high(strip)).flex_shrink_0(), &[]);
                let header = add(div().w_full().h(high(strip)).flex_shrink_0(), &[]);
                let body = add(div().w_full().flex_1(), &[]);
                let view = add(div().size_full().flex().flex_col(), &[header, body]);
                let pane = add(div().flex_1().w_full().flex().flex_col(), &[view]);
                let centre = add(div().flex_1().flex().flex_col(), &[tabs, pane]);
                let side = add(div().w(if shares { crate::relative(13.0 * bar / 801.3) } else { px(13.0 * bar).into() }).h_full().flex_shrink_0(), &[]);
                let row = add(div().flex_1().w_full().flex(), &[side, centre]);
                let root = add(div().size_full().flex().flex_col(), &[menu, row]);
                let available = size(AvailableSpace::Definite(px(wide)), AvailableSpace::Definite(px(tall)));
                layout.compute(root, available, scale, |_, _, _| Size::default());
                // (In device pixels, summed down the tree as `place` sums them.)
                let at = |chain: &[LayoutId]| {
                    let (mut x, mut y) = (0.0, 0.0);
                    for id in chain {
                        let b = layout.placed(*id, scale).0;
                        (x, y) = (x + b.origin.x.0 * scale, y + b.origin.y.0 * scale);
                    }
                    let b = layout.placed(*chain.last().unwrap(), scale).0;
                    (x.round(), y.round(), (x + b.size.width.0 * scale).round(), (y + b.size.height.0 * scale).round())
                };
                let (tabs_at, header_at, body_at) = (at(&[root, row, centre, tabs]), at(&[root, row, centre, pane, view, header]), at(&[root, row, centre, pane, view, body]));
                let (side_at, centre_at) = (at(&[root, row, side]), at(&[root, row, centre]));
                let why = format!("at {scale}x, a bar {bar} and rows {strip} points high{}", if shares { ", as shares of the window" } else { "" });
                assert_eq!(header_at.1, tabs_at.3, "the header's top and the strip's foot, {why}");
                assert_eq!(body_at.1, header_at.3, "the body's top and the header's foot, {why}");
                assert_eq!(centre_at.0, side_at.2, "the centre's left and the side's right, {why}");
                assert_eq!(body_at.3, (tall * scale).round(), "the body's foot and the window's, {why}");
                assert_eq!(centre_at.2, (wide * scale).round(), "the centre's right and the window's, {why}");
            }
        }
    }

    /// What laying out and rounding a window's worth of nodes costs (a
    /// column of 300 rows, each a row of five boxes in a box: 1,801
    /// nodes, one row changed each time, as a list does when a row's
    /// text changes): `jato test --release -p olo-interface
    /// rounding_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn rounding_cost() {
        let scale = 1.25;
        let mut layout: Layout<u32> = Layout::new();
        let rem = px(16.0);
        let mut rows = Vec::new();
        let mut first = None;
        for _ in 0..300 {
            let mut cells = Vec::new();
            for c in 0..5 {
                let cell = layout.add(&style(div().w(px(13.0 + c as f32)).h(px(17.0)).flex_shrink_0()), rem, scale, None);
                cells.push(cell);
            }
            first.get_or_insert(cells[0]);
            let inner = layout.add(&style(div().flex().gap(px(3.0))), rem, scale, None);
            layout.set_children(inner, &cells);
            let row = layout.add(&style(div().w_full().h(px(21.0)).flex_shrink_0().flex()), rem, scale, None);
            layout.set_children(row, &[inner]);
            rows.push(row);
        }
        let root = layout.add(&style(div().size_full().flex().flex_col()), rem, scale, None);
        layout.set_children(root, &rows);
        let available = size(AvailableSpace::Definite(px(801.0)), AvailableSpace::Definite(px(601.0)));
        layout.compute(root, available, scale, |_, _, _| Size::default());
        let cell = first.unwrap();
        let rounds = 2000;
        let t = std::time::Instant::now();
        for i in 0..rounds {
            layout.set_style(cell, &style(div().w(px(13.0 + (i % 2) as f32)).h(px(17.0)).flex_shrink_0()), rem, scale);
            layout.compute(root, available, scale, |_, _, _| Size::default());
            std::hint::black_box(layout.placed(cell, scale));
        }
        println!("1,801 nodes, one changed: {:.1} µs a layout", t.elapsed().as_secs_f64() * 1e6 / f64::from(rounds));
    }

    /// A flex-column item with `w_full().max_w(554)` measures its wrapped
    /// child's height at its clamped width (554 minus padding = 540), not at
    /// the column's full width (2000). Without clamping `child.size.cross` in
    /// taffy's `determine_flex_base_size` / `determine_container_main_size`,
    /// Rex's subagent transcript prose measured 2–4x too short on wide
    /// windows, disabling the scrollbar and clipping text off the bottom.
    #[test]
    fn a_clamped_cross_size_measures_wrapped_height_at_its_max_width() {
        for scale in [1.0, 2.0] {
            let mut layout: Layout<u32> = Layout::new();
            let rem = px(16.0);
            let mut add = |element: crate::Element, children: &[LayoutId], measured: Option<u32>| {
                let id = layout.add(&style(element), rem, scale, measured);
                layout.set_children(id, children);
                id
            };
            let text = add(div(), &[], Some(1));
            let inner = add(div().w_full(), &[text], None);
            let prose = add(div().w_full().max_w(px(554.0)).pl(px(14.0)), &[inner], None);
            let msg = add(div().w_full().flex().flex_col(), &[prose], None);
            let stream = add(div().w_full().flex().flex_col(), &[msg], None);
            let available = size(AvailableSpace::Definite(px(2000.0)), AvailableSpace::Definite(px(1200.0)));
            // Constant-area wrapped text: ~2000 pt tall at 540 pt wide, ~540 pt tall at 2000 pt wide.
            let area = 540.0 * 2000.0;
            layout.compute(stream, available, scale, |_, known, avail| {
                let w = known
                    .width
                    .or(match avail.width {
                        AvailableSpace::Definite(x) => Some(x),
                        _ => None,
                    })
                    .unwrap_or(px(540.0));
                let wf = f32::from(w).max(1.0);
                size(px(wf), px((area / wf).ceil()))
            });
            let placed = |id| layout.placed(id, scale).0;
            assert_eq!(placed(prose).size.width, px(554.0), "at {scale}x");
            assert_eq!(placed(text).size.width, px(540.0), "at {scale}x");
            assert_eq!(placed(prose).size.height, px(2000.0), "prose wrapper must match wrapped text height at {scale}x");
            assert_eq!(placed(msg).size.height, px(2000.0), "message flex column must match wrapped text height at {scale}x");
            assert_eq!(placed(stream).size.height, px(2000.0), "stream column must overflow viewport at {scale}x");
        }
    }

    /// What fills a pane by percentage keeps the pane's size when
    /// something beside it is laid out again. Rescue's terminal, a view
    /// filling the pane over its list of processes: the list's rows
    /// changed, the column was measured again (its panes asked their
    /// size with no height to be a part of), and the terminal was handed
    /// back the layout it had been given in that asking: no height
    /// (taffy 0.9.0 told a node's layouts apart without its parent's
    /// size).
    #[test]
    fn what_fills_a_pane_keeps_its_size_when_a_neighbour_changes() {
        let mut layout: Layout<u32> = Layout::new();
        let rem = px(16.0);
        let mut add = |element: crate::Element, children: &[LayoutId], measured: Option<u32>| {
            let id = layout.add(&style(element), rem, 1.0, measured);
            layout.set_children(id, children);
            id
        };
        let screen = add(div().size_full(), &[], None);
        let fills = add(div().size_full(), &[screen], None);
        let pane = add(div().flex_1().min_h_0(), &[fills], None);
        let heading = add(div().w(px(53.0)).h(px(16.0)), &[], None);
        let shell = add(div().flex_1().min_h_0().flex().flex_col().gap(px(6.0)), &[heading, pane], None);
        let text = add(div(), &[], Some(1));
        let row = add(div().flex().items_center().gap(px(8.0)), &[text], None);
        let list = add(div().flex_1().min_h_0(), &[], None);
        let processes = add(div().h(px(300.0)).flex_shrink_0().flex().flex_col().gap(px(6.0)), &[list, row], None);
        let right = add(div().flex_1().min_w_0().flex().flex_col().gap(px(12.0)), &[shell, processes], None);
        let left = add(div().w(px(300.0)).flex_shrink_0(), &[], None);
        let both = add(div().flex_1().min_h_0().flex().gap(px(16.0)), &[left, right], None);
        let root = add(div().size_full().flex().flex_col().p(px(16.0)), &[both], None);
        let available = size(AvailableSpace::Definite(px(1280.0)), AvailableSpace::Definite(px(800.0)));
        let mut wide = 40.0;
        for round in 0..4 {
            layout.compute(root, available, 1.0, |_, _, _| size(px(wide), px(16.0)));
            let pane_size = layout.placed(pane, 1.0).0.size;
            assert!(pane_size.height > px(300.0), "{pane_size:?}");
            for node in [fills, screen] {
                assert_eq!(layout.placed(node, 1.0).0.size, pane_size, "round {round}");
            }
            // (The note beside the buttons says something else.)
            wide += 25.0;
            layout.mark_dirty(text);
        }
    }

    fn layout_add(layout: &mut Layout<u32>, style: &Style, rem: Pixels) -> LayoutId {
        layout.add(style, rem, 1.0, None)
    }
}
