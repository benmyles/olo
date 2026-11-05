//! Byte decoding and static CSS syntax only; no networking or browser.
use olo_web::{
    css::{ByteOptions, Limits, Syntax},
    text::{DecodeLimits, Decoder, UTF8},
};
use serde_json::json;
use std::{hint::black_box, time::Instant};
fn main() {
    let rounds = std::env::args().nth(1).map(|s| s.parse::<usize>().unwrap()).unwrap_or(10);
    assert!(rounds > 0);
    let mut results = Vec::new();
    for (case, bytes, enc) in [
        ("ascii", vec![b'a'; 2 * 1024 * 1024], UTF8),
        ("unicode", "a🙂é".repeat(300000).into_bytes(), UTF8),
        ("malformed", b"a\xf0\x9f\x99\"\xff\xe2\x82\"".repeat(100000), UTF8),
        ("latin", vec![0xe9; 1024 * 1024], encoding_rs::WINDOWS_1252),
        ("utf16", b"a\0=\xd8B\xde".repeat(300000), encoding_rs::UTF_16LE),
    ] {
        for chunk in [16384, bytes.len()] {
            let mut output_bytes = 0;
            let start = Instant::now();
            for _ in 0..rounds {
                let mut decoder = Decoder::new(enc, DecodeLimits::default(), None);
                let mut output = 0;
                for part in bytes.chunks(chunk) {
                    decoder
                        .push(black_box(part), false, |s| {
                            output += black_box(s).len();
                            Ok(())
                        })
                        .unwrap();
                }
                decoder
                    .push(&[], true, |s| {
                        output += black_box(s).len();
                        Ok(())
                    })
                    .unwrap();
                output_bytes = output;
                black_box(decoder.progress());
            }
            results.push(json!({"operation":"decode","case":case,"chunk":chunk,"source_bytes":bytes.len(),"output_bytes":output_bytes,"ms":start.elapsed().as_secs_f64()*1000./rounds as f64}));
        }
    }
    for (case, bytes, protocol) in [
        ("stylesheet", ".card > a:hover { color: #abc; margin: 12.5px 0; background: url(\"image.png\"); }".repeat(8192).into_bytes(), None),
        ("literals", format!("/*{}*/a {{ content: \"{}\"; }}", "x".repeat(1024 * 1024), "ab🙂".repeat(180000)).into_bytes(), None),
        ("latin_literals", [b"a{content:\"".as_slice(), &vec![0xe9; 1024 * 1024], b"\"}"].concat(), Some("windows-1252")),
    ] {
        let first = Instant::now();
        let syntax = Syntax::parse_bytes(&bytes, ByteOptions { protocol_encoding: protocol, ..Default::default() }, Limits::default(), None).unwrap().syntax;
        let rules = syntax.stylesheet(None).unwrap();
        let first_ms = first.elapsed().as_secs_f64() * 1000.;
        let memory = syntax.memory();
        let count = rules.len();
        drop(rules);
        drop(syntax);
        let start = Instant::now();
        for _ in 0..rounds {
            let syntax = Syntax::parse_bytes(black_box(&bytes), ByteOptions { protocol_encoding: protocol, ..Default::default() }, Limits::default(), None).unwrap().syntax;
            black_box(syntax.stylesheet(None).unwrap().len());
        }
        results.push(json!({"operation":"parse_bytes","case":case,"source_bytes":bytes.len(),"decoded_bytes":memory.source_bytes,"source_reserved_bytes":memory.source_reserved_bytes,"nodes":memory.nodes,"node_reserved_bytes":memory.node_reserved_bytes,"owned_text_bytes":memory.decoded_bytes,"items":count,"first_ms":first_ms,"ms":start.elapsed().as_secs_f64()*1000./rounds as f64}));
    }
    println!("{}", serde_json::to_string_pretty(&json!({"scope":"Streaming text decoding and static CSS syntax; no style, layout, UI, network or Chrome comparison", "rounds":rounds,"scalar_scans":cfg!(feature="scalar-scans"),"baseline_css_byte_copy":cfg!(feature="baseline-css-byte-copy"),"baseline_utf8_errors":cfg!(feature="baseline-utf8-errors"),"decoder_size":std::mem::size_of::<Decoder<'_>>(),"css_decoder_size":std::mem::size_of::<olo_web::css::ByteDecoder<'_>>(),"results":results})).unwrap());
}
