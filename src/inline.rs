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

// Nothing outside the crate can reach the inline parser until the `run` entry
// point lands, so rustc sees these as unused. The allow comes off with `run`,
// as it does in `block` and `html`.
#![allow(dead_code)]

use crate::util::{is_alnum, is_space};

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
