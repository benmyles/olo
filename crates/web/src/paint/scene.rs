use super::*;
use crate::{
    css::style::{Keyword as K, Property as P, Style, Value},
    dom::Document,
    layout::{BoxId, BoxKind, Point, Rect},
    text::shaping::Direction,
};
use std::{ops::Range, sync::Arc};
const NONE: u32 = u32::MAX;
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub canvas: [u8; 4],
}
impl Default for Options {
    fn default() -> Self {
        Self { canvas: [255; 4] }
    }
}
#[derive(Clone, Debug)]
pub enum Primitive {
    Fill { color: [u8; 4] },
    Border { owner: BoxId, colors: [[u8; 4]; 4] },
    Text { owner: BoxId, fragment: u32, glyphs: Range<u32>, origin: Point, color: [u8; 4] },
}
#[derive(Clone, Debug)]
pub struct Command {
    pub bounds: Rect,
    pub primitive: Primitive,
    pub(super) clip: u32,
}
pub struct Scene {
    pub(super) layout: Arc<layout::Layout>,
    pub(super) commands: Box<[Command]>,
    pub(super) clips: Box<[Rect]>,
    pub(super) index: index::Index,
    pub(super) base: [u8; 4],
    pub(super) canvas: [u8; 4],
    extent: Rect,
}
impl Scene {
    pub fn layout(&self) -> &Arc<layout::Layout> {
        &self.layout
    }
    pub fn commands(&self) -> &[Command] {
        &self.commands
    }
    pub fn content_extent(&self) -> Rect {
        self.extent
    }
    pub fn device_scale(&self) -> f64 {
        self.layout.device_scale()
    }
    pub fn command_payload_bytes(&self) -> usize {
        self.commands.len() * size_of::<Command>() + self.clips.len() * size_of::<Rect>() + self.index.payload_bytes()
    }
    pub fn query(&self, region: Rect, out: &mut Vec<u32>, cancel: Option<&AtomicBool>) -> Result<()> {
        if !valid(region) {
            return Err(Error::Invalid);
        }
        self.index.query(&self.commands, region, out, cancel)?;
        out.retain(|&id| {
            let c = &self.commands[id as usize];
            c.clip == NONE || intersects(self.clips[c.clip as usize], region)
        });
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    pub boxes: usize,
    pub glyphs: usize,
    pub commands: usize,
    pub finished: bool,
}
struct State {
    origin: Point,
    group: u32,
    clip: u32,
}
struct Draft {
    group: u32,
    text: bool,
    command: Command,
}
pub struct BuildJob<'a> {
    doc: &'a Document,
    layout: Arc<layout::Layout>,
    engine: &'a mut Engine,
    limits: Limits,
    cancel: Option<&'a AtomicBool>,
    options: Options,
    canvas: [u8; 4],
    canvas_owner: Option<BoxId>,
    extent: Rect,
    states: Vec<State>,
    draft: Vec<Draft>,
    clips: Vec<Rect>,
    next: usize,
    groups: u32,
    progress: Progress,
    failed: bool,
}
fn color(s: &Style, p: P) -> Result<[u8; 4]> {
    Ok(s.used_color(p)?.srgb8_clipped())
}
fn visible(s: &Style) -> bool {
    matches!(s.value(P::Visibility), Value::Keyword(K::Visible))
}
fn intersection(a: Rect, b: Rect) -> Rect {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    Rect { x, y, width: ((a.x + a.width).min(b.x + b.width) - x).max(0.), height: ((a.y + a.height).min(b.y + b.height) - y).max(0.) }
}
impl<'a> BuildJob<'a> {
    pub fn new(doc: &'a Document, layout: Arc<layout::Layout>, engine: &'a mut Engine, options: Options, limits: Limits, cancel: Option<&'a AtomicBool>) -> Result<Self> {
        check(cancel)?;
        layout.cache().tree().validate(doc)?;
        if options.canvas[3] != 255 {
            return Err(Error::Invalid);
        }
        if layout.cache().tree().boxes().len() > limits.boxes {
            return Err(Error::Limit);
        }
        let tree = layout.cache().tree();
        let mut canvas = [0; 4];
        let mut owner = None;
        if let Some((root, _)) = tree.iter().find(|(_, b)| matches!(b.kind, BoxKind::Block { root: true, .. })) {
            let c = color(tree.style(root).ok_or(Error::Invalid)?, P::BackgroundColor)?;
            canvas = c;
            owner = Some(root);
            if c[3] == 0
                && let Some((body, _)) = tree.iter().find(|(_, b)| matches!(b.kind, BoxKind::Block { .. }) && doc.name(b.node).ok() == Some(Some("body")))
            {
                canvas = color(tree.style(body).ok_or(Error::Invalid)?, P::BackgroundColor)?;
                owner = Some(body)
            }
        }
        Ok(Self {
            doc,
            layout,
            engine,
            limits,
            cancel,
            options,
            canvas,
            canvas_owner: owner,
            extent: Rect::default(),
            states: Vec::new(),
            draft: Vec::new(),
            clips: Vec::new(),
            next: 0,
            groups: 0,
            progress: Progress::default(),
            failed: false,
        })
    }
    pub fn progress(&self) -> Progress {
        self.progress
    }
    fn emit(&mut self, group: u32, text: bool, bounds: Rect, primitive: Primitive, clip: u32) -> Result<()> {
        if !valid(bounds) {
            return Err(Error::Invalid);
        }
        if bounds.width == 0. || bounds.height == 0. {
            return Ok(());
        }
        if self.draft.len() >= self.limits.commands {
            return Err(Error::Limit);
        }
        self.extent = union(self.extent, if clip == NONE { bounds } else { intersection(bounds, self.clips[clip as usize]) });
        self.draft.push(Draft { group, text, command: Command { bounds, primitive, clip } });
        Ok(())
    }
    fn text(&mut self, id: BoxId, source: u32, at: Point, group: u32, clip: u32) -> Result<()> {
        // Layout is held outside self's mutable command builder borrow.
        let layout = self.layout.clone();
        let text = layout.text_layout(id).ok_or(Error::Invalid)?;
        let source = layout.cache().tree().text_source(source).ok_or(Error::Invalid)?;
        let prepared = text.text().prepared();
        let size = text.text().paragraph().font_size;
        for (li, line) in text.lines().iter().enumerate() {
            check(self.cancel)?;
            // Native text has subpixel x placement and a device-pixel baseline.
            // Retain the painted baseline in the scene so its index and agent
            // bounds agree with the actual glyph placement.
            let scale = layout.device_scale();
            let baseline = ((at.y + text.baseline(li).ok_or(Error::Invalid)?) * scale).round() / scale;
            for fi in line.fragments.clone() {
                let f = &text.fragments()[fi as usize];
                let run = text.run(f).ok_or(Error::Invalid)?;
                if !matches!(run.direction, Direction::LeftToRight | Direction::RightToLeft) {
                    return Err(Error::Unsupported("vertical glyph painting"));
                }
                let factor = size / f64::from(run.font.units_per_em());
                let mut pen = 0i64;
                let mut segment: Option<(u32, Point, [u8; 4], Rect)> = None;
                for gi in f.glyphs.clone() {
                    check(self.cancel)?;
                    self.progress.glyphs += 1;
                    if self.progress.glyphs > self.limits.glyph_work {
                        return Err(Error::Limit);
                    }
                    let g = run.glyphs[gi as usize];
                    let mapping = prepared.mapping_at_output(g.cluster).ok_or(Error::Invalid)?;
                    let span = source.spans.get(mapping.span as usize).ok_or(Error::Invalid)?;
                    let s = layout.cache().tree().styles().snapshot(span.style_node).ok_or(Error::Stale)?;
                    let mut c = color(s, P::Color)?;
                    if !visible(s) {
                        c[3] = 0
                    }
                    if span.style_node != layout.cache().tree().get(source.container).ok_or(Error::Invalid)?.style_node && color(s, P::BackgroundColor)?[3] != 0 {
                        return Err(Error::Unsupported("inline backgrounds"));
                    }
                    // A cluster shared across different paint styles needs caret clipping.
                    // Never silently color the entire ligature from its first source span.
                    let stop = if run.direction == Direction::RightToLeft { run.glyphs.partition_point(|n| n.cluster > g.cluster) } else { run.glyphs.partition_point(|n| n.cluster <= g.cluster) };
                    let end = if run.direction == Direction::RightToLeft {
                        stop.checked_sub(1).map_or(run.range.end, |i| run.glyphs[i].cluster)
                    } else {
                        run.glyphs.get(stop).map_or(run.range.end, |g| g.cluster)
                    };
                    let project_end = prepared.mapping_at_output(end.saturating_sub(1)).ok_or(Error::Invalid)?;
                    let input_end = if project_end.kind == crate::text::prepare::MappingKind::Copy { project_end.input.start + (end - project_end.output.start) } else { project_end.input.end };
                    let input_start = if mapping.kind == crate::text::prepare::MappingKind::Copy { mapping.input.start + (g.cluster - mapping.output.start) } else { mapping.input.start };
                    let lo = source.spans.partition_point(|s| s.range.end <= input_start);
                    let hi = source.spans.partition_point(|s| s.range.start < input_end);
                    for next in &source.spans[lo..hi] {
                        let ns = layout.cache().tree().styles().snapshot(next.style_node).ok_or(Error::Stale)?;
                        let mut nc = color(ns, P::Color)?;
                        if !visible(ns) {
                            nc[3] = 0
                        }
                        if nc != c {
                            return Err(Error::Unsupported("paint boundary inside a glyph cluster"));
                        }
                    }
                    let origin = Point { x: at.x + f.x + pen as f64 * factor, y: baseline };
                    let ink = self.engine.ink(&run.font, g.id, self.cancel)?;
                    pen += i64::from(g.x_advance);
                    let bounds = ink.bounds.map(|r| Rect {
                        x: origin.x + (f64::from(g.x_offset) + r.x) * factor,
                        y: baseline - (f64::from(g.y_offset) + r.y + r.height) * factor,
                        width: r.width * factor,
                        height: r.height * factor,
                    });
                    if let Some((start, start_at, old, b)) = segment.take() {
                        if old == c && gi - start < 64 {
                            segment = Some((start, start_at, c, bounds.map_or(b, |n| union(b, n))));
                        } else {
                            self.emit(group, true, b, Primitive::Text { owner: id, fragment: fi, glyphs: start..gi, origin: start_at, color: old }, clip)?;
                        }
                    }
                    if segment.is_none()
                        && let Some(b) = bounds
                        && c[3] != 0
                    {
                        segment = Some((gi, origin, c, b));
                    }
                }
                if let Some((start, origin, c, b)) = segment {
                    self.emit(group, true, b, Primitive::Text { owner: id, fragment: fi, glyphs: start..f.glyphs.end, origin, color: c }, clip)?;
                }
            }
        }
        Ok(())
    }
    pub fn step(&mut self, budget: usize) -> Result<Progress> {
        if self.failed {
            return Err(Error::Failed);
        }
        let r = self.advance(budget);
        if r.is_err() {
            self.failed = true
        }
        r
    }
    fn advance(&mut self, budget: usize) -> Result<Progress> {
        check(self.cancel)?;
        for _ in 0..budget {
            check(self.cancel)?;
            let layout = self.layout.clone();
            let tree = layout.cache().tree();
            let Some((id, b)) = tree.iter().nth(self.next) else {
                self.progress.finished = true;
                break;
            };
            self.next += 1;
            let (parent, mut group, clip) = if let Some(parent) = b.parent {
                let p = &self.states[parent.index()];
                (p.origin, p.group, p.clip)
            } else {
                (Point::default(), 0, NONE)
            };
            let g = *layout.geometry(id).ok_or(Error::Invalid)?;
            let mut outer = g.border_box;
            outer.x += parent.x;
            outer.y += parent.y;
            if !valid(outer) {
                return Err(Error::Invalid);
            }
            self.extent = union(self.extent, if clip == NONE { outer } else { intersection(outer, self.clips[clip as usize]) });
            let local = g.content_box();
            let origin = Point { x: parent.x + local.x, y: parent.y + local.y };
            let mut children_clip = clip;
            match b.kind {
                BoxKind::Viewport => {}
                BoxKind::Block { .. } => {
                    let s = tree.style(id).ok_or(Error::Invalid)?;
                    if s.value(P::Opacity).scalar() != Some(1.)
                        || !matches!(s.value(P::ZIndex), Value::Keyword(K::Auto) | Value::Numeric(_))
                        || s.value(P::ZIndex).scalar().is_some_and(|v| v != 0.)
                        || !matches!(s.value(P::Isolation), Value::Keyword(K::Auto))
                    {
                        return Err(Error::Unsupported("opacity/z-index/isolation stacking"));
                    }
                    if matches!(s.value(P::Position), Value::Keyword(K::Relative)) {
                        self.groups = self.groups.checked_add(1).ok_or(Error::Limit)?;
                        group = self.groups;
                    }
                    if visible(s) {
                        let bg = color(s, P::BackgroundColor)?;
                        if self.canvas_owner != Some(id) && bg[3] != 0 {
                            self.emit(group, false, outer, Primitive::Fill { color: bg }, clip)?;
                        }
                        let widths = [g.border_width.top, g.border_width.right, g.border_width.bottom, g.border_width.left];
                        let mut colors = [[0; 4]; 4];
                        for i in 0..4 {
                            if widths[i] > 0. {
                                let p = [P::BorderTopStyle, P::BorderRightStyle, P::BorderBottomStyle, P::BorderLeftStyle][i];
                                if !matches!(s.value(p), Value::Keyword(K::Solid)) {
                                    return Err(Error::Unsupported("non-solid border painting"));
                                }
                                colors[i] = color(s, [P::BorderTopColor, P::BorderRightColor, P::BorderBottomColor, P::BorderLeftColor][i])?;
                            }
                        }
                        if widths.iter().any(|w| *w > 0.) {
                            self.emit(group, false, outer, Primitive::Border { owner: id, colors }, clip)?;
                        }
                    }
                    let x = !matches!(s.value(P::OverflowX), Value::Keyword(K::Visible));
                    let y = !matches!(s.value(P::OverflowY), Value::Keyword(K::Visible));
                    if x || y {
                        let p = Rect {
                            x: outer.x + g.border_width.left,
                            y: outer.y + g.border_width.top,
                            width: (outer.width - g.border_width.left - g.border_width.right).max(0.),
                            height: (outer.height - g.border_width.top - g.border_width.bottom).max(0.),
                        };
                        let old = if clip == NONE { Rect { x: -1e30, y: -1e30, width: 2e30, height: 2e30 } } else { self.clips[clip as usize] };
                        let clip_rect = intersection(
                            old,
                            Rect { x: if x { p.x } else { old.x }, y: if y { p.y } else { old.y }, width: if x { p.width } else { old.width }, height: if y { p.height } else { old.height } },
                        );
                        children_clip = self.clips.len() as u32;
                        self.clips.push(clip_rect);
                    }
                }
                BoxKind::InlineContent { source } => self.text(id, source, Point { x: outer.x, y: outer.y }, group, clip)?,
            }
            self.states.push(State { origin, group, clip: children_clip });
            self.progress.boxes += 1;
            self.progress.commands = self.draft.len();
        }
        Ok(self.progress)
    }
    pub fn finish(mut self) -> Result<Arc<Scene>> {
        if self.failed {
            return Err(Error::Failed);
        }
        check(self.cancel)?;
        if !self.progress.finished {
            return Err(Error::Incomplete);
        }
        self.layout.cache().tree().validate(self.doc)?;
        self.draft.sort_by_key(|d| (d.group, d.text));
        let commands: Box<[Command]> = self.draft.into_iter().map(|d| d.command).collect();
        let index = index::Index::new(&commands, self.cancel)?;
        Ok(Arc::new(Scene { extent: self.extent, layout: self.layout, commands, clips: self.clips.into_boxed_slice(), index, base: self.options.canvas, canvas: self.canvas }))
    }
}
