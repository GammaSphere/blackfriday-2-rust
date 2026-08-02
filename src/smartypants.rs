//! Smart punctuation substitution, ported from upstream `smartypants.go`.
//!
//! [`SpRenderer`] rewrites straight quotes as curly ones, `--` as an em dash,
//! `(c)` as `&copy;` and so on. It runs over already-escaped text, one byte at
//! a time, and carries open/close quote state between calls.
//!
//! # Dispatch
//!
//! Upstream stores a `[256]smartCallback` of closures, each bound to the
//! renderer it was built from — a self-referential structure Rust will not
//! allow, since the callbacks need `&mut` access to the same state that owns
//! them. The table here holds a fieldless `SmartCallback` tag instead, and a
//! private `dispatch` maps the tag to the method. The observable result is
//! identical: the same byte values get a handler for the same flags, which
//! [`SpRenderer::new`] is tested against directly.
//!
//! # A deliberate divergence
//!
//! `smartLeftAngle` scans for `>` and then writes `text[:i+1]`, without
//! checking whether the scan ran off the end. In Go that is not a compile
//! error and not always a runtime one: slicing past `len` is legal up to
//! `cap`, so the behaviour depends on the allocator. Measured, upstream either
//! emits a stray NUL byte or panics outright, purely as a function of the
//! input's length. This port writes the text it was actually given. See
//! `BUGS.md` — the fault is reachable from `Run`, via a document title.

use crate::flags::HtmlFlags;
use crate::util::{is_punct, is_space};

/// Which handler a byte value dispatches to.
///
/// Stands in for upstream's `smartCallback` function values. `Amp` carries the
/// two parameters that `smartAmp` closes over, which is why upstream needs
/// four distinct closures where this needs one variant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SmartCallback {
    /// `"` without [`HtmlFlags::SMARTYPANTS_ANGLED_QUOTES`].
    DoubleQuote,
    /// `"` with [`HtmlFlags::SMARTYPANTS_ANGLED_QUOTES`].
    AngledDoubleQuote,
    /// `&`, which may begin an already-escaped `&quot;`.
    Amp {
        /// `d` for `&ldquo;`/`&rdquo;`, `a` for `&laquo;`/`&raquo;`.
        quote: u8,
        /// Whether to pad the quote with a non-breaking space.
        add_nbsp: bool,
    },
    /// `'`, covering both quotes and contractions.
    SingleQuote,
    /// `(`, for `(c)`, `(r)` and `(tm)`.
    Parens,
    /// `-` with [`HtmlFlags::SMARTYPANTS_DASHES`] alone.
    Dash,
    /// `-` with [`HtmlFlags::SMARTYPANTS_LATEX_DASHES`] as well.
    DashLatex,
    /// `.`, for ellipses.
    Period,
    /// `1` and `3`, for the three named fraction entities.
    Number,
    /// `1`..=`9` with [`HtmlFlags::SMARTYPANTS_FRACTIONS`].
    NumberGeneric,
    /// `<`, which passes an HTML tag through untouched.
    LeftAngle,
    /// `` ` ``, for the ``` ``quoted'' ``` TeX convention.
    Backtick,
}

/// Holds smartypants state across a document.
///
/// Ported from `SPRenderer`. Build it with [`SpRenderer::new`].
pub struct SpRenderer {
    /// Whether an opening single quote is outstanding.
    in_single_quote: bool,
    /// Whether an opening double quote is outstanding.
    in_double_quote: bool,
    /// Handler per byte value; `None` means "copy the byte through".
    callbacks: [Option<SmartCallback>; 256],
}

/// Whether `c` ends a word: NUL, whitespace or punctuation.
///
/// Ported from `wordBoundary`. NUL stands for "off the edge of the buffer",
/// which is how upstream signals a missing neighbour.
const fn word_boundary(c: u8) -> bool {
    c == 0 || is_space(c) || is_punct(c)
}

/// ASCII-only lowercase, matching upstream's `tolower`.
const fn tolower(c: u8) -> u8 {
    if c.is_ascii_uppercase() {
        c - b'A' + b'a'
    } else {
        c
    }
}

/// Decides whether a quote opens or closes, then writes the entity.
///
/// Ported from `smartQuoteHelper`. The sixteen arms enumerate every
/// combination of `previous_char` and `next_char` over the classes {NUL,
/// space, punctuation, other}, and the order matters: each arm is reached only
/// when the ones above it did not match, so the later arms carry an implied
/// "and the character is normal".
///
/// The return value is always `true`. Upstream declares it as a `bool` and
/// two callers test it, so the shape is kept — see [`SpRenderer::process`]'s
/// module documentation on reproducing rather than tidying.
fn smart_quote_helper(
    out: &mut Vec<u8>,
    previous_char: u8,
    next_char: u8,
    quote: u8,
    is_open: &mut bool,
    add_nbsp: bool,
) -> bool {
    if previous_char == 0 && next_char == 0 {
        // No context at all, so toggle.
        *is_open = !*is_open;
    } else if is_space(previous_char) && next_char == 0 {
        // [ "] might be [ "<code>foo...]
        *is_open = true;
    } else if is_punct(previous_char) && next_char == 0 {
        // [!"] could be [Run!"] or [("<code>...]
        *is_open = false;
    } else if next_char == 0 {
        // [a"] is probably a close.
        *is_open = false;
    } else if previous_char == 0 && is_space(next_char) {
        // [" ] might be [...foo</code>" ]
        *is_open = false;
    } else if is_space(previous_char) && is_space(next_char) {
        // [ " ] no help either, so toggle.
        *is_open = !*is_open;
    } else if is_punct(previous_char) && is_space(next_char) {
        // [!" ] is probably a close.
        *is_open = false;
    } else if is_space(next_char) {
        // [a" ] one of the easy cases.
        *is_open = false;
    } else if previous_char == 0 && is_punct(next_char) {
        // ["!] could be ["$1.95] or [</code>"!...]
        *is_open = false;
    } else if is_space(previous_char) && is_punct(next_char) {
        // [ "!] looks more like [ "$1.95]
        *is_open = true;
    } else if is_punct(previous_char) && is_punct(next_char) {
        // [!"!] no help either, so toggle.
        *is_open = !*is_open;
    } else if is_punct(next_char) {
        // [a"!] is probably a close.
        *is_open = false;
    } else if previous_char == 0 {
        // ["a] is probably an open.
        *is_open = true;
    } else if is_space(previous_char) {
        // [ "a] one of the easy cases.
        *is_open = true;
    } else if is_punct(previous_char) {
        // [!"a] is probably an open.
        *is_open = true;
    } else {
        // [a'b] maybe a contraction?
        *is_open = false;
    }

    // With only one byte of lookahead this space also lands after a lone
    // double quote, which upstream notes and accepts.
    if add_nbsp && !*is_open {
        out.extend_from_slice(b"&nbsp;");
    }

    out.push(b'&');
    out.push(if *is_open { b'l' } else { b'r' });
    out.push(quote);
    out.extend_from_slice(b"quo;");

    if add_nbsp && *is_open {
        out.extend_from_slice(b"&nbsp;");
    }

    true
}

impl SpRenderer {
    /// Builds a renderer with the callback table `flags` selects.
    ///
    /// Ported from `NewSmartypantsRenderer` (`smartypants.go:384`). Note that
    /// `-` gets no handler at all unless [`HtmlFlags::SMARTYPANTS_DASHES`] is
    /// set: [`HtmlFlags::SMARTYPANTS_LATEX_DASHES`] on its own does nothing.
    pub fn new(flags: HtmlFlags) -> Self {
        let mut callbacks: [Option<SmartCallback>; 256] = [None; 256];
        let add_nbsp = flags.intersects(HtmlFlags::SMARTYPANTS_QUOTES_NBSP);

        if !flags.intersects(HtmlFlags::SMARTYPANTS_ANGLED_QUOTES) {
            callbacks[b'"' as usize] = Some(SmartCallback::DoubleQuote);
            callbacks[b'&' as usize] = Some(SmartCallback::Amp {
                quote: b'd',
                add_nbsp,
            });
        } else {
            callbacks[b'"' as usize] = Some(SmartCallback::AngledDoubleQuote);
            callbacks[b'&' as usize] = Some(SmartCallback::Amp {
                quote: b'a',
                add_nbsp,
            });
        }
        callbacks[b'\'' as usize] = Some(SmartCallback::SingleQuote);
        callbacks[b'(' as usize] = Some(SmartCallback::Parens);
        if flags.intersects(HtmlFlags::SMARTYPANTS_DASHES) {
            callbacks[b'-' as usize] =
                Some(if flags.intersects(HtmlFlags::SMARTYPANTS_LATEX_DASHES) {
                    SmartCallback::DashLatex
                } else {
                    SmartCallback::Dash
                });
        }
        callbacks[b'.' as usize] = Some(SmartCallback::Period);
        if !flags.intersects(HtmlFlags::SMARTYPANTS_FRACTIONS) {
            callbacks[b'1' as usize] = Some(SmartCallback::Number);
            callbacks[b'3' as usize] = Some(SmartCallback::Number);
        } else {
            for ch in b'1'..=b'9' {
                callbacks[ch as usize] = Some(SmartCallback::NumberGeneric);
            }
        }
        callbacks[b'<' as usize] = Some(SmartCallback::LeftAngle);
        callbacks[b'`' as usize] = Some(SmartCallback::Backtick);

        SpRenderer {
            in_single_quote: false,
            in_double_quote: false,
            callbacks,
        }
    }

    /// Rewrites `text` into `out`, substituting smart punctuation.
    ///
    /// Ported from `Process` (`smartypants.go:437`). Bytes without a handler
    /// are copied in runs; a handler consumes the byte it was dispatched on
    /// plus however many more it reports.
    pub fn process(&mut self, out: &mut Vec<u8>, text: &[u8]) {
        let mut mark = 0;
        let mut i = 0;
        while i < text.len() {
            if let Some(action) = self.callbacks[text[i] as usize] {
                if i > mark {
                    out.extend_from_slice(&text[mark..i]);
                }
                let previous_char = if i > 0 { text[i - 1] } else { 0 };
                // Upstream renders into a scratch buffer and copies it over,
                // which matters only because a handler may write nothing.
                let mut tmp = Vec::new();
                i += self.dispatch(action, &mut tmp, previous_char, &text[i..]);
                out.extend_from_slice(&tmp);
                mark = i + 1;
            }
            i += 1;
        }
        if mark < text.len() {
            out.extend_from_slice(&text[mark..]);
        }
    }

    /// Routes a table entry to its handler, returning extra bytes consumed.
    fn dispatch(
        &mut self,
        action: SmartCallback,
        out: &mut Vec<u8>,
        previous_char: u8,
        text: &[u8],
    ) -> usize {
        match action {
            SmartCallback::DoubleQuote => {
                self.smart_double_quote_variant(out, previous_char, text, b'd')
            }
            SmartCallback::AngledDoubleQuote => {
                self.smart_double_quote_variant(out, previous_char, text, b'a')
            }
            SmartCallback::Amp { quote, add_nbsp } => {
                self.smart_amp_variant(out, previous_char, text, quote, add_nbsp)
            }
            SmartCallback::SingleQuote => self.smart_single_quote(out, previous_char, text),
            SmartCallback::Parens => Self::smart_parens(out, text),
            SmartCallback::Dash => Self::smart_dash(out, previous_char, text),
            SmartCallback::DashLatex => Self::smart_dash_latex(out, text),
            SmartCallback::Period => Self::smart_period(out, text),
            SmartCallback::Number => Self::smart_number(out, previous_char, text),
            SmartCallback::NumberGeneric => Self::smart_number_generic(out, previous_char, text),
            SmartCallback::LeftAngle => Self::smart_left_angle(out, text),
            SmartCallback::Backtick => self.smart_backtick(out, previous_char, text),
        }
    }

    /// Handles `'`: contractions first, then an opening or closing quote.
    ///
    /// Ported from `smartSingleQuote` (`smartypants.go:123`).
    fn smart_single_quote(&mut self, out: &mut Vec<u8>, previous_char: u8, text: &[u8]) -> usize {
        if text.len() >= 2 {
            let t1 = tolower(text[1]);

            if t1 == b'\'' {
                let next_char = if text.len() >= 3 { text[2] } else { 0 };
                if smart_quote_helper(
                    out,
                    previous_char,
                    next_char,
                    b'd',
                    &mut self.in_double_quote,
                    false,
                ) {
                    return 1;
                }
            }

            // 's 't 'm 'd -- the one-letter contractions.
            if matches!(t1, b's' | b't' | b'm' | b'd') && (text.len() < 3 || word_boundary(text[2]))
            {
                out.extend_from_slice(b"&rsquo;");
                return 0;
            }

            if text.len() >= 3 {
                let t2 = tolower(text[2]);

                // 're 'll 've -- the two-letter ones.
                if ((t1 == b'r' && t2 == b'e')
                    || (t1 == b'l' && t2 == b'l')
                    || (t1 == b'v' && t2 == b'e'))
                    && (text.len() < 4 || word_boundary(text[3]))
                {
                    out.extend_from_slice(b"&rsquo;");
                    return 0;
                }
            }
        }

        let next_char = if text.len() > 1 { text[1] } else { 0 };
        if smart_quote_helper(
            out,
            previous_char,
            next_char,
            b's',
            &mut self.in_single_quote,
            false,
        ) {
            return 0;
        }

        // Unreachable: the helper is constant. Kept because upstream has it.
        out.push(text[0]);
        0
    }

    /// Handles `(`: `(c)`, `(r)` and `(tm)`.
    ///
    /// Ported from `smartParens` (`smartypants.go:164`).
    fn smart_parens(out: &mut Vec<u8>, text: &[u8]) -> usize {
        if text.len() >= 3 {
            let t1 = tolower(text[1]);
            let t2 = tolower(text[2]);

            if t1 == b'c' && t2 == b')' {
                out.extend_from_slice(b"&copy;");
                return 2;
            }

            if t1 == b'r' && t2 == b')' {
                out.extend_from_slice(b"&reg;");
                return 2;
            }

            if text.len() >= 4 && t1 == b't' && t2 == b'm' && text[3] == b')' {
                out.extend_from_slice(b"&trade;");
                return 3;
            }
        }

        out.push(text[0]);
        0
    }

    /// Handles `-`: `--` is an em dash, a spaced `-` is an en dash.
    ///
    /// Ported from `smartDash` (`smartypants.go:189`).
    fn smart_dash(out: &mut Vec<u8>, previous_char: u8, text: &[u8]) -> usize {
        if text.len() >= 2 {
            if text[1] == b'-' {
                out.extend_from_slice(b"&mdash;");
                return 1;
            }

            if word_boundary(previous_char) && word_boundary(text[1]) {
                out.extend_from_slice(b"&ndash;");
                return 0;
            }
        }

        out.push(text[0]);
        0
    }

    /// Handles `-` the TeX way: `---` is an em dash, `--` an en dash.
    ///
    /// Ported from `smartDashLatex` (`smartypants.go:203`).
    fn smart_dash_latex(out: &mut Vec<u8>, text: &[u8]) -> usize {
        if text.len() >= 3 && text[1] == b'-' && text[2] == b'-' {
            out.extend_from_slice(b"&mdash;");
            return 2;
        }
        if text.len() >= 2 && text[1] == b'-' {
            out.extend_from_slice(b"&ndash;");
            return 1;
        }

        out.push(text[0]);
        0
    }

    /// Handles `&`, which by this point may be an escaped `&quot;`.
    ///
    /// Ported from `smartAmpVariant` (`smartypants.go:217`). The `&#0;` arm
    /// swallows the sequence without writing anything.
    fn smart_amp_variant(
        &mut self,
        out: &mut Vec<u8>,
        previous_char: u8,
        text: &[u8],
        quote: u8,
        add_nbsp: bool,
    ) -> usize {
        if text.starts_with(b"&quot;") {
            let next_char = if text.len() >= 7 { text[6] } else { 0 };
            if smart_quote_helper(
                out,
                previous_char,
                next_char,
                quote,
                &mut self.in_double_quote,
                add_nbsp,
            ) {
                return 5;
            }
        }

        if text.starts_with(b"&#0;") {
            return 3;
        }

        out.push(b'&');
        0
    }

    /// Handles `.`, for `...` and `. . .`.
    ///
    /// Ported from `smartPeriod` (`smartypants.go:246`).
    fn smart_period(out: &mut Vec<u8>, text: &[u8]) -> usize {
        if text.len() >= 3 && text[1] == b'.' && text[2] == b'.' {
            out.extend_from_slice(b"&hellip;");
            return 2;
        }

        if text.len() >= 5
            && text[1] == b' '
            && text[2] == b'.'
            && text[3] == b' '
            && text[4] == b'.'
        {
            out.extend_from_slice(b"&hellip;");
            return 4;
        }

        out.push(text[0]);
        0
    }

    /// Handles `` ` ``, so ``` ``a'' ``` becomes a double quote pair.
    ///
    /// Ported from `smartBacktick` (`smartypants.go:260`).
    fn smart_backtick(&mut self, out: &mut Vec<u8>, previous_char: u8, text: &[u8]) -> usize {
        if text.len() >= 2 && text[1] == b'`' {
            let next_char = if text.len() >= 3 { text[2] } else { 0 };
            if smart_quote_helper(
                out,
                previous_char,
                next_char,
                b'd',
                &mut self.in_double_quote,
                false,
            ) {
                return 1;
            }
        }

        out.push(text[0]);
        0
    }

    /// Handles any digit as a possible fraction numerator.
    ///
    /// Ported from `smartNumberGeneric` (`smartypants.go:274`). The
    /// denominator may be separated by `/` or by U+2044 FRACTION SLASH; a
    /// trailing `/` rejects the match, which is what keeps `1/23/2005` a date.
    fn smart_number_generic(out: &mut Vec<u8>, previous_char: u8, text: &[u8]) -> usize {
        if word_boundary(previous_char) && previous_char != b'/' && text.len() >= 3 {
            let mut num_end = 0;
            while text.len() > num_end && text[num_end].is_ascii_digit() {
                num_end += 1;
            }
            if num_end == 0 {
                out.push(text[0]);
                return 0;
            }

            let mut den_start = num_end + 1;
            if text.len() > num_end + 3
                && text[num_end] == 0xe2
                && text[num_end + 1] == 0x81
                && text[num_end + 2] == 0x84
            {
                den_start = num_end + 3;
            } else if text.len() < num_end + 2 || text[num_end] != b'/' {
                out.push(text[0]);
                return 0;
            }

            let mut den_end = den_start;
            while text.len() > den_end && text[den_end].is_ascii_digit() {
                den_end += 1;
            }
            if den_end == den_start {
                out.push(text[0]);
                return 0;
            }

            if text.len() == den_end || (word_boundary(text[den_end]) && text[den_end] != b'/') {
                out.extend_from_slice(b"<sup>");
                out.extend_from_slice(&text[..num_end]);
                out.extend_from_slice(b"</sup>&frasl;<sub>");
                out.extend_from_slice(&text[den_start..den_end]);
                out.extend_from_slice(b"</sub>");
                return den_end - 1;
            }
        }

        out.push(text[0]);
        0
    }

    /// Handles `1` and `3`, for the three fraction entities HTML names.
    ///
    /// Ported from `smartNumber` (`smartypants.go:324`). `1/4th` and `3/4ths`
    /// are accepted as ordinals; `1/2th` is not, which is upstream's asymmetry
    /// rather than an oversight here.
    fn smart_number(out: &mut Vec<u8>, previous_char: u8, text: &[u8]) -> usize {
        if word_boundary(previous_char) && previous_char != b'/' && text.len() >= 3 {
            // Upstream nests these as `if prefix { if suffix { ... } }`; there
            // is no `else` on either level, so collapsing is faithful.
            if text[0] == b'1'
                && text[1] == b'/'
                && text[2] == b'2'
                && (text.len() < 4 || (word_boundary(text[3]) && text[3] != b'/'))
            {
                out.extend_from_slice(b"&frac12;");
                return 2;
            }

            if text[0] == b'1'
                && text[1] == b'/'
                && text[2] == b'4'
                && (text.len() < 4
                    || (word_boundary(text[3]) && text[3] != b'/')
                    || (text.len() >= 5 && tolower(text[3]) == b't' && tolower(text[4]) == b'h'))
            {
                out.extend_from_slice(b"&frac14;");
                return 2;
            }

            if text[0] == b'3'
                && text[1] == b'/'
                && text[2] == b'4'
                && (text.len() < 4
                    || (word_boundary(text[3]) && text[3] != b'/')
                    || (text.len() >= 6
                        && tolower(text[3]) == b't'
                        && tolower(text[4]) == b'h'
                        && tolower(text[5]) == b's'))
            {
                out.extend_from_slice(b"&frac34;");
                return 2;
            }
        }

        out.push(text[0]);
        0
    }

    /// Decides between `&ldquo;`/`&rdquo;` and `&laquo;`/`&raquo;`.
    ///
    /// Ported from `smartDoubleQuoteVariant` (`smartypants.go:351`). The
    /// `&quot;` fallback is unreachable, the helper being constant.
    fn smart_double_quote_variant(
        &mut self,
        out: &mut Vec<u8>,
        previous_char: u8,
        text: &[u8],
        quote: u8,
    ) -> usize {
        let next_char = if text.len() > 1 { text[1] } else { 0 };
        if !smart_quote_helper(
            out,
            previous_char,
            next_char,
            quote,
            &mut self.in_double_quote,
            false,
        ) {
            out.extend_from_slice(b"&quot;");
        }

        0
    }

    /// Copies an HTML tag through untouched, so its attributes survive.
    ///
    /// Ported from `smartLeftAngle` (`smartypants.go:369`), with one change:
    /// upstream writes `text[:i+1]` even when the scan found no `>` and `i`
    /// has reached the end. This writes what is there. See the module
    /// documentation and `BUGS.md`.
    fn smart_left_angle(out: &mut Vec<u8>, text: &[u8]) -> usize {
        let mut i = 0;

        while i < text.len() && text[i] != b'>' {
            i += 1;
        }

        out.extend_from_slice(&text[..text.len().min(i + 1)]);
        i
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/go-smartypants.txt");

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
    fn callback_table_matches_go_for_every_flag_combination() {
        let mut n = 0;
        for f in rows("T") {
            let flags = HtmlFlags::from_bits_retain(f[1].parse().unwrap());
            let want = unhex(field(&f, 2));
            let r = SpRenderer::new(flags);
            let got: Vec<u8> = (0..256)
                .filter(|&i| r.callbacks[i].is_some())
                .map(|i| i as u8)
                .collect();
            assert_eq!(got, want, "installed callbacks for flags {}", f[1]);
            n += 1;
        }
        assert!(n >= 9, "thin corpus: {n}");
    }

    #[test]
    fn latex_dashes_alone_installs_no_dash_handler() {
        // Easy to get wrong: the LaTeX flag only picks *which* dash handler,
        // it does not enable one.
        let r = SpRenderer::new(HtmlFlags::SMARTYPANTS_LATEX_DASHES);
        assert!(r.callbacks[b'-' as usize].is_none());
        let r = SpRenderer::new(HtmlFlags::SMARTYPANTS_DASHES);
        assert_eq!(r.callbacks[b'-' as usize], Some(SmartCallback::Dash));
        let r =
            SpRenderer::new(HtmlFlags::SMARTYPANTS_DASHES | HtmlFlags::SMARTYPANTS_LATEX_DASHES);
        assert_eq!(r.callbacks[b'-' as usize], Some(SmartCallback::DashLatex));
    }

    #[test]
    fn process_matches_go() {
        let mut n = 0;
        let mut skipped = 0;
        for f in rows("P") {
            let flags = HtmlFlags::from_bits_retain(f[1].parse().unwrap());
            let input = unhex(field(&f, 2));
            let want = unhex(field(&f, 3));
            let want_single = field(&f, 4) == "true";
            let want_double = field(&f, 5) == "true";

            let mut r = SpRenderer::new(flags);
            let mut got = Vec::new();
            r.process(&mut got, &input);

            // The rows where Go read past the slice are handled separately.
            if input.contains(&b'<') && !input.contains(&b'>') {
                skipped += 1;
                continue;
            }

            assert_eq!(
                got,
                want,
                "process({:?}) with flags {}",
                String::from_utf8_lossy(&input),
                f[1]
            );
            assert_eq!(r.in_single_quote, want_single, "in_single_quote");
            assert_eq!(r.in_double_quote, want_double, "in_double_quote");
            n += 1;
        }
        assert!(n >= 900, "thin corpus: {n}");
        assert!(skipped > 0, "the out-of-range rows should still be present");
    }

    #[test]
    fn smart_quote_helper_matches_go() {
        let mut n = 0;
        for f in rows("H") {
            let quote = f[1].as_bytes()[0];
            let add_nbsp = f[2] == "true";
            let prev: u8 = f[3].parse().unwrap();
            let next: u8 = f[4].parse().unwrap();
            let start = f[5] == "true";
            let want = unhex(field(&f, 6));
            let want_open = field(&f, 7) == "true";

            let mut is_open = start;
            let mut got = Vec::new();
            let ret = smart_quote_helper(&mut got, prev, next, quote, &mut is_open, add_nbsp);

            assert_eq!(got, want, "helper({prev}, {next}, {quote}, {start})");
            assert_eq!(is_open, want_open, "is_open after ({prev}, {next})");
            assert!(ret, "upstream's return value is constant");
            n += 1;
        }
        assert!(n >= 500, "thin corpus: {n}");
    }

    #[test]
    fn quote_state_persists_across_process_calls() {
        let mut n = 0;
        let sequences: [&[&str]; 5] = [
            &[r#""a"#, r#"b""#],
            &[r#"""#, r#"""#, r#"""#],
            &["'", "'", "'"],
            &[r#""a""#, r#""b""#],
            &["&quot;", "&quot;"],
        ];
        for f in rows("S") {
            let idx: usize = f[1].parse().unwrap();
            let want = unhex(field(&f, 2));
            let want_single = field(&f, 3) == "true";
            let want_double = field(&f, 4) == "true";

            let mut r = SpRenderer::new(HtmlFlags::from_bits_retain(0));
            let mut got = Vec::new();
            for part in sequences[idx] {
                r.process(&mut got, part.as_bytes());
            }
            assert_eq!(got, want, "sequence {idx}");
            assert_eq!(r.in_single_quote, want_single, "sequence {idx} single");
            assert_eq!(r.in_double_quote, want_double, "sequence {idx} double");
            n += 1;
        }
        assert_eq!(n, 5);
    }

    #[test]
    fn unterminated_tag_writes_only_what_it_was_given() {
        // The declared divergence. Upstream emits "<b\0" here when the input
        // slice happened to have spare capacity, and panics when it did not.
        let mut r = SpRenderer::new(HtmlFlags::from_bits_retain(0));
        let mut got = Vec::new();
        r.process(&mut got, b"<b");
        assert_eq!(got, b"<b");

        let mut got = Vec::new();
        r.process(&mut got, b"a<b");
        assert_eq!(got, b"a<b");

        // A terminated tag is unaffected, and its contents are not touched
        // even though they contain characters with handlers.
        let mut got = Vec::new();
        r.process(&mut got, b"<a title=\"x--y\">");
        assert_eq!(got, b"<a title=\"x--y\">");
    }

    #[test]
    fn fraction_slash_is_accepted_as_a_separator() {
        let mut r = SpRenderer::new(HtmlFlags::SMARTYPANTS_FRACTIONS);
        let mut got = Vec::new();
        // U+2044 FRACTION SLASH, the three-byte path through smartNumberGeneric.
        r.process(&mut got, "5\u{2044}8".as_bytes());
        assert_eq!(got, b"<sup>5</sup>&frasl;<sub>8</sub>");
    }

    #[test]
    fn dates_are_not_turned_into_fractions() {
        let mut r = SpRenderer::new(HtmlFlags::SMARTYPANTS_FRACTIONS);
        let mut got = Vec::new();
        r.process(&mut got, b"1/23/2005");
        assert_eq!(got, b"1/23/2005", "the trailing slash rejects the match");
    }
}
