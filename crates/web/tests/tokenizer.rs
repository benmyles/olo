use olo_web::html::{Limits, Mode, Token, Tokenizer};
use serde_json::{Value, json};

// Unmodified fixtures from html5lib-tests, MIT, revision
// c5495c539ca8a79c3c5107f5f14010bcff9a35d5. See html5lib/LICENSE.
// These compare emitted tokens only. Parse-error diagnostics are NOT yet
// implemented or counted as conforming; see docs/browser/TODO.md.
fn unescape(s: &str) -> Option<String> {
    let mut result = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            assert_eq!(chars.next(), Some('u'));
            let digits: String = chars.by_ref().take(4).collect();
            let code = u32::from_str_radix(&digits, 16).unwrap();
            result.push(char::from_u32(code)?);
        } else {
            result.push(c);
        }
    }
    Some(result)
}

fn unescape_json(v: &mut Value) -> bool {
    match v {
        Value::String(s) => {
            let Some(decoded) = unescape(s) else { return false };
            *s = decoded;
        }
        Value::Array(a) => {
            for v in a {
                if !unescape_json(v) {
                    return false;
                }
            }
        }
        Value::Object(m) => {
            for v in m.values_mut() {
                if !unescape_json(v) {
                    return false;
                }
            }
        }
        _ => {}
    }
    true
}

fn output(input: &str, mode: Mode) -> Value {
    let mut tokenizer = Tokenizer::new(input, Limits::default()).unwrap();
    tokenizer.set_mode(mode);
    let mut out: Vec<Value> = Vec::new();
    while let Some(token) = tokenizer.next_token().unwrap() {
        let value = match token {
            Token::StartTag(tag) => {
                let attrs: serde_json::Map<String, Value> = tag.attributes.into_iter().map(|a| (a.name.into_owned(), Value::String(a.value.into_owned()))).collect();
                if tag.self_closing { json!(["StartTag", tag.name, attrs, true]) } else { json!(["StartTag", tag.name, attrs]) }
            }
            Token::EndTag(tag) => json!(["EndTag", tag.name]),
            Token::Doctype(d) => json!(["DOCTYPE", d.name, d.public_id, d.system_id, !d.force_quirks]),
            Token::Comment(s) => json!(["Comment", s]),
            Token::ProcessingInstruction { target, data } => json!(["ProcessingInstruction", target, data]),
            Token::Text(s) => {
                if let Some(previous) = out.last_mut().filter(|p| p[0] == "Character") {
                    let mut text = previous[1].as_str().unwrap().to_owned();
                    text.push_str(&s);
                    previous[1] = Value::String(text);
                    continue;
                }
                json!(["Character", s])
            }
        };
        out.push(value);
    }
    json!(out)
}

#[test]
fn html5lib_tokens() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/html5lib");
    let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "test")).collect();
    files.sort();
    let mut failures = Vec::new();
    let mut passed = 0;
    let mut unsupported = Vec::new();
    for file in files {
        let data: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        // XML infoset coercion is not an HTML tokenizer requirement.
        let Some(tests) = data["tests"].as_array() else { continue };
        for test in tests {
            let mut test = test.clone();
            if test["doubleEscaped"] == true {
                let decoded_input = unescape(test["input"].as_str().unwrap());
                if decoded_input.is_none() || !unescape_json(&mut test["output"]) {
                    unsupported.push(json!({"file":file.file_name().unwrap().to_str(), "test":test["description"], "reason":"non-scalar input requires a UTF-16 tokenizer interface"}));
                    continue;
                }
                test["input"] = Value::String(decoded_input.unwrap());
            }
            let states = test["initialStates"].as_array().cloned().unwrap_or_else(|| vec![json!("Data state")]);
            for state in states {
                let end = test["lastStartTag"].as_str().unwrap_or("").to_owned();
                let mode = match state.as_str().unwrap() {
                    "Data state" => Mode::Data,
                    "RCDATA state" => Mode::Rcdata(end),
                    "RAWTEXT state" => Mode::RawText(end),
                    "Script data state" => Mode::ScriptData(end),
                    "PLAINTEXT state" => Mode::Plaintext,
                    "CDATA section state" => Mode::Cdata,
                    _ => {
                        unsupported.push(json!({"file":file.file_name().unwrap().to_str(), "test":test["description"], "reason":state}));
                        continue;
                    }
                };
                let actual = output(test["input"].as_str().unwrap(), mode);
                if actual != test["output"] {
                    failures.push(json!({"file":file.file_name().unwrap().to_str(), "test":test["description"], "state":state, "input":test["input"], "actual":actual, "expected":test["output"]}));
                } else {
                    passed += 1;
                }
            }
        }
    }
    let report = json!({"passed":passed, "failed":failures.len(), "unsupported":unsupported, "failures":failures, "diagnostics_checked":false});
    if let Ok(path) = std::env::var("OLO_HTML5LIB_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    let first: Vec<_> = failures.iter().take(15).collect();
    eprintln!("tokens: {passed} passed, {} failed, {} unsupported; parse-error diagnostics not checked", failures.len(), report["unsupported"].as_array().unwrap().len());
    assert!(failures.is_empty(), "{}", serde_json::to_string_pretty(&first).unwrap());
}

#[test]
fn limits_and_cancellation() {
    use olo_web::html::Error;
    use std::sync::atomic::AtomicBool;
    assert!(matches!(Tokenizer::new("1234", Limits { input_bytes: 3, ..Limits::default() }), Err(Error::InputLimit)));
    let mut t = Tokenizer::new("<a x y>", Limits { attributes_per_tag: 1, ..Limits::default() }).unwrap();
    assert_eq!(t.next_token(), Err(Error::AttributeLimit));
    assert_eq!(t.next_token(), Ok(None));
    let mut t = Tokenizer::new("<a><b>", Limits { tokens: 1, ..Limits::default() }).unwrap();
    t.next_token().unwrap();
    assert_eq!(t.next_token(), Err(Error::TokenLimit));
    let flag = AtomicBool::new(true);
    let mut t = Tokenizer::new("<a>", Limits::default()).unwrap().with_cancellation(&flag);
    assert_eq!(t.next_token(), Err(Error::Cancelled));
}

#[test]
fn unchanged_text_and_attributes_borrow_the_input() {
    use std::borrow::Cow;
    let mut t = Tokenizer::new("<div id='hello'>plain text", Limits::default()).unwrap();
    let Token::StartTag(tag) = t.next_token().unwrap().unwrap() else { panic!() };
    assert!(matches!(tag.name, Cow::Borrowed(_)));
    assert!(matches!(tag.attributes[0].name, Cow::Borrowed(_)));
    assert!(matches!(tag.attributes[0].value, Cow::Borrowed(_)));
    assert!(matches!(t.next_token().unwrap(), Some(Token::Text(Cow::Borrowed("plain text")))));
}

#[test]
fn malformed_unicode_input_always_makes_progress() {
    // Deterministic adversarial smoke test; not a replacement for fuzzing.
    let alphabet = ['<', '>', '/', '!', '?', '-', '=', '\'', '"', '&', ';', '\r', '\n', '\0', 'é', '🙂', 'a', 'Z', '0', ' '];
    let mut seed = 0x893a_4092_d827_3201u64;
    for _ in 0..20_000 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let len = (seed % 128) as usize;
        let mut input = String::new();
        for _ in 0..len {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            input.push(alphabet[seed as usize % alphabet.len()]);
        }
        for mode in [Mode::Data, Mode::Rcdata("a".into()), Mode::RawText("a".into()), Mode::ScriptData("script".into()), Mode::Cdata, Mode::Plaintext] {
            let mut parser = Tokenizer::new(&input, Limits::default()).unwrap();
            parser.set_mode(mode);
            while parser.next_token().unwrap().is_some() {
                assert!(parser.position() > 0 && parser.position() <= input.len());
            }
        }
    }
}
