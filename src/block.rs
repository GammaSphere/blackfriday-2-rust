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
use crate::node::{NodeId, NodeType};
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
