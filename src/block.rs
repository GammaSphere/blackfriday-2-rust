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

// The scanners below are crate-internal and are driven by the `block`
// dispatcher, which lands with the rest of the block constructs. Until then
// only the tests reach them. Removed once `run` wires the pipeline together.
#![allow(dead_code)]

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
pub(crate) fn is_fence_line(
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
