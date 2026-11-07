//! Compact document storage with document-scoped, generational handles.
//! This is storage and tree mutation, not yet a complete DOM/Web IDL API.
//! No recursive tree ownership, per-node strings, locks, timers or UI work.

use std::{
    borrow::Cow,
    collections::HashMap,
    num::NonZeroU32,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

type Link = Option<NonZeroU32>;
type Atom = NonZeroU32;
static NEXT_DOCUMENT: AtomicU64 = AtomicU64::new(1);

fn index(link: NonZeroU32) -> usize {
    link.get() as usize - 1
}
fn link(index: usize) -> NonZeroU32 {
    NonZeroU32::new(u32::try_from(index + 1).expect("bounded arena")).unwrap()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId {
    document: u64,
    slot: u32,
    generation: u32,
}

impl NodeId {
    pub fn document(&self) -> u64 {
        self.document
    }
    pub fn slot(&self) -> u32 {
        self.slot
    }
    pub fn generation(&self) -> u32 {
        self.generation
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:x}:{:x}:{:x}", self.document, self.slot, self.generation)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Namespace {
    Html,
    Svg,
    MathMl,
}

/// Attribute namespaces recognized by the HTML foreign-content parser.
/// Arbitrary XML namespace URIs and Web IDL setters remain unimplemented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum AttributeNamespace {
    Xlink = 1,
    Xml = 2,
    Xmlns = 3,
}
impl AttributeNamespace {
    pub fn uri(self) -> &'static str {
        match self {
            Self::Xlink => "http://www.w3.org/1999/xlink",
            Self::Xml => "http://www.w3.org/XML/1998/namespace",
            Self::Xmlns => "http://www.w3.org/2000/xmlns/",
        }
    }
}

// Namespace bits share the interned-name key. An attribute stays 16 bytes,
// including its value and link, and ordinary attributes need no side table.
// The name arena explicitly bounds IDs to the remaining 30 bits.
const NAME_MASK: u32 = (1 << 30) - 1;
#[derive(Clone, Copy, Debug)]
struct AttributeName(NonZeroU32);
impl AttributeName {
    fn new(atom: Atom, namespace: Option<AttributeNamespace>) -> Self {
        Self(NonZeroU32::new(atom.get() | (namespace.map_or(0, |n| n as u32) << 30)).unwrap())
    }
    fn atom(self) -> Atom {
        NonZeroU32::new(self.0.get() & NAME_MASK).unwrap()
    }
    fn namespace(self) -> Option<AttributeNamespace> {
        match self.0.get() >> 30 {
            1 => Some(AttributeNamespace::Xlink),
            2 => Some(AttributeNamespace::Xml),
            3 => Some(AttributeNamespace::Xmlns),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttributeRef<'a> {
    pub qualified_name: &'a str,
    pub local_name: &'a str,
    pub prefix: Option<&'a str>,
    pub namespace: Option<AttributeNamespace>,
    pub value: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeType {
    Document,
    Fragment,
    Element,
    Text,
    Comment,
    ProcessingInstruction,
    Doctype,
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub nodes: usize,
    pub attributes: usize,
    pub source_bytes: usize,
    pub owned_text_bytes: usize,
    pub name_bytes: usize,
    pub depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { nodes: 1_000_000, attributes: 2_000_000, source_bytes: 32 * 1024 * 1024, owned_text_bytes: 64 * 1024 * 1024, name_bytes: 4 * 1024 * 1024, depth: 1024 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    WrongDocument,
    StaleNode,
    Hierarchy,
    Cycle,
    NodeLimit,
    AttributeLimit,
    TextLimit,
    NameLimit,
    DepthLimit,
    InvalidName,
    RevisionOverflow,
    Cancelled,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DOM {:?}", self)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

// Two u32s: a range in the original UTF-8 source, or an owned-string
// pool index when len is MAX (boxed) or MAX-1 (amortized append buffer).
// Text tokens without escapes keep no copy.
#[derive(Clone, Copy, Debug)]
struct TextValue {
    start: u32,
    len: u32,
}

#[derive(Clone, Debug)]
enum Data {
    Vacant(Link),
    Document,
    Fragment { host: Link },
    Template { attributes: Link, contents: Link },
    Element { name: Atom, namespace: Namespace, attributes: Link },
    Text(TextValue),
    Comment(TextValue),
    ProcessingInstruction { target: Atom, data: TextValue },
    Doctype(NonZeroU32),
}

#[derive(Clone, Debug)]
struct DoctypeData {
    name: Atom,
    public_id: TextValue,
    system_id: TextValue,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Doctype<'a> {
    pub name: &'a str,
    pub public_id: &'a str,
    pub system_id: &'a str,
}

#[derive(Clone, Debug)]
struct Node {
    generation: u32,
    parent: Link,
    first: Link,
    last: Link,
    previous: Link,
    next: Link,
    data: Data,
}

impl Node {
    fn new(data: Data, generation: u32) -> Self {
        Self { generation, parent: None, first: None, last: None, previous: None, next: None, data }
    }
}

#[derive(Clone, Debug)]
struct Attribute {
    name: AttributeName,
    value: TextValue,
    next: Link,
}

#[derive(Debug, Clone)]
struct Pool<T> {
    slots: Vec<Option<T>>,
    free: Vec<u32>,
    live: usize,
}
impl<T> Default for Pool<T> {
    fn default() -> Self {
        Self { slots: Vec::new(), free: Vec::new(), live: 0 }
    }
}
impl<T> Pool<T> {
    fn insert(&mut self, value: T) -> NonZeroU32 {
        self.live += 1;
        if let Some(at) = self.free.pop() {
            self.slots[at as usize] = Some(value);
            link(at as usize)
        } else {
            let at = self.slots.len();
            self.slots.push(Some(value));
            link(at)
        }
    }
    fn get(&self, id: NonZeroU32) -> &T {
        self.slots[index(id)].as_ref().expect("live pool entry")
    }
    fn get_mut(&mut self, id: NonZeroU32) -> &mut T {
        self.slots[index(id)].as_mut().expect("live pool entry")
    }
    fn remove(&mut self, id: NonZeroU32) -> T {
        let at = index(id);
        let value = self.slots[at].take().expect("live pool entry");
        self.free.push(at as u32);
        self.live -= 1;
        value
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Memory {
    pub source_bytes: usize,
    pub node_size: usize,
    pub live_nodes: usize,
    pub node_reserved_bytes: usize,
    pub live_attributes: usize,
    pub attribute_reserved_bytes: usize,
    pub owned_text_bytes: usize,
    pub owned_text_slots: usize,
    pub interned_name_bytes: usize,
}

/// A document owns one immutable source and flat, reusable arenas.
/// References to a removed node fail even after its slot is reused; IDs
/// from another navigation/document always fail. Read methods borrow the
/// document, so mutation cannot invalidate a live Rust view/iterator.
/// One change to the document, for restyling only what changed: the
/// revision it made, and what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// An element's attributes changed (its own matching inputs), which
    /// one unknown.
    Node(NodeId),
    /// One attribute set, changed or removed: its name and the values
    /// before and after, for telling which selectors could care.
    Attribute { node: NodeId, name: Box<str>, old: Option<Box<str>>, new: Option<Box<str>> },
    /// An element's `style` attribute changed: its declared values, never
    /// what selectors match (unless one tests the attribute itself).
    Style(NodeId),
    /// A text node's data changed (what its parent lays out).
    Text(NodeId),
    /// A node's child list changed: children inserted, removed or moved
    /// (every child's sibling-dependent matching, and what the node holds).
    Children(NodeId),
    /// A change the log does not describe: everything may have changed.
    Unknown,
}
/// How many changes the log keeps before giving up on describing them.
const CHANGES: usize = 1 << 16;
#[derive(Clone)]
pub struct Document {
    id: u64,
    revision: u64,
    /// Changes since `changes_from`, each with the revision it made.
    changes: Vec<(u64, Change)>,
    changes_from: u64,
    source: Arc<str>,
    nodes: Vec<Node>,
    free_nodes: Link,
    live_nodes: usize,
    attributes: Pool<Attribute>,
    doctypes: Pool<DoctypeData>,
    owned: Pool<Box<str>>,
    growing: Pool<String>,
    owned_bytes: usize,
    names: Vec<Arc<str>>,
    atoms: HashMap<Arc<str>, Atom>,
    name_bytes: usize,
    limits: Limits,
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Source and form text can contain credentials/session tokens.
        // Debugging a document must not dump them into logs or a panic.
        f.debug_struct("Document").field("id", &self.id).field("revision", &self.revision).field("memory", &self.memory()).finish_non_exhaustive()
    }
}

impl Document {
    pub fn new(source: Arc<str>, limits: Limits) -> Result<Self> {
        if source.len() > limits.source_bytes || source.len() >= (u32::MAX - 1) as usize {
            return Err(Error::TextLimit);
        }
        if limits.nodes == 0 || limits.nodes >= u32::MAX as usize {
            return Err(Error::NodeLimit);
        }
        if limits.attributes >= u32::MAX as usize {
            return Err(Error::AttributeLimit);
        }
        let id = NEXT_DOCUMENT.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1)).map_err(|_| Error::RevisionOverflow)?;
        Ok(Self {
            id,
            revision: 0,
            changes: Vec::new(),
            changes_from: 0,
            source,
            nodes: vec![Node::new(Data::Document, 1)],
            free_nodes: None,
            live_nodes: 1,
            attributes: Pool::default(),
            doctypes: Pool::default(),
            owned: Pool::default(),
            growing: Pool::default(),
            owned_bytes: 0,
            names: Vec::new(),
            atoms: HashMap::new(),
            name_bytes: 0,
            limits,
        })
    }

    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn root(&self) -> NodeId {
        self.handle(link(0))
    }
    pub fn source(&self) -> &Arc<str> {
        &self.source
    }
    pub fn memory(&self) -> Memory {
        Memory {
            source_bytes: self.source.len(),
            node_size: std::mem::size_of::<Node>(),
            live_nodes: self.live_nodes,
            node_reserved_bytes: self.nodes.capacity() * std::mem::size_of::<Node>(),
            live_attributes: self.attributes.live,
            attribute_reserved_bytes: self.attributes.slots.capacity() * std::mem::size_of::<Option<Attribute>>(),
            owned_text_bytes: self.owned_bytes,
            owned_text_slots: self.owned.slots.len() + self.growing.slots.len(),
            interned_name_bytes: self.name_bytes,
        }
    }

    pub(crate) fn handle(&self, at: NonZeroU32) -> NodeId {
        NodeId { document: self.id, slot: index(at) as u32, generation: self.nodes[index(at)].generation }
    }
    fn resolve(&self, id: NodeId) -> Result<NonZeroU32> {
        if id.document != self.id {
            return Err(Error::WrongDocument);
        }
        let node = self.nodes.get(id.slot as usize).ok_or(Error::StaleNode)?;
        if node.generation != id.generation || matches!(node.data, Data::Vacant(_)) {
            return Err(Error::StaleNode);
        }
        Ok(link(id.slot as usize))
    }
    /// A mutation the log cannot describe: restyles start over.
    fn bump(&mut self) -> Result<()> {
        self.bump_with(Change::Unknown)
    }
    fn bump_with(&mut self, change: Change) -> Result<()> {
        self.revision = self.revision.checked_add(1).ok_or(Error::RevisionOverflow)?;
        if self.changes.len() >= CHANGES {
            self.changes.clear();
            self.changes_from = self.revision;
        }
        self.changes.push((self.revision, change));
        Ok(())
    }
    /// The changes made after `revision`, oldest first, or None when the log
    /// no longer reaches back that far (then everything may have changed).
    pub fn changes_since(&self, revision: u64) -> Option<&[(u64, Change)]> {
        if revision < self.changes_from {
            return None;
        }
        let start = self.changes.partition_point(|(r, _)| *r <= revision);
        Some(&self.changes[start..])
    }
    fn can_allocate(&self) -> Result<()> {
        self.can_allocate_count(1)
    }
    fn can_allocate_count(&self, count: usize) -> Result<()> {
        if count > self.limits.nodes.saturating_sub(self.live_nodes) {
            Err(Error::NodeLimit)
        } else if self.revision.checked_add(count as u64).is_none() {
            Err(Error::RevisionOverflow)
        } else {
            Ok(())
        }
    }
    fn allocate(&mut self, data: Data) -> NodeId {
        let at = if let Some(at) = self.free_nodes {
            let Data::Vacant(next) = self.nodes[index(at)].data else { unreachable!() };
            self.free_nodes = next;
            let generation = self.nodes[index(at)].generation;
            self.nodes[index(at)] = Node::new(data, generation);
            at
        } else {
            let at = link(self.nodes.len());
            self.nodes.push(Node::new(data, 1));
            at
        };
        self.live_nodes += 1;
        self.revision += 1; // checked by can_allocate before any allocation
        self.handle(at)
    }

    fn atom(&mut self, name: &str) -> Result<Atom> {
        if let Some(id) = self.atoms.get(name) {
            return Ok(*id);
        }
        if name.len() > self.limits.name_bytes.saturating_sub(self.name_bytes) || self.names.len() >= NAME_MASK as usize {
            return Err(Error::NameLimit);
        }
        let id = link(self.names.len());
        let name: Arc<str> = name.into();
        self.name_bytes += name.len();
        self.names.push(name.clone());
        self.atoms.insert(name, id);
        Ok(id)
    }
    fn text(&self, value: TextValue) -> &str {
        if value.len == u32::MAX {
            self.owned.get(NonZeroU32::new(value.start).unwrap())
        } else if value.len == u32::MAX - 1 {
            self.growing.get(NonZeroU32::new(value.start).unwrap())
        } else {
            &self.source[value.start as usize..(value.start + value.len) as usize]
        }
    }
    fn owned_len(&self, value: TextValue) -> usize {
        if value.len == u32::MAX {
            self.text(value).len()
        } else if value.len == u32::MAX - 1 {
            self.growing.get(NonZeroU32::new(value.start).unwrap()).capacity()
        } else {
            0
        }
    }
    fn store_text(&mut self, text: Cow<'_, str>, credit: usize) -> Result<TextValue> {
        if text.is_empty() {
            return Ok(TextValue { start: 0, len: 0 });
        }
        let source_start = self.source.as_ptr() as usize;
        let text_start = text.as_ptr() as usize;
        if let Some(offset) = text_start.checked_sub(source_start).filter(|offset| *offset <= self.source.len() && text.len() <= self.source.len() - *offset) {
            return Ok(TextValue { start: offset as u32, len: text.len() as u32 });
        }
        let retained = self.owned_bytes.saturating_sub(credit);
        if text.len() > self.limits.owned_text_bytes.saturating_sub(retained) {
            return Err(Error::TextLimit);
        }
        let len = text.len();
        #[cfg(feature = "baseline-storage")]
        let retained: Box<str> = text.as_ref().into();
        #[cfg(not(feature = "baseline-storage"))]
        let retained = text.into_owned().into_boxed_str();
        let id = self.owned.insert(retained);
        self.owned_bytes += len;
        Ok(TextValue { start: id.get(), len: u32::MAX })
    }
    fn free_text(&mut self, value: TextValue) {
        if value.len == u32::MAX {
            self.owned_bytes -= self.owned.remove(NonZeroU32::new(value.start).unwrap()).len();
        } else if value.len == u32::MAX - 1 {
            self.owned_bytes -= self.growing.remove(NonZeroU32::new(value.start).unwrap()).capacity();
        }
    }

    /// Parsing can also use names accepted by HTML's permissive tokenizer.
    /// Web IDL's name validation belongs at that boundary; it must follow
    /// the current DOM Standard, whose names are broader than XML's.
    pub fn create_element(&mut self, name: &str, namespace: Namespace) -> Result<NodeId> {
        self.can_allocate()?;
        if name.is_empty() || name.contains('\0') {
            return Err(Error::InvalidName);
        }
        let normalized = if namespace == Namespace::Html && name.bytes().any(|b| b.is_ascii_uppercase()) { Cow::Owned(name.to_ascii_lowercase()) } else { Cow::Borrowed(name) };
        if namespace == Namespace::Html && normalized == "template" {
            return self.create_template();
        }
        let name = self.atom(&normalized)?;
        Ok(self.allocate(Data::Element { name, namespace, attributes: None }))
    }
    pub fn create_fragment(&mut self) -> Result<NodeId> {
        self.can_allocate()?;
        Ok(self.allocate(Data::Fragment { host: None }))
    }
    fn create_template(&mut self) -> Result<NodeId> {
        // Host and content root are one bounded allocation. The special
        // variant fits the existing node slot without a side table.
        self.can_allocate_count(2)?;
        let host = self.allocate(Data::Template { attributes: None, contents: None });
        let fragment = self.allocate(Data::Fragment { host: Some(link(host.slot as usize)) });
        let Data::Template { contents, .. } = &mut self.nodes[host.slot as usize].data else { unreachable!() };
        *contents = Some(link(fragment.slot as usize));
        Ok(host)
    }
    pub fn template_contents(&self, id: NodeId) -> Result<Option<NodeId>> {
        Ok(match self.nodes[index(self.resolve(id)?)].data {
            Data::Template { contents, .. } => contents.map(|at| self.handle(at)),
            _ => None,
        })
    }
    // Ownership traversal includes template content roots. Public DOM
    // child/sibling APIs expose only the actual light-DOM relationships.
    fn owned_first(&self, at: NonZeroU32) -> Link {
        if let Data::Template { contents: Some(contents), .. } = self.nodes[index(at)].data { Some(contents) } else { self.nodes[index(at)].first }
    }
    fn owned_next(&self, at: NonZeroU32) -> Link {
        if let Data::Fragment { host: Some(host) } = self.nodes[index(at)].data { self.nodes[index(host)].first } else { self.nodes[index(at)].next }
    }
    fn owned_parent(&self, at: NonZeroU32) -> Link {
        if let Data::Fragment { host: Some(host) } = self.nodes[index(at)].data { Some(host) } else { self.nodes[index(at)].parent }
    }
    pub fn create_doctype<'a>(&mut self, name: &str, public_id: impl Into<Cow<'a, str>>, system_id: impl Into<Cow<'a, str>>) -> Result<NodeId> {
        self.can_allocate()?;
        let name = self.atom(name)?;
        let public_id = self.store_text(public_id.into(), 0)?;
        let system_id = match self.store_text(system_id.into(), 0) {
            Ok(v) => v,
            Err(e) => {
                self.free_text(public_id);
                return Err(e);
            }
        };
        let id = self.doctypes.insert(DoctypeData { name, public_id, system_id });
        Ok(self.allocate(Data::Doctype(id)))
    }
    pub fn doctype(&self, id: NodeId) -> Result<Option<Doctype<'_>>> {
        Ok(match self.nodes[index(self.resolve(id)?)].data {
            Data::Doctype(id) => {
                let d = self.doctypes.get(id);
                Some(Doctype { name: &self.names[index(d.name)], public_id: self.text(d.public_id), system_id: self.text(d.system_id) })
            }
            _ => None,
        })
    }
    pub fn create_text<'a>(&mut self, text: impl Into<Cow<'a, str>>) -> Result<NodeId> {
        self.can_allocate()?;
        let value = self.store_text(text.into(), 0)?;
        Ok(self.allocate(Data::Text(value)))
    }
    pub fn create_comment<'a>(&mut self, text: impl Into<Cow<'a, str>>) -> Result<NodeId> {
        self.can_allocate()?;
        let value = self.store_text(text.into(), 0)?;
        Ok(self.allocate(Data::Comment(value)))
    }
    pub fn create_processing_instruction<'a>(&mut self, target: &str, data: impl Into<Cow<'a, str>>) -> Result<NodeId> {
        self.can_allocate()?;
        if target.is_empty() {
            return Err(Error::InvalidName);
        }
        let target = self.atom(target)?;
        let data = self.store_text(data.into(), 0)?;
        Ok(self.allocate(Data::ProcessingInstruction { target, data }))
    }

    pub fn node_type(&self, id: NodeId) -> Result<NodeType> {
        Ok(match self.nodes[index(self.resolve(id)?)].data {
            Data::Document => NodeType::Document,
            Data::Fragment { .. } => NodeType::Fragment,
            Data::Element { .. } | Data::Template { .. } => NodeType::Element,
            Data::Text(_) => NodeType::Text,
            Data::Comment(_) => NodeType::Comment,
            Data::ProcessingInstruction { .. } => NodeType::ProcessingInstruction,
            Data::Doctype(_) => NodeType::Doctype,
            Data::Vacant(_) => unreachable!(),
        })
    }
    pub fn name(&self, id: NodeId) -> Result<Option<&str>> {
        Ok(match self.nodes[index(self.resolve(id)?)].data {
            Data::Template { .. } => Some("template"),
            Data::Element { name, .. } | Data::ProcessingInstruction { target: name, .. } => Some(self.names[index(name)].as_ref()),
            Data::Doctype(id) => Some(&self.names[index(self.doctypes.get(id).name)]),
            _ => None,
        })
    }
    /// One checked lookup for the tree builder's namespace-sensitive rules.
    pub(crate) fn element_name_and_namespace(&self, id: NodeId) -> Result<(&str, Namespace)> {
        let at = self.resolve(id)?;
        match self.nodes[index(at)].data {
            Data::Template { .. } => Ok(("template", Namespace::Html)),
            Data::Element { name, namespace, .. } => Ok((&self.names[index(name)], namespace)),
            _ => Err(Error::Hierarchy),
        }
    }
    pub fn namespace(&self, id: NodeId) -> Result<Option<Namespace>> {
        Ok(match self.nodes[index(self.resolve(id)?)].data {
            Data::Template { .. } => Some(Namespace::Html),
            Data::Element { namespace, .. } => Some(namespace),
            _ => None,
        })
    }
    pub fn value(&self, id: NodeId) -> Result<Option<&str>> {
        Ok(match self.nodes[index(self.resolve(id)?)].data {
            Data::Text(v) | Data::Comment(v) | Data::ProcessingInstruction { data: v, .. } => Some(self.text(v)),
            _ => None,
        })
    }
    pub fn parent(&self, id: NodeId) -> Result<Option<NodeId>> {
        Ok(self.nodes[index(self.resolve(id)?)].parent.map(|at| self.handle(at)))
    }
    pub fn first_child(&self, id: NodeId) -> Result<Option<NodeId>> {
        Ok(self.nodes[index(self.resolve(id)?)].first.map(|at| self.handle(at)))
    }
    pub fn last_child(&self, id: NodeId) -> Result<Option<NodeId>> {
        Ok(self.nodes[index(self.resolve(id)?)].last.map(|at| self.handle(at)))
    }
    pub fn previous_sibling(&self, id: NodeId) -> Result<Option<NodeId>> {
        Ok(self.nodes[index(self.resolve(id)?)].previous.map(|at| self.handle(at)))
    }
    pub fn next_sibling(&self, id: NodeId) -> Result<Option<NodeId>> {
        Ok(self.nodes[index(self.resolve(id)?)].next.map(|at| self.handle(at)))
    }
    pub fn children(&self, id: NodeId) -> Result<Children<'_>> {
        Ok(Children { document: self, next: self.nodes[index(self.resolve(id)?)].first })
    }

    pub fn set_value<'a>(&mut self, id: NodeId, text: impl Into<Cow<'a, str>>) -> Result<bool> {
        let text = text.into();
        let at = self.resolve(id)?;
        let old = match self.nodes[index(at)].data {
            Data::Text(v) | Data::Comment(v) | Data::ProcessingInstruction { data: v, .. } => v,
            _ => return Err(Error::Hierarchy),
        };
        if self.text(old) == text.as_ref() {
            return Ok(false);
        }
        if self.revision == u64::MAX {
            return Err(Error::RevisionOverflow);
        }
        let new = self.store_text(text, self.owned_len(old))?;
        self.free_text(old);
        match &mut self.nodes[index(at)].data {
            Data::Text(v) | Data::Comment(v) | Data::ProcessingInstruction { data: v, .. } => *v = new,
            _ => unreachable!(),
        }
        let id = self.handle(at);
        self.bump_with(Change::Text(id))?;
        Ok(true)
    }

    /// Adjacent source slices merge without copying. Other text promotes
    /// once to a reusable String and grows geometrically. The quota counts
    /// capacity, including spare capacity, rather than just visible bytes.
    pub fn append_value<'a>(&mut self, id: NodeId, text: impl Into<Cow<'a, str>>) -> Result<bool> {
        let text = text.into();
        let at = self.resolve(id)?;
        let old = match self.nodes[index(at)].data {
            Data::Text(v) | Data::Comment(v) | Data::ProcessingInstruction { data: v, .. } => v,
            _ => return Err(Error::Hierarchy),
        };
        if text.is_empty() {
            return Ok(false);
        }
        if self.revision == u64::MAX {
            return Err(Error::RevisionOverflow);
        }
        if self.text(old).is_empty() {
            return self.set_value(id, text);
        }
        if old.len < u32::MAX - 1 {
            let end = old.start as usize + old.len as usize;
            if self.source.get(end..).is_some_and(|rest| text.as_ptr() == rest.as_ptr() && text.len() <= rest.len()) {
                self.replace_character_value(at, TextValue { start: old.start, len: old.len + text.len() as u32 });
                let id = self.handle(at);
                self.bump_with(Change::Text(id))?;
                return Ok(true);
            }
        }
        let required = self.text(old).len().checked_add(text.len()).ok_or(Error::TextLimit)?;
        let cost = self.owned_len(old);
        let available = self.limits.owned_text_bytes.saturating_sub(self.owned_bytes - cost);
        if required > available {
            return Err(Error::TextLimit);
        }
        if old.len == u32::MAX - 1 {
            let value = self.growing.get_mut(NonZeroU32::new(old.start).unwrap());
            if value.capacity() >= required {
                value.push_str(&text);
                let id = self.handle(at);
                self.bump_with(Change::Text(id))?;
                return Ok(true);
            }
        }
        let desired = required.max(self.text(old).len().saturating_mul(2)).min(available);
        let mut value = if old.len == u32::MAX {
            self.owned_bytes -= cost;
            self.owned.remove(NonZeroU32::new(old.start).unwrap()).into_string()
        } else if old.len == u32::MAX - 1 {
            self.owned_bytes -= cost;
            self.growing.remove(NonZeroU32::new(old.start).unwrap())
        } else {
            let mut value = String::with_capacity(desired);
            value.push_str(self.text(old));
            value
        };
        if desired > value.capacity() {
            value.reserve_exact(desired - value.len());
        }
        value.push_str(&text);
        self.owned_bytes += value.capacity();
        let slot = self.growing.insert(value);
        self.replace_character_value(at, TextValue { start: slot.get(), len: u32::MAX - 1 });
        let id = self.handle(at);
        self.bump_with(Change::Text(id))?;
        Ok(true)
    }

    fn replace_character_value(&mut self, at: NonZeroU32, value: TextValue) {
        match &mut self.nodes[index(at)].data {
            Data::Text(v) | Data::Comment(v) | Data::ProcessingInstruction { data: v, .. } => *v = value,
            _ => unreachable!(),
        }
    }

    fn attribute_head(&self, at: NonZeroU32) -> Result<Link> {
        match self.nodes[index(at)].data {
            Data::Element { attributes, .. } | Data::Template { attributes, .. } => Ok(attributes),
            _ => Err(Error::Hierarchy),
        }
    }
    fn normalize_attribute<'n>(&self, at: NonZeroU32, name: &'n str) -> Cow<'n, str> {
        if matches!(self.nodes[index(at)].data, Data::Element { namespace: Namespace::Html, .. } | Data::Template { .. }) && name.bytes().any(|b| b.is_ascii_uppercase()) {
            Cow::Owned(name.to_ascii_lowercase())
        } else {
            Cow::Borrowed(name)
        }
    }
    pub fn attributes(&self, id: NodeId) -> Result<Attributes<'_>> {
        let at = self.resolve(id)?;
        Ok(Attributes { document: self, next: self.attribute_head(at)? })
    }
    /// Structured attribute identity, including the namespace and prefix.
    pub fn attributes_ns(&self, id: NodeId) -> Result<NamespacedAttributes<'_>> {
        let at = self.resolve(id)?;
        Ok(NamespacedAttributes { document: self, next: self.attribute_head(at)? })
    }
    pub fn attribute_ns(&self, id: NodeId, namespace: Option<AttributeNamespace>, local_name: &str) -> Result<Option<&str>> {
        Ok(self.attributes_ns(id)?.find(|a| a.namespace == namespace && a.local_name == local_name).map(|a| a.value))
    }
    pub fn attribute(&self, id: NodeId, name: &str) -> Result<Option<&str>> {
        let at = self.resolve(id)?;
        let name = self.normalize_attribute(at, name);
        let atom = self.atoms.get(name.as_ref());
        let mut next = self.attribute_head(at)?;
        while let Some(at) = next {
            let a = self.attributes.get(at);
            if Some(&a.name.atom()) == atom {
                return Ok(Some(self.text(a.value)));
            }
            next = a.next;
        }
        Ok(None)
    }
    pub fn set_attribute<'a>(&mut self, id: NodeId, name: &str, value: impl Into<Cow<'a, str>>) -> Result<bool> {
        let value = value.into();
        let at = self.resolve(id)?;
        let mut next = self.attribute_head(at)?;
        if name.is_empty() || name.contains('\0') {
            return Err(Error::InvalidName);
        }
        let normalized = self.normalize_attribute(at, name);
        let atom = self.atoms.get(normalized.as_ref()).copied();
        let mut last = None;
        while let Some(attr) = next {
            let old = self.attributes.get(attr);
            if Some(old.name.atom()) == atom {
                if self.text(old.value) == value.as_ref() {
                    return Ok(false);
                }
                if self.revision == u64::MAX {
                    return Err(Error::RevisionOverflow);
                }
                let old = old.value;
                let previous: Box<str> = if normalized.as_ref() == "style" { Box::from("") } else { Box::from(self.text(old)) };
                let new = self.store_text(value, self.owned_len(old))?;
                self.free_text(old);
                self.attributes.get_mut(attr).value = new;
                let id = self.handle(at);
                let change = if normalized.as_ref() == "style" {
                    Change::Style(id)
                } else {
                    Change::Attribute { node: id, name: Box::from(normalized.as_ref()), old: Some(previous), new: Some(Box::from(self.text(new))) }
                };
                self.bump_with(change)?;
                return Ok(true);
            }
            last = Some(attr);
            next = old.next;
        }
        if self.attributes.live >= self.limits.attributes {
            return Err(Error::AttributeLimit);
        }
        if self.revision == u64::MAX {
            return Err(Error::RevisionOverflow);
        }
        let name = self.atom(&normalized)?;
        let value = self.store_text(value, 0)?;
        let attr = self.attributes.insert(Attribute { name: AttributeName::new(name, None), value, next: None });
        if let Some(last) = last {
            self.attributes.get_mut(last).next = Some(attr);
        } else {
            let (Data::Element { attributes, .. } | Data::Template { attributes, .. }) = &mut self.nodes[index(at)].data else { unreachable!() };
            *attributes = Some(attr);
        }
        let id = self.handle(at);
        let change =
            if normalized.as_ref() == "style" { Change::Style(id) } else { Change::Attribute { node: id, name: Box::from(normalized.as_ref()), old: None, new: Some(Box::from(self.text(value))) } };
        self.bump_with(change)?;
        Ok(true)
    }

    /// Parser-only path for a fresh element. The tokenizer has already
    /// normalized/deduplicated these names, so do not search an increasingly
    /// long attribute list for every insertion (quadratic on large tags).
    pub(crate) fn initial_attributes<'a>(&mut self, id: NodeId, attributes: impl IntoIterator<Item = (Cow<'a, str>, Cow<'a, str>)>) -> Result<()> {
        self.initial_attributes_ns(id, attributes.into_iter().map(|(name, value)| (name, value, None)))
    }
    pub(crate) fn initial_attributes_ns<'a>(&mut self, id: NodeId, attributes: impl IntoIterator<Item = (Cow<'a, str>, Cow<'a, str>, Option<AttributeNamespace>)>) -> Result<()> {
        let node = self.resolve(id)?;
        if self.attribute_head(node)?.is_some() {
            return Err(Error::Hierarchy);
        }
        let mut last = None;
        for (name, value, namespace) in attributes {
            if self.attributes.live >= self.limits.attributes {
                return Err(Error::AttributeLimit);
            }
            if self.revision == u64::MAX {
                return Err(Error::RevisionOverflow);
            }
            let name = self.atom(&name)?;
            let value = self.store_text(value, 0)?;
            let attr = self.attributes.insert(Attribute { name: AttributeName::new(name, namespace), value, next: None });
            if let Some(last) = last {
                self.attributes.get_mut(last).next = Some(attr);
            } else {
                let (Data::Element { attributes, .. } | Data::Template { attributes, .. }) = &mut self.nodes[index(node)].data else { unreachable!() };
                *attributes = Some(attr);
            }
            last = Some(attr);
            let id = self.handle(node);
            self.bump_with(Change::Node(id))?;
        }
        Ok(())
    }
    pub fn remove_attribute(&mut self, id: NodeId, name: &str) -> Result<bool> {
        let at = self.resolve(id)?;
        let normalized = self.normalize_attribute(at, name);
        let atom = self.atoms.get(normalized.as_ref()).copied();
        let mut next = self.attribute_head(at)?;
        let mut previous = None;
        while let Some(attr) = next {
            let a = self.attributes.get(attr);
            if Some(a.name.atom()) == atom {
                let id = self.handle(at);
                let change = if normalized.as_ref() == "style" {
                    Change::Style(id)
                } else {
                    Change::Attribute { node: id, name: Box::from(normalized.as_ref()), old: Some(Box::from(self.text(a.value))), new: None }
                };
                self.bump_with(change)?;
                let a = self.attributes.remove(attr);
                if let Some(previous) = previous {
                    self.attributes.get_mut(previous).next = a.next;
                } else {
                    let (Data::Element { attributes, .. } | Data::Template { attributes, .. }) = &mut self.nodes[index(at)].data else { unreachable!() };
                    *attributes = a.next;
                }
                self.free_text(a.value);
                return Ok(true);
            }
            previous = Some(attr);
            next = a.next;
        }
        Ok(false)
    }

    fn unlink(&mut self, at: NonZeroU32) {
        let n = &self.nodes[index(at)];
        let (parent, previous, next) = (n.parent, n.previous, n.next);
        if let Some(previous) = previous {
            self.nodes[index(previous)].next = next;
        } else if let Some(parent) = parent {
            self.nodes[index(parent)].first = next;
        }
        if let Some(next) = next {
            self.nodes[index(next)].previous = previous;
        } else if let Some(parent) = parent {
            self.nodes[index(parent)].last = previous;
        }
        let n = &mut self.nodes[index(at)];
        n.parent = None;
        n.previous = None;
        n.next = None;
    }
    pub fn detach(&mut self, id: NodeId) -> Result<bool> {
        let at = self.resolve(id)?;
        let Some(parent) = self.nodes[index(at)].parent else {
            return Ok(false);
        };
        let parent = self.handle(parent);
        self.bump_with(Change::Children(parent))?;
        self.unlink(at);
        Ok(true)
    }

    /// Preflight the complete mutation before relinking anything. A
    /// fragment is emptied and its children are inserted in order.
    pub fn insert_before(&mut self, parent: NodeId, node: NodeId, before: Option<NodeId>) -> Result<bool> {
        let parent = self.resolve(parent)?;
        let node = self.resolve(node)?;
        let before = before.map(|id| self.resolve(id)).transpose()?;
        if !matches!(self.nodes[index(parent)].data, Data::Document | Data::Fragment { .. } | Data::Element { .. } | Data::Template { .. }) || matches!(self.nodes[index(node)].data, Data::Document) {
            return Err(Error::Hierarchy);
        }
        if before.is_some_and(|id| self.nodes[index(id)].parent != Some(parent)) {
            return Err(Error::Hierarchy);
        }
        let mut ancestor = Some(parent);
        let mut depth = 0;
        while let Some(at) = ancestor {
            if at == node {
                return Err(Error::Cycle);
            }
            depth += 1;
            if depth > self.limits.depth {
                return Err(Error::DepthLimit);
            }
            ancestor = self.owned_parent(at);
        }
        if before == Some(node) || (self.nodes[index(node)].parent == Some(parent) && self.nodes[index(node)].next == before) {
            return Ok(false);
        }
        let fragment = matches!(self.nodes[index(node)].data, Data::Fragment { .. });
        let single = [node];
        let inserted: Cow<'_, [NonZeroU32]> = if fragment {
            Cow::Owned(self.children(self.handle(node))?.map(|id| link(id.slot as usize)).collect())
        } else if cfg!(feature = "baseline-storage") {
            Cow::Owned(single.to_vec())
        } else {
            Cow::Borrowed(&single)
        };
        if inserted.is_empty() {
            return Ok(false);
        }
        // Check moved subtree height iteratively, without recursive Rust
        // calls or an auxiliary stack proportional to the tree's size.
        for &root in inserted.as_ref() {
            let mut cursor = root;
            let mut relative_depth = 1;
            loop {
                if depth + relative_depth > self.limits.depth {
                    return Err(Error::DepthLimit);
                }
                if let Some(child) = self.owned_first(cursor) {
                    cursor = child;
                    relative_depth += 1;
                    continue;
                }
                while cursor != root && self.owned_next(cursor).is_none() {
                    cursor = self.owned_parent(cursor).unwrap();
                    relative_depth -= 1;
                }
                if cursor == root {
                    break;
                }
                cursor = self.owned_next(cursor).unwrap();
            }
        }
        if matches!(self.nodes[index(parent)].data, Data::Document) {
            let (mut element, mut doctype) = (false, false);
            let mut consider = |child: NonZeroU32| -> Result<()> {
                match self.nodes[index(child)].data {
                    Data::Element { .. } | Data::Template { .. } => {
                        if element {
                            return Err(Error::Hierarchy);
                        }
                        element = true;
                    }
                    Data::Doctype(_) => {
                        if element || doctype {
                            return Err(Error::Hierarchy);
                        }
                        doctype = true;
                    }
                    Data::Text(_) | Data::Document | Data::Fragment { .. } => return Err(Error::Hierarchy),
                    _ => {}
                }
                Ok(())
            };
            for child in self.children(self.handle(parent))?.map(|id| link(id.slot as usize)) {
                if before == Some(child) {
                    for &new in inserted.as_ref() {
                        consider(new)?;
                    }
                }
                if fragment || child != node {
                    consider(child)?;
                }
            }
            if before.is_none() {
                for &new in inserted.as_ref() {
                    consider(new)?;
                }
            }
        } else if inserted.iter().any(|id| matches!(self.nodes[index(*id)].data, Data::Doctype(_))) {
            return Err(Error::Hierarchy);
        }
        let parent_id = self.handle(parent);
        self.bump_with(Change::Children(parent_id))?;
        for &child in inserted.as_ref() {
            if let Some(old) = self.nodes[index(child)].parent
                && old != parent
            {
                let old = self.handle(old);
                self.changes.push((self.revision, Change::Children(old)));
            }
            self.unlink(child);
            let previous = if let Some(before) = before { self.nodes[index(before)].previous } else { self.nodes[index(parent)].last };
            let n = &mut self.nodes[index(child)];
            n.parent = Some(parent);
            n.previous = previous;
            n.next = before;
            if let Some(previous) = previous {
                self.nodes[index(previous)].next = Some(child);
            } else {
                self.nodes[index(parent)].first = Some(child);
            }
            if let Some(before) = before {
                self.nodes[index(before)].previous = Some(child);
            } else {
                self.nodes[index(parent)].last = Some(child);
            }
        }
        Ok(true)
    }
    pub fn append_child(&mut self, parent: NodeId, node: NodeId) -> Result<bool> {
        self.insert_before(parent, node, None)
    }

    fn copy_text(&mut self, value: TextValue) -> Result<TextValue> {
        if value.len < u32::MAX - 1 {
            return Ok(value);
        }
        if self.text(value).len() > self.limits.owned_text_bytes.saturating_sub(self.owned_bytes) {
            return Err(Error::TextLimit);
        }
        let copied = self.text(value).to_owned();
        self.store_text(Cow::Owned(copied), 0)
    }

    fn clone_shallow(&mut self, id: NodeId) -> Result<NodeId> {
        let at = self.resolve(id)?;
        self.can_allocate()?;
        let data = match self.nodes[index(at)].data {
            Data::Element { name, namespace, .. } => Data::Element { name, namespace, attributes: None },
            Data::Text(v) => Data::Text(self.copy_text(v)?),
            Data::Comment(v) => Data::Comment(self.copy_text(v)?),
            Data::ProcessingInstruction { target, data } => Data::ProcessingInstruction { target, data: self.copy_text(data)? },
            Data::Fragment { .. } => Data::Fragment { host: None },
            Data::Template { .. } => return self.clone_template_shallow(id),
            Data::Doctype(id) => {
                let (name, public, system) = {
                    let d = self.doctypes.get(id);
                    (d.name, d.public_id, d.system_id)
                };
                let public_id = self.copy_text(public)?;
                let system_id = match self.copy_text(system) {
                    Ok(v) => v,
                    Err(e) => {
                        self.free_text(public_id);
                        return Err(e);
                    }
                };
                Data::Doctype(self.doctypes.insert(DoctypeData { name, public_id, system_id }))
            }
            Data::Document | Data::Vacant(_) => return Err(Error::Hierarchy),
        };
        let copied = self.allocate(data);
        self.copy_attributes(at, copied)?;
        Ok(copied)
    }
    fn clone_template_shallow(&mut self, id: NodeId) -> Result<NodeId> {
        let copied = self.create_template()?;
        self.copy_attributes(link(id.slot as usize), copied)?;
        Ok(copied)
    }
    fn copy_attributes(&mut self, at: NonZeroU32, copied: NodeId) -> Result<()> {
        if let Data::Element { attributes, .. } | Data::Template { attributes, .. } = self.nodes[index(at)].data {
            let result = (|| -> Result<()> {
                let mut next = attributes;
                let mut last = None;
                while let Some(attr) = next {
                    if self.attributes.live >= self.limits.attributes {
                        return Err(Error::AttributeLimit);
                    }
                    let (name, value, following) = {
                        let a = self.attributes.get(attr);
                        (a.name, a.value, a.next)
                    };
                    let value = self.copy_text(value)?;
                    let new = self.attributes.insert(Attribute { name, value, next: None });
                    if let Some(last) = last {
                        self.attributes.get_mut(last).next = Some(new);
                    } else {
                        let (Data::Element { attributes, .. } | Data::Template { attributes, .. }) = &mut self.nodes[copied.slot as usize].data else { unreachable!() };
                        *attributes = Some(new);
                    }
                    last = Some(new);
                    next = following;
                }
                Ok(())
            })();
            if let Err(e) = result {
                self.destroy_subtree(copied)?;
                return Err(e);
            }
        }
        Ok(())
    }

    /// Clone into this document, preserving source ranges and attribute
    /// order. Character-data buffers are independent after cloning. A
    /// Document itself needs a new document identity and is not accepted.
    pub fn clone_subtree(&mut self, id: NodeId) -> Result<NodeId> {
        self.clone_subtree_cancellable(id, None)
    }

    pub fn clone_subtree_cancellable(&mut self, id: NodeId, cancelled: Option<&std::sync::atomic::AtomicBool>) -> Result<NodeId> {
        if cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(Error::Cancelled);
        }
        let root = self.clone_shallow(id)?;
        let result = (|| -> Result<()> {
            let mut frames = vec![(self.owned_first(link(id.slot as usize)), root)];
            while let Some(frame) = frames.last_mut() {
                if cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) {
                    return Err(Error::Cancelled);
                }
                let Some(child) = frame.0 else {
                    frames.pop();
                    continue;
                };
                frame.0 = self.owned_next(child);
                let parent = frame.1;
                let copied = if matches!(self.nodes[index(child)].data, Data::Fragment { host: Some(_) }) {
                    self.template_contents(parent)?.expect("cloned template's content root")
                } else {
                    let copied = self.clone_shallow(self.handle(child))?;
                    if let Err(e) = self.append_child(parent, copied) {
                        self.destroy_subtree(copied)?;
                        return Err(e);
                    }
                    copied
                };
                frames.push((self.owned_first(child), copied));
            }
            Ok(())
        })();
        if let Err(e) = result {
            self.destroy_subtree(root)?;
            return Err(e);
        }
        Ok(root)
    }

    /// Explicit reclamation for unreachable nodes. DOM removeChild only
    /// detaches: JS/V8 may still root a detached node. Call destruction
    /// only once no host/JS/agent operation is retaining the subtree.
    pub fn destroy_subtree(&mut self, id: NodeId) -> Result<usize> {
        let root = self.resolve(id)?;
        if matches!(self.nodes[index(root)].data, Data::Document | Data::Fragment { host: Some(_) }) {
            return Err(Error::Hierarchy);
        }
        match self.nodes[index(root)].parent {
            Some(parent) => {
                let parent = self.handle(parent);
                self.bump_with(Change::Children(parent))?;
            }
            None => self.bump()?,
        }
        self.unlink(root);
        let mut cursor = root;
        let mut removed = 0;
        loop {
            if let Data::Template { contents, .. } = &mut self.nodes[index(cursor)].data
                && let Some(fragment) = contents.take()
            {
                cursor = fragment;
                continue;
            }
            if let Some(child) = self.nodes[index(cursor)].first.take() {
                cursor = child;
                continue;
            }
            let (next, parent) = {
                let n = &self.nodes[index(cursor)];
                (n.next, self.owned_parent(cursor))
            };
            let data = std::mem::replace(&mut self.nodes[index(cursor)].data, Data::Vacant(None));
            match data {
                Data::Doctype(id) => {
                    let d = self.doctypes.remove(id);
                    self.free_text(d.public_id);
                    self.free_text(d.system_id);
                }
                Data::Text(v) | Data::Comment(v) | Data::ProcessingInstruction { data: v, .. } => self.free_text(v),
                Data::Element { attributes, .. } | Data::Template { attributes, .. } => {
                    let mut next = attributes;
                    while let Some(attr) = next {
                        let a = self.attributes.remove(attr);
                        next = a.next;
                        self.free_text(a.value);
                    }
                }
                _ => {}
            }
            let n = &mut self.nodes[index(cursor)];
            // Never wrap a generation and accidentally revive an old ID.
            if let Some(generation) = n.generation.checked_add(1) {
                *n = Node::new(Data::Vacant(self.free_nodes), generation);
                self.free_nodes = Some(cursor);
            }
            self.live_nodes -= 1;
            removed += 1;
            if cursor == root {
                break;
            }
            cursor = next.or(parent).expect("non-root subtree node");
        }
        Ok(removed)
    }
}

pub struct Children<'a> {
    document: &'a Document,
    next: Link,
}
impl Iterator for Children<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<Self::Item> {
        let at = self.next?;
        self.next = self.document.nodes[index(at)].next;
        Some(self.document.handle(at))
    }
}

pub struct Attributes<'a> {
    document: &'a Document,
    next: Link,
}
impl<'a> Iterator for Attributes<'a> {
    type Item = (&'a str, &'a str);
    fn next(&mut self) -> Option<Self::Item> {
        let at = self.next?;
        let a = self.document.attributes.get(at);
        self.next = a.next;
        Some((&self.document.names[index(a.name.atom())], self.document.text(a.value)))
    }
}

pub struct NamespacedAttributes<'a> {
    document: &'a Document,
    next: Link,
}
impl<'a> Iterator for NamespacedAttributes<'a> {
    type Item = AttributeRef<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        let at = self.next?;
        let a = self.document.attributes.get(at);
        self.next = a.next;
        let qualified_name = self.document.names[index(a.name.atom())].as_ref();
        let namespace = a.name.namespace();
        let (prefix, local_name) = if namespace.is_some() { qualified_name.split_once(':').map_or((None, qualified_name), |(p, n)| (Some(p), n)) } else { (None, qualified_name) };
        Some(AttributeRef { qualified_name, local_name, prefix, namespace, value: self.document.text(a.value) })
    }
}
