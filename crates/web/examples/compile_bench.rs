//! Compile the style sheets given (files) N times, as a page compiles its
//! rules: `compile_bench N A.css B.css …` prints the time per round.
use olo_web::css::{
    self,
    cascade::{Origin, SheetBuilder},
};
use std::sync::Arc;
fn main() {
    let mut args = std::env::args().skip(1);
    let rounds: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(10);
    let sheets: Vec<css::Syntax> = args.map(|p| css::Syntax::parse(Arc::from(std::fs::read_to_string(p).unwrap()), Default::default(), None).unwrap()).collect();
    let media = css::media::Media::screen(1440., 900., 1., false);
    for _ in 0..rounds {
        let started = std::time::Instant::now();
        let mut builder = SheetBuilder::new(Default::default()).with_media(media);
        for s in &sheets {
            builder.add_scoped_sheet_from(s, Origin::Author, None, None, None).unwrap();
        }
        let added = started.elapsed();
        let rules = builder.finish(None).unwrap();
        std::hint::black_box(&rules);
        println!("add {:.1} ms, finish {:.1} ms", added.as_secs_f64() * 1e3, (started.elapsed() - added).as_secs_f64() * 1e3);
    }
}
