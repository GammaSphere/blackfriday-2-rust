//! HTML escaping, ported from upstream `esc.go`.
//!
//! # Output goes to a `Vec<u8>`, not an `io::Write`
//!
//! Upstream's signatures take an `io.Writer` and discard every error the writes
//! return — `w.Write(...)` appears bare throughout `esc.go` and `html.go`. The
//! observable behaviour is therefore "append bytes, never fail", which is
//! exactly `Vec<u8>`. Threading `io::Result` through the renderer would add an
//! error path the original does not have and that no caller could observe, so
//! the port appends to a buffer and the public entry point hands back the
//! finished bytes.
//!
//! # Two different entity tables are in play
//!
//! [`escape_html`] consults blackfriday's own table (the crate-private
//! `entities` module) to decide whether an `&` already opens a valid entity.
//! [`esc_link`] first runs its input through [`crate::unescape`], a port of
//! Go's `html.UnescapeString`, which uses Go's *standard library* table
//! instead. The two tables differ in keys, values and size; the comparison is
//! in the `html_entities` module docs.

use crate::entities;
use crate::unescape::unescape_string;
use crate::util::is_alnum;

/// The four bytes upstream rewrites, and what it rewrites them to.
///
/// Go spells this as a sparse `[256][]byte` and tests entries against `nil`.
#[inline]
const fn html_escape_seq(c: u8) -> Option<&'static [u8]> {
    match c {
        b'&' => Some(b"&amp;"),
        b'<' => Some(b"&lt;"),
        b'>' => Some(b"&gt;"),
        b'"' => Some(b"&quot;"),
        _ => None,
    }
}

/// Escapes HTML metacharacters, leaving already-valid entities intact.
///
/// `&amp;` survives as `&amp;` rather than becoming `&amp;amp;`, while a bare
/// `&` becomes `&amp;`.
///
/// ```
/// # use blackfriday::esc::escape_html;
/// let mut out = Vec::new();
/// escape_html(&mut out, b"AT&T and AT&amp;T");
/// assert_eq!(out, b"AT&amp;T and AT&amp;T");
/// ```
pub fn escape_html(out: &mut Vec<u8>, s: &[u8]) {
    escape_entities(out, s, false);
}

/// Escapes HTML metacharacters unconditionally, including inside valid
/// entities.
///
/// Used for code spans and code blocks, where an entity in the source is
/// literal text rather than markup.
///
/// ```
/// # use blackfriday::esc::escape_all_html;
/// let mut out = Vec::new();
/// escape_all_html(&mut out, b"AT&amp;T");
/// assert_eq!(out, b"AT&amp;amp;T");
/// ```
pub fn escape_all_html(out: &mut Vec<u8>, s: &[u8]) {
    escape_entities(out, s, true);
}

/// Unescapes link text, then escapes it for HTML output.
///
/// Ported from `escLink` (`esc.go:67`). The round trip is what normalises a
/// destination that already contains entities: `?a=1&amp;b=2` unescapes to
/// `?a=1&b=2` and re-escapes to `?a=1&amp;b=2`, while a raw `?a=1&b=2` reaches
/// the same result from the other direction.
///
/// The unescape half is Go's `html.UnescapeString`, so numeric references and
/// semicolon-less entities are decoded here even though `escape_html` alone
/// would not recognise them.
///
/// ```
/// # use blackfriday::esc::esc_link;
/// let mut out = Vec::new();
/// esc_link(&mut out, b"http://example.com/?a=1&b=2");
/// assert_eq!(out, b"http://example.com/?a=1&amp;b=2");
///
/// let mut out = Vec::new();
/// esc_link(&mut out, b"?x=&#38;");
/// assert_eq!(out, b"?x=&amp;");
/// ```
pub fn esc_link(out: &mut Vec<u8>, text: &[u8]) {
    let unescaped = unescape_string(text);
    escape_html(out, &unescaped);
}

/// The shared body of [`escape_html`] and [`escape_all_html`].
///
/// Ported structurally from Go rather than rewritten, because the interplay
/// between `start` and `end` is load-bearing. `end` advances one byte at a
/// time, but a recognised entity jumps `start` past the whole entity — so for
/// several iterations afterwards `start` sits *ahead* of `end`, and the loop
/// relies on none of the skipped bytes being escapable.
///
/// That holds, and it is worth knowing why: `s[start..end]` would panic if
/// `start > end`, so the only thing standing between this loop and a panic is
/// that a matched entity cannot contain an escapable byte. Every key in the
/// table is `&`, then ASCII alphanumerics, then `;` — no interior `&`, no `<`,
/// `>` or `"` — verified across all 2,231 entries. A malformed run like
/// `&&amp;` does not match, so it takes the escaping branch instead.
fn escape_entities(out: &mut Vec<u8>, s: &[u8], escape_valid_entities: bool) {
    let mut start = 0usize;
    let mut end = 0usize;

    while end < s.len() {
        if let Some(esc_seq) = html_escape_seq(s[end]) {
            let (is_entity, entity_end) = node_is_entity(s, end);
            if is_entity && !escape_valid_entities {
                debug_assert!(start <= entity_end + 1);
                out.extend_from_slice(&s[start..entity_end + 1]);
                start = entity_end + 1;
            } else {
                debug_assert!(
                    start <= end,
                    "an entity containing an escapable byte would land here"
                );
                out.extend_from_slice(&s[start..end]);
                out.extend_from_slice(esc_seq);
                start = end + 1;
            }
        }
        end += 1;
    }

    if start < s.len() && end <= s.len() {
        out.extend_from_slice(&s[start..end]);
    }
}

/// Decides whether the `&` at `end` opens a known entity.
///
/// Returns the verdict and the index of the closing `;` — or of the byte that
/// ended the scan when there is no match. Note the scan tolerates `&` and `#`
/// mid-run even though no table entry contains either; that tolerance is
/// upstream's and is preserved.
fn node_is_entity(s: &[u8], end: usize) -> (bool, usize) {
    let mut is_entity = false;
    let mut end_entity_pos = end + 1;

    if s[end] == b'&' {
        while end_entity_pos < s.len() {
            if s[end_entity_pos] == b';' && entities::is_entity(&s[end..end_entity_pos + 1]) {
                is_entity = true;
                break;
            }
            if !is_alnum(s[end_entity_pos])
                && s[end_entity_pos] != b'&'
                && s[end_entity_pos] != b'#'
            {
                break;
            }
            end_entity_pos += 1;
        }
    }

    (is_entity, end_entity_pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn esc(s: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        escape_html(&mut out, s);
        out
    }

    fn esc_all(s: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        escape_all_html(&mut out, s);
        out
    }

    /// Measured Go output for both escapers, produced by `tools/genentities`
    /// against blackfriday v2.1.0. Hex-encoded: the corpus includes inputs
    /// that are not valid UTF-8.
    const FIXTURE: &str = include_str!("../tests/fixtures/go-esc.txt");

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
            if f.first() != Some(&"E") {
                continue;
            }
            let input = unhex(f[1]);
            let want_html = unhex(f[2]);
            let want_all = unhex(f[3]);
            assert_eq!(
                esc(&input),
                want_html,
                "escape_html({:?})",
                String::from_utf8_lossy(&input)
            );
            assert_eq!(
                esc_all(&input),
                want_all,
                "escape_all_html({:?})",
                String::from_utf8_lossy(&input)
            );
            checked += 1;
        }
        assert!(checked >= 40, "expected a real corpus, got {checked}");
    }

    /// The exact table from the pinned `TestEsc` (`esc_test.go:8`).
    #[test]
    fn matches_pinned_test_esc() {
        for (input, want) in [
            (&b"abc"[..], &b"abc"[..]),
            (b"a&c", b"a&amp;c"),
            (b"<", b"&lt;"),
            (b"[]:<", b"[]:&lt;"),
            (b"Hello <!--", b"Hello &lt;!--"),
        ] {
            assert_eq!(
                esc(input),
                want,
                "escape_html({:?})",
                String::from_utf8_lossy(input)
            );
        }
    }

    #[test]
    fn valid_entities_pass_through_unchanged() {
        assert_eq!(esc(b"AT&amp;T"), b"AT&amp;T");
        assert_eq!(esc(b"&lt;"), b"&lt;");
        assert_eq!(esc(b"&nbsp;x"), b"&nbsp;x");
        // Bare entity forms have no trailing ';', so nodeIsEntity never
        // recognises them and the & is escaped.
        assert_eq!(esc(b"&amp"), b"&amp;amp");
    }

    #[test]
    fn unknown_entities_are_escaped() {
        assert_eq!(esc(b"&notarealentity;"), b"&amp;notarealentity;");
        assert_eq!(esc(b"&;"), b"&amp;;");
        assert_eq!(esc(b"&"), b"&amp;");
    }

    #[test]
    fn escape_all_html_rewrites_valid_entities_too() {
        assert_eq!(esc_all(b"AT&amp;T"), b"AT&amp;amp;T");
        assert_eq!(esc_all(b"&lt;"), b"&amp;lt;");
        // The other three metacharacters behave the same either way.
        assert_eq!(esc_all(b"a<b>c\"d"), esc(b"a<b>c\"d"));
    }

    #[test]
    fn all_four_metacharacters_are_rewritten() {
        assert_eq!(esc(b"<"), b"&lt;");
        assert_eq!(esc(b">"), b"&gt;");
        assert_eq!(esc(b"\""), b"&quot;");
        assert_eq!(esc(b"&"), b"&amp;");
        assert_eq!(esc(b"<>&\""), b"&lt;&gt;&amp;&quot;");
        // Single quote is deliberately NOT escaped.
        assert_eq!(esc(b"'"), b"'");
    }

    #[test]
    fn empty_input_produces_empty_output() {
        assert_eq!(esc(b""), b"");
        assert_eq!(esc_all(b""), b"");
    }

    #[test]
    fn adjacent_ampersands_do_not_confuse_the_scanner() {
        // The scanner tolerates '&' mid-entity, so "&&amp;" is scanned as one
        // run that fails to match, then rescanned from the second '&' which
        // does match. This is the case that would panic if a matched entity
        // could contain an escapable byte.
        assert_eq!(esc(b"&&amp;"), b"&amp;&amp;");
        assert_eq!(esc(b"&&"), b"&amp;&amp;");
        assert_eq!(esc(b"&a&amp;"), b"&amp;a&amp;");
        assert_eq!(esc(b"&&&"), b"&amp;&amp;&amp;");
    }

    #[test]
    fn entity_at_end_of_input_is_handled() {
        // start lands exactly on s.len(), so the trailing write must not fire.
        assert_eq!(esc(b"x&amp;"), b"x&amp;");
        assert_eq!(esc(b"&amp;"), b"&amp;");
    }

    #[test]
    fn text_after_an_entity_is_not_lost() {
        // Regression guard for the start/end interplay: `end` is still inside
        // the entity when `start` has already moved past it.
        assert_eq!(esc(b"&amp;tail"), b"&amp;tail");
        assert_eq!(esc(b"a&amp;b&amp;c"), b"a&amp;b&amp;c");
        assert_eq!(esc(b"&amp;&lt;&gt;"), b"&amp;&lt;&gt;");
    }

    #[test]
    fn numeric_references_are_escaped_not_recognised() {
        // '#' keeps the scan going but no table key contains one, so these
        // never match and the & is escaped.
        assert_eq!(esc(b"&#38;"), b"&amp;#38;");
        assert_eq!(esc(b"&#x26;"), b"&amp;#x26;");
    }

    #[test]
    fn case_sensitivity_carries_through() {
        assert_eq!(esc(b"&AMP;"), b"&AMP;");
        assert_eq!(esc(b"&Gt;"), b"&Gt;"); // U+226B, a real and distinct entity
        assert_eq!(esc(b"&gT;"), b"&amp;gT;");
    }

    #[test]
    fn output_appends_rather_than_replacing() {
        let mut out = b"existing:".to_vec();
        escape_html(&mut out, b"<");
        assert_eq!(out, b"existing:&lt;");
    }

    #[test]
    fn non_utf8_bytes_pass_through_untouched() {
        // Escaping is byte-oriented; invalid UTF-8 is not the escaper's problem.
        assert_eq!(esc(b"\xff\xfe<"), b"\xff\xfe&lt;".to_vec());
        assert_eq!(esc(b"caf\xe9 & bar"), b"caf\xe9 &amp; bar".to_vec());
    }

    #[test]
    fn long_run_without_metacharacters_is_copied_once() {
        let input = b"the quick brown fox jumps over the lazy dog".repeat(10);
        assert_eq!(esc(&input), input);
    }
}
