use olo_web::css::{
    Syntax,
    cascade::{self, Axes, Builder, Defaulted, Direction, Origin, Priority, TextOrientation, WritingMode},
    math,
    properties::{Declaration, Property as P, Value},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn declaration(s: &str) -> Declaration {
    let s = Syntax::parse(Arc::from(s), Default::default(), None).unwrap();
    Declaration::compile(&s.one_declaration(None).unwrap(), Default::default(), None).unwrap()
}
fn rows(source: &str) -> Vec<(Declaration, Priority)> {
    let s = Syntax::parse(Arc::from(source), Default::default(), None).unwrap();
    s.declarations(None)
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, d)| (Declaration::compile(d, Default::default(), None).unwrap(), Priority { declaration: i as u32, rule: 1, ..Default::default() }))
        .collect()
}
fn snapshot<'a>(rows: &'a [(Declaration, Priority)], has_parent: bool) -> cascade::Cascade<'a> {
    let mut b = Builder::new(Default::default(), None);
    for (d, p) in rows {
        b.add(d, *p).unwrap();
    }
    b.finish(has_parent, None).unwrap()
}
fn number(v: Defaulted) -> f64 {
    let Defaulted::Specified(Value::Numeric(n)) = v else { panic!("{v:?}") };
    n.resolve(&math::NoContext, &mut Default::default(), Default::default(), None).unwrap()
}
#[test]
fn all_ten_mode_direction_combinations_map_every_supported_group() {
    // Independent literal table from CSS Writing Modes 4 §6.4.
    use WritingMode::*;
    for (mode, direction, sides) in [
        (HorizontalTb, Direction::Ltr, [0, 2, 3, 1]),
        (HorizontalTb, Direction::Rtl, [0, 2, 1, 3]),
        (VerticalRl, Direction::Ltr, [1, 3, 0, 2]),
        (VerticalRl, Direction::Rtl, [1, 3, 2, 0]),
        (SidewaysRl, Direction::Ltr, [1, 3, 0, 2]),
        (SidewaysRl, Direction::Rtl, [1, 3, 2, 0]),
        (VerticalLr, Direction::Ltr, [3, 1, 0, 2]),
        (VerticalLr, Direction::Rtl, [3, 1, 2, 0]),
        (SidewaysLr, Direction::Ltr, [3, 1, 2, 0]),
        (SidewaysLr, Direction::Rtl, [3, 1, 0, 2]),
    ] {
        let axes = Axes { writing_mode: mode, direction, ..Default::default() };
        let upright = Axes { text_orientation: TextOrientation::Upright, ..axes };
        if matches!(mode, VerticalRl | VerticalLr) {
            assert_eq!(upright.used_direction(), Direction::Ltr);
            assert_eq!(upright.physical(P::MarginInlineStart), P::MarginTop);
        } else {
            // Sideways writing modes use horizontal typographic conventions;
            // text-orientation has no effect in them (Writing Modes 4 §5.1).
            assert_eq!(upright.used_direction(), direction);
            assert_eq!(upright.pair(P::MarginTop), axes.pair(P::MarginTop));
        }
        for (physical, logical) in [
            ([P::Top, P::Right, P::Bottom, P::Left], [P::InsetBlockStart, P::InsetBlockEnd, P::InsetInlineStart, P::InsetInlineEnd]),
            ([P::MarginTop, P::MarginRight, P::MarginBottom, P::MarginLeft], [P::MarginBlockStart, P::MarginBlockEnd, P::MarginInlineStart, P::MarginInlineEnd]),
            ([P::PaddingTop, P::PaddingRight, P::PaddingBottom, P::PaddingLeft], [P::PaddingBlockStart, P::PaddingBlockEnd, P::PaddingInlineStart, P::PaddingInlineEnd]),
        ] {
            for i in 0..4 {
                let p = physical[sides[i]];
                let l = logical[i];
                assert_eq!(axes.pair(p), Some([p, l]));
                assert_eq!(axes.pair(l), Some([p, l]));
                assert_eq!(axes.physical(l), p);
                for source in [format!("{}:3px; {}:4px", p.as_str(), l.as_str()), format!("{}:3px; {}:4px", l.as_str(), p.as_str())] {
                    let rows = rows(&source);
                    let c = snapshot(&rows, false);
                    for query in [p, l] {
                        assert_eq!(number(c.resolver().projected(query, axes, true).unwrap().value), 4.);
                    }
                }
            }
        }
        for [x, y, block, inline] in [
            [P::Width, P::Height, P::BlockSize, P::InlineSize],
            [P::MinWidth, P::MinHeight, P::MinBlockSize, P::MinInlineSize],
            [P::MaxWidth, P::MaxHeight, P::MaxBlockSize, P::MaxInlineSize],
            [P::OverflowX, P::OverflowY, P::OverflowBlock, P::OverflowInline],
        ] {
            let (px, py) = if mode == HorizontalTb { (inline, block) } else { (block, inline) };
            for pair in [[x, px], [y, py]] {
                for p in pair {
                    assert_eq!(axes.pair(p), Some(pair));
                }
            }
        }
        for &p in P::ALL {
            if let Some([physical, logical]) = axes.pair(p) {
                assert_eq!(axes.pair(physical), axes.pair(logical));
                assert_ne!(physical, logical);
            } else {
                assert_eq!(axes.physical(p), p);
            }
        }
    }
}
#[test]
fn pending_values_use_declared_longhand_and_share_shorthand_expansion() {
    let rows = rows("--size:30px; --m:3px 7px; inline-size:var(--size); margin-inline:var(--m); --d:rtl; direction:var(--d); writing-mode:vertical-rl");
    let c = snapshot(&rows, false);
    let mut r = c.resolver();
    let axes = r.axes(None, true).unwrap();
    assert_eq!(axes.direction, Direction::Rtl);
    assert_eq!(number(r.projected(P::Height, axes, true).unwrap().value), 30.);
    assert_eq!(number(r.projected(P::MarginBottom, axes, true).unwrap().value), 3.);
    assert_eq!(number(r.projected(P::MarginTop, axes, true).unwrap().value), 7.);
    assert_eq!(r.pending_cached(), 3);
    // Invalid computed logical value defaults; it cannot reveal the older
    // physical declaration that had already lost the cascade.
    let rows = self::rows("width:10px; inline-size:var(--missing)");
    let c = snapshot(&rows, false);
    assert!(matches!(c.resolver().projected(P::Width, Axes::default(), true).unwrap().value, Defaulted::Initial));
}
#[test]
fn rollback_crosses_aliases_and_respects_origin_layer_rule_and_importance() {
    for (wide, want) in [("revert", 10.), ("revert-layer", 20.), ("revert-rule", 30.)] {
        let rows = [
            (declaration("width:10px"), Priority { origin: Origin::User, rule: 1, ..Default::default() }),
            (declaration("inline-size:20px"), Priority { layer: 0, rule: 2, ..Default::default() }),
            (declaration("width:30px"), Priority { layer: 1, rule: 3, ..Default::default() }),
            (declaration(&format!("inline-size:var(--x,{wide})")), Priority { layer: 1, rule: 4, order: 2, ..Default::default() }),
        ];
        let c = snapshot(&rows, false);
        for query in [P::Width, P::InlineSize] {
            assert_eq!(number(c.resolver().projected(query, Axes::default(), true).unwrap().value), want);
        }
    }
    let rows = [
        (declaration("width:10px"), Priority { layer: 0, rule: 1, ..Default::default() }),
        (declaration("inline-size:revert-layer !important"), Priority { layer: 1, rule: 2, ..Default::default() }),
        (declaration("width:30px !important"), Priority { layer: 2, rule: 3, ..Default::default() }),
    ];
    assert_eq!(number(snapshot(&rows, false).resolver().projected(P::Width, Axes::default(), true).unwrap().value), 10.);
    let rows = [
        (declaration("width:revert-rule !important"), Priority { layer: 1, rule: 1, ..Default::default() }),
        (declaration("inline-size:revert-layer"), Priority { layer: 2, rule: 2, ..Default::default() }),
    ];
    assert!(matches!(snapshot(&rows, false).resolver().projected(P::Width, Axes::default(), true).unwrap().value, Defaulted::Initial));
    let rows = self::rows("inline-size:20px !important; width:30px");
    assert_eq!(number(snapshot(&rows, false).resolver().projected(P::Width, Axes::default(), true).unwrap().value), 20.);
}
#[test]
fn inheritance_retains_declared_source_and_all_prefers_physical_on_ties() {
    let parent_rows = rows("margin-left:11px; margin-right:22px");
    let parent = snapshot(&parent_rows, false);
    let axes = Axes { direction: Direction::Rtl, ..Default::default() };
    for (source, want) in [("margin-inline-start:inherit", 11.), ("margin-right:inherit", 22.), ("all:inherit", 22.), ("all:var(--missing,inherit)", 22.)] {
        let rows = rows(source);
        let c = snapshot(&rows, true);
        for query in [P::MarginRight, P::MarginInlineStart] {
            let result = c.resolver().projected(query, axes, true).unwrap();
            assert!(matches!(result.value, Defaulted::Inherit));
            assert_eq!(number(parent.resolver().projected(result.source, Axes::default(), true).unwrap().value), want);
        }
    }
}
#[test]
fn axes_defaulting_variables_upright_and_parent_direction_are_coherent() {
    let parent = Axes { writing_mode: WritingMode::VerticalRl, direction: Direction::Rtl, text_orientation: TextOrientation::Upright };
    assert_eq!(parent.used_direction(), Direction::Ltr);
    assert_eq!(parent.physical(P::MarginInlineStart), P::MarginTop);
    let rows = rows("writing-mode:var(--missing); text-orientation:sideways");
    let c = snapshot(&rows, true);
    let axes = c.resolver().axes(Some(parent), true).unwrap();
    assert_eq!(axes.writing_mode, parent.writing_mode);
    assert_eq!(axes.direction, Direction::Rtl);
    assert_eq!(axes.used_direction(), Direction::Rtl);
    assert_eq!(axes.physical(P::MarginInlineStart), P::MarginBottom);
    let rows = self::rows("writing-mode:initial; direction:initial; text-orientation:initial");
    assert_eq!(snapshot(&rows, true).resolver().axes(Some(parent), true).unwrap(), Axes::default());
    let rows = self::rows("writing-mode:inherit");
    assert_eq!(snapshot(&rows, false).resolver().axes(None, true).unwrap(), Axes::default());
    assert!(matches!(snapshot(&rows, true).resolver().axes(None, true), Err(cascade::Error::Unsupported("computed parent axes"))));
}
#[test]
fn paired_queries_obey_cancellation_depth_taint_and_memory_limits() {
    let rows = rows("--x:1px; margin-inline:var(--x,2px)");
    let cancelled = AtomicBool::new(false);
    let mut b = Builder::new(Default::default(), Some(&cancelled));
    b.add_tainted(&rows[0].0, rows[0].1, true).unwrap();
    b.add(&rows[1].0, rows[1].1).unwrap();
    let c = b.finish(false, None).unwrap();
    let mut r = c.resolver();
    assert_eq!(number(r.projected(P::MarginLeft, Axes::default(), true).unwrap().value), 1.);
    assert_eq!(number(r.projected(P::MarginRight, Axes::default(), false).unwrap().value), 2.);
    assert_eq!(r.pending_cached(), 2);
    cancelled.store(true, Ordering::Relaxed);
    assert!(matches!(r.projected(P::MarginLeft, Axes::default(), true), Err(cascade::Error::Cancelled)));
    for limits in [cascade::Limits { depth: 0, ..Default::default() }, cascade::Limits { typed_retained_bytes: 0, ..Default::default() }] {
        let mut b = Builder::new(limits, None);
        b.add(&rows[1].0, rows[1].1).unwrap();
        let c = b.finish(false, None).unwrap();
        assert!(matches!(c.resolver().projected(P::MarginLeft, Axes::default(), true), Err(cascade::Error::Limit)));
    }
    let rows = self::rows("width:10px; inline-size:revert-rule");
    let mut b = Builder::new(cascade::Limits { work: 2, ..Default::default() }, None);
    for (d, p) in &rows {
        b.add(d, *p).unwrap();
    }
    let c = b.finish(false, None).unwrap();
    assert!(matches!(c.resolver().projected(P::Width, Axes::default(), true), Err(cascade::Error::Limit)));
}

#[path = "support/cascade_fixtures.rs"]
mod fixtures;
#[test]
fn independent_pinned_logical_ports() {
    let report = fixtures::evaluate(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/logical-fixtures"));
    if let Ok(path) = std::env::var("OLO_LOGICAL_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    let failures: Vec<_> = report["results"].as_array().unwrap().iter().filter(|r| matches!(r["outcome"].as_str(), Some("mismatch" | "error" | "panic"))).collect();
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(report["total"], 5757);
    assert_eq!(report["counts"]["passed"], 5726);
    assert_eq!(report["counts"]["unsupported"], 31);
}
