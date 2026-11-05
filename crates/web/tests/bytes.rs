use olo_web::{
    css::{self, ByteDecoder, ByteOptions, EncodingSource, Syntax},
    text::{self, CharacterEncoding as Encoding, DecodeError, DecodeLimits, Decoder, UTF8},
};
use std::sync::atomic::{AtomicBool, Ordering};
fn decoded(bytes: &[u8], encoding: &'static Encoding, chunk: usize) -> (String, bool) {
    let mut decoder = Decoder::new(encoding, DecodeLimits::default(), None);
    let mut out = String::new();
    for part in bytes.chunks(chunk) {
        decoder
            .push(part, false, |s| {
                out.push_str(s);
                Ok(())
            })
            .unwrap();
    }
    decoder
        .push(&[], true, |s| {
            out.push_str(s);
            Ok(())
        })
        .unwrap();
    assert_eq!(decoder.progress().input_bytes, bytes.len());
    assert_eq!(decoder.progress().output_bytes, out.len());
    assert_eq!(decoder.push(&[], true, |_| panic!("finished decoder called sink")), Err(DecodeError::Finished));
    (out, decoder.progress().had_errors)
}
fn check(bytes: &[u8], encoding: &'static Encoding, sizes: &[usize]) {
    let (expected, errors) = encoding.decode_without_bom_handling(bytes);
    for &size in sizes {
        let actual = decoded(bytes, encoding, size);
        assert_eq!(actual.0, expected, "{} chunk={size}, bytes={bytes:?}", encoding.name());
        assert_eq!(actual.1, errors, "{} chunk={size}, bytes={bytes:?}", encoding.name());
    }
}
#[test]
fn utf8_exhaustive_short_sequences_and_malformed_boundaries_match_reference() {
    for n in 0..=u16::MAX {
        check(&n.to_le_bytes(), UTF8, &[1, 2]);
    }
    for bytes in [
        &b"\xef\xbb\xbf\0\r\n"[..],
        &b"\xed\xa0\x80\xe0\x80\x80\xf4\x90\x80\x80"[..],
        &b"\xc2\"\xe2\x82\"\xf0\x9f\x99\"\xf0\x9f\x99\x82"[..],
        &b"\xf0\x9f\x99\x82\xe2\x82\xac\xc3\xa9"[..],
        &b"\xf0\x9f\x99"[..],
    ] {
        check(bytes, UTF8, &[1, 2, 3, 4, 7, 65536]);
    }
    let mut bytes = vec![b'a'; 65535];
    bytes.extend_from_slice("🙂é𐀀".as_bytes());
    check(&bytes, UTF8, &[65534, 65535, 65536, 65537, 99999]);
}
#[test]
fn all_standard_codecs_preserve_state_and_replacement_across_chunks() {
    let labels = [
        "utf-8",
        "ibm866",
        "iso-8859-2",
        "iso-8859-3",
        "iso-8859-4",
        "iso-8859-5",
        "iso-8859-6",
        "iso-8859-7",
        "iso-8859-8",
        "iso-8859-8-i",
        "iso-8859-10",
        "iso-8859-13",
        "iso-8859-14",
        "iso-8859-15",
        "iso-8859-16",
        "koi8-r",
        "koi8-u",
        "macintosh",
        "windows-874",
        "windows-1250",
        "windows-1251",
        "windows-1252",
        "windows-1253",
        "windows-1254",
        "windows-1255",
        "windows-1256",
        "windows-1257",
        "windows-1258",
        "x-mac-cyrillic",
        "gbk",
        "gb18030",
        "big5",
        "euc-jp",
        "iso-2022-jp",
        "shift_jis",
        "euc-kr",
        "replacement",
        "utf-16be",
        "utf-16le",
        "x-user-defined",
    ];
    let mut seed = 0x8e09_4fab_dc16_3527u64;
    for label in labels {
        let encoding = Encoding::for_label(label.as_bytes()).unwrap();
        for _ in 0..128 {
            let mut bytes = [0; 128];
            for b in &mut bytes {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                *b = seed as u8;
            }
            check(&bytes, encoding, &[1, 2, 3, 11, 65536]);
        }
        check(b"\x1b$B$3$s$K$A$O\x1b(B\x82\x22\xff\x00\xd8", encoding, &[1, 2, 3, 7]);
    }
    // More than the staging buffer, and a split surrogate/state sequence.
    let bytes = b"a\0=\xd8B\xde".repeat(50000);
    check(&bytes, encoding_rs::UTF_16LE, &[65535, 65536, 1000000]);
    let bytes = b"\x1b$B$3$s$K$A$O\x1b(Bhello".repeat(10000);
    check(&bytes, encoding_rs::ISO_2022_JP, &[65535, 65536, 1000000]);
}
#[test]
fn css_fixture_bytes_at_every_split_use_independent_expected_encoding() {
    let cases: serde_json::Value = serde_json::from_str(include_str!("css-parsing-tests/stylesheet_bytes.json")).unwrap();
    let (pairs, remainder) = cases.as_array().unwrap().as_chunks::<2>();
    assert!(remainder.is_empty());
    for pair in pairs {
        let input = &pair[0];
        let bytes: Vec<u8> = input["css_bytes"].as_str().unwrap().chars().map(|c| c as u8).collect();
        let options =
            ByteOptions { protocol_encoding: input["protocol_encoding"].as_str(), environment_encoding: input["environment_encoding"].as_str().and_then(|s| Encoding::for_label(s.as_bytes())) };
        let encoding = Encoding::for_label(pair[1][1].as_str().unwrap().as_bytes()).unwrap();
        let bom = Encoding::for_bom(&bytes).map_or(0, |(_, n)| n);
        let (expected, errors) = encoding.decode_without_bom_handling(&bytes[bom..]);
        for split in 0..=bytes.len() {
            let mut decoder = ByteDecoder::new(options, DecodeLimits::default(), None);
            let mut out = String::new();
            decoder
                .push(&bytes[..split], false, |s| {
                    out.push_str(s);
                    Ok(())
                })
                .unwrap();
            decoder
                .push(&bytes[split..], true, |s| {
                    out.push_str(s);
                    Ok(())
                })
                .unwrap();
            assert_eq!(decoder.choice().unwrap().encoding, encoding);
            assert_eq!(decoder.choice().unwrap().bom_bytes, bom);
            assert_eq!(out, expected);
            assert_eq!(decoder.progress().had_errors, errors);
            assert_eq!(decoder.progress().input_bytes, bytes.len());
            assert_eq!(decoder.progress().output_bytes, out.len());
            assert_eq!(decoder.push(&[], true, |_| panic!()), Err(DecodeError::Finished));
        }
    }
}
#[test]
fn css_encoding_precedence_prefix_bound_and_exact_byte_pattern() {
    let env = encoding_rs::WINDOWS_1252;
    let opts = ByteOptions { protocol_encoding: Some(" \tiso-8859-5\r"), environment_encoding: Some(env) };
    for (bytes, source, enc) in [
        (&b"\xff\xfe@\0"[..], EncodingSource::Bom, encoding_rs::UTF_16LE),
        (&b"\xef\xbb\xbf@charset \"windows-1252\";"[..], EncodingSource::Bom, UTF8),
        (&b"@charset \"utf-8\";"[..], EncodingSource::Protocol, encoding_rs::ISO_8859_5),
    ] {
        let choice = css::encoding(bytes, opts);
        assert_eq!(choice.source, source);
        assert_eq!(choice.encoding, enc);
    }
    let opts = ByteOptions { protocol_encoding: Some("unknown"), environment_encoding: Some(env) };
    for prefix in
        ["@Charset \"utf-8\";", "@charset  \"utf-8\";", "@charset 'utf-8';", " @charset \"utf-8\";", "@charset \"utf-8\" ;", "@charset \"unknown\";", "@charset \"utf-8\"", "@charset \"utƒ-8\";"]
    {
        let choice = css::encoding(prefix.as_bytes(), opts);
        assert_eq!(choice.source, EncodingSource::Environment, "{prefix}");
        assert_eq!(choice.encoding, env);
    }
    for label in ["UTF-16", "utf-16be", "utf-16le"] {
        let choice = css::encoding(format!("@charset \"{label}\";").as_bytes(), opts);
        assert_eq!(choice.source, EncodingSource::Charset);
        assert_eq!(choice.encoding, UTF8);
    }
    for len in [1023, 1024, 1025, 2048] {
        let mut bytes = b"@charset \"".to_vec();
        bytes.extend(vec![b' '; len - bytes.len() - b"utf-8\";".len()]);
        bytes.extend_from_slice(b"utf-8\";");
        let expected = if len <= 1024 { UTF8 } else { env };
        assert_eq!(css::encoding(&bytes, opts).encoding, expected);
        for size in [1, 2, 3, 17, 1023, 1024, 10000] {
            let mut decoder = ByteDecoder::new(opts, DecodeLimits::default(), None);
            for part in bytes.chunks(size) {
                decoder.push(part, false, |_| Ok(())).unwrap();
            }
            decoder.push(&[], true, |_| Ok(())).unwrap();
            assert_eq!(decoder.choice().unwrap().encoding, expected, "len={len}, chunk={size}");
        }
    }
    assert_eq!(css::encoding(&[], ByteOptions::default()).source, EncodingSource::Default);
    let mut decoder = ByteDecoder::new(ByteOptions::default(), DecodeLimits::default(), None);
    decoder.push(b"@charset \"", false, |_| panic!()).unwrap();
    assert!(decoder.choice().is_none());
    assert_eq!(decoder.progress().input_bytes, 10);
}
#[test]
fn resource_limits_cancellation_and_failure_are_terminal_and_do_not_leak_output() {
    for enc in [UTF8, encoding_rs::WINDOWS_1252, encoding_rs::UTF_16LE] {
        let mut decoder = Decoder::new(enc, DecodeLimits { input_bytes: 1, output_bytes: 10 }, None);
        assert_eq!(decoder.push(b"ab", true, |_| panic!()), Err(DecodeError::InputLimit));
        assert_eq!(decoder.push(&[], true, |_| panic!()), Err(DecodeError::InputLimit));
        let mut decoder = Decoder::new(enc, DecodeLimits { input_bytes: 100, output_bytes: 1 }, None);
        let mut out = String::new();
        assert_eq!(
            decoder.push(b"\xff\xff", true, |s| {
                out.push_str(s);
                Ok(())
            }),
            Err(DecodeError::OutputLimit)
        );
        assert!(out.len() <= 1);
        assert_eq!(decoder.push(b"a", true, |_| panic!()), Err(DecodeError::OutputLimit));
    }
    let cancelled = AtomicBool::new(false);
    let mut decoder = Decoder::new(UTF8, DecodeLimits::default(), Some(&cancelled));
    let bytes = vec![b'a'; 200000];
    let mut delivered = 0;
    assert_eq!(
        decoder.push(&bytes, true, |s| {
            delivered += s.len();
            cancelled.store(true, Ordering::Relaxed);
            Ok(())
        }),
        Err(DecodeError::Cancelled)
    );
    assert!(delivered <= 65536);
    assert_eq!(decoder.push(&[], true, |_| panic!()), Err(DecodeError::Cancelled));
    let mut decoder = ByteDecoder::new(ByteOptions::default(), DecodeLimits::default(), Some(&cancelled));
    assert_eq!(decoder.push(b"abc", true, |_| panic!()), Err(DecodeError::Cancelled));
    let mut decoder = ByteDecoder::new(ByteOptions::default(), DecodeLimits { input_bytes: 3, output_bytes: 3 }, None);
    decoder.push(b"ab", false, |_| panic!()).unwrap();
    assert_eq!(decoder.push(b"cd", true, |_| panic!()), Err(DecodeError::InputLimit));
    assert_eq!(decoder.push(&[], true, |_| panic!()), Err(DecodeError::InputLimit));
    let mut decoder = ByteDecoder::new(ByteOptions::default(), DecodeLimits::default(), None);
    assert_eq!(decoder.push(b"abc", true, |_| Err(DecodeError::Cancelled)), Err(DecodeError::Cancelled));
    assert_eq!(decoder.progress().output_bytes, 3);
    assert_eq!(decoder.push(&[], true, |_| panic!()), Err(DecodeError::Cancelled));
    let mut output = String::new();
    for _ in 0..17 {
        text::collect(&mut output, "a", 17).unwrap();
        assert!(output.capacity() <= 17);
    }
    assert_eq!(text::collect(&mut output, "a", 17), Err(DecodeError::OutputLimit));
    assert!(matches!(Syntax::parse_bytes(b"\xff", ByteOptions::default(), css::Limits { input_bytes: 1, ..Default::default() }, None), Err(css::ByteParseError::Decode(DecodeError::OutputLimit))));
    assert!(matches!(Syntax::parse_bytes(b"a{}", ByteOptions::default(), css::Limits { tokens: 1, ..Default::default() }, None), Err(css::ByteParseError::Syntax(css::Error::TokenLimit))));
}

#[test]
fn decoded_source_keeps_its_buffer_and_reports_retained_capacity() {
    let source = String::from(r#"a{content:"hello"}"#);
    let ptr = source.as_ptr();
    let capacity = source.capacity();
    let syntax = Syntax::parse_owned(source, css::Limits::default(), None).unwrap();
    assert_eq!(syntax.source().as_ptr(), ptr);
    assert_eq!(syntax.memory().source_reserved_bytes, capacity);
    let mut source = String::with_capacity(1000);
    source.push('a');
    assert!(matches!(Syntax::parse_owned(source, css::Limits { input_bytes: 100, ..Default::default() }, None), Err(css::Error::InputLimit)));
    for (bytes, protocol) in [(r#"a{content:"🙂"}"#.as_bytes().to_vec(), None), (b"a{content:\"\xe9\xe9\"}".to_vec(), Some("windows-1252")), (b"\xff\xfea\0{\0}\0".to_vec(), None)] {
        let parsed = Syntax::parse_bytes(&bytes, ByteOptions { protocol_encoding: protocol, ..Default::default() }, css::Limits::default(), None).unwrap();
        assert_eq!(parsed.syntax.memory().source_reserved_bytes, parsed.syntax.source().len());
        assert_eq!(parsed.syntax.memory().decoded_bytes, 0);
    }
}
