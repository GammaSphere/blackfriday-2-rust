//! Inline-level parsing, ported from upstream `inline.go`.
//!
//! This commit carries the module's pure helpers — the ones that answer a
//! question about a byte slice without touching parser state. The inline
//! dispatch itself follows.
//!
//! # Two regexps, hand-coded
//!
//! `inline.go` compiles two patterns at package level, and the port carries no
//! dependencies, so both are written out by hand here:
//!
//! - `htmlEntityRe`, `&([a-zA-Z]{2,31}[0-9]{0,2}|#([0-9]{1,7}|[xX][0-9a-fA-F]{1,6}));`
//!   — see `find_html_entities`.
//! - `anchorRe`, which recognises a complete `<a href="…">…</a>` — see
//!   `anchor_match_len`.
//!
//! Neither needs backtracking once the structure is looked at closely, and the
//! reasoning is recorded at each function. Both are checked against the real
//! `regexp` output rather than against that reasoning.

use crate::flags::Extensions;
use crate::markdown::{InternalReference, Markdown};
use crate::node::{NodeId, NodeType};
use crate::util::{is_alnum, is_letter, is_punct, is_space};

/// The characters a backslash may escape.
///
/// Ported from `escapeChars` (`inline.go:661`).
pub(crate) const ESCAPE_CHARS: &[u8] = b"\\`*_{}[]()#+-.!:|&<>~";

/// What kind of autolink a `<…>` run turned out to be.
///
/// Ported from `autolinkType` (`inline.go:620`).
// The shared `Autolink` suffix is upstream's; renaming would cost the
// one-to-one correspondence this port is checked against.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum AutolinkType {
    /// Not an autolink at all — an HTML tag, or nothing.
    #[default]
    NotAutolink,
    /// A URI autolink, `<http://example.com>`.
    NormalAutolink,
    /// A bare email address, `<user@example.com>`.
    EmailAutolink,
}

/// Copies `src` to `out`, dropping backslashes.
///
/// Ported from `unescapeText` (`inline.go:680`). A trailing lone backslash is
/// dropped rather than kept, since the loop breaks when there is no byte after
/// it to copy.
pub(crate) fn unescape_text(out: &mut Vec<u8>, src: &[u8]) {
    let mut i = 0;
    while i < src.len() {
        let org = i;
        while i < src.len() && src[i] != b'\\' {
            i += 1;
        }

        if i > org {
            out.extend_from_slice(&src[org..i]);
        }

        if i + 1 >= src.len() {
            break;
        }

        out.push(src[i + 1]);
        i += 2;
    }
}

/// Strips a `mailto:` or `mailto://` prefix.
///
/// Ported from `stripMailto` (`inline.go:609`). Case-sensitive, so `MAILTO:`
/// is left alone.
pub(crate) fn strip_mailto(link: &[u8]) -> &[u8] {
    if link.starts_with(b"mailto://") {
        &link[9..]
    } else if link.starts_with(b"mailto:") {
        &link[7..]
    } else {
        link
    }
}

/// ASCII-only case-insensitive prefix test.
///
/// Ported from `hasPrefixCaseInsensitive` (`inline.go:742`), which upstream
/// wrote by hand to avoid `strings.ToLower` dragging in the Unicode tables.
///
/// The comparison is `b != s[i] && b != s[i]+delta`, and `s[i]+delta`
/// **overflows** for any byte above 0xE5. Go wraps silently; Rust panics in
/// debug, so this uses [`u8::wrapping_add`] to keep the same answers on
/// non-ASCII input.
pub(crate) fn has_prefix_case_insensitive(s: &[u8], prefix: &[u8]) -> bool {
    if s.len() < prefix.len() {
        return false;
    }
    let delta = b'a' - b'A';
    for (i, &b) in prefix.iter().enumerate() {
        if b != s[i] && b != s[i].wrapping_add(delta) {
            return false;
        }
    }
    true
}

/// Whether `c` terminates a bare URL.
///
/// Ported from `isEndOfLink` (`inline.go:901`).
pub(crate) const fn is_end_of_link(c: u8) -> bool {
    is_space(c) || c == b'<'
}

/// Length of the tag or autolink starting at `data[0]`, and which it is.
///
/// Ported from `tagLength` (`inline.go:931`).
///
/// # The final fallthrough is unguarded
///
/// Upstream ends with
///
/// ```text
/// i += bytes.IndexByte(data[i:], '>')
/// if i < 0 { return autolink, 0 }
/// return autolink, i + 1
/// ```
///
/// `IndexByte` returns `-1` when there is no `>`, so `i` becomes `i - 1` — and
/// the guard can only fire when `i` was `0`, which cannot happen this far in.
/// A tag with no closing `>` therefore reports the offset it had reached
/// instead of failing. Measured on `<no closing` and friends; reproduced.
pub(crate) fn tag_length(data: &[u8]) -> (AutolinkType, usize) {
    // A valid tag cannot be shorter than three bytes.
    if data.len() < 3 {
        return (AutolinkType::NotAutolink, 0);
    }

    // Begins with '<', optionally '/', then a letter or digit.
    if data[0] != b'<' {
        return (AutolinkType::NotAutolink, 0);
    }
    let mut i = if data[1] == b'/' { 2 } else { 1 };

    if !is_alnum(data[i]) {
        return (AutolinkType::NotAutolink, 0);
    }

    let mut autolink = AutolinkType::NotAutolink;

    // Find the end of what could be a scheme or a hostname.
    while i < data.len()
        && (is_alnum(data[i]) || data[i] == b'.' || data[i] == b'+' || data[i] == b'-')
    {
        i += 1;
    }

    if i > 1 && i < data.len() && data[i] == b'@' {
        let j = is_mailto_auto_link(&data[i..]);
        if j != 0 {
            return (AutolinkType::EmailAutolink, i + j);
        }
    }

    if i > 2 && i < data.len() && data[i] == b':' {
        autolink = AutolinkType::NormalAutolink;
        i += 1;
    }

    // Complete-autolink test: no whitespace, no quote.
    if i >= data.len() {
        autolink = AutolinkType::NotAutolink;
    } else if autolink != AutolinkType::NotAutolink {
        let j = i;

        while i < data.len() {
            if data[i] == b'\\' {
                i += 2;
            } else if data[i] == b'>' || data[i] == b'\'' || data[i] == b'"' || is_space(data[i]) {
                break;
            } else {
                i += 1;
            }
        }

        if i >= data.len() {
            return (autolink, 0);
        }
        if i > j && data[i] == b'>' {
            return (autolink, i + 1);
        }

        // One of the forbidden characters turned up.
        autolink = AutolinkType::NotAutolink;
    }

    match data[i..].iter().position(|&b| b == b'>') {
        Some(k) => (autolink, i + k + 1),
        // The unguarded `i += -1` described above, spelled out.
        None => (autolink, i),
    }
}

/// Length of an email autolink body ending at `>`, or `0`.
///
/// Ported from `isMailtoAutoLink` (`inline.go:1009`). The address is
/// `[-@._a-zA-Z0-9]+` with exactly one `@`, which is looser than Markdown's
/// original rule.
pub(crate) fn is_mailto_auto_link(data: &[u8]) -> usize {
    let mut nb = 0;

    for (i, &c) in data.iter().enumerate() {
        if is_alnum(c) {
            continue;
        }

        match c {
            b'@' => nb += 1,
            // Upstream writes `break` here, which in Go's switch means "this
            // case is done" — it continues the loop, it does not leave it.
            b'-' | b'.' | b'_' => {}
            b'>' => {
                return if nb == 1 { i + 1 } else { 0 };
            }
            _ => return 0,
        }
    }

    0
}

/// Offset of the next emphasis character, skipping code spans and links.
///
/// Ported from `helperFindEmphChar` (`inline.go:1039`). Returns `0` both for
/// "not found" and for "found at offset zero"; callers treat `0` as failure,
/// which is upstream's convention and is why an emphasis run can never open at
/// the very first byte of the data handed here.
pub(crate) fn helper_find_emph_char(data: &[u8], c: u8) -> usize {
    let mut i = 0;

    while i < data.len() {
        while i < data.len() && data[i] != c && data[i] != b'`' && data[i] != b'[' {
            i += 1;
        }
        if i >= data.len() {
            return 0;
        }
        // Escaped characters do not count.
        if i != 0 && data[i - 1] == b'\\' {
            i += 1;
            continue;
        }
        if data[i] == c {
            return i;
        }

        if data[i] == b'`' {
            // Skip a code span.
            let mut tmp_i = 0;
            i += 1;
            while i < data.len() && data[i] != b'`' {
                if tmp_i == 0 && data[i] == c {
                    tmp_i = i;
                }
                i += 1;
            }
            if i >= data.len() {
                return tmp_i;
            }
            i += 1;
        } else if data[i] == b'[' {
            // Skip a link.
            let mut tmp_i = 0;
            i += 1;
            while i < data.len() && data[i] != b']' {
                if tmp_i == 0 && data[i] == c {
                    tmp_i = i;
                }
                i += 1;
            }
            i += 1;
            while i < data.len() && (data[i] == b' ' || data[i] == b'\n') {
                i += 1;
            }
            if i >= data.len() {
                return tmp_i;
            }
            if data[i] != b'[' && data[i] != b'(' {
                // Not a link after all.
                if tmp_i > 0 {
                    return tmp_i;
                }
                continue;
            }
            let cc = data[i];
            i += 1;
            while i < data.len() && data[i] != cc {
                if tmp_i == 0 && data[i] == c {
                    return i;
                }
                i += 1;
            }
            if i >= data.len() {
                return tmp_i;
            }
            i += 1;
        }
    }
    0
}

/// Go's `\s` inside a regexp: `[\t\n\f\r ]`.
const fn is_re_space(c: u8) -> bool {
    matches!(c, b'\t' | b'\n' | 0x0c | b'\r' | b' ')
}

/// Length of an HTML entity starting at `d[0] == b'&'`, or `None`.
///
/// One alternation of `htmlEntityRe`. Neither branch needs backtracking:
///
/// - The named branch takes as many letters as it can (capped at 31) and then
///   as many digits as it can (capped at 2). Giving a letter back leaves a
///   letter where a digit or `;` must be, and giving a digit back leaves a
///   digit where `;` must be, so a shorter match can never succeed where the
///   greedy one failed.
/// - The numeric branch is the same argument with one alternation in front,
///   and Go tries decimal before hex because that is the order written.
fn html_entity_len(d: &[u8]) -> Option<usize> {
    debug_assert_eq!(d.first(), Some(&b'&'));

    // Named: [a-zA-Z]{2,31}[0-9]{0,2} ";"
    let mut letters = 0;
    while 1 + letters < d.len() && d[1 + letters].is_ascii_alphabetic() && letters < 31 {
        letters += 1;
    }
    if letters >= 2 {
        let mut digits = 0;
        while 1 + letters + digits < d.len()
            && d[1 + letters + digits].is_ascii_digit()
            && digits < 2
        {
            digits += 1;
        }
        let p = 1 + letters + digits;
        if p < d.len() && d[p] == b';' {
            return Some(p + 1);
        }
    }

    // Numeric: "#" ( [0-9]{1,7} | [xX][0-9a-fA-F]{1,6} ) ";"
    if d.len() > 1 && d[1] == b'#' {
        let mut digits = 0;
        while 2 + digits < d.len() && d[2 + digits].is_ascii_digit() && digits < 7 {
            digits += 1;
        }
        if digits >= 1 {
            let p = 2 + digits;
            if p < d.len() && d[p] == b';' {
                return Some(p + 1);
            }
        }

        if d.len() > 2 && (d[2] == b'x' || d[2] == b'X') {
            let mut hex = 0;
            while 3 + hex < d.len() && d[3 + hex].is_ascii_hexdigit() && hex < 6 {
                hex += 1;
            }
            if hex >= 1 {
                let p = 3 + hex;
                if p < d.len() && d[p] == b';' {
                    return Some(p + 1);
                }
            }
        }
    }

    None
}

/// Every non-overlapping HTML entity in `data`, as `(start, end)` pairs.
///
/// Stands in for `htmlEntityRe.FindAllIndex(data, -1)`. Go scans left to
/// right and resumes after each match, which is what this loop does.
pub(crate) fn find_html_entities(data: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'&' {
            if let Some(len) = html_entity_len(&data[i..]) {
                out.push((i, i + len));
                i += len;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Whether an entity ends exactly where a bare link was cut.
///
/// Ported from `linkEndsWithEntity` (`inline.go:732`). Used to keep
/// `http://x/?a=1&amp;` from losing its final semicolon.
pub(crate) fn link_ends_with_entity(data: &[u8], link_end: usize) -> bool {
    let ranges = find_html_entities(&data[..link_end]);
    ranges.last().is_some_and(|&(_, end)| end == link_end)
}

/// Whether `c` may appear in the URL part of `anchorRe`.
///
/// The class `[-A-Za-z0-9+&@#/%?=~_|!:,.;()]`.
const fn is_url_char(c: u8) -> bool {
    matches!(c,
        b'-' | b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'&' | b'@' | b'#'
        | b'/' | b'%' | b'?' | b'=' | b'~' | b'_' | b'|' | b'!' | b':' | b','
        | b'.' | b';' | b'(' | b')')
}

/// End offset of `((https?|ftp)://|/)[urlchars]+` starting at `i`, or `None`.
///
/// The scheme alternation is tried longest-first because `https?` is greedy;
/// the bare `/` is the second alternative and is only reached when the first
/// fails, exactly as Go's leftmost-first matching would.
fn url_len(d: &[u8], mut i: usize) -> Option<usize> {
    let rest = d.get(i..)?;
    let scheme = if rest.starts_with(b"https://") {
        8
    } else if rest.starts_with(b"http://") {
        7
    } else if rest.starts_with(b"ftp://") {
        6
    } else if rest.starts_with(b"/") {
        1
    } else {
        return None;
    };
    i += scheme;

    let start = i;
    while i < d.len() && is_url_char(d[i]) {
        i += 1;
    }
    // The `+` needs at least one character, and no character in the class can
    // also end it, so the greedy run never needs to give anything back.
    (i > start).then_some(i)
}

/// Length of a complete `<a href="…">…</a>` at the start of `data`, or `None`.
///
/// Stands in for `anchorRe`, which is
///
/// ```text
/// ^(<a\shref="URL"(\stitle="[^"<>]+")?\s?>URL</a>)
/// ```
///
/// with `URL` being `((https?|ftp):\/\/|\/)[-A-Za-z0-9+&@#\/%?=~_|!:,.;\(\)]+`.
/// The pattern is anchored, so only offset zero can match.
///
/// Two groups are optional and both are greedy, so each is attempted before
/// the alternative of skipping it — that is what leftmost-first means, and it
/// is why this is written as "try with, fall back to without" rather than as a
/// straight-line scan.
pub(crate) fn anchor_match_len(data: &[u8]) -> Option<usize> {
    if !data.starts_with(b"<a") {
        return None;
    }
    let mut i = 2;

    if i >= data.len() || !is_re_space(data[i]) {
        return None;
    }
    i += 1;

    if !data[i..].starts_with(b"href=\"") {
        return None;
    }
    i += 6;

    i = url_len(data, i)?;
    if i >= data.len() || data[i] != b'"' {
        return None;
    }
    i += 1;

    // (\stitle="[^"<>]+")?
    let with_title = (|| {
        let mut j = i;
        if j >= data.len() || !is_re_space(data[j]) {
            return None;
        }
        j += 1;
        if !data[j..].starts_with(b"title=\"") {
            return None;
        }
        j += 7;
        let start = j;
        while j < data.len() && data[j] != b'"' && data[j] != b'<' && data[j] != b'>' {
            j += 1;
        }
        if j == start || j >= data.len() {
            return None;
        }
        Some(j + 1)
    })();

    with_title
        .and_then(|j| anchor_tail(data, j))
        .or_else(|| anchor_tail(data, i))
}

/// The `\s?>URL</a>` tail of `anchorRe`, from offset `i`.
fn anchor_tail(data: &[u8], i: usize) -> Option<usize> {
    if i < data.len() && is_re_space(data[i]) {
        if let Some(end) = anchor_close(data, i + 1) {
            return Some(end);
        }
    }
    anchor_close(data, i)
}

/// The `>URL</a>` tail, once the optional space has been settled.
fn anchor_close(data: &[u8], mut i: usize) -> Option<usize> {
    if i >= data.len() || data[i] != b'>' {
        return None;
    }
    i += 1;
    i = url_len(data, i)?;
    data[i..].starts_with(b"</a>").then_some(i + 4)
}

/// Normalises a URI.
///
/// Ported from `normalizeURI` (`inline.go:1226`), which is `return s` behind a
/// `TODO: implement`. Kept as a named function so the call site still reads
/// like upstream's and a future fix has somewhere to go.
pub(crate) fn normalize_uri(s: &[u8]) -> Vec<u8> {
    s.to_vec()
}

/// One inline handler.
///
/// Ported from `inlineParser` (`markdown.go:105`). Upstream's handlers take the
/// parser explicitly rather than closing over it, so these are plain function
/// pointers — unlike smartypants, no tag enum is needed.
pub(crate) type InlineParser = fn(&mut Markdown, &[u8], usize) -> (usize, Option<NodeId>);

/// Builds the dispatch table for an extension set.
///
/// Ported from the registration block in `New` (`markdown.go:285`).
pub(crate) fn callbacks(extensions: Extensions) -> [Option<InlineParser>; 256] {
    let mut cb: [Option<InlineParser>; 256] = [None; 256];
    cb[b' ' as usize] = Some(maybe_line_break as InlineParser);
    cb[b'*' as usize] = Some(emphasis as InlineParser);
    cb[b'_' as usize] = Some(emphasis as InlineParser);
    if extensions.intersects(Extensions::STRIKETHROUGH) {
        cb[b'~' as usize] = Some(emphasis as InlineParser);
    }
    cb[b'`' as usize] = Some(code_span as InlineParser);
    cb[b'\n' as usize] = Some(line_break as InlineParser);
    cb[b'[' as usize] = Some(link as InlineParser);
    cb[b'<' as usize] = Some(left_angle as InlineParser);
    cb[b'\\' as usize] = Some(escape as InlineParser);
    cb[b'&' as usize] = Some(entity as InlineParser);
    cb[b'!' as usize] = Some(maybe_image as InlineParser);
    cb[b'^' as usize] = Some(maybe_inline_footnote as InlineParser);
    if extensions.intersects(Extensions::AUTOLINK) {
        for c in *b"hmfHMF" {
            cb[c as usize] = Some(maybe_auto_link as InlineParser);
        }
    }
    cb
}

/// Creates a `Text` node holding `s`.
///
/// Ported from `text` (`inline.go:1220`).
fn text_node(p: &mut Markdown, s: &[u8]) -> NodeId {
    let node = p.arena.new_node(NodeType::Text);
    p.arena.get_mut(node).literal = s.to_vec();
    node
}

/// Creates a childless node of `node_type`.
fn bare_node(p: &mut Markdown, node_type: NodeType) -> NodeId {
    p.arena.new_node(node_type)
}

impl Markdown {
    /// Parses `data` as inline content, appending children to `curr_block`.
    ///
    /// Ported from `inline` (`inline.go:49`).
    ///
    /// Two details are load-bearing. A `Text` node is appended for the run
    /// before each match **even when that run is empty**, so the tree carries
    /// zero-length text nodes; and the trailing run drops a final newline. Both
    /// show up in a tree comparison even though neither changes the HTML.
    pub(crate) fn inline(&mut self, curr_block: NodeId, data: &[u8]) {
        // Handlers recurse into this, so the depth is capped.
        if self.nesting >= self.max_nesting || data.is_empty() {
            return;
        }
        self.nesting += 1;

        let (mut beg, mut end) = (0usize, 0usize);
        while end < data.len() {
            match self.inline_callback[data[end] as usize] {
                Some(handler) => {
                    let (consumed, node) = handler(self, data, end);
                    if consumed == 0 {
                        // The handler declined.
                        end += 1;
                    } else {
                        let t = text_node(self, &data[beg..end]);
                        self.arena.append_child(curr_block, t);
                        if let Some(n) = node {
                            self.arena.append_child(curr_block, n);
                        }
                        // Skip past whatever the handler used.
                        beg = end + consumed;
                        end = beg;
                    }
                }
                None => end += 1,
            }
        }

        if beg < data.len() {
            if data[end - 1] == b'\n' {
                end -= 1;
            }
            let t = text_node(self, &data[beg..end]);
            self.arena.append_child(curr_block, t);
        }

        self.nesting -= 1;
    }
}

/// `*`, `_` and `~`: single, double or triple emphasis.
///
/// Ported from `emphasis` (`inline.go:86`).
fn emphasis(p: &mut Markdown, data: &[u8], offset: usize) -> (usize, Option<NodeId>) {
    let data = &data[offset..];
    let c = data[0];

    if data.len() > 2 && data[1] != c {
        // Whitespace cannot follow an opening delimiter, and strikethrough
        // only exists in its doubled form.
        if c == b'~' || is_space(data[1]) {
            return (0, None);
        }
        let (ret, node) = helper_emphasis(p, &data[1..], c);
        if ret == 0 {
            return (0, None);
        }
        return (ret + 1, node);
    }

    if data.len() > 3 && data[1] == c && data[2] != c {
        if is_space(data[2]) {
            return (0, None);
        }
        let (ret, node) = helper_double_emphasis(p, &data[2..], c);
        if ret == 0 {
            return (0, None);
        }
        return (ret + 2, node);
    }

    if data.len() > 4 && data[1] == c && data[2] == c && data[3] != c {
        if c == b'~' || is_space(data[3]) {
            return (0, None);
        }
        let (ret, node) = helper_triple_emphasis(p, data, 3, c);
        if ret == 0 {
            return (0, None);
        }
        return (ret + 3, node);
    }

    (0, None)
}

/// `` ` ``: an inline code span.
///
/// Ported from `codeSpan` (`inline.go:131`). A span whose contents are all
/// spaces is consumed but produces no node.
fn code_span(p: &mut Markdown, data: &[u8], offset: usize) -> (usize, Option<NodeId>) {
    let data = &data[offset..];

    let mut nb = 0;
    while nb < data.len() && data[nb] == b'`' {
        nb += 1;
    }

    // Find a run of the same length.
    let mut i = 0;
    let mut end = nb;
    while end < data.len() && i < nb {
        if data[end] == b'`' {
            i += 1;
        } else {
            i = 0;
        }
        end += 1;
    }

    if i < nb && end >= data.len() {
        return (0, None);
    }

    let mut f_begin = nb;
    while f_begin < end && data[f_begin] == b' ' {
        f_begin += 1;
    }

    let mut f_end = end - nb;
    while f_end > f_begin && data[f_end - 1] == b' ' {
        f_end -= 1;
    }

    if f_begin != f_end {
        let code = bare_node(p, NodeType::Code);
        p.arena.get_mut(code).literal = data[f_begin..f_end].to_vec();
        return (end, Some(code));
    }

    (end, None)
}

/// ` `: a newline preceded by two spaces becomes `<br>`.
///
/// Ported from `maybeLineBreak` (`inline.go:178`). A single trailing space
/// before a newline is consumed without producing anything, which is how the
/// space disappears from the output.
fn maybe_line_break(p: &mut Markdown, data: &[u8], offset: usize) -> (usize, Option<NodeId>) {
    let orig_offset = offset;
    let mut offset = offset;
    while offset < data.len() && data[offset] == b' ' {
        offset += 1;
    }

    if offset < data.len() && data[offset] == b'\n' {
        if offset - orig_offset >= 2 {
            let br = bare_node(p, NodeType::Hardbreak);
            return (offset - orig_offset + 1, Some(br));
        }
        return (offset - orig_offset, None);
    }
    (0, None)
}

/// `\n`: a break in its own right when `HARD_LINE_BREAK` is set.
///
/// Ported from `lineBreak` (`inline.go:194`).
fn line_break(p: &mut Markdown, _data: &[u8], _offset: usize) -> (usize, Option<NodeId>) {
    if p.extensions.intersects(Extensions::HARD_LINE_BREAK) {
        let br = bare_node(p, NodeType::Hardbreak);
        return (1, Some(br));
    }
    (0, None)
}

/// Which of the four bracketed constructs is being parsed.
///
/// Ported from `linkType` (`inline.go:201`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LinkType {
    /// `[text](url)` or `[text][ref]`.
    Normal,
    /// `![alt](url)`.
    Img,
    /// `[^id]`, defined elsewhere in the document.
    DeferredFootnote,
    /// `^[text]`, defined where it is used.
    InlineFootnote,
}

/// Whether a `[…]` at `pos` opens a reference rather than a footnote.
///
/// Ported from `isReferenceStyleLink` (`inline.go:210`).
fn is_reference_style_link(data: &[u8], pos: usize, t: LinkType) -> bool {
    if t == LinkType::DeferredFootnote {
        return false;
    }
    pos + 1 < data.len() && data[pos] == b'[' && data[pos + 1] != b'^'
}

/// `!`: an image, when a bracket follows.
///
/// Ported from `maybeImage` (`inline.go:217`).
fn maybe_image(p: &mut Markdown, data: &[u8], offset: usize) -> (usize, Option<NodeId>) {
    if offset + 1 < data.len() && data[offset + 1] == b'[' {
        return link(p, data, offset);
    }
    (0, None)
}

/// `^`: an inline footnote, when a bracket follows.
///
/// Ported from `maybeInlineFootnote` (`inline.go:224`). Note it does not check
/// the `Footnotes` extension; `link` decides, and without the extension the
/// caret is treated as ordinary text by falling through to a normal link.
fn maybe_inline_footnote(p: &mut Markdown, data: &[u8], offset: usize) -> (usize, Option<NodeId>) {
    if offset + 1 < data.len() && data[offset + 1] == b'[' {
        return link(p, data, offset);
    }
    (0, None)
}

/// `[`, and the entry point for images and footnotes.
///
/// Ported from `link` (`inline.go:232`), the longest function in the file.
#[allow(clippy::too_many_lines)]
fn link(p: &mut Markdown, data: &[u8], offset: usize) -> (usize, Option<NodeId>) {
    // No links inside links, footnotes or deferred footnotes.
    if p.inside_link
        && ((offset > 0 && data[offset - 1] == b'[')
            || (offset + 1 < data.len() && data[offset + 1] == b'^'))
    {
        return (0, None);
    }

    let mut offset = offset;
    let footnotes = p.extensions.intersects(Extensions::FOOTNOTES);
    let t = if footnotes && offset + 1 < data.len() && data[offset + 1] == b'^' {
        // `![^text]` is a deferred footnote following an exclamation mark.
        LinkType::DeferredFootnote
    } else if data[offset] == b'!' {
        offset += 1;
        LinkType::Img
    } else if footnotes {
        if data[offset] == b'^' {
            offset += 1;
            LinkType::InlineFootnote
        } else {
            // Upstream's second arm here repeats the first case's condition
            // and so can never be taken; the zero value of linkType wins.
            LinkType::Normal
        }
    } else {
        LinkType::Normal
    };

    let data = &data[offset..];

    let mut i = 1;
    let mut note_id: i32 = 0;
    // Go leaves this nil until something assigns it, and the renderer's
    // Image arm tests it against nil rather than against its length -- so an
    // empty-but-present title emits `title=""` and an absent one emits
    // nothing. Vec<u8> cannot hold that distinction; Option<Vec<u8>> can.
    let mut title: Option<Vec<u8>> = None;
    let mut link_dest: Vec<u8> = Vec::new();
    let mut alt_content: Vec<u8> = Vec::new();
    let mut text_has_nl = false;

    if t == LinkType::DeferredFootnote {
        i += 1;
    }

    // Find the matching close bracket.
    let mut level = 1i32;
    while level > 0 && i < data.len() {
        if data[i] == b'\n' {
            text_has_nl = true;
        } else if crate::block::is_backslash_escaped(data, i) {
            // Upstream `continue`s, which in a Go for-loop still runs the
            // post statement, so this arm does nothing at all.
        } else if data[i] == b'[' {
            level += 1;
        } else if data[i] == b']' {
            level -= 1;
            if level <= 0 {
                i -= 1; // compensate for the increment below
            }
        }
        i += 1;
    }

    if i >= data.len() {
        return (0, None);
    }

    let txt_e = i;
    i += 1;
    let mut footnote_node: Option<NodeId> = None;

    // Skip any run of whitespace, which is far more lax than Markdown's
    // original syntax allows.
    while i < data.len() && is_space(data[i]) {
        i += 1;
    }

    if i < data.len() && data[i] == b'(' {
        // Inline style: [text](url "title")
        i += 1;

        while i < data.len() && is_space(data[i]) {
            i += 1;
        }

        let link_b = i;

        // Scan to ' " or )
        while i < data.len() {
            if data[i] == b'\\' {
                i += 2;
            } else if data[i] == b')' || data[i] == b'\'' || data[i] == b'"' {
                break;
            } else {
                i += 1;
            }
        }

        if i >= data.len() {
            return (0, None);
        }
        let mut link_e = i;

        // The title, if there is one.
        let (mut title_b, mut title_e) = (0usize, 0usize);
        if data[i] == b'\'' || data[i] == b'"' {
            i += 1;
            title_b = i;

            while i < data.len() {
                if data[i] == b'\\' {
                    i += 2;
                } else if data[i] == b')' {
                    break;
                } else {
                    i += 1;
                }
            }

            if i >= data.len() {
                return (0, None);
            }

            title_e = i - 1;
            while title_e > title_b && is_space(data[title_e]) {
                title_e -= 1;
            }

            // Without a closing quote there was no title after all.
            if data[title_e] != b'\'' && data[title_e] != b'"' {
                title_b = 0;
                title_e = 0;
                link_e = i;
            }
        }

        while link_e > link_b && is_space(data[link_e - 1]) {
            link_e -= 1;
        }

        // Optional angle brackets around the destination.
        let mut link_b = link_b;
        if data[link_b] == b'<' {
            link_b += 1;
        }
        if data[link_e - 1] == b'>' {
            link_e -= 1;
        }

        if link_e > link_b {
            link_dest = data[link_b..link_e].to_vec();
        }

        if title_e > title_b {
            title = Some(data[title_b..title_e].to_vec());
        }

        i += 1;
    } else if is_reference_style_link(data, i, t) {
        // Reference style: [text][id]
        let mut alt_content_considered = false;

        i += 1;
        let link_b = i;
        while i < data.len() && data[i] != b']' {
            i += 1;
        }
        if i >= data.len() {
            return (0, None);
        }
        let link_e = i;

        let id: Vec<u8> = if link_b == link_e {
            if text_has_nl {
                collapse_newlines(data, txt_e, 1)
            } else {
                alt_content_considered = true;
                data[1..txt_e].to_vec()
            }
        } else {
            data[link_b..link_e].to_vec()
        };

        let Some(lr) = p.get_ref(&String::from_utf8_lossy(&id)) else {
            return (0, None);
        };

        link_dest = lr.link;
        title = Some(lr.title);
        if alt_content_considered {
            alt_content = lr.text;
        }
        i += 1;
    } else {
        // Shortcut reference, deferred footnote, or inline footnote.
        let id: Vec<u8> = if text_has_nl {
            collapse_newlines(data, txt_e, 1)
        } else if t == LinkType::DeferredFootnote {
            data[2..txt_e].to_vec() // drop the '^'
        } else {
            data[1..txt_e].to_vec()
        };

        let fnode = bare_node(p, NodeType::Item);
        footnote_node = Some(fnode);

        if t == LinkType::InlineFootnote {
            note_id = p.notes.len() as i32 + 1;

            // The anchor is the slug, truncated to sixteen bytes -- note that
            // upstream sizes the buffer from the *id* and then copies the
            // slug into it, so a slug longer than the id is cut short and a
            // shorter one leaves trailing NULs.
            let fragment: Vec<u8> = if !id.is_empty() {
                let n = if id.len() < 16 { id.len() } else { 16 };
                let mut frag = vec![0u8; n];
                let slug = crate::util::slugify(&id);
                let copied = n.min(slug.len());
                frag[..copied].copy_from_slice(&slug[..copied]);
                frag
            } else {
                format!("footnote-{note_id}").into_bytes()
            };

            let r = InternalReference {
                note_id,
                has_block: false,
                link: fragment,
                title: id,
                footnote: Some(fnode),
                text: Vec::new(),
            };

            link_dest = r.link.clone();
            title = Some(r.title.clone());
            p.notes.push(r);
        } else {
            let key = String::from_utf8_lossy(&id).into_owned();
            let Some((mut lr, from_table)) = p.get_ref_owned(&key) else {
                return (0, None);
            };

            if t == LinkType::DeferredFootnote {
                lr.note_id = p.notes.len() as i32 + 1;
                lr.footnote = Some(fnode);
                p.notes.push(lr.clone());
                // Go held a pointer into p.refs, so this assignment is visible
                // to every later lookup of the same id. A reference the
                // override callback invented is freshly built each call and
                // has nowhere to be written back to.
                if from_table {
                    p.put_ref(&key, lr.clone());
                }
            }

            link_dest = lr.link;
            // For a footnote the title holds the note's contents.
            title = Some(lr.title);
            note_id = lr.note_id;
        }

        // Rewind over the whitespace skipped above.
        i = txt_e + 1;
    }

    let mut u_link: Vec<u8> = Vec::new();
    if t == LinkType::Normal || t == LinkType::Img {
        if !link_dest.is_empty() {
            unescape_text(&mut u_link, &link_dest);
        }

        // A link needs something to click on and somewhere to go.
        if u_link.is_empty() || (t == LinkType::Normal && txt_e <= 1) {
            return (0, None);
        }
    }

    let link_node = match t {
        LinkType::Normal => {
            let node = bare_node(p, NodeType::Link);
            p.arena.get_mut(node).link.destination = normalize_uri(&u_link);
            p.arena.get_mut(node).link.title = title;
            if !alt_content.is_empty() {
                let t = text_node(p, &alt_content);
                p.arena.append_child(node, t);
            } else {
                // Links cannot nest, so turn link parsing off and recurse.
                let inside_link = p.inside_link;
                p.inside_link = true;
                let inner = data[1..txt_e].to_vec();
                p.inline(node, &inner);
                p.inside_link = inside_link;
            }
            node
        }
        LinkType::Img => {
            let node = bare_node(p, NodeType::Image);
            p.arena.get_mut(node).link.destination = u_link;
            p.arena.get_mut(node).link.title = title;
            let alt = data[1..txt_e].to_vec();
            let t = text_node(p, &alt);
            p.arena.append_child(node, t);
            i += 1;
            node
        }
        LinkType::InlineFootnote | LinkType::DeferredFootnote => {
            let node = bare_node(p, NodeType::Link);
            p.arena.get_mut(node).link.destination = link_dest;
            p.arena.get_mut(node).link.title = title;
            p.arena.get_mut(node).link.note_id = note_id;
            p.arena.get_mut(node).link.footnote = footnote_node;
            if t == LinkType::InlineFootnote {
                i += 1;
            }
            node
        }
    };

    (i, Some(link_node))
}

/// Builds a reference id from link text that spans lines.
///
/// The body of upstream's two identical inline loops: copy every byte, turning
/// each newline into a single space unless a space already precedes it.
fn collapse_newlines(data: &[u8], txt_e: usize, from: usize) -> Vec<u8> {
    let mut b = Vec::new();
    for j in from..txt_e {
        if data[j] != b'\n' {
            b.push(data[j]);
        } else if data[j - 1] != b' ' {
            b.push(b' ');
        }
    }
    b
}

/// `<`: an HTML tag or an autolink.
///
/// Ported from `leftAngle` (`inline.go:630`).
fn left_angle(p: &mut Markdown, data: &[u8], offset: usize) -> (usize, Option<NodeId>) {
    let data = &data[offset..];
    let (altype, mut end) = tag_length(data);
    let size = p.inline_html_comment(data);
    if size > 0 {
        end = size;
    }
    if end > 2 {
        if altype != AutolinkType::NotAutolink {
            let mut u_link = Vec::new();
            unescape_text(&mut u_link, &data[1..end - 1]);
            if !u_link.is_empty() {
                let node = bare_node(p, NodeType::Link);
                let destination = if altype == AutolinkType::EmailAutolink {
                    let mut d = b"mailto:".to_vec();
                    d.extend_from_slice(&u_link);
                    d
                } else {
                    u_link.clone()
                };
                p.arena.get_mut(node).link.destination = destination;
                let t = text_node(p, strip_mailto(&u_link));
                p.arena.append_child(node, t);
                return (end, Some(node));
            }
        } else {
            let html_tag = bare_node(p, NodeType::HTMLSpan);
            p.arena.get_mut(html_tag).literal = data[..end].to_vec();
            return (end, Some(html_tag));
        }
    }

    (end, None)
}

/// `\`: a backslash escape.
///
/// Ported from `escape` (`inline.go:663`). A trailing lone backslash reports
/// two bytes consumed even though only one exists, which pushes the caller's
/// cursor past the end and ends the scan.
fn escape(p: &mut Markdown, data: &[u8], offset: usize) -> (usize, Option<NodeId>) {
    let data = &data[offset..];

    if data.len() > 1 {
        if p.extensions.intersects(Extensions::BACKSLASH_LINE_BREAK) && data[1] == b'\n' {
            let br = bare_node(p, NodeType::Hardbreak);
            return (2, Some(br));
        }
        if !ESCAPE_CHARS.contains(&data[1]) {
            return (0, None);
        }

        let t = text_node(p, &data[1..2]);
        return (2, Some(t));
    }

    (2, None)
}

/// `&`: an entity, or a lone ampersand.
///
/// Ported from `entity` (`inline.go:703`). `&amp;` is collapsed back to a bare
/// `&` so the renderer's escaper does not turn it into `&amp;amp;`.
fn entity(p: &mut Markdown, data: &[u8], offset: usize) -> (usize, Option<NodeId>) {
    let data = &data[offset..];

    let mut end = 1;

    if end < data.len() && data[end] == b'#' {
        end += 1;
    }

    while end < data.len() && is_alnum(data[end]) {
        end += 1;
    }

    if end < data.len() && data[end] == b';' {
        end += 1; // a real entity
    } else {
        return (0, None); // a lone '&'
    }

    let ent: &[u8] = if &data[..end] == b"&amp;" {
        b"&"
    } else {
        &data[..end]
    };

    let t = text_node(p, ent);
    (end, Some(t))
}

/// `h`, `m`, `f` and their capitals: a bare URL, when one of the protocols
/// matches.
///
/// Ported from `maybeAutoLink` (`inline.go:765`).
fn maybe_auto_link(p: &mut Markdown, data: &[u8], offset: usize) -> (usize, Option<NodeId>) {
    /// The protocols a bare URL may start with.
    const PROTOCOL_PREFIXES: [&[u8]; 5] =
        [b"http://", b"https://", b"ftp://", b"file://", b"mailto:"];
    /// `len("ftp://")`, the shortest of them.
    const SHORTEST_PREFIX: usize = 6;

    // A cheap test first, to rule out most bytes.
    if p.inside_link || data.len() < offset + SHORTEST_PREFIX {
        return (0, None);
    }
    for prefix in PROTOCOL_PREFIXES {
        // 8 is the length of the longest prefix.
        let end_of_head = (offset + 8).min(data.len());
        if has_prefix_case_insensitive(&data[offset..end_of_head], prefix) {
            return auto_link(p, data, offset);
        }
    }
    (0, None)
}

/// Extends a bare URL backwards and forwards, and decides where it ends.
///
/// Ported from `autoLink` (`inline.go:782`).
fn auto_link(p: &mut Markdown, data: &[u8], offset: usize) -> (usize, Option<NodeId>) {
    // A more expensive check that this is not already inside an anchor.
    let mut anchor_start = offset;
    let mut offset_from_anchor = 0;
    while anchor_start > 0 && data[anchor_start] != b'<' {
        anchor_start -= 1;
        offset_from_anchor += 1;
    }

    if let Some(len) = anchor_match_len(&data[anchor_start..]) {
        let anchor_str = &data[anchor_start..anchor_start + len];
        let anchor_close = bare_node(p, NodeType::HTMLSpan);
        p.arena.get_mut(anchor_close).literal = anchor_str[offset_from_anchor..].to_vec();
        return (anchor_str.len() - offset_from_anchor, Some(anchor_close));
    }

    // Scan back to a word boundary.
    let mut rewind = 0;
    while offset - rewind > 0 && rewind <= 7 && is_letter(data[offset - rewind - 1]) {
        rewind += 1;
    }
    if rewind > 6 {
        // The longest protocol understood is "mailto", six letters.
        return (0, None);
    }

    let orig_data = data;
    let data = &data[offset - rewind..];

    if !crate::html::is_safe_link(data) {
        return (0, None);
    }

    let mut link_end = 0;
    while link_end < data.len() && !is_end_of_link(data[link_end]) {
        link_end += 1;
    }

    // Trailing punctuation is usually not part of the URL.
    if (data[link_end - 1] == b'.' || data[link_end - 1] == b',') && data[link_end - 2] != b'\\' {
        link_end -= 1;
    }

    // A semicolon may be the tail of an entity, though.
    if data[link_end - 1] == b';'
        && data[link_end - 2] != b'\\'
        && !link_ends_with_entity(data, link_end)
    {
        link_end -= 1;
    }

    // A closing bracket counts only if its opener is inside the URL. Upstream
    // spells the four cases out at length; the point is that
    // `http://x/Pikachu_(Electric)` keeps its parenthesis but
    // `(see http://x/page)` does not.
    let copen = match data[link_end - 1] {
        b'"' => b'"',
        b'\'' => b'\'',
        b')' => b'(',
        b']' => b'[',
        b'}' => b'{',
        _ => 0,
    };

    if copen != 0 {
        // Go's bufEnd is a signed int and the loop relies on it going
        // negative, so this has to be signed too. It cannot run off the top:
        // `data` is `orig_data[offset - rewind..]` and `link_end <= data.len()`.
        let mut buf_end = (offset - rewind + link_end) as isize - 2;
        let mut open_delim = 1;

        while buf_end >= 0 && orig_data[buf_end as usize] != b'\n' && open_delim != 0 {
            if orig_data[buf_end as usize] == data[link_end - 1] {
                open_delim += 1;
            }

            if orig_data[buf_end as usize] == copen {
                open_delim -= 1;
            }

            buf_end -= 1;
        }

        if open_delim == 0 {
            link_end -= 1;
        }
    }

    let mut u_link = Vec::new();
    unescape_text(&mut u_link, &data[..link_end]);

    if !u_link.is_empty() {
        let node = bare_node(p, NodeType::Link);
        p.arena.get_mut(node).link.destination = u_link.clone();
        let t = text_node(p, &u_link);
        p.arena.append_child(node, t);
        return (link_end, Some(node));
    }

    (link_end, None)
}

/// Single emphasis, `<em>`.
///
/// Ported from `helperEmphasis` (`inline.go:1112`).
fn helper_emphasis(p: &mut Markdown, data: &[u8], c: u8) -> (usize, Option<NodeId>) {
    let mut i = 0;

    // Skip one symbol when this was reached from the triple-emphasis path.
    if data.len() > 1 && data[0] == c && data[1] == c {
        i = 1;
    }

    while i < data.len() {
        let length = helper_find_emph_char(&data[i..], c);
        if length == 0 {
            return (0, None);
        }
        i += length;
        if i >= data.len() {
            return (0, None);
        }

        if i + 1 < data.len() && data[i + 1] == c {
            i += 1;
            continue;
        }

        if data[i] == c && !is_space(data[i - 1]) {
            if p.extensions.intersects(Extensions::NO_INTRA_EMPHASIS)
                && !(i + 1 == data.len() || is_space(data[i + 1]) || is_punct(data[i + 1]))
            {
                continue;
            }

            let emph = bare_node(p, NodeType::Emph);
            let inner = data[..i].to_vec();
            p.inline(emph, &inner);
            return (i + 1, Some(emph));
        }
    }

    (0, None)
}

/// Double emphasis: `<strong>`, or `<del>` for `~~`.
///
/// Ported from `helperDoubleEmphasis` (`inline.go:1152`).
fn helper_double_emphasis(p: &mut Markdown, data: &[u8], c: u8) -> (usize, Option<NodeId>) {
    let mut i = 0;

    while i < data.len() {
        let length = helper_find_emph_char(&data[i..], c);
        if length == 0 {
            return (0, None);
        }
        i += length;

        if i + 1 < data.len() && data[i] == c && data[i + 1] == c && i > 0 && !is_space(data[i - 1])
        {
            let node_type = if c == b'~' {
                NodeType::Del
            } else {
                NodeType::Strong
            };
            let node = bare_node(p, node_type);
            let inner = data[..i].to_vec();
            p.inline(node, &inner);
            return (i + 2, Some(node));
        }
        i += 1;
    }
    (0, None)
}

/// Triple emphasis: `<strong><em>`, falling back to the shorter forms.
///
/// Ported from `helperTripleEmphasis` (`inline.go:1176`).
fn helper_triple_emphasis(
    p: &mut Markdown,
    data: &[u8],
    offset: usize,
    c: u8,
) -> (usize, Option<NodeId>) {
    let mut i = 0;
    let orig_data = data;
    let data = &data[offset..];

    while i < data.len() {
        let length = helper_find_emph_char(&data[i..], c);
        if length == 0 {
            return (0, None);
        }
        i += length;

        // Skip a delimiter that whitespace precedes.
        if data[i] != c || is_space(data[i - 1]) {
            continue;
        }

        if i + 2 < data.len() && data[i + 1] == c && data[i + 2] == c {
            let strong = bare_node(p, NodeType::Strong);
            let em = bare_node(p, NodeType::Emph);
            p.arena.append_child(strong, em);
            let inner = data[..i].to_vec();
            p.inline(em, &inner);
            return (i + 3, Some(strong));
        } else if i + 1 < data.len() && data[i + 1] == c {
            // Two found; hand back to single emphasis.
            let (length, node) = helper_emphasis(p, &orig_data[offset - 2..], c);
            if length == 0 {
                return (0, None);
            }
            return (length - 2, node);
        } else {
            // One found; hand back to double emphasis.
            let (length, node) = helper_double_emphasis(p, &orig_data[offset - 1..], c);
            if length == 0 {
                return (0, None);
            }
            return (length - 1, node);
        }
    }
    (0, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/go-inline.txt");

    fn rows(tag: &str) -> impl Iterator<Item = Vec<String>> + '_ {
        FIXTURE.lines().filter_map(move |l| {
            let f: Vec<String> = l.split(' ').map(str::to_string).collect();
            (f.first().map(String::as_str) == Some(tag)).then_some(f)
        })
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn field(f: &[String], i: usize) -> &str {
        f.get(i).map(String::as_str).unwrap_or("")
    }

    #[test]
    fn unescape_text_matches_go() {
        let mut n = 0;
        for f in rows("U") {
            let input = unhex(field(&f, 1));
            let want = unhex(field(&f, 2));
            let mut got = Vec::new();
            unescape_text(&mut got, &input);
            assert_eq!(got, want, "unescape {:?}", String::from_utf8_lossy(&input));
            n += 1;
        }
        assert!(n >= 14, "thin corpus: {n}");
    }

    #[test]
    fn strip_mailto_matches_go() {
        let mut n = 0;
        for f in rows("M") {
            let input = unhex(field(&f, 1));
            let want = unhex(field(&f, 2));
            assert_eq!(strip_mailto(&input), &want[..]);
            n += 1;
        }
        assert!(n >= 8, "thin corpus: {n}");
    }

    #[test]
    fn case_insensitive_prefix_matches_go_including_the_overflow() {
        let mut n = 0;
        for f in rows("H") {
            let s = unhex(field(&f, 1));
            let p = unhex(field(&f, 2));
            assert_eq!(
                has_prefix_case_insensitive(&s, &p),
                f[3] == "true",
                "prefix {:?} of {:?}",
                String::from_utf8_lossy(&p),
                String::from_utf8_lossy(&s)
            );
            n += 1;
        }
        assert!(n >= 12, "thin corpus: {n}");
        // The rows above include bytes past 0xE5, where Go's `s[i]+delta`
        // wraps. A plain `+` would panic here in a debug build.
        assert!(!has_prefix_case_insensitive(
            b"\xff\xfe\xfd\xfc\xfb\xfa\xf9",
            b"http://"
        ));
    }

    #[test]
    fn is_end_of_link_matches_go() {
        let want = unhex(field(&rows("E").next().unwrap(), 1));
        let got: Vec<u8> = (0..=255u8).filter(|&c| is_end_of_link(c)).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn tag_length_matches_go() {
        let mut n = 0;
        for f in rows("T") {
            let input = unhex(field(&f, 1));
            let want_type: i32 = f[2].parse().unwrap();
            let want_end: usize = f[3].parse().unwrap();
            let (at, end) = tag_length(&input);
            assert_eq!(
                at as i32,
                want_type,
                "type for {:?}",
                String::from_utf8_lossy(&input)
            );
            assert_eq!(
                end,
                want_end,
                "end for {:?}",
                String::from_utf8_lossy(&input)
            );
            n += 1;
        }
        assert!(n >= 45, "thin corpus: {n}");
    }

    #[test]
    fn an_unclosed_tag_reports_its_offset_rather_than_failing() {
        // The unguarded IndexByte, pinned. A correct implementation would
        // return 0 here.
        let (at, end) = tag_length(b"<no closing");
        assert_eq!(at, AutolinkType::NotAutolink);
        assert_ne!(end, 0, "upstream does not fail on a missing '>'");
    }

    #[test]
    fn is_mailto_auto_link_matches_go() {
        let mut n = 0;
        for f in rows("A") {
            let input = unhex(field(&f, 1));
            let want: usize = f[2].parse().unwrap();
            assert_eq!(
                is_mailto_auto_link(&input),
                want,
                "{:?}",
                String::from_utf8_lossy(&input)
            );
            n += 1;
        }
        assert!(n >= 12, "thin corpus: {n}");
    }

    #[test]
    fn is_safe_link_matches_gos_inline_definition() {
        // isSafeLink lives in inline.go but the renderer uses it too, so the
        // port keeps one copy in `html`. This checks that copy against the
        // measurements taken here.
        let mut n = 0;
        for f in rows("S") {
            let input = unhex(field(&f, 1));
            assert_eq!(
                crate::html::is_safe_link(&input),
                f[2] == "true",
                "{:?}",
                String::from_utf8_lossy(&input)
            );
            n += 1;
        }
        assert!(n >= 20, "thin corpus: {n}");
    }

    #[test]
    fn helper_find_emph_char_matches_go() {
        let mut n = 0;
        for f in rows("F") {
            let input = unhex(field(&f, 1));
            let c: u8 = f[2].parse().unwrap();
            let want: usize = f[3].parse().unwrap();
            assert_eq!(
                helper_find_emph_char(&input, c),
                want,
                "find {:?} in {:?}",
                c as char,
                String::from_utf8_lossy(&input)
            );
            n += 1;
        }
        assert!(n >= 20, "thin corpus: {n}");
    }

    #[test]
    fn html_entity_matching_matches_gos_regexp() {
        let mut n = 0;
        for f in rows("N") {
            let input = unhex(field(&f, 1));
            let want: Vec<(usize, usize)> = if f[2] == "-" {
                Vec::new()
            } else {
                f[2].split(',')
                    .filter(|s| !s.is_empty())
                    .map(|pair| {
                        let (a, b) = pair.split_once(':').unwrap();
                        (a.parse().unwrap(), b.parse().unwrap())
                    })
                    .collect()
            };
            assert_eq!(
                find_html_entities(&input),
                want,
                "entities in {:?}",
                String::from_utf8_lossy(&input)
            );
            n += 1;
        }
        assert!(n >= 30, "thin corpus: {n}");
    }

    #[test]
    fn link_ends_with_entity_matches_go() {
        let mut n = 0;
        for f in rows("L") {
            let input = unhex(field(&f, 1));
            let cut: usize = f[2].parse().unwrap();
            assert_eq!(
                link_ends_with_entity(&input, cut),
                f[3] == "true",
                "cut {cut} of {:?}",
                String::from_utf8_lossy(&input)
            );
            n += 1;
        }
        assert!(n >= 200, "thin corpus: {n}");
    }

    #[test]
    fn anchor_matching_matches_gos_regexp() {
        let mut n = 0;
        for f in rows("R") {
            let input = unhex(field(&f, 1));
            let want = (f[2] != "-").then(|| unhex(field(&f, 2)));
            let got = anchor_match_len(&input).map(|len| input[..len].to_vec());
            assert_eq!(
                got.as_deref().map(String::from_utf8_lossy),
                want.as_deref().map(String::from_utf8_lossy),
                "anchor in {:?}",
                String::from_utf8_lossy(&input)
            );
            n += 1;
        }
        assert!(n >= 18, "thin corpus: {n}");
    }

    #[test]
    fn escape_chars_match_go() {
        let want = unhex(field(&rows("C").next().unwrap(), 1));
        assert_eq!(ESCAPE_CHARS, &want[..]);
    }

    #[test]
    fn normalize_uri_is_upstreams_todo() {
        assert_eq!(normalize_uri(b"http://x/ y"), b"http://x/ y");
    }
}
