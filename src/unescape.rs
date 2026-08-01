//! A port of Go's `html.UnescapeString`.
//!
//! Upstream's `escLink` (`esc.go:67`) runs link text through
//! `html.UnescapeString` before escaping it, so behavioural equivalence here
//! means matching Go's *standard library*, not just blackfriday. This is a
//! structural port of `$GOROOT/src/html/escape.go` (BSD-3-Clause, The Go
//! Authors) over the tables in the crate-private `html_entities` module.
//!
//! It is a full HTML5 character-reference decoder, which is rather more than
//! the name suggests:
//!
//! - decimal (`&#38;`) and hexadecimal (`&#x26;`, `&#X26;`) numeric references,
//!   with or without the closing semicolon;
//! - named references, including the 106 that are valid without a semicolon;
//! - a **longest-match fallback**: when `&notit;` fails to resolve as a whole,
//!   Go retries progressively shorter prefixes, finds `not`, and produces
//!   `¬it;`;
//! - the Windows-1252 compatibility remapping for numeric references in
//!   `0x80..=0x9F`, so `&#128;` is `€` rather than U+0080.
//!
//! # Overflow is deliberate
//!
//! Go accumulates the numeric value in a `rune`, which is `int32`, and Go's
//! integer arithmetic wraps silently. A long enough digit run therefore
//! overflows into a negative number, which then fails every range check and
//! reaches `utf8.EncodeRune` — where a negative rune is emitted as U+FFFD.
//!
//! Rust panics on overflow in debug builds, so a direct transcription would
//! *crash* on `&#99999999999;` in debug and diverge in release. The
//! accumulation below uses `wrapping_mul`/`wrapping_add` to reproduce Go's
//! behaviour exactly, and the final `char::from_u32` fallback turns the
//! resulting nonsense into U+FFFD the same way `EncodeRune` does.

use crate::html_entities::{entity, entity2, LONGEST_ENTITY_WITHOUT_SEMICOLON};

/// Windows-1252 replacements for numeric references in `0x80..=0x9F`.
///
/// Verbatim from Go's `replacementTable`. Index 0 is what `0x80` becomes.
/// `0x00` mapping to U+FFFD and `0x0D` being a no-op are handled in code, as
/// they are in Go.
const REPLACEMENT_TABLE: [char; 32] = [
    '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}', '\u{017D}', '\u{008F}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
];

#[inline]
fn push_char(out: &mut Vec<u8>, c: char) {
    let mut buf = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

/// Reads one character reference starting at `src` and appends its expansion.
///
/// Returns the index just past whatever was consumed. Precondition:
/// `s[src] == b'&'`.
///
/// Go writes in place into `b[dst:]` with the invariant `dst <= src`; since an
/// expansion is never longer than its source text, appending to a fresh buffer
/// is equivalent and avoids the aliasing entirely.
///
/// Go's `attribute` is `const attribute = false`, so the two branches it guards
/// — the `foo=` special case and the `!attribute` gate on the longest-match
/// fallback — are dead and live respectively at compile time. Only the reachable
/// behaviour is implemented.
fn unescape_entity(s_full: &[u8], src: usize, out: &mut Vec<u8>) -> usize {
    let s = &s_full[src..];
    // i starts at 1 because we already know s[0] == '&'.
    let mut i = 1usize;

    if s.len() <= 1 {
        out.push(s[0]);
        return src + 1;
    }

    if s[i] == b'#' {
        if s.len() <= 3 {
            // Need at least "&#." to have anything to decode.
            out.push(s[0]);
            return src + 1;
        }
        i += 1;
        let mut c = s[i];
        let mut hex = false;
        if c == b'x' || c == b'X' {
            hex = true;
            i += 1;
        }

        // int32 with wrapping, matching Go's rune arithmetic. See module docs.
        let mut x: i32 = 0;
        while i < s.len() {
            c = s[i];
            i += 1;
            if hex {
                if c.is_ascii_digit() {
                    x = x.wrapping_mul(16).wrapping_add((c - b'0') as i32);
                    continue;
                } else if (b'a'..=b'f').contains(&c) {
                    x = x.wrapping_mul(16).wrapping_add((c - b'a' + 10) as i32);
                    continue;
                } else if (b'A'..=b'F').contains(&c) {
                    x = x.wrapping_mul(16).wrapping_add((c - b'A' + 10) as i32);
                    continue;
                }
            } else if c.is_ascii_digit() {
                x = x.wrapping_mul(10).wrapping_add((c - b'0') as i32);
                continue;
            }
            if c != b';' {
                i -= 1;
            }
            break;
        }

        if i <= 3 {
            // No digits matched.
            out.push(s[0]);
            return src + 1;
        }

        let ch = if (0x80..=0x9F).contains(&x) {
            REPLACEMENT_TABLE[(x - 0x80) as usize]
        } else if x == 0 || (0xD800..=0xDFFF).contains(&x) || x > 0x10FFFF {
            '\u{FFFD}'
        } else {
            // Catches the wrapped-negative case, exactly as EncodeRune does.
            char::from_u32(x as u32).unwrap_or('\u{FFFD}')
        };
        push_char(out, ch);
        return src + i;
    }

    // Consume the maximum run of characters that could name a reference.
    while i < s.len() {
        let c = s[i];
        i += 1;
        if c.is_ascii_alphanumeric() {
            continue;
        }
        if c != b';' {
            i -= 1;
        }
        break;
    }

    let entity_name = &s[1..i];
    if entity_name.is_empty() {
        // No-op; falls through to the verbatim copy below.
    } else if let Some(x) = entity(entity_name) {
        push_char(out, x);
        return src + i;
    } else if let Some((a, b)) = entity2(entity_name) {
        push_char(out, a);
        push_char(out, b);
        return src + i;
    } else {
        // Longest-match fallback: retry shorter prefixes, since an entity
        // written without its semicolon may be a prefix of the run consumed
        // above. "&notit;" resolves "not" and leaves "it;" as literal text.
        let mut max_len = entity_name.len() - 1;
        if max_len > LONGEST_ENTITY_WITHOUT_SEMICOLON {
            max_len = LONGEST_ENTITY_WITHOUT_SEMICOLON;
        }
        let mut j = max_len;
        while j > 1 {
            if let Some(x) = entity(&entity_name[..j]) {
                push_char(out, x);
                return src + j + 1;
            }
            j -= 1;
        }
    }

    out.extend_from_slice(&s[..i]);
    src + i
}

/// Unescapes HTML character references, mirroring Go's `html.UnescapeString`.
///
/// Operates on bytes rather than `&str`: Go's `string(b)` conversion does not
/// validate UTF-8, so malformed input round-trips through the original
/// unchanged, and it must here too.
///
/// ```
/// # use blackfriday::unescape::unescape_string;
/// assert_eq!(unescape_string(b"a&amp;b"), b"a&b");
/// assert_eq!(unescape_string(b"&#38;"), b"&");
/// assert_eq!(unescape_string(b"&#x26;"), b"&");
/// // Longest-match: "not" resolves, "it;" is left alone.
/// assert_eq!(unescape_string("&notit;".as_bytes()), "¬it;".as_bytes());
/// ```
pub fn unescape_string(s: &[u8]) -> Vec<u8> {
    let Some(first) = s.iter().position(|&c| c == b'&') else {
        return s.to_vec();
    };

    let mut out = Vec::with_capacity(s.len());
    out.extend_from_slice(&s[..first]);
    let mut src = unescape_entity(s, first, &mut out);

    while src < s.len() {
        let i = if s[src] == b'&' {
            0
        } else {
            match s[src..].iter().position(|&c| c == b'&') {
                Some(k) => k,
                None => {
                    out.extend_from_slice(&s[src..]);
                    break;
                }
            }
        };
        if i > 0 {
            out.extend_from_slice(&s[src..src + i]);
        }
        src = unescape_entity(s, src + i, &mut out);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn un(s: &[u8]) -> Vec<u8> {
        unescape_string(s)
    }

    /// Measured Go output, produced by `tools/genhtmlent`.
    const FIXTURE: &str = include_str!("../tests/fixtures/go-unescape.txt");

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
            if f.first() != Some(&"U") {
                continue;
            }
            let input = unhex(f[1]);
            let want = unhex(f.get(2).copied().unwrap_or(""));
            assert_eq!(
                un(&input),
                want,
                "unescape_string({:?})",
                String::from_utf8_lossy(&input)
            );
            checked += 1;
        }
        assert!(checked >= 50, "expected a real corpus, got {checked}");
    }

    #[test]
    fn plain_text_is_returned_unchanged() {
        assert_eq!(un(b""), b"");
        assert_eq!(un(b"no entities here"), b"no entities here");
    }

    #[test]
    fn named_references_decode() {
        assert_eq!(un(b"&amp;"), b"&");
        assert_eq!(un(b"&lt;&gt;"), b"<>");
        assert_eq!(un("&aacute;".as_bytes()), "á".as_bytes());
        assert_eq!(un(b"x&amp;y"), b"x&y");
    }

    #[test]
    fn semicolon_less_named_references_decode() {
        // 106 entities are valid without their semicolon.
        assert_eq!(un(b"&amp"), b"&");
        assert_eq!(un(b"&lt"), b"<");
        assert_eq!(un(b"&ampx"), b"&x");
    }

    #[test]
    fn longest_match_fallback_retries_shorter_prefixes() {
        // "notit;" is not an entity, but "not" is; the rest is literal.
        assert_eq!(un("&notit;".as_bytes()), "¬it;".as_bytes());
        assert_eq!(un("&noti".as_bytes()), "¬i".as_bytes());
    }

    #[test]
    fn numeric_references_decode_in_both_bases() {
        assert_eq!(un(b"&#38;"), b"&");
        assert_eq!(un(b"&#x26;"), b"&");
        assert_eq!(un(b"&#X26;"), b"&");
        assert_eq!(un(b"&#38"), b"&"); // semicolon optional
        assert_eq!(un("&#225;".as_bytes()), "á".as_bytes());
        assert_eq!(un("&#xE1;".as_bytes()), "á".as_bytes());
    }

    #[test]
    fn windows_1252_compatibility_remapping() {
        // The reason a naive port gets these wrong: 0x80..=0x9F are remapped,
        // not taken literally.
        assert_eq!(un("&#128;".as_bytes()), "€".as_bytes()); // not U+0080
        assert_eq!(un("&#x80;".as_bytes()), "€".as_bytes());
        assert_eq!(un("&#153;".as_bytes()), "™".as_bytes()); // U+2122
        assert_eq!(un("&#159;".as_bytes()), "\u{178}".as_bytes()); // last entry
    }

    #[test]
    fn invalid_code_points_become_replacement_char() {
        assert_eq!(un(b"&#0;"), "\u{FFFD}".as_bytes());
        assert_eq!(un(b"&#xD800;"), "\u{FFFD}".as_bytes()); // surrogate
        assert_eq!(un(b"&#x110000;"), "\u{FFFD}".as_bytes()); // above max
    }

    #[test]
    fn overflowing_numeric_reference_does_not_panic() {
        // Go wraps int32 silently; a direct transcription would panic here in
        // a debug build. See the module docs.
        assert_eq!(un(b"&#99999999999;"), "\u{FFFD}".as_bytes());
        assert_eq!(un(b"&#xFFFFFFFFFF;"), "\u{FFFD}".as_bytes());
        let long = [b"&#".as_slice(), &b"9".repeat(400), b";"].concat();
        let _ = un(&long); // must not panic
    }

    #[test]
    fn malformed_references_are_left_alone() {
        assert_eq!(un(b"&"), b"&");
        assert_eq!(un(b"&;"), b"&;");
        assert_eq!(un(b"&#"), b"&#");
        assert_eq!(un(b"&#;"), b"&#;");
        assert_eq!(un(b"a & b"), b"a & b");
    }

    #[test]
    fn empty_hex_reference_decodes_to_replacement_not_literal() {
        // Surprising, and measured rather than assumed: "&#x;" is four bytes,
        // so it clears the `len(s) <= 3` guard. After consuming 'x' and then
        // ';' the cursor sits at 4, which also clears the `i <= 3` "no digits
        // matched" check -- so the accumulated value 0 is decoded, and 0 maps
        // to U+FFFD. "&#;" is only three bytes and does stay literal.
        assert_eq!(un(b"&#x;"), "\u{FFFD}".as_bytes());
        assert_eq!(un(b"&#X;"), "\u{FFFD}".as_bytes());
        assert_eq!(un(b"&#;"), b"&#;");
    }

    #[test]
    fn unknown_entity_still_hits_the_longest_match_fallback() {
        // Another one worth pinning: this does NOT stay literal. The fallback
        // walks back to "not", so the tail becomes ordinary text. Blackfriday's
        // own escape_html has no such rule and leaves the same input alone,
        // which is why the two tables must not be confused.
        assert_eq!(un(b"&notarealentity;"), "¬arealentity;".as_bytes());
        assert_eq!(un(b"&notin;"), "∉".as_bytes()); // a real entity, wins outright
        assert_eq!(un(b"&not"), "¬".as_bytes());
    }

    #[test]
    fn consecutive_and_trailing_references() {
        assert_eq!(un(b"&amp;&amp;"), b"&&");
        assert_eq!(un(b"&amp;&lt;&gt;"), b"&<>");
        assert_eq!(un(b"tail&amp;"), b"tail&");
        assert_eq!(un(b"&amp;head"), b"&head");
    }

    #[test]
    fn two_code_point_entities_expand_to_both() {
        // entity2: 91 names decode to a pair.
        assert_eq!(
            un(b"&NotEqualTilde;"),
            "\u{2242}\u{338}".as_bytes(),
            "must emit both code points"
        );
    }

    #[test]
    fn invalid_utf8_round_trips() {
        assert_eq!(un(b"\xff\xfe"), b"\xff\xfe");
        assert_eq!(un(b"\xff&amp;\xfe"), b"\xff&\xfe".to_vec());
    }

    #[test]
    fn output_is_never_longer_than_input() {
        // The invariant Go relies on for in-place rewriting.
        for case in [
            &b"&amp;"[..],
            b"&#38;",
            b"&NotEqualTilde;",
            b"&notit;",
            b"&#x110000;",
            b"&amp",
        ] {
            assert!(
                un(case).len() <= case.len(),
                "expansion grew for {:?}",
                String::from_utf8_lossy(case)
            );
        }
    }
}
