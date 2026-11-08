//! A folder as kept: its entries, by name, each with what's needed to put
//! it back (and to see, next time, that a file hasn't changed without
//! reading it: size, change time, inode).

use serde::{Deserialize, Serialize};

use crate::crypto::Id;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    File,
    Dir,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// Its name's bytes (any a file system allows).
    #[serde(with = "bytes")]
    pub name: Vec<u8>,
    pub kind: Kind,
    /// Permission bits.
    pub mode: u32,
    /// Modified and changed, ns since 1970.
    pub mtime: i64,
    pub ctime: i64,
    pub ino: u64,
    pub size: u64,
    /// A file's chunks, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<Id>,
    /// A folder's tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtree: Option<Id>,
    /// A link's target.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_bytes")]
    pub target: Option<Vec<u8>>,
}

impl Node {
    pub fn name_str(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.name)
    }

    /// The same file as `other` was, by what the file system says (no
    /// need to read it again).
    pub fn unchanged(&self, other: &Node) -> bool {
        self.kind == other.kind && self.size == other.size && self.mtime == other.mtime && self.ctime == other.ctime && self.ino == other.ino
    }
}

/// A folder's entries, sorted by name.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tree {
    pub nodes: Vec<Node>,
}

impl Tree {
    pub fn get(&self, name: &[u8]) -> Option<&Node> {
        self.nodes.binary_search_by(|n| n.name.as_slice().cmp(name)).ok().map(|i| &self.nodes[i])
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        ciborium::into_writer(self, &mut out).expect("a tree encodes");
        out
    }

    pub fn decode(data: &[u8]) -> crate::Result<Tree> {
        ciborium::from_reader(data).map_err(|e| crate::Error::Damaged(format!("a folder's list: {e}")))
    }
}

mod bytes {
    pub fn serialize<S: serde::Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(v)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Vec<u8>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("bytes")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Vec<u8>, E> {
                Ok(v.to_vec())
            }
            fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<Vec<u8>, E> {
                Ok(v)
            }
        }
        d.deserialize_bytes(V)
    }
}

mod opt_bytes {
    pub fn serialize<S: serde::Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(b) => s.serialize_bytes(b),
            None => s.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        super::bytes::deserialize(d).map(Some)
    }
}
