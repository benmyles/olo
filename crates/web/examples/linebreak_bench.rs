use olo_web::text::linebreak::{self as lb, Kind, Opportunity};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};
fn build(source: Arc<str>) -> (lb::Breaks, f64, f64) {
    let at = Instant::now();
    let mut job = lb::Job::new(source, Default::default(), None).unwrap();
    let ctor = at.elapsed().as_secs_f64() * 1e6;
    let mut max = 0f64;
    loop {
        let at = Instant::now();
        let p = job.step(4096).unwrap();
        max = max.max(at.elapsed().as_secs_f64() * 1e6);
        if p.finished {
            break;
        }
    }
    (job.finish().unwrap(), ctor, max)
}
fn main() {
    let rounds: usize = std::env::args().nth(1).unwrap_or_else(|| "200".into()).parse().unwrap();
    assert!(rounds > 0);
    // Independently authored positions, before timings. These are default
    // UAX14 cases, not dictionary/CSS/layout/typographic references.
    let combining = format!("a{}b ", "\u{308}".repeat(128));
    let end = combining.len();
    let units: [(&str, &str, &[usize], bool); 10] = [
        ("latin", "alpha beta gamma. ", &[6, 11, 18], false),
        ("cjk", "漢字。かなカナ ", &[3, 9, 12, 15, 18, 22], false),
        ("arabic", "مرحبا بالعالم ", &[11, 26], false),
        ("devanagari", "अक्षर परीक्षण ", &[16, 38], false),
        ("thai-default", "ภาษาไทย ทดสอบ ", &[22, 38], false),
        ("emoji", "👩\u{200d}💻 🇺🇸 👍🏽 ", &[12, 21, 30], false),
        ("joiners", "one\u{a0}two word\u{2060}join ", &[9, 21], false),
        ("hard-breaks", "a\r\nb\nc\u{2028}", &[3, 5, 9], true),
        ("long-word", "abcdefghijklmnopqrstuvwxyz0123456789", &[], false),
        ("combining", &combining, std::slice::from_ref(&end), false),
    ];
    let mut cases = Vec::new();
    for count in [128, 2048] {
        for (scene, unit, points, hard) in units {
            let source: Arc<str> = Arc::from(unit.repeat(count));
            let mut expected = Vec::new();
            for repeat in 0..count {
                for &point in points {
                    let offset = (repeat * unit.len() + point) as u32;
                    expected.push(Opportunity { offset, kind: if hard || offset as usize == source.len() { Kind::Required } else { Kind::Allowed } });
                }
            }
            if expected.last().is_none_or(|o| o.offset as usize != source.len()) {
                expected.push(Opportunity { offset: source.len() as u32, kind: Kind::Required });
            }
            let at = Instant::now();
            let (first, ctor, max) = build(source.clone());
            let cold = at.elapsed().as_secs_f64() * 1e6;
            assert_eq!(first.iter().collect::<Vec<_>>(), expected, "{scene}");
            assert!(Arc::ptr_eq(first.source(), &source));
            let payload = first.payload_bytes();
            let complex = first.complex_context_scalars();
            drop(first);
            let at = Instant::now();
            let mut maximum = 0f64;
            let mut ctor_max = 0f64;
            for _ in 0..rounds {
                let (p, c, s) = build(black_box(source.clone()));
                maximum = maximum.max(s);
                ctor_max = ctor_max.max(c);
                black_box(p);
            }
            let mean = at.elapsed().as_secs_f64() * 1e6 / rounds as f64;
            cases.push(json!({"scene":scene,"source_bytes":source.len(),"opportunities":expected.len(),"offset_payload_bytes":payload,"complex_context_scalars":complex,"build_drop_us":mean,"first_build_us":cold,"first_ctor_us":ctor,"first_max_4096_byte_step_us":max,"max_ctor_us":ctor_max,"max_4096_byte_step_us":maximum}));
        }
    }
    println!(
        "{}",
        json!({"rounds":rounds,"baseline_ascii":cfg!(feature="baseline-linebreak-ascii"),"baseline_runs":cfg!(feature="baseline-linebreak-runs"),"class_table_control":cfg!(feature="baseline-linebreak-class-table"),"job_size":size_of::<lb::Job<'static>>(),"cases":cases})
    );
}
