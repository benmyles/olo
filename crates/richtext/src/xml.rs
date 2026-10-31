//! XML read as a flat list of starts, ends and text (local names, no
//! namespaces), and text made safe to write.

pub enum X {
    Start(String, Vec<(String, String)>),
    End(String),
    Text(String),
}

pub fn events(xml: &str) -> Vec<X> {
    use quick_xml::events::Event;
    let mut r = quick_xml::Reader::from_str(xml);
    let mut out = Vec::new();
    let attrs = |e: &quick_xml::events::BytesStart| {
        e.attributes()
            .flatten()
            .map(|a| {
                let k = a.key.local_name().as_ref().to_owned();
                let v = a.normalized_value(quick_xml::XmlVersion::Implicit1_0).map(|v| v.into_owned()).unwrap_or_default();
                (k, v)
            })
            .collect::<Vec<_>>()
    };
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => out.push(X::Start(e.local_name().as_ref().to_owned(), attrs(&e))),
            Ok(Event::Empty(e)) => {
                let n = e.local_name().as_ref().to_owned();
                out.push(X::Start(n.clone(), attrs(&e)));
                out.push(X::End(n));
            }
            Ok(Event::End(e)) => out.push(X::End(e.local_name().as_ref().to_owned())),
            Ok(Event::Text(t)) => out.push(X::Text(t.xml10_content().into_owned())),
            Ok(Event::CData(t)) => out.push(X::Text(AsRef::<str>::as_ref(&t).to_owned())),
            Ok(Event::GeneralRef(g)) => {
                let s = match g.resolve_char_ref() {
                    Ok(Some(c)) => c.to_string(),
                    _ => match g.xml10_content().as_ref() {
                        "amp" => "&".into(),
                        "lt" => "<".into(),
                        "gt" => ">".into(),
                        "quot" => "\"".into(),
                        "apos" => "'".into(),
                        _ => String::new(),
                    },
                };
                out.push(X::Text(s));
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

pub fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// Text (or an attribute's value) escaped for XML; characters XML 1.0
/// can't hold are dropped.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

/// "RRGGBB" (or "#RRGGBB") as a colour.
pub fn hex_color(s: &str) -> Option<[u8; 3]> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// A zip archive's file as text.
pub fn zip_text(z: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>, name: &str) -> Option<String> {
    use std::io::Read;
    let mut f = z.by_name(name).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    Some(s)
}

/// A file in the zip as it is (a picture), if it's there.
pub fn zip_bytes(z: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>, name: &str) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut f = z.by_name(name).ok()?;
    let mut v = Vec::new();
    f.read_to_end(&mut v).ok()?;
    Some(v)
}

/// Monospaced, by a font's name.
pub fn is_mono(font: &str) -> bool {
    let f = font.to_ascii_lowercase();
    ["mono", "courier", "consolas", "menlo", "code", "fixed"].iter().any(|k| f.contains(k))
}
