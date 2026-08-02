//! The parser type and public entry points, ported from upstream `markdown.go`.
//!
//! # Functional options do not survive the trip
//!
//! Go configures the parser with variadic functional options:
//!
//! ```go
//! output := Run(input, WithNoExtensions(), WithExtensions(exts), WithRenderer(r))
//! ```
//!
//! `Option` is `func(*Markdown)`, and `New` applies each in turn, so later
//! options overwrite earlier ones — upstream documents that "you can use any
//! number of With* arguments, even contradicting ones."
//!
//! Rust has no variadics, so the shape cannot be transcribed. [`Options`] is a
//! builder instead: each `with_*` method consumes and returns `Self`, so the
//! same last-writer-wins ordering falls out of normal method chaining, and the
//! set of knobs is visible in one type rather than scattered across free
//! functions.
//!
//! ```
//! # use blackfriday::markdown::Options;
//! # use blackfriday::Extensions;
//! // Equivalent to Run(input, WithNoExtensions(), WithExtensions(Tables))
//! let opts = Options::none().with_extensions(Extensions::TABLES);
//! assert_eq!(opts.extensions(), Extensions::TABLES);
//! ```
//!
//! # The reference override returns two bits of information, not one
//!
//! `ReferenceOverrideFunc` is `func(string) (*Reference, bool)`, and all three
//! reachable combinations mean different things:
//!
//! | Go return | Meaning |
//! |---|---|
//! | `(nil, false)` | not overridden — run the normal lookup |
//! | `(&r, true)` | overridden to `r` |
//! | `(nil, true)` | overridden to *nothing* — the reference does not resolve |
//!
//! Collapsing that to `Option<Reference>` would merge the last two and quietly
//! turn "explicitly unresolvable" into "look it up anyway", so the port uses
//! the explicit [`RefOverride`] enum.

use crate::flags::Extensions;
use crate::node::{Arena, NodeId, NodeType, WalkStatus};

/// Maximum nesting depth for blocks and inline elements.
///
/// Upstream sets this in `New` rather than as a constant; it is named here so
/// the recursion guards in the parsers can refer to it.
pub const MAX_NESTING: usize = 16;

/// The details of a link, as seen by a [`RefOverride`] callback.
///
/// Mirrors upstream's exported `Reference`. Distinct from the parser's own
/// internal reference bookkeeping, which is not part of the public API.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Reference {
    /// Usually the URL the reference points to.
    pub link: String,
    /// Alternate text describing the link in more detail.
    pub title: String,
    /// Optional text to override the ref with, when the syntax used was
    /// `[refid][]`.
    pub text: String,
}

/// The outcome of a reference-override callback.
///
/// See the module docs for why this is an enum rather than `Option<Reference>`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RefOverride {
    /// Fall through to the parser's own reference table.
    ///
    /// Go: `(nil, false)`.
    NotOverridden,
    /// Resolve to this reference.
    ///
    /// Go: `(&r, true)`.
    To(Reference),
    /// Resolve to nothing; the reference is treated as undefined even if the
    /// document defines it.
    ///
    /// Go: `(nil, true)`.
    ToNothing,
}

/// A reference-override callback.
type RefOverrideFn = Box<dyn Fn(&str) -> RefOverride>;

/// The parser's internal record of a link reference.
///
/// Upstream's unexported `reference`. Byte-oriented because it is filled from
/// the document, and `title` distinguishes absent from empty for the same
/// reason [`crate::node`] does.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct InternalReference {
    pub(crate) link: Vec<u8>,
    pub(crate) title: Vec<u8>,
    /// Zero when this is not a footnote reference.
    pub(crate) note_id: i32,
    pub(crate) has_block: bool,
    /// The `Item` node within the footnote list, when this is a footnote.
    pub(crate) footnote: Option<NodeId>,
    /// Only populated by the reference-override feature.
    pub(crate) text: Vec<u8>,
}

/// Produces output from a parsed document.
///
/// Ported from upstream's `Renderer` interface. Writers are `Vec<u8>` rather
/// than `io::Write` for the reason given in [`crate::esc`]: upstream discards
/// every write error, so the observable contract is "append bytes, never fail".
pub trait Renderer {
    /// Renders one node.
    ///
    /// Called once for every leaf node and twice for every container — first
    /// with `entering` true, then with it false after the children are done.
    fn render_node(
        &mut self,
        out: &mut Vec<u8>,
        arena: &Arena,
        node: NodeId,
        entering: bool,
    ) -> WalkStatus;

    /// Writes any content preceding the document body.
    ///
    /// Receives the whole tree, since a renderer may need to inspect it — the
    /// HTML renderer builds its table of contents here.
    fn render_header(&mut self, out: &mut Vec<u8>, arena: &Arena, ast: NodeId);

    /// Writes any content following the document body.
    fn render_footer(&mut self, out: &mut Vec<u8>, arena: &Arena, ast: NodeId);
}

/// Parser configuration.
///
/// Built by chaining; later calls win, matching Go's option ordering.
#[derive(Default)]
pub struct Options {
    extensions: Extensions,
    reference_override: Option<RefOverrideFn>,
}

impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options")
            .field("extensions", &self.extensions)
            .field(
                "reference_override",
                &self.reference_override.as_ref().map(|_| "<fn>"),
            )
            .finish()
    }
}

impl Options {
    /// The defaults the top-level `run` entry point uses:
    /// [`Extensions::COMMON`].
    pub fn common() -> Self {
        Options {
            extensions: Extensions::COMMON,
            reference_override: None,
        }
    }

    /// All extensions off.
    ///
    /// Corresponds to Go's `WithNoExtensions`. Note that Go's version also
    /// swaps in a renderer built with `HTMLFlagsNone` as a side effect; here
    /// the renderer is chosen separately, so this only clears extensions.
    pub fn none() -> Self {
        Options {
            extensions: Extensions::NONE,
            reference_override: None,
        }
    }

    /// Sets the extension set, replacing any previous value.
    #[must_use]
    pub fn with_extensions(mut self, e: Extensions) -> Self {
        self.extensions = e;
        self
    }

    /// Sets a callback consulted before the parser's own reference table.
    #[must_use]
    pub fn with_ref_override<F>(mut self, f: F) -> Self
    where
        F: Fn(&str) -> RefOverride + 'static,
    {
        self.reference_override = Some(Box::new(f));
        self
    }

    /// The configured extension set.
    pub fn extensions(&self) -> Extensions {
        self.extensions
    }
}

/// Holds the extension set and the runtime state used while parsing.
///
/// Upstream's `Markdown`. The name is kept for traceability against `.go`
/// sources even though `Parser` would read better in Rust.
///
/// The pointer fields of the Go original — `doc`, `tip`, `oldTip`,
/// `lastMatchedContainer` — are [`NodeId`]s into an owned [`Arena`].
// The parser state is written by `new` and read by the scanners in `block.rs`
// and `inline.rs`. None of it is reachable from outside the crate until the
// `run` entry point lands, so rustc sees write-only fields and methods only the
// tests call. The allow is scoped to this type and comes off with `run`.
#[allow(dead_code)]
pub struct Markdown {
    pub(crate) arena: Arena,
    pub(crate) extensions: Extensions,
    reference_override: Option<RefOverrideFn>,
    pub(crate) refs: std::collections::HashMap<String, InternalReference>,

    pub(crate) nesting: usize,
    pub(crate) max_nesting: usize,
    pub(crate) inside_link: bool,

    /// Ordered footnotes. Empty when the extension is off, matching Go's nil
    /// slice — nothing distinguishes the two observably.
    pub(crate) notes: Vec<InternalReference>,

    pub(crate) doc: NodeId,
    pub(crate) tip: NodeId,
    pub(crate) old_tip: NodeId,
    pub(crate) last_matched_container: NodeId,
    pub(crate) all_closed: bool,
}

#[allow(dead_code)] // see the note on the struct
impl Markdown {
    /// Constructs a parser, mirroring Go's `New`.
    pub fn new(options: Options) -> Self {
        let mut arena = Arena::new();
        let doc = arena.new_node(NodeType::Document);
        Markdown {
            arena,
            extensions: options.extensions,
            reference_override: options.reference_override,
            refs: std::collections::HashMap::new(),
            nesting: 0,
            max_nesting: MAX_NESTING,
            inside_link: false,
            notes: Vec::new(),
            doc,
            tip: doc,
            old_tip: doc,
            last_matched_container: doc,
            all_closed: true,
        }
    }

    /// The document root.
    pub fn document(&self) -> NodeId {
        self.doc
    }

    /// The arena backing the tree.
    pub fn arena(&self) -> &Arena {
        &self.arena
    }

    /// Resolves a reference id, consulting the override callback first.
    ///
    /// Reference ids are matched case-insensitively, so the lookup lowercases
    /// its key exactly as Go does — note that Go uses `strings.ToLower`, which
    /// is Unicode-aware, not an ASCII fold.
    pub(crate) fn get_ref(&self, refid: &str) -> Option<InternalReference> {
        if let Some(over) = &self.reference_override {
            match over(refid) {
                RefOverride::ToNothing => return None,
                RefOverride::To(r) => {
                    return Some(InternalReference {
                        link: r.link.into_bytes(),
                        title: r.title.into_bytes(),
                        note_id: 0,
                        has_block: false,
                        footnote: None,
                        text: r.text.into_bytes(),
                    })
                }
                RefOverride::NotOverridden => {}
            }
        }
        self.refs.get(&refid.to_lowercase()).cloned()
    }

    /// Closes `block`, moving the insertion point to its parent.
    pub(crate) fn finalize(&mut self, block: NodeId) {
        let above = self.arena[block].parent();
        self.arena[block].open = false;
        // Go assigns p.tip = above unconditionally, including when above is
        // nil. Only the document root has no parent, and finalize is never
        // called on it while parsing continues.
        if let Some(a) = above {
            self.tip = a;
        }
    }

    /// Appends a new node of `node_type` at the current insertion point.
    pub(crate) fn add_child(&mut self, node_type: NodeType) -> NodeId {
        let node = self.arena.new_node(node_type);
        self.add_existing_child(node)
    }

    /// Appends an existing node at the current insertion point, closing blocks
    /// that cannot contain it.
    pub(crate) fn add_existing_child(&mut self, node: NodeId) -> NodeId {
        let node_type = self.arena[node].node_type;
        while !self.arena[self.tip].node_type.can_contain(node_type) {
            let tip = self.tip;
            self.finalize(tip);
        }
        let tip = self.tip;
        self.arena.append_child(tip, node);
        self.tip = node;
        node
    }

    /// Closes every block between the last match and the old insertion point.
    pub(crate) fn close_unmatched_blocks(&mut self) {
        if !self.all_closed {
            while self.old_tip != self.last_matched_container {
                let parent = self.arena[self.old_tip].parent();
                let old = self.old_tip;
                self.finalize(old);
                match parent {
                    Some(p) => self.old_tip = p,
                    None => break,
                }
            }
            self.all_closed = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_are_last_writer_wins_like_go() {
        // Go: Run(input, WithNoExtensions(), WithExtensions(Tables))
        let o = Options::none().with_extensions(Extensions::TABLES);
        assert_eq!(o.extensions(), Extensions::TABLES);

        // ...and the other order gives the other answer.
        let o = Options::common().with_extensions(Extensions::NONE);
        assert_eq!(o.extensions(), Extensions::NONE);
    }

    #[test]
    fn default_options_match_go_run_defaults() {
        assert_eq!(Options::common().extensions(), Extensions::COMMON);
        assert_eq!(Options::common().extensions().bits(), 102590);
        assert_eq!(Options::none().extensions(), Extensions::NONE);
        assert_eq!(Options::default().extensions(), Extensions::NONE);
    }

    #[test]
    fn new_parser_starts_at_an_open_document_root() {
        let p = Markdown::new(Options::common());
        assert_eq!(p.arena[p.doc].node_type, NodeType::Document);
        assert!(p.arena[p.doc].open);
        assert_eq!(p.tip, p.doc);
        assert_eq!(p.old_tip, p.doc);
        assert_eq!(p.last_matched_container, p.doc);
        assert!(p.all_closed);
        assert_eq!(p.max_nesting, 16);
        assert!(!p.inside_link);
    }

    #[test]
    fn add_child_closes_blocks_that_cannot_contain_the_new_node() {
        let mut p = Markdown::new(Options::common());
        let para = p.add_child(NodeType::Paragraph);
        assert_eq!(p.tip, para);
        assert_eq!(p.arena[para].parent(), Some(p.doc));

        // Document cannot contain Item, and Paragraph cannot contain anything,
        // so adding a List walks back up to Document first.
        let list = p.add_child(NodeType::List);
        assert_eq!(p.arena[list].parent(), Some(p.doc));
        assert!(!p.arena[para].open, "paragraph should have been finalized");

        // List can only contain Item.
        let item = p.add_child(NodeType::Item);
        assert_eq!(p.arena[item].parent(), Some(list));
    }

    #[test]
    fn finalize_moves_the_tip_to_the_parent() {
        let mut p = Markdown::new(Options::common());
        let para = p.add_child(NodeType::Paragraph);
        p.finalize(para);
        assert!(!p.arena[para].open);
        assert_eq!(p.tip, p.doc);
    }

    #[test]
    fn close_unmatched_blocks_is_a_no_op_when_already_closed() {
        let mut p = Markdown::new(Options::common());
        let para = p.add_child(NodeType::Paragraph);
        p.all_closed = true;
        p.close_unmatched_blocks();
        assert!(p.arena[para].open, "nothing should have been finalized");
    }

    #[test]
    fn close_unmatched_blocks_walks_up_to_the_last_match() {
        let mut p = Markdown::new(Options::common());
        let quote = p.add_child(NodeType::BlockQuote);
        let para = p.add_child(NodeType::Paragraph);

        p.old_tip = para;
        p.last_matched_container = quote;
        p.all_closed = false;
        p.close_unmatched_blocks();

        assert!(!p.arena[para].open);
        assert!(p.arena[quote].open, "stops at the last matched container");
        assert!(p.all_closed);
    }

    #[test]
    fn ref_lookup_is_case_insensitive() {
        let mut p = Markdown::new(Options::common());
        p.refs.insert(
            "myref".to_string(),
            InternalReference {
                link: b"http://example.com".to_vec(),
                ..Default::default()
            },
        );
        assert!(p.get_ref("myref").is_some());
        assert!(p.get_ref("MyRef").is_some());
        assert!(p.get_ref("MYREF").is_some());
        assert!(p.get_ref("other").is_none());
    }

    #[test]
    fn ref_override_distinguishes_not_overridden_from_overridden_to_nothing() {
        let make = |o: RefOverride| {
            let mut p = Markdown::new(Options::common().with_ref_override(move |_| o.clone()));
            p.refs.insert(
                "x".to_string(),
                InternalReference {
                    link: b"from-document".to_vec(),
                    ..Default::default()
                },
            );
            p
        };

        // (nil, false): fall through to the document's own table.
        let p = make(RefOverride::NotOverridden);
        assert_eq!(p.get_ref("x").unwrap().link, b"from-document");

        // (nil, true): explicitly unresolvable, even though the document
        // defines it. Collapsing this into Option<Reference> would have
        // returned "from-document" here.
        let p = make(RefOverride::ToNothing);
        assert!(p.get_ref("x").is_none());

        // (&r, true): use the supplied reference.
        let p = make(RefOverride::To(Reference {
            link: "from-override".into(),
            title: "t".into(),
            text: "".into(),
        }));
        let r = p.get_ref("x").unwrap();
        assert_eq!(r.link, b"from-override");
        assert_eq!(r.title, b"t");
    }

    #[test]
    fn ref_override_receives_the_id_as_written() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&seen);
        let p = Markdown::new(Options::common().with_ref_override(move |id| {
            sink.borrow_mut().push(id.to_string());
            RefOverride::NotOverridden
        }));
        let _ = p.get_ref("MixedCase");
        assert_eq!(seen.borrow().as_slice(), ["MixedCase"]);
    }
}
