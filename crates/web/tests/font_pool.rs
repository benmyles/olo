use olo_web::{
    css::{
        Syntax,
        cascade::{Origin, RuleSet, SheetBuilder},
        color,
        properties::{self as p, Property as P},
        style::{self, Defaults, FontMetrics, FontRequest, Host, StyleTree},
    },
    dom::Document,
    html,
};
use std::{
    cell::Cell,
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
#[path = "support/style_host.rs"]
mod fixed;
struct Context {
    id: u64,
    calls: Cell<usize>,
    revision: Cell<u64>,
    fail: Cell<bool>,
    context_reads: Cell<usize>,
    switch_after: Cell<Option<usize>>,
}
impl Context {
    fn new(id: u64) -> Self {
        Self { id, calls: Cell::new(0), revision: Cell::new(1), fail: Cell::new(false), context_reads: Cell::new(0), switch_after: Cell::new(None) }
    }
}
impl Host for Context {
    fn revision(&self) -> u64 {
        self.revision.get()
    }
    fn font_context(&self) -> u64 {
        let reads = self.context_reads.get();
        self.context_reads.set(reads + 1);
        self.id + u64::from(self.switch_after.get().is_some_and(|limit| reads >= limit))
    }
    fn scheme(&self) -> color::Scheme {
        color::Scheme::Light
    }
    fn system_color(&self, c: color::SystemColor, s: color::Scheme) -> color::Result<color::Color> {
        fixed::FixtureHost.system_color(c, s)
    }
    fn font_size(&self, k: p::Keyword, n: f64) -> style::Result<f64> {
        fixed::FixtureHost.font_size(k, n)
    }
    fn device_scale(&self) -> f64 {
        1.
    }
    fn font(&self, r: &FontRequest) -> style::Result<FontMetrics> {
        self.calls.set(self.calls.get() + 1);
        if self.fail.get() {
            return Err(style::Error::ContextUnavailable);
        }
        // Supplied data vary with numeric/context/axis inputs; pointer and
        // callback checks also cover family contents. This tests wiring,
        // not native selected-font or rasterization behavior.
        let slope = match r.slant {
            style::FontSlant::Normal => 0.,
            style::FontSlant::Italic => 1.,
            style::FontSlant::Left => 2.,
            style::FontSlant::Right => 3.,
            style::FontSlant::Oblique(a) => 4. + a,
        };
        let metric =
            r.size * 0.5 + r.weight / 1000. + r.width / 100. + slope + r.axes.writing_mode as u8 as f64 + r.axes.direction as u8 as f64 + r.axes.text_orientation as u8 as f64 + self.id as f64;
        Ok(FontMetrics { id: self.id, ex: metric, cap: metric, ch: metric, ic: metric, normal_line_height: r.size * 1.2 })
    }
}
fn inputs(source: &str, css: &str) -> (Document, RuleSet) {
    let doc = html::parse(Arc::from(source), Default::default(), None).unwrap().document;
    let syntax = Syntax::parse(Arc::from(css), Default::default(), None).unwrap();
    let mut b = SheetBuilder::new(Default::default());
    b.add_sheet(&syntax, Origin::Author, None).unwrap();
    let rules = b.finish(None).unwrap();
    assert!(rules.issues().is_empty());
    (doc, rules)
}
fn build<'h>(doc: &Document, rules: &RuleSet, defaults: Arc<Defaults>, limits: style::JobLimits, host: impl Fn(olo_web::dom::NodeId) -> &'h Context) -> (StyleTree, style::FontCacheStats) {
    let mut job = style::Job::new(doc, rules, defaults, Default::default(), limits, None).unwrap();
    while !job.step(3, &host).unwrap().finished {}
    let stats = job.font_cache_stats();
    (job.finish().unwrap(), stats)
}
fn named<'a>(doc: &Document, tree: &'a StyleTree, id: &str) -> &'a style::Style {
    tree.iter().find(|(n, _)| doc.attribute(*n, "id").unwrap() == Some(id)).unwrap().1
}
#[test]
fn nonadjacent_requests_and_separate_equal_family_sources_share_without_reselecting() {
    let host = Context::new(1);
    let (doc, rules) = inputs(
        "<main><i id=a style='font-family:Face,serif'></i><i id=b style='font-family:Other,serif'></i><i id=c style='font-family:Face,serif'></i><i id=d style='font-family:Face,serif'></i></main>",
        "i{font-size:20px;width:1ex}",
    );
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let before = host.calls.get();
    let (tree, stats) = build(&doc, &rules, defaults, Default::default(), |_| &host);
    let (a, b, c, d) = (named(&doc, &tree, "a"), named(&doc, &tree, "b"), named(&doc, &tree, "c"), named(&doc, &tree, "d"));
    assert!(Arc::ptr_eq(a.font(), c.font()));
    assert!(Arc::ptr_eq(a.font(), d.font()));
    assert!(!Arc::ptr_eq(a.font(), b.font()));
    assert_eq!(host.calls.get() - before, 2);
    assert_eq!(stats.hits, 2);
    assert_eq!(stats.entries, 2);
    assert_eq!(a.value(P::Width).scalar(), Some(a.font().metrics.ex));
}
#[test]
fn identical_requests_with_different_subject_contexts_never_share() {
    let a = Context::new(1);
    let b = Context::new(2);
    let (doc, rules) = inputs("<main><i id=a></i><i id=b></i><i id=c></i><i id=d></i></main>", "i{font-size:20px;font-family:Face;width:1ex}");
    let defaults = Defaults::new(&a, Default::default(), None).unwrap();
    let (tree, stats) = build(&doc, &rules, defaults, Default::default(), |n| if matches!(doc.attribute(n, "id").unwrap(), Some("b" | "d")) { &b } else { &a });
    let (sa, sb, sc, sd) = (named(&doc, &tree, "a"), named(&doc, &tree, "b"), named(&doc, &tree, "c"), named(&doc, &tree, "d"));
    assert!(Arc::ptr_eq(sa.font(), sc.font()));
    assert!(Arc::ptr_eq(sb.font(), sd.font()));
    assert!(!Arc::ptr_eq(sa.font(), sb.font()));
    assert_ne!(sa.font().metrics.ex, sb.font().metrics.ex);
    assert_eq!(stats.entries, 2);
    assert_eq!(stats.hits, 2);
}
#[test]
fn every_numeric_slant_and_axis_dimension_is_part_of_the_key() {
    let host = Context::new(1);
    let changes = [
        "",
        "font-size:21px",
        "font-weight:500",
        "font-width:75%",
        "font-style:italic",
        "font-style:left",
        "font-style:right",
        "font-style:oblique 1deg",
        "font-style:oblique 2deg",
        "writing-mode:vertical-rl",
        "writing-mode:vertical-lr",
        "writing-mode:sideways-rl",
        "direction:rtl",
        "text-orientation:upright",
        "text-orientation:sideways",
        "font-family:Other",
    ];
    let mut source = String::from("<main>");
    for repeat in 0..2 {
        for (i, css) in changes.iter().enumerate() {
            source.push_str(&format!("<i id=n{repeat}-{i} style='{css}'></i>"));
        }
    }
    source.push_str("</main>");
    let (doc, rules) = inputs(&source, "i{font-size:20px;font-family:Face;width:1ex}");
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let before = host.calls.get();
    let (tree, stats) = build(&doc, &rules, defaults, Default::default(), |_| &host);
    let mut unique = HashSet::new();
    for i in 0..changes.len() {
        let a = named(&doc, &tree, &format!("n0-{i}"));
        let b = named(&doc, &tree, &format!("n1-{i}"));
        assert!(Arc::ptr_eq(a.font(), b.font()), "dimension {i}");
        unique.insert(Arc::as_ptr(a.font()));
        assert_eq!(a.value(P::Width).scalar(), Some(a.font().metrics.ex));
    }
    assert_eq!(unique.len(), changes.len());
    assert_eq!(host.calls.get() - before, changes.len());
    assert_eq!(stats.entries, changes.len());
    assert_eq!(stats.hits, changes.len());
}
#[test]
fn zero_and_saturated_pools_preserve_values_and_bound_retention() {
    let (doc, rules) = inputs("<main><i id=a></i><b id=b></b><i id=c></i><b id=d></b></main>", "i{font-size:20px;width:1ex}b{font-size:21px;width:1ex}");
    let h = Context::new(1);
    let defaults = Defaults::new(&h, Default::default(), None).unwrap();
    let (off, off_stats) = build(&doc, &rules, defaults.clone(), style::JobLimits { font_cache_entries: 0, ..Default::default() }, |_| &h);
    let (small, small_stats) = build(&doc, &rules, defaults, style::JobLimits { font_cache_entries: 1, ..Default::default() }, |_| &h);
    assert_eq!(off_stats.entries, 0);
    assert_eq!(off_stats.reserved_bytes, 0);
    assert_eq!(small_stats.entries, 1);
    assert_eq!(small_stats.hits, 1);
    assert!(Arc::ptr_eq(named(&doc, &small, "a").font(), named(&doc, &small, "c").font()));
    assert!(!Arc::ptr_eq(named(&doc, &small, "b").font(), named(&doc, &small, "d").font()));
    for id in ["a", "b", "c", "d"] {
        assert_eq!(named(&doc, &off, id).font().metrics, named(&doc, &small, id).font().metrics);
    }
}
#[test]
fn cancellation_and_revision_changes_reject_cached_results_and_poison_the_job() {
    let host = Context::new(1);
    let (doc, rules) = inputs("<main><i id=a></i><i id=b></i></main>", "i{font-size:20px}");
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let cancel = AtomicBool::new(false);
    for stale in [false, true] {
        let mut job = style::Job::new(&doc, &rules, defaults.clone(), Default::default(), Default::default(), Some(&cancel)).unwrap();
        while job.font_cache_stats().entries == 0 {
            job.step(1, |_| &host).unwrap();
        }
        if stale {
            host.revision.set(2);
        } else {
            cancel.store(true, Ordering::Relaxed);
        }
        let result = job.step(10, |_| &host);
        assert!(if stale { matches!(result, Err(style::Error::Stale)) } else { matches!(result, Err(style::Error::Cancelled)) });
        assert!(matches!(job.completed(), Err(style::Error::Failed)));
        assert!(matches!(job.step(1, |_| &host), Err(style::Error::Failed)));
        assert!(matches!(job.finish(), Err(style::Error::Failed)));
        cancel.store(false, Ordering::Relaxed);
        host.revision.set(1);
    }
}

#[test]
fn failed_misses_do_not_publish_and_successful_pools_do_not_outlive_the_tree() {
    let host = Context::new(1);
    let (doc, rules) = inputs("<main><i id=a></i><i id=b></i></main>", "i{font-size:20px}");
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let mut job = style::Job::new(&doc, &rules, defaults.clone(), Default::default(), Default::default(), None).unwrap();
    host.fail.set(true);
    assert!(matches!(job.step(100, |_| &host), Err(style::Error::ContextUnavailable)));
    assert_eq!(job.font_cache_stats().entries, 0);
    assert!(matches!(job.finish(), Err(style::Error::Failed)));
    host.fail.set(false);
    let (tree, stats) = build(&doc, &rules, defaults, Default::default(), |_| &host);
    assert_eq!(stats.entries, 1);
    let weak = Arc::downgrade(named(&doc, &tree, "a").font());
    drop(tree);
    assert!(weak.upgrade().is_none());
}

#[test]
fn context_changes_during_a_pool_hit_are_rejected_before_publication() {
    let host = Context::new(1);
    let (doc, rules) = inputs("<main><i id=a></i><i id=b></i></main>", "i{font-size:20px}");
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let mut job = style::Job::new(&doc, &rules, defaults, Default::default(), Default::default(), None).unwrap();
    while job.font_cache_stats().entries == 0 {
        job.step(1, |_| &host).unwrap();
    }
    host.switch_after.set(Some(host.context_reads.get() + 2));
    assert!(matches!(job.step(1, |_| &host), Err(style::Error::Stale)));
    assert_eq!(job.font_cache_stats().hits, 1);
    assert!(matches!(job.finish(), Err(style::Error::Failed)));
}

#[test]
fn pool_scratch_is_charged_to_the_job_budget() {
    let host = Context::new(1);
    let (doc, rules) = inputs("<i id=a></i>", "i{font-size:20px}");
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let attempt = |entries, bytes| {
        let mut job = style::Job::new(&doc, &rules, defaults.clone(), Default::default(), style::JobLimits { font_cache_entries: entries, retained_bytes: bytes, ..Default::default() }, None).unwrap();
        match job.step(100, |_| &host) {
            Ok(p) => {
                assert!(p.finished);
                let stats = job.font_cache_stats();
                assert!(job.finish().is_ok());
                Some(stats)
            }
            Err(style::Error::Limit) => None,
            Err(e) => panic!("unexpected {e}"),
        }
    };
    let minimum = |entries| {
        let (mut low, mut high) = (0, 1024 * 1024);
        while low < high {
            let mid = low + (high - low) / 2;
            if attempt(entries, mid).is_some() { high = mid } else { low = mid + 1 }
        }
        low
    };
    let off = minimum(0);
    let on = minimum(4096);
    let stats = attempt(4096, on).unwrap();
    assert_eq!(stats.entries, 1);
    assert!(stats.reserved_bytes > 0);
    assert_eq!(on, off + stats.reserved_bytes);
    assert!(attempt(4096, on - 1).is_none());
}

#[test]
fn huge_valid_family_lists_skip_optional_hashing_and_preserve_selection() {
    let host = Context::new(1);
    let family = "a".repeat(70 * 1024);
    let source = format!("<main><i id=a style='font-family:{family}'></i><i id=b style='font-family:{family}'></i></main>");
    let (doc, rules) = inputs(&source, "i{font-size:20px;width:1ex}");
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let (tree, stats) = build(&doc, &rules, defaults, Default::default(), |_| &host);
    assert_eq!(stats.entries, 0);
    assert_eq!(stats.reserved_bytes, 0);
    let a = named(&doc, &tree, "a");
    let b = named(&doc, &tree, "b");
    assert_eq!(a.font().metrics, b.font().metrics);
    assert!(!Arc::ptr_eq(a.font(), b.font()));
}
