//! Release-only lexical baseline. No network, UI, DOM or rendering claim.
//! cargo run --release -p olo-web --example tokenize_bench -- 100
use olo_web::html::{Limits, Token, Tokenizer};
use serde_json::json;
use std::{hint::black_box, time::Instant};

fn main() {
    let rounds: usize = std::env::args().nth(1).map(|s| s.parse().expect("rounds")).unwrap_or(100);
    assert!(rounds > 0);
    let cases = [
        ("text", "A plain Unicode paragraph: Καλημέρα, 日本語, hello. ".repeat(16_384)),
        ("article", "<section class=\"article\" data-id=\"123\"><h2>Headline</h2><p>A paragraph with <a href=\"/story\">a link</a> and more words.</p></section>\n".repeat(8192)),
        ("references", "<p title='&notit; &amp; &#x1f642;'>Text &notit; &amp; &CounterClockwiseContourIntegral; &#128; &#0;</p>".repeat(4096)),
        ("attributes", format!("<div {}>", (0..4096).map(|i| format!("data-{i}='value' ")).collect::<String>())),
    ];
    let mut results = Vec::new();
    for (case, input) in cases {
        let run = || {
            let mut parser = Tokenizer::new(black_box(input.as_str()), Limits::default()).unwrap();
            let mut tokens = 0;
            let mut borrowed = 0;
            while let Some(token) = parser.next_token().unwrap() {
                if matches!(&token, Token::Text(std::borrow::Cow::Borrowed(_))) {
                    borrowed += 1;
                }
                black_box(token);
                tokens += 1;
            }
            (tokens, borrowed)
        };
        let counts = run(); // warm code and allocator; measure input-to-token latency separately
        let first_start = Instant::now();
        let first = Tokenizer::new(&input, Limits::default()).unwrap().next_token().unwrap();
        black_box(first);
        let first_us = first_start.elapsed().as_secs_f64() * 1e6;
        let start = Instant::now();
        for _ in 0..rounds {
            black_box(run());
        }
        let secs = start.elapsed().as_secs_f64();
        results.push(json!({"case":case,"input_bytes":input.len(),"rounds":rounds,"tokens_per_round":counts.0,"borrowed_text_tokens_per_round":counts.1,"total_seconds":secs,"mib_per_second":(input.len()*rounds) as f64 / secs / 1048576.0,"parse_us":secs*1e6/rounds as f64,"first_token_us":first_us}));
    }
    println!("{}", serde_json::to_string_pretty(&json!({"scalar_scans":cfg!(feature="scalar-scans"),"results":results})).unwrap());
}
