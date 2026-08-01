//! Byte-level helpers, ported from the bottom of upstream `markdown.go`.
//!
//! Everything here operates on raw bytes, not `char`s. That is deliberate:
//! blackfriday's block and inline scanners index into `[]byte` and compare
//! single bytes, so a port that reached for `char` would have to decode UTF-8
//! at every step and would disagree with the original on malformed input.
//!
//! # `expandTabs` is not ported
//!
//! Upstream defines `expandTabs` at `markdown.go:842` and never calls it.
//! Grepping the pinned commit for the identifier finds exactly one hit — the
//! definition itself — across all source and test files. It is unreachable
//! dead code in v2.
//!
//! Porting it would mean reproducing Go's `utf8.DecodeRune` error semantics
//! (invalid sequences advance one byte and count as one column, overlong
//! encodings and surrogates are rejected) purely to satisfy a function nothing
//! can call. That is a meaningful amount of subtle code with no observable
//! behaviour to be equivalent to, so it is deliberately omitted and recorded
//! here instead. If a future change makes it reachable, this note is the
//! reason it is missing.

/// Test if a character is a punctuation symbol.
///
/// Matches Go's `ispunct`, which walks the literal set
/// ``!"#$%&'()*+,-./:;<=>?@[\]^_`{|}~`` — the ASCII punctuation block, and
/// nothing outside ASCII.
#[inline]
pub const fn is_punct(c: u8) -> bool {
    matches!(
        c,
        b'!' | b'"'
            | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'\''
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b'-'
            | b'.'
            | b'/'
            | b':'
            | b';'
            | b'<'
            | b'='
            | b'>'
            | b'?'
            | b'@'
            | b'['
            | b'\\'
            | b']'
            | b'^'
            | b'_'
            | b'`'
            | b'{'
            | b'|'
            | b'}'
            | b'~'
    )
}

/// Test if a character is a whitespace character.
///
/// Note this is *not* ASCII whitespace in the usual sense: it is horizontal or
/// vertical space as blackfriday defines them below.
#[inline]
pub const fn is_space(c: u8) -> bool {
    is_horizontal_space(c) || is_vertical_space(c)
}

/// Test if a character is horizontal whitespace: space or tab.
#[inline]
pub const fn is_horizontal_space(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

/// Test if a character is vertical whitespace.
///
/// Includes form feed and vertical tab, which many Markdown implementations
/// leave out.
#[inline]
pub const fn is_vertical_space(c: u8) -> bool {
    c == b'\n' || c == b'\r' || c == 0x0c || c == 0x0b
}

/// Test if a character is an ASCII letter.
///
/// ASCII only — no Unicode. Upstream carries a `TODO` wondering whether some
/// call sites ought to be Unicode-aware; the port preserves the current
/// behaviour rather than the aspiration.
#[inline]
pub const fn is_letter(c: u8) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_uppercase()
}

/// Test if a character is an ASCII letter or digit.
#[inline]
pub const fn is_alnum(c: u8) -> bool {
    c.is_ascii_digit() || is_letter(c)
}

/// Find whether a line counts as indented.
///
/// Returns the number of characters the indent occupies, or `0` when the line
/// is not indented. A leading tab counts as an indent of `1` regardless of
/// `indent_size`.
pub fn is_indented(data: &[u8], indent_size: usize) -> usize {
    if data.is_empty() {
        return 0;
    }
    if data[0] == b'\t' {
        return 1;
    }
    if data.len() < indent_size {
        return 0;
    }
    for &b in &data[..indent_size] {
        if b != b' ' {
            return 0;
        }
    }
    indent_size
}

/// Create a URL-safe slug for fragments.
///
/// Every run of non-alphanumeric bytes collapses to a single `-`, and leading
/// and trailing dashes are trimmed. Case is **preserved** — `slugify` is not
/// `SanitizedAnchorName`, which lowercases and is Unicode-aware. The two are
/// easy to confuse; upstream uses this one for link fragments and footnote
/// anchors.
///
/// Two behaviours here look like bugs and are not:
///
/// - An all-punctuation input does not slug to the empty string. `slugify("!")`
///   and `slugify("!!!")` both give `"-"`, because the trailing-trim loop is
///   guarded by `b > 0` and so never trims index 0.
/// - Because a run of symbols emits only one `-`, the output can never contain
///   two adjacent dashes, which is what keeps the trim loops from crossing.
pub fn slugify(input: &[u8]) -> Vec<u8> {
    if input.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<u8> = Vec::with_capacity(input.len());
    let mut sym = false;

    for &ch in input {
        if is_alnum(ch) {
            sym = false;
            out.push(ch);
        } else if sym {
            continue;
        } else {
            out.push(b'-');
            sym = true;
        }
    }

    // Go: `for a, ch = range out { if ch != '-' { break } }` leaves `a` at the
    // index it broke on, or at the final index if it never broke.
    let mut a = 0usize;
    for (i, &ch) in out.iter().enumerate() {
        a = i;
        if ch != b'-' {
            break;
        }
    }
    // Go: `for b = len(out) - 1; b > 0; b--`. The `b > 0` guard is why index 0
    // survives even when it holds a dash.
    let mut b = out.len() - 1;
    while b > 0 {
        if out[b] != b'-' {
            break;
        }
        b -= 1;
    }

    out[a..=b].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Measured Go answers, produced by `tools/genhelpers` against
    /// blackfriday v2.1.0. Byte strings are hex-encoded because `slugify`
    /// works on bytes and its output need not be valid UTF-8.
    const FIXTURE: &str = include_str!("../tests/fixtures/go-helpers.txt");

    fn unhex(s: &str) -> Vec<u8> {
        assert!(s.len() % 2 == 0, "odd-length hex: {s:?}");
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("bad hex"))
            .collect()
    }

    #[test]
    fn byte_predicates_match_go_for_all_256_bytes() {
        let mut checked = 0;
        for line in FIXTURE.lines() {
            let f: Vec<&str> = line.split(' ').collect();
            if f.first() != Some(&"B") {
                continue;
            }
            let c = unhex(f[1])[0];
            let want = |s: &str| s == "true";
            assert_eq!(is_punct(c), want(f[2]), "is_punct({c:#04x})");
            assert_eq!(is_space(c), want(f[3]), "is_space({c:#04x})");
            assert_eq!(
                is_horizontal_space(c),
                want(f[4]),
                "is_horizontal_space({c:#04x})"
            );
            assert_eq!(
                is_vertical_space(c),
                want(f[5]),
                "is_vertical_space({c:#04x})"
            );
            assert_eq!(is_letter(c), want(f[6]), "is_letter({c:#04x})");
            assert_eq!(is_alnum(c), want(f[7]), "is_alnum({c:#04x})");
            checked += 1;
        }
        assert_eq!(checked, 256, "fixture must cover every byte");
    }

    #[test]
    fn slugify_matches_go() {
        let mut checked = 0;
        for line in FIXTURE.lines() {
            let f: Vec<&str> = line.split(' ').collect();
            if f.first() != Some(&"S") {
                continue;
            }
            let input = unhex(f[1]);
            let want = unhex(f[2]);
            let got = slugify(&input);
            assert_eq!(
                got,
                want,
                "slugify({:?}) got {:?} want {:?}",
                String::from_utf8_lossy(&input),
                String::from_utf8_lossy(&got),
                String::from_utf8_lossy(&want)
            );
            checked += 1;
        }
        assert!(checked >= 30, "expected a real corpus, got {checked}");
    }

    #[test]
    fn is_indented_matches_go() {
        let mut checked = 0;
        for line in FIXTURE.lines() {
            let f: Vec<&str> = line.split(' ').collect();
            if f.first() != Some(&"I") {
                continue;
            }
            let data = unhex(f[1]);
            let size: usize = f[2].parse().unwrap();
            let want: usize = f[3].parse().unwrap();
            assert_eq!(
                is_indented(&data, size),
                want,
                "is_indented({:?}, {size})",
                String::from_utf8_lossy(&data)
            );
            checked += 1;
        }
        assert!(checked >= 15, "expected a real corpus, got {checked}");
    }

    #[test]
    fn punctuation_set_is_exactly_ascii_punct() {
        // Go builds the set from a literal string; this pins the same members
        // without depending on the fixture, and confirms nothing outside ASCII
        // is punctuation.
        let expected = br##"!"#$%&'()*+,-./:;<=>?@[\]^_`{|}~"##;
        for c in 0u8..=255 {
            assert_eq!(is_punct(c), expected.contains(&c), "byte {c:#04x}");
        }
    }

    #[test]
    fn vertical_space_includes_form_feed_and_vertical_tab() {
        assert!(is_vertical_space(b'\n'));
        assert!(is_vertical_space(b'\r'));
        assert!(is_vertical_space(0x0c)); // \f
        assert!(is_vertical_space(0x0b)); // \v
        assert!(!is_vertical_space(b' '));
        assert!(!is_vertical_space(b'\t'));
        // \v and \f are whitespace here but not horizontal whitespace.
        assert!(is_space(0x0b) && !is_horizontal_space(0x0b));
    }

    #[test]
    fn letters_and_alnum_are_ascii_only() {
        assert!(is_letter(b'a') && is_letter(b'Z'));
        assert!(!is_letter(b'0'));
        assert!(is_alnum(b'0') && is_alnum(b'z'));
        // High bytes are never letters, even though they may be part of a
        // multi-byte UTF-8 letter.
        for c in 0x80u8..=0xFF {
            assert!(!is_letter(c), "byte {c:#04x} must not be a letter");
            assert!(!is_alnum(c), "byte {c:#04x} must not be alnum");
        }
    }

    #[test]
    fn slugify_preserves_case_unlike_anchor_names() {
        // The two functions are easy to confuse; this pins the difference.
        assert_eq!(slugify(b"Hello, World!"), b"Hello-World");
    }

    #[test]
    fn slugify_all_punctuation_yields_a_single_dash_not_empty() {
        // The b > 0 guard in Go's trailing trim means index 0 is never removed.
        assert_eq!(slugify(b"!"), b"-");
        assert_eq!(slugify(b"!!!"), b"-");
        assert_eq!(slugify(b"-"), b"-");
        assert_eq!(slugify(b"   "), b"-");
    }

    #[test]
    fn slugify_never_emits_adjacent_dashes() {
        for input in [
            &b"a  b"[..],
            b"!!!a!!!b!!!",
            b"...",
            b"a - b - c",
            b"\xff\xfe\xfda\xff",
        ] {
            let out = slugify(input);
            assert!(
                !out.windows(2).any(|w| w == b"--"),
                "slugify({:?}) produced adjacent dashes: {:?}",
                String::from_utf8_lossy(input),
                String::from_utf8_lossy(&out)
            );
        }
    }

    #[test]
    fn slugify_of_empty_is_empty() {
        assert_eq!(slugify(b""), b"");
    }

    #[test]
    fn is_indented_treats_a_leading_tab_as_one() {
        assert_eq!(is_indented(b"\t", 4), 1);
        assert_eq!(is_indented(b"\tx", 4), 1);
        assert_eq!(is_indented(b"\t", 8), 1);
        assert_eq!(is_indented(b"    ", 4), 4);
        assert_eq!(is_indented(b"   ", 4), 0);
        assert_eq!(is_indented(b"", 4), 0);
        // A zero indent size matches trivially on any non-empty, non-tab line.
        assert_eq!(is_indented(b"abc", 0), 0);
    }
}
