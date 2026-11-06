use olo_web::html::bytes::encoding;
#[test]
fn byte_encoding_priority_and_meta_prescan() {
    let cases: &[(&[u8], Option<&str>, &str, usize)] = &[
        (b"\xef\xbb\xbf<meta charset=windows-1252>", Some("shift_jis"), "UTF-8", 3),
        (b"<meta charset=utf-8>", Some("shift_jis"), "Shift_JIS", 0),
        (b"<!-- <meta charset=shift_jis> --><META CHARSET='utf-8'>", None, "UTF-8", 0),
        (b"<div title='<meta charset=shift_jis>'><meta charset=utf-8>", None, "UTF-8", 0),
        (b"<meta content='text/html; charset=shift_jis' http-equiv=content-type>", None, "Shift_JIS", 0),
        (b"<meta content='text/html; charset=shift_jis'>", None, "windows-1252", 0),
        (b"<meta charset=utf-16le>", None, "UTF-8", 0),
        (b"<meta charset=x-user-defined>", None, "windows-1252", 0),
        (b"<meta charset=utf-8 charset=shift_jis>", None, "UTF-8", 0),
        (b"<meta charset=invalid><meta charset=utf-8>", None, "UTF-8", 0),
        (b"<meta charset=invalid content='charset=utf-8' http-equiv=content-type>", None, "windows-1252", 0),
        (b"<!--><meta charset=utf-8>", None, "UTF-8", 0),
        (b"<meta charset=utf-8", None, "windows-1252", 0),
        (b"<?xml version='1.0' encoding='shift_jis'?>", None, "Shift_JIS", 0),
        (b"<\0?\0x\0", None, "UTF-16LE", 0),
        (b"hello", None, "windows-1252", 0),
    ];
    for (bytes, protocol, expected, skip) in cases {
        let (actual, bom) = encoding(bytes, *protocol);
        assert_eq!((actual.name(), bom), (*expected, *skip), "{bytes:?}");
    }
    let mut late = vec![b' '; 1024];
    late.extend_from_slice(b"<meta charset=utf-8>");
    assert_eq!(encoding(&late, None).0.name(), "windows-1252");
}
