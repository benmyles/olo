//! A dconf change, as clients send it (`Change`'s argument): a GVariant
//! `a{smv}`, little-endian, each path with its new value or nothing (a
//! key reset; a folder, ending in "/", reset with everything in it). Read
//! here by GVariant's framing alone, so each value stays the bytes the
//! program sent (a "v": the value, a NUL, its type), stored as they are.

/// One change: the path, and the value's "v" bytes (None: reset).
pub type Change = (String, Option<Vec<u8>>);

/// How wide a container's framing offsets are, for its size.
fn offset_size(len: usize) -> usize {
    match len {
        0..=0xff => 1,
        0x100..=0xffff => 2,
        0x1_0000..=0xffff_ffff => 4,
        _ => 8,
    }
}

fn offset_at(b: &[u8], at: usize, size: usize) -> Result<usize, String> {
    let s = b.get(at..at + size).ok_or("a framing offset out of range")?;
    let mut v = 0usize;
    for (i, byte) in s.iter().enumerate() {
        v |= (*byte as usize) << (8 * i);
    }
    Ok(v)
}

fn align8(n: usize) -> usize {
    (n + 7) & !7
}

/// The elements of an array of variable-sized, 8-aligned elements.
fn elements(b: &[u8]) -> Result<Vec<&[u8]>, String> {
    if b.is_empty() {
        return Ok(Vec::new());
    }
    let size = offset_size(b.len());
    let last_end = offset_at(b, b.len() - size, size)?;
    if last_end > b.len() || (b.len() - last_end) % size != 0 {
        return Err("a bad array".into());
    }
    let n = (b.len() - last_end) / size;
    let mut out = Vec::with_capacity(n);
    let mut start = 0;
    for i in 0..n {
        let end = offset_at(b, last_end + i * size, size)?;
        if start > end || end > last_end {
            return Err("a bad array element".into());
        }
        out.push(&b[start..end]);
        start = align8(end);
    }
    Ok(out)
}

/// A "v": a value, a NUL, then its type (no NUL after it).
fn is_variant(v: &[u8]) -> bool {
    v.iter().rposition(|&c| c == 0).is_some_and(|nul| nul + 1 < v.len() && v[nul + 1..].iter().all(|c| c.is_ascii_graphic()))
}

/// Read a change.
pub fn parse(blob: &[u8]) -> Result<Vec<Change>, String> {
    let mut out = Vec::new();
    for entry in elements(blob)? {
        // {s mv}: the key, NUL-ended; the value 8-aligned after it; one
        // framing offset (the key's end) last.
        let size = offset_size(entry.len());
        if entry.len() < size {
            return Err("a bad entry".into());
        }
        let body = &entry[..entry.len() - size];
        let key_end = offset_at(entry, entry.len() - size, size)?;
        if key_end == 0 || key_end > body.len() || body[key_end - 1] != 0 {
            return Err("a bad key".into());
        }
        let key = std::str::from_utf8(&body[..key_end - 1]).map_err(|_| "a key that isn't UTF-8")?;
        if !key.starts_with('/') || key.contains("//") {
            return Err(format!("not a dconf path: {key}"));
        }
        let start = align8(key_end).min(body.len());
        let maybe = &body[start..];
        let value = if maybe.is_empty() {
            None
        } else {
            // Just: the "v", then one NUL.
            let (v, last) = maybe.split_at(maybe.len() - 1);
            if last != [0] || !is_variant(v) {
                return Err(format!("a bad value for {key}"));
            }
            if key.ends_with('/') {
                return Err(format!("a value for a folder: {key}"));
            }
            Some(v.to_vec())
        };
        out.push((key.to_owned(), value));
    }
    Ok(out)
}

/// `db` with `changes` made.
pub fn apply(db: &mut crate::gvdb::Db, changes: &[Change]) {
    for (path, value) in changes {
        match value {
            Some(v) => {
                db.insert(path.clone(), v.clone());
            }
            None if path.ends_with('/') => db.retain(|k, _| !k.starts_with(path.as_str())),
            None => {
                db.remove(path);
            }
        }
    }
}

/// What `Notify` says of `changes`: the prefix they share (up to a
/// folder's "/", or the one key), and each path after it.
pub fn notify_args(changes: &[Change]) -> (String, Vec<String>) {
    let paths: Vec<&str> = changes.iter().map(|(p, _)| p.as_str()).collect();
    if paths.len() == 1 {
        return (paths[0].to_owned(), vec![String::new()]);
    }
    let mut prefix = paths.first().copied().unwrap_or("/");
    for p in &paths[1..] {
        let common = prefix.bytes().zip(p.bytes()).take_while(|(a, b)| a == b).count();
        prefix = &prefix[..common];
    }
    // Cut back to a folder.
    let prefix = &prefix[..prefix.rfind('/').map_or(0, |i| i + 1)];
    let prefix = if prefix.is_empty() { "/" } else { prefix };
    (prefix.to_owned(), paths.iter().map(|p| p[prefix.len()..].to_owned()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `dconf write /a/b "'x'"` sends, built as GLib builds it
    /// (zvariant here: the client's side, not the service's).
    #[allow(deprecated)]
    fn blob(changes: &[(&str, Option<zvariant::Value<'_>>)]) -> Vec<u8> {
        use zvariant::{LE, serialized::Context};
        let map: std::collections::HashMap<&str, Option<zvariant::Value<'_>>> = changes.iter().map(|(k, v)| (*k, v.clone())).collect();
        zvariant::to_bytes(Context::new_gvariant(LE, 0), &map).unwrap().bytes().to_vec()
    }

    #[test]
    fn reads_what_clients_send() {
        let one = parse(&blob(&[("/a/b", Some(zvariant::Value::from("x")))])).unwrap();
        assert_eq!(one, [("/a/b".to_owned(), Some(b"x\0\0s".to_vec()))]);
        let reset = parse(&blob(&[("/a/b", None)])).unwrap();
        assert_eq!(reset, [("/a/b".to_owned(), None)]);
        let dir = parse(&blob(&[("/org/test/", None)])).unwrap();
        assert_eq!(dir, [("/org/test/".to_owned(), None)]);
        let mut many = parse(&blob(&[
            ("/org/test/n", Some(zvariant::Value::from(22i32))),
            ("/org/test/s", Some(zvariant::Value::from("a longer string, well past one byte of offsets ".repeat(8)))),
            ("/org/test/gone", None),
        ]))
        .unwrap();
        many.sort();
        assert_eq!(many[0], ("/org/test/gone".to_owned(), None));
        assert_eq!(many[1], ("/org/test/n".to_owned(), Some(vec![22, 0, 0, 0, 0, b'i'])));
        assert!(many[2].1.as_ref().unwrap().ends_with(b"past one byte of offsets \0\0s"));
        assert!(parse(&[]).unwrap().is_empty());
        assert!(parse(b"\x01\x02\x09").is_err());
        assert!(parse(&blob(&[("no-slash", None)])).is_err());
    }

    #[test]
    fn changes_made_and_told() {
        let mut db = crate::gvdb::Db::new();
        apply(&mut db, &[("/org/test/a".into(), Some(b"x\0\0s".to_vec())), ("/org/test/sub/b".into(), Some(b"y\0\0s".to_vec())), ("/other".into(), Some(b"z\0\0s".to_vec()))]);
        assert_eq!(db.len(), 3);
        apply(&mut db, &[("/org/test/".into(), None)]);
        assert_eq!(db.keys().collect::<Vec<_>>(), ["/other"]);
        apply(&mut db, &[("/other".into(), None)]);
        assert!(db.is_empty());
        assert_eq!(notify_args(&[("/org/test/a".into(), None)]), ("/org/test/a".to_owned(), vec![String::new()]));
        assert_eq!(notify_args(&[("/org/test/a".into(), None), ("/org/test/b/c".into(), None)]), ("/org/test/".to_owned(), vec!["a".to_owned(), "b/c".to_owned()]));
        assert_eq!(notify_args(&[("/org/x".into(), None), ("/net/y".into(), None)]), ("/".to_owned(), vec!["org/x".to_owned(), "net/y".to_owned()]));
        assert_eq!(notify_args(&[("/org/test/ab".into(), None), ("/org/test/ac".into(), None)]), ("/org/test/".to_owned(), vec!["ab".to_owned(), "ac".to_owned()]));
    }
}
