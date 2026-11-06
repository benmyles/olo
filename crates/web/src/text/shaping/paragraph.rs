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
pub struct Paragraph {
    pub analysis: Analysis,
    pub runs: Box<[Run]>,
    pub report: ParagraphReport,
    pub font_size: f64,
    pub language: Box<str>,
    request: FontRequest,
    font_context: u64,
}
impl Paragraph {
    pub fn request(&self) -> &FontRequest {
        &self.request
    }
    pub fn font_context(&self) -> u64 {
        self.font_context
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
        Ok(p.range.start..end)
    }
    pub fn glyph_payload_bytes(&self) -> usize {
        self.runs.iter().map(Run::glyph_payload_bytes).sum()
    }
    pub fn advance(&self) -> (f64, f64) {
        self.runs.iter().map(|r| r.advance(self.font_size)).fold((0., 0.), |(x, y), (a, b)| (x + a, y + b))
    }
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
}
impl<'a> ParagraphJob<'a> {
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
        })
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
    fn choice(&mut self, text: &str) -> Result<Choice> {
        if let Some(choice) = self.fallback.get(text) {
            return Ok(choice.clone());
        }
        let strict = self.scope.cluster_selection(&self.request, text, true, self.cancel).map_err(Error::from)?;
        let (selection, relaxed) =
            if let Some(s) = strict { (Some(s), false) } else { (self.scope.cluster_selection(&self.request, text, false, self.cancel).map_err(Error::from)?, text.chars().any(variation_selector)) };
        let missing = selection.is_none();
        let font = if let Some(s) = selection { self.engine.font(&s)?.0 } else { self.primary.as_ref().ok_or(Error::Invalid)?.clone() };
        let choice = Choice { font, missing, relaxed };
        let bytes = text.len() + size_of::<(Box<str>, Choice)>() * 4;
        if self.fallback.len() < self.limits.fallback_cache_entries && self.fallback_bytes.saturating_add(bytes) <= self.limits.fallback_cache_bytes {
            self.fallback.insert(Box::from(text), choice.clone());
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
        let run = self.engine.shape_run(
            p.font.selection(),
            self.analysis.source.clone(),
            p.range.clone(),
            RunOptions {
                direction: if p.level % 2 == 0 { Direction::LeftToRight } else { Direction::RightToLeft },
                script: p.script,
                language: Some(self.scope.language()),
                features: &[],
                beginning_of_text: p.range.start == para.range.start,
                end_of_text: p.range.end == context_end,
                context: Some((para.range.start, context_end)),
            },
            self.cancel,
        )?;
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
            let primary = primary.as_ref().ok_or(Error::Invalid)?;
            let charmap = charmap.as_ref().ok_or(Error::Invalid)?;
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
            let choice = if covered { Choice { font: primary.clone(), missing: false, relaxed: false } } else { self.choice(text)? };
            self.report.missing_clusters += usize::from(choice.missing);
            self.report.variation_fallbacks += usize::from(choice.relaxed);
            let same = self.pending.as_ref().is_some_and(|p| {
                p.level == level && p.script == script && p.paragraph == paragraph && (Arc::ptr_eq(&p.font, &choice.font) || FontKey::new(p.font.selection()) == FontKey::new(choice.font.selection()))
            });
            if !same {
                self.flush()?;
                self.pending = Some(Pending { range: range.clone(), level, script, paragraph, font: choice.font });
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
            request: self.request,
            font_context: self.scope.context(),
        })
    }
}
