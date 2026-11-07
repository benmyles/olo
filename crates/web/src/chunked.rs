//! A vector in shared chunks: copied in O(chunks), each chunk behind an
//! `Arc`, a write copying only its chunk (once per copy). For what a live
//! page copies every frame to change a little of: its document (the
//! snapshot rendered while scripts keep theirs) and its style tree.
use std::sync::Arc;

#[derive(Clone, Debug)]
/// A vector kept in shared chunks of `CHUNK`: a copy is a copy of the
/// chunks' pointers (a live page copies its style tree each frame to patch
/// a few elements: 0.18 ms for 2,600 styles copied whole), and a write
/// copies only its own chunk, once.
pub(crate) struct Chunked<T> {
    chunks: Vec<Arc<Vec<T>>>,
    len: usize,
}
const CHUNK: usize = 64;
impl<T> Chunked<T> {
    pub(crate) const fn new() -> Self {
        Self { chunks: Vec::new(), len: 0 }
    }
}
impl<T: Clone> Chunked<T> {
    pub(crate) fn len(&self) -> usize {
        self.len
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub(crate) fn push(&mut self, value: T) {
        if self.chunks.last().is_none_or(|c| c.len() == CHUNK) {
            self.chunks.push(Arc::new(Vec::with_capacity(CHUNK)));
        }
        Arc::make_mut(self.chunks.last_mut().expect("made above")).push(value);
        self.len += 1;
    }
    pub(crate) fn first(&self) -> Option<&T> {
        self.chunks.first().and_then(|c| c.first())
    }
    pub(crate) fn last(&self) -> Option<&T> {
        self.chunks.last().and_then(|c| c.last())
    }
    pub(crate) fn iter(&self) -> impl Iterator<Item = &T> {
        self.chunks.iter().flat_map(|c| c.iter())
    }
    pub(crate) fn capacity(&self) -> usize {
        self.chunks.len() * CHUNK
    }
    pub(crate) fn get(&self, i: usize) -> Option<&T> {
        self.chunks.get(i / CHUNK).and_then(|c| c.get(i % CHUNK))
    }
    pub(crate) fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        self.chunks.get_mut(i / CHUNK).and_then(|c| Arc::make_mut(c).get_mut(i % CHUNK))
    }
}
impl<T> std::ops::Index<usize> for Chunked<T> {
    type Output = T;
    fn index(&self, i: usize) -> &T {
        &self.chunks[i / CHUNK][i % CHUNK]
    }
}
impl<T: Clone> std::ops::IndexMut<usize> for Chunked<T> {
    fn index_mut(&mut self, i: usize) -> &mut T {
        &mut Arc::make_mut(&mut self.chunks[i / CHUNK])[i % CHUNK]
    }
}
