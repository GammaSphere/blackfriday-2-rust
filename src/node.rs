//! The syntax tree, ported from upstream `node.go`.
//!
//! # Arena instead of pointers
//!
//! Go's `Node` is a mutable cyclic graph — every node points at its parent,
//! first and last child, and both siblings:
//!
//! ```go
//! type Node struct {
//!     Parent, FirstChild, LastChild, Prev, Next *Node
//!     ...
//! }
//! ```
//!
//! Expressing that directly in Rust means `Rc<RefCell<Node>>` with `Weak`
//! back-edges: reference-count traffic on every traversal step, runtime borrow
//! panics as a new failure mode the Go original does not have, and `Weak`
//! upgrades scattered through the parser. Instead every node lives in an
//! [`Arena`] (`Vec<Node>`) and the links are [`NodeId`] indices. `NodeId` is a
//! `Copy` newtype over `usize`, so it behaves like the Go pointer it replaces
//! while owning nothing.
//!
//! The arena only ever grows during a parse, so an id handed out early stays
//! valid no matter how many nodes are added later. Nodes are never freed
//! individually; [`Arena::unlink`] detaches a node from the tree but leaves its
//! storage in place, exactly as Go leaves an unlinked node for the GC.
//!
//! # Traversal has to tolerate mutation
//!
//! The obvious Rust walker takes `&Arena` and a closure. That does not work
//! here, because upstream mutates the tree *during* a walk — `markdown.go:410`
//! parses inline content into fresh child nodes and clears `node.content` from
//! inside the visitor:
//!
//! ```go
//! p.doc.Walk(func(node *Node, entering bool) WalkStatus {
//!     if node.Type == Paragraph || node.Type == Heading || node.Type == TableCell {
//!         p.inline(node, node.content)   // appends children
//!         node.content = nil             // mutates this node
//!     }
//!     return GoToNext
//! })
//! ```
//!
//! A closure holding `&Arena` could not call `p.inline`, which needs
//! `&mut Arena`. So [`Walker`] is a *borrow-free cursor*: it stores only a
//! `NodeId` and an `entering` flag — precisely what Go's `nodeWalker` stores —
//! and borrows the arena for the duration of a single [`Walker::advance`] call.
//! Between steps the caller holds no borrow at all and may mutate freely.
//!
//! Note that this faithfully reproduces Go's behaviour of descending into
//! children created by the visitor: after the `Paragraph` visit above, the
//! walker steps to `FirstChild`, which is now one of the newly parsed inline
//! nodes. That is intentional and load-bearing.
//!
//! [`Arena::walk`] is the read-only convenience form for callers that do not
//! mutate — the HTML renderer, mainly.
//!
//! # nil versus empty
//!
//! Go distinguishes a nil `[]byte` from an empty one, and upstream depends on
//! it in two places: `html.go:613` tests `node.LinkData.Title != nil`, and
//! `html.go:747`/`754` test `node.ListData.RefLink != nil`. Those two fields
//! are therefore `Option<Vec<u8>>`. Every other byte field is only ever
//! length-checked, so a plain `Vec<u8>` is indistinguishable and is used
//! instead. If a divergence ever traces back to an empty-versus-absent
//! confusion, these are the first fields to re-examine.
//!
//! # Node data layout
//!
//! Go embeds all five data structs into every `Node` regardless of its type, so
//! reading `node.Level` on a `Text` node yields a zero value rather than an
//! error, and the parser relies on that. A Rust `enum` payload would be more
//! idiomatic but would turn those reads into a different program. The structs
//! are kept present on every node, merely grouped under named fields
//! (`node.heading.level`) instead of flattened into the node's namespace.

use crate::flags::{CellAlignFlags, ListType};
use std::fmt;
use std::ops::{Index, IndexMut};

/// Identifies a single node within an [`Arena`].
///
/// Replaces the `*Node` of the Go original. Cheap to copy and pass around; it
/// carries no borrow, which is what lets [`Walker`] survive mutation of the
/// tree it is walking.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NodeId(usize);

impl NodeId {
    /// The underlying index. Useful for diagnostics and stable ordering.
    #[inline]
    pub const fn index(self) -> usize {
        self.0
    }
}

/// The type of a single syntax-tree node.
///
/// Discriminants match the Go `iota` ordering exactly (`Document == 0` through
/// `TableRow == 23`), so the numeric value can cross the FFI boundary unchanged.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
#[repr(i32)]
pub enum NodeType {
    /// The root of every tree.
    #[default]
    Document = 0,
    /// A block quote.
    BlockQuote = 1,
    /// An ordered, unordered or definition list.
    List = 2,
    /// A single list item.
    Item = 3,
    /// A paragraph.
    Paragraph = 4,
    /// A heading, ATX or setext.
    Heading = 5,
    /// A horizontal rule.
    HorizontalRule = 6,
    /// Emphasis, rendered as `<em>`.
    Emph = 7,
    /// Strong emphasis, rendered as `<strong>`.
    Strong = 8,
    /// Struck-through text, rendered as `<del>`.
    Del = 9,
    /// A link.
    Link = 10,
    /// An image.
    Image = 11,
    /// A run of literal text.
    Text = 12,
    /// A block of raw HTML.
    HTMLBlock = 13,
    /// A code block, fenced or indented.
    CodeBlock = 14,
    /// A soft line break.
    Softbreak = 15,
    /// A hard line break.
    Hardbreak = 16,
    /// An inline code span.
    Code = 17,
    /// A span of raw inline HTML.
    HTMLSpan = 18,
    /// A table.
    Table = 19,
    /// A single table cell.
    TableCell = 20,
    /// A table header section.
    TableHead = 21,
    /// A table body section.
    TableBody = 22,
    /// A single table row.
    TableRow = 23,
}

impl NodeType {
    /// The name Go's `NodeType.String()` produces.
    pub const fn name(self) -> &'static str {
        match self {
            NodeType::Document => "Document",
            NodeType::BlockQuote => "BlockQuote",
            NodeType::List => "List",
            NodeType::Item => "Item",
            NodeType::Paragraph => "Paragraph",
            NodeType::Heading => "Heading",
            NodeType::HorizontalRule => "HorizontalRule",
            NodeType::Emph => "Emph",
            NodeType::Strong => "Strong",
            NodeType::Del => "Del",
            NodeType::Link => "Link",
            NodeType::Image => "Image",
            NodeType::Text => "Text",
            NodeType::HTMLBlock => "HTMLBlock",
            NodeType::CodeBlock => "CodeBlock",
            NodeType::Softbreak => "Softbreak",
            NodeType::Hardbreak => "Hardbreak",
            NodeType::Code => "Code",
            NodeType::HTMLSpan => "HTMLSpan",
            NodeType::Table => "Table",
            NodeType::TableCell => "TableCell",
            NodeType::TableHead => "TableHead",
            NodeType::TableBody => "TableBody",
            NodeType::TableRow => "TableRow",
        }
    }

    /// True when this type may contain children.
    ///
    /// Ported verbatim from Go's `IsContainer`, including the fact that
    /// `CodeBlock`, `HTMLBlock`, `Text`, `Code`, `HTMLSpan`, `HorizontalRule`,
    /// `Softbreak` and `Hardbreak` are leaves.
    pub const fn is_container(self) -> bool {
        matches!(
            self,
            NodeType::Document
                | NodeType::BlockQuote
                | NodeType::List
                | NodeType::Item
                | NodeType::Paragraph
                | NodeType::Heading
                | NodeType::Emph
                | NodeType::Strong
                | NodeType::Del
                | NodeType::Link
                | NodeType::Image
                | NodeType::Table
                | NodeType::TableHead
                | NodeType::TableBody
                | NodeType::TableRow
                | NodeType::TableCell
        )
    }

    /// The inverse of [`NodeType::is_container`].
    pub const fn is_leaf(self) -> bool {
        !self.is_container()
    }

    /// Whether a node of this type may contain a child of type `t`.
    ///
    /// Ported from Go's unexported `canContain`.
    pub const fn can_contain(self, t: NodeType) -> bool {
        match self {
            NodeType::List => matches!(t, NodeType::Item),
            NodeType::Document | NodeType::BlockQuote | NodeType::Item => {
                !matches!(t, NodeType::Item)
            }
            NodeType::Table => matches!(t, NodeType::TableHead | NodeType::TableBody),
            NodeType::TableHead | NodeType::TableBody => matches!(t, NodeType::TableRow),
            NodeType::TableRow => matches!(t, NodeType::TableCell),
            _ => false,
        }
    }
}

impl fmt::Display for NodeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Fields relevant to [`NodeType::List`] and [`NodeType::Item`].
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ListData {
    /// Combination of [`ListType`] flags.
    pub list_flags: ListType,
    /// Skip the `<p>` wrapper around item content when true.
    pub tight: bool,
    /// `*`, `+` or `-` in bullet lists.
    pub bullet_char: u8,
    /// `.` or `)` after the number in ordered lists.
    pub delimiter: u8,
    /// When present, turns this item into a footnote item and changes
    /// rendering. Absence is meaningful — see the module docs.
    pub ref_link: Option<Vec<u8>>,
    /// True when this list holds footnotes.
    pub is_footnotes_list: bool,
}

/// Fields relevant to [`NodeType::Link`].
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct LinkData {
    /// What goes into `href`.
    pub destination: Vec<u8>,
    /// What goes into `title`. Absence is meaningful — see the module docs.
    pub title: Option<Vec<u8>>,
    /// Serial number of a footnote, zero when this is not one.
    pub note_id: i32,
    /// The footnote node, when this link refers to one.
    pub footnote: Option<NodeId>,
}

/// Fields relevant to [`NodeType::CodeBlock`].
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct CodeBlockData {
    /// Fenced rather than indented.
    pub is_fenced: bool,
    /// The info string following the opening fence.
    pub info: Vec<u8>,
    /// The character the fence is built from.
    pub fence_char: u8,
    /// How many fence characters the opening fence used.
    pub fence_length: usize,
    /// Offset of the fence within its line.
    pub fence_offset: usize,
}

/// Fields relevant to [`NodeType::TableCell`].
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct TableCellData {
    /// True when the cell sits in the header row.
    pub is_header: bool,
    /// Value for the `align` attribute.
    pub align: CellAlignFlags,
}

/// Fields relevant to [`NodeType::Heading`].
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct HeadingData {
    /// Heading level, 1 through 6.
    pub level: i32,
    /// Explicit or generated heading ID, when there is one.
    /// Bytes, not a `String`: Go's `HeadingID` is a `string` filled from
    /// `{#id}` in the document, which need not be valid UTF-8, and going
    /// through `from_utf8_lossy` would rewrite those bytes as U+FFFD.
    pub heading_id: Vec<u8>,
    /// True when this is a title block rather than a heading.
    pub is_titleblock: bool,
}

/// A single element of the syntax tree.
///
/// Links to neighbouring nodes are [`NodeId`]s into the owning [`Arena`] rather
/// than pointers; see the module docs.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Node {
    /// What kind of node this is.
    pub node_type: NodeType,

    parent: Option<NodeId>,
    first_child: Option<NodeId>,
    last_child: Option<NodeId>,
    prev: Option<NodeId>,
    next: Option<NodeId>,

    /// Text contents of leaf nodes.
    pub literal: Vec<u8>,

    /// Populated when `node_type` is [`NodeType::Heading`].
    pub heading: HeadingData,
    /// Populated when `node_type` is [`NodeType::List`] or [`NodeType::Item`].
    pub list: ListData,
    /// Populated when `node_type` is [`NodeType::CodeBlock`].
    pub code_block: CodeBlockData,
    /// Populated when `node_type` is [`NodeType::Link`].
    pub link: LinkData,
    /// Populated when `node_type` is [`NodeType::TableCell`].
    pub table_cell: TableCellData,

    /// Raw markdown of a block node, consumed by the inline pass.
    pub(crate) content: Vec<u8>,
    /// An open block that has not finished parsing yet.
    pub(crate) open: bool,
}

impl Node {
    fn new(node_type: NodeType) -> Self {
        Node {
            node_type,
            parent: None,
            first_child: None,
            last_child: None,
            prev: None,
            next: None,
            literal: Vec::new(),
            heading: HeadingData::default(),
            list: ListData::default(),
            code_block: CodeBlockData::default(),
            link: LinkData::default(),
            table_cell: TableCellData::default(),
            content: Vec::new(),
            open: true,
        }
    }

    /// The parent node, if any.
    #[inline]
    pub const fn parent(&self) -> Option<NodeId> {
        self.parent
    }
    /// The first child, if any.
    #[inline]
    pub const fn first_child(&self) -> Option<NodeId> {
        self.first_child
    }
    /// The last child, if any.
    #[inline]
    pub const fn last_child(&self) -> Option<NodeId> {
        self.last_child
    }
    /// The previous sibling, if any.
    #[inline]
    pub const fn prev(&self) -> Option<NodeId> {
        self.prev
    }
    /// The next sibling, if any.
    #[inline]
    pub const fn next(&self) -> Option<NodeId> {
        self.next
    }

    /// True when this node may contain children.
    #[inline]
    pub const fn is_container(&self) -> bool {
        self.node_type.is_container()
    }

    /// True when this node is a leaf.
    #[inline]
    pub const fn is_leaf(&self) -> bool {
        self.node_type.is_leaf()
    }
}

impl fmt::Display for Node {
    /// Matches Go's `Node.String()`: `Type: 'literal'`, truncating the literal
    /// past 16 bytes and appending an ellipsis.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (snippet, ellipsis) = if self.literal.len() > 16 {
            (&self.literal[..16], "...")
        } else {
            (&self.literal[..], "")
        };
        write!(
            f,
            "{}: '{}{}'",
            self.node_type,
            String::from_utf8_lossy(snippet),
            ellipsis
        )
    }
}

/// Owns every [`Node`] in a syntax tree.
///
/// Allocation is append-only: ids stay valid for the lifetime of the arena.
#[derive(Clone, Debug, Default)]
pub struct Arena {
    nodes: Vec<Node>,
}

impl Arena {
    /// A new, empty arena.
    pub fn new() -> Self {
        Arena { nodes: Vec::new() }
    }

    /// A new arena with room for `capacity` nodes.
    pub fn with_capacity(capacity: usize) -> Self {
        Arena {
            nodes: Vec::with_capacity(capacity),
        }
    }

    /// Reserves room for at least `additional` more nodes.
    ///
    /// Go allocates each node separately, so it never pays a growth cost; the
    /// arena trades that for locality and pays it in one lump when the backing
    /// `Vec` doubles. `Node` is a wide struct, so that lump is a large memcpy
    /// landing in the middle of a parse — visible as a worse p99 than Go's
    /// before the parser started reserving up front.
    pub fn reserve(&mut self, additional: usize) {
        self.nodes.reserve(additional);
    }

    /// Allocates a node, mirroring Go's `NewNode`.
    ///
    /// The node starts detached and open.
    pub fn new_node(&mut self, node_type: NodeType) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(Node::new(node_type));
        id
    }

    /// How many nodes have been allocated.
    #[inline]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// True when nothing has been allocated.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Borrows a node.
    #[inline]
    pub fn get(&self, id: NodeId) -> &Node {
        &self.nodes[id.0]
    }

    /// Mutably borrows a node.
    #[inline]
    pub fn get_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id.0]
    }

    /// Detaches `id` from its parent and siblings, mirroring Go's `Unlink`.
    ///
    /// The node's storage is retained; only the links change.
    pub fn unlink(&mut self, id: NodeId) {
        let (parent, prev, next) = {
            let n = self.get(id);
            (n.parent, n.prev, n.next)
        };

        match prev {
            Some(p) => self.get_mut(p).next = next,
            None => {
                if let Some(p) = parent {
                    self.get_mut(p).first_child = next;
                }
            }
        }
        match next {
            Some(n) => self.get_mut(n).prev = prev,
            None => {
                if let Some(p) = parent {
                    self.get_mut(p).last_child = prev;
                }
            }
        }

        let n = self.get_mut(id);
        n.parent = None;
        n.next = None;
        n.prev = None;
    }

    /// Appends `child` to `parent`, mirroring Go's `AppendChild`.
    ///
    /// `child` is unlinked from any current position first, exactly as Go does.
    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        self.unlink(child);
        self.get_mut(child).parent = Some(parent);
        match self.get(parent).last_child {
            Some(last) => {
                self.get_mut(last).next = Some(child);
                self.get_mut(child).prev = Some(last);
                self.get_mut(parent).last_child = Some(child);
            }
            None => {
                self.get_mut(parent).first_child = Some(child);
                self.get_mut(parent).last_child = Some(child);
            }
        }
    }

    /// Inserts `sibling` immediately before `node`, mirroring Go's
    /// `InsertBefore`.
    ///
    /// Upstream never calls this — it is exported API with no internal user —
    /// but it is ported for surface completeness. Like the Go version it
    /// assumes `node` has a parent when `node` is a first child.
    pub fn insert_before(&mut self, node: NodeId, sibling: NodeId) {
        self.unlink(sibling);
        let prev = self.get(node).prev;
        self.get_mut(sibling).prev = prev;
        if let Some(p) = prev {
            self.get_mut(p).next = Some(sibling);
        }
        self.get_mut(sibling).next = Some(node);
        self.get_mut(node).prev = Some(sibling);
        let parent = self.get(node).parent;
        self.get_mut(sibling).parent = parent;
        if prev.is_none() {
            if let Some(p) = parent {
                self.get_mut(p).first_child = Some(sibling);
            }
        }
    }

    /// Iterates the children of `id` in order.
    pub fn children(&self, id: NodeId) -> Children<'_> {
        Children {
            arena: self,
            next: self.get(id).first_child,
        }
    }

    /// Walks the subtree rooted at `root`, read-only.
    ///
    /// Equivalent to Go's `Node.Walk`. The visitor is called twice for every
    /// container node — once entering, once leaving — and once for every leaf.
    ///
    /// Callers that need to mutate the tree while walking must drive a
    /// [`Walker`] directly; see the module docs.
    pub fn walk<F>(&self, root: NodeId, mut visitor: F)
    where
        F: FnMut(&Arena, NodeId, bool) -> WalkStatus,
    {
        let mut w = Walker::new(root);
        while let Some((id, entering)) = w.current() {
            match visitor(self, id, entering) {
                WalkStatus::Terminate => return,
                status => w.advance(self, status),
            }
        }
    }

    /// Renders the subtree as Go's unexported `dumpString` would.
    ///
    /// Only useful for diagnostics and differential debugging.
    pub fn dump(&self, root: NodeId) -> String {
        fn go(arena: &Arena, id: NodeId, depth: usize, out: &mut String) {
            let n = arena.get(id);
            let content: &[u8] = if n.literal.is_empty() {
                &n.content
            } else {
                &n.literal
            };
            out.push_str(&"\t".repeat(depth));
            out.push_str(&format!(
                "{}({:?})\n",
                n.node_type,
                String::from_utf8_lossy(content)
            ));
            let mut child = n.first_child;
            while let Some(c) = child {
                go(arena, c, depth + 1, out);
                child = arena.get(c).next;
            }
        }
        let mut out = String::new();
        go(self, root, 0, &mut out);
        out
    }
}

impl Index<NodeId> for Arena {
    type Output = Node;
    #[inline]
    fn index(&self, id: NodeId) -> &Node {
        self.get(id)
    }
}

impl IndexMut<NodeId> for Arena {
    #[inline]
    fn index_mut(&mut self, id: NodeId) -> &mut Node {
        self.get_mut(id)
    }
}

/// Iterator over a node's children, produced by [`Arena::children`].
pub struct Children<'a> {
    arena: &'a Arena,
    next: Option<NodeId>,
}

impl Iterator for Children<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        let cur = self.next?;
        self.next = self.arena.get(cur).next;
        Some(cur)
    }
}

/// Controls how [`Walker`] proceeds after visiting a node.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[repr(i32)]
pub enum WalkStatus {
    /// Continue to the next node.
    #[default]
    GoToNext = 0,
    /// Skip the current node's children.
    SkipChildren = 1,
    /// Stop the traversal.
    Terminate = 2,
}

/// A cursor over a syntax tree.
///
/// Holds no borrow on the [`Arena`] — only a [`NodeId`] and a direction flag,
/// mirroring Go's `nodeWalker`. That is what allows the tree to be mutated
/// between steps, which upstream's inline-parsing pass requires.
///
/// ```
/// # use blackfriday::node::{Arena, NodeType, WalkStatus, Walker};
/// let mut arena = Arena::new();
/// let doc = arena.new_node(NodeType::Document);
/// let para = arena.new_node(NodeType::Paragraph);
/// arena.append_child(doc, para);
///
/// let mut w = Walker::new(doc);
/// let mut visits = Vec::new();
/// while let Some((id, entering)) = w.current() {
///     visits.push((arena[id].node_type, entering));
///     w.advance(&arena, WalkStatus::GoToNext);
/// }
/// assert_eq!(visits.len(), 4); // enter/leave for each container
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Walker {
    current: Option<NodeId>,
    root: NodeId,
    entering: bool,
}

impl Walker {
    /// Starts a traversal at `root`, entering.
    pub const fn new(root: NodeId) -> Self {
        Walker {
            current: Some(root),
            root,
            entering: true,
        }
    }

    /// The node about to be visited and whether this is the entering visit.
    ///
    /// `None` once the traversal is finished.
    #[inline]
    pub const fn current(&self) -> Option<(NodeId, bool)> {
        match self.current {
            Some(id) => Some((id, self.entering)),
            None => None,
        }
    }

    /// Advances past the current node.
    ///
    /// [`WalkStatus::Terminate`] ends the traversal. The arena is borrowed only
    /// for the duration of this call.
    pub fn advance(&mut self, arena: &Arena, status: WalkStatus) {
        match status {
            WalkStatus::Terminate => {
                self.current = None;
                return;
            }
            WalkStatus::SkipChildren => self.entering = false,
            WalkStatus::GoToNext => {}
        }
        self.step(arena);
    }

    /// Ported verbatim from Go's `nodeWalker.next`.
    fn step(&mut self, arena: &Arena) {
        let Some(cur) = self.current else { return };
        let node = arena.get(cur);

        if (!node.is_container() || !self.entering) && cur == self.root {
            self.current = None;
            return;
        }

        if self.entering && node.is_container() {
            match node.first_child {
                Some(child) => {
                    self.current = Some(child);
                    self.entering = true;
                }
                None => self.entering = false,
            }
        } else if node.next.is_none() {
            self.current = node.parent;
            self.entering = false;
        } else {
            self.current = node.next;
            self.entering = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> (Arena, NodeId, Vec<NodeId>) {
        let mut a = Arena::new();
        let doc = a.new_node(NodeType::Document);
        let p1 = a.new_node(NodeType::Paragraph);
        let t1 = a.new_node(NodeType::Text);
        let p2 = a.new_node(NodeType::Paragraph);
        let t2 = a.new_node(NodeType::Text);
        a.append_child(doc, p1);
        a.append_child(p1, t1);
        a.append_child(doc, p2);
        a.append_child(p2, t2);
        (a, doc, vec![p1, t1, p2, t2])
    }

    #[test]
    fn node_type_discriminants_match_go_iota() {
        assert_eq!(NodeType::Document as i32, 0);
        assert_eq!(NodeType::Heading as i32, 5);
        assert_eq!(NodeType::Text as i32, 12);
        assert_eq!(NodeType::Code as i32, 17);
        assert_eq!(NodeType::TableRow as i32, 23);
        assert_eq!(NodeType::Text.name(), "Text");
        assert_eq!(NodeType::HTMLBlock.to_string(), "HTMLBlock");
    }

    #[test]
    fn container_classification_matches_go() {
        for t in [
            NodeType::Document,
            NodeType::BlockQuote,
            NodeType::List,
            NodeType::Item,
            NodeType::Paragraph,
            NodeType::Heading,
            NodeType::Emph,
            NodeType::Strong,
            NodeType::Del,
            NodeType::Link,
            NodeType::Image,
            NodeType::Table,
            NodeType::TableHead,
            NodeType::TableBody,
            NodeType::TableRow,
            NodeType::TableCell,
        ] {
            assert!(t.is_container(), "{t} should be a container");
        }
        for t in [
            NodeType::HorizontalRule,
            NodeType::Text,
            NodeType::HTMLBlock,
            NodeType::CodeBlock,
            NodeType::Softbreak,
            NodeType::Hardbreak,
            NodeType::Code,
            NodeType::HTMLSpan,
        ] {
            assert!(t.is_leaf(), "{t} should be a leaf");
        }
    }

    #[test]
    fn can_contain_matches_go() {
        assert!(NodeType::List.can_contain(NodeType::Item));
        assert!(!NodeType::List.can_contain(NodeType::Paragraph));
        assert!(NodeType::Document.can_contain(NodeType::Paragraph));
        assert!(!NodeType::Document.can_contain(NodeType::Item));
        assert!(NodeType::Item.can_contain(NodeType::List));
        assert!(NodeType::Table.can_contain(NodeType::TableHead));
        assert!(!NodeType::Table.can_contain(NodeType::TableRow));
        assert!(NodeType::TableHead.can_contain(NodeType::TableRow));
        assert!(NodeType::TableRow.can_contain(NodeType::TableCell));
        assert!(!NodeType::Paragraph.can_contain(NodeType::Text));
    }

    #[test]
    fn append_child_builds_sibling_links() {
        let (a, doc, ids) = tree();
        let [p1, t1, p2, _t2] = ids[..] else {
            unreachable!()
        };
        assert_eq!(a[doc].first_child(), Some(p1));
        assert_eq!(a[doc].last_child(), Some(p2));
        assert_eq!(a[p1].next(), Some(p2));
        assert_eq!(a[p2].prev(), Some(p1));
        assert_eq!(a[p1].parent(), Some(doc));
        assert_eq!(a[t1].parent(), Some(p1));
        assert_eq!(a[p1].next().and_then(|n| a[n].next()), None);
        assert_eq!(a.children(doc).collect::<Vec<_>>(), vec![p1, p2]);
    }

    #[test]
    fn append_child_relocates_an_attached_node() {
        // Go's AppendChild calls Unlink first, so moving a node is legal.
        let (mut a, doc, ids) = tree();
        let [p1, t1, p2, t2] = ids[..] else {
            unreachable!()
        };
        a.append_child(p2, t1);
        assert_eq!(a[p1].first_child(), None);
        assert_eq!(a[p1].last_child(), None);
        assert_eq!(a.children(p2).collect::<Vec<_>>(), vec![t2, t1]);
        assert_eq!(a[t1].parent(), Some(p2));
        assert_eq!(a[doc].first_child(), Some(p1));
    }

    #[test]
    fn unlink_repairs_both_neighbours() {
        let (mut a, doc, ids) = tree();
        let [p1, _t1, p2, _t2] = ids[..] else {
            unreachable!()
        };
        let p3 = a.new_node(NodeType::Paragraph);
        a.append_child(doc, p3);

        a.unlink(p2);
        assert_eq!(a.children(doc).collect::<Vec<_>>(), vec![p1, p3]);
        assert_eq!(a[p1].next(), Some(p3));
        assert_eq!(a[p3].prev(), Some(p1));
        assert_eq!(a[p2].parent(), None);
        assert_eq!(a[p2].next(), None);
        assert_eq!(a[p2].prev(), None);

        // Unlinking the first child moves the parent's head.
        a.unlink(p1);
        assert_eq!(a[doc].first_child(), Some(p3));
        // Unlinking the last child moves the parent's tail.
        a.unlink(p3);
        assert_eq!(a[doc].first_child(), None);
        assert_eq!(a[doc].last_child(), None);
    }

    #[test]
    fn insert_before_splices_in_place() {
        let (mut a, doc, ids) = tree();
        let [p1, _t1, p2, _t2] = ids[..] else {
            unreachable!()
        };
        let mid = a.new_node(NodeType::HorizontalRule);
        a.insert_before(p2, mid);
        assert_eq!(a.children(doc).collect::<Vec<_>>(), vec![p1, mid, p2]);

        let head = a.new_node(NodeType::HorizontalRule);
        a.insert_before(p1, head);
        assert_eq!(a[doc].first_child(), Some(head));
        assert_eq!(a.children(doc).collect::<Vec<_>>(), vec![head, p1, mid, p2]);
    }

    #[test]
    fn walk_visits_containers_twice_and_leaves_once() {
        let (a, doc, _) = tree();
        let mut seen = Vec::new();
        a.walk(doc, |arena, id, entering| {
            seen.push((arena[id].node_type, entering));
            WalkStatus::GoToNext
        });
        assert_eq!(
            seen,
            vec![
                (NodeType::Document, true),
                (NodeType::Paragraph, true),
                (NodeType::Text, true),
                (NodeType::Paragraph, false),
                (NodeType::Paragraph, true),
                (NodeType::Text, true),
                (NodeType::Paragraph, false),
                (NodeType::Document, false),
            ]
        );
    }

    #[test]
    fn walk_of_a_lone_leaf_visits_once() {
        let mut a = Arena::new();
        let t = a.new_node(NodeType::Text);
        let mut seen = 0;
        a.walk(t, |_, _, _| {
            seen += 1;
            WalkStatus::GoToNext
        });
        assert_eq!(seen, 1);
    }

    #[test]
    fn skip_children_omits_the_subtree() {
        let (a, doc, _) = tree();
        let mut seen = Vec::new();
        a.walk(doc, |arena, id, entering| {
            seen.push((arena[id].node_type, entering));
            if arena[id].node_type == NodeType::Paragraph && entering {
                WalkStatus::SkipChildren
            } else {
                WalkStatus::GoToNext
            }
        });
        assert_eq!(
            seen,
            vec![
                (NodeType::Document, true),
                (NodeType::Paragraph, true),
                (NodeType::Paragraph, true),
                (NodeType::Document, false),
            ]
        );
    }

    #[test]
    fn terminate_stops_immediately() {
        let (a, doc, _) = tree();
        let mut seen = 0;
        a.walk(doc, |_, _, _| {
            seen += 1;
            WalkStatus::Terminate
        });
        assert_eq!(seen, 1);
    }

    #[test]
    fn walker_tolerates_mutation_mid_traversal() {
        // This is the reason Walker is a borrow-free cursor. Upstream's inline
        // pass appends children from inside the visitor and expects the walk to
        // then descend into them (markdown.go:410).
        //
        // The expected sequence is not hand-derived: it is what Go prints for
        // the same tree and the same mutation. See tools/walkorder.
        let (mut a, doc, _) = tree();
        let mut w = Walker::new(doc);
        let mut appended = false;
        let mut seen = Vec::new();

        while let Some((id, entering)) = w.current() {
            seen.push((a[id].node_type, entering));
            if entering && a[id].node_type == NodeType::Paragraph && !appended {
                let extra = a.new_node(NodeType::Emph);
                a.append_child(id, extra);
                appended = true;
            }
            w.advance(&a, WalkStatus::GoToNext);
        }

        assert!(appended);
        assert_eq!(
            seen,
            vec![
                (NodeType::Document, true),
                (NodeType::Paragraph, true),
                (NodeType::Text, true),
                (NodeType::Emph, true),
                (NodeType::Emph, false),
                (NodeType::Paragraph, false),
                (NodeType::Paragraph, true),
                (NodeType::Text, true),
                (NodeType::Paragraph, false),
                (NodeType::Document, false),
            ],
            "walk must descend into children created by the visitor"
        );
    }

    #[test]
    fn node_display_truncates_like_go() {
        let mut a = Arena::new();
        let t = a.new_node(NodeType::Text);
        a[t].literal = b"short".to_vec();
        assert_eq!(a[t].to_string(), "Text: 'short'");

        a[t].literal = b"0123456789abcdefGHIJ".to_vec();
        assert_eq!(a[t].to_string(), "Text: '0123456789abcdef...'");

        // Exactly 16 bytes is not truncated: Go tests `len(snippet) > 16`.
        a[t].literal = b"0123456789abcdef".to_vec();
        assert_eq!(a[t].to_string(), "Text: '0123456789abcdef'");
    }

    #[test]
    fn ids_survive_arena_growth() {
        let (mut a, doc, ids) = tree();
        let p1 = ids[0];
        for _ in 0..1000 {
            a.new_node(NodeType::Text);
        }
        assert_eq!(a[doc].first_child(), Some(p1));
        assert_eq!(a[p1].parent(), Some(doc));
    }

    #[test]
    fn nil_and_empty_are_distinguishable_where_go_needs_it() {
        let mut a = Arena::new();
        let l = a.new_node(NodeType::Link);
        assert_eq!(a[l].link.title, None);
        a[l].link.title = Some(Vec::new());
        assert_eq!(a[l].link.title.as_deref(), Some(&b""[..]));
        assert!(a[l].link.title.is_some(), "empty title is not absent");

        let item = a.new_node(NodeType::Item);
        assert_eq!(a[item].list.ref_link, None);
        a[item].list.ref_link = Some(Vec::new());
        assert!(a[item].list.ref_link.is_some());
    }

    #[test]
    fn new_nodes_start_open_and_detached() {
        let mut a = Arena::new();
        let n = a.new_node(NodeType::Paragraph);
        assert!(a[n].open);
        assert_eq!(a[n].parent(), None);
        assert_eq!(a[n].first_child(), None);
    }

    #[test]
    fn dump_matches_go_shape() {
        let (mut a, doc, ids) = tree();
        a[ids[1]].literal = b"hi".to_vec();
        let out = a.dump(doc);
        assert!(out.starts_with("Document(\"\")\n"));
        assert!(out.contains("\tParagraph(\"\")\n"));
        assert!(out.contains("\t\tText(\"hi\")\n"));
    }
}
