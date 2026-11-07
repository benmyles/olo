use super::*;
use crate::{
    css::style::{FontRequest, Style, WritingMode},
    font::Scope,
    text::unicode::{Unicode18, ignorable, variation_selector},
};
use unicode_bidi::{BidiDataSource, BidiInfo, Level, ParagraphInfo};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseDirection {
    Auto,
    Ltr,
    Rtl,
}
impl BaseDirection {
    fn level(self) -> Option<Level> {
        match self {
            Self::Auto => None,
            Self::Ltr => Some(Level::ltr()),
            Self::Rtl => Some(Level::rtl()),
        }
    }
}
/// Resolved UAX #9 paragraph data, independent of any particular line width.
/// Unicode 18 properties replace the dependency's older bundled data. Shaping
/// handles mirroring/marks; visual_runs applies L1/L2 only after line breaking.
pub struct Analysis {
    source: Arc<str>,
    levels: Vec<Level>,
    classes: Vec<unicode_bidi::BidiClass>,
    paragraphs: Vec<ParagraphInfo>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisualRun {
    pub range: Range<u32>,
    pub level: u8,
}
impl Analysis {
    pub fn new(source: Arc<str>, base: BaseDirection, byte_limit: usize, cancel: Option<&AtomicBool>) -> Result<Self> {
        check(cancel)?;
        if source.len() > byte_limit || source.len() > u32::MAX as usize {
            return Err(Error::Limit);
        }
        if !cfg!(feature = "baseline-text-ascii") && base != BaseDirection::Rtl && source.bytes().all(|b| (b' '..=b'~').contains(&b)) {
            let levels = vec![Level::ltr(); source.len()];
            let classes = source.bytes().map(|b| Unicode18.bidi_class(char::from(b))).collect();
            let paragraphs = if source.is_empty() { Vec::new() } else { vec![ParagraphInfo { range: 0..source.len(), level: Level::ltr() }] };
            check(cancel)?;
            return Ok(Self { source, levels, classes, paragraphs });
        }
        let (levels, classes, paragraphs) = {
            let bidi = BidiInfo::new_with_data_source(&Unicode18, &source, base.level());
            (bidi.levels, bidi.original_classes, bidi.paragraphs)
        };
        check(cancel)?;
        Ok(Self { source, levels, classes, paragraphs })
    }
    pub fn source(&self) -> &Arc<str> {
        &self.source
    }
    pub fn level_at(&self, byte: usize) -> Option<u8> {
        self.levels.get(byte).map(Level::number)
    }
    pub fn paragraphs(&self) -> impl Iterator<Item = (Range<usize>, u8)> + '_ {
        self.paragraphs.iter().map(|p| (p.range.clone(), p.level.number()))
    }
    pub fn paragraph_level(&self, byte: usize) -> Option<u8> {
        self.paragraphs.get(self.paragraphs.partition_point(|p| p.range.end <= byte)).filter(|p| p.range.contains(&byte)).map(|p| p.level.number())
    }
    pub fn visual_runs(&self, line: Range<usize>) -> Result<Vec<VisualRun>> {
        if line.start > line.end || line.end > self.source.len() || !self.source.is_char_boundary(line.start) || !self.source.is_char_boundary(line.end) {
            return Err(Error::Invalid);
        }
        if line.is_empty() {
            return Ok(Vec::new());
        }
        let paragraph = self.paragraphs.get(self.paragraphs.partition_point(|p| p.range.end <= line.start)).filter(|p| p.range.start <= line.start && p.range.end >= line.end).ok_or(Error::Invalid)?;
        // Copy only this line's byte data, never the entire paragraph per line. The
        // dependency applies L1/L2 against already-resolved paragraph embedding levels.
        let p = ParagraphInfo { range: 0..line.len(), level: paragraph.level };
        let bidi = BidiInfo { text: &self.source[line.clone()], levels: self.levels[line.clone()].to_vec(), original_classes: self.classes[line.clone()].to_vec(), paragraphs: vec![p.clone()] };
        let (levels, runs) = bidi.visual_runs(&p, 0..line.len());
        Ok(runs.into_iter().map(|r| VisualRun { level: levels[r.start].number(), range: (r.start + line.start) as u32..(r.end + line.start) as u32 }).collect())
    }
}
#[derive(Clone, Copy, Debug)]
pub struct ParagraphLimits {
    pub text_bytes: usize,
    pub clusters: usize,
    pub cluster_bytes: usize,
    pub runs: usize,
    pub glyphs: usize,
    pub fallback_cache_entries: usize,
    pub fallback_cache_bytes: usize,
}
impl Default for ParagraphLimits {
    fn default() -> Self {
        Self { text_bytes: 64 * 1024, clusters: 65536, cluster_bytes: 4096, runs: 4096, glyphs: 131072, fallback_cache_entries: 1024, fallback_cache_bytes: 1024 * 1024 }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ParagraphReport {
    /// Layout-mode LF controls, retained in bidi/source data without glyphs.
    pub hard_breaks: usize,
    /// No candidate mapped every non-ignorable scalar in this grapheme.
    pub missing_clusters: usize,
    /// No candidate supported the requested UVS; ordinary base coverage was used.
    pub variation_fallbacks: usize,
    /// Literal glyph zero count, not a whole-font/script conformance verdict.
    pub zero_glyphs: usize,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    pub clusters: usize,
    pub shaped_bytes: usize,
    pub glyphs: usize,
    pub finished: bool,
}
#[derive(Default)]
struct ShapingBreaks(Option<triomphe::ThinArc<(), u32>>);
impl std::ops::Deref for ShapingBreaks {
    type Target = [u32];
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().map_or(&[], |b| &b.slice)
    }
}
pub struct Paragraph {
    pub analysis: Analysis,
    pub runs: Box<[Run]>,
    pub report: ParagraphReport,
    pub font_size: f64,
    pub language: Box<str>,
    /// Ranges styled differently from the paragraph (a bold span, a larger
    /// link, a superscript), in source order, never overlapping.
    pub styles: Box<[StyleRun]>,
    request: FontRequest,
    font_context: u64,
    shaping_breaks: ShapingBreaks,
}
/// A differently styled range of a paragraph: its font, size and the line
/// metrics its inline box contributes (CSS px), with a baseline shift.
#[derive(Clone, Debug)]
pub struct StyleRun {
    pub range: Range<u32>,
    pub request: FontRequest,
    pub size: f64,
    pub line_height: f64,
    pub ascent: f64,
    /// Positive, below the baseline.
    pub descent: f64,
    /// Baseline shift, up positive (superscripts), in CSS px.
    pub shift: f64,
}
impl Paragraph {
    pub fn request(&self) -> &FontRequest {
        &self.request
    }
    /// The baseline shift of a run (0 for the paragraph's own style).
    pub fn shift(&self, run: &Run) -> f64 {
        self.styles.get(run.style as usize).map_or(0., |s| s.shift)
    }
    pub fn font_context(&self) -> u64 {
        self.font_context
    }
    pub fn shaping_breaks(&self) -> &[u32] {
        &self.shaping_breaks
    }
    /// Shaping context for a source interval; excludes its paragraph's LF.
    pub fn context_range(&self, range: Range<usize>) -> Result<Range<usize>> {
        let p = self
            .analysis
            .paragraphs
            .get(self.analysis.paragraphs.partition_point(|p| p.range.end <= range.start))
            .filter(|p| range.start >= p.range.start && range.end <= p.range.end)
            .ok_or(Error::Invalid)?;
        let mut end = p.range.end;
        if end > p.range.start && self.analysis.source.as_bytes()[end - 1] == b'\n' {
            end -= 1;
        }
        if range.end > end {
            return Err(Error::Invalid);
        }
        shaping_context(p.range.start..end, range, &self.shaping_breaks)
    }
    pub fn glyph_payload_bytes(&self) -> usize {
        self.runs.iter().map(Run::glyph_payload_bytes).sum()
    }
    pub fn advance(&self) -> (f64, f64) {
        self.runs.iter().map(|r| r.advance(r.size)).fold((0., 0.), |(x, y), (a, b)| (x + a, y + b))
    }
}
fn shaping_context(context: Range<usize>, range: Range<usize>, boundaries: &[u32]) -> Result<Range<usize>> {
    let lo = boundaries.partition_point(|b| (*b as usize) <= range.start);
    let start = if lo == 0 { context.start } else { context.start.max(boundaries[lo - 1] as usize) };
    let end = boundaries.get(lo).map_or(context.end, |b| context.end.min(*b as usize));
    if range.start < start || range.end > end {
        return Err(Error::Invalid);
    }
    Ok(start..end)
}
struct Cluster {
    range: Range<usize>,
    level: u8,
    script: Script,
    paragraph: usize,
}
#[derive(Clone)]
struct Choice {
    font: Arc<GlyphFont>,
    missing: bool,
    relaxed: bool,
}
struct Pending {
    range: Range<usize>,
    level: u8,
    script: Script,
    paragraph: usize,
    font: Arc<GlyphFont>,
    style: u32,
}
/// One uniform-font-request text worker, preparing unwrapped glyph runs for
/// layout. Whitespace processing, styled inline spans, spacing and final line
/// boundary reshaping remain the layout caller's responsibility. No UI work.
pub struct ParagraphJob<'a> {
    engine: &'a mut Engine,
    scope: &'a Scope,
    request: FontRequest,
    analysis: Analysis,
    clusters: Vec<Cluster>,
    next: usize,
    primary: Option<Arc<GlyphFont>>,
    fallback: HashMap<Box<str>, Choice>,
    fallback_bytes: usize,
    pending: Option<Pending>,
    runs: Vec<Run>,
    limits: ParagraphLimits,
    cancel: Option<&'a AtomicBool>,
    progress: Progress,
    report: ParagraphReport,
    failed: bool,
    allow_lf: bool,
    shaping_breaks: ShapingBreaks,
    inline_objects: Box<[u32]>,
    styles: Vec<StyleRun>,
    /// Each style run's first-available font, resolved once.
    style_primaries: Vec<Option<Arc<GlyphFont>>>,
}
impl<'a> ParagraphJob<'a> {
    /// Differently styled ranges: their boundaries become shaping breaks and
    /// their clusters shape in their own font and size.
    pub fn with_style_runs(mut self, styles: Vec<StyleRun>) -> Result<Self> {
        check(self.cancel)?;
        if self.failed || self.progress.finished || self.next != 0 || self.pending.is_some() || !self.runs.is_empty() || !self.styles.is_empty() {
            return Err(Error::Invalid);
        }
        if styles.len() > self.limits.runs {
            return Err(Error::Limit);
        }
        let source_len = self.analysis.source().len();
        let mut cuts = self.shaping_breaks.to_vec();
        for (i, s) in styles.iter().enumerate() {
            check(self.cancel)?;
            if s.range.start >= s.range.end || s.range.end as usize > source_len || i > 0 && styles[i - 1].range.end > s.range.start || !s.size.is_finite() || s.size < 0. {
                return Err(Error::Invalid);
            }
            if s.range.start > 0 {
                cuts.push(s.range.start);
            }
            if (s.range.end as usize) < source_len {
                cuts.push(s.range.end);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        self.set_shaping_breaks(cuts.into_boxed_slice())?;
        let mut primaries = Vec::with_capacity(styles.len());
        for s in &styles {
            let selected = self.scope.first_available(&s.request, self.cancel).map_err(Error::from)?;
            primaries.push(Some(self.engine.font(&selected)?.0));
        }
        self.styles = styles;
        self.style_primaries = primaries;
        Ok(self)
    }
    /// The style run covering a source offset, if any.
    fn style_at(&self, at: usize) -> u32 {
        let i = self.styles.partition_point(|s| (s.range.end as usize) <= at);
        if self.styles.get(i).is_some_and(|s| (s.range.start as usize) <= at) { i as u32 } else { u32::MAX }
    }
    pub fn new(engine: &'a mut Engine, scope: &'a Scope, request: FontRequest, source: Arc<str>, base: BaseDirection, limits: ParagraphLimits, cancel: Option<&'a AtomicBool>) -> Result<Self> {
        Self::build(engine, scope, request, source, base, limits, cancel, false)
    }
    /// Layout input: LF ends a bidi paragraph and has no glyph. Tabs and other
    /// unprocessed controls remain unsupported until their layout policies exist.
    pub fn for_layout(engine: &'a mut Engine, scope: &'a Scope, request: FontRequest, source: Arc<str>, base: BaseDirection, limits: ParagraphLimits, cancel: Option<&'a AtomicBool>) -> Result<Self> {
        Self::build(engine, scope, request, source, base, limits, cancel, true)
    }
    #[allow(clippy::too_many_arguments)]
    fn build(
        engine: &'a mut Engine,
        scope: &'a Scope,
        request: FontRequest,
        source: Arc<str>,
        base: BaseDirection,
        limits: ParagraphLimits,
        cancel: Option<&'a AtomicBool>,
        allow_lf: bool,
    ) -> Result<Self> {
        check(cancel)?;
        if request.axes.writing_mode != WritingMode::HorizontalTb {
            return Err(Error::Unsupported("vertical paragraph itemization"));
        }
        let analysis = Analysis::new(source, base, limits.text_bytes, cancel)?;
        let has_controls = analysis.classes.iter().any(|c| matches!(c, unicode_bidi::BidiClass::B | unicode_bidi::BidiClass::S));
        if (has_controls && (!allow_lf || analysis.source.char_indices().any(|(i, c)| matches!(analysis.classes[i], unicode_bidi::BidiClass::B | unicode_bidi::BidiClass::S) && c != '\n')))
            || analysis.source.contains(['\u{2028}', '\u{c}'])
        {
            return Err(Error::Unsupported("paragraph controls require CSS whitespace/line processing"));
        }
        let selected = scope.first_available(&request, cancel).map_err(Error::from)?;
        let mut levels = analysis.levels.clone();
        for p in &analysis.paragraphs {
            for visual in analysis.visual_runs(p.range.clone())? {
                let level = Level::new(visual.level).map_err(|_| Error::Invalid)?;
                levels[visual.range.start as usize..visual.range.end as usize].fill(level);
            }
        }
        let mut clusters = Vec::new();
        let mut paragraph = 0;
        for range in crate::text::unicode::graphemes(&analysis.source) {
            let start = range.start;
            let text = &analysis.source[range];
            check(cancel)?;
            if clusters.len() >= limits.clusters || text.len() > limits.cluster_bytes {
                return Err(Error::Limit);
            }
            while analysis.paragraphs.get(paragraph).is_some_and(|p| start >= p.range.end) {
                paragraph += 1;
            }
            let mut script = Script::COMMON;
            for c in text.chars() {
                let s = if !cfg!(feature = "baseline-text-ascii") && c.is_ascii() { if c.is_ascii_alphabetic() { Script::LATIN } else { Script::COMMON } } else { harfrust::unicode::script(c as u32) };
                if s != Script::COMMON && s != Script::INHERITED && !ignorable(c) {
                    script = s;
                    break;
                }
            }
            clusters.push(Cluster { range: start..start + text.len(), level: levels[start].number(), script, paragraph });
        }
        // Common/inherited clusters borrow surrounding script only within one
        // paragraph/embedding-level segment. No grapheme is split for font fallback.
        let mut previous = None;
        for i in 0..clusters.len() {
            if i == 0 || clusters[i].paragraph != clusters[i - 1].paragraph || clusters[i].level != clusters[i - 1].level {
                previous = None;
            }
            if matches!(clusters[i].script, Script::COMMON | Script::INHERITED) {
                if let Some(s) = previous {
                    clusters[i].script = s;
                }
            } else {
                previous = Some(clusters[i].script);
            }
        }
        let mut following = None;
        for i in (0..clusters.len()).rev() {
            if i + 1 == clusters.len() || clusters[i].paragraph != clusters[i + 1].paragraph || clusters[i].level != clusters[i + 1].level {
                following = None;
            }
            if matches!(clusters[i].script, Script::COMMON | Script::INHERITED) {
                if let Some(s) = following {
                    clusters[i].script = s;
                }
            } else {
                following = Some(clusters[i].script);
            }
        }
        let primary = if clusters.iter().all(|c| &analysis.source[c.range.clone()] == "\n") { None } else { Some(engine.font(&selected)?.0) };
        Ok(Self {
            engine,
            scope,
            request,
            analysis,
            clusters,
            next: 0,
            primary,
            fallback: HashMap::new(),
            fallback_bytes: 0,
            pending: None,
            runs: Vec::new(),
            limits,
            cancel,
            progress: Progress::default(),
            report: ParagraphReport::default(),
            failed: false,
            allow_lf,
            shaping_breaks: ShapingBreaks::default(),
            inline_objects: Box::default(),
            styles: Vec::new(),
            style_primaries: Vec::new(),
        })
    }
    /// Explicit inline box edges isolate shaping context without inserting
    /// fictional characters into the text or changing its bidi analysis.
    pub fn with_shaping_breaks(mut self, boundaries: Box<[u32]>) -> Result<Self> {
        check(self.cancel)?;
        if self.failed || self.progress.finished || self.next != 0 || self.pending.is_some() || !self.runs.is_empty() || !self.shaping_breaks.is_empty() {
            return Err(Error::Invalid);
        }
        self.set_shaping_breaks(boundaries)?;
        Ok(self)
    }
    /// U+FFFC carriers owned by atomic inline boxes. They retain their Unicode
    /// bidi/line-break identity but never request a font glyph or fallback.
    pub fn with_inline_objects(mut self, positions: Box<[u32]>) -> Result<Self> {
        check(self.cancel)?;
        if self.failed || self.progress.finished || self.next != 0 || self.pending.is_some() || !self.runs.is_empty() || !self.inline_objects.is_empty() {
            return Err(Error::Invalid);
        }
        if positions.len() > self.limits.clusters {
            return Err(Error::Limit);
        }
        if positions.is_empty() {
            return Ok(self);
        }
        let source = self.analysis.source();
        let mut cuts = self.shaping_breaks.to_vec();
        for (i, &at) in positions.iter().enumerate() {
            check(self.cancel)?;
            let end = at.checked_add(3).ok_or(Error::Invalid)?;
            if source.get(at as usize..end as usize) != Some("\u{fffc}") || i > 0 && positions[i - 1] >= at {
                return Err(Error::Invalid);
            }
            if at > 0 {
                cuts.push(at);
            }
            if (end as usize) < source.len() {
                cuts.push(end);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        self.set_shaping_breaks(cuts.into_boxed_slice())?;
        self.inline_objects = positions;
        Ok(self)
    }
    fn set_shaping_breaks(&mut self, boundaries: Box<[u32]>) -> Result<()> {
        if boundaries.len() > self.limits.clusters {
            return Err(Error::Limit);
        }
        let source = self.analysis.source();
        if boundaries.windows(2).any(|p| p[0] >= p[1]) || boundaries.iter().any(|b| *b == 0 || *b as usize >= source.len() || !source.is_char_boundary(*b as usize)) {
            return Err(Error::Invalid);
        }
        if !boundaries.is_empty() {
            let mut clusters = Vec::new();
            let mut boundary = 0;
            for cluster in std::mem::take(&mut self.clusters) {
                check(self.cancel)?;
                let mut start = cluster.range.start;
                while boundaries.get(boundary).is_some_and(|b| (*b as usize) <= start) {
                    boundary += 1;
                }
                while let Some(b) = boundaries.get(boundary).map(|b| *b as usize).filter(|b| *b < cluster.range.end) {
                    if clusters.len() >= self.limits.clusters {
                        return Err(Error::Limit);
                    }
                    clusters.push(Cluster { range: start..b, level: cluster.level, script: cluster.script, paragraph: cluster.paragraph });
                    start = b;
                    boundary += 1;
                }
                if clusters.len() >= self.limits.clusters {
                    return Err(Error::Limit);
                }
                clusters.push(Cluster { range: start..cluster.range.end, level: cluster.level, script: cluster.script, paragraph: cluster.paragraph });
            }
            self.clusters = clusters;
        }
        self.shaping_breaks = ShapingBreaks((!boundaries.is_empty()).then(|| triomphe::ThinArc::from_header_and_iter((), boundaries.into_vec().into_iter())));
        Ok(())
    }
    pub fn for_style(engine: &'a mut Engine, scope: &'a Scope, style: &Style, source: Arc<str>, base: BaseDirection, limits: ParagraphLimits, cancel: Option<&'a AtomicBool>) -> Result<Self> {
        if style.font().context() != scope.context() {
            return Err(Error::Stale);
        }
        Self::new(engine, scope, style.font().request.clone(), source, base, limits, cancel)
    }
    pub fn for_layout_style(engine: &'a mut Engine, scope: &'a Scope, style: &Style, source: Arc<str>, limits: ParagraphLimits, cancel: Option<&'a AtomicBool>) -> Result<Self> {
        if style.font().context() != scope.context() {
            return Err(Error::Stale);
        }
        let base = if style.axes().direction == crate::css::style::Direction::Rtl { BaseDirection::Rtl } else { BaseDirection::Ltr };
        Self::for_layout(engine, scope, style.font().request.clone(), source, base, limits, cancel)
    }
    pub fn progress(&self) -> Progress {
        self.progress
    }
    pub fn report(&self) -> ParagraphReport {
        self.report
    }
    pub fn completed_runs(&self) -> Result<&[Run]> {
        if self.failed { Err(Error::Failed) } else { Ok(&self.runs) }
    }
    fn choice(&mut self, text: &str, style: u32) -> Result<Choice> {
        // The fallback cache is per style (its request decides the candidates).
        let key: Box<str> = if style == u32::MAX { Box::from(text) } else { Box::from(format!("{style}\u{0}{text}")) };
        if let Some(choice) = self.fallback.get(&key) {
            return Ok(choice.clone());
        }
        let request = self.styles.get(style as usize).map_or(&self.request, |s| &s.request).clone();
        let strict = self.scope.cluster_selection(&request, text, true, self.cancel).map_err(Error::from)?;
        let (selection, relaxed) =
            if let Some(s) = strict { (Some(s), false) } else { (self.scope.cluster_selection(&request, text, false, self.cancel).map_err(Error::from)?, text.chars().any(variation_selector)) };
        let missing = selection.is_none();
        let primary = self.style_primaries.get(style as usize).cloned().flatten().or_else(|| self.primary.clone());
        let font = if let Some(s) = selection { self.engine.font(&s)?.0 } else { primary.ok_or(Error::Invalid)? };
        let choice = Choice { font, missing, relaxed };
        let bytes = key.len() + size_of::<(Box<str>, Choice)>() * 4;
        if self.fallback.len() < self.limits.fallback_cache_entries && self.fallback_bytes.saturating_add(bytes) <= self.limits.fallback_cache_bytes {
            self.fallback.insert(key, choice.clone());
            self.fallback_bytes += bytes;
        }
        Ok(choice)
    }
    fn flush(&mut self) -> Result<()> {
        let Some(p) = self.pending.take() else { return Ok(()) };
        if self.runs.len() >= self.limits.runs {
            return Err(Error::Limit);
        }
        let para = self.analysis.paragraphs.get(p.paragraph).ok_or(Error::Invalid)?;
        let context_end = para.range.end - usize::from(self.allow_lf && self.analysis.source.as_bytes()[para.range.end - 1] == b'\n');
        let context = shaping_context(para.range.start..context_end, p.range.clone(), &self.shaping_breaks)?;
        let mut run = self.engine.shape_run(
            p.font.selection(),
            self.analysis.source.clone(),
            p.range.clone(),
            RunOptions {
                direction: if p.level % 2 == 0 { Direction::LeftToRight } else { Direction::RightToLeft },
                script: p.script,
                language: Some(self.scope.language()),
                features: &[],
                beginning_of_text: p.range.start == context.start,
                end_of_text: p.range.end == context.end,
                context: Some((context.start, context.end)),
            },
            self.cancel,
        )?;
        run.style = p.style;
        run.size = self.styles.get(p.style as usize).map_or(self.request.size, |s| s.size);
        if self.progress.glyphs.saturating_add(run.glyphs.len()) > self.limits.glyphs {
            return Err(Error::Limit);
        }
        self.progress.glyphs += run.glyphs.len();
        self.progress.shaped_bytes += p.range.len();
        self.report.zero_glyphs += run.glyphs.iter().filter(|g| g.id == 0).count();
        self.runs.push(run);
        Ok(())
    }
    pub fn step(&mut self, clusters: usize) -> Result<Progress> {
        if self.failed {
            return Err(Error::Failed);
        }
        let result = self.step_inner(clusters);
        if result.is_err() {
            self.failed = true;
            self.pending = None;
        }
        result
    }
    fn step_inner(&mut self, budget: usize) -> Result<Progress> {
        check(self.cancel)?;
        if self.next == self.clusters.len() {
            self.flush()?;
            self.progress.finished = true;
            return Ok(self.progress);
        }
        let source = self.analysis.source.clone();
        let primary = self.primary.clone();
        let charmap = primary.as_ref().map(|p| p.model.charmap());
        for _ in 0..budget {
            check(self.cancel)?;
            let Some(cluster) = self.clusters.get(self.next) else {
                self.flush()?;
                self.progress.finished = true;
                return Ok(self.progress);
            };
            let range = cluster.range.clone();
            let (level, script, paragraph) = (cluster.level, cluster.script, cluster.paragraph);
            let text = &source[range.clone()];
            if self.allow_lf && text == "\n" {
                self.flush()?;
                self.next += 1;
                self.progress.clusters += 1;
                self.report.hard_breaks += 1;
                continue;
            }
            if !self.inline_objects.is_empty() && self.inline_objects.binary_search(&(range.start as u32)).is_ok() {
                if text != "\u{fffc}" {
                    return Err(Error::Invalid);
                }
                self.flush()?;
                if self.runs.len() >= self.limits.runs {
                    return Err(Error::Limit);
                }
                self.runs.push(Run {
                    source: source.clone(),
                    range: range.start as u32..range.end as u32,
                    font: primary.as_ref().ok_or(Error::Invalid)?.clone(),
                    script,
                    direction: if level % 2 == 0 { Direction::LeftToRight } else { Direction::RightToLeft },
                    glyphs: Box::default(),
                    size: self.request.size,
                    style: u32::MAX,
                });
                self.next += 1;
                self.progress.clusters += 1;
                continue;
            }
            let style = self.style_at(range.start);
            let styled_primary = self.style_primaries.get(style as usize).cloned().flatten();
            let primary = styled_primary.as_ref().or(primary.as_ref()).ok_or(Error::Invalid)?;
            let styled_charmap = styled_primary.as_ref().map(|p| p.model.charmap());
            let charmap = styled_charmap.as_ref().or(charmap.as_ref()).ok_or(Error::Invalid)?;
            let mut previous = None;
            let mut covered = true;
            for c in text.chars() {
                check(self.cancel)?;
                if variation_selector(c) {
                    if previous.is_some_and(|base| charmap.map_unicode_variant(base as u32, c as u32).is_none()) {
                        covered = false;
                        break;
                    }
                } else if !ignorable(c) && charmap.map_unicode(c as u32).is_none() {
                    covered = false;
                    break;
                }
                previous = Some(c);
            }
            let choice = if covered { Choice { font: primary.clone(), missing: false, relaxed: false } } else { self.choice(text, style)? };
            self.report.missing_clusters += usize::from(choice.missing);
            self.report.variation_fallbacks += usize::from(choice.relaxed);
            let same = self.shaping_breaks.binary_search(&(range.start as u32)).is_err()
                && self.pending.as_ref().is_some_and(|p| {
                    p.level == level
                        && p.script == script
                        && p.paragraph == paragraph
                        && p.style == style
                        && (Arc::ptr_eq(&p.font, &choice.font) || FontKey::new(p.font.selection()) == FontKey::new(choice.font.selection()))
                });
            if !same {
                self.flush()?;
                self.pending = Some(Pending { range: range.clone(), level, script, paragraph, font: choice.font, style });
            } else {
                self.pending.as_mut().unwrap().range.end = range.end;
            }
            // Never split blindly at an input cap: that could break a ligature across
            // graphemes. A future line breaker chooses and reshapes safe boundaries.
            if self.pending.as_ref().unwrap().range.len() > self.engine.limits.run_bytes {
                return Err(Error::Limit);
            }
            self.next += 1;
            self.progress.clusters += 1;
        }
        if self.next == self.clusters.len() {
            self.flush()?;
            self.progress.finished = true;
        }
        Ok(self.progress)
    }
    pub fn finish(self) -> Result<Paragraph> {
        if self.failed {
            return Err(Error::Failed);
        }
        check(self.cancel)?;
        if !self.progress.finished {
            return Err(Error::Incomplete);
        }
        Ok(Paragraph {
            analysis: self.analysis,
            runs: self.runs.into_boxed_slice(),
            report: self.report,
            font_size: self.request.size,
            language: Box::from(self.scope.language()),
            styles: self.styles.into_boxed_slice(),
            request: self.request,
            font_context: self.scope.context(),
            shaping_breaks: self.shaping_breaks,
        })
    }
}
