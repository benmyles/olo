//! A dconf database file: GLib's GVDB format (gvdb-format.h), as dconf's
//! own writer makes it. Read into each key's value, and written back the
//! same way, so dconf's readers (every GSettings program, mapping the
//! file) find what they always did.
//!
//! The file: a 24-byte header ("GVariant", version 0, options 0, the
//! root table's start and end), then the root hash table (no bloom
//! filter, one bucket per item, the items in bucket order), then the
//! strings, values and lists it points to. Each item is a key: a value
//! (`v`, a serialized GVariant of type "v", 8-aligned), or a folder (`L`,
//! its children's item numbers, 4-aligned), linked to its parent folder
//! (up to "/"), whose key it leaves out of its own ("gnome/" under
//! "/org/"). A value is kept as its bytes, never decoded: what a program
//! wrote is what every reader gets back.

use std::collections::BTreeMap;

/// Each key's value: its full path, and the GVariant "v" data
/// (little-endian) as stored.
pub type Db = BTreeMap<String, Vec<u8>>;

const SIGNATURE: &[u8; 8] = b"GVariant";
const HEADER: usize = 24;
const ITEM: usize = 24;
const NONE: u32 = u32::MAX;

/// gvdb's hash of a key (djb, over signed chars).
pub fn hash(key: &str) -> u32 {
    key.bytes().fold(5381u32, |h, c| h.wrapping_mul(33).wrapping_add(c as i8 as i32 as u32))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, String> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or_else(|| format!("truncated at {at}"))
}

fn u16_at(b: &[u8], at: usize) -> Result<u16, String> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or_else(|| format!("truncated at {at}"))
}

/// The bytes a pointer (start, end) names, `align`-aligned.
fn slice(b: &[u8], start: u32, end: u32, align: usize) -> Result<&[u8], String> {
    let (s, e) = (start as usize, end as usize);
    if s > e || e > b.len() || s % align != 0 {
        return Err(format!("a bad pointer {s}..{e}"));
    }
    Ok(&b[s..e])
}

struct RawItem {
    parent: u32,
    key: String,
    kind: u8,
    start: u32,
    end: u32,
}

/// Read a database file (an empty file, or none, is an empty database).
pub fn read(b: &[u8]) -> Result<Db, String> {
    if b.is_empty() {
        return Ok(Db::new());
    }
    if b.len() < HEADER || &b[..8] != SIGNATURE {
        return Err("not a GVDB file (or not little-endian)".into());
    }
    let (start, end) = (u32_at(b, 16)?, u32_at(b, 20)?);
    let table = slice(b, start, end, 4)?;
    if table.len() < 8 {
        return Ok(Db::new());
    }
    let n_bloom = (u32_at(table, 0)? & ((1 << 27) - 1)) as usize;
    let n_buckets = u32_at(table, 4)? as usize;
    let items_at = 8 + 4 * n_bloom + 4 * n_buckets;
    if items_at > table.len() || (table.len() - items_at) % ITEM != 0 {
        return Err("a bad hash table".into());
    }
    let n = (table.len() - items_at) / ITEM;
    let mut items = Vec::with_capacity(n);
    for i in 0..n {
        let at = items_at + i * ITEM;
        let key_start = u32_at(table, at + 8)?;
        let key_size = u16_at(table, at + 12)? as u32;
        let key = slice(b, key_start, key_start + key_size, 1)?;
        items.push(RawItem {
            parent: u32_at(table, at + 4)?,
            key: String::from_utf8(key.to_vec()).map_err(|_| "a key that isn't UTF-8")?,
            kind: table[at + 14],
            start: u32_at(table, at + 16)?,
            end: u32_at(table, at + 20)?,
        });
    }
    let full = |mut i: usize| -> Result<String, String> {
        let mut parts = Vec::new();
        for _ in 0..=n {
            let item = &items[i];
            parts.push(item.key.as_str());
            if item.parent == NONE {
                parts.reverse();
                return Ok(parts.concat());
            }
            i = item.parent as usize;
            if i >= n {
                return Err("a parent that isn't there".into());
            }
        }
        Err("parents in a loop".into())
    };
    let mut db = Db::new();
    for (i, item) in items.iter().enumerate() {
        if item.kind == b'v' {
            db.insert(full(i)?, slice(b, item.start, item.end, 8)?.to_vec());
        }
    }
    Ok(db)
}

fn align(out: &mut Vec<u8>, to: usize) {
    while out.len() % to != 0 {
        out.push(0);
    }
}

/// The folder a key or folder is in ("/org/gnome/" for
/// "/org/gnome/desktop/" and for "/org/gnome/x"); None for "/".
pub fn parent(key: &str) -> Option<&str> {
    if key == "/" {
        return None;
    }
    let trimmed = key.strip_suffix('/').unwrap_or(key);
    trimmed.rfind('/').map(|i| &key[..=i])
}

/// Write a database file, as dconf's writer would.
pub fn write(db: &Db) -> Vec<u8> {
    // Every key, and every folder up to "/".
    let mut keys: BTreeMap<String, Option<&[u8]>> = BTreeMap::new();
    for (k, v) in db {
        keys.insert(k.clone(), Some(v));
        let mut p = parent(k);
        while let Some(dir) = p {
            if keys.contains_key(dir) {
                break;
            }
            keys.insert(dir.to_owned(), None);
            p = parent(dir);
        }
    }
    let names: Vec<&String> = keys.keys().collect();
    let n = names.len();
    if n == 0 {
        // An empty table.
        let mut out = Vec::new();
        out.extend_from_slice(SIGNATURE);
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(&(HEADER as u32).to_le_bytes());
        out.extend_from_slice(&(HEADER as u32 + 8).to_le_bytes());
        out.extend_from_slice(&[0; 8]);
        return out;
    }
    // One bucket per item; items numbered in bucket order.
    let n_buckets = n;
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| ((hash(names[i]) as usize) % n_buckets, i));
    let mut index = vec![0u32; n];
    for (pos, &i) in order.iter().enumerate() {
        index[i] = pos as u32;
    }
    let position: BTreeMap<&str, usize> = names.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
    let mut buckets = vec![0u32; n_buckets];
    {
        let mut counts = vec![0u32; n_buckets];
        for k in &names {
            counts[(hash(k) as usize) % n_buckets] += 1;
        }
        let mut sum = 0;
        for (b, c) in buckets.iter_mut().zip(&counts) {
            *b = sum;
            sum += c;
        }
    }
    let table_start = HEADER;
    let table_len = 8 + 4 * n_buckets + ITEM * n;
    let mut out = vec![0u8; table_start + table_len];
    out[..8].copy_from_slice(SIGNATURE);
    out[16..20].copy_from_slice(&(table_start as u32).to_le_bytes());
    out[20..24].copy_from_slice(&((table_start + table_len) as u32).to_le_bytes());
    out[table_start + 4..table_start + 8].copy_from_slice(&(n_buckets as u32).to_le_bytes());
    for (b, first) in buckets.iter().enumerate() {
        let at = table_start + 8 + 4 * b;
        out[at..at + 4].copy_from_slice(&first.to_le_bytes());
    }
    // Each folder's children.
    let mut children: BTreeMap<&str, Vec<u32>> = BTreeMap::new();
    for k in &names {
        if let Some(p) = parent(k) {
            children.entry(p).or_default().push(index[position[k.as_str()]]);
        }
    }
    let items_at = table_start + 8 + 4 * n_buckets;
    for (i, k) in names.iter().enumerate() {
        let p = parent(k);
        let stored = p.map_or(k.as_str(), |p| &k[p.len()..]);
        let key_start = out.len() as u32;
        out.extend_from_slice(stored.as_bytes());
        let (kind, start, end) = match keys[k.as_str()] {
            Some(value) => {
                align(&mut out, 8);
                let s = out.len() as u32;
                out.extend_from_slice(value);
                (b'v', s, out.len() as u32)
            }
            None => {
                align(&mut out, 4);
                let s = out.len() as u32;
                for c in children.get(k.as_str()).map(Vec::as_slice).unwrap_or_default() {
                    out.extend_from_slice(&c.to_le_bytes());
                }
                (b'L', s, out.len() as u32)
            }
        };
        let at = items_at + ITEM * index[i] as usize;
        out[at..at + 4].copy_from_slice(&hash(k).to_le_bytes());
        let parent_index = p.map_or(NONE, |p| index[position[p]]);
        out[at + 4..at + 8].copy_from_slice(&parent_index.to_le_bytes());
        out[at + 8..at + 12].copy_from_slice(&key_start.to_le_bytes());
        out[at + 12..at + 14].copy_from_slice(&(stored.len() as u16).to_le_bytes());
        out[at + 14] = kind;
        out[at + 16..at + 20].copy_from_slice(&start.to_le_bytes());
        out[at + 20..at + 24].copy_from_slice(&end.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A GVariant "v" holding the string `s`.
    pub fn string(s: &str) -> Vec<u8> {
        let mut v = s.as_bytes().to_vec();
        v.extend_from_slice(b"\0\0s");
        v
    }

    #[test]
    fn written_and_read_back() {
        let mut db = Db::new();
        db.insert("/org/gnome/desktop/interface/gtk-theme".into(), string("Olo-bebox-theme"));
        db.insert("/org/gnome/desktop/interface/cursor-size".into(), vec![22, 0, 0, 0, 0, b'i']);
        db.insert("/org/gnome/desktop/wm/preferences/button-layout".into(), string("close:maximize"));
        db.insert("/top".into(), string("x"));
        let bytes = write(&db);
        assert_eq!(read(&bytes).unwrap(), db);
        assert_eq!(read(&write(&Db::new())).unwrap(), Db::new());
        assert_eq!(read(&[]).unwrap(), Db::new());
        assert!(read(b"not a database at all....").is_err());
    }

    #[test]
    fn folders_link_their_children() {
        assert_eq!(parent("/org/gnome/desktop/"), Some("/org/gnome/"));
        assert_eq!(parent("/org/gnome/x"), Some("/org/gnome/"));
        assert_eq!(parent("/org/"), Some("/"));
        assert_eq!(parent("/x"), Some("/"));
        assert_eq!(parent("/"), None);
        // gvdb's hash, as GLib computes it.
        assert_eq!(hash(""), 5381);
        assert_eq!(hash("/"), 5381 * 33 + b'/' as u32);
    }
}
