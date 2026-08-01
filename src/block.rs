//! Block-level parsing, ported from upstream `block.go`.
//!
//! Currently holds `SanitizedAnchorName`; the block scanners land in
//! subsequent commits.

use crate::unicode_tables::{is_letter_or_number, simple_to_lower};

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
