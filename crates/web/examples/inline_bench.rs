use olo_web::{
    css::{
        Syntax,
        properties::{Declaration, Value},
        style::{Axes, FontRequest, FontSlant},
    },
    font::{Builder, FontSet, GenericPolicy, Scope},
    text::{
        layout::{self, Options, Text},
        linebreak,
        prepare::{self, Span, WhiteSpace},
        shaping::{self, BaseDirection, Engine},
    },
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{hint::black_box, path::Path, sync::Arc, time::Instant};
fn request(name: &str) -> FontRequest {
    let s = Syntax::parse(Arc::from(format!("'{name}'")), Default::default(), None).unwrap();
    let Declaration::Known { values, .. } = Declaration::parse("font-family", s.components(), false, Default::default(), None).unwrap() else { panic!() };
    let Value::Families(f) = &values[0].value else { panic!() };
    FontRequest { families: f.clone(), size: 20., weight: 400., width: 100., slant: FontSlant::Normal, axes: Axes::default() }
}
fn build(e: &mut Engine, s: &Scope, source: Arc<str>, name: &str) -> (Arc<Text>, f64, f64) {
    let mut p = prepare::Job::new(source.clone(), Arc::from([Span { range: 0..source.len() as u32, whitespace: WhiteSpace::default() }]), Default::default(), None).unwrap();
    while !p.step(4096).unwrap().finished {}
    let p = Arc::new(p.finish().unwrap());
    let mut s = shaping::ParagraphJob::for_layout(e, s, request(name), p.text().clone(), BaseDirection::Auto, Default::default(), None).unwrap();
    while !s.step(128).unwrap().finished {}
    let s = Arc::new(s.finish().unwrap());
    let mut b = linebreak::Job::new(p.text().clone(), Default::default(), None).unwrap();
    while !b.step(4096).unwrap().finished {}
    let b = Arc::new(b.finish().unwrap());
    let at = Instant::now();
    let mut t = layout::TextJob::new(e, p, s, b, Default::default(), None).unwrap();
    let ctor = at.elapsed().as_secs_f64() * 1e6;
    let mut max = 0f64;
    loop {
        let at = Instant::now();
        let progress = t.step(128).unwrap();
        max = max.max(at.elapsed().as_secs_f64() * 1e6);
        if progress.finished {
            break;
        }
    }
    (t.finish().unwrap(), ctor, max)
}
fn fit(e: &mut Engine, t: Arc<Text>, width: f64) -> (layout::Layout, f64, f64, usize) {
    let at = Instant::now();
    let mut j = layout::Job::new(e, t, Options { width, ..Default::default() }, Default::default(), None).unwrap();
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
    let reshaped = j.progress().reshaped_glyphs;
    (j.finish().unwrap(), ctor, max, reshaped)
}
fn digest(l: &layout::Layout) -> String {
    let mut h = Sha256::new();
    for line in l.lines() {
        h.update(line.source.start.to_le_bytes());
        h.update(line.source.end.to_le_bytes());
        h.update(line.advance.to_le_bytes());
        h.update(line.offset.to_le_bytes());
        for f in &l.fragments()[line.fragments.start as usize..line.fragments.end as usize] {
            h.update(f.x.to_le_bytes());
            h.update(f.advance.to_le_bytes());
            for g in &l.run(f).unwrap().glyphs[f.glyphs.start as usize..f.glyphs.end as usize] {
                for v in [g.id, g.cluster, g.x_advance as u32, g.y_advance as u32, g.x_offset as u32, g.y_offset as u32] {
                    h.update(v.to_le_bytes())
                }
            }
        }
    }
    format!("{:x}", h.finalize())
}
fn main() {
    let rounds: usize = std::env::args().nth(1).unwrap_or_else(|| "50".into()).parse().unwrap();
    assert!(rounds > 0);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut b = Builder::new(Default::default());
    for path in [
        "data/fonts/NotoSans[wght].ttf",
        "crates/web/tests/shaping-fixtures/fonts/notosansarabic/NotoSansArabic[wdth,wght].ttf",
        "crates/web/tests/shaping-fixtures/fonts/notosanshebrew/NotoSansHebrew[wdth,wght].ttf",
    ] {
        b.load_file(&root.join(path), |_| {}, None).unwrap();
    }
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Noto Sans", "Noto Sans Arabic", "Noto Sans Hebrew"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let mut e = Engine::default();
    let mut cases = Vec::new();
    for (scene, unit, name) in
        [("latin", "office affine alpha beta gamma. ", "Noto Sans"), ("arabic", "مرحبا بالعالم العربية سلام ", "Noto Sans Arabic"), ("mixed", "Hello שלום world مرحبا text. ", "Noto Sans")]
    {
        let source: Arc<str> = Arc::from(unit.repeat(128));
        let (t, ctor, step) = build(&mut e, &scope, source.clone(), name);
        let mut build_max = step;
        let mut build_ctor_max = ctor;
        let at = Instant::now();
        for _ in 0..rounds {
            let (t, c, s) = build(&mut e, &scope, black_box(source.clone()), name);
            build_ctor_max = build_ctor_max.max(c);
            build_max = build_max.max(s);
            black_box(t);
        }
        let build_us = at.elapsed().as_secs_f64() * 1e6 / rounds as f64;
        for width in [120., 400., 1000.] {
            let (l, ctor, step, reshaped) = fit(&mut e, t.clone(), width);
            let signature = digest(&l);
            let lines = l.lines().len();
            let fragments = l.fragments().len();
            let payload = l.geometry_payload_bytes();
            assert!(l.lines().iter().all(|l| l.advance.is_finite()));
            drop(l);
            let mut max = step;
            let mut ctor_max = ctor;
            let at = Instant::now();
            for _ in 0..rounds {
                let (l, c, s, _) = fit(&mut e, black_box(t.clone()), width);
                max = max.max(s);
                ctor_max = ctor_max.max(c);
                black_box(l);
            }
            cases.push(json!({"scene":scene,"width":width,"source_bytes":source.len(),"text_geometry_payload_bytes":t.geometry_payload_bytes(),"layout_geometry_payload_bytes":payload,"lines":lines,"fragments":fragments,"reshaped_glyphs":reshaped,"digest":signature,"build_drop_us":build_us,"max_build_ctor_us":build_ctor_max,"max_128_point_build_step_us":build_max,"resize_drop_us":at.elapsed().as_secs_f64()*1e6/rounds as f64,"max_resize_ctor_us":ctor_max,"max_64_candidate_resize_step_us":max}));
        }
    }
    println!(
        "{}",
        json!({"rounds":rounds,"reshape_control":cfg!(feature="baseline-inline-reshape"),"text_job_bytes":size_of::<layout::TextJob<'static>>(),"layout_job_bytes":size_of::<layout::Job<'static>>(),"cases":cases})
    );
}
