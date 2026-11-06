#[path = "support/features.rs"]
mod build_features;
use olo_web::{
    css::{
        Syntax,
        cascade::{Origin, SheetBuilder},
        style::{self, Defaults},
    },
    font::{Builder, FontSet, GenericPolicy, Scope},
    html,
    layout::{self, BoxKind},
    text::shaping::Engine,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{hint::black_box, path::Path, sync::Arc, time::Instant};
#[path = "../tests/support/native_font_host.rs"]
mod native_host;
fn build(doc: &olo_web::dom::Document, styles: Arc<style::StyleTree>, e: &mut Engine, s: &Scope, cached_texts: usize) -> (Arc<layout::Cache>, f64, f64) {
    let mut j = layout::BuildJob::new(doc, styles, Default::default(), None).unwrap();
    let mut max = 0f64;
    loop {
        let at = Instant::now();
        let p = j.step(128).unwrap();
        max = max.max(at.elapsed().as_secs_f64() * 1e6);
        if p.finished {
            break;
        }
    }
    let t = j.finish().unwrap();
    let (c, prepare) = prepare(e, s, t, cached_texts);
    (c, max, prepare)
}
fn prepare(e: &mut Engine, s: &Scope, t: Arc<layout::Tree>, cached_texts: usize) -> (Arc<layout::Cache>, f64) {
    let mut j = layout::PrepareJob::new(e, s, t, layout::Limits { cached_texts, ..Default::default() }, None).unwrap();
    let mut max = 0f64;
    loop {
        let at = Instant::now();
        let p = j.step(1).unwrap();
        max = max.max(at.elapsed().as_secs_f64() * 1e6);
        if p.finished {
            break;
        }
    }
    (j.finish().unwrap(), max)
}
fn fit(e: &mut Engine, s: &Scope, c: Arc<layout::Cache>, width: f64) -> (layout::Layout, f64, f64) {
    let at = Instant::now();
    let mut j = layout::Job::new(e, s, c, layout::Viewport { width, height: 800., scale: 1. }, Default::default(), None).unwrap();
    let ctor = at.elapsed().as_secs_f64() * 1e6;
    let mut max = 0f64;
    loop {
        let at = Instant::now();
        let p = j.step(64).unwrap();
        max = max.max(at.elapsed().as_secs_f64() * 1e6);
        if p.finished {
            break;
        }
    }
    (j.finish().unwrap(), ctor, max)
}
fn digest(l: &layout::Layout) -> String {
    let mut h = Sha256::new();
    for (id, _) in l.cache().tree().iter() {
        let r = l.bounds(id).unwrap();
        for v in [r.x, r.y, r.width, r.height] {
            h.update(v.to_le_bytes())
        }
        if let Some(t) = l.text_layout(id) {
            for line in t.lines() {
                h.update(line.source.start.to_le_bytes());
                h.update(line.source.end.to_le_bytes());
                h.update(line.advance.to_le_bytes());
                for f in &t.fragments()[line.fragments.start as usize..line.fragments.end as usize] {
                    h.update(f.x.to_le_bytes());
                    h.update(f.advance.to_le_bytes());
                    for g in &t.run(f).unwrap().glyphs[f.glyphs.start as usize..f.glyphs.end as usize] {
                        for v in [g.id, g.cluster, g.x_advance as u32, g.y_advance as u32, g.x_offset as u32, g.y_offset as u32] {
                            h.update(v.to_le_bytes())
                        }
                    }
                }
            }
        }
    }
    format!("{:x}", h.finalize())
}
fn main() {
    let rounds: usize = std::env::args().nth(1).unwrap_or_else(|| "10".into()).parse().unwrap();
    assert!(rounds > 0);
    let reprepare = std::env::args().nth(2).as_deref() == Some("reprepare");
    let no_intern = std::env::args().nth(3).as_deref() == Some("no-intern");
    let cached_texts = if no_intern { 0 } else { 4096 };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut b = Builder::new(Default::default());
    b.load_file(&root.join("data/fonts/NotoSans[wght].ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Noto Sans"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut e = Engine::default();
    let mut cases = Vec::new();
    for count in [100, 1000] {
        for scene in ["blocks", "text", "unique", "nested"] {
            let unit = match scene {
                "blocks" => "<div class=row></div>",
                "text" => "<p class=row>office alpha beta gamma delta.</p>",
                _ => "<section><p class=row>office alpha beta gamma.</p><p class=row>delta affine office.</p></section>",
            };
            let body = if scene == "unique" { (0..count).map(|i| format!("<p class=row>office alpha beta gamma delta {i}.</p>")).collect::<String>() } else { unit.repeat(count) };
            let source = Arc::from(format!("<!doctype html><html><head></head><body>{body}</body></html>"));
            let doc = html::parse(source, Default::default(), None).unwrap().document;
            let syntax=Syntax::parse(Arc::from("html,body,div,p,section{display:block;margin:0;padding:0}head{display:none}html{font-family:'Noto Sans';font-size:20px;line-height:24px}body{padding:8px}.row{margin-bottom:5px}div.row{height:10px}section{display:flow-root;padding:1px}"),Default::default(),None).unwrap();
            let mut b = SheetBuilder::new(Default::default());
            b.add_sheet(&syntax, Origin::Author, None).unwrap();
            let rules = b.finish(None).unwrap();
            let defaults = Defaults::new(&host, Default::default(), None).unwrap();
            let mut j = style::Job::new(&doc, &rules, defaults, Default::default(), Default::default(), None).unwrap();
            while !j.step(128, |_| &host).unwrap().finished {}
            let styles = Arc::new(j.finish().unwrap());
            assert_eq!(styles.report().stylesheet_issues, 0);
            let (c, tree_max, prepare_max) = build(&doc, styles.clone(), &mut e, &scope, cached_texts);
            let tree = c.tree().clone();
            let tree_payload = tree.storage_payload_bytes();
            let glyph_payload = c.glyph_geometry_payload_bytes();
            let boxes = tree.boxes().len();
            let texts = tree.iter().filter(|(_, b)| matches!(b.kind, BoxKind::InlineContent { .. })).count();
            let mut max_tree = tree_max;
            let mut max_prepare = prepare_max;
            let at = Instant::now();
            for _ in 0..rounds {
                let (c, t, p) = build(&doc, styles.clone(), &mut e, &scope, cached_texts);
                max_tree = max_tree.max(t);
                max_prepare = max_prepare.max(p);
                black_box(c);
            }
            let build_us = at.elapsed().as_secs_f64() * 1e6 / rounds as f64;
            let retained = if reprepare {
                drop(c);
                None
            } else {
                Some(c)
            };
            for width in [120., 400., 1000.] {
                let c = retained.clone().unwrap_or_else(|| prepare(&mut e, &scope, tree.clone(), cached_texts).0);
                let (l, ctor, max) = fit(&mut e, &scope, c, width);
                let signature = digest(&l);
                let payload = l.geometry_payload_bytes();
                drop(l);
                let mut ctor_max = ctor;
                let mut slice_max = max;
                let mut prepare_max = 0f64;
                let at = Instant::now();
                for _ in 0..rounds {
                    let c = if let Some(c) = &retained {
                        c.clone()
                    } else {
                        let (c, p) = prepare(&mut e, &scope, tree.clone(), cached_texts);
                        prepare_max = prepare_max.max(p);
                        c
                    };
                    let (l, c, s) = fit(&mut e, &scope, c, width);
                    ctor_max = ctor_max.max(c);
                    slice_max = slice_max.max(s);
                    black_box(l);
                }
                cases.push(json!({"scene":scene,"count":count,"width":width,"boxes":boxes,"text_paragraphs":texts,"tree_payload_bytes":tree_payload,"cache_glyph_geometry_payload_bytes":glyph_payload,"layout_geometry_payload_bytes":payload,"digest":signature,"build_tree_prepare_drop_us":build_us,"max_128_action_tree_step_us":max_tree,"max_one_paragraph_prepare_step_us":max_prepare,"resize_drop_us":at.elapsed().as_secs_f64()*1e6/rounds as f64,"max_resize_ctor_us":ctor_max,"max_64_action_resize_step_us":slice_max,"max_control_prepare_step_us":prepare_max}));
            }
        }
    }
    println!(
        "{}",
        json!({"features":build_features::enabled(),"rounds":rounds,"reprepare":reprepare,"no_intern":no_intern,"geometry_bytes":size_of::<layout::Geometry>(),"box_node_bytes":size_of::<layout::BoxNode>(),"build_job_bytes":size_of::<layout::BuildJob<'static>>(),"prepare_job_bytes":size_of::<layout::PrepareJob<'static>>(),"flow_job_bytes":size_of::<layout::Job<'static>>(),"scope":"Prebuilt DOM/styles; original box grouping, text cache and normal-flow geometry. Retained resize vs same-tree text re-preparation. No rasterizer/network/JS/UI/browser evidence. Payload excludes shared DOM/styles/fonts/source maps and headers/capacity/hash/allocator overhead.","cases":cases})
    );
}
