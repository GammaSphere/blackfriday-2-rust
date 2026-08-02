//! Block-level parsing, ported from upstream `block.go`.
//!
//! # Several scanners here can panic, and that is faithful
//!
//! `is_hrule`, `is_prefix_heading` and `is_underlined_heading` index their
//! input without a bounds check, exactly as upstream does. Measured against
//! blackfriday v2.1.0, these six calls panic in Go:
//!
//! | Function | Input |
//! |---|---|
//! | `isHRule` | `""`, `" "`, `"  "`, `"   "` |
//! | `isPrefixHeading` | `""` |
//! | `isUnderlinedHeading` | `""` |
//!
//! The `isHRule` space cases are the interesting ones: the leading-space skip
//! is `for i < 3 && data[i] == ' '`, which has no length guard, so one to three
//! spaces with nothing after them runs off the end.
//!
//! Rust's slice indexing panics on the same inputs for the same reason, so a
//! direct transcription is already equivalent. The tests assert the panics
//! rather than papering over them: returning `false` instead would be a
//! *divergence*, and a silent one. Whether any of this is reachable through the
//! public `Run` entry point is a separate question, left to the differential
//! fuzzing stage.

use crate::markdown::Markdown;
use crate::node::{Arena, NodeId, NodeType};
use crate::unicode_tables::{is_letter_or_number, simple_to_lower};

/// Advances past a run of `ch` starting at `start`.
///
/// Ported from `skipChar` (`block.go:1577`).
pub(crate) fn skip_char(data: &[u8], start: usize, ch: u8) -> usize {
    let mut i = start;
    while i < data.len() && data[i] == ch {
        i += 1;
    }
    i
}

/// Advances to the next `ch`, or to the end of `data`.
///
/// Ported from `skipUntilChar` (`block.go:1585`).
pub(crate) fn skip_until_char(data: &[u8], start: usize, ch: u8) -> usize {
    let mut i = start;
    while i < data.len() && data[i] != ch {
        i += 1;
    }
    i
}

/// Whether position `i` is preceded by an odd number of backslashes.
///
/// Ported from `isBackslashEscaped` (`block.go:778`). Go's guard is
/// `i-backslashes-1 >= 0` on signed ints; on `usize` that underflows rather
/// than going negative, so it is spelled `i > backslashes` instead.
pub(crate) fn is_backslash_escaped(data: &[u8], i: usize) -> bool {
    let mut backslashes = 0usize;
    while i > backslashes && data[i - backslashes - 1] == b'\\' {
        backslashes += 1;
    }
    backslashes & 1 == 1
}

/// Length of a leading blank line, or `0` if the line is not blank.
///
/// Ported from `isEmpty` (`block.go:522`). Only spaces and tabs count as blank;
/// the returned length includes the terminating newline when there is one.
/// Unlike its neighbours this one is safe on empty input, and upstream says so.
pub(crate) fn is_empty(data: &[u8]) -> usize {
    if data.is_empty() {
        return 0;
    }
    let mut i = 0;
    while i < data.len() && data[i] != b'\n' {
        if data[i] != b' ' && data[i] != b'\t' {
            return 0;
        }
        i += 1;
    }
    if i < data.len() && data[i] == b'\n' {
        i += 1;
    }
    i
}

/// Whether `data` begins a horizontal rule.
///
/// Ported from `isHRule` (`block.go:540`). Needs at least three of the same
/// marker; only spaces may appear between them.
///
/// # Panics
///
/// On `""`, `" "`, `"  "` and `"   "`, matching upstream. See the module docs.
pub(crate) fn is_hrule(data: &[u8]) -> bool {
    let mut i = 0usize;

    // Skip up to three spaces. Upstream has no length guard here, which is
    // what makes an all-spaces input of length <= 3 panic.
    while i < 3 && data[i] == b' ' {
        i += 1;
    }

    if data[i] != b'*' && data[i] != b'-' && data[i] != b'_' {
        return false;
    }
    let c = data[i];

    // The whole line must be that character or whitespace.
    let mut n = 0usize;
    while i < data.len() && data[i] != b'\n' {
        if data[i] == c {
            n += 1;
        } else if data[i] != b' ' {
            return false;
        }
        i += 1;
    }

    n >= 3
}

/// The heading level of an underlined (setext) heading, or `0`.
///
/// Ported from `isUnderlinedHeading` (`block.go:270`).
///
/// # Panics
///
/// On empty input, matching upstream.
pub(crate) fn is_underlined_heading(data: &[u8]) -> i32 {
    if data[0] == b'=' {
        let i = skip_char(data, 1, b'=');
        let i = skip_char(data, i, b' ');
        return if i < data.len() && data[i] == b'\n' {
            1
        } else {
            0
        };
    }

    if data[0] == b'-' {
        let i = skip_char(data, 1, b'-');
        let i = skip_char(data, i, b' ');
        return if i < data.len() && data[i] == b'\n' {
            2
        } else {
            0
        };
    }

    0
}

/// Decodes the first UTF-8 character of `data`, with its length in bytes.
///
/// Returns `None` at the end of input or on an invalid sequence. Go's
/// `utf8.DecodeRune` yields `RuneError` for an invalid byte; since every caller
/// here only asks "is this whitespace?", and `RuneError` is not whitespace,
/// reporting `None` reaches the same decision without a full decoder.
fn first_char(data: &[u8]) -> Option<(char, usize)> {
    for n in 1..=4.min(data.len()) {
        if let Ok(s) = std::str::from_utf8(&data[..n]) {
            if let Some(c) = s.chars().next() {
                return Some((c, n));
            }
        }
    }
    None
}

/// Decodes the last UTF-8 character of `data`, with its length in bytes.
fn last_char(data: &[u8]) -> Option<(char, usize)> {
    for n in 1..=4.min(data.len()) {
        if let Ok(s) = std::str::from_utf8(&data[data.len() - n..]) {
            if let Some(c) = s.chars().next() {
                return Some((c, n));
            }
        }
    }
    None
}

/// Go's `strings.TrimSpace`, over bytes.
///
/// Operating on bytes rather than `&str` matters: the info string is a slice of
/// the document and need not be valid UTF-8. Going through
/// `String::from_utf8_lossy` would replace interior invalid bytes with U+FFFD,
/// which Go does not do — Go's `string([]byte)` conversion is a reinterpretation,
/// not a validation, so those bytes survive untouched.
///
/// `char::is_whitespace` is the Unicode `White_Space` property, and Go's
/// `unicode.IsSpace` is the same set: its Latin-1 fast path lists exactly the
/// `White_Space` members below U+0100, and it defers to the `White_Space` table
/// above that.
fn trim_space(data: &[u8]) -> &[u8] {
    let mut s = data;
    while let Some((c, n)) = first_char(s) {
        if !c.is_whitespace() {
            break;
        }
        s = &s[n..];
    }
    while let Some((c, n)) = last_char(s) {
        if !c.is_whitespace() {
            break;
        }
        s = &s[..s.len() - n];
    }
    s
}

/// Detects a code fence at the start of `data`.
///
/// Ported from `isFenceLine` (`block.go:572`). Returns the index just past the
/// fence line (including its newline) and the marker run that opened it, or
/// `(0, empty)` when there is no fence.
///
/// `info` mirrors Go's `info *string`: passing `None` skips the entire
/// info-string branch, which is not merely an optimisation — it changes which
/// `return` is reached, and therefore the end index, for input that ends
/// immediately after the marker.
///
/// `old_marker` is the opening marker when looking for a closing fence; a
/// non-empty value that does not match rejects the line. This is one of the two
/// unexported functions the pinned suite calls directly
/// (`TestIsFenceLine`, `block_test.go:1864`).
pub fn is_fence_line(
    data: &[u8],
    info: Option<&mut Vec<u8>>,
    old_marker: &[u8],
) -> (usize, Vec<u8>) {
    let mut i = 0usize;
    let mut size = 0usize;

    // Skip up to three spaces. Unlike is_hrule, this one is length-guarded.
    while i < data.len() && i < 3 && data[i] == b' ' {
        i += 1;
    }

    if i >= data.len() {
        return (0, Vec::new());
    }
    if data[i] != b'~' && data[i] != b'`' {
        return (0, Vec::new());
    }
    let c = data[i];

    while i < data.len() && data[i] == c {
        size += 1;
        i += 1;
    }

    if size < 3 {
        return (0, Vec::new());
    }
    let marker = data[i - size..i].to_vec();

    // A closing fence must use the same marker as the opening one.
    if !old_marker.is_empty() && marker != old_marker {
        return (0, Vec::new());
    }

    if let Some(info_out) = info {
        let mut info_length = 0usize;
        i = skip_char(data, i, b' ');

        if i >= data.len() {
            // Go writes `if i >= len(data) { if i == len(data) {...}; return 0 }`.
            // `i > len(data)` is unreachable, so the inner test is always true.
            return (i, marker);
        }

        let mut info_start = i;

        if data[i] == b'{' {
            i += 1;
            info_start += 1;

            while i < data.len() && data[i] != b'}' && data[i] != b'\n' {
                info_length += 1;
                i += 1;
            }

            if i >= data.len() || data[i] != b'}' {
                return (0, Vec::new());
            }

            // Strip whitespace inside the braces.
            while info_length > 0 && crate::util::is_space(data[info_start]) {
                info_start += 1;
                info_length -= 1;
            }
            while info_length > 0 && crate::util::is_space(data[info_start + info_length - 1]) {
                info_length -= 1;
            }
            i += 1;
            i = skip_char(data, i, b' ');
        } else {
            while i < data.len() && !crate::util::is_vertical_space(data[i]) {
                info_length += 1;
                i += 1;
            }
        }

        info_out.clear();
        info_out.extend_from_slice(trim_space(&data[info_start..info_start + info_length]));
    }

    if i == data.len() {
        return (i, marker);
    }
    // Go also tests `i > len(data)` here, which cannot happen.
    if data[i] != b'\n' {
        return (0, Vec::new());
    }
    (i + 1, marker) // Take the newline into account.
}

/// The character class upstream calls `escapable` (`block.go:26`).
///
/// The pattern is `[!"#$%&'()*+,./:;<=>?@[\\\]^_`{|}~-]`, which turns out to be
/// exactly the 32 ASCII punctuation characters — the same set as
/// [`crate::util::is_punct`]. A test asserts that equivalence rather than
/// trusting it.
#[inline]
fn is_escapable(c: u8) -> bool {
    crate::util::is_punct(c)
}

/// Matches upstream's `charEntity` at `data[i..]`, returning its length.
///
/// The pattern is `&(?:#x[a-f0-9]{1,8}|#[0-9]{1,8}|[a-z][a-z0-9]{1,31});` under
/// `(?i)`, so the hex digits and the name are both case-insensitive.
fn char_entity_len(data: &[u8], i: usize) -> Option<usize> {
    if data.get(i) != Some(&b'&') {
        return None;
    }
    let mut j = i + 1;

    let count = if data.get(j) == Some(&b'#') {
        j += 1;
        if matches!(data.get(j), Some(&b'x') | Some(&b'X')) {
            j += 1;
            let start = j;
            while j < data.len() && j - start < 8 && data[j].is_ascii_hexdigit() {
                j += 1;
            }
            j - start
        } else {
            let start = j;
            while j < data.len() && j - start < 8 && data[j].is_ascii_digit() {
                j += 1;
            }
            j - start
        }
    } else {
        // [a-z][a-z0-9]{1,31}: at least two characters total.
        if !data.get(j).is_some_and(u8::is_ascii_alphabetic) {
            return None;
        }
        j += 1;
        let start = j;
        while j < data.len() && j - start < 31 && data[j].is_ascii_alphanumeric() {
            j += 1;
        }
        if j - start < 1 {
            return None;
        }
        j - start
    };

    if count == 0 {
        return None;
    }
    if data.get(j) != Some(&b';') {
        return None;
    }
    Some(j + 1 - i)
}

/// Expands one match of `reEntityOrEscapedChar`.
///
/// Ported from `unescapeChar` (`block.go:716`).
fn unescape_char(s: &[u8]) -> Vec<u8> {
    if s[0] == b'\\' {
        return vec![s[1]];
    }
    crate::unescape::unescape_string(s)
}

/// Expands backslash escapes and character entities.
///
/// Ported from `unescapeString` (`block.go:723`). **This reproduces an upstream
/// bug on purpose**; see `BUGS.md`.
///
/// Upstream guards the work with `reBackslashOrAmp`, declared as
/// `regexp.MustCompile("[\\&]")`. That pattern is `[\&]`, and inside an RE2
/// character class a backslash escapes the next byte — so the class contains
/// only `&`, never a backslash, despite the name. A string holding a backslash
/// escape but no ampersand therefore skips the replacement entirely:
///
/// ```
/// # use blackfriday::block::unescape_string;
/// // The escape is left alone...
/// assert_eq!(unescape_string(br"\-go"), br"\-go");
/// // ...but the same escape is expanded when an unrelated & appears.
/// assert_eq!(unescape_string(br"\-go&amp;x"), b"-go&x");
/// ```
///
/// Fixing it here would be a divergence, so the guard is written to match the
/// class upstream actually compiled.
pub fn unescape_string(s: &[u8]) -> Vec<u8> {
    // reBackslashOrAmp: the class is `&` only. Writing `|| c == b'\\'` here
    // would "fix" the bug and break equivalence.
    if !s.contains(&b'&') {
        return s.to_vec();
    }

    let mut out = Vec::with_capacity(s.len());
    let mut i = 0usize;
    while i < s.len() {
        // reEntityOrEscapedChar, first alternative: \\ followed by escapable.
        if s[i] == b'\\' && i + 1 < s.len() && is_escapable(s[i + 1]) {
            out.extend_from_slice(&unescape_char(&s[i..i + 2]));
            i += 2;
            continue;
        }
        // Second alternative: a character entity.
        if let Some(len) = char_entity_len(s, i) {
            out.extend_from_slice(&unescape_char(&s[i..i + len]));
            i += len;
            continue;
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

/// Splits a finished code block's staging buffer into its info string and body.
///
/// Ported from `finalizeCodeBlock` (`block.go:730`). Fenced blocks arrive with
/// the info string on the first line — [`Markdown::fenced_code_block`] writes it
/// there — so this splits at the first newline and unescapes the front half.
/// Indented blocks have no info line and take the buffer verbatim.
///
/// # Panics
///
/// If a fenced block's content holds no newline. Go indexes with the `-1` that
/// `bytes.IndexByte` returns and panics identically. Both are unreachable in
/// practice: the only producer always writes `info` followed by `'\n'`.
fn finalize_code_block(arena: &mut Arena, block: NodeId) {
    if arena[block].code_block.is_fenced {
        let content = std::mem::take(&mut arena[block].content);
        let newline_pos = content
            .iter()
            .position(|&c| c == b'\n')
            .expect("fenced code block content always begins with an info line");
        let first_line = &content[..newline_pos];
        let rest = content[newline_pos + 1..].to_vec();

        // bytes.Trim(firstLine, "\n") strips newlines from both ends. It cannot
        // do anything here, since first_line stops at the first newline, but it
        // is kept so the port does not quietly depend on that reasoning.
        let trimmed: &[u8] = {
            let mut s = first_line;
            while let Some((&b'\n', tail)) = s.split_first() {
                s = tail;
            }
            while let Some((&b'\n', head)) = s.split_last() {
                s = head;
            }
            s
        };

        arena[block].code_block.info = unescape_string(trimmed);
        arena[block].literal = rest;
    } else {
        arena[block].literal = std::mem::take(&mut arena[block].content);
    }
    arena[block].content = Vec::new();
}

/// Length of an unordered list item prefix, or `0`.
///
/// Ported from `uliPrefix` (`block.go:1068`). Needs one of `*`, `+` or `-`
/// followed by a space or tab, after at most three leading spaces.
///
/// Go's guard is `if i >= len(data)-1`, on signed ints, so an empty slice makes
/// it `0 >= -1` and returns early. On `usize` that subtraction underflows, so
/// it is written `i + 1 >= data.len()`, which agrees on every length.
pub(crate) fn uli_prefix(data: &[u8]) -> usize {
    let mut i = 0usize;
    while i < data.len() && i < 3 && data[i] == b' ' {
        i += 1;
    }
    if i + 1 >= data.len() {
        return 0;
    }
    if (data[i] != b'*' && data[i] != b'+' && data[i] != b'-')
        || (data[i + 1] != b' ' && data[i + 1] != b'\t')
    {
        return 0;
    }
    i + 2
}

/// Length of an ordered list item prefix, or `0`.
///
/// Ported from `oliPrefix` (`block.go:1086`). Digits then a `.` then a space or
/// tab; `1) x` is not an ordered item.
pub(crate) fn oli_prefix(data: &[u8]) -> usize {
    let mut i = 0usize;
    while i < 3 && i < data.len() && data[i] == b' ' {
        i += 1;
    }

    let start = i;
    while i < data.len() && data[i].is_ascii_digit() {
        i += 1;
    }
    // Same signed-versus-usize care as uli_prefix.
    if start == i || i + 1 >= data.len() {
        return 0;
    }

    if data[i] != b'.' || !(data[i + 1] == b' ' || data[i + 1] == b'\t') {
        return 0;
    }
    i + 2
}

/// Length of a definition list item prefix, or `0`.
///
/// Ported from `dliPrefix` (`block.go:1111`). Returns only ever `0` or `2`.
///
/// Upstream ends with a loop that looks like it should skip spaces:
///
/// ```go
/// for i < len(data) && data[i] == ' ' {
///     i++
/// }
/// return i + 2
/// ```
///
/// but `i` is still `0` there and `data[0]` has just been established to be
/// `':'`, so the condition is false on the first test and the loop never runs.
/// It is dead code, and the measured results confirm the function returns
/// nothing but `0` and `2`. Ported as written rather than "corrected", since
/// making it skip spaces would change the prefix length.
pub(crate) fn dli_prefix(data: &[u8]) -> usize {
    if data.len() < 2 {
        return 0;
    }
    let mut i = 0usize;
    if data[i] != b':' || !(data[i + 1] == b' ' || data[i + 1] == b'\t') {
        return 0;
    }
    // Vestigial in upstream: data[0] is ':' so this never advances.
    while i < data.len() && data[i] == b' ' {
        i += 1;
    }
    i + 2
}

/// Whether the item at `data` differs in type from the list holding it.
///
/// Ported from `listTypeChanged` (`block.go:1153`).
pub(crate) fn list_type_changed(data: &[u8], flags: crate::ListType) -> bool {
    use crate::ListType;
    // Go writes this as an if / else-if chain whose arms all return true, which
    // is an OR. `||` short-circuits the same way the chain does, so later
    // prefixes are still only scanned when the earlier ones did not match.
    (dli_prefix(data) > 0 && !flags.intersects(ListType::DEFINITION))
        || (oli_prefix(data) > 0 && !flags.intersects(ListType::ORDERED))
        || (uli_prefix(data) > 0
            && (flags.intersects(ListType::ORDERED) || flags.intersects(ListType::DEFINITION)))
}

/// Whether a block ends with a blank line.
///
/// Ported from `endsWithBlankLine` (`block.go:1166`), where the body that would
/// answer the question is commented out behind a `TODO: figure this out. Always
/// false now.` The loop walks down `LastChild` through lists and items and then
/// returns `false` regardless.
///
/// This is preserved rather than implemented. Making it work would change how
/// [`finalize_list`] sets `tight`, and therefore the rendered output — a
/// divergence, and one the pinned suite would not catch since it encodes the
/// current behaviour. Measured across `List`, `Item`, `Paragraph`, `CodeBlock`
/// and `Document`: always `false`.
pub(crate) fn ends_with_blank_line(arena: &Arena, block: NodeId) -> bool {
    let mut cur = Some(block);
    while let Some(id) = cur {
        // Upstream checks `block.lastLineBlank` here; it is commented out.
        match arena[id].node_type {
            NodeType::List | NodeType::Item => cur = arena[id].last_child(),
            _ => break,
        }
    }
    false
}

/// Closes a list, deciding whether it renders tight.
///
/// Ported from `finalizeList` (`block.go:1182`). Because
/// [`ends_with_blank_line`] is stubbed to `false` upstream, neither branch that
/// would clear `tight` can fire, so in practice this only closes the node. The
/// structure is kept so the port tracks upstream if that `TODO` is ever
/// resolved.
pub(crate) fn finalize_list(arena: &mut Arena, block: NodeId) {
    arena[block].open = false;

    let mut item = arena[block].first_child();
    while let Some(it) = item {
        if ends_with_blank_line(arena, it) && arena[it].next().is_some() {
            arena[block].list.tight = false;
            break;
        }
        let mut sub_item = arena[it].first_child();
        while let Some(sub) = sub_item {
            if ends_with_blank_line(arena, sub)
                && (arena[it].next().is_some() || arena[sub].next().is_some())
            {
                arena[block].list.tight = false;
                break;
            }
            sub_item = arena[sub].next();
        }
        item = arena[it].next();
    }
}

/// Go's `bytes.Replace(s, old, new, -1)`.
fn replace_all(s: &[u8], old: &[u8], new: &[u8]) -> Vec<u8> {
    if old.is_empty() {
        return s.to_vec();
    }
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0usize;
    while i < s.len() {
        if s[i..].starts_with(old) {
            out.extend_from_slice(new);
            i += old.len();
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

/// Tags that open a preformatted HTML block.
///
/// Ported from the `blockTags` map (`markdown.go`). Sorted for binary search.
static BLOCK_TAGS: [&str; 38] = [
    "address",
    "article",
    "aside",
    "blockquote",
    "canvas",
    "del",
    "div",
    "dl",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "iframe",
    "ins",
    "main",
    "math",
    "nav",
    "noscript",
    "ol",
    "output",
    "p",
    "pre",
    "progress",
    "script",
    "section",
    "style",
    "table",
    "ul",
    "video",
];

/// Moves a finished HTML block's staging content into its literal.
///
/// Ported from `finalizeHTMLBlock` (`block.go:416`).
fn finalize_html_block(arena: &mut Arena, block: NodeId) {
    arena[block].literal = std::mem::take(&mut arena[block].content);
    arena[block].content = Vec::new();
}

/// Reads a block tag name from the start of `data`.
///
/// Ported from `htmlFindTag` (`block.go:475`). The lookup is case-sensitive, so
/// `DIV` is not a block tag even though `div` is.
pub(crate) fn html_find_tag(data: &[u8]) -> Option<&str> {
    let mut i = 0usize;
    while i < data.len() && crate::util::is_alnum(data[i]) {
        i += 1;
    }
    let key = std::str::from_utf8(&data[..i]).ok()?;
    BLOCK_TAGS.binary_search(&key).ok().map(|n| BLOCK_TAGS[n])
}

/// Finds where a preformatted HTML block ends.
///
/// Ported from `htmlFindEnd` (`block.go:487`). Assumes `data` starts with `</`.
/// Without [`crate::Extensions::LAX_HTML_BLOCKS`] the closing tag must be
/// followed by *two* blank lines' worth of emptiness, not one.
pub(crate) fn html_find_end(tag: &str, data: &[u8], lax: bool) -> usize {
    // `hr` is self-closing and handled elsewhere; upstream short-circuits it.
    if tag == "hr" {
        return 2;
    }
    let closetag = format!("</{tag}>");
    if !data.starts_with(closetag.as_bytes()) {
        return 0;
    }
    let mut i = closetag.len();

    let skip = is_empty(&data[i..]);
    if skip == 0 {
        return 0;
    }
    i += skip;

    if i >= data.len() {
        return i;
    }
    if lax {
        return i;
    }
    let skip = is_empty(&data[i..]);
    if skip == 0 {
        return 0;
    }
    i + skip
}

/// Length of a blockquote prefix, or `0`.
///
/// Ported from `quotePrefix` (`block.go:943`). A single space after the `>` is
/// consumed as part of the prefix; a second one is not.
pub(crate) fn quote_prefix(data: &[u8]) -> usize {
    let mut i = 0usize;
    while i < 3 && i < data.len() && data[i] == b' ' {
        i += 1;
    }
    if i < data.len() && data[i] == b'>' {
        if i + 1 < data.len() && data[i + 1] == b' ' {
            return i + 2;
        }
        return i + 1;
    }
    0
}

/// Length of an indented-code prefix, or `0`.
///
/// Ported from `codePrefix` (`block.go:1008`). A tab counts as one byte of
/// prefix; spaces must number exactly four.
pub(crate) fn code_prefix(data: &[u8]) -> usize {
    if !data.is_empty() && data[0] == b'\t' {
        return 1;
    }
    if data.len() >= 4 && &data[..4] == b"    " {
        return 4;
    }
    0
}

/// Whether a blockquote ends at this point.
///
/// Ported from `terminateBlockquote` (`block.go:959`): a blank line followed by
/// something that is neither a quote prefix nor another blank line.
pub(crate) fn terminate_blockquote(data: &[u8], beg: usize, end: usize) -> bool {
    if is_empty(&data[beg..]) == 0 {
        return false;
    }
    if end >= data.len() {
        return true;
    }
    quote_prefix(&data[end..]) == 0 && is_empty(&data[end..]) == 0
}

impl Markdown {
    /// Appends a block of `typ` holding `content`, closing anything unmatched.
    ///
    /// Ported from `addBlock` (`block.go:200`). Upstream passes an `offset`
    /// argument to `addChild` that `addChild` ignores entirely, so it is
    /// dropped here rather than carried as a parameter nothing reads.
    pub(crate) fn add_block(&mut self, typ: NodeType, content: &[u8]) -> NodeId {
        self.close_unmatched_blocks();
        let container = self.add_child(typ);
        self.arena[container].content = content.to_vec();
        container
    }

    /// Parses a fenced code block, returning how much input it consumed.
    ///
    /// Ported from `fencedCodeBlock` (`block.go:669`). Returns `0` when there is
    /// no complete fenced block; with `do_render` false it has no side effects,
    /// which is how the paragraph scanner peeks ahead without committing.
    ///
    /// A closing fence is mandatory: running to the end of the buffer without
    /// one gives `0`, not a block that swallows the remainder.
    ///
    /// Note `fence_length` is the length of the whole opening *line*, not the
    /// marker run — `beg - 1` where `beg` is just past the newline. So
    /// ```` ```go ```` yields 5, not 3. That is upstream's meaning and the HTML
    /// renderer never reads it, but it is preserved for any custom renderer
    /// that does.
    pub(crate) fn fenced_code_block(&mut self, data: &[u8], do_render: bool) -> usize {
        let mut info = Vec::new();
        let (mut beg, marker) = is_fence_line(data, Some(&mut info), b"");
        if beg == 0 || beg >= data.len() {
            return 0;
        }
        let fence_length = beg - 1;

        let mut work: Vec<u8> = Vec::new();
        work.extend_from_slice(&info);
        work.push(b'\n');

        loop {
            // Safe to assume beg < data.len() here.
            let (fence_end, _) = is_fence_line(&data[beg..], None, &marker);
            if fence_end != 0 {
                beg += fence_end;
                break;
            }

            let end = skip_until_char(data, beg, b'\n') + 1;

            // Ran out of input without a closing marker.
            if end >= data.len() {
                return 0;
            }

            if do_render {
                work.extend_from_slice(&data[beg..end]);
            }
            beg = end;
        }

        if do_render {
            let block = self.add_block(NodeType::CodeBlock, &work);
            self.arena[block].code_block.is_fenced = true;
            self.arena[block].code_block.fence_length = fence_length;
            finalize_code_block(&mut self.arena, block);
        }

        beg
    }

    /// Parses an indented code block, returning how much input it consumed.
    ///
    /// Ported from `code` (`block.go:1018`). Blank lines are kept as bare
    /// newlines and do not end the block; the first non-blank line without an
    /// indent prefix does.
    ///
    /// Trailing newlines are stripped and exactly one is re-appended, so the
    /// literal always ends with a single newline regardless of how many blank
    /// lines trailed the source.
    pub(crate) fn code(&mut self, data: &[u8]) -> usize {
        let mut work: Vec<u8> = Vec::new();

        let mut i = 0usize;
        while i < data.len() {
            let mut beg = i;
            while i < data.len() && data[i] != b'\n' {
                i += 1;
            }
            if i < data.len() && data[i] == b'\n' {
                i += 1;
            }

            let blankline = is_empty(&data[beg..i]) > 0;
            let pre = code_prefix(&data[beg..i]);
            if pre > 0 {
                beg += pre;
            } else if !blankline {
                // A non-empty, non-prefixed line ends the block.
                i = beg;
                break;
            }

            if blankline {
                work.push(b'\n');
            } else {
                work.extend_from_slice(&data[beg..i]);
            }
        }

        let mut eol = work.len();
        while eol > 0 && work[eol - 1] == b'\n' {
            eol -= 1;
        }
        work.truncate(eol);
        work.push(b'\n');

        let block = self.add_block(NodeType::CodeBlock, &work);
        self.arena[block].code_block.is_fenced = false;
        finalize_code_block(&mut self.arena, block);

        i
    }

    /// Parses a pandoc-style title block.
    ///
    /// Ported from `titleBlock` (`block.go:294`). **Reproduces an upstream bug**;
    /// see `BUGS.md` §2.
    ///
    /// Go only assigns the scan index inside the loop, so when *every* line
    /// starts with `%` the loop never breaks and the index stays `0`. The joined
    /// data is then empty and `consumed` is `0` — yet `addBlock` runs anyway, so
    /// a stray empty `Heading` is appended and `block()` falls through to the
    /// paragraph handler. `% a\n` is fine; `% a` without the newline is not.
    ///
    /// `do_render` is accepted and ignored, exactly as upstream does — unlike
    /// [`Markdown::fenced_code_block`], this one always mutates the tree.
    ///
    /// # Panics
    ///
    /// On empty input, matching upstream's unguarded `data[0]`.
    pub(crate) fn title_block(&mut self, data: &[u8], _do_render: bool) -> usize {
        if data[0] != b'%' {
            return 0;
        }
        let split: Vec<&[u8]> = data.split(|&c| c == b'\n').collect();

        // Upstream: `var i int` then assignment only inside the loop. When no
        // line fails the prefix test, i keeps its zero value. The commented-out
        // `// - 1` in the original suggests the author was unsure here too.
        let mut i = 0usize;
        for (idx, b) in split.iter().enumerate() {
            if !b.starts_with(b"%") {
                i = idx;
                break;
            }
        }

        let joined = split[0..i].join(&b'\n');
        let consumed = joined.len();

        let mut content = joined;
        if content.starts_with(b"% ") {
            content.drain(..2);
        }
        content = replace_all(&content, b"\n% ", b"\n");

        // Unconditional, even when consumed == 0. This is the bug.
        let block = self.add_block(NodeType::Heading, &content);
        self.arena[block].heading.level = 1;
        self.arena[block].heading.is_titleblock = true;

        consumed
    }

    /// Parses an `<hr>` block, the only self-closing block tag recognised.
    ///
    /// Ported from `htmlHr` (`block.go:442`). The tag must be followed by a
    /// blank line; trailing newlines are trimmed from the stored literal.
    pub(crate) fn html_hr(&mut self, data: &[u8], do_render: bool) -> usize {
        if data.len() < 4 {
            return 0;
        }
        if data[0] != b'<'
            || (data[1] != b'h' && data[1] != b'H')
            || (data[2] != b'r' && data[2] != b'R')
        {
            return 0;
        }
        if data[3] != b' ' && data[3] != b'/' && data[3] != b'>' {
            // Not an <hr> tag after all; at least not a valid one.
            return 0;
        }
        let mut i = 3usize;
        while i < data.len() && data[i] != b'>' && data[i] != b'\n' {
            i += 1;
        }
        if i < data.len() && data[i] == b'>' {
            i += 1;
            let j = is_empty(&data[i..]);
            if j > 0 {
                let size = i + j;
                if do_render {
                    let mut end = size;
                    while end > 0 && data[end - 1] == b'\n' {
                        end -= 1;
                    }
                    let block = self.add_block(NodeType::HTMLBlock, &data[..end]);
                    finalize_html_block(&mut self.arena, block);
                }
                return size;
            }
        }
        0
    }

    /// Parses one table row, appending a `TableRow` and its cells.
    ///
    /// Ported from `tableRow` (`block.go:899`). Rows with too few cells are
    /// padded with empty ones; rows with too many have the excess silently
    /// dropped, since the loop is bounded by the column count.
    ///
    /// Cell boundaries respect backslash escaping, so `a \| b` is one cell.
    ///
    /// # Panics
    ///
    /// On empty input, matching upstream's unguarded `data[0]`.
    pub(crate) fn table_row(
        &mut self,
        data: &[u8],
        columns: &[crate::CellAlignFlags],
        header: bool,
    ) {
        self.add_block(NodeType::TableRow, b"");
        let mut i = 0usize;

        if data[i] == b'|' && !is_backslash_escaped(data, i) {
            i += 1;
        }

        let mut col = 0usize;
        while col < columns.len() && i < data.len() {
            while i < data.len() && data[i] == b' ' {
                i += 1;
            }

            let cell_start = i;

            while i < data.len()
                && (data[i] != b'|' || is_backslash_escaped(data, i))
                && data[i] != b'\n'
            {
                i += 1;
            }

            let mut cell_end = i;

            // Skip the end-of-cell marker, possibly taking us past the end of
            // the buffer. Upstream says so in a comment; `i` is only ever
            // compared afterwards, never indexed, so it is safe on usize too.
            i += 1;

            while cell_end > cell_start && cell_end - 1 < data.len() && data[cell_end - 1] == b' ' {
                cell_end -= 1;
            }

            let cell = self.add_block(NodeType::TableCell, &data[cell_start..cell_end]);
            self.arena[cell].table_cell.is_header = header;
            self.arena[cell].table_cell.align = columns[col];
            col += 1;
        }

        // Pad out with empty columns to reach the right number.
        while col < columns.len() {
            let cell = self.add_block(NodeType::TableCell, b"");
            self.arena[cell].table_cell.is_header = header;
            self.arena[cell].table_cell.align = columns[col];
            col += 1;
        }

        // Rows with too many cells are silently ignored.
    }

    /// Parses a table header and its alignment row.
    ///
    /// Ported from `tableHeader` (`block.go:786`). Returns how much input the
    /// header consumed and the per-column alignment, or `0` when this is not a
    /// table. On success it appends a `TableHead` and the header row.
    ///
    /// Go uses named return values, so its bare `return`s hand back `0`
    /// alongside whatever `columns` had been built so far. Every caller
    /// discards the columns when the size is `0`, so that is preserved without
    /// mattering.
    pub(crate) fn table_header(&mut self, data: &[u8]) -> (usize, Vec<crate::CellAlignFlags>) {
        use crate::CellAlignFlags;

        let mut i = 0usize;
        let mut col_count = 1usize;
        while i < data.len() && data[i] != b'\n' {
            if data[i] == b'|' && !is_backslash_escaped(data, i) {
                col_count += 1;
            }
            i += 1;
        }

        // Doesn't look like a table header.
        if col_count == 1 {
            return (0, Vec::new());
        }

        // Include the newline in the data sent to table_row.
        let mut j = i;
        if j < data.len() && data[j] == b'\n' {
            j += 1;
        }
        let header = data[..j].to_vec();

        // The column count ignores pipes at the start or end of the line.
        if data[0] == b'|' {
            col_count -= 1;
        }
        // Note `i > 2`, not `i > 0`: upstream's own bound.
        if i > 2 && data[i - 1] == b'|' && !is_backslash_escaped(data, i - 1) {
            col_count -= 1;
        }

        let mut columns = vec![CellAlignFlags::NONE; col_count];

        // Move on to the header underline.
        i += 1;
        if i >= data.len() {
            return (0, columns);
        }

        if data[i] == b'|' && !is_backslash_escaped(data, i) {
            i += 1;
        }
        i = skip_char(data, i, b' ');

        // Each column header is `/ *:?-+:? *|/` with dashes + colons >= 3, and
        // a trailing `|` optional on the last column.
        let mut col = 0usize;
        while i < data.len() && data[i] != b'\n' {
            let mut dashes = 0usize;

            if data[i] == b':' {
                i += 1;
                columns[col] |= CellAlignFlags::LEFT;
                dashes += 1;
            }
            while i < data.len() && data[i] == b'-' {
                i += 1;
                dashes += 1;
            }
            if i < data.len() && data[i] == b':' {
                i += 1;
                columns[col] |= CellAlignFlags::RIGHT;
                dashes += 1;
            }
            while i < data.len() && data[i] == b' ' {
                i += 1;
            }
            if i == data.len() {
                return (0, columns);
            }

            // The end-of-column test, kept in upstream's order.
            if dashes < 3 {
                // Not a valid column.
                return (0, columns);
            } else if data[i] == b'|' && !is_backslash_escaped(data, i) {
                // Marker found; skip past trailing whitespace.
                col += 1;
                i += 1;
                while i < data.len() && data[i] == b' ' {
                    i += 1;
                }
                // Trailing junk after the last column.
                if col >= col_count && i < data.len() && data[i] != b'\n' {
                    return (0, columns);
                }
            } else if (data[i] != b'|' || is_backslash_escaped(data, i)) && col + 1 < col_count {
                // Something else where a marker was required.
                return (0, columns);
            } else if data[i] == b'\n' {
                // The marker is optional for the last column.
                col += 1;
            } else {
                // Trailing junk after the last column.
                return (0, columns);
            }
        }
        if col != col_count {
            return (0, columns);
        }

        self.add_block(NodeType::TableHead, b"");
        self.table_row(&header, &columns, true);
        let mut size = i;
        if size < data.len() && data[size] == b'\n' {
            size += 1;
        }
        (size, columns)
    }

    /// Parses a whole table.
    ///
    /// Ported from `table` (`block.go:743`). Appends a `Table` node up front and
    /// unlinks it again if the header turns out not to be one — the arena keeps
    /// the storage, exactly as Go leaves the node to its GC.
    ///
    /// Body rows end at the first line containing no `|`.
    pub(crate) fn table(&mut self, data: &[u8]) -> usize {
        let table = self.add_block(NodeType::Table, b"");
        let (mut i, columns) = self.table_header(data);
        if i == 0 {
            self.tip = self.arena[table].parent().unwrap_or(self.doc);
            self.arena.unlink(table);
            return 0;
        }

        self.add_block(NodeType::TableBody, b"");

        while i < data.len() {
            let mut pipes = 0usize;
            let row_start = i;
            while i < data.len() && data[i] != b'\n' {
                if data[i] == b'|' {
                    pipes += 1;
                }
                i += 1;
            }

            if pipes == 0 {
                i = row_start;
                break;
            }

            // Include the newline in the data sent to table_row.
            if i < data.len() && data[i] == b'\n' {
                i += 1;
            }
            let row = data[row_start..i].to_vec();
            self.table_row(&row, &columns, false);
        }

        i
    }

    /// Length of an HTML comment at the start of `data`, or `0`.
    ///
    /// Ported from `inlineHTMLComment` (`inline.go`). Lives here because
    /// [`Markdown::html_comment`] is its only caller so far; it moves to the
    /// inline module when the rest of that file lands.
    ///
    /// The scan starts at index 5, so `<!-->` and `<!--->` are not comments
    /// however they look — the closing `-->` has to begin at index 3 or later.
    pub(crate) fn inline_html_comment(&self, data: &[u8]) -> usize {
        if data.len() < 5 {
            return 0;
        }
        if data[0] != b'<' || data[1] != b'!' || data[2] != b'-' || data[3] != b'-' {
            return 0;
        }
        let mut i = 5usize;
        // Scan for the end-of-comment marker, across lines if necessary.
        while i < data.len() && !(data[i - 2] == b'-' && data[i - 1] == b'-' && data[i] == b'>') {
            i += 1;
        }
        // No end-of-comment marker.
        if i >= data.len() {
            return 0;
        }
        i + 1
    }

    /// Parses an HTML comment block, which must be followed by a blank line.
    ///
    /// Ported from `htmlComment` (`block.go:422`).
    pub(crate) fn html_comment(&mut self, data: &[u8], do_render: bool) -> usize {
        let i = self.inline_html_comment(data);
        let j = is_empty(&data[i..]);
        if j > 0 {
            let size = i + j;
            if do_render {
                let mut end = size;
                while end > 0 && data[end - 1] == b'\n' {
                    end -= 1;
                }
                let block = self.add_block(NodeType::HTMLBlock, &data[..end]);
                finalize_html_block(&mut self.arena, block);
            }
            return size;
        }
        0
    }

    /// Parses a block of preformatted HTML.
    ///
    /// Ported from `html` (`block.go:318`). Falls back to comment and `<hr>`
    /// handling when the opening tag is not a recognised block tag.
    ///
    /// Upstream's first search pass — an unindented closing tag followed by a
    /// blank line — is entirely commented out, so `found` is always false when
    /// the second pass begins and only the indented search actually runs. The
    /// `!found &&` guard on it is therefore always true. Ported as it behaves,
    /// with the vestigial condition dropped rather than transcribed as an
    /// always-true test.
    ///
    /// The `ins`/`del` exclusion is real and comes from Markdown.pl.
    ///
    /// # Panics
    ///
    /// On empty input, matching upstream's unguarded `data[0]`.
    pub(crate) fn html(&mut self, data: &[u8], do_render: bool) -> usize {
        if data[0] != b'<' {
            return 0;
        }
        let curtag = html_find_tag(&data[1..]).map(str::to_string);

        let Some(curtag) = curtag else {
            // Not a block tag: try the special cases.
            let size = self.html_comment(data, do_render);
            if size > 0 {
                return size;
            }
            let size = self.html_hr(data, do_render);
            if size > 0 {
                return size;
            }
            return 0;
        };

        let mut found = false;
        let mut i = 0usize;

        // Second pass: look for an indented match. Skipped for ins and del,
        // following original Markdown.pl.
        if curtag != "ins" && curtag != "del" {
            let lax = self
                .extensions
                .intersects(crate::Extensions::LAX_HTML_BLOCKS);
            i = 1;
            while i < data.len() {
                i += 1;
                while i < data.len() && !(data[i - 1] == b'<' && data[i] == b'/') {
                    i += 1;
                }

                if i + 2 + curtag.len() >= data.len() {
                    break;
                }

                let j = html_find_end(&curtag, &data[i - 1..], lax);
                if j > 0 {
                    i += j - 1;
                    found = true;
                    break;
                }
            }
        }

        if !found {
            return 0;
        }

        if do_render {
            let mut end = i;
            while end > 0 && data[end - 1] == b'\n' {
                end -= 1;
            }
            let block = self.add_block(NodeType::HTMLBlock, &data[..end]);
            finalize_html_block(&mut self.arena, block);
        }

        i
    }

    /// Appends a `Paragraph` node, trimming surrounding whitespace.
    ///
    /// Ported from `renderParagraph` (`block.go:1428`). Empty input produces no
    /// node at all.
    ///
    /// # Panics
    ///
    /// On input consisting only of spaces, such as `"   "`. Go's leading-space
    /// trim is `for data[beg] == ' ' { beg++ }` with no length guard, so it runs
    /// off the end; Rust's bounds check panics at the same point. Reachability
    /// from the public entry point is unconfirmed — every call site passes a
    /// slice ending at a line boundary, and an all-spaces line is caught by
    /// `is_empty` first — so this is recorded as a latent hazard rather than
    /// claimed as a reachable bug.
    pub(crate) fn render_paragraph(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        // Trim leading spaces. Unguarded upstream; see the note above.
        let mut beg = 0usize;
        while data[beg] == b' ' {
            beg += 1;
        }

        let mut end = data.len();
        if data[data.len() - 1] == b'\n' {
            end -= 1;
        }
        while end > beg && data[end - 1] == b' ' {
            end -= 1;
        }

        self.add_block(NodeType::Paragraph, &data[beg..end]);
    }

    /// Parses block-level constructs out of `data`, one at a time.
    ///
    /// Ported from `block` (`block.go:37`). This is the dispatcher every other
    /// block parser recurses through, which is why it and the four parsers
    /// below had to land together.
    ///
    /// Recursion is bounded by `max_nesting`; past that the call returns
    /// immediately, silently discarding the input rather than erroring.
    pub(crate) fn block(&mut self, data: &[u8]) {
        use crate::flags::Extensions;

        // Called recursively: enforce a maximum depth.
        if self.nesting >= self.max_nesting {
            return;
        }
        self.nesting += 1;

        let mut data = data;
        while !data.is_empty() {
            // Prefixed heading.
            if self.is_prefix_heading(data) {
                let n = self.prefix_heading(data);
                data = &data[n..];
                continue;
            }

            // Block of preformatted HTML.
            if data[0] == b'<' {
                let i = self.html(data, true);
                if i > 0 {
                    data = &data[i..];
                    continue;
                }
            }

            // Title block.
            if self.extensions.intersects(Extensions::TITLEBLOCK) && data[0] == b'%' {
                let i = self.title_block(data, true);
                if i > 0 {
                    data = &data[i..];
                    continue;
                }
            }

            // Blank lines.
            let i = is_empty(data);
            if i > 0 {
                data = &data[i..];
                continue;
            }

            // Indented code block.
            if code_prefix(data) > 0 {
                let n = self.code(data);
                data = &data[n..];
                continue;
            }

            // Fenced code block.
            if self.extensions.intersects(Extensions::FENCED_CODE) {
                let i = self.fenced_code_block(data, true);
                if i > 0 {
                    data = &data[i..];
                    continue;
                }
            }

            // Horizontal rule.
            if is_hrule(data) {
                self.add_block(NodeType::HorizontalRule, b"");
                let mut i = 0usize;
                while i < data.len() && data[i] != b'\n' {
                    i += 1;
                }
                data = &data[i..];
                continue;
            }

            // Block quote.
            if quote_prefix(data) > 0 {
                let n = self.quote(data);
                data = &data[n..];
                continue;
            }

            // Table.
            if self.extensions.intersects(Extensions::TABLES) {
                let i = self.table(data);
                if i > 0 {
                    data = &data[i..];
                    continue;
                }
            }

            // Unordered list.
            if uli_prefix(data) > 0 {
                let n = self.list(data, crate::ListType::NONE);
                data = &data[n..];
                continue;
            }

            // Ordered list.
            if oli_prefix(data) > 0 {
                let n = self.list(data, crate::ListType::ORDERED);
                data = &data[n..];
                continue;
            }

            // Definition list.
            if self.extensions.intersects(Extensions::DEFINITION_LISTS) && dli_prefix(data) > 0 {
                let n = self.list(data, crate::ListType::DEFINITION);
                data = &data[n..];
                continue;
            }

            // Anything else is a paragraph. Note this also finds underlined
            // headings.
            let n = self.paragraph(data);
            data = &data[n..];
        }

        self.nesting -= 1;
    }

    /// Parses a blockquote, recursing into its contents.
    ///
    /// Ported from `quote` (`block.go:970`). Fenced code inside a quote is
    /// swallowed whole, so its contents cannot terminate the quote.
    pub(crate) fn quote(&mut self, data: &[u8]) -> usize {
        use crate::flags::Extensions;

        let block = self.add_block(NodeType::BlockQuote, b"");
        let mut raw: Vec<u8> = Vec::new();
        let mut beg = 0usize;
        let mut end = 0usize;

        while beg < data.len() {
            end = beg;
            // Step over whole lines, collecting them. Check for fenced code and
            // if one is found, take it in its entirety regardless of contents.
            while end < data.len() && data[end] != b'\n' {
                if self.extensions.intersects(Extensions::FENCED_CODE) {
                    let i = self.fenced_code_block(&data[end..], false);
                    if i > 0 {
                        // -1 compensates for the extra end += 1 after the loop.
                        end += i - 1;
                        break;
                    }
                }
                end += 1;
            }
            if end < data.len() && data[end] == b'\n' {
                end += 1;
            }
            let pre = quote_prefix(&data[beg..]);
            if pre > 0 {
                beg += pre;
            } else if terminate_blockquote(data, beg, end) {
                break;
            }
            raw.extend_from_slice(&data[beg..end]);
            beg = end;
        }

        self.block(&raw);
        self.finalize(block);
        end
    }

    /// Parses an ordered, unordered or definition list.
    ///
    /// Ported from `list` (`block.go:1127`). The list starts tight and is
    /// loosened only if an item reports containing a block.
    pub(crate) fn list(&mut self, data: &[u8], flags: crate::ListType) -> usize {
        use crate::ListType;

        let mut flags = flags | ListType::ITEM_BEGINNING_OF_LIST;
        let block = self.add_block(NodeType::List, b"");
        self.arena[block].list.list_flags = flags;
        self.arena[block].list.tight = true;

        let mut i = 0usize;
        while i < data.len() {
            let skip = self.list_item(&data[i..], &mut flags);
            if flags.intersects(ListType::ITEM_CONTAINS_BLOCK) {
                self.arena[block].list.tight = false;
            }
            i += skip;
            if skip == 0 || flags.intersects(ListType::ITEM_END_OF_LIST) {
                break;
            }
            flags = flags.without(ListType::ITEM_BEGINNING_OF_LIST);
        }

        let above = self.arena[block].parent();
        finalize_list(&mut self.arena, block);
        if let Some(a) = above {
            self.tip = a;
        }
        i
    }

    /// Parses a single list item.
    ///
    /// Ported from `listItem` (`block.go:1207`). Assumes any sublist prefix has
    /// already been removed. `flags` is read *and written*: the caller learns
    /// through it whether the item contained a block or ended the list.
    ///
    /// # Panics
    ///
    /// On empty input, matching upstream's unguarded `data[0]`.
    pub(crate) fn list_item(&mut self, data: &[u8], flags: &mut crate::ListType) -> usize {
        use crate::flags::Extensions;
        use crate::ListType;

        // Track the indentation of the first line.
        let mut item_indent = 0usize;
        if data[0] == b'\t' {
            item_indent += 4;
        } else {
            while item_indent < 3 && data[item_indent] == b' ' {
                item_indent += 1;
            }
        }

        let mut bullet_char = b'*';
        let mut i = uli_prefix(data);
        if i == 0 {
            i = oli_prefix(data);
        } else {
            bullet_char = data[i - 2];
        }
        if i == 0 {
            i = dli_prefix(data);
            // Reset the definition-term flag.
            if i > 0 {
                *flags = flags.without(ListType::TERM);
            }
        }
        if i == 0 {
            // In a definition list, set the term flag and carry on.
            if flags.intersects(ListType::DEFINITION) {
                *flags |= ListType::TERM;
            } else {
                return 0;
            }
        }

        // Skip leading whitespace on the first line.
        while i < data.len() && data[i] == b' ' {
            i += 1;
        }

        // Find the end of the line.
        let mut line = i;
        while i > 0 && i < data.len() && data[i - 1] != b'\n' {
            i += 1;
        }

        let mut raw: Vec<u8> = Vec::new();
        raw.extend_from_slice(&data[line..i]);
        line = i;

        let mut contains_blank_line = false;
        let mut sublist = 0usize;
        let mut code_block_marker: Vec<u8> = Vec::new();

        'gatherlines: while line < data.len() {
            i += 1;

            while i < data.len() && data[i - 1] != b'\n' {
                i += 1;
            }

            // An empty line is assumed to belong to this item.
            if is_empty(&data[line..i]) > 0 {
                contains_blank_line = true;
                line = i;
                continue;
            }

            // Work out the indentation.
            let mut indent = 0usize;
            let mut indent_index = 0usize;
            if data[line] == b'\t' {
                indent_index += 1;
                indent += 4;
            } else {
                while indent < 4 && line + indent < i && data[line + indent] == b' ' {
                    indent += 1;
                    indent_index += 1;
                }
            }

            let chunk = &data[line + indent_index..i];

            if self.extensions.intersects(Extensions::FENCED_CODE) {
                // Track whether we are inside a code block; if so, skip the
                // normal list processing entirely.
                let (_, marker) = is_fence_line(chunk, None, &code_block_marker);
                if !marker.is_empty() {
                    if code_block_marker.is_empty() {
                        code_block_marker = marker.clone();
                    } else {
                        code_block_marker.clear();
                    }
                }
                if !code_block_marker.is_empty() || !marker.is_empty() {
                    raw.extend_from_slice(&data[line + indent_index..i]);
                    line = i;
                    continue 'gatherlines;
                }
            }

            // Work out how this line fits in.
            if (uli_prefix(chunk) > 0 && !is_hrule(chunk))
                || oli_prefix(chunk) > 0
                || dli_prefix(chunk) > 0
            {
                // To nest, it must be indented further; otherwise it is either
                // a different kind of list or the next item in this one.
                if indent <= item_indent {
                    if list_type_changed(chunk, *flags) {
                        *flags |= ListType::ITEM_END_OF_LIST;
                    } else if contains_blank_line {
                        *flags |= ListType::ITEM_CONTAINS_BLOCK;
                    }
                    break 'gatherlines;
                }

                if contains_blank_line {
                    *flags |= ListType::ITEM_CONTAINS_BLOCK;
                }

                // First item of the nested list?
                if sublist == 0 {
                    sublist = raw.len();
                }
            } else if self.is_prefix_heading(chunk) {
                // An unindented heading is not nested, and ends the list.
                if contains_blank_line && indent < 4 {
                    *flags |= ListType::ITEM_END_OF_LIST;
                    break 'gatherlines;
                }
                *flags |= ListType::ITEM_CONTAINS_BLOCK;
            } else if contains_blank_line && indent < 4 {
                // After a blank line, content belongs to this item only if it
                // is indented four spaces, whatever the item's own indent.
                if flags.intersects(ListType::DEFINITION) && i + 1 < data.len() {
                    // Is the next item still part of this list?
                    let mut next = i;
                    while next < data.len() && data[next] != b'\n' {
                        next += 1;
                    }
                    while next + 1 < data.len() && data[next] == b'\n' {
                        next += 1;
                    }
                    if i + 1 < data.len() && data[i] != b':' && data[next] != b':' {
                        *flags |= ListType::ITEM_END_OF_LIST;
                    }
                } else {
                    *flags |= ListType::ITEM_END_OF_LIST;
                }
                break 'gatherlines;
            } else if contains_blank_line {
                // A blank line means this should be parsed as a block.
                raw.push(b'\n');
                *flags |= ListType::ITEM_CONTAINS_BLOCK;
            }

            // Re-introduce a blank that preceded this line.
            if contains_blank_line {
                contains_blank_line = false;
                raw.push(b'\n');
            }

            raw.extend_from_slice(&data[line + indent_index..i]);
            line = i;
        }

        let block = self.add_block(NodeType::Item, b"");
        self.arena[block].list.list_flags = *flags;
        self.arena[block].list.tight = false;
        self.arena[block].list.bullet_char = bullet_char;
        // Only '.' is possible in Markdown; CommonMark also allows ')'.
        self.arena[block].list.delimiter = b'.';

        if flags.intersects(ListType::ITEM_CONTAINS_BLOCK) && !flags.intersects(ListType::TERM) {
            // Intermediate render of a block item, except for a definition term.
            if sublist > 0 {
                let head = raw[..sublist].to_vec();
                let tail = raw[sublist..].to_vec();
                self.block(&head);
                self.block(&tail);
            } else {
                self.block(&raw);
            }
        } else {
            // Intermediate render of an inline item.
            if sublist > 0 {
                let child = self.add_child(NodeType::Paragraph);
                self.arena[child].content = raw[..sublist].to_vec();
                let tail = raw[sublist..].to_vec();
                self.block(&tail);
            } else {
                let child = self.add_child(NodeType::Paragraph);
                self.arena[child].content = raw;
            }
        }

        line
    }

    /// Parses a paragraph, which is also where underlined headings are found.
    ///
    /// Ported from `paragraph` (`block.go:1453`). Returns how much input was
    /// consumed; the paragraph itself is emitted by
    /// [`Markdown::render_paragraph`].
    pub(crate) fn paragraph(&mut self, data: &[u8]) -> usize {
        use crate::flags::Extensions;

        // prev: start of the previous line; line: start of the current line;
        // i: the cursor, i.e. the end of the current line.
        // `prev` is assigned from `line` at the top of every iteration before
        // it is read, so it needs no initial value -- Go's `var prev int` zero
        // is likewise never observed.
        let mut prev;
        let mut line = 0usize;
        let mut i = 0usize;
        let tab_size = if self.extensions.intersects(Extensions::TAB_SIZE_EIGHT) {
            crate::TAB_SIZE_DOUBLE
        } else {
            crate::TAB_SIZE_DEFAULT
        };

        while i < data.len() {
            prev = line;
            let current = &data[i..];
            line = i;

            // A reference or footnote ends the paragraph before it, and we
            // report having consumed through the end of that reference.
            let ref_end = self.is_reference(current, tab_size);
            if ref_end > 0 {
                self.render_paragraph(&data[..i]);
                return i + ref_end;
            }

            // A blank line ends the paragraph.
            let n = is_empty(current);
            if n > 0 {
                // Unless a definition list item follows it.
                if self.extensions.intersects(Extensions::DEFINITION_LISTS)
                    && i + 1 < data.len()
                    && data[i + 1] == b':'
                {
                    let rest = data[prev..].to_vec();
                    return self.list(&rest, crate::ListType::DEFINITION);
                }

                self.render_paragraph(&data[..i]);
                return i + n;
            }

            // An underline marks a heading, so the paragraph ended on the
            // previous line.
            if i > 0 {
                let level = is_underlined_heading(current);
                if level > 0 {
                    self.render_paragraph(&data[..prev]);

                    // Ignore leading and trailing whitespace.
                    let mut eol = i - 1;
                    while prev < eol && data[prev] == b' ' {
                        prev += 1;
                    }
                    while eol > prev && data[eol - 1] == b' ' {
                        eol -= 1;
                    }

                    let mut id = String::new();
                    if self.extensions.intersects(Extensions::AUTO_HEADING_IDS) {
                        id = sanitized_anchor_name_bytes(&data[prev..eol]);
                    }

                    let block = self.add_block(NodeType::Heading, &data[prev..eol]);
                    self.arena[block].heading.level = level;
                    self.arena[block].heading.heading_id = id;

                    // Find the end of the underline.
                    while i < data.len() && data[i] != b'\n' {
                        i += 1;
                    }
                    return i;
                }
            }

            // A block of HTML on the next line ends the paragraph.
            if self.extensions.intersects(Extensions::LAX_HTML_BLOCKS)
                && data[i] == b'<'
                && self.html(current, false) > 0
            {
                self.render_paragraph(&data[..i]);
                return i;
            }

            // A prefixed heading or horizontal rule ends the paragraph.
            if self.is_prefix_heading(current) || is_hrule(current) {
                self.render_paragraph(&data[..i]);
                return i;
            }

            // A fenced code block ends the paragraph.
            if self.extensions.intersects(Extensions::FENCED_CODE)
                && self.fenced_code_block(current, false) > 0
            {
                self.render_paragraph(&data[..i]);
                return i;
            }

            // A definition list item means the previous line was a term.
            if self.extensions.intersects(Extensions::DEFINITION_LISTS) && dli_prefix(current) != 0
            {
                let rest = data[prev..].to_vec();
                return self.list(&rest, crate::ListType::DEFINITION);
            }

            // With NoEmptyLineBeforeBlock, a list or quote ends it too.
            if self
                .extensions
                .intersects(Extensions::NO_EMPTY_LINE_BEFORE_BLOCK)
                && (uli_prefix(current) != 0
                    || oli_prefix(current) != 0
                    || quote_prefix(current) != 0
                    || code_prefix(current) != 0)
            {
                self.render_paragraph(&data[..i]);
                return i;
            }

            // Otherwise scan to the start of the next line.
            match data[i..].iter().position(|&c| c == b'\n') {
                Some(nl) => i += nl + 1,
                None => i += data[i..].len(),
            }
        }

        self.render_paragraph(&data[..i]);
        i
    }

    /// Whether `data` begins an ATX heading.
    ///
    /// Ported from `isPrefixHeading` (`block.go:207`). With
    /// [`crate::Extensions::SPACE_HEADINGS`] the `#` run must be followed by a
    /// space, so `#h` is a heading without the extension and ordinary text
    /// with it.
    ///
    /// # Panics
    ///
    /// On empty input, matching upstream.
    pub(crate) fn is_prefix_heading(&self, data: &[u8]) -> bool {
        if data[0] != b'#' {
            return false;
        }

        if self
            .extensions
            .intersects(crate::Extensions::SPACE_HEADINGS)
        {
            let mut level = 0usize;
            while level < 6 && level < data.len() && data[level] == b'#' {
                level += 1;
            }
            if level == data.len() || data[level] != b' ' {
                return false;
            }
        }
        true
    }

    /// Parses an ATX heading, appending a `Heading` node.
    ///
    /// Ported from `prefixHeading` (`block.go:224`). Returns how much input to
    /// skip, which is *not* the same as the heading's extent when an explicit
    /// `{#id}` is present: the id is stripped from the rendered text but still
    /// consumed.
    ///
    /// Note the trailing-`#` trim honours backslash escaping, so `# h \#`
    /// keeps its final `#`, and no node is emitted at all when the heading
    /// text is empty.
    pub(crate) fn prefix_heading(&mut self, data: &[u8]) -> usize {
        let mut level = 0usize;
        while level < 6 && level < data.len() && data[level] == b'#' {
            level += 1;
        }
        let i = skip_char(data, level, b' ');
        let mut end = skip_until_char(data, i, b'\n');
        let mut skip = end;
        let mut id = String::new();

        if self.extensions.intersects(crate::Extensions::HEADING_IDS) {
            // Find the start and end of an explicit {#id}.
            let mut j = i;
            while j + 1 < end && (data[j] != b'{' || data[j + 1] != b'#') {
                j += 1;
            }
            let mut k = j + 1;
            while k < end && data[k] != b'}' {
                k += 1;
            }
            if j < end && k < end {
                id = String::from_utf8_lossy(&data[j + 2..k]).into_owned();
                end = j;
                skip = k + 1;
                while end > 0 && data[end - 1] == b' ' {
                    end -= 1;
                }
            }
        }

        while end > 0 && data[end - 1] == b'#' {
            if is_backslash_escaped(data, end - 1) {
                break;
            }
            end -= 1;
        }
        while end > 0 && data[end - 1] == b' ' {
            end -= 1;
        }

        if end > i {
            if id.is_empty()
                && self
                    .extensions
                    .intersects(crate::Extensions::AUTO_HEADING_IDS)
            {
                id = sanitized_anchor_name_bytes(&data[i..end]);
            }
            let block = self.add_block(NodeType::Heading, &data[i..end]);
            self.arena[block].heading.heading_id = id;
            self.arena[block].heading.level = level as i32;
        }

        skip
    }
}

/// Returns a sanitized anchor name for the given text.
///
/// Ported from `block.go:1596`. Every run of characters that are neither
/// letters nor numbers collapses to a single `-`, letters are lowercased, and
/// no leading dash is emitted.
///
/// # Not the same as [`crate::util::slugify`]
///
/// The two are easy to confuse and upstream uses both. `slugify` is
/// byte-oriented, ASCII-only and **preserves case**; this one decodes UTF-8,
/// consults Unicode tables and **lowercases**. `slugify("Hello, World!")` is
/// `"Hello-World"`, while `sanitized_anchor_name("Hello, World!")` is
/// `"hello-world"`.
///
/// # Why this does not use Rust's `char::is_alphabetic`
///
/// Go tests `unicode.IsLetter(r) || unicode.IsNumber(r)`, which is
/// General_Category L or N. Rust's `char::is_alphabetic` is the Alphabetic
/// property — a *superset* that also includes Nl and Other_Alphabetic. Measured
/// across the whole code space that is 11,171 code points where the two
/// disagree, and Go's set is a strict subset.
///
/// Those are mostly combining marks, and the difference is observable:
///
/// ```
/// # use blackfriday::block::sanitized_anchor_name;
/// // U+0345 is Other_Alphabetic. Go drops it and leaves a dash behind;
/// // a port built on is_alphabetic() would keep it and emit "a\u{345}b".
/// assert_eq!(sanitized_anchor_name("a\u{345}b"), "a-b");
/// ```
///
/// Lowercasing has the same problem in miniature: Go's `unicode.ToLower` is
/// *simple* case mapping, Rust's `to_lowercase` is *full* and may yield more
/// than one char. U+0130 is the only such case inside this set.
///
/// ```
/// # use blackfriday::block::sanitized_anchor_name;
/// // Rust's full mapping gives "i\u{307}"; Go's simple mapping gives "i".
/// assert_eq!(sanitized_anchor_name("\u{130}stanbul"), "istanbul");
/// ```
///
/// The tables in the crate-private `unicode_tables` module are generated from
/// Go's own data by `tools/genunicode`, so this is not an approximation of Go's
/// behaviour — it is Go's behaviour.
pub fn sanitized_anchor_name(text: &str) -> String {
    let mut anchor_name = String::with_capacity(text.len());
    let mut future_dash = false;

    for r in text.chars() {
        if is_letter_or_number(r) {
            if future_dash && !anchor_name.is_empty() {
                anchor_name.push('-');
            }
            future_dash = false;
            anchor_name.push(simple_to_lower(r));
        } else {
            future_dash = true;
        }
    }

    anchor_name
}

/// Byte-slice form of [`sanitized_anchor_name`].
///
/// Upstream reaches this function as `SanitizedAnchorName(string(data[i:end]))`
/// (`block.go:261`), converting a `[]byte` that need not be valid UTF-8. Go's
/// `range` over such a string yields `U+FFFD` once per invalid byte, whereas
/// Rust's lossy conversion emits one `U+FFFD` per maximal invalid subsequence —
/// so the two can produce a different *number* of replacement characters.
///
/// That difference is not observable here. `U+FFFD` is General_Category So, so
/// it is neither a letter nor a number under either implementation; whether it
/// appears once or three times, the only effect is to set `future_dash`. The
/// fixture covers several malformed inputs to keep that claim honest.
pub fn sanitized_anchor_name_bytes(text: &[u8]) -> String {
    sanitized_anchor_name(&String::from_utf8_lossy(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Measured Go output, produced by `tools/genanchor` against
    /// blackfriday v2.1.0. Hex-encoded so malformed UTF-8 survives the trip.
    const FIXTURE: &str = include_str!("../tests/fixtures/go-anchor.txt");

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("bad hex"))
            .collect()
    }

    #[test]
    fn matches_go_across_the_corpus() {
        let mut checked = 0;
        for line in FIXTURE.lines() {
            let f: Vec<&str> = line.split(' ').collect();
            if f.first() != Some(&"A") {
                continue;
            }
            let input = unhex(f[1]);
            let want = String::from_utf8(unhex(f.get(2).copied().unwrap_or("")))
                .expect("Go output is always valid UTF-8");
            let got = sanitized_anchor_name_bytes(&input);
            assert_eq!(
                got,
                want,
                "sanitized_anchor_name({:?})",
                String::from_utf8_lossy(&input)
            );
            checked += 1;
        }
        assert!(checked >= 50, "expected a real corpus, got {checked}");
    }

    /// The exact cases from the pinned `TestSanitizedAnchorName`
    /// (`block_test.go:1879`), spelled out so a fixture regeneration cannot
    /// quietly weaken them.
    #[test]
    fn matches_the_pinned_upstream_test_cases() {
        assert_eq!(
            sanitized_anchor_name("This is a header"),
            "this-is-a-header"
        );
        assert_eq!(
            sanitized_anchor_name("This is also          a header"),
            "this-is-also-a-header"
        );
        assert_eq!(sanitized_anchor_name("main.go"), "main-go");
        assert_eq!(sanitized_anchor_name("Article 123"), "article-123");
        assert_eq!(
            sanitized_anchor_name("<- Let's try this, shall we?"),
            "let-s-try-this-shall-we"
        );
        assert_eq!(sanitized_anchor_name("        "), "");
        assert_eq!(sanitized_anchor_name("Hello, 世界"), "hello-世界");
    }

    #[test]
    fn other_alphabetic_marks_are_dropped_not_kept() {
        // The 11,171 code points where Rust's is_alphabetic() disagrees with
        // Go's IsLetter. Each of these would survive into the anchor name if
        // the port had used std's predicate.
        for (input, want, mark) in [
            ("a\u{345}b", "a-b", '\u{345}'), // GREEK YPOGEGRAMMENI, Mn
            ("k\u{5b0}t", "k-t", '\u{5b0}'), // HEBREW POINT SHEVA, Mn
            ("x\u{9be}y", "x-y", '\u{9be}'), // BENGALI VOWEL SIGN AA, Mc
            ("\u{102b}", "", '\u{102b}'),    // MYANMAR VOWEL SIGN TALL AA, Mc
            ("\u{e31}", "", '\u{e31}'),      // THAI CHARACTER MAI HAN-AKAT, Mn
        ] {
            assert_eq!(sanitized_anchor_name(input), want, "input {input:?}");
            // The premise: std would have called this a letter, our table does
            // not. If a Rust upgrade ever changes that, this fails loudly
            // rather than silently reintroducing the divergence.
            assert!(
                mark.is_alphabetic(),
                "{mark:?} is expected to be Alphabetic in Rust's tables"
            );
            assert!(
                !is_letter_or_number(mark),
                "{mark:?} must not be a letter or number under Go's tables"
            );
        }

        // A combining mark that is NOT Other_Alphabetic: both agree it is not a
        // letter, so this one is a control rather than a divergence case.
        assert_eq!(sanitized_anchor_name("a\u{300}b"), "a-b");
        assert!(!'\u{300}'.is_alphabetic());
        assert!(!is_letter_or_number('\u{300}'));
    }

    #[test]
    fn lowercasing_is_simple_not_full() {
        // Rust's char::to_lowercase yields "i\u{307}" for U+0130; Go's
        // unicode.ToLower yields 'i'. This is the only such case in the set.
        assert_eq!(sanitized_anchor_name("\u{130}"), "i");
        assert_eq!(sanitized_anchor_name("\u{130}stanbul"), "istanbul");
        assert_eq!('\u{130}'.to_lowercase().count(), 2, "premise of this test");

        // Sharp S: capital maps to small, small is already lowercase.
        assert_eq!(sanitized_anchor_name("\u{1e9e}"), "\u{df}");
        assert_eq!(sanitized_anchor_name("\u{df}"), "\u{df}");
    }

    #[test]
    fn numbers_that_are_not_digits_are_kept() {
        assert_eq!(sanitized_anchor_name("\u{2160}"), "\u{2170}"); // Nl, lowercases
        assert_eq!(sanitized_anchor_name("\u{bd}"), "\u{bd}"); // No, 1/2
        assert_eq!(sanitized_anchor_name("\u{2461}"), "\u{2461}"); // No, circled 2
    }

    #[test]
    fn no_leading_dash_is_ever_emitted() {
        // future_dash is only honoured once anchor_name is non-empty.
        assert_eq!(sanitized_anchor_name("   leading"), "leading");
        assert_eq!(sanitized_anchor_name("-leading-dash"), "leading-dash");
        assert_eq!(sanitized_anchor_name("!!!abc"), "abc");
    }

    #[test]
    fn no_trailing_dash_is_ever_emitted() {
        // A dash is only appended when a letter follows it, so trailing
        // punctuation cannot leave one behind.
        assert_eq!(sanitized_anchor_name("trailing   "), "trailing");
        assert_eq!(sanitized_anchor_name("trailing-dash-"), "trailing-dash");
        assert_eq!(sanitized_anchor_name("abc!!!"), "abc");
    }

    #[test]
    fn runs_of_separators_collapse_to_one_dash() {
        assert_eq!(
            sanitized_anchor_name("multiple   spaces   here"),
            "multiple-spaces-here"
        );
        assert_eq!(
            sanitized_anchor_name("Tabs\tand\nnewlines"),
            "tabs-and-newlines"
        );
        assert_eq!(sanitized_anchor_name("a.b.c"), "a-b-c");
    }

    #[test]
    fn empty_and_separator_only_inputs_yield_empty() {
        assert_eq!(sanitized_anchor_name(""), "");
        assert_eq!(sanitized_anchor_name("-"), "");
        assert_eq!(sanitized_anchor_name("---"), "");
        assert_eq!(sanitized_anchor_name("!!!"), "");
        assert_eq!(sanitized_anchor_name("___"), "");
        // Contrast with slugify, which returns "-" for these.
        assert_eq!(crate::util::slugify(b"!!!"), b"-");
    }

    #[test]
    fn invalid_utf8_behaves_like_go_despite_different_replacement_counts() {
        // Go emits one U+FFFD per bad byte; Rust's lossy conversion emits one
        // per maximal invalid subsequence. U+FFFD is category So either way, so
        // only future_dash is affected and the output is identical.
        assert_eq!(sanitized_anchor_name_bytes(b"a\xffb"), "a-b");
        assert_eq!(sanitized_anchor_name_bytes(b"a\x80\x80b"), "a-b");
        assert_eq!(sanitized_anchor_name_bytes(b"\xff\xfe"), "");
        assert_eq!(sanitized_anchor_name_bytes(b"\xc3"), "");
    }

    /// Measured Go answers for the block scanners, from `tools/genblock`.
    /// `PANIC` rows record inputs on which upstream panics rather than returns.
    const BLOCK_FIXTURE: &str = include_str!("../tests/fixtures/go-block.txt");

    fn rows(tag: &str) -> impl Iterator<Item = Vec<String>> + '_ {
        BLOCK_FIXTURE.lines().filter_map(move |l| {
            let f: Vec<String> = l.split(' ').map(str::to_string).collect();
            (f.first().map(String::as_str) == Some(tag)).then_some(f)
        })
    }

    fn catch<T>(f: impl FnOnce() -> T + std::panic::UnwindSafe) -> Option<T> {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let r = std::panic::catch_unwind(f).ok();
        std::panic::set_hook(prev);
        r
    }

    #[test]
    fn is_empty_matches_go() {
        let mut n = 0;
        for f in rows("E") {
            let data = unhex(&f[1]);
            assert_eq!(
                is_empty(&data),
                f[2].parse::<usize>().unwrap(),
                "is_empty({:?})",
                String::from_utf8_lossy(&data)
            );
            n += 1;
        }
        assert!(n >= 50, "thin corpus: {n}");
    }

    #[test]
    fn is_hrule_matches_go_including_where_go_panics() {
        let mut panics = 0;
        let mut n = 0;
        for f in rows("R") {
            let data = unhex(&f[1]);
            let got = catch(|| is_hrule(&data));
            if f[2] == "PANIC" {
                assert!(
                    got.is_none(),
                    "upstream panics on {:?}; the port must too, not return {:?}",
                    String::from_utf8_lossy(&data),
                    got
                );
                panics += 1;
            } else {
                assert_eq!(
                    got,
                    Some(f[2] == "true"),
                    "is_hrule({:?})",
                    String::from_utf8_lossy(&data)
                );
            }
            n += 1;
        }
        assert!(n >= 50, "thin corpus: {n}");
        assert_eq!(panics, 4, "expected the four unguarded-index cases");
    }

    #[test]
    fn is_underlined_heading_matches_go_including_where_go_panics() {
        let mut panics = 0;
        for f in rows("U") {
            let data = unhex(&f[1]);
            let got = catch(|| is_underlined_heading(&data));
            if f[2] == "PANIC" {
                assert!(got.is_none(), "must panic on {:?}", data);
                panics += 1;
            } else {
                assert_eq!(
                    got,
                    Some(f[2].parse::<i32>().unwrap()),
                    "is_underlined_heading({:?})",
                    String::from_utf8_lossy(&data)
                );
            }
        }
        assert_eq!(panics, 1);
    }

    #[test]
    fn is_prefix_heading_matches_go_under_both_extension_settings() {
        use crate::markdown::Options;
        use crate::Extensions;
        let plain = Markdown::new(Options::none());
        let spaced = Markdown::new(Options::none().with_extensions(Extensions::SPACE_HEADINGS));
        let mut panics = 0;
        for f in rows("P") {
            let data = unhex(&f[1]);
            for (parser, want) in [(&plain, &f[2]), (&spaced, &f[3])] {
                let got = catch(std::panic::AssertUnwindSafe(|| {
                    parser.is_prefix_heading(&data)
                }));
                if want == "PANIC" {
                    assert!(got.is_none(), "must panic on {:?}", data);
                    panics += 1;
                } else {
                    assert_eq!(
                        got,
                        Some(want == "true"),
                        "is_prefix_heading({:?})",
                        String::from_utf8_lossy(&data)
                    );
                }
            }
        }
        assert_eq!(panics, 2, "empty input panics under both settings");
    }

    #[test]
    fn skip_helpers_match_go() {
        let mut n = 0;
        for f in rows("S") {
            let data = unhex(&f[1]);
            let start: usize = f[2].parse().unwrap();
            let ch = u8::from_str_radix(&f[3], 16).unwrap();
            assert_eq!(skip_char(&data, start, ch), f[4].parse::<usize>().unwrap());
            assert_eq!(
                skip_until_char(&data, start, ch),
                f[5].parse::<usize>().unwrap()
            );
            n += 1;
        }
        assert!(n >= 5);
    }

    #[test]
    fn is_backslash_escaped_matches_go() {
        let mut n = 0;
        for f in rows("B") {
            let data = unhex(&f[1]);
            let i: usize = f[2].parse().unwrap();
            assert_eq!(
                is_backslash_escaped(&data, i),
                f[3] == "true",
                "is_backslash_escaped({:?}, {i})",
                String::from_utf8_lossy(&data)
            );
            n += 1;
        }
        assert!(n >= 5);
    }

    /// Measured Go answers for the fence and prefix scanners.
    const FENCE_FIXTURE: &str = include_str!("../tests/fixtures/go-fence.txt");

    fn fence_rows(tag: &str) -> impl Iterator<Item = Vec<String>> + '_ {
        FENCE_FIXTURE.lines().filter_map(move |l| {
            let f: Vec<String> = l.split(' ').map(str::to_string).collect();
            (f.first().map(String::as_str) == Some(tag)).then_some(f)
        })
    }

    #[test]
    fn is_fence_line_matches_go_with_and_without_an_info_pointer() {
        let mut n = 0;
        for f in fence_rows("F") {
            let data = unhex(&f[1]);
            let old = unhex(&f[2]);
            let ctx = || {
                format!(
                    "is_fence_line({:?}, old={:?})",
                    String::from_utf8_lossy(&data),
                    String::from_utf8_lossy(&old)
                )
            };

            // info = nil
            let (end, marker) = is_fence_line(&data, None, &old);
            assert_eq!(end, f[3].parse::<usize>().unwrap(), "{} end [nil]", ctx());
            assert_eq!(marker, unhex(&f[4]), "{} marker [nil]", ctx());

            // info = &s
            let mut info = Vec::new();
            let (end, marker) = is_fence_line(&data, Some(&mut info), &old);
            assert_eq!(end, f[6].parse::<usize>().unwrap(), "{} end [info]", ctx());
            assert_eq!(marker, unhex(&f[7]), "{} marker [info]", ctx());
            assert_eq!(info, unhex(&f[8]), "{} info", ctx());
            n += 1;
        }
        assert!(n >= 100, "thin corpus: {n}");
    }

    /// Measured Go answers for `unescapeString` and the regexp guard.
    const CODE_FIXTURE: &str = include_str!("../tests/fixtures/go-code.txt");

    fn code_rows(tag: &str) -> impl Iterator<Item = Vec<String>> + '_ {
        CODE_FIXTURE.lines().filter_map(move |l| {
            let f: Vec<String> = l.split(' ').map(str::to_string).collect();
            (f.first().map(String::as_str) == Some(tag)).then_some(f)
        })
    }

    #[test]
    fn unescape_string_matches_go_including_the_backslash_bug() {
        let mut n = 0;
        for f in code_rows("U") {
            let input = unhex(&f[1]);
            let want = unhex(f.get(2).map(String::as_str).unwrap_or(""));
            assert_eq!(
                unescape_string(&input),
                want,
                "unescape_string({:?})",
                String::from_utf8_lossy(&input)
            );
            n += 1;
        }
        assert!(n >= 18, "thin corpus: {n}");
    }

    #[test]
    fn the_re_backslash_or_amp_guard_ignores_backslashes() {
        // See BUGS.md #1. The fixture records what Go's regexp really matches.
        for f in code_rows("M") {
            let input = unhex(&f[1]);
            let go_matched = f[2] == "true";
            let port_matched = input.contains(&b'&');
            assert_eq!(
                port_matched,
                go_matched,
                "reBackslashOrAmp.Match({:?})",
                String::from_utf8_lossy(&input)
            );
        }
        // Spelled out, because this is the bug rather than an accident:
        assert!(!unescape_string(br"\-").is_empty());
        assert_eq!(unescape_string(br"\-"), br"\-", "escape skipped entirely");
        assert_eq!(unescape_string(br"\-&amp;"), b"-&", "same escape, expanded");
    }

    #[test]
    fn fenced_code_block_matches_go() {
        use crate::markdown::Options;
        let mut n = 0;
        for f in code_rows("F") {
            let data = unhex(&f[1]);
            let ctx = format!("fenced_code_block({:?})", String::from_utf8_lossy(&data));

            // do_render = false, then true: both return values are recorded.
            let mut p = Markdown::new(Options::none());
            assert_eq!(
                p.fenced_code_block(&data, false),
                f[2].parse::<usize>().unwrap(),
                "{ctx} [no render]"
            );
            let mut p = Markdown::new(Options::none());
            assert_eq!(
                p.fenced_code_block(&data, true),
                f[3].parse::<usize>().unwrap(),
                "{ctx} [render]"
            );

            // f[4] is the "|" separator; f[5..] is the rendered node state.
            let mut p = Markdown::new(Options::none());
            let ret = p.fenced_code_block(&data, true);
            assert_eq!(ret, f[5].parse::<usize>().unwrap(), "{ctx} ret");
            let (lit, info, fenced, flen) = match p.arena()[p.document()].first_child() {
                Some(c) => (
                    p.arena()[c].literal.clone(),
                    p.arena()[c].code_block.info.clone(),
                    p.arena()[c].code_block.is_fenced,
                    p.arena()[c].code_block.fence_length,
                ),
                None => (Vec::new(), Vec::new(), false, 0),
            };
            assert_eq!(lit, unhex(&f[6]), "{ctx} literal");
            assert_eq!(info, unhex(&f[7]), "{ctx} info");
            assert_eq!(fenced, f[8] == "true", "{ctx} is_fenced");
            assert_eq!(flen, f[9].parse::<usize>().unwrap(), "{ctx} fence_length");
            n += 1;
        }
        assert!(n >= 15, "thin corpus: {n}");
    }

    #[test]
    fn indented_code_matches_go() {
        use crate::markdown::Options;
        let mut n = 0;
        for f in code_rows("C") {
            let data = unhex(&f[1]);
            let mut p = Markdown::new(Options::none());
            let ret = p.code(&data);
            let (lit, info) = match p.arena()[p.document()].first_child() {
                Some(c) => (
                    p.arena()[c].literal.clone(),
                    p.arena()[c].code_block.info.clone(),
                ),
                None => (Vec::new(), Vec::new()),
            };
            let ctx = format!("code({:?})", String::from_utf8_lossy(&data));
            assert_eq!(ret, f[2].parse::<usize>().unwrap(), "{ctx} ret");
            assert_eq!(lit, unhex(&f[3]), "{ctx} literal");
            assert_eq!(info, unhex(&f[4]), "{ctx} info");
            n += 1;
        }
        assert!(n >= 12, "thin corpus: {n}");
    }

    #[test]
    fn a_fenced_block_needs_a_closing_fence() {
        use crate::markdown::Options;
        let mut p = Markdown::new(Options::none());
        // Runs to the end of the buffer with no closer: rejected outright
        // rather than swallowing the rest of the document.
        assert_eq!(p.fenced_code_block(b"```\nx\n", true), 0);
        assert_eq!(p.arena()[p.document()].first_child(), None);
        // A closer with a different marker does not count.
        let mut p = Markdown::new(Options::none());
        assert_eq!(p.fenced_code_block(b"```\nx\n~~~\n", true), 0);
    }

    #[test]
    fn fence_length_is_the_opening_line_not_the_marker() {
        use crate::markdown::Options;
        let mut p = Markdown::new(Options::none());
        p.fenced_code_block(b"```go\ncode\n```\n", true);
        let c = p.arena()[p.document()].first_child().unwrap();
        assert_eq!(p.arena()[c].code_block.fence_length, 5, "len(\"```go\")");
        assert_eq!(p.arena()[c].code_block.info, b"go");
        assert_eq!(p.arena()[c].literal, b"code\n");
    }

    #[test]
    fn indented_code_keeps_blank_lines_and_collapses_trailing_ones() {
        use crate::markdown::Options;
        let lit = |src: &[u8]| {
            let mut p = Markdown::new(Options::none());
            p.code(src);
            let c = p.arena()[p.document()].first_child().unwrap();
            p.arena()[c].literal.clone()
        };
        assert_eq!(lit(b"    a\n    b\n"), b"a\nb\n");
        // An interior blank line survives as a bare newline.
        assert_eq!(lit(b"    a\n\n    b\n"), b"a\n\nb\n");
        // Trailing blanks collapse to exactly one newline.
        assert_eq!(lit(b"    a\n\n\n"), b"a\n");
        // Tabs count as an indent too.
        assert_eq!(lit(b"\ta\n\tb\n"), b"a\nb\n");
    }

    #[test]
    fn indented_code_stops_at_the_first_unindented_line() {
        use crate::markdown::Options;
        let mut p = Markdown::new(Options::none());
        let consumed = p.code(b"    a\nplain\n");
        assert_eq!(consumed, 6, "stops before \"plain\"");
        let c = p.arena()[p.document()].first_child().unwrap();
        assert_eq!(p.arena()[c].literal, b"a\n");
    }

    /// Measured Go answers for the list-item scanners.
    const LIST_FIXTURE: &str = include_str!("../tests/fixtures/go-list.txt");

    fn list_rows(tag: &str) -> impl Iterator<Item = Vec<String>> + '_ {
        LIST_FIXTURE.lines().filter_map(move |l| {
            let f: Vec<String> = l.split(' ').map(str::to_string).collect();
            (f.first().map(String::as_str) == Some(tag)).then_some(f)
        })
    }

    #[test]
    fn list_item_prefixes_match_go() {
        let mut n = 0;
        for f in list_rows("P") {
            let data = unhex(&f[1]);
            let ctx = format!("{:?}", String::from_utf8_lossy(&data));
            assert_eq!(
                uli_prefix(&data),
                f[2].parse::<usize>().unwrap(),
                "uli_prefix({ctx})"
            );
            assert_eq!(
                oli_prefix(&data),
                f[3].parse::<usize>().unwrap(),
                "oli_prefix({ctx})"
            );
            assert_eq!(
                dli_prefix(&data),
                f[4].parse::<usize>().unwrap(),
                "dli_prefix({ctx})"
            );
            n += 1;
        }
        assert!(n >= 40, "thin corpus: {n}");
    }

    #[test]
    fn list_type_changed_matches_go() {
        let mut n = 0;
        for f in list_rows("T") {
            let data = unhex(&f[1]);
            let flags = crate::ListType::from_bits_retain(f[2].parse::<i32>().unwrap());
            assert_eq!(
                list_type_changed(&data, flags),
                f[3] == "true",
                "list_type_changed({:?}, {flags:?})",
                String::from_utf8_lossy(&data)
            );
            n += 1;
        }
        assert!(n >= 20, "thin corpus: {n}");
    }

    #[test]
    fn prefix_scanners_survive_short_input() {
        // Go's `i >= len(data)-1` relies on signed arithmetic; on usize the
        // same expression would underflow on empty input.
        for d in [&b""[..], b" ", b"*", b"1", b":", b"  ", b"   ", b"1."] {
            let _ = uli_prefix(d);
            let _ = oli_prefix(d);
            let _ = dli_prefix(d);
        }
        assert_eq!(uli_prefix(b""), 0);
        assert_eq!(oli_prefix(b""), 0);
        assert_eq!(dli_prefix(b""), 0);
        assert_eq!(uli_prefix(b"*"), 0, "marker with nothing after it");
        assert_eq!(oli_prefix(b"1."), 0, "dot with nothing after it");
    }

    #[test]
    fn ordered_items_need_a_dot_not_a_paren() {
        assert_eq!(oli_prefix(b"1. x"), 3);
        assert_eq!(oli_prefix(b"1) x"), 0, "blackfriday predates that syntax");
        assert_eq!(oli_prefix(b"12. x"), 4);
        assert_eq!(oli_prefix(b"1.\tx"), 3, "a tab counts too");
    }

    #[test]
    fn dli_prefix_only_ever_returns_zero_or_two() {
        // The space-skipping loop at the end of dliPrefix is dead code: i is
        // still 0 and data[0] is ':'. Confirmed across the whole corpus.
        for f in list_rows("P") {
            let got = dli_prefix(&unhex(&f[1]));
            assert!(got == 0 || got == 2, "dli_prefix returned {got}");
        }
        assert_eq!(dli_prefix(b": x"), 2);
        assert_eq!(dli_prefix(b":  x"), 2, "extra spaces are NOT skipped");
        assert_eq!(dli_prefix(b":   x"), 2);
    }

    #[test]
    fn ends_with_blank_line_is_always_false_upstream() {
        use crate::markdown::Options;
        let mut p = Markdown::new(Options::none());
        let list = p.arena.new_node(NodeType::List);
        let item = p.arena.new_node(NodeType::Item);
        let para = p.arena.new_node(NodeType::Paragraph);
        p.arena.append_child(list, item);
        p.arena.append_child(item, para);

        // Upstream's body is commented out behind a TODO; every shape is false.
        for n in [list, item, para] {
            assert!(!ends_with_blank_line(&p.arena, n));
        }
        for f in list_rows("E") {
            assert_eq!(f[2], "false", "fixture says {} is {}", f[1], f[2]);
        }
    }

    #[test]
    fn finalize_list_closes_the_node_but_never_clears_tight() {
        use crate::markdown::Options;
        let mut p = Markdown::new(Options::none());
        let list = p.arena.new_node(NodeType::List);
        p.arena[list].list.tight = true;
        let i1 = p.arena.new_node(NodeType::Item);
        let i2 = p.arena.new_node(NodeType::Item);
        p.arena.append_child(list, i1);
        p.arena.append_child(list, i2);
        let pa = p.arena.new_node(NodeType::Paragraph);
        let pb = p.arena.new_node(NodeType::Paragraph);
        p.arena.append_child(i1, pa);
        p.arena.append_child(i2, pb);

        finalize_list(&mut p.arena, list);

        assert!(!p.arena[list].open, "the list is closed");
        assert!(
            p.arena[list].list.tight,
            "tight survives, because ends_with_blank_line is stubbed to false"
        );
        // And that is what Go does too.
        let l = list_rows("L").next().unwrap();
        assert_eq!(l[1], "tight=true");
    }

    /// Measured Go answers for the title-block and HTML-block scanners.
    const HTML_FIXTURE: &str = include_str!("../tests/fixtures/go-html.txt");

    fn html_rows(tag: &str) -> impl Iterator<Item = Vec<String>> + '_ {
        HTML_FIXTURE.lines().filter_map(move |l| {
            let f: Vec<String> = l.split(' ').map(str::to_string).collect();
            (f.first().map(String::as_str) == Some(tag)).then_some(f)
        })
    }

    #[test]
    fn title_block_matches_go() {
        use crate::markdown::Options;
        use crate::Extensions;
        let mut n = 0;
        for f in html_rows("T") {
            let data = unhex(&f[1]);
            if f[2] == "PANIC" {
                let got = catch(std::panic::AssertUnwindSafe(|| {
                    let mut p =
                        Markdown::new(Options::none().with_extensions(Extensions::TITLEBLOCK));
                    p.title_block(&data, true)
                }));
                assert!(got.is_none(), "must panic on {data:?}");
                n += 1;
                continue;
            }
            let mut p = Markdown::new(Options::none().with_extensions(Extensions::TITLEBLOCK));
            let consumed = p.title_block(&data, true);
            let ctx = format!("title_block({:?})", String::from_utf8_lossy(&data));
            assert_eq!(consumed, f[2].parse::<usize>().unwrap(), "{ctx} consumed");

            let kids = p.arena().children(p.document()).count();
            assert_eq!(kids, f[3].parse::<usize>().unwrap(), "{ctx} node count");
            if kids > 0 {
                let c = p.arena()[p.document()].first_child().unwrap();
                assert_eq!(p.arena()[c].content, unhex(&f[4]), "{ctx} content");
                assert_eq!(
                    p.arena()[c].heading.level,
                    f[5].parse::<i32>().unwrap(),
                    "{ctx} level"
                );
                assert_eq!(
                    p.arena()[c].heading.is_titleblock,
                    f[6] == "true",
                    "{ctx} is_titleblock"
                );
            }
            n += 1;
        }
        assert!(n >= 10, "thin corpus: {n}");
    }

    #[test]
    fn title_block_reproduces_the_zero_consumption_bug() {
        use crate::markdown::Options;
        use crate::Extensions;
        // See BUGS.md #2. Every line starts with '%' and there is no trailing
        // newline, so Go's scan index never leaves zero: the joined data is
        // empty, consumed is 0, and yet a node is appended anyway.
        let mut p = Markdown::new(Options::none().with_extensions(Extensions::TITLEBLOCK));
        assert_eq!(p.title_block(b"% a", true), 0, "consumes nothing");
        let c = p.arena()[p.document()]
            .first_child()
            .expect("a stray node is still appended -- that is the bug");
        assert_eq!(p.arena()[c].content, b"", "and its content is empty");
        assert!(p.arena()[c].heading.is_titleblock);

        // With the newline it behaves correctly.
        let mut p = Markdown::new(Options::none().with_extensions(Extensions::TITLEBLOCK));
        assert_eq!(p.title_block(b"% a\n", true), 3);
        let c = p.arena()[p.document()].first_child().unwrap();
        assert_eq!(p.arena()[c].content, b"a");
    }

    #[test]
    fn title_block_ignores_do_render_entirely() {
        use crate::markdown::Options;
        use crate::Extensions;
        // Unlike fenced_code_block and html_hr, this one always mutates.
        for render in [true, false] {
            let mut p = Markdown::new(Options::none().with_extensions(Extensions::TITLEBLOCK));
            p.title_block(b"% a\n", render);
            assert!(
                p.arena()[p.document()].first_child().is_some(),
                "do_render={render} still appended a node"
            );
        }
        // The fixture records identical results for both, which is why.
        for f in html_rows("T") {
            let pipe = f.iter().position(|s| s == "|").unwrap();
            assert_eq!(f[2..pipe], f[pipe + 1..], "do_render changed nothing");
        }
    }

    #[test]
    fn html_hr_matches_go() {
        use crate::markdown::Options;
        let mut n = 0;
        for f in html_rows("H") {
            let data = unhex(&f[1]);
            let mut p = Markdown::new(Options::none());
            assert_eq!(
                p.html_hr(&data, false),
                f[2].parse::<usize>().unwrap(),
                "html_hr({:?})",
                String::from_utf8_lossy(&data)
            );
            n += 1;
        }
        assert!(n >= 10, "thin corpus: {n}");
    }

    #[test]
    fn html_find_tag_matches_go() {
        let mut n = 0;
        for f in html_rows("G") {
            let data = unhex(&f[1]);
            let want = if f[3] == "true" {
                Some(String::from_utf8(unhex(&f[2])).unwrap())
            } else {
                None
            };
            assert_eq!(
                html_find_tag(&data).map(str::to_string),
                want,
                "html_find_tag({:?})",
                String::from_utf8_lossy(&data)
            );
            n += 1;
        }
        assert!(n >= 8, "thin corpus: {n}");
    }

    #[test]
    fn html_find_end_matches_go_in_both_lax_modes() {
        let mut n = 0;
        for f in html_rows("F") {
            let tag = &f[1];
            let data = unhex(&f[2]);
            let ctx = format!(
                "html_find_end({tag:?}, {:?})",
                String::from_utf8_lossy(&data)
            );
            assert_eq!(
                html_find_end(tag, &data, false),
                f[3].parse::<usize>().unwrap(),
                "{ctx} strict"
            );
            assert_eq!(
                html_find_end(tag, &data, true),
                f[4].parse::<usize>().unwrap(),
                "{ctx} lax"
            );
            n += 1;
        }
        assert!(n >= 5, "thin corpus: {n}");
    }

    #[test]
    fn block_tags_table_is_sorted_and_case_sensitive() {
        for w in BLOCK_TAGS.windows(2) {
            assert!(w[0] < w[1], "{:?} then {:?}", w[0], w[1]);
        }
        assert_eq!(BLOCK_TAGS.len(), 38);
        assert_eq!(html_find_tag(b"div>"), Some("div"));
        assert_eq!(html_find_tag(b"DIV>"), None, "lookup is case-sensitive");
        assert_eq!(html_find_tag(b"notatag>"), None);
        assert_eq!(html_find_tag(b""), None);
    }

    #[test]
    fn replace_all_matches_go_bytes_replace() {
        assert_eq!(replace_all(b"a\n% b\n% c", b"\n% ", b"\n"), b"a\nb\nc");
        assert_eq!(replace_all(b"aaa", b"a", b"bb"), b"bbbbbb");
        assert_eq!(replace_all(b"abc", b"x", b"y"), b"abc");
        assert_eq!(replace_all(b"", b"x", b"y"), b"");
        assert_eq!(
            replace_all(b"abc", b"", b"y"),
            b"abc",
            "empty old is a no-op"
        );
    }

    /// Measured Go answers for the table scanners.
    const TABLE_FIXTURE: &str = include_str!("../tests/fixtures/go-table.txt");

    fn table_rows(tag: &str) -> impl Iterator<Item = Vec<String>> + '_ {
        TABLE_FIXTURE.lines().filter_map(move |l| {
            let f: Vec<String> = l.split(' ').map(str::to_string).collect();
            (f.first().map(String::as_str) == Some(tag)).then_some(f)
        })
    }

    /// Mirrors the generator's `treeOf`: the document as a flat node listing,
    /// so a scanner that returns the right size while building the wrong tree
    /// still fails.
    fn tree_of(p: &Markdown, node: NodeId, depth: usize) -> String {
        let mut s = String::new();
        for c in p.arena().children(node) {
            s.push_str(&">".repeat(depth));
            let n = &p.arena()[c];
            let content: &[u8] = if n.content.is_empty() {
                &n.literal
            } else {
                &n.content
            };
            s.push_str(&format!(
                "{}({})",
                n.node_type,
                content
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            ));
            if n.node_type == NodeType::TableCell {
                s.push_str(&format!(
                    "[{},{}]",
                    n.table_cell.align.bits(),
                    n.table_cell.is_header
                ));
            }
            s.push(';');
            s.push_str(&tree_of(p, c, depth + 1));
        }
        s
    }

    fn or_dash(s: String) -> String {
        if s.is_empty() {
            "-".to_string()
        } else {
            s
        }
    }

    #[test]
    fn table_header_matches_go() {
        use crate::markdown::Options;
        use crate::Extensions;
        let mut n = 0;
        for f in table_rows("H") {
            let data = unhex(&f[1]);
            let mut p = Markdown::new(Options::none().with_extensions(Extensions::TABLES));
            let (size, cols) = p.table_header(&data);
            let ctx = format!("table_header({:?})", String::from_utf8_lossy(&data));

            assert_eq!(size, f[2].parse::<usize>().unwrap(), "{ctx} size");
            let want_cols = if f[3] == "-" {
                String::new()
            } else {
                f[3].clone()
            };
            let got_cols = cols
                .iter()
                .map(|c| c.bits().to_string())
                .collect::<Vec<_>>()
                .join(",");
            // Columns are only meaningful when size != 0; Go's named returns
            // hand back a partially built slice otherwise.
            if size != 0 {
                assert_eq!(got_cols, want_cols, "{ctx} columns");
            }
            let doc = p.document();
            assert_eq!(or_dash(tree_of(&p, doc, 0)), f[4], "{ctx} tree");
            n += 1;
        }
        assert!(n >= 18, "thin corpus: {n}");
    }

    #[test]
    fn table_matches_go() {
        use crate::markdown::Options;
        use crate::Extensions;
        let mut n = 0;
        for f in table_rows("T") {
            let data = unhex(&f[1]);
            let mut p = Markdown::new(Options::none().with_extensions(Extensions::TABLES));
            let got = p.table(&data);
            let ctx = format!("table({:?})", String::from_utf8_lossy(&data));
            assert_eq!(got, f[2].parse::<usize>().unwrap(), "{ctx} size");
            let doc = p.document();
            assert_eq!(or_dash(tree_of(&p, doc, 0)), f[3], "{ctx} tree");
            n += 1;
        }
        assert!(n >= 18, "thin corpus: {n}");
    }

    #[test]
    fn table_row_matches_go() {
        use crate::markdown::Options;
        use crate::CellAlignFlags;
        use crate::Extensions;
        let cols = [CellAlignFlags::LEFT, CellAlignFlags::RIGHT];
        let mut n = 0;
        for f in table_rows("R") {
            let data = unhex(&f[1]);
            let mut p = Markdown::new(Options::none().with_extensions(Extensions::TABLES));
            p.table_row(&data, &cols, false);
            let doc = p.document();
            assert_eq!(
                or_dash(tree_of(&p, doc, 0)),
                f[2],
                "table_row({:?})",
                String::from_utf8_lossy(&data)
            );
            n += 1;
        }
        assert!(n >= 6, "thin corpus: {n}");
    }

    #[test]
    fn a_failed_table_leaves_no_node_behind() {
        use crate::markdown::Options;
        use crate::Extensions;
        // table() appends the Table node before it knows whether the header
        // parses, and unlinks it again on failure.
        let mut p = Markdown::new(Options::none().with_extensions(Extensions::TABLES));
        assert_eq!(p.table(b"no pipes here\n"), 0);
        assert_eq!(p.arena()[p.document()].first_child(), None);
        assert_eq!(p.tip, p.document(), "the tip is restored too");
    }

    #[test]
    fn table_alignment_comes_from_the_underline_colons() {
        use crate::markdown::Options;
        use crate::CellAlignFlags;
        use crate::Extensions;
        let cols = |src: &[u8]| {
            let mut p = Markdown::new(Options::none().with_extensions(Extensions::TABLES));
            p.table_header(src).1
        };
        assert_eq!(
            cols(b"a | b\n:-- | --:\n"),
            vec![CellAlignFlags::LEFT, CellAlignFlags::RIGHT]
        );
        assert_eq!(
            cols(b"a | b\n:-: | ---\n"),
            vec![CellAlignFlags::CENTER, CellAlignFlags::NONE],
            "colons on both sides give CENTER, which is LEFT|RIGHT"
        );
    }

    #[test]
    fn rows_are_padded_and_truncated_to_the_column_count() {
        use crate::markdown::Options;
        use crate::CellAlignFlags;
        use crate::Extensions;
        let cells = |src: &[u8]| {
            let mut p = Markdown::new(Options::none().with_extensions(Extensions::TABLES));
            p.table_row(src, &[CellAlignFlags::NONE, CellAlignFlags::NONE], false);
            let row = p.arena()[p.document()].first_child().unwrap();
            p.arena()
                .children(row)
                .map(|c| p.arena()[c].content.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(cells(b"1 | 2\n"), vec![b"1".to_vec(), b"2".to_vec()]);
        // Too few: padded with empties.
        assert_eq!(cells(b"1\n"), vec![b"1".to_vec(), b"".to_vec()]);
        // Too many: the excess is silently dropped.
        assert_eq!(cells(b"1 | 2 | 3\n"), vec![b"1".to_vec(), b"2".to_vec()]);
    }

    #[test]
    fn escaped_pipes_do_not_split_cells() {
        use crate::markdown::Options;
        use crate::Extensions;
        let mut p = Markdown::new(Options::none().with_extensions(Extensions::TABLES));
        let (size, cols) = p.table_header(b"a \\| b | c\n--- | ---\n");
        assert_ne!(size, 0, "an escaped pipe still leaves two columns");
        assert_eq!(cols.len(), 2);
    }

    /// Measured Go answers for the HTML-block and paragraph scanners.
    const HTMLBLOCK_FIXTURE: &str = include_str!("../tests/fixtures/go-htmlblock.txt");

    fn hb_rows(tag: &str) -> impl Iterator<Item = Vec<String>> + '_ {
        HTMLBLOCK_FIXTURE.lines().filter_map(move |l| {
            let f: Vec<String> = l.split(' ').map(str::to_string).collect();
            (f.first().map(String::as_str) == Some(tag)).then_some(f)
        })
    }

    #[test]
    fn inline_html_comment_matches_go() {
        use crate::markdown::Options;
        let p = Markdown::new(Options::none());
        let mut n = 0;
        for f in hb_rows("I") {
            let data = unhex(&f[1]);
            assert_eq!(
                p.inline_html_comment(&data),
                f[2].parse::<usize>().unwrap(),
                "inline_html_comment({:?})",
                String::from_utf8_lossy(&data)
            );
            n += 1;
        }
        assert!(n >= 10, "thin corpus: {n}");
    }

    #[test]
    fn html_comment_matches_go() {
        use crate::markdown::Options;
        let mut n = 0;
        for f in hb_rows("C") {
            let data = unhex(&f[1]);
            let mut p = Markdown::new(Options::none());
            let got = p.html_comment(&data, true);
            let ctx = format!("html_comment({:?})", String::from_utf8_lossy(&data));
            assert_eq!(got, f[2].parse::<usize>().unwrap(), "{ctx} size");
            let lit = match p.arena()[p.document()].first_child() {
                Some(c) => p.arena()[c].literal.clone(),
                None => Vec::new(),
            };
            assert_eq!(lit, unhex(&f[3]), "{ctx} literal");
            n += 1;
        }
        assert!(n >= 5, "thin corpus: {n}");
    }

    #[test]
    fn html_block_matches_go_in_both_lax_modes() {
        use crate::markdown::Options;
        use crate::Extensions;
        let mut n = 0;
        for f in hb_rows("B") {
            let data = unhex(&f[1]);
            let pipe = f.iter().position(|s| s == "|").unwrap();
            let ctx = format!("html({:?})", String::from_utf8_lossy(&data));

            for (lax, base) in [(false, 2usize), (true, pipe + 1)] {
                let ext = if lax {
                    Extensions::LAX_HTML_BLOCKS
                } else {
                    Extensions::NONE
                };
                let mut p = Markdown::new(Options::none().with_extensions(ext));
                let got = p.html(&data, true);
                assert_eq!(
                    got,
                    f[base].parse::<usize>().unwrap(),
                    "{ctx} size [lax={lax}]"
                );
                let lit = match p.arena()[p.document()].first_child() {
                    Some(c) => p.arena()[c].literal.clone(),
                    None => Vec::new(),
                };
                assert_eq!(lit, unhex(&f[base + 1]), "{ctx} literal [lax={lax}]");
            }
            n += 1;
        }
        assert!(n >= 12, "thin corpus: {n}");
    }

    #[test]
    fn render_paragraph_matches_go_including_where_go_panics() {
        use crate::markdown::Options;
        let mut panics = 0;
        let mut n = 0;
        for f in hb_rows("P") {
            let data = unhex(&f[1]);
            let got = catch(std::panic::AssertUnwindSafe(|| {
                let mut p = Markdown::new(Options::none());
                p.render_paragraph(&data);
                let kids = p.arena().children(p.document()).count();
                let lit = match p.arena()[p.document()].first_child() {
                    Some(c) => p.arena()[c].content.clone(),
                    None => Vec::new(),
                };
                (kids, lit)
            }));
            if f[2] == "PANIC" {
                assert!(
                    got.is_none(),
                    "upstream panics on {:?}; the port must too",
                    String::from_utf8_lossy(&data)
                );
                panics += 1;
            } else {
                let (kids, lit) = got.expect("should not have panicked");
                let ctx = format!("render_paragraph({:?})", String::from_utf8_lossy(&data));
                assert_eq!(kids, f[2].parse::<usize>().unwrap(), "{ctx} node count");
                assert_eq!(lit, unhex(&f[3]), "{ctx} content");
            }
            n += 1;
        }
        assert!(n >= 9, "thin corpus: {n}");
        assert_eq!(panics, 1, "the all-spaces case");
    }

    #[test]
    fn render_paragraph_panics_on_all_spaces_like_upstream() {
        use crate::markdown::Options;
        // Go's leading-space trim has no length guard. Documented as a latent
        // hazard rather than a reachable bug: every call site passes a slice
        // ending at a line boundary, and an all-spaces line is caught by
        // is_empty before it gets here.
        let got = catch(std::panic::AssertUnwindSafe(|| {
            let mut p = Markdown::new(Options::none());
            p.render_paragraph(b"   ");
        }));
        assert!(got.is_none());

        // A single trailing newline makes it safe again.
        let mut p = Markdown::new(Options::none());
        p.render_paragraph(b"   \n");
        assert_eq!(p.arena().children(p.document()).count(), 1);
    }

    #[test]
    fn html_comment_start_offset_rejects_short_forms() {
        use crate::markdown::Options;
        let p = Markdown::new(Options::none());
        // The scan starts at index 5, and the loop condition is checked before
        // the first increment -- so `<!--->` already has `--` at 3..4 and `>`
        // at 5, and matches at once. `<!-->` is one byte shorter, so `i < len`
        // fails immediately and it does not match at all. Measured; my first
        // guess had both of these at 0.
        assert_eq!(p.inline_html_comment(b"<!-->"), 0, "too short to reach i=5");
        assert_eq!(p.inline_html_comment(b"<!--->"), 6, "matches at i=5");
        assert_eq!(p.inline_html_comment(b"<!---->"), 7);
        assert_eq!(p.inline_html_comment(b"<!--x-->"), 8);
        assert_eq!(p.inline_html_comment(b"<!-- x -->"), 10);
        assert_eq!(p.inline_html_comment(b"<!-- x --"), 0, "no closer");
        // Stops at the first closer, not the last.
        assert_eq!(p.inline_html_comment(b"<!-- a --><!-- b -->"), 10);
    }

    #[test]
    fn ins_and_del_are_excluded_from_the_html_block_search() {
        use crate::markdown::Options;
        // Following original Markdown.pl. Both are in blockTags, but the
        // closing-tag search is skipped for them, so no block is produced.
        for tag in ["ins", "del"] {
            let src = format!("<{tag}>x</{tag}>\n\n");
            let mut p = Markdown::new(Options::none());
            assert_eq!(p.html(src.as_bytes(), true), 0, "<{tag}> must not match");
        }
        // A comparable tag that is not excluded does match.
        let mut p = Markdown::new(Options::none());
        assert!(p.html(b"<div>x</div>\n\n", true) > 0);
    }

    /// Measured Go trees for the block dispatcher, run end to end.
    const DISPATCH_FIXTURE: &str = include_str!("../tests/fixtures/go-dispatch.txt");

    /// Mirrors the generator's `dispatchTree`.
    fn dispatch_tree(p: &Markdown, node: NodeId, depth: usize) -> String {
        let mut s = String::new();
        for c in p.arena().children(node) {
            s.push_str(&">".repeat(depth));
            let n = &p.arena()[c];
            let content: &[u8] = if n.content.is_empty() {
                &n.literal
            } else {
                &n.content
            };
            s.push_str(&format!(
                "{}({})",
                n.node_type,
                content
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            ));
            match n.node_type {
                NodeType::Heading => s.push_str(&format!("[L{}]", n.heading.level)),
                NodeType::List | NodeType::Item => s.push_str(&format!(
                    "[F{},T{}]",
                    n.list.list_flags.bits(),
                    n.list.tight
                )),
                NodeType::CodeBlock => s.push_str(&format!(
                    "[fenced={},info={}]",
                    n.code_block.is_fenced,
                    n.code_block
                        .info
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                )),
                _ => {}
            }
            s.push(';');
            s.push_str(&dispatch_tree(p, c, depth + 1));
        }
        s
    }

    #[test]
    fn block_dispatcher_matches_go_end_to_end() {
        use crate::markdown::Options;
        use crate::Extensions;

        let ext_of = |name: &str| match name {
            "none" => Extensions::NONE,
            "common" => Extensions::COMMON,
            "all" => {
                Extensions::COMMON
                    | Extensions::FOOTNOTES
                    | Extensions::TITLEBLOCK
                    | Extensions::DEFINITION_LISTS
                    | Extensions::LAX_HTML_BLOCKS
                    | Extensions::NO_EMPTY_LINE_BEFORE_BLOCK
                    | Extensions::AUTO_HEADING_IDS
            }
            other => panic!("unknown extension set {other}"),
        };

        let mut n = 0;
        for line in DISPATCH_FIXTURE.lines() {
            let f: Vec<&str> = line.split(' ').collect();
            if f.first() != Some(&"D") {
                continue;
            }
            let data = unhex(f[1]);
            let ext = ext_of(f[2]);
            let want = f[3];

            let mut p = Markdown::new(Options::none().with_extensions(ext));
            p.block(&data);
            let doc = p.document();
            let got = {
                let t = dispatch_tree(&p, doc, 0);
                if t.is_empty() {
                    "-".to_string()
                } else {
                    t
                }
            };
            assert_eq!(
                got,
                want,
                "block({:?}) with {}",
                String::from_utf8_lossy(&data),
                f[2]
            );
            n += 1;
        }
        assert!(n >= 100, "thin corpus: {n}");
    }

    #[test]
    fn nesting_is_bounded() {
        use crate::markdown::Options;
        use crate::Extensions;
        // Deeply nested quotes would recurse without the max_nesting guard.
        let deep = ">".repeat(200) + " x\n";
        let mut p = Markdown::new(Options::none().with_extensions(Extensions::COMMON));
        p.block(deep.as_bytes()); // must return rather than blow the stack
        assert_eq!(p.nesting, 0, "nesting is unwound on the way out");
    }

    #[test]
    fn escapable_is_exactly_the_ascii_punctuation_set() {
        // Upstream spells the class out; this asserts it equals is_punct rather
        // than assuming it.
        let listed = br##"!"#$%&'()*+,./:;<=>?@[\]^_`{|}~-"##;
        for c in 0u8..=255 {
            assert_eq!(is_escapable(c), listed.contains(&c), "byte {c:#04x}");
            assert_eq!(is_escapable(c), crate::util::is_punct(c), "byte {c:#04x}");
        }
    }

    #[test]
    fn char_entity_shapes_match_the_pattern() {
        // &(?:#x[a-f0-9]{1,8}|#[0-9]{1,8}|[a-z][a-z0-9]{1,31}); under (?i)
        assert_eq!(char_entity_len(b"&amp;", 0), Some(5));
        assert_eq!(char_entity_len(b"&AMP;", 0), Some(5), "case-insensitive");
        assert_eq!(char_entity_len(b"&#38;", 0), Some(5));
        assert_eq!(char_entity_len(b"&#x26;", 0), Some(6));
        assert_eq!(char_entity_len(b"&#X26;", 0), Some(6));
        // A name needs at least two characters.
        assert_eq!(char_entity_len(b"&a;", 0), None);
        assert_eq!(char_entity_len(b"&ab;", 0), Some(4));
        // Digits cannot start a name.
        assert_eq!(char_entity_len(b"&1a;", 0), None);
        // The semicolon is mandatory.
        assert_eq!(char_entity_len(b"&amp", 0), None);
        assert_eq!(char_entity_len(b"&;", 0), None);
        assert_eq!(char_entity_len(b"&#;", 0), None);
        // Numeric references cap at eight digits.
        assert_eq!(char_entity_len(b"&#123456789;", 0), None);
        assert_eq!(char_entity_len(b"&#12345678;", 0), Some(11));
    }

    #[test]
    fn quote_and_code_prefix_match_go() {
        let mut n = 0;
        for f in fence_rows("Q") {
            let data = unhex(&f[1]);
            assert_eq!(
                quote_prefix(&data),
                f[2].parse::<usize>().unwrap(),
                "quote_prefix({:?})",
                String::from_utf8_lossy(&data)
            );
            assert_eq!(
                code_prefix(&data),
                f[3].parse::<usize>().unwrap(),
                "code_prefix({:?})",
                String::from_utf8_lossy(&data)
            );
            n += 1;
        }
        assert!(n >= 15);
    }

    #[test]
    fn terminate_blockquote_matches_go() {
        let mut n = 0;
        for f in fence_rows("T") {
            let data = unhex(&f[1]);
            let beg: usize = f[2].parse().unwrap();
            let end: usize = f[3].parse().unwrap();
            assert_eq!(
                terminate_blockquote(&data, beg, end),
                f[4] == "true",
                "terminate_blockquote({:?}, {beg}, {end})",
                String::from_utf8_lossy(&data)
            );
            n += 1;
        }
        assert!(n >= 5);
    }

    #[test]
    fn the_info_pointer_changes_the_result_not_just_the_output() {
        // Passing nil skips the entire brace branch, so a `{` is simply "not a
        // newline" and the fence is rejected. With an info pointer the braces
        // are consumed and the same input is a valid fence. Measured, not
        // guessed -- my first attempt at this test asserted the wrong pair.
        assert_eq!(is_fence_line(b"```{go}", None, b"").0, 0);
        let mut info = Vec::new();
        assert_eq!(is_fence_line(b"```{go}", Some(&mut info), b"").0, 7);
        assert_eq!(info, b"go");

        // Where there is nothing after the marker at all, both agree.
        assert_eq!(is_fence_line(b"```", None, b"").0, 3);
        let mut info = Vec::new();
        assert_eq!(is_fence_line(b"```", Some(&mut info), b"").0, 3);
    }

    #[test]
    fn info_is_written_even_when_the_fence_is_then_rejected() {
        // Go assigns *info before the trailing-newline check, so a rejected
        // fence can still leave the caller's info string modified. Preserved
        // deliberately: a caller reusing one buffer across calls would see it.
        let mut info = Vec::new();
        let (end, marker) = is_fence_line(b"``` {go} extra\n", Some(&mut info), b"");
        assert_eq!((end, marker.as_slice()), (0, &b""[..]));
        assert_eq!(info, b"go", "info was set before the rejection");
    }

    #[test]
    fn fence_marker_must_match_the_opening_one() {
        let (end, marker) = is_fence_line(b"```\n", None, b"");
        assert_eq!((end, marker.as_slice()), (4, &b"```"[..]));
        // A closing fence with a different marker is rejected.
        assert_eq!(is_fence_line(b"~~~\n", None, b"```").0, 0);
        assert_eq!(is_fence_line(b"```\n", None, b"~~~").0, 0);
        // Same character but a different length is also a mismatch.
        assert_eq!(is_fence_line(b"````\n", None, b"```").0, 0);
    }

    #[test]
    fn fence_needs_three_markers_and_at_most_three_leading_spaces() {
        assert_eq!(is_fence_line(b"``\n", None, b"").0, 0);
        assert_eq!(is_fence_line(b"~~\n", None, b"").0, 0);
        assert_eq!(is_fence_line(b"   ```\n", None, b"").0, 7);
        assert_eq!(
            is_fence_line(b"    ```\n", None, b"").0,
            0,
            "four is too many"
        );
    }

    #[test]
    fn fence_info_string_is_trimmed_and_braces_are_stripped() {
        let info_of = |src: &[u8]| {
            let mut info = Vec::new();
            is_fence_line(src, Some(&mut info), b"");
            info
        };
        assert_eq!(info_of(b"```go\n"), b"go");
        assert_eq!(info_of(b"```   go   \n"), b"go");
        assert_eq!(info_of(b"```{go}\n"), b"go");
        assert_eq!(info_of(b"``` {  go  }\n"), b"go");
        assert_eq!(info_of(b"```{}\n"), b"");
        assert_eq!(info_of(b"```\n"), b"");
        // An unclosed brace rejects the fence either way: with an info pointer
        // the scan runs out looking for `}`, and without one the `{` simply is
        // not the newline the fence needs.
        assert_eq!(is_fence_line(b"```{go\n", None, b"").0, 0);
        let mut info = Vec::new();
        assert_eq!(is_fence_line(b"```{go\n", Some(&mut info), b"").0, 0);
    }

    #[test]
    fn trim_space_keeps_interior_invalid_utf8() {
        // The reason trim_space works on bytes: from_utf8_lossy would rewrite
        // the 0xff to U+FFFD, which Go's string([]byte) conversion does not.
        assert_eq!(trim_space(b"  a\xffb  "), b"a\xffb");
        assert_eq!(trim_space(b"\xff"), b"\xff");
        assert_eq!(trim_space(b"  "), b"");
        assert_eq!(trim_space(b""), b"");
        assert_eq!(trim_space(b"a"), b"a");
        assert_eq!(trim_space(b"\t x \n"), b"x");
        // Unicode whitespace counts, matching unicode.IsSpace.
        assert_eq!(trim_space("\u{a0}x\u{2003}".as_bytes()), b"x");
    }

    #[test]
    fn space_headings_extension_changes_the_answer() {
        use crate::markdown::Options;
        use crate::Extensions;
        let plain = Markdown::new(Options::none());
        let spaced = Markdown::new(Options::none().with_extensions(Extensions::SPACE_HEADINGS));
        // "#h" is a heading without the extension, plain text with it.
        assert!(plain.is_prefix_heading(b"#h"));
        assert!(!spaced.is_prefix_heading(b"#h"));
        assert!(plain.is_prefix_heading(b"# h"));
        assert!(spaced.is_prefix_heading(b"# h"));
    }

    #[test]
    fn prefix_heading_builds_a_node_with_level_and_text() {
        use crate::markdown::Options;
        let mut p = Markdown::new(Options::none());
        let skip = p.prefix_heading(b"## Hello\nrest");
        assert_eq!(skip, 8, "skip stops at the newline");
        let h = p.arena()[p.document()].first_child().unwrap();
        assert_eq!(p.arena()[h].node_type, NodeType::Heading);
        assert_eq!(p.arena()[h].heading.level, 2);
        assert_eq!(p.arena()[h].content, b"Hello");
    }

    #[test]
    fn prefix_heading_trims_closing_hashes_unless_escaped() {
        use crate::markdown::Options;
        let text = |src: &[u8]| {
            let mut p = Markdown::new(Options::none());
            p.prefix_heading(src);
            let h = p.arena()[p.document()].first_child().unwrap();
            p.arena()[h].content.clone()
        };
        assert_eq!(text(b"# h #\n"), b"h");
        assert_eq!(text(b"# h ###\n"), b"h");
        assert_eq!(text(b"# h #x\n"), b"h #x");
        // A backslash-escaped hash survives the trim.
        assert_eq!(text(b"# h \\#\n"), b"h \\#");
    }

    #[test]
    fn prefix_heading_emits_nothing_when_the_text_is_empty() {
        use crate::markdown::Options;
        let mut p = Markdown::new(Options::none());
        p.prefix_heading(b"#\n");
        assert_eq!(
            p.arena()[p.document()].first_child(),
            None,
            "an empty heading produces no node at all"
        );
    }

    #[test]
    fn heading_ids_extension_extracts_and_strips_the_id() {
        use crate::markdown::Options;
        use crate::Extensions;
        let mut p = Markdown::new(Options::none().with_extensions(Extensions::HEADING_IDS));
        let skip = p.prefix_heading(b"# Hello {#custom}\n");
        let h = p.arena()[p.document()].first_child().unwrap();
        assert_eq!(p.arena()[h].heading.heading_id, "custom");
        assert_eq!(p.arena()[h].content, b"Hello", "id is stripped from text");
        assert_eq!(skip, 17, "but still consumed");
    }

    #[test]
    fn auto_heading_ids_extension_derives_the_id_from_the_text() {
        use crate::markdown::Options;
        use crate::Extensions;
        let mut p = Markdown::new(Options::none().with_extensions(Extensions::AUTO_HEADING_IDS));
        p.prefix_heading(b"# Hello, World!\n");
        let h = p.arena()[p.document()].first_child().unwrap();
        assert_eq!(p.arena()[h].heading.heading_id, "hello-world");
    }

    #[test]
    fn level_saturates_at_six() {
        use crate::markdown::Options;
        let level = |src: &[u8]| {
            let mut p = Markdown::new(Options::none());
            p.prefix_heading(src);
            let h = p.arena()[p.document()].first_child().unwrap();
            p.arena()[h].heading.level
        };
        assert_eq!(level(b"###### h\n"), 6);
        // A seventh hash is not a level; it becomes part of the text.
        assert_eq!(level(b"####### h\n"), 6);
    }

    #[test]
    fn tables_are_self_consistent() {
        // ASCII sanity, cheap guard against a botched regeneration.
        for c in 'a'..='z' {
            assert!(is_letter_or_number(c));
            assert_eq!(simple_to_lower(c), c);
        }
        for c in 'A'..='Z' {
            assert!(is_letter_or_number(c));
            assert_eq!(simple_to_lower(c), c.to_ascii_lowercase());
        }
        for c in '0'..='9' {
            assert!(is_letter_or_number(c));
        }
        for c in [' ', '-', '.', '!', '\t', '\n'] {
            assert!(!is_letter_or_number(c));
        }
    }
}
