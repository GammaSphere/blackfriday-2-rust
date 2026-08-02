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

/// Scans the link and optional title of a reference definition.
///
/// Ported from `scanLinkRef` (`markdown.go:649`). Returns the link and title
/// spans plus the end of the definition; a `line_end` of `0` means no valid
/// reference.
///
/// Go uses named return values that stay zero on the early `return`, so a
/// rejected scan still hands back whatever offsets had been computed. Only
/// `line_end` is consulted to decide validity, so that is preserved as-is.
///
/// # Panics
///
/// If the input ends immediately after an opening `<`, matching upstream:
/// `link_offset` then equals the length and the following index panics in both
/// languages.
fn scan_link_ref(data: &[u8], mut i: usize) -> (usize, usize, usize, usize, usize) {
    let mut title_offset = 0usize;
    let mut title_end = 0usize;
    let mut line_end = 0usize;

    // Link: a whitespace-free run, optionally between angle brackets.
    if data[i] == b'<' {
        i += 1;
    }
    let mut link_offset = i;
    while i < data.len()
        && data[i] != b' '
        && data[i] != b'\t'
        && data[i] != b'\n'
        && data[i] != b'\r'
    {
        i += 1;
    }
    let mut link_end = i;
    // Go's && short-circuits before `data[linkEnd-1]`, which is what keeps a
    // zero link_end from indexing out of range here too.
    if data[link_offset] == b'<' && link_end > 0 && data[link_end - 1] == b'>' {
        link_offset += 1;
        link_end -= 1;
    }

    // Optional spacer: (space | tab)* (newline | ' | " | '(' )
    while i < data.len() && (data[i] == b' ' || data[i] == b'\t') {
        i += 1;
    }
    if i < data.len()
        && data[i] != b'\n'
        && data[i] != b'\r'
        && data[i] != b'\''
        && data[i] != b'"'
        && data[i] != b'('
    {
        return (link_offset, link_end, title_offset, title_end, line_end);
    }

    // End of line.
    if i >= data.len() || data[i] == b'\r' || data[i] == b'\n' {
        line_end = i;
    }
    if i + 1 < data.len() && data[i] == b'\r' && data[i + 1] == b'\n' {
        line_end += 1;
    }

    // Optional (space|tab)* after the newline.
    if line_end > 0 {
        i = line_end + 1;
        while i < data.len() && (data[i] == b' ' || data[i] == b'\t') {
            i += 1;
        }
    }

    // Optional title: a non-newline run enclosed in ' " or ( alone on its line.
    if i + 1 < data.len() && (data[i] == b'\'' || data[i] == b'"' || data[i] == b'(') {
        i += 1;
        title_offset = i;

        while i < data.len() && data[i] != b'\n' && data[i] != b'\r' {
            i += 1;
        }
        if i + 1 < data.len() && data[i] == b'\n' && data[i + 1] == b'\r' {
            title_end = i + 1;
        } else {
            title_end = i;
        }

        // Step back over trailing whitespace.
        i -= 1;
        while i > title_offset && (data[i] == b' ' || data[i] == b'\t') {
            i -= 1;
        }
        if i > title_offset && (data[i] == b'\'' || data[i] == b'"' || data[i] == b')') {
            line_end = title_end;
            title_end = i;
        }
    }

    (link_offset, link_end, title_offset, title_end, line_end)
}

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

    /// Scans a link-reference definition, registering it and returning its
    /// length, or `0`.
    ///
    /// Ported from `isReference` (`markdown.go:546`). Ids are stored
    /// lowercased, matching the case-insensitive lookup in [`Self::get_ref`].
    ///
    /// A footnote may have an empty id (`[^]:`), but an ordinary reference may
    /// not (`[]:`) — that asymmetry is upstream's and is preserved.
    pub(crate) fn is_reference(&mut self, data: &[u8], tab_size: usize) -> usize {
        use crate::flags::Extensions;

        // Up to three optional leading spaces.
        if data.len() < 4 {
            return 0;
        }
        let mut i = 0usize;
        while i < 3 && data[i] == b' ' {
            i += 1;
        }

        let mut note_id = 0i32;

        // Id: anything but a newline, between brackets.
        if data[i] != b'[' {
            return 0;
        }
        i += 1;
        if self.extensions.intersects(Extensions::FOOTNOTES) && i < data.len() && data[i] == b'^' {
            // Any non-zero value will do; real note ids are assigned later.
            note_id = 1;
            i += 1;
        }
        let id_offset = i;
        while i < data.len() && data[i] != b'\n' && data[i] != b'\r' && data[i] != b']' {
            i += 1;
        }
        if i >= data.len() || data[i] != b']' {
            return 0;
        }
        let id_end = i;
        // Footnotes may be empty; plain references may not.
        if note_id == 0 && id_offset == id_end {
            return 0;
        }

        // Spacer: colon (space | tab)* newline? (space | tab)*
        i += 1;
        if i >= data.len() || data[i] != b':' {
            return 0;
        }
        i += 1;
        while i < data.len() && (data[i] == b' ' || data[i] == b'\t') {
            i += 1;
        }
        if i < data.len() && (data[i] == b'\n' || data[i] == b'\r') {
            i += 1;
            if i < data.len() && data[i] == b'\n' && data[i - 1] == b'\r' {
                i += 1;
            }
        }
        while i < data.len() && (data[i] == b' ' || data[i] == b'\t') {
            i += 1;
        }
        if i >= data.len() {
            return 0;
        }

        let (link_offset, link_end, title_offset, title_end, line_end, raw, has_block);
        if self.extensions.intersects(Extensions::FOOTNOTES) && note_id != 0 {
            let (bs, be, contents, hb) = self.scan_footnote(data, i, tab_size);
            link_offset = bs;
            link_end = be;
            raw = contents;
            has_block = hb;
            title_offset = 0;
            title_end = 0;
            line_end = link_end;
        } else {
            let (lo, le, to, te, ln) = scan_link_ref(data, i);
            link_offset = lo;
            link_end = le;
            title_offset = to;
            title_end = te;
            line_end = ln;
            raw = Vec::new();
            has_block = false;
        }
        if line_end == 0 {
            return 0;
        }

        let mut r = InternalReference {
            note_id,
            has_block,
            ..Default::default()
        };
        if note_id > 0 {
            // The link field is reused for the id, since footnotes have no
            // link; and title holds the contained text rather than a title.
            r.link = data[id_offset..id_end].to_vec();
            r.title = raw;
        } else {
            r.link = data[link_offset..link_end].to_vec();
            r.title = data[title_offset..title_end].to_vec();
        }

        // Id matches are case-insensitive.
        let id = String::from_utf8_lossy(&data[id_offset..id_end]).to_lowercase();
        self.refs.insert(id, r);

        line_end
    }

    /// Extracts a footnote's body, shifting it left by one indent.
    ///
    /// Ported from `scanFootnote` (`markdown.go:723`). Returns the body's start
    /// and end in the input, the de-indented contents, and whether it spanned
    /// more than the first line.
    pub(crate) fn scan_footnote(
        &self,
        data: &[u8],
        mut i: usize,
        indent_size: usize,
    ) -> (usize, usize, Vec<u8>, bool) {
        use crate::block::is_empty;
        use crate::util::is_indented;

        if i == 0 || data.is_empty() {
            return (0, 0, Vec::new(), false);
        }

        while i < data.len() && data[i] == b' ' {
            i += 1;
        }

        let block_start = i;

        let mut block_end = i;
        while i < data.len() && data[i - 1] != b'\n' {
            i += 1;
        }

        let mut raw: Vec<u8> = Vec::new();
        raw.extend_from_slice(&data[block_end..i]);
        block_end = i;

        let mut has_block = false;
        let mut contains_blank_line = false;

        while block_end < data.len() {
            i += 1;
            while i < data.len() && data[i - 1] != b'\n' {
                i += 1;
            }

            // An empty line is assumed to belong to this item.
            if is_empty(&data[block_end..i]) > 0 {
                contains_blank_line = true;
                block_end = i;
                continue;
            }

            let n = is_indented(&data[block_end..i], indent_size);
            if n == 0 {
                // End of the block; this line is not included.
                break;
            }

            if contains_blank_line {
                raw.push(b'\n');
                contains_blank_line = false;
            }

            raw.extend_from_slice(&data[block_end + n..i]);
            has_block = true;
            block_end = i;
        }

        if data[block_end - 1] != b'\n' {
            raw.push(b'\n');
        }

        (block_start, block_end, raw, has_block)
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

    /// Measured Go answers for the reference scanners.
    const REF_FIXTURE: &str = include_str!("../tests/fixtures/go-ref.txt");

    fn ref_rows(tag: &str) -> impl Iterator<Item = Vec<String>> + '_ {
        REF_FIXTURE.lines().filter_map(move |l| {
            let f: Vec<String> = l.split(' ').map(str::to_string).collect();
            (f.first().map(String::as_str) == Some(tag)).then_some(f)
        })
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("bad hex"))
            .collect()
    }

    /// Renders the parser's ref table the way the generator does, so the two
    /// can be compared directly.
    fn refs_repr(p: &Markdown) -> String {
        let mut keys: Vec<&String> = p.refs.keys().collect();
        keys.sort();
        let mut out = String::new();
        for k in keys {
            let r = &p.refs[k];
            out.push_str(&format!(
                " {}/{}/{}/{}/{}",
                hex(k.as_bytes()),
                hex(&r.link),
                hex(&r.title),
                r.note_id,
                r.has_block
            ));
        }
        out
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn is_reference_matches_go_with_and_without_footnotes() {
        use crate::flags::Extensions;
        let mut n = 0;
        for f in ref_rows("R") {
            let data = unhex(&f[1]);
            let pipe = f.iter().position(|s| s == "|").unwrap();
            let ctx = format!("is_reference({:?})", String::from_utf8_lossy(&data));

            for (footnotes, span) in [(false, 2..pipe), (true, pipe + 1..f.len())] {
                let ext = if footnotes {
                    Extensions::FOOTNOTES
                } else {
                    Extensions::NONE
                };
                let mut p = Markdown::new(Options::none().with_extensions(ext));
                let got = p.is_reference(&data, crate::TAB_SIZE_DEFAULT);
                let want: Vec<String> = f[span].to_vec();
                assert_eq!(
                    got,
                    want[0].parse::<usize>().unwrap(),
                    "{ctx} size [footnotes={footnotes}]"
                );
                let want_refs = want[1..]
                    .iter()
                    .map(|s| format!(" {s}"))
                    .collect::<String>();
                assert_eq!(
                    refs_repr(&p),
                    want_refs,
                    "{ctx} refs [footnotes={footnotes}]"
                );
            }
            n += 1;
        }
        assert!(n >= 18, "thin corpus: {n}");
    }

    #[test]
    fn scan_link_ref_matches_go() {
        let mut n = 0;
        for f in ref_rows("L") {
            let data = unhex(&f[1]);
            let i: usize = f[2].parse().unwrap();
            let got = super::scan_link_ref(&data, i);
            let want = (
                f[3].parse::<usize>().unwrap(),
                f[4].parse::<usize>().unwrap(),
                f[5].parse::<usize>().unwrap(),
                f[6].parse::<usize>().unwrap(),
                f[7].parse::<usize>().unwrap(),
            );
            assert_eq!(
                got,
                want,
                "scan_link_ref({:?}, {i})",
                String::from_utf8_lossy(&data)
            );
            n += 1;
        }
        assert!(n >= 5, "thin corpus: {n}");
    }

    #[test]
    fn scan_footnote_matches_go() {
        use crate::flags::Extensions;
        let mut n = 0;
        for f in ref_rows("N") {
            let data = unhex(&f[1]);
            let i: usize = f[2].parse().unwrap();
            let p = Markdown::new(Options::none().with_extensions(Extensions::FOOTNOTES));
            let (bs, be, contents, hb) = p.scan_footnote(&data, i, crate::TAB_SIZE_DEFAULT);
            let ctx = format!("scan_footnote({:?}, {i})", String::from_utf8_lossy(&data));
            assert_eq!(bs, f[3].parse::<usize>().unwrap(), "{ctx} start");
            assert_eq!(be, f[4].parse::<usize>().unwrap(), "{ctx} end");
            assert_eq!(contents, unhex(&f[5]), "{ctx} contents");
            assert_eq!(hb, f[6] == "true", "{ctx} has_block");
            n += 1;
        }
        assert!(n >= 5, "thin corpus: {n}");
    }

    #[test]
    fn footnotes_may_have_an_empty_id_but_references_may_not() {
        use crate::flags::Extensions;
        // [^]: is legal, []: is not. The asymmetry is upstream's.
        let mut p = Markdown::new(Options::none().with_extensions(Extensions::FOOTNOTES));
        assert!(p.is_reference(b"[^]: note\n", 4) > 0);

        let mut p = Markdown::new(Options::none());
        assert_eq!(p.is_reference(b"[]: http://example.com\n", 4), 0);
    }

    #[test]
    fn reference_ids_are_stored_lowercased() {
        let mut p = Markdown::new(Options::none());
        assert!(p.is_reference(b"[MixedCase]: http://x\n", 4) > 0);
        assert!(p.refs.contains_key("mixedcase"));
        assert!(!p.refs.contains_key("MixedCase"));
        // ...which is what makes the case-insensitive lookup work.
        assert!(p.get_ref("MIXEDCASE").is_some());
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
