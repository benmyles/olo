//! Compare a prepared-text projection with independently captured Chrome
//! innerText. These are different pipeline stages; differences stay visible.
use olo_web::text::prepare::{Collapse, Job, Span, WhiteSpace};
use serde_json::json;
use std::{path::Path, sync::Arc};
fn main() {
    let input = std::env::args().nth(1).expect("Chrome observation JSON path");
    let output = std::env::args().nth(2).expect("output JSON path");
    let data: serde_json::Value = serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
    let (mut identical, mut different, mut unsupported) = (0, 0, 0);
    let mut results = Vec::new();
    for row in data["rows"].as_array().unwrap() {
        if row["supported"] == false {
            unsupported += 1;
            results.push(json!({"name":row["name"],"outcome":"unsupported_by_reference"}));
            continue;
        }
        let mut source = String::new();
        let mut spans = Vec::new();
        for part in row["spans"].as_array().unwrap() {
            let start = source.len() as u32;
            source.push_str(part["text"].as_str().unwrap());
            let whitespace = match part["mode"].as_str().unwrap() {
                "normal" => WhiteSpace::default(),
                "nowrap" => WhiteSpace { wrap: false, ..Default::default() },
                "pre" => WhiteSpace { collapse: Collapse::Preserve, wrap: false },
                "pre-wrap" => WhiteSpace { collapse: Collapse::Preserve, wrap: true },
                "pre-line" => WhiteSpace { collapse: Collapse::PreserveBreaks, wrap: true },
                "break-spaces" => WhiteSpace { collapse: Collapse::BreakSpaces, wrap: true },
                _ => panic!(),
            };
            if start < source.len() as u32 {
                spans.push(Span { range: start..source.len() as u32, whitespace });
            }
        }
        let mut job = Job::new(Arc::from(source), Arc::from(spans), Default::default(), None).unwrap();
        while !job.step(4096).unwrap().finished {}
        let p = job.finish().unwrap();
        let same = p.text().as_ref() == row["innerText"].as_str().unwrap();
        if same {
            identical += 1;
        } else {
            different += 1;
        }
        results.push(json!({"name":row["name"],"outcome":if same{"identical_projection"}else{"different_projection"},"native_phase_one":p.text().as_ref(),"chrome_inner_text":row["innerText"],"chrome_width":row["width"]}));
    }
    let report = json!({"scope":"Diagnostic projections, not equivalent pipeline stages or website/WPT/layout acceptance. CR plaintext preservation, bidi-control handling, phase II and UA segment-break policy need separate layout evidence.","reference_user_agent":data["userAgent"],"counts":{"identical_projection":identical,"different_projection":different,"unsupported_by_reference":unsupported},"results":results});
    std::fs::write(Path::new(&output), serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    if different != 0 || unsupported != 0 {
        std::process::exit(1);
    }
}
